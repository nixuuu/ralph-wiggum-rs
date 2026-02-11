use std::sync::Arc;
use std::time::{Instant, SystemTime};

use crate::commands::task::orchestrate::events::WorkerPhase;
use crate::commands::task::orchestrate::output::MultiplexedOutput;
use crate::commands::task::orchestrate::shared_types::{WorkerState, WorkerStatus};
use crate::commands::task::orchestrate::worker::Worker;
use crate::commands::task::orchestrate::worktree::WorktreeInfo;
use crate::shared::error::Result;
use crate::shared::progress::TaskStatus;
use crate::shared::tasks::TasksFile;

use super::orchestrator::{Orchestrator, RunLoopContext};

// ── Worker slot tracking ────────────────────────────────────────────

/// Per-worker tracking state within the orchestrator.
#[derive(Debug, Clone)]
pub(super) enum WorkerSlot {
    Idle,
    Busy {
        /// Task ID tracked in worker slot for debugging and state consistency.
        #[allow(dead_code)] // Tracked for debugging and consistency but not actively used in logic
        task_id: String,
        worktree: WorktreeInfo,
    },
}

// ── Task assignment logic ───────────────────────────────────────────

impl Orchestrator {
    /// Assign ready tasks to idle workers.
    pub(super) async fn assign_tasks(
        &self,
        ctx: &mut RunLoopContext<'_>,
    ) -> Result<()> {
        // Always refresh ready_queue before assigning — guards against
        // stale state from hot reload or missed refresh paths.
        ctx.scheduler.refresh_ready_queue();

        // Find idle workers
        let idle_workers: Vec<u32> = ctx.worker_slots
            .iter()
            .filter(|(_, slot)| matches!(slot, WorkerSlot::Idle))
            .map(|(id, _)| *id)
            .collect();

        let mut started_ids: Vec<String> = Vec::new();

        for worker_id in idle_workers {
            let Some(task_id) = ctx.scheduler.next_ready_task() else {
                break;
            };

            // Find task info from progress summary
            let task_info = ctx.progress.tasks.iter().find(|t| t.id == task_id);
            let leaf = ctx.tasks_file.find_leaf(&task_id);
            let task_desc = leaf
                .as_ref()
                .map(crate::shared::tasks::format_task_prompt)
                .unwrap_or_else(|| task_id.clone());

            // Resolve model for this task
            let model = self.resolve_model(&task_id, ctx.tasks_file);

            // Create worktree
            let worktree = ctx.worktree_manager
                .create_worktree(worker_id, &task_id)
                .await?;

            // Print assignment via TUI
            let msg = ctx.tui.mux_output.format_worker_line(
                worker_id,
                &format!("Assigned: {task_id} → {}", worktree.branch),
            );
            ctx.tui.dashboard.push_log_line(&msg);

            // Mark task as started in scheduler
            ctx.scheduler.mark_started(&task_id);
            started_ids.push(task_id.clone());

            // Update TUI worker status
            ctx.tui.mux_output.assign_worker(worker_id, &task_id);
            let ws = WorkerStatus {
                state: WorkerState::Implementing,
                task_id: Some(task_id.clone()),
                component: task_info.map(|t| t.component.clone()),
                phase: Some(WorkerPhase::Implement),
                cost_usd: 0.0,
                input_tokens: 0,
                output_tokens: 0,
            };
            ctx.tui.dashboard.update_worker_status(worker_id, ws);
            ctx.tui.task_start_times
                .insert(task_id.clone(), Instant::now());

            // Expand setup commands before worktree is moved into WorkerSlot
            let expanded_setup: Vec<(String, String)> = self
                .setup_commands
                .iter()
                .map(|cmd| {
                    let expanded = expand_setup_command(
                        cmd.command(),
                        &self.project_root,
                        &worktree.path,
                        &task_id,
                        worker_id,
                    );
                    (expanded, cmd.label().to_string())
                })
                .collect();

            // Extract path before moving worktree into WorkerSlot
            let worktree_path = worktree.path.clone();

            // Update worker slot (moves worktree, avoids full WorktreeInfo clone)
            ctx.worker_slots.insert(
                worker_id,
                WorkerSlot::Busy {
                    task_id: task_id.clone(),
                    worktree,
                },
            );

            // Spawn worker as tokio task
            let worker = Worker::new(
                worker_id,
                ctx.event_tx.clone(),
                Arc::clone(&ctx.flags.shutdown),
                self.system_prompt.clone(),
                self.config.max_retries,
                self.use_nerd_font,
            );

            let verification_cmds = self.verification_commands.clone();
            // task_id moved (not cloned) — last use of the owned String
            let task_id_owned = task_id;
            let task_desc_owned = task_desc;
            let model_owned = model;

            let handle = tokio::spawn(async move {
                worker
                    .execute_task(
                        &task_id_owned,
                        &task_desc_owned,
                        model_owned.as_deref(),
                        &worktree_path,
                        &expanded_setup,
                        verification_cmds.as_deref(),
                    )
                    .await
            });

            ctx.join_handles.insert(worker_id, handle);
        }

        // Diagnostic: warn when idle workers exist but scheduler has pending/ready tasks
        let remaining_idle = ctx
            .worker_slots
            .values()
            .filter(|s| matches!(s, WorkerSlot::Idle))
            .count();
        if remaining_idle > 0 {
            let status = ctx.scheduler.status();
            if status.pending > 0 || status.ready > 0 {
                let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                    "⚠ {} idle workers, scheduler: {} ready, {} pending, {} done, {} in_progress, {} blocked",
                    remaining_idle, status.ready, status.pending, status.done, status.in_progress, status.blocked,
                ));
                ctx.tui.dashboard.push_log_line(&msg);
            }
        }

        // Batch-update tasks.yml for all newly started tasks
        if !started_ids.is_empty() {
            let updates: Vec<(String, TaskStatus)> = started_ids
                .into_iter()
                .map(|id| (id, TaskStatus::InProgress))
                .collect();
            if let Err(e) = TasksFile::batch_update_statuses(&self.tasks_path, &updates) {
                let msg = MultiplexedOutput::format_orchestrator_line(
                    &format!("Warning: tasks.yml batch update failed: {e}"),
                );
                ctx.tui.dashboard.push_log_line(&msg);
            } else if let Some(mt) = get_mtime(&self.tasks_path) {
                ctx.progress_mtime = Some(mt);
            }
        }

        Ok(())
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Expand template variables in a setup command string.
pub(super) fn expand_setup_command(
    cmd: &str,
    root_dir: &std::path::Path,
    worktree_dir: &std::path::Path,
    task_id: &str,
    worker_id: u32,
) -> String {
    cmd.replace("{ROOT_DIR}", &root_dir.display().to_string())
        .replace("{WORKTREE_DIR}", &worktree_dir.display().to_string())
        .replace("{TASK_ID}", task_id)
        .replace("{WORKER_ID}", &worker_id.to_string())
}

