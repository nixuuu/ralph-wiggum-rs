#![allow(dead_code)]
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::process::Command;
use tokio::sync::mpsc;

use crate::commands::task::orchestrate::events::{WorkerEvent, WorkerEventKind, WorkerPhase};
use crate::commands::task::orchestrate::verify;
use crate::commands::task::orchestrate::worker_runner::{WorkerRunner, WorkerRunnerConfig};
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::VerifyCommand;

/// Configuration for Worker.
///
/// Groups configuration parameters for worker behavior: retry logic, prompt customization,
/// UI settings, and timeout configuration.
#[derive(Clone, Default)]
pub struct WorkerConfig {
    pub system_prompt: String,
    pub max_retries: u32,
    pub use_nerd_font: bool,
    pub prompt_prefix: Option<String>,
    pub prompt_suffix: Option<String>,
    pub phase_timeout: Option<std::time::Duration>,
    pub git_timeout: std::time::Duration,
    pub setup_timeout: std::time::Duration,
}

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
    config: WorkerConfig,
}

impl Worker {
    pub fn new(
        id: u32,
        event_tx: mpsc::Sender<WorkerEvent>,
        shutdown: Arc<AtomicBool>,
        config: WorkerConfig,
    ) -> Self {
        Self {
            id,
            event_tx,
            shutdown,
            config,
        }
    }

    /// Execute a task through the full lifecycle with retry logic.
    ///
    /// Flow: Setup → Implement → git commit → DirectVerify → Review+Fix → git commit → DirectVerify → [retry or success]
    ///
    /// If verify fails after implement, the failure report is passed to review+fix phase.
    /// If verify fails after review+fix, retries the full cycle up to `max_retries` times.
    /// Empty `verify_commands` skips verify phases entirely.
    pub async fn execute_task(
        &self,
        task_id: &str,
        task_desc: &str,
        model: Option<&str>,
        worktree_path: &Path,
        setup_commands: &[(String, String)],
        verify_commands: &[VerifyCommand],
    ) -> Result<TaskResult> {
        self.send_event(WorkerEventKind::TaskStarted {
            worker_id: self.id,
            task_id: task_id.to_string(),
        })
        .await;

        // Phase 0: Setup (raw shell commands, non-fatal)
        if !setup_commands.is_empty() {
            self.run_setup(task_id, setup_commands, worktree_path).await;
        }

        let mut total_cost = 0.0_f64;
        let mut total_input_tokens = 0_u64;
        let mut total_output_tokens = 0_u64;
        let mut retries = 0_u32;

        loop {
            let runner_config = WorkerRunnerConfig {
                use_nerd_font: self.config.use_nerd_font,
                prompt_prefix: self.config.prompt_prefix.clone(),
                prompt_suffix: self.config.prompt_suffix.clone(),
                phase_timeout: self.config.phase_timeout,
            };
            let runner = WorkerRunner::new(
                self.id,
                task_id.to_string(),
                self.event_tx.clone(),
                self.shutdown.clone(),
                runner_config,
            );

            // Phase 1: Implement
            let impl_result = match runner
                .run_implement(task_desc, &self.config.system_prompt, model, worktree_path)
                .await
            {
                Ok(result) => result,
                Err(e) => {
                    return self
                        .handle_failure(task_id, &e.to_string(), retries, total_cost)
                        .await;
                }
            };

            // Accumulate cost metrics
            total_cost += impl_result.cost_usd;
            total_input_tokens += impl_result.input_tokens;
            total_output_tokens += impl_result.output_tokens;

            // Commit after implement phase
            self.git_commit(worktree_path, task_id, &WorkerPhase::Implement)
                .await
                .ok();

            // Direct verify after implement (skipped when verify_commands is empty)
            let verify_report = if !verify_commands.is_empty() {
                self.send_event(WorkerEventKind::PhaseStarted {
                    worker_id: self.id,
                    task_id: task_id.to_string(),
                    phase: WorkerPhase::Verify,
                })
                .await;

                let vr = verify::run_verify_commands(
                    verify_commands,
                    worktree_path,
                    &self.event_tx,
                    self.id,
                )
                .await;

                self.send_event(WorkerEventKind::PhaseCompleted {
                    worker_id: self.id,
                    task_id: task_id.to_string(),
                    phase: WorkerPhase::Verify,
                    success: vr.success,
                })
                .await;

                if vr.success {
                    None
                } else {
                    // Build failure report from failed command
                    let report = vr
                        .results
                        .iter()
                        .filter(|r| !r.success)
                        .map(verify::format_failure_report)
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some(report)
                }
            } else {
                None
            };

            // Phase 2: Review + Fix (with verify report if verify failed)
            let review_result = match runner
                .run_review(
                    &impl_result.output,
                    task_desc,
                    model,
                    worktree_path,
                    verify_report.as_deref(),
                )
                .await
            {
                Ok(result) => result,
                Err(e) => {
                    return self
                        .handle_failure(task_id, &e.to_string(), retries, total_cost)
                        .await;
                }
            };

            // Accumulate cost metrics
            total_cost += review_result.cost_usd;
            total_input_tokens += review_result.input_tokens;
            total_output_tokens += review_result.output_tokens;

            // Commit after review phase
            self.git_commit(worktree_path, task_id, &WorkerPhase::ReviewFix)
                .await
                .ok();

            // Direct verify after review+fix (skipped when verify_commands is empty)
            let verified = if !verify_commands.is_empty() {
                self.send_event(WorkerEventKind::PhaseStarted {
                    worker_id: self.id,
                    task_id: task_id.to_string(),
                    phase: WorkerPhase::Verify,
                })
                .await;

                let vr = verify::run_verify_commands(
                    verify_commands,
                    worktree_path,
                    &self.event_tx,
                    self.id,
                )
                .await;

                self.send_event(WorkerEventKind::PhaseCompleted {
                    worker_id: self.id,
                    task_id: task_id.to_string(),
                    phase: WorkerPhase::Verify,
                    success: vr.success,
                })
                .await;

                vr.success
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
            if retries >= self.config.max_retries {
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
                retries_left: self.config.max_retries - retries,
            })
            .await;
        }
    }

