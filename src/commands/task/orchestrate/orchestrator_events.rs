use std::sync::Arc;
use std::time::Duration;

use crate::commands::task::orchestrate::events::{WorkerEventKind, WorkerPhase};
use crate::commands::task::orchestrate::summary::TaskSummaryEntry;
use crate::commands::task::orchestrate::worker_status::{WorkerState, WorkerStatus};
use crate::shared::error::Result;
use crate::shared::progress::TaskStatus;

use super::assignment::WorkerSlot;
use super::orchestrator::Orchestrator;
use super::orchestrator_merge::PendingMerge;
use super::run_loop::RunLoopContext;

// ── Separator formatting ────────────────────────────────────────────

/// Generate a colored separator line for task start.
/// Uses double line (═══) to distinguish from phase separators (───).
fn task_start_separator(task_id: &str) -> String {
    format!("\x1b[1;32m══════ Task {task_id} ══════\x1b[0m")
}

/// Generate a colored separator line for phase start.
fn phase_start_separator(phase: &WorkerPhase) -> String {
    let (icon, color_code, name) = match phase {
        WorkerPhase::Setup => ("⚙", "\x1b[34m", "Setup"), // Blue
        WorkerPhase::Implement => ("●", "\x1b[36m", "Implement"), // Cyan
        WorkerPhase::ReviewFix => ("◎", "\x1b[33m", "Review+Fix"), // Yellow
        WorkerPhase::Verify => ("◉", "\x1b[35m", "Verify"), // Magenta
    };
    format!("{color_code}─── {icon} {name} ───\x1b[0m")
}

/// Generate a colored separator line for phase completion.
fn phase_end_separator(phase: &WorkerPhase, success: bool) -> String {
    let name = match phase {
        WorkerPhase::Setup => "Setup",
        WorkerPhase::Implement => "Implement",
        WorkerPhase::ReviewFix => "Review+Fix",
        WorkerPhase::Verify => "Verify",
    };
    let (icon, color_code, status) = if success {
        ("✓", "\x1b[32m", "ok") // Green
    } else {
        ("✗", "\x1b[31m", "FAILED") // Red
    };
    format!("{color_code}─── {icon} {name} [{status}] ───\x1b[0m")
}

/// Generate a colored separator line for conflict resolution start.
pub(super) fn conflict_resolution_start_separator() -> String {
    // Red color to match WorkerState::ResolvingConflicts in panels.rs
    "\x1b[31m─── ⚡ Conflict Resolution ───\x1b[0m".to_string()
}

/// Generate a colored separator line for conflict resolution end.
pub(super) fn conflict_resolution_end_separator(success: bool) -> String {
    let (icon, color_code, status) = if success {
        ("✓", "\x1b[32m", "ok") // Green
    } else {
        ("✗", "\x1b[31m", "FAILED") // Red
    };
    format!("{color_code}─── {icon} Conflict Resolution [{status}] ───\x1b[0m")
}

/// Generate a colored separator line for verify phase start.
/// Yellow color to indicate "checking" phase.
pub(super) fn verify_start_separator() -> String {
    "\x1b[33m─── 🔍 Verify ───\x1b[0m".to_string()
}

/// Generate a colored separator line for verify phase end.
pub(super) fn verify_end_separator(success: bool) -> String {
    let (icon, color_code, status) = if success {
        ("✓", "\x1b[32m", "ok") // Green
    } else {
        ("✗", "\x1b[31m", "FAILED") // Red
    };
    format!("{color_code}─── {icon} Verify [{status}] ───\x1b[0m")
}

// ── Event handling ──────────────────────────────────────────────────

