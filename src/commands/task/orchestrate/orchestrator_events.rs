use std::sync::Arc;
use std::time::Duration;

use crate::commands::task::orchestrate::events::{ProfileStatus, WorkerEventKind, WorkerPhase};
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

// ── Helper functions ────────────────────────────────────────────────

/// Set worker to idle state.
///
/// Consolidates the pattern of setting worker to idle in orchestrator events.
fn set_worker_idle(ctx: &mut RunLoopContext, worker_id: u32) {
    // Update app status to idle
    ctx.tui
        .app
        .update_worker_status(worker_id, WorkerStatus::idle(worker_id));

    // Clear multiplexed output
    ctx.tui.mux_output.clear_worker(worker_id);
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

// ── Shared helpers for event handlers ───────────────────────────────

/// Parameters for building a `WorkerStatus`.
struct WorkerStatusParams<'a> {
    state: WorkerState,
    task_id: &'a str,
    component: Option<String>,
    phase: Option<WorkerPhase>,
    model: Option<&'a String>,
    cost_usd: f64,
    input_tokens: u64,
    output_tokens: u64,
    verify_profiles: Vec<(String, Option<bool>)>,
}

/// Build a `WorkerStatus` from grouped parameters.
/// Reduces repeated struct construction across multiple handlers.
fn build_worker_status(p: WorkerStatusParams) -> WorkerStatus {
    WorkerStatus {
        state: p.state,
        task_id: Some(p.task_id.to_string()),
        component: p.component,
        phase: p.phase,
        model: p
            .model
            .map(|m| crate::shared::tasks::shorten_model_name(m)),
        cost_usd: p.cost_usd,
        input_tokens: p.input_tokens,
        output_tokens: p.output_tokens,
        verify_profiles: p.verify_profiles,
    }
}

/// Update `cached_tasks_file` with new task status and sync `progress_mtime`.
/// Returns true if the update was applied (tasks_file write succeeded).
fn sync_task_status(
    orchestrator: &Orchestrator,
    ctx: &mut RunLoopContext,
    task_id: &str,
    status: TaskStatus,
) {
    if let Some(mt) = orchestrator.update_tasks_file(task_id, status.clone(), ctx.tui) {
        ctx.progress_mtime = Some(mt);
        let mut updated = (*ctx.cached_tasks_file).clone();
        let _ = updated.update_status(task_id, status);
        ctx.cached_tasks_file = Arc::new(updated);
    }
}

/// Look up a task's component from the cached_tasks_file.
fn lookup_component(ctx: &RunLoopContext, task_id: &str) -> Option<String> {
    ctx.cached_tasks_file
        .as_ref()
        .find_leaf(task_id)
        .map(|leaf| leaf.component.clone())
}

// ── Event handling (dispatcher) ─────────────────────────────────────