    /// Run setup commands sequentially in the worktree.
    /// On error: logs warning, continues to next command.
    async fn run_setup(&self, task_id: &str, commands: &[(String, String)], worktree_path: &Path) {
        self.send_event(WorkerEventKind::PhaseStarted {
            worker_id: self.id,
            task_id: task_id.to_string(),
            phase: WorkerPhase::Setup,
        })
        .await;

        let mut all_ok = true;
        for (cmd, label) in commands {
            // Send label line to dashboard output
            self.send_event(WorkerEventKind::OutputLines {
                worker_id: self.id,
                lines: vec![format!("$ {label}")],
            })
            .await;

            let timeout_result = tokio::time::timeout(
                self.config.setup_timeout,
                Command::new("sh")
                    .args(["-c", cmd])
                    .current_dir(worktree_path)
                    .output(),
            )
            .await;

            match timeout_result {
                Ok(Ok(out)) => {
                    self.send_output_bytes(&out.stdout).await;
                    self.send_output_bytes(&out.stderr).await;
                    if !out.status.success() {
                        all_ok = false;
                        let code = out.status.code().unwrap_or(-1);
                        self.send_event(WorkerEventKind::OutputLines {
                            worker_id: self.id,
                            lines: vec![format!("⚠ setup command failed (exit {code}): {label}")],
                        })
                        .await;
                    }
                }
                Ok(Err(e)) => {
                    all_ok = false;
                    self.send_event(WorkerEventKind::OutputLines {
                        worker_id: self.id,
                        lines: vec![format!("⚠ setup command error: {e}")],
                    })
                    .await;
                }
                Err(_) => {
                    all_ok = false;
                    let timeout_secs = self.config.setup_timeout.as_secs();
                    self.send_event(WorkerEventKind::OutputLines {
                        worker_id: self.id,
                        lines: vec![format!(
                            "⚠ setup command timed out after {timeout_secs}s: {label}"
                        )],
                    })
                    .await;
                }
            }
        }

        self.send_event(WorkerEventKind::PhaseCompleted {
            worker_id: self.id,
            task_id: task_id.to_string(),
            phase: WorkerPhase::Setup,
            success: all_ok,
        })
        .await;
    }