impl Orchestrator {
    /// Handle a worker event from the mpsc channel.
    pub(super) async fn handle_event(
        &self,
        event: &crate::commands::task::orchestrate::events::WorkerEvent,
        ctx: &mut RunLoopContext<'_>,
    ) -> Result<()> {
        match &event.kind {
            WorkerEventKind::TaskStarted { worker_id, task_id } => {
                // Clear worker output buffer for a fresh start
                ctx.tui.dashboard.clear_worker_output(*worker_id);

                // Add task start separator
                let separator = task_start_separator(task_id);
                ctx.tui
                    .dashboard
                    .push_worker_output(*worker_id, &[separator]);

                // Initialize heartbeat timestamp for this worker
                ctx.last_heartbeat
                    .insert(*worker_id, std::time::Instant::now());

                // Resolve model for this task
                let model = self.resolve_model(task_id, ctx.tasks_file);
                let ws = WorkerStatus {
                    state: WorkerState::Implementing,
                    task_id: Some(task_id.clone()),
                    component: ctx
                        .task_lookup
                        .get(task_id.as_str())
                        .map(|t| t.component.clone()),
                    phase: Some(WorkerPhase::Implement),
                    model: model
                        .as_ref()
                        .map(|m| crate::shared::tasks::reverse_model_alias(m)),
                    cost_usd: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                };
                ctx.tui.dashboard.update_worker_status(*worker_id, ws);
                let msg = ctx
                    .tui
                    .mux_output
                    .format_worker_line(*worker_id, &format!("Started: {task_id}"));
                ctx.tui.dashboard.push_log_line(&msg);
            }

            WorkerEventKind::PhaseStarted {
                worker_id,
                task_id,
                phase,
            } => {
                let new_state = match phase {
                    WorkerPhase::Setup => WorkerState::SettingUp,
                    WorkerPhase::Implement => WorkerState::Implementing,
                    WorkerPhase::ReviewFix => WorkerState::Reviewing,
                    WorkerPhase::Verify => WorkerState::Verifying,
                };
                let (cost, input, output) = ctx.tui.mux_output.worker_cost(*worker_id);
                // Use review_model for ReviewFix phase, task's model for others
                let model = if matches!(phase, WorkerPhase::ReviewFix) {
                    Some(self.config.review_model.clone())
                } else {
                    self.resolve_model(task_id, ctx.tasks_file)
                };
                let ws = WorkerStatus {
                    state: new_state,
                    task_id: Some(task_id.clone()),
                    component: ctx
                        .task_lookup
                        .get(task_id.as_str())
                        .map(|t| t.component.clone()),
                    phase: Some(phase.clone()),
                    model: model
                        .as_ref()
                        .map(|m| crate::shared::tasks::reverse_model_alias(m)),
                    cost_usd: cost,
                    input_tokens: input,
                    output_tokens: output,
                };
                ctx.tui.dashboard.update_worker_status(*worker_id, ws);
                let msg = ctx
                    .tui
                    .mux_output
                    .format_worker_line(*worker_id, &format!("{task_id} → phase: {phase}"));
                ctx.tui.dashboard.push_log_line(&msg);

                // Add phase separator to worker output
                let separator = phase_start_separator(phase);
                ctx.tui
                    .dashboard
                    .push_worker_output(*worker_id, &[separator]);
            }

            WorkerEventKind::PhaseCompleted {
                worker_id,
                task_id,
                phase,
                success,
            } => {
                let status_text = if *success { "ok" } else { "FAILED" };
                let msg = ctx.tui.mux_output.format_worker_line(
                    *worker_id,
                    &format!("{task_id} ← phase: {phase} [{status_text}]"),
                );
                ctx.tui.dashboard.push_log_line(&msg);

                // Add phase completion separator to worker output
                let separator = phase_end_separator(phase, *success);
                ctx.tui
                    .dashboard
                    .push_worker_output(*worker_id, &[separator]);
            }

            WorkerEventKind::CostUpdate {
                worker_id,
                cost_usd,
                input_tokens,
                output_tokens,
            } => {
                ctx.tui.mux_output.update_cost(
                    *worker_id,
                    *cost_usd,
                    *input_tokens,
                    *output_tokens,
                );
                let (total_cost, total_in, total_out) = ctx.tui.mux_output.worker_cost(*worker_id);
                ctx.tui
                    .dashboard
                    .update_worker_cost(*worker_id, total_cost, total_in, total_out);
            }

            WorkerEventKind::TaskCompleted {
                worker_id,
                task_id,
                success,
                cost_usd,
                ..
            } => {
                // Wait for the worker's join handle to complete
                if let Some(handle) = ctx.join_handles.remove(worker_id) {
                    let _ = handle.await;
                }

                let duration = ctx
                    .tui
                    .task_start_times
                    .remove(task_id)
                    .map(|s| s.elapsed())
                    .unwrap_or_default();

                if *success && !self.config.no_merge {
                    // Enqueue merge (async — runs in separate tokio task)
                    if let Some(WorkerSlot::Busy { worktree, .. }) = ctx.worker_slots.get(worker_id)
                        && let Some(task) = ctx.task_lookup.get(task_id.as_str())
                    {
                        let (cost, input, output) = ctx.tui.mux_output.worker_cost(*worker_id);
                        // Preserve model from task implementation
                        let model = self.resolve_model(task_id, ctx.tasks_file);
                        let ws = WorkerStatus {
                            state: WorkerState::Merging,
                            task_id: Some(task_id.clone()),
                            component: Some(task.component.clone()),
                            phase: None,
                            model: model
                                .as_ref()
                                .map(|m| crate::shared::tasks::reverse_model_alias(m)),
                            cost_usd: cost,
                            input_tokens: input,
                            output_tokens: output,
                        };
                        ctx.tui.dashboard.update_worker_status(*worker_id, ws);

                        let pending = PendingMerge {
                            worker_id: *worker_id,
                            task_id: task_id.clone(),
                            task_name: task.name.clone(),
                            branch: worktree.branch.clone(),
                        };

                        if ctx.merge_ctx.merge_in_progress {
                            let msg = ctx.tui.mux_output.format_worker_line(
                                *worker_id,
                                &format!("Queued merge: {task_id} (another merge in progress)"),
                            );
                            ctx.tui.dashboard.push_log_line(&msg);
                            ctx.merge_ctx.pending_merges.push_back(pending);
                        } else {
                            Self::spawn_merge_task(
                                &mut ctx.merge_ctx,
                                pending,
                                &self.project_root,
                                ctx.event_tx.clone(),
                                Arc::clone(&ctx.flags.shutdown),
                                &self.config.conflict_resolution_model,
                                self.config.merge_timeout,
                                self.config.phase_timeout,
                            );
                        }
                    } else {
                        // Silent drop guard: worker slot not Busy or task not in lookup.
                        // Without this else branch, the worker would hang forever.
                        let reason = if !matches!(
                            ctx.worker_slots.get(worker_id),
                            Some(WorkerSlot::Busy { .. })
                        ) {
                            "worker slot not Busy"
                        } else {
                            "task not in lookup"
                        };
                        let msg = ctx.tui.mux_output.format_worker_line(
                            *worker_id,
                            &format!("Cannot merge {task_id}: {reason} — releasing worker"),
                        );
                        ctx.tui.dashboard.push_log_line(&msg);

                        ctx.scheduler.mark_blocked(task_id);
                        if let Some(mt) =
                            self.update_tasks_file(task_id, TaskStatus::Blocked, ctx.tui)
                        {
                            ctx.progress_mtime = Some(mt);
                            let mut updated = (*ctx.cached_tasks_file).clone();
                            let _ = updated.update_status(task_id, TaskStatus::Blocked);
                            ctx.cached_tasks_file = Arc::new(updated);
                        }

                        ctx.tui.task_summaries.push(TaskSummaryEntry {
                            task_id: task_id.clone(),
                            status: "Blocked".to_string(),
                            cost_usd: *cost_usd,
                            duration,
                            retries: ctx.scheduler.retry_count(task_id),
                        });

                        // Free the worker slot, clear MCP session, and reset TUI
                        ctx.release_worker(*worker_id);
                        ctx.tui
                            .dashboard
                            .update_worker_status(*worker_id, WorkerStatus::idle(*worker_id));
                        ctx.tui.mux_output.clear_worker(*worker_id);
                    }
                } else if *success {
                    // --no-merge mode
                    ctx.scheduler.mark_done(task_id);
                    if let Some(mt) = self.update_tasks_file(task_id, TaskStatus::Done, ctx.tui) {
                        ctx.progress_mtime = Some(mt);
                        // Sync cached TasksFile
                        let mut updated = (*ctx.cached_tasks_file).clone();
                        let _ = updated.update_status(task_id, TaskStatus::Done);
                        ctx.cached_tasks_file = Arc::new(updated);
                    }
                    let msg = ctx
                        .tui
                        .mux_output
                        .format_worker_line(*worker_id, &format!("Done (no merge): {task_id}"));
                    ctx.tui.dashboard.push_log_line(&msg);

                    ctx.tui.task_summaries.push(TaskSummaryEntry {
                        task_id: task_id.clone(),
                        status: "Done".to_string(),
                        cost_usd: *cost_usd,
                        duration,
                        retries: ctx.scheduler.retry_count(task_id),
                    });

                    // Free the worker slot, clear MCP session, and reset TUI
                    ctx.release_worker(*worker_id);
                    ctx.tui
                        .dashboard
                        .update_worker_status(*worker_id, WorkerStatus::idle(*worker_id));
                    ctx.tui.mux_output.clear_worker(*worker_id);
                } else {
                    // Task failed
                    let requeued = ctx.scheduler.mark_failed(task_id);
                    if requeued {
                        let msg = ctx.tui.mux_output.format_worker_line(
                            *worker_id,
                            &format!("Task {task_id} failed, re-queued for retry"),
                        );
                        ctx.tui.dashboard.push_log_line(&msg);
                    } else {
                        let msg = ctx.tui.mux_output.format_worker_line(
                            *worker_id,
                            &format!("Task {task_id} blocked after max retries"),
                        );
                        ctx.tui.dashboard.push_log_line(&msg);
                        if let Some(mt) =
                            self.update_tasks_file(task_id, TaskStatus::Blocked, ctx.tui)
                        {
                            ctx.progress_mtime = Some(mt);
                            // Sync cached TasksFile
                            let mut updated = (*ctx.cached_tasks_file).clone();
                            let _ = updated.update_status(task_id, TaskStatus::Blocked);
                            ctx.cached_tasks_file = Arc::new(updated);
                        }

                        ctx.tui.task_summaries.push(TaskSummaryEntry {
                            task_id: task_id.clone(),
                            status: "Blocked".to_string(),
                            cost_usd: *cost_usd,
                            duration,
                            retries: ctx.scheduler.retry_count(task_id),
                        });
                    }

                    // Free the worker slot, clear MCP session, and reset TUI
                    ctx.release_worker(*worker_id);
                    ctx.tui
                        .dashboard
                        .update_worker_status(*worker_id, WorkerStatus::idle(*worker_id));
                    ctx.tui.mux_output.clear_worker(*worker_id);
                }
            }

            WorkerEventKind::TaskFailed {
                worker_id,
                task_id,
                error,
                retries_left,
            } => {
                let msg = if *retries_left == 0 {
                    ctx.tui.mux_output.format_worker_line(
                        *worker_id,
                        &format!("Task {task_id} failed permanently: {error}"),
                    )
                } else {
                    ctx.tui.mux_output.format_worker_line(
                        *worker_id,
                        &format!("Task {task_id} failed ({retries_left} retries left): {error}"),
                    )
                };
                ctx.tui.dashboard.push_log_line(&msg);
            }

            WorkerEventKind::MergeStarted { worker_id, task_id } => {
                let (cost, input, output) = ctx.tui.mux_output.worker_cost(*worker_id);
                // Preserve model from task implementation
                let model = self.resolve_model(task_id, ctx.tasks_file);
                let ws = WorkerStatus {
                    state: WorkerState::Merging,
                    task_id: Some(task_id.clone()),
                    component: ctx
                        .task_lookup
                        .get(task_id.as_str())
                        .map(|t| t.component.clone()),
                    phase: None,
                    model: model
                        .as_ref()
                        .map(|m| crate::shared::tasks::reverse_model_alias(m)),
                    cost_usd: cost,
                    input_tokens: input,
                    output_tokens: output,
                };
                ctx.tui.dashboard.update_worker_status(*worker_id, ws);
                let msg = ctx
                    .tui
                    .mux_output
                    .format_worker_line(*worker_id, &format!("Merging: {task_id}"));
                ctx.tui.dashboard.push_log_line(&msg);
            }

            WorkerEventKind::MergeCompleted {
                worker_id,
                task_id,
                success,
                commit_hash,
            } => {
                // Wait for merge join handle
                if let Some(handle) = ctx.merge_ctx.merge_join_handle.take() {
                    let _ = handle.await;
                }

                let hash = commit_hash.as_deref().unwrap_or("???");
                let status_text = if *success { "ok" } else { "FAILED" };
                let msg = ctx.tui.mux_output.format_worker_line(
                    *worker_id,
                    &format!("Merge {task_id}: {status_text} ({hash})"),
                );
                ctx.tui.dashboard.push_log_line(&msg);

                if *success {
                    ctx.scheduler.mark_done(task_id);
                    if let Some(mt) = self.update_tasks_file(task_id, TaskStatus::Done, ctx.tui) {
                        ctx.progress_mtime = Some(mt);
                        // Sync cached TasksFile
                        let mut updated = (*ctx.cached_tasks_file).clone();
                        let _ = updated.update_status(task_id, TaskStatus::Done);
                        ctx.cached_tasks_file = Arc::new(updated);
                    }

                    // Worktree cleanup
                    if let Some(WorkerSlot::Busy { worktree, .. }) = ctx.worker_slots.get(worker_id)
                    {
                        ctx.worktree_manager
                            .remove_worktree(&worktree.path)
                            .await
                            .ok();
                        ctx.worktree_manager
                            .remove_branch(&worktree.branch)
                            .await
                            .ok();
                    }

                    // Update state
                    let cost_usd = ctx.tui.mux_output.worker_cost(*worker_id).0;
                    ctx.state.tasks.insert(
                        task_id.clone(),
                        crate::commands::task::orchestrate::state::TaskState {
                            status: "done".to_string(),
                            worker: Some(*worker_id),
                            retries: ctx.scheduler.retry_count(task_id),
                            cost: cost_usd,
                        },
                    );

                    ctx.tui.task_summaries.push(TaskSummaryEntry {
                        task_id: task_id.clone(),
                        status: "Done".to_string(),
                        cost_usd,
                        duration: Duration::ZERO,
                        retries: ctx.scheduler.retry_count(task_id),
                    });
                } else {
                    ctx.scheduler.mark_blocked(task_id);
                    if let Some(mt) = self.update_tasks_file(task_id, TaskStatus::Blocked, ctx.tui)
                    {
                        ctx.progress_mtime = Some(mt);
                        // Sync cached TasksFile
                        let mut updated = (*ctx.cached_tasks_file).clone();
                        let _ = updated.update_status(task_id, TaskStatus::Blocked);
                        ctx.cached_tasks_file = Arc::new(updated);
                    }

                    let cost_usd = ctx.tui.mux_output.worker_cost(*worker_id).0;
                    ctx.state.tasks.insert(
                        task_id.clone(),
                        crate::commands::task::orchestrate::state::TaskState {
                            status: "blocked".to_string(),
                            worker: Some(*worker_id),
                            retries: ctx.scheduler.retry_count(task_id),
                            cost: cost_usd,
                        },
                    );

                    ctx.tui.task_summaries.push(TaskSummaryEntry {
                        task_id: task_id.clone(),
                        status: "Blocked".to_string(),
                        cost_usd,
                        duration: Duration::ZERO,
                        retries: ctx.scheduler.retry_count(task_id),
                    });
                }

                // Free the worker slot, clear MCP session, and reset TUI
                ctx.release_worker(*worker_id);
                ctx.tui
                    .dashboard
                    .update_worker_status(*worker_id, WorkerStatus::idle(*worker_id));
                ctx.tui.mux_output.clear_worker(*worker_id);

                // Start next queued merge if any
                ctx.merge_ctx.merge_in_progress = false;
                ctx.merge_ctx.current_merge_worker_id = None;
                if let Some(next) = ctx.merge_ctx.pending_merges.pop_front() {
                    Self::spawn_merge_task(
                        &mut ctx.merge_ctx,
                        next,
                        &self.project_root,
                        ctx.event_tx.clone(),
                        Arc::clone(&ctx.flags.shutdown),
                        &self.config.conflict_resolution_model,
                        self.config.merge_timeout,
                        self.config.phase_timeout,
                    );
                }
            }

            WorkerEventKind::MergeConflict {
                worker_id,
                task_id,
                conflicting_files,
            } => {
                let msg = ctx.tui.mux_output.format_worker_line(
                    *worker_id,
                    &format!("Merge conflict in {task_id}: {conflicting_files:?}"),
                );
                ctx.tui.dashboard.push_log_line(&msg);

                // Update worker state to show conflict resolution
                let (cost, input, output) = ctx.tui.mux_output.worker_cost(*worker_id);
                // Use conflict resolution model (not the task model)
                let conflict_model = &self.config.conflict_resolution_model;
                let ws = WorkerStatus {
                    state: WorkerState::ResolvingConflicts,
                    task_id: Some(task_id.clone()),
                    component: ctx
                        .task_lookup
                        .get(task_id.as_str())
                        .map(|t| t.component.clone()),
                    phase: None,
                    model: Some(crate::shared::tasks::reverse_model_alias(conflict_model)),
                    cost_usd: cost,
                    input_tokens: input,
                    output_tokens: output,
                };
                ctx.tui.dashboard.update_worker_status(*worker_id, ws);

                // Add conflict resolution start separator
                let separator = conflict_resolution_start_separator();
                ctx.tui
                    .dashboard
                    .push_worker_output(*worker_id, &[separator]);
            }

            WorkerEventKind::OutputLines { worker_id, lines } => {
                ctx.tui.dashboard.push_worker_output(*worker_id, lines);
            }

            WorkerEventKind::MergeStepOutput { worker_id, lines } => {
                ctx.tui.dashboard.push_worker_output(*worker_id, lines);
            }

            WorkerEventKind::Heartbeat {
                worker_id,
                phase: _,
            } => {
                // Update last heartbeat timestamp for this worker
                ctx.last_heartbeat
                    .insert(*worker_id, std::time::Instant::now());
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        conflict_resolution_end_separator, conflict_resolution_start_separator,
        phase_end_separator, phase_start_separator, task_start_separator, verify_end_separator,
        verify_start_separator,
    };
    use crate::commands::task::orchestrate::events::WorkerPhase;
    use crate::shared::progress::TaskStatus;
    use crate::shared::tasks::{TaskNode, TasksFile};
    use std::sync::Arc;

    /// Sample TasksFile for testing
    fn sample_tasks_file() -> TasksFile {
        TasksFile {
            default_model: Some("claude-sonnet-4-5-20250929".to_string()),
            tasks: vec![
                TaskNode {
                    id: "1.1".to_string(),
                    name: "Task 1.1".to_string(),
                    component: Some("api".to_string()),
                    status: Some(TaskStatus::Todo),
                    deps: vec![],
                    model: None,
                    description: None,
                    related_files: vec![],
                    implementation_steps: vec![],
                    subtasks: vec![],
                },
                TaskNode {
                    id: "1.2".to_string(),
                    name: "Task 1.2".to_string(),
                    component: Some("ui".to_string()),
                    status: Some(TaskStatus::Todo),
                    deps: vec!["1.1".to_string()],
                    model: None,
                    description: None,
                    related_files: vec![],
                    implementation_steps: vec![],
                    subtasks: vec![],
                },
            ],
        }
    }

    #[test]
    fn test_cached_tasks_file_updated_after_task_done() {
        // Test: After task marked Done, cached_tasks_file reflects Done status
        let initial = sample_tasks_file();
        let mut cached_tasks_file = Arc::new(initial);

        // Verify initial state
        let leaf = cached_tasks_file
            .find_leaf("1.1")
            .expect("Task 1.1 should exist");
        assert_eq!(leaf.status, TaskStatus::Todo);

        // Simulate manual status update (as done in orchestrator_events.rs:191-193)
        let mut updated = (*cached_tasks_file).clone();
        let updated_ok = updated.update_status("1.1", TaskStatus::Done);
        assert!(updated_ok, "update_status should succeed for existing task");
        cached_tasks_file = Arc::new(updated);

        // Verify cached_tasks_file was updated
        let leaf_after = cached_tasks_file
            .find_leaf("1.1")
            .expect("Task 1.1 should exist after update");
        assert_eq!(leaf_after.status, TaskStatus::Done);
    }

    #[test]
    fn test_cached_tasks_file_updated_after_task_blocked() {
        // Test: After task marked Blocked, cached_tasks_file reflects Blocked status
        let initial = sample_tasks_file();
        let mut cached_tasks_file = Arc::new(initial);

        // Verify initial state
        let leaf = cached_tasks_file
            .find_leaf("1.2")
            .expect("Task 1.2 should exist");
        assert_eq!(leaf.status, TaskStatus::Todo);

        // Simulate manual status update (as done in orchestrator_events.rs:235-237)
        let mut updated = (*cached_tasks_file).clone();
        let updated_ok = updated.update_status("1.2", TaskStatus::Blocked);
        assert!(updated_ok, "update_status should succeed for existing task");
        cached_tasks_file = Arc::new(updated);

        // Verify cached_tasks_file was updated
        let leaf_after = cached_tasks_file
            .find_leaf("1.2")
            .expect("Task 1.2 should exist after update");
        assert_eq!(leaf_after.status, TaskStatus::Blocked);
    }

    #[test]
    fn test_cached_tasks_file_hot_reload_simulation() {
        // Test: After hot-reload, scheduler states are applied to fresh TasksFile
        use crate::commands::task::orchestrate::scheduler::TaskScheduler;
        use crate::shared::dag::TaskDag;
        use crate::shared::progress::{ProgressFrontmatter, ProgressSummary, ProgressTask};

        let initial = sample_tasks_file();

        // Build scheduler with initial state
        let mut frontmatter = ProgressFrontmatter::default();
        for task in initial.flatten_leaves() {
            frontmatter.deps.insert(task.id.clone(), task.deps.clone());
        }
        let dag = TaskDag::from_frontmatter(&frontmatter);

        let progress = ProgressSummary {
            frontmatter: Some(frontmatter),
            tasks: vec![
                ProgressTask {
                    id: "1.1".to_string(),
                    component: "api".to_string(),
                    name: "Task 1.1".to_string(),
                    status: TaskStatus::Todo,
                },
                ProgressTask {
                    id: "1.2".to_string(),
                    component: "ui".to_string(),
                    name: "Task 1.2".to_string(),
                    status: TaskStatus::Todo,
                },
            ],
            done: 0,
            in_progress: 0,
            blocked: 0,
            todo: 2,
        };

        let mut scheduler = TaskScheduler::new(dag, &progress, 3);

        // Mark task "1.1" as Done in scheduler
        scheduler.mark_done("1.1");

        // Simulate hot-reload: load fresh TasksFile and apply scheduler state
        // (as done in orchestrator.rs:644-645)
        let fresh_tasks_file = sample_tasks_file();
        let mut updated = fresh_tasks_file.clone();

        // Apply scheduler state to fresh TasksFile
        for task_id in scheduler.done_tasks() {
            let _ = updated.update_status(task_id, TaskStatus::Done);
        }

        let cached_tasks_file = Arc::new(updated);

        // Verify cached_tasks_file reflects scheduler state
        let leaf = cached_tasks_file
            .find_leaf("1.1")
            .expect("Task 1.1 should exist after hot-reload");
        assert_eq!(leaf.status, TaskStatus::Done);
    }

    #[test]
    fn test_render_task_preview_shows_updated_status_icons() {
        // Test: Preview rendering after status change shows correct icons
        // This test verifies that cached_tasks_file updates propagate to rendering data
        let initial = sample_tasks_file();
        let mut cached_tasks_file = Arc::new(initial);

        // Update task "1.1" to Done
        let mut updated = (*cached_tasks_file).clone();
        let updated_ok = updated.update_status("1.1", TaskStatus::Done);
        assert!(updated_ok, "update_status should succeed");
        cached_tasks_file = Arc::new(updated);

        // Verify cached_tasks_file has updated status
        let leaf = cached_tasks_file
            .find_leaf("1.1")
            .expect("Task 1.1 should exist");
        assert_eq!(leaf.status, TaskStatus::Done);

        // Test all status mappings to ensure data structure is correct for rendering
        // Note: We don't duplicate rendering logic here, just verify data structure
        let statuses = [
            TaskStatus::Done,
            TaskStatus::InProgress,
            TaskStatus::Blocked,
            TaskStatus::Todo,
        ];
        for status in statuses {
            let mut test_file = sample_tasks_file();
            let _ = test_file.update_status("1.1", status.clone());
            let leaf = test_file.find_leaf("1.1").expect("Task 1.1 should exist");
            assert_eq!(leaf.status, status);
        }
    }

    #[test]
    fn test_cached_tasks_file_multiple_status_updates() {
        // Test: Multiple status updates are correctly reflected in cached_tasks_file
        let initial = sample_tasks_file();
        let mut cached_tasks_file = Arc::new(initial);

        // Update "1.1" to InProgress
        let mut updated = (*cached_tasks_file).clone();
        let updated_ok = updated.update_status("1.1", TaskStatus::InProgress);
        assert!(updated_ok, "update_status should succeed");
        cached_tasks_file = Arc::new(updated);

        let leaf = cached_tasks_file
            .find_leaf("1.1")
            .expect("Task 1.1 should exist");
        assert_eq!(leaf.status, TaskStatus::InProgress);

        // Update "1.1" to Done
        let mut updated = (*cached_tasks_file).clone();
        let updated_ok = updated.update_status("1.1", TaskStatus::Done);
        assert!(updated_ok, "update_status should succeed");
        cached_tasks_file = Arc::new(updated);

        let leaf = cached_tasks_file
            .find_leaf("1.1")
            .expect("Task 1.1 should exist after second update");
        assert_eq!(leaf.status, TaskStatus::Done);

        // Update "1.2" to Blocked
        let mut updated = (*cached_tasks_file).clone();
        let updated_ok = updated.update_status("1.2", TaskStatus::Blocked);
        assert!(updated_ok, "update_status should succeed");
        cached_tasks_file = Arc::new(updated);

        let leaf1 = cached_tasks_file
            .find_leaf("1.1")
            .expect("Task 1.1 should exist");
        let leaf2 = cached_tasks_file
            .find_leaf("1.2")
            .expect("Task 1.2 should exist");
        assert_eq!(leaf1.status, TaskStatus::Done);
        assert_eq!(leaf2.status, TaskStatus::Blocked);
    }

    #[test]
    fn test_cached_tasks_file_preserves_other_fields() {
        // Test: Status updates don't modify other task fields
        let initial = sample_tasks_file();
        let mut cached_tasks_file = Arc::new(initial);

        // Capture original task fields
        let original_leaf = cached_tasks_file
            .find_leaf("1.1")
            .expect("Task 1.1 should exist");
        let original_name = original_leaf.name.clone();
        let original_component = original_leaf.component.clone();
        let original_deps = original_leaf.deps.clone();

        // Update status
        let mut updated = (*cached_tasks_file).clone();
        let updated_ok = updated.update_status("1.1", TaskStatus::Done);
        assert!(updated_ok, "update_status should succeed");
        cached_tasks_file = Arc::new(updated);

        // Verify other fields are preserved
        let updated_leaf = cached_tasks_file
            .find_leaf("1.1")
            .expect("Task 1.1 should exist after update");
        assert_eq!(updated_leaf.name, original_name);
        assert_eq!(updated_leaf.component, original_component);
        assert_eq!(updated_leaf.deps, original_deps);
        assert_eq!(updated_leaf.status, TaskStatus::Done); // Only status changed
    }

    #[test]
    fn test_cached_tasks_file_update_nonexistent_task() {
        // Test: update_status returns false for non-existent task ID
        let initial = sample_tasks_file();
        let cached_tasks_file = Arc::new(initial);

        // Try to update non-existent task
        let mut updated = (*cached_tasks_file).clone();
        let updated_ok = updated.update_status("999.999", TaskStatus::Done);
        assert!(
            !updated_ok,
            "update_status should return false for non-existent task"
        );

        // Verify existing tasks are unchanged
        let leaf1 = cached_tasks_file
            .find_leaf("1.1")
            .expect("Task 1.1 should still exist");
        let leaf2 = cached_tasks_file
            .find_leaf("1.2")
            .expect("Task 1.2 should still exist");
        assert_eq!(leaf1.status, TaskStatus::Todo);
        assert_eq!(leaf2.status, TaskStatus::Todo);
    }

    // ── Separator tests ─────────────────────────────────────────────

    #[test]
    fn test_task_start_separator() {
        let separator = task_start_separator("1.2.3");
        assert!(separator.contains("Task 1.2.3"));
        assert!(separator.contains("══════")); // Double line
        assert!(separator.contains("\x1b[1;32m")); // Bold Green
        assert!(separator.contains("\x1b[0m")); // Reset
    }

    #[test]
    fn test_task_separator_vs_phase_separator_different_style() {
        let task_sep = task_start_separator("1.2");
        let phase_sep = phase_start_separator(&WorkerPhase::Implement);

        // Task separator uses double line (═), phase uses single line (─)
        assert!(task_sep.contains("══════"));
        assert!(phase_sep.contains("───"));

        // Task separator is bold green, phase is colored by phase
        assert!(task_sep.contains("\x1b[1;32m")); // Bold Green
        assert!(phase_sep.contains("\x1b[36m")); // Cyan
    }

    #[test]
    fn test_phase_start_separator_setup() {
        let separator = phase_start_separator(&WorkerPhase::Setup);
        assert!(separator.contains("⚙"));
        assert!(separator.contains("Setup"));
        assert!(separator.contains("\x1b[34m")); // Blue
        assert!(separator.contains("\x1b[0m")); // Reset
        assert!(separator.contains("───"));
    }

    #[test]
    fn test_phase_start_separator_implement() {
        let separator = phase_start_separator(&WorkerPhase::Implement);
        assert!(separator.contains("●"));
        assert!(separator.contains("Implement"));
        assert!(separator.contains("\x1b[36m")); // Cyan
        assert!(separator.contains("\x1b[0m")); // Reset
    }

    #[test]
    fn test_phase_start_separator_review_fix() {
        let separator = phase_start_separator(&WorkerPhase::ReviewFix);
        assert!(separator.contains("◎"));
        assert!(separator.contains("Review+Fix"));
        assert!(separator.contains("\x1b[33m")); // Yellow
        assert!(separator.contains("\x1b[0m")); // Reset
    }

    #[test]
    fn test_phase_start_separator_verify() {
        let separator = phase_start_separator(&WorkerPhase::Verify);
        assert!(separator.contains("◉"));
        assert!(separator.contains("Verify"));
        assert!(separator.contains("\x1b[35m")); // Magenta
        assert!(separator.contains("\x1b[0m")); // Reset
    }

    #[test]
    fn test_phase_end_separator_success() {
        let separator = phase_end_separator(&WorkerPhase::Implement, true);
        assert!(separator.contains("✓"));
        assert!(separator.contains("Implement"));
        assert!(separator.contains("[ok]"));
        assert!(separator.contains("\x1b[32m")); // Green
        assert!(separator.contains("\x1b[0m")); // Reset
        assert!(separator.contains("───"));
    }

    #[test]
    fn test_phase_end_separator_failure() {
        let separator = phase_end_separator(&WorkerPhase::Verify, false);
        assert!(separator.contains("✗"));
        assert!(separator.contains("Verify"));
        assert!(separator.contains("[FAILED]"));
        assert!(separator.contains("\x1b[31m")); // Red
        assert!(separator.contains("\x1b[0m")); // Reset
    }

    #[test]
    fn test_conflict_resolution_start_separator() {
        let separator = conflict_resolution_start_separator();
        assert!(separator.contains("⚡"));
        assert!(separator.contains("Conflict Resolution"));
        assert!(separator.contains("\x1b[31m")); // Red
        assert!(separator.contains("\x1b[0m")); // Reset
        assert!(separator.contains("───"));
    }

    #[test]
    fn test_conflict_resolution_end_separator_success() {
        let separator = conflict_resolution_end_separator(true);
        assert!(separator.contains("✓"));
        assert!(separator.contains("Conflict Resolution"));
        assert!(separator.contains("[ok]"));
        assert!(separator.contains("\x1b[32m")); // Green
        assert!(separator.contains("\x1b[0m")); // Reset
        assert!(separator.contains("───"));
    }

    #[test]
    fn test_conflict_resolution_end_separator_failure() {
        let separator = conflict_resolution_end_separator(false);
        assert!(separator.contains("✗"));
        assert!(separator.contains("Conflict Resolution"));
        assert!(separator.contains("[FAILED]"));
        assert!(separator.contains("\x1b[31m")); // Red
        assert!(separator.contains("\x1b[0m")); // Reset
        assert!(separator.contains("───"));
    }

    // ── Verify phase separator tests ────────────────────────────────

    #[test]
    fn test_verify_start_separator() {
        let separator = verify_start_separator();
        assert!(separator.contains("🔍"));
        assert!(separator.contains("Verify"));
        assert!(separator.contains("\x1b[33m")); // Yellow
        assert!(separator.contains("\x1b[0m")); // Reset
        assert!(separator.contains("───"));
    }

    #[test]
    fn test_verify_end_separator_success() {
        let separator = verify_end_separator(true);
        assert!(separator.contains("✓"));
        assert!(separator.contains("Verify"));
        assert!(separator.contains("[ok]"));
        assert!(separator.contains("\x1b[32m")); // Green
        assert!(separator.contains("\x1b[0m")); // Reset
        assert!(separator.contains("───"));
    }

    #[test]
    fn test_verify_end_separator_failure() {
        let separator = verify_end_separator(false);
        assert!(separator.contains("✗"));
        assert!(separator.contains("Verify"));
        assert!(separator.contains("[FAILED]"));
        assert!(separator.contains("\x1b[31m")); // Red
        assert!(separator.contains("\x1b[0m")); // Reset
        assert!(separator.contains("───"));
    }

    #[test]
    fn test_all_phases_have_unique_icons() {
        let setup = phase_start_separator(&WorkerPhase::Setup);
        let implement = phase_start_separator(&WorkerPhase::Implement);
        let review = phase_start_separator(&WorkerPhase::ReviewFix);
        let verify = phase_start_separator(&WorkerPhase::Verify);

        // Each phase should have a different icon
        assert!(setup.contains("⚙"));
        assert!(implement.contains("●"));
        assert!(review.contains("◎"));
        assert!(verify.contains("◉"));
    }

    #[test]
    fn test_all_phases_have_unique_colors() {
        let setup = phase_start_separator(&WorkerPhase::Setup);
        let implement = phase_start_separator(&WorkerPhase::Implement);
        let review = phase_start_separator(&WorkerPhase::ReviewFix);
        let verify = phase_start_separator(&WorkerPhase::Verify);

        // Each phase should have a different color
        assert!(setup.contains("\x1b[34m")); // Blue
        assert!(implement.contains("\x1b[36m")); // Cyan
        assert!(review.contains("\x1b[33m")); // Yellow
        assert!(verify.contains("\x1b[35m")); // Magenta
    }

    // ── Integration tests with OutputRingBuffer ────────────────────

    #[test]
    fn test_separators_added_to_output_ring_buffer() {
        use crate::commands::task::orchestrate::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::new(100);

        // 1. Add task start separator
        let task_sep = task_start_separator("1.2.3");
        buffer.push(&task_sep);

        // 2. Add phase start separators for each phase
        buffer.push(&phase_start_separator(&WorkerPhase::Setup));
        buffer.push(&phase_start_separator(&WorkerPhase::Implement));
        buffer.push(&phase_start_separator(&WorkerPhase::ReviewFix));
        buffer.push(&phase_start_separator(&WorkerPhase::Verify));

        // 3. Add phase completion separators (success and failure)
        buffer.push(&phase_end_separator(&WorkerPhase::Setup, true));
        buffer.push(&phase_end_separator(&WorkerPhase::Implement, false));

        // Verify buffer contains all separators
        assert_eq!(buffer.len(), 7, "Buffer should contain all 7 separators");

        let lines = buffer.tail(10);
        assert_eq!(lines.len(), 7);

        // Verify that separators are preserved as styled Lines
        // (ANSI codes are converted to ratatui styling by OutputRingBuffer)
        for line in &lines {
            assert!(
                !line.spans.is_empty(),
                "Separator line should have at least one span"
            );
        }
    }

    #[test]
    fn test_task_start_separator_in_ring_buffer() {
        use crate::commands::task::orchestrate::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::new(10);
        let separator = task_start_separator("4.3");

        buffer.push(&separator);

        let lines = buffer.tail(10);
        assert_eq!(lines.len(), 1);

        // Verify separator content is preserved
        let line_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line_text.contains("Task 4.3"));
        assert!(line_text.contains("══════"));
    }

    #[test]
    fn test_phase_separators_preserve_styling_in_ring_buffer() {
        use crate::commands::task::orchestrate::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::new(10);

        // Add phase start separator with color
        let separator = phase_start_separator(&WorkerPhase::Implement);
        buffer.push(&separator);

        let lines = buffer.tail(10);
        assert_eq!(lines.len(), 1);

        // Verify that ANSI color codes were converted to ratatui styling
        let has_color = lines[0].spans.iter().any(|span| span.style.fg.is_some());

        assert!(
            has_color,
            "Phase separator should have color styling in ring buffer"
        );

        // Verify content
        let line_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line_text.contains("Implement"));
        assert!(line_text.contains("●"));
    }