impl Orchestrator {
    /// Handle a worker event from the mpsc channel.
    ///
    /// Dispatches each `WorkerEventKind` variant to a dedicated handler method.
    pub(super) async fn handle_event(
        &self,
        event: &crate::commands::task::orchestrate::events::WorkerEvent,
        ctx: &mut RunLoopContext<'_>,
    ) -> Result<()> {
        // Log event dispatch for debugging
        let event_kind = match &event.kind {
            WorkerEventKind::TaskStarted { worker_id, task_id } => {
                format!("TaskStarted(worker={}, task={})", worker_id, task_id)
            }
            WorkerEventKind::PhaseStarted {
                worker_id,
                task_id,
                phase,
                profiles,
            } => {
                let profile_info = if let Some(p) = profiles {
                    format!(" profiles=[{}]", p.join(", "))
                } else {
                    String::new()
                };
                format!(
                    "PhaseStarted(worker={}, task={}, phase={:?}{})",
                    worker_id, task_id, phase, profile_info
                )
            }
            WorkerEventKind::PhaseCompleted {
                worker_id,
                phase,
                success,
                profile_results,
                ..
            } => {
                let profile_info = if let Some(results) = profile_results {
                    let summary: Vec<String> = results
                        .iter()
                        .map(|r| {
                            let status = match r.success {
                                Some(true) => "✓",
                                Some(false) => "✗",
                                None => "⏳",
                            };
                            format!("{} {}", r.name, status)
                        })
                        .collect();
                    format!(" profiles=[{}]", summary.join(", "))
                } else {
                    String::new()
                };

                format!(
                    "PhaseCompleted(worker={}, phase={:?}, success={}{})",
                    worker_id, phase, success, profile_info
                )
            }
            WorkerEventKind::CostUpdate {
                worker_id,
                cost_usd,
                ..
            } => {
                format!("CostUpdate(worker={}, cost=${:.4})", worker_id, cost_usd)
            }
            WorkerEventKind::TaskCompleted {
                worker_id,
                task_id,
                success,
                ..
            } => {
                format!(
                    "TaskCompleted(worker={}, task={}, success={})",
                    worker_id, task_id, success
                )
            }
            WorkerEventKind::TaskFailed {
                worker_id, task_id, ..
            } => {
                format!("TaskFailed(worker={}, task={})", worker_id, task_id)
            }
            WorkerEventKind::OutputLines { worker_id, lines } => {
                format!("OutputLines(worker={}, lines={})", worker_id, lines.len())
            }
            WorkerEventKind::MergeStepOutput { worker_id, lines } => {
                format!(
                    "MergeStepOutput(worker={}, lines={})",
                    worker_id,
                    lines.len()
                )
            }
            WorkerEventKind::Heartbeat { worker_id, phase } => {
                format!("Heartbeat(worker={}, phase={:?})", worker_id, phase)
            }
            WorkerEventKind::MergeStarted { worker_id, task_id } => {
                format!("MergeStarted(worker={}, task={})", worker_id, task_id)
            }
            WorkerEventKind::MergeCompleted {
                worker_id,
                task_id,
                success,
                ..
            } => {
                format!(
                    "MergeCompleted(worker={}, task={}, success={})",
                    worker_id, task_id, success
                )
            }
            WorkerEventKind::MergeConflict {
                worker_id, task_id, ..
            } => {
                format!("MergeConflict(worker={}, task={})", worker_id, task_id)
            }
            WorkerEventKind::UserMessageSent { worker_id, .. } => {
                format!("UserMessageSent(worker={})", worker_id)
            }
            WorkerEventKind::UserMessageReceived { worker_id, .. } => {
                format!("UserMessageReceived(worker={})", worker_id)
            }
        };
        crate::diag_debug!("Orchestrator event: {}", event_kind);

        match &event.kind {
            WorkerEventKind::TaskStarted { worker_id, task_id } => {
                self.handle_task_started(*worker_id, task_id, ctx);
            }
            WorkerEventKind::PhaseStarted {
                worker_id,
                task_id,
                phase,
                profiles,
            } => {
                self.handle_phase_started(*worker_id, task_id, phase, profiles.clone(), ctx);
            }
            WorkerEventKind::PhaseCompleted {
                worker_id,
                task_id,
                phase,
                success,
                profile_results,
            } => {
                Self::handle_phase_completed(
                    *worker_id,
                    task_id,
                    phase,
                    *success,
                    profile_results.clone(),
                    ctx,
                );
            }
            WorkerEventKind::CostUpdate {
                worker_id,
                cost_usd,
                input_tokens,
                output_tokens,
            } => {
                Self::handle_cost_update(*worker_id, *cost_usd, *input_tokens, *output_tokens, ctx);
            }
            WorkerEventKind::TaskCompleted {
                worker_id,
                task_id,
                success,
                cost_usd,
                ..
            } => {
                self.handle_task_completed(*worker_id, task_id, *success, *cost_usd, ctx)
                    .await;
            }
            WorkerEventKind::TaskFailed {
                worker_id,
                task_id,
                error,
                retries_left,
            } => {
                Self::handle_task_failed(*worker_id, task_id, error, *retries_left, ctx);
            }
            WorkerEventKind::MergeStarted { worker_id, task_id } => {
                self.handle_merge_started(*worker_id, task_id, ctx);
            }
            WorkerEventKind::MergeCompleted {
                worker_id,
                task_id,
                success,
                commit_hash,
            } => {
                self.handle_merge_completed(
                    *worker_id,
                    task_id,
                    *success,
                    commit_hash.as_deref(),
                    ctx,
                )
                .await;
            }
            WorkerEventKind::MergeConflict {
                worker_id,
                task_id,
                conflicting_files,
            } => {
                self.handle_merge_conflict(*worker_id, task_id, conflicting_files, ctx);
            }
            WorkerEventKind::OutputLines { worker_id, lines } => {
                ctx.tui.app.push_worker_output(*worker_id, lines);
            }
            WorkerEventKind::MergeStepOutput { worker_id, lines } => {
                ctx.tui.app.push_worker_output(*worker_id, lines);
            }
            WorkerEventKind::Heartbeat { worker_id, .. } => {
                ctx.last_heartbeat
                    .insert(*worker_id, std::time::Instant::now());
            }
            WorkerEventKind::UserMessageSent { worker_id, message } => {
                Self::handle_user_message_sent(*worker_id, message, ctx);
            }
            WorkerEventKind::UserMessageReceived { worker_id, message } => {
                Self::handle_user_message_received(*worker_id, message, ctx);
            }
        }

        Ok(())
    }

