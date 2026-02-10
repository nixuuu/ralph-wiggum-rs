#![allow(dead_code)]
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::process::Command;
use tokio::sync::mpsc;

use crate::commands::task::orchestrate::events::{WorkerEvent, WorkerEventKind, WorkerPhase};
use crate::commands::task::orchestrate::worker_runner::WorkerRunner;
use crate::shared::error::{RalphError, Result};

/// Result of executing a task through the 3-phase worker lifecycle.
#[derive(Debug, Clone)]
pub struct TaskResult {
    pub task_id: String,
    pub success: bool,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub commit_hash: Option<String>,
    pub retries: u32,
    pub files_changed: Vec<String>,
}

/// Worker that executes tasks through a 3-phase lifecycle:
/// implement → review+fix → verify.
///
/// Each worker operates in an isolated git worktree and communicates
/// with the orchestrator via mpsc channel events.
pub struct Worker {
    id: u32,
    event_tx: mpsc::Sender<WorkerEvent>,
    shutdown: Arc<AtomicBool>,
    system_prompt: String,
    max_retries: u32,
    use_nerd_font: bool,
}

impl Worker {
    pub fn new(
        id: u32,
        event_tx: mpsc::Sender<WorkerEvent>,
        shutdown: Arc<AtomicBool>,
        system_prompt: String,
        max_retries: u32,
        use_nerd_font: bool,
    ) -> Self {
        Self {
            id,
            event_tx,
            shutdown,
            system_prompt,
            max_retries,
            use_nerd_font,
        }
    }

    /// Execute a task through the full 3-phase lifecycle with retry logic.
    ///
    /// Phases: implement → review+fix → verify
    /// If verify fails, retries the full cycle up to `max_retries` times.
    /// After max retries, marks the task as failed.
    pub async fn execute_task(
        &self,
        task_id: &str,
        task_desc: &str,
        model: Option<&str>,
        worktree_path: &Path,
        verification_commands: Option<&str>,
    ) -> Result<TaskResult> {
        self.send_event(WorkerEventKind::TaskStarted {
            worker_id: self.id,
            task_id: task_id.to_string(),
        })
        .await;

        let total_cost = 0.0_f64;
        let total_input_tokens = 0_u64;
        let total_output_tokens = 0_u64;
        let mut retries = 0_u32;

        loop {
            let runner = WorkerRunner::new(
                self.id,
                task_id.to_string(),
                self.event_tx.clone(),
                self.shutdown.clone(),
                self.use_nerd_font,
            );

            // Phase 1: Implement
            let impl_output = match runner
                .run_implement(task_desc, &self.system_prompt, model, worktree_path)
                .await
            {
                Ok(output) => output,
                Err(e) => {
                    return self
                        .handle_failure(task_id, &e.to_string(), retries, total_cost)
                        .await;
                }
            };

            // Commit after implement phase
            self.git_commit(worktree_path, task_id, &WorkerPhase::Implement)
                .await
                .ok();

            // Phase 2: Review + Fix
            match runner
                .run_review(&impl_output, task_desc, model, worktree_path)
                .await
            {
                Ok(_) => {}
                Err(e) => {
                    return self
                        .handle_failure(task_id, &e.to_string(), retries, total_cost)
                        .await;
                }
            };

            // Commit after review phase
            self.git_commit(worktree_path, task_id, &WorkerPhase::ReviewFix)
                .await
                .ok();

            // Phase 3: Verify (skipped when no verify_commands configured)
            let verified = if let Some(verify_cmds) = verification_commands {
                match runner
                    .run_verify(verify_cmds, model, worktree_path)
                    .await
                {
                    Ok(passed) => {
                        self.git_commit(worktree_path, task_id, &WorkerPhase::Verify)
                            .await
                            .ok();
                        passed
                    }
                    Err(e) => {
                        return self
                            .handle_failure(task_id, &e.to_string(), retries, total_cost)
                            .await;
                    }
                }
            } else {
                true
            };

            if verified {
                // Get changed files
                let files_changed = self.get_changed_files(worktree_path).await;
                let commit_hash = self.get_head_hash(worktree_path).await;

                let result = TaskResult {
                    task_id: task_id.to_string(),
                    success: true,
                    cost_usd: total_cost,
                    input_tokens: total_input_tokens,
                    output_tokens: total_output_tokens,
                    commit_hash,
                    retries,
                    files_changed: files_changed.clone(),
                };

                self.send_event(WorkerEventKind::TaskCompleted {
                    worker_id: self.id,
                    task_id: task_id.to_string(),
                    success: true,
                    cost_usd: total_cost,
                    input_tokens: total_input_tokens,
                    output_tokens: total_output_tokens,
                    files_changed,
                    commit_hash: result.commit_hash.clone(),
                })
                .await;

                return Ok(result);
            }

            // Verify failed — retry or give up
            retries += 1;
            if retries >= self.max_retries {
                return self
                    .handle_failure(
                        task_id,
                        "verification failed after max retries",
                        retries,
                        total_cost,
                    )
                    .await;
            }

            self.send_event(WorkerEventKind::TaskFailed {
                worker_id: self.id,
                task_id: task_id.to_string(),
                error: "verification failed, retrying".to_string(),
                retries_left: self.max_retries - retries,
            })
            .await;
        }
    }