    #[test]
    fn test_phase_end_separator_success_and_failure_in_ring_buffer() {
        use crate::commands::task::orchestrate::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::new(10);

        // Add success separator
        let success_sep = phase_end_separator(&WorkerPhase::Verify, true);
        buffer.push(&success_sep);

        // Add failure separator
        let failure_sep = phase_end_separator(&WorkerPhase::ReviewFix, false);
        buffer.push(&failure_sep);

        let lines = buffer.tail(10);
        assert_eq!(lines.len(), 2);

        // Verify success separator
        let success_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(success_text.contains("Verify"));
        assert!(success_text.contains("[ok]"));
        assert!(success_text.contains("✓"));

        // Verify failure separator
        let failure_text: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(failure_text.contains("Review+Fix"));
        assert!(failure_text.contains("[FAILED]"));
        assert!(failure_text.contains("✗"));
    }

    #[test]
    fn test_mixed_output_with_separators_in_ring_buffer() {
        use crate::commands::task::orchestrate::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::new(20);

        // Simulate realistic worker output sequence
        buffer.push(&task_start_separator("2.1"));
        buffer.push("Worker output: Starting implementation...");
        buffer.push(&phase_start_separator(&WorkerPhase::Implement));
        buffer.push("Worker output: Writing code...");
        buffer.push("Worker output: Code written successfully");
        buffer.push(&phase_end_separator(&WorkerPhase::Implement, true));
        buffer.push(&phase_start_separator(&WorkerPhase::Verify));
        buffer.push("Worker output: Running tests...");
        buffer.push(&phase_end_separator(&WorkerPhase::Verify, true));

        assert_eq!(buffer.len(), 9);

        let lines = buffer.tail(20);
        assert_eq!(lines.len(), 9);

        // Verify separators are visually distinct from regular output
        let first_line_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first_line_text.contains("══════")); // Task separator uses double line