    // ── Per-event handler methods ───────────────────────────────────

    /// Worker started processing a new task — initialize TUI and heartbeat.
    fn handle_task_started(&self, worker_id: u32, task_id: &str, ctx: &mut RunLoopContext) {
        ctx.tui.app.clear_worker_output(worker_id);

        let separator = task_start_separator(task_id);
        ctx.tui.app.push_worker_output(worker_id, &[separator]);

        ctx.last_heartbeat
            .insert(worker_id, std::time::Instant::now());

        let model = self.resolve_model(task_id, ctx.cached_tasks_file.as_ref());
        let ws = build_worker_status(WorkerStatusParams {
            state: WorkerState::Implementing,
            task_id,
            component: lookup_component(ctx, task_id),
            phase: Some(WorkerPhase::Implement),
            model: model.as_ref(),
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            verify_profiles: Vec::new(),
        });
        ctx.tui.app.update_worker_status(worker_id, ws);

        let msg = ctx
            .tui
            .mux_output
            .format_worker_line(worker_id, &format!("Started: {task_id}"));
        ctx.tui.app.push_log_line(&msg);
    }

    /// Worker entered a new execution phase — update dashboard and log.
    fn handle_phase_started(
        &self,
        worker_id: u32,
        task_id: &str,
        phase: &WorkerPhase,
        profiles: Option<Vec<String>>,
        ctx: &mut RunLoopContext,
    ) {
        let new_state = match phase {
            WorkerPhase::Setup => WorkerState::SettingUp,
            WorkerPhase::Implement => WorkerState::Implementing,
            WorkerPhase::ReviewFix => WorkerState::Reviewing,
            WorkerPhase::Verify => WorkerState::Verifying,
        };
        let (cost, input, output) = ctx.tui.mux_output.worker_cost(worker_id);

        // Use review_model for ReviewFix phase, task's model for others
        let model = if matches!(phase, WorkerPhase::ReviewFix) {
            Some(self.config.review_model.clone())
        } else {
            self.resolve_model(task_id, ctx.cached_tasks_file.as_ref())
        };

        // For Verify phase, initialize profiles with in-progress status (None)
        let verify_profiles = if matches!(phase, WorkerPhase::Verify) {
            profiles
                .unwrap_or_default()
                .into_iter()
                .map(|name| (name, None))
                .collect()
        } else {
            Vec::new()
        };

        let ws = build_worker_status(WorkerStatusParams {
            state: new_state,
            task_id,
            component: lookup_component(ctx, task_id),
            phase: Some(phase.clone()),
            model: model.as_ref(),
            cost_usd: cost,
            input_tokens: input,
            output_tokens: output,
            verify_profiles,
        });
        ctx.tui.app.update_worker_status(worker_id, ws);

        let msg = ctx
            .tui
            .mux_output
            .format_worker_line(worker_id, &format!("{task_id} → phase: {phase}"));
        ctx.tui.app.push_log_line(&msg);

        let separator = phase_start_separator(phase);
        ctx.tui.app.push_worker_output(worker_id, &[separator]);
    }