/// Get file modification time.
pub(super) fn get_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_setup_command_all_variables() {
        let result = expand_setup_command(
            "cp {ROOT_DIR}/.env {WORKTREE_DIR}/.env && echo {TASK_ID} w{WORKER_ID}",
            std::path::Path::new("/home/user/project"),
            std::path::Path::new("/tmp/worktrees/w1"),
            "T03",
            1,
        );
        assert_eq!(
            result,
            "cp /home/user/project/.env /tmp/worktrees/w1/.env && echo T03 w1"
        );
    }

    #[test]
    fn test_expand_setup_command_no_variables() {
        let result = expand_setup_command(
            "npm install",
            std::path::Path::new("/root"),
            std::path::Path::new("/wt"),
            "T01",
            2,
        );
        assert_eq!(result, "npm install");
    }

    #[test]
    fn test_get_mtime_nonexistent() {
        assert!(get_mtime(std::path::Path::new("/nonexistent/file")).is_none());
    }

    #[test]
    fn test_worker_slot_states() {
        let idle = WorkerSlot::Idle;
        assert!(matches!(idle, WorkerSlot::Idle));

        let busy = WorkerSlot::Busy {
            task_id: "T01".to_string(),
            worktree: WorktreeInfo {
                path: std::path::PathBuf::from("/tmp/wt1"),
                branch: "ralph/w1/T01".to_string(),
                worker_id: 1,
                task_id: "T01".to_string(),
            },
        };
        assert!(matches!(busy, WorkerSlot::Busy { .. }));
    }
}
