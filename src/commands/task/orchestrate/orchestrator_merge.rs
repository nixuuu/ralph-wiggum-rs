use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::commands::task::orchestrate::events::{WorkerEvent, WorkerEventKind};
use crate::commands::task::orchestrate::merge;
use crate::commands::task::orchestrate::output::MultiplexedOutput;
use crate::shared::error::Result;
use crate::shared::progress::TaskStatus;
use crate::shared::tasks::TasksFile;

use super::assignment::get_mtime;
use super::orchestrator::Orchestrator;
use super::orchestrator_tui::TuiContext;

// ── Merge context ───────────────────────────────────────────────────

/// Serialized merge tracking — max 1 merge at a time to avoid git conflicts.
pub(super) struct MergeContext {
    pub(super) merge_in_progress: bool,
    pub(super) pending_merges: VecDeque<PendingMerge>,
    pub(super) merge_join_handle: Option<JoinHandle<()>>,
}

impl MergeContext {
    pub(super) fn new() -> Self {
        Self {
            merge_in_progress: false,
            pending_merges: VecDeque::new(),
            merge_join_handle: None,
        }
    }
}

/// Data needed to perform a merge after a worker completes.
pub(super) struct PendingMerge {
    pub(super) worker_id: u32,
    pub(super) task_id: String,
    pub(super) task_name: String,
    pub(super) branch: String,
}

// ── Merge spawning ──────────────────────────────────────────────────