    /// Worker finished an execution phase — log result, update profile statuses, add separator.
    fn handle_phase_completed(
        worker_id: u32,
        task_id: &str,
        phase: &WorkerPhase,
        success: bool,
        profile_results: Option<Vec<ProfileStatus>>,
        ctx: &mut RunLoopContext,
    ) {
        // Update verify_profiles on dashboard with final results
        if let Some(results) = &profile_results {
            let profiles = results
                .iter()
                .map(|r| (r.name.clone(), r.success))
                .collect();
            ctx.tui.app.update_verify_profiles(worker_id, profiles);
        }

        let status_text = if success { "ok" } else { "FAILED" };
        let msg = ctx.tui.mux_output.format_worker_line(
            worker_id,
            &format!("{task_id} ← phase: {phase} [{status_text}]"),
        );
        ctx.tui.app.push_log_line(&msg);

        let separator = phase_end_separator(phase, success);
        ctx.tui.app.push_worker_output(worker_id, &[separator]);
    }

    /// Incremental cost update from a worker — propagate to mux_output and dashboard.
    fn handle_cost_update(
        worker_id: u32,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
        ctx: &mut RunLoopContext,
    ) {
        ctx.tui
            .mux_output
            .update_cost(worker_id, cost_usd, input_tokens, output_tokens);
        let (total_cost, total_in, total_out) = ctx.tui.mux_output.worker_cost(worker_id);
        ctx.tui
            .app
            .update_worker_cost(worker_id, total_cost, total_in, total_out);
    }

    /// Task completed (success or failure) — handle merge, retry, or finalization.
    async fn handle_task_completed(
        &self,
        worker_id: u32,
        task_id: &str,
        success: bool,
        cost_usd: f64,
        ctx: &mut RunLoopContext<'_>,
    ) {
        // Wait for the worker's join handle to complete
        if let Some(handle) = ctx.join_handles.remove(&worker_id) {
            let _ = handle.await;
        }

        let duration = ctx
            .tui
            .task_start_times
            .remove(task_id)
            .map(|s| s.elapsed())
            .unwrap_or_default();

        if success && !self.config.no_merge {
            self.handle_task_success_with_merge(worker_id, task_id, cost_usd, duration, ctx);
        } else if success {
            self.handle_task_success_no_merge(worker_id, task_id, cost_usd, duration, ctx);
        } else {
            self.handle_task_failure(worker_id, task_id, cost_usd, duration, ctx)
                .await;
        }
    }

