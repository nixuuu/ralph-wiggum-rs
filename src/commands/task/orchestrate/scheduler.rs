use std::collections::{HashMap, HashSet, VecDeque};

use crate::shared::dag::TaskDag;
use crate::shared::progress::{ProgressSummary, TaskStatus};
use crate::shared::tasks::TasksFile;

/// FIFO task scheduler with DAG dependency awareness.
///
/// Manages task states (pending, in-progress, done, blocked) and
/// provides ready tasks based on dependency completion.
pub struct TaskScheduler {
    dag: TaskDag,
    done: HashSet<String>,
    in_progress: HashSet<String>,
    blocked: HashSet<String>,
    failed_retries: HashMap<String, u32>,
    max_retries: u32,
    ready_queue: VecDeque<String>,
}

impl TaskScheduler {
    /// Create a scheduler from a DAG and current progress state.
    ///
    /// Pre-populates done/blocked sets from progress, then computes
    /// the initial ready queue.
    pub fn new(mut dag: TaskDag, progress: &ProgressSummary, max_retries: u32) -> Self {
        let mut done = HashSet::new();
        let mut blocked = HashSet::new();

        // Register all tasks from progress in the DAG
        dag.register_tasks(progress.tasks.iter().map(|t| t.id.clone()));

        // Initialize sets from current progress state
        for task in &progress.tasks {
            match task.status {
                TaskStatus::Done => {
                    done.insert(task.id.clone());
                }
                TaskStatus::Blocked => {
                    blocked.insert(task.id.clone());
                }
                _ => {}
            }
        }

        let in_progress = HashSet::new();
        let ready_queue = VecDeque::new();

        let mut scheduler = Self {
            dag,
            done,
            in_progress,
            blocked,
            failed_retries: HashMap::new(),
            max_retries,
            ready_queue,
        };
        scheduler.refresh_ready_queue();
        scheduler
    }

    /// Get the next ready task from the queue.
    /// Returns None if no tasks are ready.
    /// Skips tasks that are in the blocked set.
    pub fn next_ready_task(&mut self) -> Option<String> {
        while let Some(task) = self.ready_queue.pop_front() {
            if !self.blocked.contains(&task) {
                crate::diag_debug!("Scheduler: assigning task {} from ready queue", task);
                return Some(task);
            }
            crate::diag_debug!("Scheduler: skipping blocked task {} in ready queue", task);
        }
        None
    }

    /// Mark a task as started (in-progress).
    pub fn mark_started(&mut self, task_id: &str) {
        crate::diag_debug!("Scheduler: task {} marked as started", task_id);
        self.in_progress.insert(task_id.to_string());
    }

    /// Mark a task as successfully completed.
    /// Refreshes the ready queue to unlock dependent tasks.
    pub fn mark_done(&mut self, task_id: &str) {
        crate::diag_debug!(
            "Scheduler: task {} marked as done, refreshing queue",
            task_id
        );
        self.in_progress.remove(task_id);
        self.done.insert(task_id.to_string());
        self.refresh_ready_queue();
    }

    /// Mark a task as permanently blocked.
    pub fn mark_blocked(&mut self, task_id: &str) {
        crate::diag_debug!("Scheduler: task {} marked as blocked", task_id);
        self.in_progress.remove(task_id);
        self.blocked.insert(task_id.to_string());
    }

    /// Re-queue a task without incrementing the retry counter.
    ///
    /// Used for manual worker restart: the task is moved from in-progress
    /// back to the front of the ready queue so it gets picked up immediately
    /// by the next idle worker.
    pub fn requeue_without_retry(&mut self, task_id: &str) {
        self.in_progress.remove(task_id);
        self.ready_queue.push_front(task_id.to_string());
    }

    /// Handle a task failure: increment retry count, re-queue or block.
    ///
    /// Returns `true` if the task was re-queued, `false` if blocked.
    pub fn mark_failed(&mut self, task_id: &str) -> bool {
        self.in_progress.remove(task_id);

        let retries = self.failed_retries.entry(task_id.to_string()).or_insert(0);
        *retries += 1;

        if *retries >= self.max_retries {
            crate::diag_debug!(
                "Scheduler: task {} failed {} times, marking as blocked",
                task_id,
                retries
            );
            self.blocked.insert(task_id.to_string());
            false
        } else {
            crate::diag_debug!(
                "Scheduler: task {} failed (retry {}/{}), re-queueing",
                task_id,
                retries,
                self.max_retries
            );
            // Re-add to ready queue for retry
            self.ready_queue.push_back(task_id.to_string());
            true
        }
    }