impl Orchestrator {
    /// Spawn a merge task in a separate tokio task.
    ///
    /// The merge runs sequentially (one at a time) and communicates via events.
    pub(super) fn spawn_merge_task(
        merge_ctx: &mut MergeContext,
        pending: PendingMerge,
        project_root: &std::path::Path,
        event_tx: mpsc::Sender<WorkerEvent>,
        shutdown: Arc<AtomicBool>,
    ) {
        merge_ctx.merge_in_progress = true;
        let root = project_root.to_path_buf();
        let PendingMerge { worker_id, task_id, task_name, branch } = pending;

        let handle = tokio::spawn(async move {
            // Emit MergeStarted
            let _ = event_tx.send(WorkerEvent::new(WorkerEventKind::MergeStarted {
                worker_id,
                task_id: task_id.clone(),
            })).await;

            // Step 1: git merge --squash
            let (step1, conflict_files) = match merge::step_merge_squash(&root, &branch).await {
                Ok(result) => result,
                Err(e) => {
                    let _ = event_tx.try_send(WorkerEvent::new(WorkerEventKind::MergeStepOutput {
                        worker_id,
                        lines: vec![format!("Merge error: {e}")],
                    }));
                    let _ = event_tx.send(WorkerEvent::new(WorkerEventKind::MergeCompleted {
                        worker_id,
                        task_id,
                        success: false,
                        commit_hash: None,
                    })).await;
                    return;
                }
            };

            // Show merge output in worker panel
            let step_lines = merge::step_output_lines(&step1);
            if !step_lines.is_empty() {
                let _ = event_tx.try_send(WorkerEvent::new(WorkerEventKind::MergeStepOutput {
                    worker_id,
                    lines: step_lines,
                }));
            }

            if !step1.success {
                if !conflict_files.is_empty() {
                    // Emit conflict event
                    let _ = event_tx.try_send(WorkerEvent::new(WorkerEventKind::MergeConflict {
                        worker_id,
                        task_id: task_id.clone(),
                        conflicting_files: conflict_files.clone(),
                    }));

                    // === AI Conflict Resolution ===
                    if shutdown.load(Ordering::Relaxed) {
                        merge::abort_merge(&root).await.ok();
                        let _ = event_tx.send(WorkerEvent::new(WorkerEventKind::MergeCompleted {
                            worker_id,
                            task_id,
                            success: false,
                            commit_hash: None,
                        })).await;
                        return;
                    }

                    let files_str = conflict_files.join(", ");
                    let prompt = format!(
                        "Resolve the merge conflicts in the following files: {files_str}\n\
                         Task context: {task_name}\n\n\
                         Files with conflict markers (<<<<<<<, =======, >>>>>>>) \
                         are in the current working directory.\n\
                         Read each file, resolve all conflicts, write the resolved version, \
                         and stage: git add <file>"
                    );

                    let _ = event_tx.try_send(WorkerEvent::new(WorkerEventKind::MergeStepOutput {
                        worker_id,
                        lines: vec!["⚡ Starting AI conflict resolution...".to_string()],
                    }));

                    let runner = crate::commands::run::runner::ClaudeRunner::oneshot(
                        prompt,
                        Some("claude-opus-4-6".to_string()),
                        Some(root.clone()),
                    );

                    let event_tx_clone = event_tx.clone();
                    let wid = worker_id;
                    let result = runner.run(
                        Arc::clone(&shutdown),
                        |claude_event| {
                            // Forward text output to worker panel
                            if let crate::commands::run::runner::ClaudeEvent::Assistant { message } = claude_event {
                                for block in &message.content {
                                    if let crate::commands::run::runner::ContentBlock::Text { text } = block {
                                        let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
                                        if !lines.is_empty() {
                                            let _ = event_tx_clone.try_send(WorkerEvent::new(
                                                WorkerEventKind::MergeStepOutput { worker_id: wid, lines },
                                            ));
                                        }
                                    }
                                }
                            }
                        },
                        || {},
                    ).await;

                    match result {
                        Ok(_) => {
                            // Check if conflicts are resolved
                            let check = tokio::process::Command::new("git")
                                .args(["diff", "--check"])
                                .current_dir(&root)
                                .output()
                                .await;

                            let resolved = check.map(|o| o.status.success()).unwrap_or(false);
                            if resolved {
                                let _ = event_tx.try_send(WorkerEvent::new(WorkerEventKind::MergeStepOutput {
                                    worker_id,
                                    lines: vec!["✓ Conflicts resolved successfully".to_string()],
                                }));
                                // Continue with commit + rev-parse below
                            } else {
                                let _ = event_tx.try_send(WorkerEvent::new(WorkerEventKind::MergeStepOutput {
                                    worker_id,
                                    lines: vec!["✗ Conflict resolution failed — aborting merge".to_string()],
                                }));
                                merge::abort_merge(&root).await.ok();
                                let _ = event_tx.send(WorkerEvent::new(WorkerEventKind::MergeCompleted {
                                    worker_id,
                                    task_id,
                                    success: false,
                                    commit_hash: None,
                                })).await;
                                return;
                            }
                        }
                        Err(_) => {
                            let _ = event_tx.try_send(WorkerEvent::new(WorkerEventKind::MergeStepOutput {
                                worker_id,
                                lines: vec!["✗ AI conflict resolution failed — aborting merge".to_string()],
                            }));
                            merge::abort_merge(&root).await.ok();
                            let _ = event_tx.send(WorkerEvent::new(WorkerEventKind::MergeCompleted {
                                worker_id,
                                task_id,
                                success: false,
                                commit_hash: None,
                            })).await;
                            return;
                        }
                    }
                } else {
                    // Non-conflict failure
                    let _ = event_tx.try_send(WorkerEvent::new(WorkerEventKind::MergeStepOutput {
                        worker_id,
                        lines: vec![format!("Merge failed: {}", step1.stderr)],
                    }));
                    let _ = event_tx.send(WorkerEvent::new(WorkerEventKind::MergeCompleted {
                        worker_id,
                        task_id,
                        success: false,
                        commit_hash: None,
                    })).await;
                    return;
                }
            }

            // Check if there are any staged changes after squash
            let has_staged = tokio::process::Command::new("git")
                .args(["diff", "--cached", "--quiet"])
                .current_dir(&root)
                .status()
                .await
                .map(|s| !s.success()) // exit 1 = has diffs, exit 0 = no diffs
                .unwrap_or(true); // fallback: assume changes exist

            if !has_staged {
                // No changes to commit — task completed without code modifications
                let _ = event_tx.try_send(WorkerEvent::new(WorkerEventKind::MergeStepOutput {
                    worker_id,
                    lines: vec!["No changes to merge — task completed without code modifications".to_string()],
                }));
                let _ = event_tx.send(WorkerEvent::new(WorkerEventKind::MergeCompleted {
                    worker_id,
                    task_id,
                    success: true,
                    commit_hash: None,
                })).await;
                return;
            }

            // Step 2: commit
            let step2 = match merge::step_commit(&root, &task_id, &task_name).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = event_tx.try_send(WorkerEvent::new(WorkerEventKind::MergeStepOutput {
                        worker_id,
                        lines: vec![format!("Commit error: {e}")],
                    }));
                    let _ = event_tx.send(WorkerEvent::new(WorkerEventKind::MergeCompleted {
                        worker_id,
                        task_id,
                        success: false,
                        commit_hash: None,
                    })).await;
                    return;
                }
            };

            let step2_lines = merge::step_output_lines(&step2);
            if !step2_lines.is_empty() {
                let _ = event_tx.try_send(WorkerEvent::new(WorkerEventKind::MergeStepOutput {
                    worker_id,
                    lines: step2_lines,
                }));
            }

            if !step2.success {
                let _ = event_tx.send(WorkerEvent::new(WorkerEventKind::MergeCompleted {
                    worker_id,
                    task_id,
                    success: false,
                    commit_hash: None,
                })).await;
                return;
            }

            // Step 3: rev-parse
            let commit_hash = match merge::step_rev_parse(&root).await {
                Ok((step3, hash)) => {
                    let step3_lines = merge::step_output_lines(&step3);
                    if !step3_lines.is_empty() {
                        let _ = event_tx.try_send(WorkerEvent::new(WorkerEventKind::MergeStepOutput {
                            worker_id,
                            lines: step3_lines,
                        }));
                    }
                    hash
                }
                Err(_) => "???".to_string(),
            };

            let _ = event_tx.send(WorkerEvent::new(WorkerEventKind::MergeCompleted {
                worker_id,
                task_id,
                success: true,
                commit_hash: Some(commit_hash),
            })).await;
        });

        merge_ctx.merge_join_handle = Some(handle);
    }

    /// Update tasks.yml status for a task. Non-fatal — logs warning on failure.
    /// Returns new mtime if successful.
    pub(super) fn update_tasks_file(
        &self,
        task_id: &str,
        new_status: TaskStatus,
        tui: &mut TuiContext,
    ) -> Option<SystemTime> {
        let result = (|| -> Result<()> {
            let mut tf = TasksFile::load(&self.tasks_path)?;
            tf.update_status(task_id, new_status);
            tf.save(&self.tasks_path)
        })();
        if let Err(e) = result {
            let msg = MultiplexedOutput::format_orchestrator_line(
                &format!("Warning: tasks.yml update failed for {task_id}: {e}"),
            );
            tui.dashboard.push_log_line(&msg);
            return None;
        }
        get_mtime(&self.tasks_path)
    }
}
