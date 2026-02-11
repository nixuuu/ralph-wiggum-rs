use std::sync::Arc;
use std::time::Duration;

use crate::commands::task::orchestrate::events::{WorkerEventKind, WorkerPhase};
use crate::commands::task::orchestrate::shared_types::{WorkerState, WorkerStatus};
use crate::commands::task::orchestrate::summary::TaskSummaryEntry;
use crate::shared::error::Result;
use crate::shared::progress::TaskStatus;

use super::assignment::WorkerSlot;
use super::orchestrator::{Orchestrator, RunLoopContext};
use super::orchestrator_merge::PendingMerge;

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
                let ws = WorkerStatus {
                    state: WorkerState::Implementing,
                    task_id: Some(task_id.clone()),
                    component: ctx
                        .task_lookup
                        .get(task_id.as_str())
                        .map(|t| t.component.clone()),
                    phase: Some(WorkerPhase::Implement),
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
                let ws = WorkerStatus {
                    state: new_state,
                    task_id: Some(task_id.clone()),
                    component: ctx
                        .task_lookup
                        .get(task_id.as_str())
                        .map(|t| t.component.clone()),
                    phase: Some(phase.clone()),
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
                        let ws = WorkerStatus {
                            state: WorkerState::Merging,
                            task_id: Some(task_id.clone()),
                            component: Some(task.component.clone()),
                            phase: None,
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
                            );
                        }
                    }
                } else if *success {
                    // --no-merge mode
                    ctx.scheduler.mark_done(task_id);
                    if let Some(mt) = self.update_tasks_file(task_id, TaskStatus::Done, ctx.tui) {
                        ctx.progress_mtime = Some(mt);
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

                    // Free the worker slot
                    ctx.worker_slots.insert(*worker_id, WorkerSlot::Idle);
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
                        }

                        ctx.tui.task_summaries.push(TaskSummaryEntry {
                            task_id: task_id.clone(),
                            status: "Blocked".to_string(),
                            cost_usd: *cost_usd,
                            duration,
                            retries: ctx.scheduler.retry_count(task_id),
                        });
                    }

                    // Free the worker slot
                    ctx.worker_slots.insert(*worker_id, WorkerSlot::Idle);
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
                let ws = WorkerStatus {
                    state: WorkerState::Merging,
                    task_id: Some(task_id.clone()),
                    component: ctx
                        .task_lookup
                        .get(task_id.as_str())
                        .map(|t| t.component.clone()),
                    phase: None,
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

                // Free the worker slot
                ctx.worker_slots.insert(*worker_id, WorkerSlot::Idle);
                ctx.tui
                    .dashboard
                    .update_worker_status(*worker_id, WorkerStatus::idle(*worker_id));
                ctx.tui.mux_output.clear_worker(*worker_id);

                // Start next queued merge if any
                ctx.merge_ctx.merge_in_progress = false;
                if let Some(next) = ctx.merge_ctx.pending_merges.pop_front() {
                    Self::spawn_merge_task(
                        &mut ctx.merge_ctx,
                        next,
                        &self.project_root,
                        ctx.event_tx.clone(),
                        Arc::clone(&ctx.flags.shutdown),
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
                let ws = WorkerStatus {
                    state: WorkerState::ResolvingConflicts,
                    task_id: Some(task_id.clone()),
                    component: ctx
                        .task_lookup
                        .get(task_id.as_str())
                        .map(|t| t.component.clone()),
                    phase: None,
                    cost_usd: cost,
                    input_tokens: input,
                    output_tokens: output,
                };
                ctx.tui.dashboard.update_worker_status(*worker_id, ws);
            }

            WorkerEventKind::OutputLines { worker_id, lines } => {
                ctx.tui.dashboard.push_worker_output(*worker_id, lines);
            }

            WorkerEventKind::MergeStepOutput { worker_id, lines } => {
                ctx.tui.dashboard.push_worker_output(*worker_id, lines);
            }
        }

        Ok(())
    }
}
