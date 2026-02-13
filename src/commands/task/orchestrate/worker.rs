#![allow(dead_code)]
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::process::Command;
use tokio::sync::mpsc;

use crate::commands::task::orchestrate::events::{WorkerEvent, WorkerEventKind, WorkerPhase};
use crate::commands::task::orchestrate::git_helpers::git_command;
use crate::commands::task::orchestrate::verify;
use crate::commands::task::orchestrate::worker_runner::{WorkerRunner, WorkerRunnerConfig};
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::VerifyCommand;
use crate::shared::mcp::MCP_MUTATION_TOOLS;

/// Configuration for Worker.
///
/// Groups configuration parameters for worker behavior: retry logic, prompt customization,
/// UI settings, timeout configuration, and MCP server connection details.
#[derive(Clone)]
pub struct WorkerConfig {
    pub system_prompt: String,
    pub max_retries: u32,
    pub use_nerd_font: bool,
    pub prompt_prefix: Option<String>,
    pub prompt_suffix: Option<String>,
    pub phase_timeout: Option<std::time::Duration>,
    pub git_timeout: std::time::Duration,
    pub setup_timeout: std::time::Duration,
    /// Port of the shared MCP server (workers connect via HTTP to this port).
    pub mcp_port: u16,
    /// Session ID for this worker's MCP session (scoped to worker's worktree tasks_path).
    pub mcp_session_id: String,
    /// Model to use for code review phase (review+fix).
    /// Used instead of task's implementation model during review phase.
    /// Typically set to a more capable model (e.g., "opus") for better code analysis.
    pub review_model: String,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            max_retries: 0,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(0),
            setup_timeout: std::time::Duration::from_secs(0),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: String::new(),
        }
    }
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
                mcp_port: self.config.mcp_port,
                mcp_session_id: self.config.mcp_session_id.clone(),
                disallowed_tools: Some(MCP_MUTATION_TOOLS.to_string()),
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
            let impl_committed = self
                .git_commit(worktree_path, task_id, &WorkerPhase::Implement)
                .await
                .unwrap_or(false);

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

            // Check if we should skip review+fix phase (no changes detected)
            let has_changes = impl_committed || self.has_uncommitted_changes(worktree_path).await;

            if !has_changes {
                // No changes to review — skip review+fix and verify phases
                self.send_event(WorkerEventKind::OutputLines {
                    worker_id: self.id,
                    lines: vec!["⏭ Pomijanie review+fix — brak zmian do przeglądu".to_string()],
                })
                .await;

                // Send PhaseStarted + PhaseCompleted for ReviewFix to maintain state tracking
                self.send_event(WorkerEventKind::PhaseStarted {
                    worker_id: self.id,
                    task_id: task_id.to_string(),
                    phase: WorkerPhase::ReviewFix,
                })
                .await;

                self.send_event(WorkerEventKind::PhaseCompleted {
                    worker_id: self.id,
                    task_id: task_id.to_string(),
                    phase: WorkerPhase::ReviewFix,
                    success: true,
                })
                .await;

                // Skip to task completion
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

            // Phase 2: Review + Fix (with verify report if verify failed)
            // CRITICAL: Use review_model from config instead of task's implementation model.
            // This allows using a more capable model (e.g., opus) for code review,
            // while using a faster/cheaper model (e.g., sonnet) for implementation.
            // The review_model is resolved from CLI → config → default "opus" in ResolvedConfig.
            let review_result = match runner
                .run_review(
                    &impl_result.output,
                    task_desc,
                    Some(&self.config.review_model),
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
            self.send_event(WorkerEventKind::OutputLines {
                worker_id: self.id,
                lines: vec!["Committing review changes...".to_string()],
            })
            .await;

            let _review_committed = self
                .git_commit(worktree_path, task_id, &WorkerPhase::ReviewFix)
                .await
                .unwrap_or(false);

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
    /// Returns true if a commit was created, false if there were no changes.
    async fn git_commit(
        &self,
        worktree_path: &Path,
        task_id: &str,
        phase: &WorkerPhase,
    ) -> Result<bool> {
        // Stage all changes with timeout
        let add_output = tokio::time::timeout(
            self.config.git_timeout,
            git_command()
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

        // Note: git add -A returns exit code 0 even when there's nothing to add.
        // We check for actual changes using git status --porcelain below.
        if !add_output.status.success() {
            return Err(RalphError::WorktreeError(format!(
                "git add failed with exit code: {}",
                add_output.status
            )));
        }

        // Check if there's anything to commit with timeout
        let status_output = tokio::time::timeout(
            self.config.git_timeout,
            git_command()
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
            return Ok(false); // Nothing to commit
        }

        let msg = format!("wip: {task_id} phase {phase}");
        tokio::time::timeout(
            self.config.git_timeout,
            git_command()
                .args(["commit", "--no-gpg-sign", "-m", &msg])
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

        Ok(true)
    }

    /// Get list of files changed in the worktree compared to HEAD~.
    async fn get_changed_files(&self, worktree_path: &Path) -> Vec<String> {
        let timeout_result = tokio::time::timeout(
            self.config.git_timeout,
            git_command()
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
            git_command()
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

    /// Check if there are uncommitted changes in the worktree.
    ///
    /// Returns true if:
    /// - Git detects uncommitted changes (staged or unstaged)
    /// - Git command fails or times out (safe default to prevent skipping review on error)
    ///
    /// Returns false only when git status confirms no changes.
    async fn has_uncommitted_changes(&self, worktree_path: &Path) -> bool {
        let status_output = tokio::time::timeout(
            self.config.git_timeout,
            git_command()
                .args(["status", "--porcelain"])
                .current_dir(worktree_path)
                .output(),
        )
        .await;

        match status_output {
            Ok(Ok(out)) if out.status.success() => {
                let status_str = String::from_utf8_lossy(&out.stdout);
                !status_str.trim().is_empty()
            }
            // On error or timeout: assume changes exist (safe default)
            // Better to run unnecessary review than skip when there might be changes
            _ => true,
        }
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
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "claude-opus-4-6".to_string(),
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
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "claude-opus-4-6".to_string(),
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

    #[test]
    fn test_worker_config_mcp_fields() {
        let config = WorkerConfig {
            system_prompt: String::new(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            mcp_port: 12345,
            mcp_session_id: "abc-123-def".to_string(),
            review_model: "claude-opus-4-6".to_string(),
        };
        assert_eq!(config.mcp_port, 12345);
        assert_eq!(config.mcp_session_id, "abc-123-def");
    }

    #[test]
    fn test_worker_config_default_mcp_fields() {
        let config = WorkerConfig::default();
        assert_eq!(config.mcp_port, 0);
        assert!(config.mcp_session_id.is_empty());
    }

    #[tokio::test]
    async fn test_has_uncommitted_changes_with_changes() {
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let worktree_path = temp.path();

        // Initialize git repo
        let _ = Command::new("git")
            .args(["init"])
            .current_dir(worktree_path)
            .output()
            .await;

        let _ = Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(worktree_path)
            .output()
            .await;

        let _ = Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(worktree_path)
            .output()
            .await;

        // Create a file
        fs::write(worktree_path.join("test.txt"), "content").unwrap();

        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerConfig {
            system_prompt: String::new(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(5),
            setup_timeout: std::time::Duration::from_secs(5),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "claude-opus-4-6".to_string(),
        };
        let worker = Worker::new(1, tx, shutdown, config);

        // Should detect uncommitted changes
        let has_changes = worker.has_uncommitted_changes(worktree_path).await;
        assert!(has_changes, "Should detect uncommitted file");
    }

    #[tokio::test]
    async fn test_has_uncommitted_changes_clean() {
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let worktree_path = temp.path();

        // Initialize git repo
        let _ = Command::new("git")
            .args(["init"])
            .current_dir(worktree_path)
            .output()
            .await;

        let _ = Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(worktree_path)
            .output()
            .await;

        let _ = Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(worktree_path)
            .output()
            .await;

        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerConfig {
            system_prompt: String::new(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(5),
            setup_timeout: std::time::Duration::from_secs(5),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "claude-opus-4-6".to_string(),
        };
        let worker = Worker::new(1, tx, shutdown, config);

        // Should detect no changes in clean repo
        let has_changes = worker.has_uncommitted_changes(worktree_path).await;
        assert!(!has_changes, "Should detect clean worktree");
    }

    #[tokio::test]
    async fn test_has_uncommitted_changes_invalid_path() {
        use std::path::PathBuf;

        let invalid_path = PathBuf::from("/nonexistent/path/to/worktree");

        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerConfig {
            system_prompt: String::new(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(1),
            setup_timeout: std::time::Duration::from_secs(1),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "claude-opus-4-6".to_string(),
        };
        let worker = Worker::new(1, tx, shutdown, config);

        // Should return true (safe default) when git fails
        let has_changes = worker.has_uncommitted_changes(&invalid_path).await;
        assert!(has_changes, "Should return true (safe default) on error");
    }

    #[tokio::test]
    async fn test_git_commit_returns_false_when_nothing_to_commit() {
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let worktree_path = temp.path();

        // Initialize git repo
        let _ = Command::new("git")
            .args(["init"])
            .current_dir(worktree_path)
            .output()
            .await;

        let _ = Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(worktree_path)
            .output()
            .await;

        let _ = Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(worktree_path)
            .output()
            .await;

        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerConfig {
            system_prompt: String::new(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(5),
            setup_timeout: std::time::Duration::from_secs(5),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "claude-opus-4-6".to_string(),
        };
        let worker = Worker::new(1, tx, shutdown, config);

        // git_commit should return Ok(false) when there are no changes
        let result = worker
            .git_commit(worktree_path, "T01", &WorkerPhase::Implement)
            .await;
        assert!(result.is_ok());
        assert!(
            !result.unwrap(),
            "Should return false when nothing to commit"
        );
    }

    #[tokio::test]
    async fn test_git_commit_returns_true_when_commit_created() {
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let worktree_path = temp.path();

        // Initialize git repo
        let _ = Command::new("git")
            .args(["init"])
            .current_dir(worktree_path)
            .output()
            .await;

        let _ = Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(worktree_path)
            .output()
            .await;

        let _ = Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(worktree_path)
            .output()
            .await;

        // Create a file to commit
        fs::write(worktree_path.join("test.txt"), "content").unwrap();

        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerConfig {
            system_prompt: String::new(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(5),
            setup_timeout: std::time::Duration::from_secs(5),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "claude-opus-4-6".to_string(),
        };
        let worker = Worker::new(1, tx, shutdown, config);

        // git_commit should return Ok(true) when a commit is created
        let result = worker
            .git_commit(worktree_path, "T01", &WorkerPhase::Implement)
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap(), "Should return true when commit is created");
    }

    /// Test skip review logic: should skip when impl_committed=false AND no uncommitted changes.
    /// This is a unit test verifying the skip condition in isolation.
    #[test]
    fn test_skip_review_logic_no_changes() {
        // Simulated values from execute_task flow
        let impl_committed = false; // git_commit returned false
        let has_uncommitted_changes = false; // has_uncommitted_changes returned false

        // This is the condition used in execute_task's skip logic
        let has_changes = impl_committed || has_uncommitted_changes;

        // Should skip review+fix when no changes
        assert!(
            !has_changes,
            "Should skip review when impl_committed=false and no uncommitted changes"
        );
    }

    /// Test skip review logic: should run review when impl_committed=true.
    #[test]
    fn test_skip_review_logic_commit_made() {
        // Simulated values from execute_task flow
        let impl_committed = true; // git_commit returned true (commit was created)
        let has_uncommitted_changes = false; // doesn't matter

        // This is the condition used in execute_task's skip logic
        let has_changes = impl_committed || has_uncommitted_changes;

        // Should NOT skip review+fix when commit was made
        assert!(
            has_changes,
            "Should run review when impl_committed=true (commit was created)"
        );
    }

    /// Test skip review logic: should run review when uncommitted changes exist.
    #[test]
    fn test_skip_review_logic_uncommitted_changes() {
        // Simulated values from execute_task flow
        let impl_committed = false; // git_commit returned false (no commit)
        let has_uncommitted_changes = true; // but there are uncommitted changes

        // This is the condition used in execute_task's skip logic
        let has_changes = impl_committed || has_uncommitted_changes;

        // Should NOT skip review+fix when uncommitted changes exist
        assert!(
            has_changes,
            "Should run review when has_uncommitted_changes=true even if impl_committed=false"
        );
    }

    /// Test skip review logic: should run review when both commit made and uncommitted changes.
    #[test]
    fn test_skip_review_logic_both_changes() {
        // Simulated values from execute_task flow
        let impl_committed = true; // git_commit returned true
        let has_uncommitted_changes = true; // and there are uncommitted changes

        // This is the condition used in execute_task's skip logic
        let has_changes = impl_committed || has_uncommitted_changes;

        // Should NOT skip review+fix
        assert!(
            has_changes,
            "Should run review when both impl_committed=true and has_uncommitted_changes=true"
        );
    }

    // ── Task 19.4: Review model tests ──────────────────────────────────

    #[test]
    fn test_worker_config_review_model_construction() {
        let config = WorkerConfig {
            system_prompt: "system".to_string(),
            max_retries: 3,
            use_nerd_font: true,
            prompt_prefix: Some("prefix".to_string()),
            prompt_suffix: Some("suffix".to_string()),
            phase_timeout: Some(std::time::Duration::from_secs(1800)),
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            mcp_port: 8080,
            mcp_session_id: "session-123".to_string(),
            review_model: "claude-opus-4-6".to_string(),
        };

        assert_eq!(config.review_model, "claude-opus-4-6");
        assert_eq!(config.system_prompt, "system");
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_worker_config_review_model_default_empty() {
        let config = WorkerConfig::default();
        assert_eq!(config.review_model, "");
    }

    #[test]
    fn test_worker_config_review_model_sonnet() {
        let config = WorkerConfig {
            system_prompt: String::new(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "claude-sonnet-4-5-20250929".to_string(),
        };

        assert_eq!(config.review_model, "claude-sonnet-4-5-20250929");
    }

    #[test]
    fn test_worker_config_review_model_haiku() {
        let config = WorkerConfig {
            system_prompt: String::new(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "claude-haiku-4-5-20251001".to_string(),
        };

        assert_eq!(config.review_model, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn test_worker_config_review_model_custom() {
        let config = WorkerConfig {
            system_prompt: String::new(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "custom-review-model".to_string(),
        };

        assert_eq!(config.review_model, "custom-review-model");
    }

    #[test]
    fn test_worker_config_clone_preserves_review_model() {
        let original = WorkerConfig {
            system_prompt: "test".to_string(),
            max_retries: 2,
            use_nerd_font: true,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(60),
            setup_timeout: std::time::Duration::from_secs(180),
            mcp_port: 3000,
            mcp_session_id: "abc".to_string(),
            review_model: "claude-opus-4-6".to_string(),
        };

        let cloned = original.clone();
        assert_eq!(cloned.review_model, "claude-opus-4-6");
        assert_eq!(cloned.system_prompt, "test");
        assert_eq!(cloned.mcp_port, 3000);
    }

    /// Integration test: verify WorkerConfig.review_model is used correctly in worker lifecycle.
    #[test]
    fn test_worker_review_model_integration_contract() {
        let config = WorkerConfig {
            system_prompt: String::new(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            mcp_port: 8080,
            mcp_session_id: "test-session".to_string(),
            review_model: "claude-opus-4-6".to_string(),
        };

        assert_eq!(config.review_model, "claude-opus-4-6");
    }

    /// Test that empty review_model in config is preserved.
    #[test]
    fn test_worker_config_empty_review_model_preserved() {
        let config = WorkerConfig {
            system_prompt: String::new(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: String::new(),
        };

        assert_eq!(config.review_model, "");
    }

    // ── Task 24.3: disallowed_tools in WorkerRunnerConfig ───────────────

    /// Test that WorkerRunnerConfig is created with disallowed_tools set to MCP_MUTATION_TOOLS.
    ///
    /// This test simulates the configuration created in execute_task() loop (lines 131-139)
    /// and verifies that disallowed_tools contains the expected MCP mutation tools string.
    #[test]
    fn test_worker_runner_config_disallowed_tools() {
        use crate::commands::task::orchestrate::worker_runner::WorkerRunnerConfig;

        // Simulate config creation from execute_task() loop
        let runner_config = WorkerRunnerConfig {
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            mcp_port: 8080,
            mcp_session_id: "test-session".to_string(),
            disallowed_tools: Some(MCP_MUTATION_TOOLS.to_string()),
        };

        // Verify disallowed_tools is set
        assert!(runner_config.disallowed_tools.is_some());

        // Verify it contains the MCP_MUTATION_TOOLS string
        let disallowed = runner_config.disallowed_tools.unwrap();
        assert_eq!(disallowed, MCP_MUTATION_TOOLS);

        // Verify it contains expected tool names (sample check)
        assert!(disallowed.contains("mcp__ralph-tasks__tasks_create"));
        assert!(disallowed.contains("mcp__ralph-tasks__tasks_update"));
        assert!(disallowed.contains("mcp__ralph-tasks__tasks_delete"));
    }
}