    /// Task succeeded and merge is enabled — enqueue merge or handle edge cases.
    fn handle_task_success_with_merge(
        &self,
        worker_id: u32,
        task_id: &str,
        cost_usd: f64,
        duration: Duration,
        ctx: &mut RunLoopContext,
    ) {
        if let Some(WorkerSlot::Busy { worktree, .. }) = ctx.worker_slots.get(&worker_id) {
            let component = lookup_component(ctx, task_id);
            let (cost, input, output) = ctx.tui.mux_output.worker_cost(worker_id);
            let model = self.resolve_model(task_id, ctx.cached_tasks_file.as_ref());
            let ws = build_worker_status(WorkerStatusParams {
                state: WorkerState::Merging,
                task_id,
                component,
                phase: None,
                model: model.as_ref(),
                cost_usd: cost,
                input_tokens: input,
                output_tokens: output,
                verify_profiles: Vec::new(),
            });
            ctx.tui.app.update_worker_status(worker_id, ws);

            // Get task name from cached_tasks_file
            let task_name = ctx
                .cached_tasks_file
                .as_ref()
                .find_leaf(task_id)
                .map(|leaf| leaf.name.clone())
                .unwrap_or_else(|| task_id.to_string());

            let pending = PendingMerge {
                worker_id,
                task_id: task_id.to_string(),
                task_name,
                branch: worktree.branch.clone(),
            };

            if ctx.merge_ctx.merge_in_progress {
                let msg = ctx.tui.mux_output.format_worker_line(
                    worker_id,
                    &format!("Queued merge: {task_id} (another merge in progress)"),
                );
                ctx.tui.app.push_log_line(&msg);
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
            // Worker slot not Busy or task not in lookup — release worker
            let reason = if !matches!(
                ctx.worker_slots.get(&worker_id),
                Some(WorkerSlot::Busy { .. })
            ) {
                "worker slot not Busy"
            } else {
                "task not in lookup"
            };
            let msg = ctx.tui.mux_output.format_worker_line(
                worker_id,
                &format!("Cannot merge {task_id}: {reason} — releasing worker"),
            );
            ctx.tui.app.push_log_line(&msg);

            ctx.scheduler.mark_blocked(task_id);
            sync_task_status(self, ctx, task_id, TaskStatus::Blocked);

            ctx.tui.task_summaries.push(TaskSummaryEntry {
                task_id: task_id.to_string(),
                status: "Blocked".to_string(),
                cost_usd,
                duration,
                retries: ctx.scheduler.retry_count(task_id),
            });

            ctx.release_worker(worker_id);
            set_worker_idle(ctx, worker_id);
        }
    }

    /// Task succeeded in --no-merge mode — mark done and release worker.
    fn handle_task_success_no_merge(
        &self,
        worker_id: u32,
        task_id: &str,
        cost_usd: f64,
        duration: Duration,
        ctx: &mut RunLoopContext,
    ) {
        ctx.scheduler.mark_done(task_id);
        sync_task_status(self, ctx, task_id, TaskStatus::Done);

        let msg = ctx
            .tui
            .mux_output
            .format_worker_line(worker_id, &format!("Done (no merge): {task_id}"));
        ctx.tui.app.push_log_line(&msg);

        ctx.tui.task_summaries.push(TaskSummaryEntry {
            task_id: task_id.to_string(),
            status: "Done".to_string(),
            cost_usd,
            duration,
            retries: ctx.scheduler.retry_count(task_id),
        });

        ctx.release_worker(worker_id);
        set_worker_idle(ctx, worker_id);
    }

    /// Task failed — retry or block, cleanup worktree, then release worker.
    ///
    /// IMPORTANT: Cleanup worktree before releasing worker to avoid orphaned worktrees
    /// when worker fails. This ensures proper cleanup even after panic/error.
    async fn handle_task_failure(
        &self,
        worker_id: u32,
        task_id: &str,
        cost_usd: f64,
        duration: Duration,
        ctx: &mut RunLoopContext<'_>,
    ) {
        // Cleanup worktree before marking task failed/blocked (task 54.4)
        if let Some(WorkerSlot::Busy { worktree, .. }) = ctx.worker_slots.get(&worker_id) {
            ctx.worktree_manager
                .remove_worktree(&worktree.path)
                .await
                .ok();
            ctx.worktree_manager
                .remove_branch(&worktree.branch)
                .await
                .ok();
        }

        let requeued = ctx.scheduler.mark_failed(task_id);
        if requeued {
            let msg = ctx.tui.mux_output.format_worker_line(
                worker_id,
                &format!("Task {task_id} failed, re-queued for retry"),
            );
            ctx.tui.app.push_log_line(&msg);
        } else {
            let msg = ctx.tui.mux_output.format_worker_line(
                worker_id,
                &format!("Task {task_id} blocked after max retries"),
            );
            ctx.tui.app.push_log_line(&msg);
            sync_task_status(self, ctx, task_id, TaskStatus::Blocked);

            ctx.tui.task_summaries.push(TaskSummaryEntry {
                task_id: task_id.to_string(),
                status: "Blocked".to_string(),
                cost_usd,
                duration,
                retries: ctx.scheduler.retry_count(task_id),
            });
        }

        ctx.release_worker(worker_id);
        set_worker_idle(ctx, worker_id);
    }

    /// Task failed with an explicit error message — log it.
    fn handle_task_failed(
        worker_id: u32,
        task_id: &str,
        error: &str,
        retries_left: u32,
        ctx: &mut RunLoopContext,
    ) {
        let msg = if retries_left == 0 {
            ctx.tui.mux_output.format_worker_line(
                worker_id,
                &format!("Task {task_id} failed permanently: {error}"),
            )
        } else {
            ctx.tui.mux_output.format_worker_line(
                worker_id,
                &format!("Task {task_id} failed ({retries_left} retries left): {error}"),
            )
        };
        ctx.tui.app.push_log_line(&msg);
    }

    /// Merge process started for a completed task — update TUI.
    fn handle_merge_started(&self, worker_id: u32, task_id: &str, ctx: &mut RunLoopContext) {
        let (cost, input, output) = ctx.tui.mux_output.worker_cost(worker_id);
        let model = self.resolve_model(task_id, ctx.cached_tasks_file.as_ref());
        let ws = build_worker_status(WorkerStatusParams {
            state: WorkerState::Merging,
            task_id,
            component: lookup_component(ctx, task_id),
            phase: None,
            model: model.as_ref(),
            cost_usd: cost,
            input_tokens: input,
            output_tokens: output,
            verify_profiles: Vec::new(),
        });
        ctx.tui.app.update_worker_status(worker_id, ws);

        let msg = ctx
            .tui
            .mux_output
            .format_worker_line(worker_id, &format!("Merging: {task_id}"));
        ctx.tui.app.push_log_line(&msg);
    }

    /// Merge completed (success or failure) — finalize task, cleanup worktree, start next merge.
    async fn handle_merge_completed(
        &self,
        worker_id: u32,
        task_id: &str,
        success: bool,
        commit_hash: Option<&str>,
        ctx: &mut RunLoopContext<'_>,
    ) {
        // Wait for merge join handle
        if let Some(handle) = ctx.merge_ctx.merge_join_handle.take() {
            let _ = handle.await;
        }

        let hash = commit_hash.unwrap_or("???");
        let status_text = if success { "ok" } else { "FAILED" };
        let msg = ctx.tui.mux_output.format_worker_line(
            worker_id,
            &format!("Merge {task_id}: {status_text} ({hash})"),
        );
        ctx.tui.app.push_log_line(&msg);

        if success {
            self.handle_merge_success(worker_id, task_id, ctx).await;
        } else {
            self.handle_merge_failure(worker_id, task_id, ctx);
        }

        // Free the worker slot, clear MCP session, and reset TUI
        ctx.release_worker(worker_id);
        set_worker_idle(ctx, worker_id);

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

    /// Merge succeeded — mark done, cleanup worktree/branch, update state.
    async fn handle_merge_success(
        &self,
        worker_id: u32,
        task_id: &str,
        ctx: &mut RunLoopContext<'_>,
    ) {
        ctx.scheduler.mark_done(task_id);
        sync_task_status(self, ctx, task_id, TaskStatus::Done);

        // Worktree cleanup
        if let Some(WorkerSlot::Busy { worktree, .. }) = ctx.worker_slots.get(&worker_id) {
            ctx.worktree_manager
                .remove_worktree(&worktree.path)
                .await
                .ok();
            ctx.worktree_manager
                .remove_branch(&worktree.branch)
                .await
                .ok();
        }

        // Update orchestrate state
        let cost_usd = ctx.tui.mux_output.worker_cost(worker_id).0;
        ctx.state.tasks.insert(
            task_id.to_string(),
            crate::commands::task::orchestrate::state::TaskState {
                status: "done".to_string(),
                worker: Some(worker_id),
                retries: ctx.scheduler.retry_count(task_id),
                cost: cost_usd,
            },
        );

        ctx.tui.task_summaries.push(TaskSummaryEntry {
            task_id: task_id.to_string(),
            status: "Done".to_string(),
            cost_usd,
            duration: Duration::ZERO,
            retries: ctx.scheduler.retry_count(task_id),
        });
    }

    /// Merge failed — mark blocked, update state.
    fn handle_merge_failure(&self, worker_id: u32, task_id: &str, ctx: &mut RunLoopContext) {
        ctx.scheduler.mark_blocked(task_id);
        sync_task_status(self, ctx, task_id, TaskStatus::Blocked);

        let cost_usd = ctx.tui.mux_output.worker_cost(worker_id).0;
        ctx.state.tasks.insert(
            task_id.to_string(),
            crate::commands::task::orchestrate::state::TaskState {
                status: "blocked".to_string(),
                worker: Some(worker_id),
                retries: ctx.scheduler.retry_count(task_id),
                cost: cost_usd,
            },
        );

        ctx.tui.task_summaries.push(TaskSummaryEntry {
            task_id: task_id.to_string(),
            status: "Blocked".to_string(),
            cost_usd,
            duration: Duration::ZERO,
            retries: ctx.scheduler.retry_count(task_id),
        });
    }

    /// Merge conflict detected — update TUI to show conflict resolution state.
    fn handle_merge_conflict(
        &self,
        worker_id: u32,
        task_id: &str,
        conflicting_files: &[String],
        ctx: &mut RunLoopContext,
    ) {
        let msg = ctx.tui.mux_output.format_worker_line(
            worker_id,
            &format!("Merge conflict in {task_id}: {conflicting_files:?}"),
        );
        ctx.tui.app.push_log_line(&msg);

        let (cost, input, output) = ctx.tui.mux_output.worker_cost(worker_id);
        let conflict_model = &self.config.conflict_resolution_model;
        let ws = build_worker_status(WorkerStatusParams {
            state: WorkerState::ResolvingConflicts,
            task_id,
            component: lookup_component(ctx, task_id),
            phase: None,
            model: Some(conflict_model),
            cost_usd: cost,
            input_tokens: input,
            output_tokens: output,
            verify_profiles: Vec::new(),
        });
        ctx.tui.app.update_worker_status(worker_id, ws);

        let separator = conflict_resolution_start_separator();
        ctx.tui.app.push_worker_output(worker_id, &[separator]);
    }

    /// User message sent to a worker — display in worker panel.
    fn handle_user_message_sent(worker_id: u32, message: &str, ctx: &mut RunLoopContext) {
        let formatted_msg = format!("▶ USER: {message}");
        ctx.tui.app.push_worker_output(worker_id, &[formatted_msg]);

        let log_msg = ctx
            .tui
            .mux_output
            .format_worker_line(worker_id, "Message sent");
        ctx.tui.app.push_log_line(&log_msg);
    }

    /// User message received by a worker — display in worker panel.
    fn handle_user_message_received(worker_id: u32, message: &str, ctx: &mut RunLoopContext) {
        let formatted_msg = format!("◀ RECEIVED: {message}");
        ctx.tui.app.push_worker_output(worker_id, &[formatted_msg]);

        let log_msg = ctx
            .tui
            .mux_output
            .format_worker_line(worker_id, "Message received");
        ctx.tui.app.push_log_line(&log_msg);
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
                    profiles: vec![],
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
                    profiles: vec![],
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

        // Simulate manual status update (as done in orchestrator_events.rs)
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

        // Simulate manual status update (as done in orchestrator_events.rs)
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
        use crate::tui::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::with_capacity(100);

        // 1. Add task start separator
        let task_sep = task_start_separator("1.2.3");
        buffer.push_str(&task_sep);

        // 2. Add phase start separators for each phase
        buffer.push_str(&phase_start_separator(&WorkerPhase::Setup));
        buffer.push_str(&phase_start_separator(&WorkerPhase::Implement));
        buffer.push_str(&phase_start_separator(&WorkerPhase::ReviewFix));
        buffer.push_str(&phase_start_separator(&WorkerPhase::Verify));

        // 3. Add phase completion separators (success and failure)
        buffer.push_str(&phase_end_separator(&WorkerPhase::Setup, true));
        buffer.push_str(&phase_end_separator(&WorkerPhase::Implement, false));

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
        use crate::tui::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::with_capacity(10);
        let separator = task_start_separator("4.3");

        buffer.push_str(&separator);

        let lines = buffer.tail(10);
        assert_eq!(lines.len(), 1);

        // Verify separator content is preserved
        let line_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line_text.contains("Task 4.3"));
        assert!(line_text.contains("══════"));
    }

    #[test]
    fn test_phase_separators_preserve_styling_in_ring_buffer() {
        use crate::tui::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::with_capacity(10);

        // Add phase start separator with color
        let separator = phase_start_separator(&WorkerPhase::Implement);
        buffer.push_str(&separator);

        let lines = buffer.tail(10);
        assert_eq!(lines.len(), 1);

        let line_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line_text.contains("Implement"));
        assert!(line_text.contains("●"));
    }