    /// Commit all changes in the worktree after a phase.
    async fn git_commit(
        &self,
        worktree_path: &Path,
        task_id: &str,
        phase: &WorkerPhase,
    ) -> Result<()> {
        // Stage all changes
        let add_output = Command::new("git")
            .args(["add", "-A"])
            .current_dir(worktree_path)
            .output()
            .await
            .map_err(|e| RalphError::WorktreeError(format!("git add failed: {e}")))?;

        if !add_output.status.success() {
            return Ok(()); // Nothing to commit
        }

        // Check if there's anything to commit
        let status_output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(worktree_path)
            .output()
            .await
            .map_err(|e| RalphError::WorktreeError(format!("git status failed: {e}")))?;

        let status_str = String::from_utf8_lossy(&status_output.stdout);
        if status_str.trim().is_empty() {
            return Ok(()); // Nothing to commit
        }

        let msg = format!("wip: {task_id} phase {phase}");
        Command::new("git")
            .args(["commit", "-m", &msg])
            .current_dir(worktree_path)
            .output()
            .await
            .map_err(|e| RalphError::WorktreeError(format!("git commit failed: {e}")))?;

        Ok(())
    }

    /// Get list of files changed in the worktree compared to HEAD~.
    async fn get_changed_files(&self, worktree_path: &Path) -> Vec<String> {
        let output = Command::new("git")
            .args(["diff", "--name-only", "HEAD~1"])
            .current_dir(worktree_path)
            .output()
            .await;

        match output {
            Ok(out) => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.to_string())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Get the HEAD commit hash in the worktree.
    async fn get_head_hash(&self, worktree_path: &Path) -> Option<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(worktree_path)
            .output()
            .await
            .ok()?;

        let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if hash.is_empty() { None } else { Some(hash) }
    }

    async fn handle_failure(
        &self,
        task_id: &str,
        error: &str,
        retries: u32,
        cost: f64,
    ) -> Result<TaskResult> {
        self.send_event(WorkerEventKind::TaskFailed {
            worker_id: self.id,
            task_id: task_id.to_string(),
            error: error.to_string(),
            retries_left: 0,
        })
        .await;

        Ok(TaskResult {
            task_id: task_id.to_string(),
            success: false,
            cost_usd: cost,
            input_tokens: 0,
            output_tokens: 0,
            commit_hash: None,
            retries,
            files_changed: Vec::new(),
        })
    }

    async fn send_event(&self, kind: WorkerEventKind) {
        let event = WorkerEvent::new(kind);
        let _ = self.event_tx.send(event).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_result_fields() {
        let result = TaskResult {
            task_id: "T01".to_string(),
            success: true,
            cost_usd: 0.042,
            input_tokens: 1200,
            output_tokens: 500,
            commit_hash: Some("abc1234".to_string()),
            retries: 0,
            files_changed: vec!["src/main.rs".to_string()],
        };
        assert!(result.success);
        assert_eq!(result.retries, 0);
        assert_eq!(result.files_changed.len(), 1);
    }

    #[test]
    fn test_worker_creation() {
        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker = Worker::new(1, tx, shutdown, "system prompt".to_string(), 3, false);
        assert_eq!(worker.id, 1);
        assert_eq!(worker.max_retries, 3);
    }

    #[tokio::test]
    async fn test_worker_send_event() {
        let (tx, mut rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker = Worker::new(2, tx, shutdown, "system prompt".to_string(), 3, false);

        worker
            .send_event(WorkerEventKind::TaskStarted {
                worker_id: 2,
                task_id: "T03".to_string(),
            })
            .await;

        let event = rx.recv().await.unwrap();
        if let WorkerEventKind::TaskStarted { worker_id, task_id } = &event.kind {
            assert_eq!(*worker_id, 2);
            assert_eq!(task_id, "T03");
        } else {
            panic!("Expected TaskStarted event");
        }
    }

    #[test]
    fn test_task_result_failed() {
        let result = TaskResult {
            task_id: "T02".to_string(),
            success: false,
            cost_usd: 0.089,
            input_tokens: 5000,
            output_tokens: 3000,
            commit_hash: None,
            retries: 3,
            files_changed: Vec::new(),
        };
        assert!(!result.success);
        assert_eq!(result.retries, 3);
        assert!(result.commit_hash.is_none());
    }
}