    /// Check if all tasks are either done or blocked.
    pub fn is_complete(&self) -> bool {
        let all_tasks = self.dag.tasks();
        all_tasks
            .iter()
            .all(|t| self.done.contains(t) || self.blocked.contains(t))
    }

    /// Re-compute the ready queue from DAG state.
    pub fn refresh_ready_queue(&mut self) {
        let ready = self.dag.ready_tasks(&self.done, &self.in_progress);
        self.ready_queue.clear();
        for task in ready {
            if !self.blocked.contains(&task) {
                self.ready_queue.push_back(task);
            }
        }
    }

    /// Add new tasks to the DAG (for hot reload support).
    ///
    /// Preserves tasks known to the scheduler (done/in_progress/blocked)
    /// that may not appear in the freshly-loaded DAG.
    pub fn add_tasks(&mut self, mut new_dag: TaskDag) {
        // Re-register tasks the scheduler already knows about so they
        // survive a DAG rebuild from tasks.yml (hot reload).
        let known: Vec<String> = self
            .done
            .iter()
            .chain(self.in_progress.iter())
            .chain(self.blocked.iter())
            .cloned()
            .collect();
        new_dag.register_tasks(known);

        self.dag = new_dag;
        self.refresh_ready_queue();
    }

    /// Synchronize the blocked set with actual statuses from `tasks.yml`.
    ///
    /// After a hot-reload, the user may have changed a task's status from
    /// `blocked` to `todo` in the file. This method detects such changes
    /// and removes those tasks from the internal `blocked` set (and clears
    /// their failed retry counters) so they re-enter the ready queue.
    ///
    /// Returns the list of task IDs that were unblocked.
    pub fn sync_blocked_with_file(&mut self, tasks_file: &TasksFile) -> Vec<String> {
        let leaves = tasks_file.flatten_leaves();
        let file_statuses: HashMap<&str, &TaskStatus> =
            leaves.iter().map(|l| (l.id.as_str(), &l.status)).collect();

        // Collect tasks to unblock: in scheduler.blocked but `todo` in the file
        let to_unblock: Vec<String> = self
            .blocked
            .iter()
            .filter(|id| {
                file_statuses
                    .get(id.as_str())
                    .is_some_and(|s| **s == TaskStatus::Todo)
            })
            .cloned()
            .collect();

        for id in &to_unblock {
            self.blocked.remove(id);
            self.failed_retries.remove(id);
        }

        if !to_unblock.is_empty() {
            self.refresh_ready_queue();
        }

        to_unblock
    }

    /// Get current counts for status display.
    pub fn status(&self) -> SchedulerStatus {
        SchedulerStatus {
            total: self.dag.tasks().len(),
            done: self.done.len(),
            in_progress: self.in_progress.len(),
            blocked: self.blocked.len(),
            ready: self.ready_queue.len(),
            pending: self
                .dag
                .tasks()
                .len()
                .saturating_sub(self.done.len())
                .saturating_sub(self.in_progress.len())
                .saturating_sub(self.blocked.len()),
        }
    }

    /// Get retry count for a specific task.
    pub fn retry_count(&self, task_id: &str) -> u32 {
        self.failed_retries.get(task_id).copied().unwrap_or(0)
    }

    pub fn done_tasks(&self) -> &HashSet<String> {
        &self.done
    }

    pub fn blocked_tasks(&self) -> &HashSet<String> {
        &self.blocked
    }

    pub fn in_progress_tasks(&self) -> &HashSet<String> {
        &self.in_progress
    }
}

/// Snapshot of scheduler state for status display.
#[derive(Debug, Clone)]
pub struct SchedulerStatus {
    pub total: usize,
    pub done: usize,
    pub in_progress: usize,
    pub blocked: usize,
    pub ready: usize,
    pub pending: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::progress::{ProgressFrontmatter, ProgressTask};

    fn make_progress(tasks: Vec<(&str, &str, &str, TaskStatus)>) -> ProgressSummary {
        let mut done = 0;
        let mut in_progress = 0;
        let mut blocked = 0;
        let mut todo = 0;
        let task_vec: Vec<ProgressTask> = tasks
            .into_iter()
            .map(|(id, comp, name, status)| {
                match &status {
                    TaskStatus::Done => done += 1,
                    TaskStatus::InProgress => in_progress += 1,
                    TaskStatus::Blocked => blocked += 1,
                    TaskStatus::Todo => todo += 1,
                }
                ProgressTask {
                    id: id.to_string(),
                    component: comp.to_string(),
                    name: name.to_string(),
                    status,
                }
            })
            .collect();

        ProgressSummary {
            tasks: task_vec,
            done,
            in_progress,
            blocked,
            todo,
            frontmatter: None,
        }
    }