    #[test]
    fn test_phase_end_separator_success_and_failure_in_ring_buffer() {
        use crate::tui::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::with_capacity(10);

        // Add success separator
        let success_sep = phase_end_separator(&WorkerPhase::Verify, true);
        buffer.push_str(&success_sep);

        // Add failure separator
        let failure_sep = phase_end_separator(&WorkerPhase::ReviewFix, false);
        buffer.push_str(&failure_sep);

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
        use crate::tui::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::with_capacity(20);

        // Simulate realistic worker output sequence
        buffer.push_str(&task_start_separator("2.1"));
        buffer.push_str("Worker output: Starting implementation...");
        buffer.push_str(&phase_start_separator(&WorkerPhase::Implement));
        buffer.push_str("Worker output: Writing code...");
        buffer.push_str("Worker output: Code written successfully");
        buffer.push_str(&phase_end_separator(&WorkerPhase::Implement, true));
        buffer.push_str(&phase_start_separator(&WorkerPhase::Verify));
        buffer.push_str("Worker output: Running tests...");
        buffer.push_str(&phase_end_separator(&WorkerPhase::Verify, true));

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
        use crate::tui::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::with_capacity(10);

        // Add conflict resolution start separator
        let start_sep = conflict_resolution_start_separator();
        buffer.push_str(&start_sep);

        // Add conflict resolution end separator (success)
        let end_sep_ok = conflict_resolution_end_separator(true);
        buffer.push_str(&end_sep_ok);

        // Add conflict resolution end separator (failure)
        let end_sep_fail = conflict_resolution_end_separator(false);
        buffer.push_str(&end_sep_fail);

        let lines = buffer.tail(10);
        assert_eq!(lines.len(), 3);

        let start_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(start_text.contains("Conflict Resolution"));
        assert!(start_text.contains("⚡"));

        let success_text: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(success_text.contains("Conflict Resolution"));
        assert!(success_text.contains("[ok]"));
        assert!(success_text.contains("✓"));

        let failure_text: String = lines[2].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(failure_text.contains("Conflict Resolution"));
        assert!(failure_text.contains("[FAILED]"));
        assert!(failure_text.contains("✗"));
    }

