use std::collections::{HashMap, HashSet, VecDeque};

use crate::shared::dag::TaskDag;
use crate::shared::progress::{ProgressSummary, TaskStatus};

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
    pub fn next_ready_task(&mut self) -> Option<String> {
        self.ready_queue.pop_front()
    }

    /// Mark a task as started (in-progress).
    pub fn mark_started(&mut self, task_id: &str) {
        self.in_progress.insert(task_id.to_string());
    }

    /// Mark a task as successfully completed.
    /// Refreshes the ready queue to unlock dependent tasks.
    pub fn mark_done(&mut self, task_id: &str) {
        self.in_progress.remove(task_id);
        self.done.insert(task_id.to_string());
        self.refresh_ready_queue();
    }

    /// Mark a task as permanently blocked.
    pub fn mark_blocked(&mut self, task_id: &str) {
        self.in_progress.remove(task_id);
        self.blocked.insert(task_id.to_string());
    }

    /// Handle a task failure: increment retry count, re-queue or block.
    ///
    /// Returns `true` if the task was re-queued, `false` if blocked.
    pub fn mark_failed(&mut self, task_id: &str) -> bool {
        self.in_progress.remove(task_id);

        let retries = self.failed_retries.entry(task_id.to_string()).or_insert(0);
        *retries += 1;

        if *retries >= self.max_retries {
            self.blocked.insert(task_id.to_string());
            false
        } else {
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

    /// Get current counts for status display.
    pub fn status(&self) -> SchedulerStatus {
        SchedulerStatus {
            total: self.dag.tasks().len(),
            done: self.done.len(),
            in_progress: self.in_progress.len(),
            blocked: self.blocked.len(),
            ready: self.ready_queue.len(),
            pending: self.dag.tasks().len()
                - self.done.len()
                - self.in_progress.len()
                - self.blocked.len(),
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
}