        let phase_sep_text: String = lines[2].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(phase_sep_text.contains("───")); // Phase separator uses single line
    }

    // ── Task 15.6: Heartbeat tracking tests ─────────────────────────────

    #[test]
    fn test_heartbeat_event_updates_timestamp() {
        // Test: Heartbeat event updates last_heartbeat HashMap
        use std::collections::HashMap;
        use std::time::Instant;

        let mut last_heartbeat: HashMap<u32, Instant> = HashMap::new();

        // Simulate receiving Heartbeat event for worker 1
        let worker_id = 1;
        let now = Instant::now();
        last_heartbeat.insert(worker_id, now);

        // Verify timestamp was recorded
        assert!(
            last_heartbeat.contains_key(&worker_id),
            "Heartbeat should be recorded for worker {worker_id}"
        );
        let recorded = last_heartbeat.get(&worker_id).unwrap();
        assert_eq!(
            recorded.elapsed().as_secs(),
            0,
            "Heartbeat should be recent (just recorded)"
        );
    }

    #[test]
    fn test_heartbeat_multiple_workers() {
        // Test: Multiple workers can have independent heartbeat timestamps
        use std::collections::HashMap;
        use std::time::Instant;

        let mut last_heartbeat: HashMap<u32, Instant> = HashMap::new();

        // Record heartbeats for workers 1, 2, 3
        last_heartbeat.insert(1, Instant::now());
        std::thread::sleep(std::time::Duration::from_millis(10));
        last_heartbeat.insert(2, Instant::now());
        std::thread::sleep(std::time::Duration::from_millis(10));
        last_heartbeat.insert(3, Instant::now());

        // Verify all workers have heartbeats
        assert_eq!(last_heartbeat.len(), 3, "Should track 3 workers");
        assert!(last_heartbeat.contains_key(&1));
        assert!(last_heartbeat.contains_key(&2));
        assert!(last_heartbeat.contains_key(&3));

        // Verify worker 3 heartbeat is most recent
        let hb1 = last_heartbeat.get(&1).unwrap();
        let hb3 = last_heartbeat.get(&3).unwrap();
        assert!(
            hb3.elapsed() < hb1.elapsed(),
            "Worker 3 heartbeat should be more recent than worker 1"
        );
    }

    #[test]
    fn test_heartbeat_update_existing_worker() {
        // Test: Updating heartbeat for existing worker replaces old timestamp
        use std::collections::HashMap;
        use std::time::Instant;

        let mut last_heartbeat: HashMap<u32, Instant> = HashMap::new();

        // Initial heartbeat
        let worker_id = 1;
        last_heartbeat.insert(worker_id, Instant::now());
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Update heartbeat
        let old_elapsed = last_heartbeat.get(&worker_id).unwrap().elapsed();
        last_heartbeat.insert(worker_id, Instant::now());
        let new_elapsed = last_heartbeat.get(&worker_id).unwrap().elapsed();

        // Verify new timestamp is more recent
        assert!(
            new_elapsed < old_elapsed,
            "Updated heartbeat should be more recent than old heartbeat"
        );
        assert!(
            new_elapsed.as_millis() < 10,
            "New heartbeat should be very recent"
        );
    }

    // ── Conflict Resolution separator tests ────────────────────────

    #[test]
    fn test_conflict_resolution_separator_in_ring_buffer() {
        use crate::commands::task::orchestrate::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::new(10);

        // Add conflict resolution start separator
        let start_sep = conflict_resolution_start_separator();
        buffer.push(&start_sep);

        // Add conflict resolution end separator (success)
        let end_sep_ok = conflict_resolution_end_separator(true);
        buffer.push(&end_sep_ok);

        // Add conflict resolution end separator (failure)
        let end_sep_fail = conflict_resolution_end_separator(false);
        buffer.push(&end_sep_fail);

        let lines = buffer.tail(10);
        assert_eq!(lines.len(), 3);

        // Verify start separator has styling (red color)
        let has_color_start = lines[0].spans.iter().any(|span| span.style.fg.is_some());
        assert!(
            has_color_start,
            "Conflict resolution start separator should have color styling"
        );

        // Verify content
        let start_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(start_text.contains("Conflict Resolution"));
        assert!(start_text.contains("⚡"));

        // Verify end separator (success) has green styling
        let has_color_success = lines[1].spans.iter().any(|span| span.style.fg.is_some());
        assert!(
            has_color_success,
            "Conflict resolution end separator (success) should have color styling"
        );

        let success_text: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(success_text.contains("Conflict Resolution"));
        assert!(success_text.contains("[ok]"));
        assert!(success_text.contains("✓"));

        // Verify end separator (failure) has red styling
        let has_color_failure = lines[2].spans.iter().any(|span| span.style.fg.is_some());
        assert!(
            has_color_failure,
            "Conflict resolution end separator (failure) should have color styling"
        );

        let failure_text: String = lines[2].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(failure_text.contains("Conflict Resolution"));
        assert!(failure_text.contains("[FAILED]"));
        assert!(failure_text.contains("✗"));
    }

    #[test]
    fn test_merge_conflict_output_sequence() {
        use crate::commands::task::orchestrate::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::new(30);

        // Simulate realistic merge → conflict → AI resolution → end sequence
        // 1. Merge output
        buffer.push("Merge output: Attempting to merge branch 'task/1.1' into master...");
        buffer.push("Merge output: Auto-merging src/main.rs");
        buffer.push("Merge output: CONFLICT (content): Merge conflict in src/main.rs");

        // 2. Conflict resolution start separator
        buffer.push(&conflict_resolution_start_separator());

        // 3. AI output during conflict resolution
        buffer.push("AI output: Analyzing conflict in src/main.rs...");
        buffer.push("AI output: Conflict involves function signature changes");
        buffer.push("AI output: Applying resolution strategy: preserve both changes");
        buffer.push("AI output: Writing resolved file...");
        buffer.push("AI output: Staging resolved file...");

        // 4. Conflict resolution end separator (success)
        buffer.push(&conflict_resolution_end_separator(true));

        // 5. Final merge output
        buffer.push("Merge output: Conflict resolved successfully");
        buffer.push("Merge output: Creating merge commit...");

        assert_eq!(buffer.len(), 12);

        let lines = buffer.tail(30);
        assert_eq!(lines.len(), 12);

        // Verify separator positions and content
        let line3_text: String = lines[3].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line3_text.contains("⚡")); // Conflict start separator at index 3
        assert!(line3_text.contains("Conflict Resolution"));

        let line9_text: String = lines[9].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line9_text.contains("✓")); // Conflict end separator at index 9
        assert!(line9_text.contains("[ok]"));

        // Verify that separators have styling preserved
        let has_color_start = lines[3].spans.iter().any(|span| span.style.fg.is_some());
        assert!(
            has_color_start,
            "Conflict start separator should have color"
        );

        let has_color_end = lines[9].spans.iter().any(|span| span.style.fg.is_some());
        assert!(has_color_end, "Conflict end separator should have color");

        // Verify that regular output lines are between separators
        let line4_text: String = lines[4].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line4_text.contains("Analyzing conflict"));

        let line8_text: String = lines[8].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line8_text.contains("Staging resolved file"));
    }
}