    #[test]
    fn test_merge_conflict_output_sequence() {
        use crate::tui::ring_buffer::OutputRingBuffer;

        let mut buffer = OutputRingBuffer::with_capacity(30);

        // Simulate realistic merge → conflict → AI resolution → end sequence
        // 1. Merge output
        buffer.push_str("Merge output: Attempting to merge branch 'task/1.1' into master...");
        buffer.push_str("Merge output: Auto-merging src/main.rs");
        buffer.push_str("Merge output: CONFLICT (content): Merge conflict in src/main.rs");

        // 2. Conflict resolution start separator
        buffer.push_str(&conflict_resolution_start_separator());

        // 3. AI output during conflict resolution
        buffer.push_str("AI output: Analyzing conflict in src/main.rs...");
        buffer.push_str("AI output: Conflict involves function signature changes");
        buffer.push_str("AI output: Applying resolution strategy: preserve both changes");
        buffer.push_str("AI output: Writing resolved file...");
        buffer.push_str("AI output: Staging resolved file...");

        // 4. Conflict resolution end separator (success)
        buffer.push_str(&conflict_resolution_end_separator(true));

        // 5. Final merge output
        buffer.push_str("Merge output: Conflict resolved successfully");
        buffer.push_str("Merge output: Creating merge commit...");

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

        let line4_text: String = lines[4].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line4_text.contains("Analyzing conflict"));

        let line8_text: String = lines[8].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line8_text.contains("Staging resolved file"));
    }
}