    /// Send raw bytes as output lines to the dashboard.
    async fn send_output_bytes(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(bytes);
        let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        if !lines.is_empty() {
            self.send_event(WorkerEventKind::OutputLines {
                worker_id: self.id,
                lines,
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
        // Stage all changes with timeout
        let add_output = tokio::time::timeout(
            self.config.git_timeout,
            Command::new("git")
                .args(["add", "-A"])
                .current_dir(worktree_path)
                .output(),
        )
        .await
        .map_err(|_| {
            RalphError::WorktreeError(format!(
                "git add timed out after {}s",
                self.config.git_timeout.as_secs()
            ))
        })?
        .map_err(|e| RalphError::WorktreeError(format!("git add failed: {e}")))?;

        if !add_output.status.success() {
            return Ok(()); // Nothing to commit
        }

        // Check if there's anything to commit with timeout
        let status_output = tokio::time::timeout(
            self.config.git_timeout,
            Command::new("git")
                .args(["status", "--porcelain"])
                .current_dir(worktree_path)
                .output(),
        )
        .await
        .map_err(|_| {
            RalphError::WorktreeError(format!(
                "git status timed out after {}s",
                self.config.git_timeout.as_secs()
            ))
        })?
        .map_err(|e| RalphError::WorktreeError(format!("git status failed: {e}")))?;

        let status_str = String::from_utf8_lossy(&status_output.stdout);
        if status_str.trim().is_empty() {
            return Ok(()); // Nothing to commit
        }

        let msg = format!("wip: {task_id} phase {phase}");
        tokio::time::timeout(
            self.config.git_timeout,
            Command::new("git")
                .args(["commit", "-m", &msg])
                .current_dir(worktree_path)
                .output(),
        )
        .await
        .map_err(|_| {
            RalphError::WorktreeError(format!(
                "git commit timed out after {}s",
                self.config.git_timeout.as_secs()
            ))
        })?
        .map_err(|e| RalphError::WorktreeError(format!("git commit failed: {e}")))?;

        Ok(())
    }

    /// Get list of files changed in the worktree compared to HEAD~.
    async fn get_changed_files(&self, worktree_path: &Path) -> Vec<String> {
        let timeout_result = tokio::time::timeout(
            self.config.git_timeout,
            Command::new("git")
                .args(["diff", "--name-only", "HEAD~1"])
                .current_dir(worktree_path)
                .output(),
        )
        .await;

        match timeout_result {
            Ok(Ok(out)) => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.to_string())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Get the HEAD commit hash in the worktree.
    async fn get_head_hash(&self, worktree_path: &Path) -> Option<String> {
        let output = tokio::time::timeout(
            self.config.git_timeout,
            Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .current_dir(worktree_path)
                .output(),
        )
        .await
        .ok()?
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
        let config = WorkerConfig {
            system_prompt: "system prompt".to_string(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
        };
        let worker = Worker::new(1, tx, shutdown, config);
        assert_eq!(worker.id, 1);
        assert_eq!(worker.config.max_retries, 3);
    }

    #[tokio::test]
    async fn test_worker_send_event() {
        let (tx, mut rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerConfig {
            system_prompt: "system prompt".to_string(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
        };
        let worker = Worker::new(2, tx, shutdown, config);

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