    fn make_dag(deps: Vec<(&str, Vec<&str>)>) -> TaskDag {
        let mut fm = ProgressFrontmatter::default();
        for (task, task_deps) in deps {
            fm.deps.insert(
                task.to_string(),
                task_deps.into_iter().map(|s| s.to_string()).collect(),
            );
        }
        TaskDag::from_frontmatter(&fm)
    }

    #[test]
    fn test_scheduler_fifo_order() {
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![]), ("T03", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Todo),
            ("T02", "api", "Second", TaskStatus::Todo),
            ("T03", "api", "Third", TaskStatus::Todo),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);
        // All are ready, should come in ID order
        assert_eq!(sched.next_ready_task(), Some("T01".to_string()));
        assert_eq!(sched.next_ready_task(), Some("T02".to_string()));
        assert_eq!(sched.next_ready_task(), Some("T03".to_string()));
        assert_eq!(sched.next_ready_task(), None);
    }

    #[test]
    fn test_scheduler_dependency_respect() {
        let dag = make_dag(vec![
            ("T01", vec![]),
            ("T02", vec!["T01"]),
            ("T03", vec!["T02"]),
        ]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Todo),
            ("T02", "api", "Second", TaskStatus::Todo),
            ("T03", "api", "Third", TaskStatus::Todo),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);

        // Only T01 is ready
        assert_eq!(sched.next_ready_task(), Some("T01".to_string()));
        assert_eq!(sched.next_ready_task(), None);

        // Mark T01 as started then done
        sched.mark_started("T01");
        sched.mark_done("T01");

        // Now T02 is ready
        assert_eq!(sched.next_ready_task(), Some("T02".to_string()));
    }

    #[test]
    fn test_scheduler_skip_done_tasks() {
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec!["T01"])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Done),
            ("T02", "api", "Second", TaskStatus::Todo),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);
        // T01 is already done, T02 should be ready
        assert_eq!(sched.next_ready_task(), Some("T02".to_string()));
    }

    #[test]
    fn test_scheduler_blocked_propagation() {
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec!["T01"])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Todo),
            ("T02", "api", "Second", TaskStatus::Todo),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);
        let task = sched.next_ready_task().unwrap();
        sched.mark_started(&task);
        sched.mark_blocked(&task);

        // T01 blocked → T02 never becomes ready
        assert_eq!(sched.next_ready_task(), None);
        assert!(!sched.is_complete()); // T02 is neither done nor blocked
    }

    #[test]
    fn test_scheduler_retry_to_blocked() {
        let dag = make_dag(vec![("T01", vec![])]);
        let progress = make_progress(vec![("T01", "api", "First", TaskStatus::Todo)]);

        let mut sched = TaskScheduler::new(dag, &progress, 2); // max 2 retries

        // First attempt
        let task = sched.next_ready_task().unwrap();
        sched.mark_started(&task);
        assert!(sched.mark_failed(&task)); // re-queued (retry 1)
        assert_eq!(sched.retry_count("T01"), 1);

        // Second attempt
        let task = sched.next_ready_task().unwrap();
        sched.mark_started(&task);
        assert!(!sched.mark_failed(&task)); // blocked (retry 2 = max)
        assert_eq!(sched.retry_count("T01"), 2);
        assert!(sched.blocked.contains("T01"));
    }

    #[test]
    fn test_scheduler_is_complete() {
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Done),
            ("T02", "api", "Second", TaskStatus::Done),
        ]);

        let sched = TaskScheduler::new(dag, &progress, 3);
        assert!(sched.is_complete());
    }

    #[test]
    fn test_scheduler_status() {
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec!["T01"]), ("T03", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Todo),
            ("T02", "api", "Second", TaskStatus::Todo),
            ("T03", "api", "Third", TaskStatus::Todo),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);
        sched.mark_started("T01"); // take from ready queue manually
        let _ = sched.next_ready_task(); // T01 (already started)
        let _ = sched.next_ready_task(); // T03

        let status = sched.status();
        assert_eq!(status.total, 3);
        assert_eq!(status.in_progress, 1);
    }

    #[test]
    fn test_scheduler_diamond_dag() {
        let dag = make_dag(vec![
            ("T01", vec![]),
            ("T02", vec!["T01"]),
            ("T03", vec!["T01"]),
            ("T04", vec!["T02", "T03"]),
        ]);
        let progress = make_progress(vec![
            ("T01", "api", "Root", TaskStatus::Todo),
            ("T02", "api", "Left", TaskStatus::Todo),
            ("T03", "api", "Right", TaskStatus::Todo),
            ("T04", "api", "Join", TaskStatus::Todo),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);

        // Only T01 ready initially
        assert_eq!(sched.next_ready_task(), Some("T01".to_string()));
        assert_eq!(sched.next_ready_task(), None);

        sched.mark_started("T01");
        sched.mark_done("T01");

        // T02 and T03 now ready
        let t1 = sched.next_ready_task().unwrap();
        let t2 = sched.next_ready_task().unwrap();
        assert!(
            (t1 == "T02" && t2 == "T03") || (t1 == "T03" && t2 == "T02"),
            "Expected T02 and T03 in some order, got {t1} and {t2}"
        );

        // T04 not ready yet
        assert_eq!(sched.next_ready_task(), None);

        sched.mark_started(&t1);
        sched.mark_done(&t1);
        sched.mark_started(&t2);
        sched.mark_done(&t2);

        // Now T04 is ready
        assert_eq!(sched.next_ready_task(), Some("T04".to_string()));
    }

    #[test]
    fn test_add_tasks_resets_complete() {
        // Setup: Scheduler with all tasks done
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Done),
            ("T02", "api", "Second", TaskStatus::Done),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);
        assert!(
            sched.is_complete(),
            "Scheduler should be complete initially"
        );

        // Add new tasks including a todo task
        let new_dag = make_dag(vec![
            ("T01", vec![]),
            ("T02", vec![]),
            ("T03", vec![]), // New task
        ]);
        sched.add_tasks(new_dag);

        // is_complete should now return false because T03 is neither done nor blocked
        assert!(
            !sched.is_complete(),
            "Scheduler should not be complete after adding new todo tasks"
        );

        // Verify T03 is in the ready queue
        assert_eq!(sched.next_ready_task(), Some("T03".to_string()));
    }

    #[test]
    fn test_requeue_without_retry_moves_to_front() {
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![]), ("T03", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Todo),
            ("T02", "api", "Second", TaskStatus::Todo),
            ("T03", "api", "Third", TaskStatus::Todo),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);

        // Take all tasks
        let t1 = sched.next_ready_task().unwrap();
        let t2 = sched.next_ready_task().unwrap();
        let t3 = sched.next_ready_task().unwrap();
        sched.mark_started(&t1);
        sched.mark_started(&t2);
        sched.mark_started(&t3);

        // Requeue T02 without retry
        sched.requeue_without_retry("T02");

        // T02 should be at the front of the ready queue
        assert_eq!(sched.next_ready_task(), Some("T02".to_string()));

        // Retry count should remain 0
        assert_eq!(sched.retry_count("T02"), 0);

        // T02 should no longer be in in_progress
        assert!(!sched.in_progress_tasks().contains(&"T02".to_string()));
    }

    #[test]
    fn test_requeue_without_retry_preserves_retry_count() {
        let dag = make_dag(vec![("T01", vec![])]);
        let progress = make_progress(vec![("T01", "api", "First", TaskStatus::Todo)]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);

        // First attempt: fail once to bump retry count
        let task = sched.next_ready_task().unwrap();
        sched.mark_started(&task);
        assert!(sched.mark_failed(&task)); // retry 1
        assert_eq!(sched.retry_count("T01"), 1);

        // Second attempt: start and requeue
        let task = sched.next_ready_task().unwrap();
        sched.mark_started(&task);
        sched.requeue_without_retry("T01");

        // Retry count should still be 1 (not incremented by requeue)
        assert_eq!(sched.retry_count("T01"), 1);

        // Task should be back at front of queue
        assert_eq!(sched.next_ready_task(), Some("T01".to_string()));
    }

    #[test]
    fn test_requeue_without_retry_front_ordering() {
        // Verify task is pushed to FRONT, not back
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Todo),
            ("T02", "api", "Second", TaskStatus::Todo),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);

        // Take T01, leave T02 in queue
        let _t1 = sched.next_ready_task().unwrap(); // T01
        sched.mark_started("T01");

        // Now T02 is still in ready queue. Requeue T01 — it should come BEFORE T02.
        sched.requeue_without_retry("T01");

        assert_eq!(sched.next_ready_task(), Some("T01".to_string()));
        assert_eq!(sched.next_ready_task(), Some("T02".to_string()));
    }

    // ── Task 38.1: sync_blocked_with_file tests ─────────────────────────

    /// Helper: build a minimal TasksFile from (id, status) pairs.
    fn make_tasks_file(tasks: Vec<(&str, TaskStatus)>) -> TasksFile {
        use crate::shared::tasks::{TaskNode, TasksFile};

        let nodes: Vec<TaskNode> = tasks
            .into_iter()
            .map(|(id, status)| TaskNode {
                id: id.to_string(),
                name: format!("Task {id}"),
                component: Some("api".to_string()),
                status: Some(status),
                deps: vec![],
                model: None,
                description: None,
                related_files: vec![],
                implementation_steps: vec![],
                acceptance_criteria: Vec::new(),
                profiles: vec![],
                subtasks: vec![],
            })
            .collect();

        TasksFile {
            default_model: None,
            tasks: nodes,
        }
    }

    #[test]
    fn test_sync_blocked_unblocks_todo_tasks() {
        // Setup: T01 done, T02 blocked by scheduler (failed retries)
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Done),
            ("T02", "api", "Second", TaskStatus::Blocked),
        ]);
        let mut sched = TaskScheduler::new(dag, &progress, 3);
        assert!(sched.blocked.contains("T02"));
        assert_eq!(sched.next_ready_task(), None);

        // User edits tasks.yml: T02 blocked → todo
        let tf = make_tasks_file(vec![("T01", TaskStatus::Done), ("T02", TaskStatus::Todo)]);

        let unblocked = sched.sync_blocked_with_file(&tf);

        assert_eq!(unblocked, vec!["T02".to_string()]);
        assert!(!sched.blocked.contains("T02"));
        assert_eq!(sched.next_ready_task(), Some("T02".to_string()));
    }

    #[test]
    fn test_sync_blocked_preserves_still_blocked_tasks() {
        // T02 blocked in scheduler AND still blocked in file → stays blocked
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Done),
            ("T02", "api", "Second", TaskStatus::Blocked),
        ]);
        let mut sched = TaskScheduler::new(dag, &progress, 3);

        let tf = make_tasks_file(vec![
            ("T01", TaskStatus::Done),
            ("T02", TaskStatus::Blocked),
        ]);

        let unblocked = sched.sync_blocked_with_file(&tf);

        assert!(unblocked.is_empty());
        assert!(sched.blocked.contains("T02"));
    }

    #[test]
    fn test_sync_blocked_does_not_touch_done_or_in_progress() {
        // T01 done, T02 in_progress, T03 blocked→todo in file
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![]), ("T03", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Done),
            ("T02", "api", "Second", TaskStatus::Todo),
            ("T03", "api", "Third", TaskStatus::Blocked),
        ]);
        let mut sched = TaskScheduler::new(dag, &progress, 3);
        sched.mark_started("T02");

        // File says T01=done, T02=in_progress, T03=todo
        let tf = make_tasks_file(vec![
            ("T01", TaskStatus::Done),
            ("T02", TaskStatus::InProgress),
            ("T03", TaskStatus::Todo),
        ]);

        let unblocked = sched.sync_blocked_with_file(&tf);

        // Only T03 should be unblocked
        assert_eq!(unblocked, vec!["T03".to_string()]);
        assert!(sched.done.contains("T01"));
        assert!(sched.in_progress.contains("T02"));
    }

    #[test]
    fn test_sync_blocked_clears_failed_retries() {
        // T01 blocked after max retries → user sets to todo → retry counter cleared
        let dag = make_dag(vec![("T01", vec![])]);
        let progress = make_progress(vec![("T01", "api", "First", TaskStatus::Todo)]);
        let mut sched = TaskScheduler::new(dag, &progress, 2);

        // Exhaust retries: fail twice → blocked
        let t = sched.next_ready_task().unwrap();
        sched.mark_started(&t);
        sched.mark_failed(&t); // retry 1
        let t = sched.next_ready_task().unwrap();
        sched.mark_started(&t);
        sched.mark_failed(&t); // retry 2 → blocked

        assert!(sched.blocked.contains("T01"));
        assert_eq!(sched.retry_count("T01"), 2);

        // User unblocks via file
        let tf = make_tasks_file(vec![("T01", TaskStatus::Todo)]);
        let unblocked = sched.sync_blocked_with_file(&tf);

        assert_eq!(unblocked, vec!["T01".to_string()]);
        assert!(!sched.blocked.contains("T01"));
        assert_eq!(sched.retry_count("T01"), 0);
        assert_eq!(sched.next_ready_task(), Some("T01".to_string()));
    }

    #[test]
    fn test_sync_blocked_noop_when_no_blocked_tasks() {
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Todo),
            ("T02", "api", "Second", TaskStatus::Todo),
        ]);
        let mut sched = TaskScheduler::new(dag, &progress, 3);

        let tf = make_tasks_file(vec![("T01", TaskStatus::Todo), ("T02", TaskStatus::Todo)]);

        let unblocked = sched.sync_blocked_with_file(&tf);
        assert!(unblocked.is_empty());
    }

    #[test]
    fn test_sync_blocked_multiple_unblocked_at_once() {
        // Multiple blocked tasks all set to todo in file
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![]), ("T03", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Blocked),
            ("T02", "api", "Second", TaskStatus::Blocked),
            ("T03", "api", "Third", TaskStatus::Blocked),
        ]);
        let mut sched = TaskScheduler::new(dag, &progress, 3);
        assert_eq!(sched.blocked.len(), 3);

        let tf = make_tasks_file(vec![
            ("T01", TaskStatus::Todo),
            ("T02", TaskStatus::Todo),
            ("T03", TaskStatus::Todo),
        ]);

        let mut unblocked = sched.sync_blocked_with_file(&tf);
        unblocked.sort();

        assert_eq!(unblocked.len(), 3);
        assert!(sched.blocked.is_empty());
        // All should now be in ready queue
        assert_eq!(sched.status().ready, 3);
    }

    #[test]
    fn test_sync_blocked_partial_unblock() {
        // T01 blocked→todo, T02 stays blocked, T03 blocked→todo
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![]), ("T03", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Blocked),
            ("T02", "api", "Second", TaskStatus::Blocked),
            ("T03", "api", "Third", TaskStatus::Blocked),
        ]);
        let mut sched = TaskScheduler::new(dag, &progress, 3);

        let tf = make_tasks_file(vec![
            ("T01", TaskStatus::Todo),
            ("T02", TaskStatus::Blocked),
            ("T03", TaskStatus::Todo),
        ]);

        let mut unblocked = sched.sync_blocked_with_file(&tf);
        unblocked.sort();

        assert_eq!(unblocked, vec!["T01".to_string(), "T03".to_string()]);
        assert!(sched.blocked.contains("T02"));
        assert!(!sched.blocked.contains("T01"));
        assert!(!sched.blocked.contains("T03"));
    }

    #[test]
    fn test_sync_blocked_removed_task_stays_blocked() {
        // Task blocked in scheduler but removed from file → stays in blocked set
        // (it will be filtered out when add_tasks is called with new DAG)
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Done),
            ("T02", "api", "Second", TaskStatus::Blocked),
        ]);
        let mut sched = TaskScheduler::new(dag, &progress, 3);
        assert!(sched.blocked.contains("T02"));

        // File now only contains T01 — T02 was removed
        let tf = make_tasks_file(vec![("T01", TaskStatus::Done)]);

        let unblocked = sched.sync_blocked_with_file(&tf);

        // T02 is not in file, so it doesn't get unblocked by sync
        assert!(unblocked.is_empty());
        assert!(sched.blocked.contains("T02"));
        // Note: T02 will be cleaned up when add_tasks() is called with the new DAG
    }

    #[test]
    fn test_sync_full_cycle_blocked_to_ready_to_assigned() {
        // Full integration test: blocked task → unblocked via file → enters ready queue
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Done),
            ("T02", "api", "Second", TaskStatus::Blocked),
        ]);
        let mut sched = TaskScheduler::new(dag, &progress, 3);

        // Initial state: T02 blocked, nothing in ready queue
        assert!(sched.blocked.contains("T02"));
        assert_eq!(sched.next_ready_task(), None);
        assert_eq!(sched.status().ready, 0);
        assert_eq!(sched.status().blocked, 1);

        // User edits file: T02 blocked → todo
        let tf = make_tasks_file(vec![("T01", TaskStatus::Done), ("T02", TaskStatus::Todo)]);
        let unblocked = sched.sync_blocked_with_file(&tf);

        // T02 should be unblocked and in ready queue
        assert_eq!(unblocked, vec!["T02".to_string()]);
        assert!(!sched.blocked.contains("T02"));
        assert_eq!(sched.status().ready, 1);
        assert_eq!(sched.status().blocked, 0);

        // Simulate worker picking up the task
        let task = sched.next_ready_task();
        assert_eq!(task, Some("T02".to_string()));
        assert_eq!(sched.status().ready, 0);

        // Worker marks it as started
        sched.mark_started("T02");
        assert!(sched.in_progress_tasks().contains(&"T02".to_string()));
        assert_eq!(sched.status().in_progress, 1);

        // Worker completes it
        sched.mark_done("T02");
        assert!(sched.done_tasks().contains(&"T02".to_string()));
        assert_eq!(sched.status().done, 2);
        assert!(sched.is_complete());
    }

    #[test]
    fn test_sync_blocked_ignores_done_tasks_in_file() {
        // Done task in scheduler with todo in file → stays done (immutable)
        let dag = make_dag(vec![("T01", vec![])]);
        let progress = make_progress(vec![("T01", "api", "First", TaskStatus::Done)]);
        let sched = TaskScheduler::new(dag, &progress, 3);

        // File incorrectly shows T01 as todo (shouldn't happen but handle gracefully)
        let tf = make_tasks_file(vec![("T01", TaskStatus::Todo)]);

        // sync_blocked_with_file only looks at blocked set, so done tasks are ignored
        let mut sched_mut = sched;
        let unblocked = sched_mut.sync_blocked_with_file(&tf);

        assert!(unblocked.is_empty());
        assert!(sched_mut.done_tasks().contains(&"T01".to_string()));
        assert_eq!(sched_mut.next_ready_task(), None);
    }

    #[test]
    fn test_sync_blocked_ignores_in_progress_tasks_in_file() {
        // In-progress task in scheduler with todo in file → stays in-progress (immutable)
        let dag = make_dag(vec![("T01", vec![])]);
        let progress = make_progress(vec![("T01", "api", "First", TaskStatus::Todo)]);
        let mut sched = TaskScheduler::new(dag, &progress, 3);

        let t = sched.next_ready_task().unwrap();
        sched.mark_started(&t);
        assert!(sched.in_progress_tasks().contains(&"T01".to_string()));

        // File shows T01 as todo
        let tf = make_tasks_file(vec![("T01", TaskStatus::Todo)]);

        let unblocked = sched.sync_blocked_with_file(&tf);

        // No unblocking happened (T01 was never in blocked set)
        assert!(unblocked.is_empty());
        assert!(sched.in_progress_tasks().contains(&"T01".to_string()));
        assert!(!sched.blocked_tasks().contains(&"T01".to_string()));
    }

    #[test]
    fn test_sync_blocked_multiple_scenarios_combined() {
        // Complex scenario: multiple tasks in different states
        let dag = make_dag(vec![
            ("T01", vec![]),
            ("T02", vec![]),
            ("T03", vec![]),
            ("T04", vec![]),
            ("T05", vec![]),
        ]);
        let progress = make_progress(vec![
            ("T01", "api", "Done task", TaskStatus::Done),
            ("T02", "api", "Blocked→todo", TaskStatus::Blocked),
            ("T03", "api", "Blocked→blocked", TaskStatus::Blocked),
            ("T04", "api", "In progress", TaskStatus::Todo),
            ("T05", "api", "Blocked→todo", TaskStatus::Blocked),
        ]);
        let mut sched = TaskScheduler::new(dag, &progress, 3);

        // Mark T04 as in-progress
        let t = sched.next_ready_task().unwrap(); // T04
        sched.mark_started(&t);

        // File state: T02 and T05 unblocked, T03 stays blocked
        let tf = make_tasks_file(vec![
            ("T01", TaskStatus::Done),
            ("T02", TaskStatus::Todo),
            ("T03", TaskStatus::Blocked),
            ("T04", TaskStatus::InProgress),
            ("T05", TaskStatus::Todo),
        ]);

        let mut unblocked = sched.sync_blocked_with_file(&tf);
        unblocked.sort();

        // T02 and T05 should be unblocked
        assert_eq!(unblocked, vec!["T02".to_string(), "T05".to_string()]);
        assert!(sched.done_tasks().contains(&"T01".to_string()));
        assert!(!sched.blocked_tasks().contains(&"T02".to_string()));
        assert!(sched.blocked_tasks().contains(&"T03".to_string()));
        assert!(sched.in_progress_tasks().contains(&"T04".to_string()));
        assert!(!sched.blocked_tasks().contains(&"T05".to_string()));

        // Ready queue should now contain T02 and T05
        assert_eq!(sched.status().ready, 2);
    }

    #[test]
    fn test_max_retries_zero_immediate_block() {
        // Edge case: max_retries=0 means first failure blocks immediately
        let dag = make_dag(vec![("T01", vec![])]);
        let progress = make_progress(vec![("T01", "api", "First", TaskStatus::Todo)]);

        let mut sched = TaskScheduler::new(dag, &progress, 0); // max_retries=0

        // First attempt
        let task = sched.next_ready_task().unwrap();
        assert_eq!(task, "T01");
        sched.mark_started(&task);

        // First failure should immediately block (no retries)
        let requeued = sched.mark_failed(&task);
        assert!(!requeued, "Task should not be requeued with max_retries=0");
        assert_eq!(sched.retry_count("T01"), 1);
        assert!(
            sched.blocked.contains("T01"),
            "T01 should be blocked after first failure"
        );

        // Task should not appear in ready queue
        assert_eq!(sched.next_ready_task(), None);
        assert_eq!(sched.status().blocked, 1);
        assert_eq!(sched.status().ready, 0);
    }

    // ── Task 53.2: Integration test - TOML max_retries=0 with scheduler ──

    /// Test that max_retries=0 parsed from .ralph.toml works correctly
    /// with the scheduler's blocking behavior.
    #[test]
    fn test_toml_max_retries_zero_scheduler_integration() {
        use crate::shared::file_config::FileConfig;

        // Parse TOML config with max_retries=0
        let toml_content = r#"
[task.orchestrate]
max_retries = 0
workers = 2
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.task.orchestrate.max_retries, 0,
            "Config should parse max_retries=0"
        );

        // Create scheduler with parsed max_retries value
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec!["T01"])]);
        let progress = make_progress(vec![
            ("T01", "api", "First task", TaskStatus::Todo),
            ("T02", "api", "Dependent task", TaskStatus::Todo),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, config.task.orchestrate.max_retries);

        // T01 is ready (no deps)
        let task = sched.next_ready_task().unwrap();
        assert_eq!(task, "T01");
        sched.mark_started(&task);

        // First failure with max_retries=0 should block immediately
        let requeued = sched.mark_failed(&task);
        assert!(!requeued, "Task should not be requeued when max_retries=0");
        assert_eq!(sched.retry_count("T01"), 1);
        assert!(
            sched.blocked_tasks().contains(&"T01".to_string()),
            "T01 should be blocked after first failure"
        );

        // T02 should NOT become ready (its dependency T01 is blocked, not done)
        assert_eq!(
            sched.next_ready_task(),
            None,
            "No tasks should be ready when dependency is blocked"
        );

        // Verify status
        let status = sched.status();
        assert_eq!(status.blocked, 1, "T01 should be blocked");
        assert_eq!(status.done, 0, "No tasks completed");
        assert_eq!(status.in_progress, 0, "No tasks in progress");
        assert_eq!(status.ready, 0, "No tasks ready");
        assert_eq!(
            status.pending, 1,
            "T02 should be pending (waiting for blocked T01)"
        );
    }

    // ── Task 51.5: Scheduler status() underflow with inconsistent sets ─

    /// Sets (done/in_progress/blocked) mogą zawierać ID spoza DAG.
    /// saturating_sub gwarantuje pending = 0 zamiast underflow.
    #[test]
    fn test_status_underflow_with_extra_done_tasks() {
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Todo),
            ("T02", "api", "Second", TaskStatus::Todo),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);

        // 5 done IDs, w tym 3 spoza DAG — sum > total
        sched.done.insert("T01".to_string());
        sched.done.insert("T02".to_string());
        sched.done.insert("T99".to_string());
        sched.done.insert("T88".to_string());
        sched.done.insert("T77".to_string());

        let status = sched.status();

        assert_eq!(status.total, 2);
        assert_eq!(status.done, 5);
        assert_eq!(status.pending, 0); // saturating_sub: max(2-5, 0) = 0
    }

    /// Underflow z niespójnymi in_progress + blocked (łącznie > total).
    #[test]
    fn test_status_underflow_with_inconsistent_in_progress_and_blocked() {
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Todo),
            ("T02", "api", "Second", TaskStatus::Todo),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);

        // in_progress z ID spoza DAG
        sched.in_progress.insert("T01".to_string());
        sched.in_progress.insert("GHOST1".to_string());
        sched.in_progress.insert("GHOST2".to_string());

        // blocked z ID spoza DAG
        sched.blocked.insert("T02".to_string());
        sched.blocked.insert("GHOST3".to_string());

        let status = sched.status();

        assert_eq!(status.total, 2);
        assert_eq!(status.in_progress, 3);
        assert_eq!(status.blocked, 2);
        // total(2) - in_progress(3) = 0 (saturated), - blocked(2) = 0
        assert_eq!(status.pending, 0);
    }

    #[test]
    fn test_next_ready_task_skips_blocked_in_queue() {
        // Task 51.7: Task added to ready queue, then marked blocked.
        // next_ready_task() should skip it and return the next unblocked task.
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec![])]);
        let progress = make_progress(vec![
            ("T01", "api", "First", TaskStatus::Todo),
            ("T02", "api", "Second", TaskStatus::Todo),
        ]);

        let mut sched = TaskScheduler::new(dag, &progress, 3);

        // Both tasks should be in ready queue after construction
        assert_eq!(sched.status().ready, 2);

        // Mark T01 as blocked via public API (does not refresh ready queue,
        // so T01 stays in the queue but should be skipped by next_ready_task)
        sched.mark_blocked("T01");

        // next_ready_task() should skip blocked T01 and return T02
        let next = sched.next_ready_task();
        assert_eq!(next, Some("T02".to_string()));

        // No more unblocked tasks in queue
        let next2 = sched.next_ready_task();
        assert_eq!(next2, None);
    }
}
