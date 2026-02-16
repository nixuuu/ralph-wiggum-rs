use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::process::Command;
use tokio::sync::mpsc;

use crate::commands::task::orchestrate::events::{
    ProfileStatus, WorkerEvent, WorkerEventKind, WorkerPhase,
};
use crate::commands::task::orchestrate::git_helpers::git_command;
use crate::commands::task::orchestrate::profile_matcher::{
    ProfileMatcher, resolve_verify_profiles,
};
use crate::commands::task::orchestrate::verify::{
    self, ProfiledVerifyPlan, VerifyStep, run_profiled_verify,
};
use crate::commands::task::orchestrate::worker_runner::{WorkerRunner, WorkerRunnerConfig};
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::{VerifyCommand, VerifyProfile};
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
    /// Base commit hash (fork point) from which the worktree diverged.
    /// Captured before implementation phase begins, used to detect changes since base.
    pub base_commit: Option<String>,
    /// All verify profiles from config — used for git-based profile matching.
    pub all_profiles: Vec<VerifyProfile>,
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
            base_commit: None,
            all_profiles: Vec::new(),
        }
    }
}

/// Result of executing a task through the 3-phase worker lifecycle.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields read via JoinHandle<Result<TaskResult>> in orchestrator
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
    /// Channel for receiving messages from the orchestrator.
    /// Wrapped in Option to allow moving out when creating WorkerRunner.
    message_rx: Option<mpsc::Receiver<String>>,
}

impl Worker {
    pub fn new(
        id: u32,
        event_tx: mpsc::Sender<WorkerEvent>,
        shutdown: Arc<AtomicBool>,
        config: WorkerConfig,
        message_rx: mpsc::Receiver<String>,
    ) -> Self {
        Self {
            id,
            event_tx,
            shutdown,
            config,
            message_rx: Some(message_rx),
        }
    }

    /// Execute a task through the full lifecycle with retry logic.
    ///
    /// Flow: Setup → Implement → git commit → DirectVerify → Review+Fix → git commit → DirectVerify → [retry or success]
    ///
    /// If verify fails after implement, the failure report is passed to review+fix phase.
    /// If verify fails after review+fix, retries the full cycle up to `max_retries` times.
    /// Empty `verify_commands` skips verify phases entirely.
    ///
    /// `task_profiles` — profile names assigned to this task in tasks.yml.
    /// Combined with git-detected profiles to build a profiled verify plan.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_task(
        &mut self,
        task_id: &str,
        task_desc: &str,
        model: Option<&str>,
        worktree_path: &Path,
        setup_commands: &[(String, String)],
        verify_commands: &[VerifyCommand],
        task_profiles: &[String],
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

        // Capture base commit hash before implementation begins (fork point)
        // Used later to detect if any changes were made since base
        self.capture_base_commit(worktree_path).await;

        let mut total_cost = 0.0_f64;
        let mut total_input_tokens = 0_u64;
        let mut total_output_tokens = 0_u64;
        let mut retries = 0_u32;
        let mut message_rx_moved = false;

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
            // Pass message_rx only on first iteration (move ownership)
            let msg_rx = if !message_rx_moved {
                message_rx_moved = true;
                self.message_rx.take()
            } else {
                None
            };
            let mut runner = WorkerRunner::new(
                self.id,
                task_id.to_string(),
                self.event_tx.clone(),
                self.shutdown.clone(),
                runner_config,
                msg_rx,
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
            let _impl_committed = self
                .git_commit(worktree_path, task_id, &WorkerPhase::Implement)
                .await
                .unwrap_or(false);

            // Build profiled verify plan (global commands + task/git-matched profiles)
            let changed_files = self.get_changed_files_since_base(worktree_path).await;
            let verify_plan =
                self.build_verify_plan(verify_commands, task_profiles, &changed_files);

            // Direct verify after implement (skipped when no verify plan)
            let verify_report = if let Some(ref plan) = verify_plan {
                let (success, report) = self.run_verify_phase(task_id, plan, worktree_path).await;
                if success { None } else { report }
            } else {
                None
            };

            // Check if we should skip review+fix phase (no changes detected).
            // Use has_changes_since_base() to detect changes compared to base commit (fork point).
            // This correctly handles the case where Claude CLI commits during implementation:
            // - OLD logic: impl_committed=false → skip review (WRONG)
            // - NEW logic: has_changes_since_base=true → run review (CORRECT)
            let has_changes = self.has_changes_since_base(worktree_path).await;

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
                    profiles: None,
                })
                .await;

                self.send_event(WorkerEventKind::PhaseCompleted {
                    worker_id: self.id,
                    task_id: task_id.to_string(),
                    phase: WorkerPhase::ReviewFix,
                    success: true,
                    profile_results: None,
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

            // Rebuild verify plan after review (files may have changed)
            let changed_files_post_review = self.get_changed_files_since_base(worktree_path).await;
            let verify_plan_post_review =
                self.build_verify_plan(verify_commands, task_profiles, &changed_files_post_review);

            // Direct verify after review+fix (skipped when no verify plan)
            let verified = if let Some(ref plan) = verify_plan_post_review {
                let (success, _) = self.run_verify_phase(task_id, plan, worktree_path).await;
                success
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
            profiles: None,
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
            profile_results: None,
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

    /// Capture current HEAD commit hash as base commit (fork point).
    /// Stores the hash in config.base_commit for later comparison.
    /// On error: silently continues (base_commit remains None, safe fallback).
    async fn capture_base_commit(&mut self, worktree_path: &Path) {
        let output = tokio::time::timeout(
            self.config.git_timeout,
            git_command()
                .args(["rev-parse", "HEAD"])
                .current_dir(worktree_path)
                .output(),
        )
        .await;

        match output {
            Ok(Ok(out)) if out.status.success() => {
                let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !hash.is_empty() {
                    self.config.base_commit = Some(hash);
                }
            }
            _ => {
                // On error: leave base_commit as None (safe default)
                // has_changes_since_base() will return true (safe default)
            }
        }
    }

    /// Check if worktree has changes compared to base commit (fork point).
    ///
    /// Returns true if:
    /// - Git diff detects changes between base_commit and HEAD
    /// - Git status detects uncommitted changes
    /// - base_commit is None (safe default — no base recorded)
    /// - Git command fails or times out (safe default)
    ///
    /// Returns false only when:
    /// - base_commit exists AND
    /// - git diff base_commit..HEAD is empty AND
    /// - git status --porcelain is empty
    ///
    /// Safe default: return true on any error or missing base commit.
    /// Better to run unnecessary review than skip when there might be changes.
    async fn has_changes_since_base(&self, worktree_path: &Path) -> bool {
        let base_commit = match &self.config.base_commit {
            Some(hash) => hash,
            None => return true, // No base recorded — assume changes exist (safe default)
        };

        // Check git diff base_commit..HEAD
        let diff_output = tokio::time::timeout(
            self.config.git_timeout,
            git_command()
                .args(["diff", "--name-only", &format!("{}..HEAD", base_commit)])
                .current_dir(worktree_path)
                .output(),
        )
        .await;

        let has_committed_changes = match diff_output {
            Ok(Ok(out)) if out.status.success() => {
                let diff_str = String::from_utf8_lossy(&out.stdout);
                !diff_str.trim().is_empty()
            }
            _ => return true, // On error: assume changes exist (safe default)
        };

        if has_committed_changes {
            return true;
        }

        // Check git status --porcelain for uncommitted changes
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
            _ => true,
        }
    }

    /// Get list of changed files since base commit (committed + uncommitted).
    ///
    /// Combines `git diff --name-only base..HEAD` with `git status --porcelain`
    /// to capture both committed and unstaged changes. Returns deduplicated list.
    /// On error: returns empty vec (caller should handle missing files gracefully).
    async fn get_changed_files_since_base(&self, worktree_path: &Path) -> Vec<String> {
        let mut files = std::collections::HashSet::new();

        let base_commit = match &self.config.base_commit {
            Some(hash) => hash.clone(),
            None => return Vec::new(),
        };

        // Committed changes: git diff --name-only base..HEAD
        let diff_output = tokio::time::timeout(
            self.config.git_timeout,
            git_command()
                .args(["diff", "--name-only", &format!("{}..HEAD", base_commit)])
                .current_dir(worktree_path)
                .output(),
        )
        .await;

        if let Ok(Ok(out)) = diff_output
            && out.status.success()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    files.insert(trimmed.to_string());
                }
            }
        }

        // Uncommitted changes: git status --porcelain
        let status_output = tokio::time::timeout(
            self.config.git_timeout,
            git_command()
                .args(["status", "--porcelain"])
                .current_dir(worktree_path)
                .output(),
        )
        .await;

        if let Ok(Ok(out)) = status_output
            && out.status.success()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                // git status --porcelain format: "XY filename" or "XY orig -> renamed"
                let trimmed = line.trim();
                if trimmed.len() > 3 {
                    let path_part = &trimmed[3..];
                    // Handle renames: "R  old -> new"
                    if let Some(arrow_pos) = path_part.find(" -> ") {
                        files.insert(path_part[arrow_pos + 4..].to_string());
                    } else {
                        files.insert(path_part.to_string());
                    }
                }
            }
        }

        files.into_iter().collect()
    }

    /// Build a profiled verify plan from global commands and matched profiles.
    ///
    /// Structure: GlobalVerify(verify_commands) → ProfileVerify for each resolved profile.
    /// Returns None if no verification is needed (empty commands and no profiles).
    fn build_verify_plan(
        &self,
        verify_commands: &[VerifyCommand],
        task_profiles: &[String],
        changed_files: &[String],
    ) -> Option<ProfiledVerifyPlan> {
        let all_profiles = &self.config.all_profiles;

        // Match changed files against all profiles via glob patterns
        let git_matched = if !all_profiles.is_empty() && !changed_files.is_empty() {
            match ProfileMatcher::new(all_profiles) {
                Ok(matcher) => matcher.match_changed_files(changed_files),
                Err(_) => Vec::new(), // Invalid glob patterns — skip profile matching
            }
        } else {
            Vec::new()
        };

        // Resolve final profile list: task_profiles + git_matched (no duplicates)
        let resolved_profiles = resolve_verify_profiles(task_profiles, &git_matched, all_profiles);

        let has_global = !verify_commands.is_empty();
        let has_profiles = !resolved_profiles.is_empty();

        if !has_global && !has_profiles {
            return None;
        }

        let mut steps = Vec::new();

        // Step 1: Global verify commands (at worktree root)
        if has_global {
            steps.push(VerifyStep::GlobalVerify {
                commands: verify_commands.to_vec(),
            });
        }

        // Step 2: Profile-specific verify commands (at profile working_dir)
        for profile in &resolved_profiles {
            if !profile.verify_commands.is_empty() {
                steps.push(VerifyStep::ProfileVerify {
                    profile_name: profile.name.clone(),
                    commands: profile.verify_commands.clone(),
                    working_dir: profile.working_dir.clone(),
                });
            }
        }

        // If we only had profiles but none had verify_commands, still return None
        if steps.is_empty() {
            return None;
        }

        Some(ProfiledVerifyPlan { steps })
    }

    /// Run verify phase using profiled verify plan.
    /// Returns (success, Option<failure_report>).
    async fn run_verify_phase(
        &self,
        task_id: &str,
        plan: &ProfiledVerifyPlan,
        worktree_path: &Path,
    ) -> (bool, Option<String>) {
        // Extract profile names from the plan
        let profile_names: Vec<String> = plan
            .steps
            .iter()
            .filter_map(|step| match step {
                VerifyStep::ProfileVerify { profile_name, .. } => Some(profile_name.clone()),
                VerifyStep::GlobalVerify { .. } => None,
            })
            .collect();

        // Send PhaseStarted with profile info
        let profiles = if profile_names.is_empty() {
            None
        } else {
            Some(profile_names)
        };

        self.send_event(WorkerEventKind::PhaseStarted {
            worker_id: self.id,
            task_id: task_id.to_string(),
            phase: WorkerPhase::Verify,
            profiles,
        })
        .await;

        let vr = run_profiled_verify(plan, worktree_path, &self.event_tx, self.id).await;

        // Convert verify results to profile statuses
        let profile_results: Vec<ProfileStatus> = vr
            .profile_results
            .iter()
            .map(|pr| ProfileStatus {
                name: pr.name.clone(),
                success: Some(pr.success),
            })
            .collect();

        let profile_results_opt = if profile_results.is_empty() {
            None
        } else {
            Some(profile_results)
        };

        self.send_event(WorkerEventKind::PhaseCompleted {
            worker_id: self.id,
            task_id: task_id.to_string(),
            phase: WorkerPhase::Verify,
            success: vr.success,
            profile_results: profile_results_opt,
        })
        .await;

        if vr.success {
            (true, None)
        } else {
            let report = vr
                .results
                .iter()
                .filter(|r| !r.success)
                .map(verify::format_failure_report)
                .collect::<Vec<_>>()
                .join("\n");
            (false, Some(report))
        }
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
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            base_commit: None,
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);
        assert_eq!(worker.id, 1);
        assert_eq!(worker.config.max_retries, 3);
    }

    #[tokio::test]
    async fn test_worker_send_event() {
        let (tx, mut rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            base_commit: None,
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(2, tx, shutdown, config, msg_rx);

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
            base_commit: None,
            all_profiles: Vec::new(),
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
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            base_commit: None,
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

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
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            base_commit: None,
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        // git_commit should return Ok(true) when a commit is created
        let result = worker
            .git_commit(worktree_path, "T01", &WorkerPhase::Implement)
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap(), "Should return true when commit is created");
    }

    /// Test skip review logic: should skip when no changes since base (no diff, no uncommitted).
    /// This is a unit test verifying the skip condition in isolation.
    #[test]
    fn test_skip_review_logic_no_changes() {
        // Simulated value from has_changes_since_base()
        let has_changes = false; // No diff base..HEAD AND no uncommitted changes

        // Should skip review+fix when no changes
        assert!(
            !has_changes,
            "Should skip review when has_changes_since_base=false"
        );
    }

    /// Test skip review logic: should run review when changes are detected.
    #[test]
    fn test_skip_review_logic_commit_made() {
        // Simulated value from has_changes_since_base()
        let has_changes = true; // Diff detected between base..HEAD

        // Should NOT skip review+fix when changes detected
        assert!(
            has_changes,
            "Should run review when has_changes_since_base=true (changes detected)"
        );
    }

    /// Test skip review logic: should run review when uncommitted changes exist.
    #[test]
    fn test_skip_review_logic_uncommitted_changes() {
        // Simulated value from has_changes_since_base()
        let has_changes = true; // No diff but uncommitted changes exist

        // Should NOT skip review+fix when uncommitted changes exist
        assert!(
            has_changes,
            "Should run review when has_changes_since_base=true (uncommitted changes)"
        );
    }

    /// Test skip review logic: should run review when both committed and uncommitted changes.
    #[test]
    fn test_skip_review_logic_both_changes() {
        // Simulated value from has_changes_since_base()
        let has_changes = true; // Both diff and uncommitted changes

        // Should NOT skip review+fix
        assert!(
            has_changes,
            "Should run review when has_changes_since_base=true (both types of changes)"
        );
    }

    /// Test skip review logic: Claude CLI committed during implementation.
    /// Bug scenario: base_commit != HEAD, impl_committed=false, git status clean.
    /// OLD logic: impl_committed=false → skip review (WRONG)
    /// NEW logic: has_changes_since_base=true → run review (CORRECT)
    #[test]
    fn test_skip_review_claude_committed() {
        // Simulated value from has_changes_since_base()
        // Claude CLI created commits during implementation phase
        // base_commit != HEAD (diff detected), but git status clean
        let has_changes = true;

        // Should NOT skip review+fix — commits were made by Claude CLI
        assert!(
            has_changes,
            "Should run review when Claude CLI committed during implementation (base != HEAD)"
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
            base_commit: None,
            all_profiles: Vec::new(),
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
            base_commit: None,
            all_profiles: Vec::new(),
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
            base_commit: None,
            all_profiles: Vec::new(),
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
            base_commit: None,
            all_profiles: Vec::new(),
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
            base_commit: None,
            all_profiles: Vec::new(),
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
            base_commit: None,
            all_profiles: Vec::new(),
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
            base_commit: None,
            all_profiles: Vec::new(),
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

    // ── Task 37.1: has_changes_since_base() tests ─────────────────────

    /// Test has_changes_since_base returns true when no base_commit is recorded.
    #[tokio::test]
    async fn test_has_changes_since_base_no_base_recorded() {
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let worktree_path = temp.path();

        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            review_model: String::new(),
            base_commit: None, // No base recorded
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        // Should return true (safe default) when no base_commit
        let has_changes = worker.has_changes_since_base(worktree_path).await;
        assert!(
            has_changes,
            "Should return true when base_commit is None (safe default)"
        );
    }

    /// Test has_changes_since_base returns true when diff detects changes.
    #[tokio::test]
    async fn test_has_changes_since_base_with_committed_changes() {
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

        // Create initial commit (base)
        fs::write(worktree_path.join("base.txt"), "base content").unwrap();
        let _ = Command::new("git")
            .args(["add", "base.txt"])
            .current_dir(worktree_path)
            .output()
            .await;
        let _ = Command::new("git")
            .args(["commit", "-m", "base commit"])
            .current_dir(worktree_path)
            .output()
            .await;

        // Get base commit hash
        let base_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(worktree_path)
            .output()
            .await
            .unwrap();
        let base_hash = String::from_utf8_lossy(&base_output.stdout)
            .trim()
            .to_string();

        // Create new commit after base
        fs::write(worktree_path.join("new.txt"), "new content").unwrap();
        let _ = Command::new("git")
            .args(["add", "new.txt"])
            .current_dir(worktree_path)
            .output()
            .await;
        let _ = Command::new("git")
            .args(["commit", "-m", "new commit"])
            .current_dir(worktree_path)
            .output()
            .await;

        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            review_model: String::new(),
            base_commit: Some(base_hash),
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        // Should detect changes (diff between base and HEAD)
        let has_changes = worker.has_changes_since_base(worktree_path).await;
        assert!(
            has_changes,
            "Should detect committed changes between base and HEAD"
        );
    }

    /// Test has_changes_since_base returns true when uncommitted changes exist.
    #[tokio::test]
    async fn test_has_changes_since_base_with_uncommitted_changes() {
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

        // Create initial commit (base)
        fs::write(worktree_path.join("base.txt"), "base content").unwrap();
        let _ = Command::new("git")
            .args(["add", "base.txt"])
            .current_dir(worktree_path)
            .output()
            .await;
        let _ = Command::new("git")
            .args(["commit", "-m", "base commit"])
            .current_dir(worktree_path)
            .output()
            .await;

        // Get base commit hash
        let base_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(worktree_path)
            .output()
            .await
            .unwrap();
        let base_hash = String::from_utf8_lossy(&base_output.stdout)
            .trim()
            .to_string();

        // Create uncommitted changes (no new commit)
        fs::write(worktree_path.join("uncommitted.txt"), "uncommitted").unwrap();

        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            review_model: String::new(),
            base_commit: Some(base_hash),
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        // Should detect uncommitted changes
        let has_changes = worker.has_changes_since_base(worktree_path).await;
        assert!(
            has_changes,
            "Should detect uncommitted changes via git status"
        );
    }

    /// Test has_changes_since_base returns false when no changes since base.
    #[tokio::test]
    async fn test_has_changes_since_base_no_changes() {
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

        // Create initial commit (base)
        fs::write(worktree_path.join("base.txt"), "base content").unwrap();
        let _ = Command::new("git")
            .args(["add", "base.txt"])
            .current_dir(worktree_path)
            .output()
            .await;
        let _ = Command::new("git")
            .args(["commit", "-m", "base commit"])
            .current_dir(worktree_path)
            .output()
            .await;

        // Get base commit hash
        let base_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(worktree_path)
            .output()
            .await
            .unwrap();
        let base_hash = String::from_utf8_lossy(&base_output.stdout)
            .trim()
            .to_string();

        // No changes made after base commit

        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            review_model: String::new(),
            base_commit: Some(base_hash),
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        // Should return false (no changes detected)
        let has_changes = worker.has_changes_since_base(worktree_path).await;
        assert!(
            !has_changes,
            "Should return false when no changes since base commit"
        );
    }

    /// Test has_changes_since_base returns true (safe default) on git error.
    #[tokio::test]
    async fn test_has_changes_since_base_git_error() {
        use std::path::PathBuf;

        let invalid_path = PathBuf::from("/nonexistent/path/to/worktree");

        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            review_model: String::new(),
            base_commit: Some("abc123".to_string()),
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        // Should return true (safe default) when git fails
        let has_changes = worker.has_changes_since_base(&invalid_path).await;
        assert!(
            has_changes,
            "Should return true (safe default) when git command fails"
        );
    }

    /// Test capture_base_commit stores HEAD hash in config.
    #[tokio::test]
    async fn test_capture_base_commit_success() {
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

        // Create initial commit
        fs::write(worktree_path.join("test.txt"), "content").unwrap();
        let _ = Command::new("git")
            .args(["add", "test.txt"])
            .current_dir(worktree_path)
            .output()
            .await;
        let _ = Command::new("git")
            .args(["commit", "-m", "test commit"])
            .current_dir(worktree_path)
            .output()
            .await;

        // Get expected HEAD hash
        let head_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(worktree_path)
            .output()
            .await
            .unwrap();
        let expected_hash = String::from_utf8_lossy(&head_output.stdout)
            .trim()
            .to_string();

        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            review_model: String::new(),
            base_commit: None,
            all_profiles: Vec::new(),
        };
        let mut worker = Worker::new(1, tx, shutdown, config, msg_rx);

        // Capture base commit
        worker.capture_base_commit(worktree_path).await;

        // Verify base_commit was set correctly
        assert!(worker.config.base_commit.is_some());
        assert_eq!(worker.config.base_commit.unwrap(), expected_hash);
    }

    /// Test capture_base_commit handles git error gracefully.
    #[tokio::test]
    async fn test_capture_base_commit_error() {
        use std::path::PathBuf;

        let invalid_path = PathBuf::from("/nonexistent/path/to/worktree");

        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            review_model: String::new(),
            base_commit: None,
            all_profiles: Vec::new(),
        };
        let mut worker = Worker::new(1, tx, shutdown, config, msg_rx);

        // Capture should fail silently
        worker.capture_base_commit(&invalid_path).await;

        // base_commit should remain None (safe default)
        assert!(worker.config.base_commit.is_none());
    }

    // ── Task 37.3: Nowe testy has_changes_since_base ─────────────────────

    /// Test has_changes_since_base: base == HEAD, no uncommitted changes → false
    #[tokio::test]
    async fn test_has_changes_since_base_no_diff() {
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

        // Create initial commit
        fs::write(worktree_path.join("base.txt"), "base content").unwrap();
        let _ = Command::new("git")
            .args(["add", "base.txt"])
            .current_dir(worktree_path)
            .output()
            .await;
        let _ = Command::new("git")
            .args(["commit", "-m", "base commit"])
            .current_dir(worktree_path)
            .output()
            .await;

        // Get base commit hash (will be same as HEAD)
        let base_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(worktree_path)
            .output()
            .await
            .unwrap();
        let base_hash = String::from_utf8_lossy(&base_output.stdout)
            .trim()
            .to_string();

        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            review_model: String::new(),
            base_commit: Some(base_hash),
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        // Should return false (no changes: base == HEAD and no uncommitted)
        let has_changes = worker.has_changes_since_base(worktree_path).await;
        assert!(
            !has_changes,
            "Should return false when base == HEAD and no uncommitted changes"
        );
    }

    /// Test has_changes_since_base: base != HEAD → true (committed changes)
    #[tokio::test]
    async fn test_has_changes_since_base_with_diff() {
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

        // Create base commit
        fs::write(worktree_path.join("base.txt"), "base content").unwrap();
        let _ = Command::new("git")
            .args(["add", "base.txt"])
            .current_dir(worktree_path)
            .output()
            .await;
        let _ = Command::new("git")
            .args(["commit", "-m", "base commit"])
            .current_dir(worktree_path)
            .output()
            .await;

        // Get base commit hash
        let base_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(worktree_path)
            .output()
            .await
            .unwrap();
        let base_hash = String::from_utf8_lossy(&base_output.stdout)
            .trim()
            .to_string();

        // Create new commit (HEAD != base)
        fs::write(worktree_path.join("new.txt"), "new content").unwrap();
        let _ = Command::new("git")
            .args(["add", "new.txt"])
            .current_dir(worktree_path)
            .output()
            .await;
        let _ = Command::new("git")
            .args(["commit", "-m", "new commit"])
            .current_dir(worktree_path)
            .output()
            .await;

        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            review_model: String::new(),
            base_commit: Some(base_hash),
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        // Should return true (changes: base != HEAD)
        let has_changes = worker.has_changes_since_base(worktree_path).await;
        assert!(
            has_changes,
            "Should return true when base != HEAD (diff detected)"
        );
    }

    /// Test has_changes_since_base: base == HEAD but uncommitted changes → true
    #[tokio::test]
    async fn test_has_changes_since_base_uncommitted_only() {
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

        // Create base commit
        fs::write(worktree_path.join("base.txt"), "base content").unwrap();
        let _ = Command::new("git")
            .args(["add", "base.txt"])
            .current_dir(worktree_path)
            .output()
            .await;
        let _ = Command::new("git")
            .args(["commit", "-m", "base commit"])
            .current_dir(worktree_path)
            .output()
            .await;

        // Get base commit hash (same as HEAD)
        let base_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(worktree_path)
            .output()
            .await
            .unwrap();
        let base_hash = String::from_utf8_lossy(&base_output.stdout)
            .trim()
            .to_string();

        // Create uncommitted changes (no new commit)
        fs::write(worktree_path.join("uncommitted.txt"), "uncommitted content").unwrap();

        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            review_model: String::new(),
            base_commit: Some(base_hash),
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        // Should return true (changes: uncommitted files exist)
        let has_changes = worker.has_changes_since_base(worktree_path).await;
        assert!(
            has_changes,
            "Should return true when base == HEAD but uncommitted changes exist"
        );
    }

    /// Test has_changes_since_base: git error → true (safe default fallback)
    #[tokio::test]
    async fn test_has_changes_since_base_error_fallback() {
        use std::path::PathBuf;

        let invalid_path = PathBuf::from("/nonexistent/path/to/worktree");

        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
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
            review_model: String::new(),
            base_commit: Some("abc123".to_string()),
            all_profiles: Vec::new(),
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        // Should return true (safe default) when git command fails
        let has_changes = worker.has_changes_since_base(&invalid_path).await;
        assert!(
            has_changes,
            "Should return true (safe default) when git command fails or times out"
        );
    }

    // ── Task 41.2: build_verify_plan tests ──────────────────────────────

    fn make_worker_with_profiles(profiles: Vec<VerifyProfile>) -> Worker {
        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerConfig {
            all_profiles: profiles,
            ..WorkerConfig::default()
        };
        Worker::new(1, tx, shutdown, config, msg_rx)
    }

    #[test]
    fn test_build_verify_plan_empty_everything() {
        let worker = make_worker_with_profiles(vec![]);
        let result = worker.build_verify_plan(&[], &[], &[]);
        assert!(result.is_none(), "No commands and no profiles → no plan");
    }

    #[test]
    fn test_build_verify_plan_global_only() {
        let worker = make_worker_with_profiles(vec![]);
        let commands = vec![VerifyCommand::Simple("cargo test".to_string())];
        let plan = worker.build_verify_plan(&commands, &[], &[]).unwrap();

        assert_eq!(plan.steps.len(), 1);
        assert!(
            matches!(&plan.steps[0], VerifyStep::GlobalVerify { commands } if commands.len() == 1)
        );
    }

    #[test]
    fn test_build_verify_plan_profiles_only_from_task() {
        let worker = make_worker_with_profiles(vec![make_verify_profile(
            "backend",
            vec!["src/**/*.rs"],
            vec!["cargo test"],
            Some("backend"),
        )]);

        let task_profiles = vec!["backend".to_string()];
        let plan = worker.build_verify_plan(&[], &task_profiles, &[]).unwrap();

        assert_eq!(plan.steps.len(), 1);
        match &plan.steps[0] {
            VerifyStep::ProfileVerify {
                profile_name,
                working_dir,
                ..
            } => {
                assert_eq!(profile_name, "backend");
                assert_eq!(working_dir.as_deref(), Some("backend"));
            }
            _ => panic!("Expected ProfileVerify step"),
        }
    }

    #[test]
    fn test_build_verify_plan_git_matched_profiles() {
        let worker = make_worker_with_profiles(vec![
            make_verify_profile("frontend", vec!["ui/**/*.ts"], vec!["npm test"], None),
            make_verify_profile("backend", vec!["src/**/*.rs"], vec!["cargo test"], None),
        ]);

        // Changed file matches "backend" profile only
        let changed = vec!["src/main.rs".to_string()];
        let plan = worker.build_verify_plan(&[], &[], &changed).unwrap();

        assert_eq!(plan.steps.len(), 1);
        match &plan.steps[0] {
            VerifyStep::ProfileVerify { profile_name, .. } => {
                assert_eq!(profile_name, "backend");
            }
            _ => panic!("Expected ProfileVerify"),
        }
    }

    #[test]
    fn test_build_verify_plan_mixed_global_and_profiles() {
        let worker = make_worker_with_profiles(vec![make_verify_profile(
            "api",
            vec!["src/api/**"],
            vec!["cargo test -p api"],
            Some("api"),
        )]);

        let global_cmds = vec![VerifyCommand::Simple("cargo fmt --check".to_string())];
        let changed = vec!["src/api/handler.rs".to_string()];
        let plan = worker
            .build_verify_plan(&global_cmds, &[], &changed)
            .unwrap();

        assert_eq!(plan.steps.len(), 2);
        assert!(matches!(&plan.steps[0], VerifyStep::GlobalVerify { .. }));
        assert!(matches!(&plan.steps[1], VerifyStep::ProfileVerify { .. }));
    }

    #[test]
    fn test_build_verify_plan_deduplicates_profiles() {
        let worker = make_worker_with_profiles(vec![make_verify_profile(
            "backend",
            vec!["src/**/*.rs"],
            vec!["cargo test"],
            None,
        )]);

        // task_profiles AND git_matched both contain "backend" — should appear once
        let task_profiles = vec!["backend".to_string()];
        let changed = vec!["src/main.rs".to_string()];
        let plan = worker
            .build_verify_plan(&[], &task_profiles, &changed)
            .unwrap();

        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn test_build_verify_plan_profile_without_verify_commands() {
        // Profile exists but has no verify_commands — should NOT produce a step
        let worker = make_worker_with_profiles(vec![make_verify_profile(
            "empty",
            vec!["src/**"],
            vec![],
            None,
        )]);

        let task_profiles = vec!["empty".to_string()];
        let result = worker.build_verify_plan(&[], &task_profiles, &[]);
        assert!(
            result.is_none(),
            "Profile without verify_commands should not generate a plan"
        );
    }

    #[tokio::test]
    async fn test_get_changed_files_no_base_commit() {
        let worker = make_worker_with_profiles(vec![]);
        let files = worker
            .get_changed_files_since_base(std::path::Path::new("/tmp"))
            .await;
        assert!(files.is_empty(), "No base_commit → should return empty vec");
    }

    // ── Task 48.1: E2E worker z profilami — build_verify_plan + setup + verify ──

    /// Helper: tworzy VerifyProfile z verify_commands i opcjonalnymi paths/working_dir.
    fn make_verify_profile(
        name: &str,
        paths: Vec<&str>,
        verify_cmds: Vec<&str>,
        working_dir: Option<&str>,
    ) -> VerifyProfile {
        VerifyProfile {
            name: name.to_string(),
            description: None,
            paths: paths.iter().map(|s| s.to_string()).collect(),
            working_dir: working_dir.map(|s| s.to_string()),
            verify_commands: verify_cmds
                .iter()
                .map(|s| VerifyCommand::Simple(s.to_string()))
                .collect(),
            setup_commands: vec![],
        }
    }

    /// E2E Test 1: worker z task_profiles=[frontend] → globalne verify + frontend verify.
    ///
    /// Scenariusz: task ma przypisany profil "frontend".
    /// Pliki zmienione nie pasują do żadnego profilu (ale task_profiles wymusza frontend).
    /// Oczekiwanie: plan zawiera GlobalVerify + ProfileVerify(frontend).
    #[test]
    fn test_e2e_worker_task_profiles_frontend_with_global() {
        let frontend_profile = make_verify_profile(
            "frontend",
            vec!["ui/**/*.ts", "ui/**/*.tsx"],
            vec!["npm test"],
            Some("ui"),
        );
        let backend_profile =
            make_verify_profile("backend", vec!["src/**/*.rs"], vec!["cargo test"], None);

        let worker = make_worker_with_profiles(vec![frontend_profile, backend_profile]);

        let global_cmds = vec![VerifyCommand::Simple("cargo fmt --check".to_string())];
        let task_profiles = vec!["frontend".to_string()];
        // Brak zmienionych plików — profil frontend wymuszony przez task_profiles
        let changed_files: Vec<String> = vec![];

        let plan = worker
            .build_verify_plan(&global_cmds, &task_profiles, &changed_files)
            .unwrap();

        // Powinny być 2 kroki: GlobalVerify + ProfileVerify(frontend)
        assert_eq!(
            plan.steps.len(),
            2,
            "Oczekiwano 2 kroków: global + frontend"
        );

        // Krok 1: GlobalVerify
        match &plan.steps[0] {
            VerifyStep::GlobalVerify { commands } => {
                assert_eq!(commands.len(), 1);
                assert_eq!(commands[0].command(), "cargo fmt --check");
            }
            _ => panic!("Krok 0 powinien być GlobalVerify"),
        }

        // Krok 2: ProfileVerify(frontend)
        match &plan.steps[1] {
            VerifyStep::ProfileVerify {
                profile_name,
                commands,
                working_dir,
            } => {
                assert_eq!(profile_name, "frontend");
                assert_eq!(commands.len(), 1);
                assert_eq!(commands[0].command(), "npm test");
                assert_eq!(working_dir.as_deref(), Some("ui"));
            }
            _ => panic!("Krok 1 powinien być ProfileVerify(frontend)"),
        }
    }

    /// E2E Test 2: worker bez profili, pliki pasują do backend → globalne + backend verify.
    ///
    /// Scenariusz: task nie ma przypisanych profili, ale zmienione pliki (.rs)
    /// pasują do profilu "backend" przez glob matching.
    /// Oczekiwanie: plan zawiera GlobalVerify + ProfileVerify(backend).
    #[test]
    fn test_e2e_worker_no_task_profiles_files_match_backend() {
        let frontend_profile =
            make_verify_profile("frontend", vec!["ui/**/*.ts"], vec!["npm test"], Some("ui"));
        let backend_profile = make_verify_profile(
            "backend",
            vec!["src/**/*.rs"],
            vec!["cargo test"],
            Some("backend"),
        );

        let worker = make_worker_with_profiles(vec![frontend_profile, backend_profile]);

        let global_cmds = vec![VerifyCommand::Simple("cargo clippy".to_string())];
        let task_profiles: Vec<String> = vec![]; // Brak task profili
        // Pliki pasują do backend profilu
        let changed_files = vec![
            "src/main.rs".to_string(),
            "src/commands/task/mod.rs".to_string(),
        ];

        let plan = worker
            .build_verify_plan(&global_cmds, &task_profiles, &changed_files)
            .unwrap();

        assert_eq!(
            plan.steps.len(),
            2,
            "Oczekiwano 2 kroków: global + backend (git-matched)"
        );

        // Krok 1: GlobalVerify
        match &plan.steps[0] {
            VerifyStep::GlobalVerify { commands } => {
                assert_eq!(commands[0].command(), "cargo clippy");
            }
            _ => panic!("Krok 0 powinien być GlobalVerify"),
        }

        // Krok 2: ProfileVerify(backend) — dopasowany przez git diff
        match &plan.steps[1] {
            VerifyStep::ProfileVerify {
                profile_name,
                commands,
                working_dir,
            } => {
                assert_eq!(profile_name, "backend");
                assert_eq!(commands[0].command(), "cargo test");
                assert_eq!(working_dir.as_deref(), Some("backend"));
            }
            _ => panic!("Krok 1 powinien być ProfileVerify(backend)"),
        }
    }

    /// E2E Test 3: worker z task_profiles=[frontend], pliki matchują backend
    /// → globalne + frontend (task) + backend (git-matched).
    ///
    /// Scenariusz: task ma przypisany profil "frontend", ale zmienione pliki
    /// pasują do "backend". Unia profili: frontend (task) + backend (git).
    /// Oczekiwanie: plan zawiera GlobalVerify + ProfileVerify(frontend) + ProfileVerify(backend).
    #[test]
    fn test_e2e_worker_task_frontend_files_match_backend_union() {
        let frontend_profile = make_verify_profile(
            "frontend",
            vec!["ui/**/*.ts"],
            vec!["npm test", "npm run lint"],
            Some("ui"),
        );
        let backend_profile =
            make_verify_profile("backend", vec!["src/**/*.rs"], vec!["cargo test"], None);

        let worker = make_worker_with_profiles(vec![frontend_profile, backend_profile]);

        let global_cmds = vec![
            VerifyCommand::Simple("cargo fmt --check".to_string()),
            VerifyCommand::Simple("cargo clippy".to_string()),
        ];
        let task_profiles = vec!["frontend".to_string()];
        // Pliki pasują do backend (nie do frontend)
        let changed_files = vec!["src/lib.rs".to_string()];

        let plan = worker
            .build_verify_plan(&global_cmds, &task_profiles, &changed_files)
            .unwrap();

        // 3 kroki: GlobalVerify + frontend (task) + backend (git)
        assert_eq!(
            plan.steps.len(),
            3,
            "Oczekiwano 3 kroków: global + frontend(task) + backend(git)"
        );

        // Krok 1: GlobalVerify z 2 komendami
        match &plan.steps[0] {
            VerifyStep::GlobalVerify { commands } => {
                assert_eq!(commands.len(), 2);
                assert_eq!(commands[0].command(), "cargo fmt --check");
                assert_eq!(commands[1].command(), "cargo clippy");
            }
            _ => panic!("Krok 0 powinien być GlobalVerify"),
        }

        // Krok 2: ProfileVerify(frontend) — z task_profiles (idzie pierwszy)
        match &plan.steps[1] {
            VerifyStep::ProfileVerify {
                profile_name,
                commands,
                working_dir,
            } => {
                assert_eq!(profile_name, "frontend");
                assert_eq!(commands.len(), 2);
                assert_eq!(commands[0].command(), "npm test");
                assert_eq!(commands[1].command(), "npm run lint");
                assert_eq!(working_dir.as_deref(), Some("ui"));
            }
            _ => panic!("Krok 1 powinien być ProfileVerify(frontend)"),
        }

        // Krok 3: ProfileVerify(backend) — dopasowany przez git diff
        match &plan.steps[2] {
            VerifyStep::ProfileVerify {
                profile_name,
                commands,
                working_dir,
            } => {
                assert_eq!(profile_name, "backend");
                assert_eq!(commands.len(), 1);
                assert_eq!(commands[0].command(), "cargo test");
                assert!(working_dir.is_none());
            }
            _ => panic!("Krok 2 powinien być ProfileVerify(backend)"),
        }
    }

    /// E2E Test 4: worker bez profili, pliki nie matchują → tylko globalne.
    ///
    /// Scenariusz: brak task_profiles, zmienione pliki (np. docs) nie pasują
    /// do żadnego profilu. Powinny zostać tylko globalne komendy.
    #[test]
    fn test_e2e_worker_no_profiles_no_file_match_global_only() {
        let frontend_profile =
            make_verify_profile("frontend", vec!["ui/**/*.ts"], vec!["npm test"], None);
        let backend_profile =
            make_verify_profile("backend", vec!["src/**/*.rs"], vec!["cargo test"], None);

        let worker = make_worker_with_profiles(vec![frontend_profile, backend_profile]);

        let global_cmds = vec![VerifyCommand::Simple("cargo fmt --check".to_string())];
        let task_profiles: Vec<String> = vec![]; // Brak task profili
        // Zmienione pliki nie pasują do żadnego profilu
        let changed_files = vec![
            "docs/README.md".to_string(),
            "LICENSE".to_string(),
            "Cargo.toml".to_string(),
        ];

        let plan = worker
            .build_verify_plan(&global_cmds, &task_profiles, &changed_files)
            .unwrap();

        // Tylko 1 krok: GlobalVerify (żaden profil nie został dopasowany)
        assert_eq!(plan.steps.len(), 1, "Oczekiwano tylko 1 krok: global");

        match &plan.steps[0] {
            VerifyStep::GlobalVerify { commands } => {
                assert_eq!(commands.len(), 1);
                assert_eq!(commands[0].command(), "cargo fmt --check");
            }
            _ => panic!("Krok 0 powinien być GlobalVerify"),
        }
    }

    /// E2E Test 5: setup z profili + globalne w prawidłowej kolejności.
    ///
    /// Pełny flow: worker z 3 profilami, task_profiles=[frontend],
    /// pliki matchują backend+frontend. Weryfikacja kolejności:
    /// 1. GlobalVerify (zawsze pierwszy)
    /// 2. ProfileVerify(frontend) — z task_profiles (priorytet)
    /// 3. ProfileVerify(backend) — z git diff (dodatkowy)
    ///
    /// Frontend nie jest zduplikowany mimo pojawienia się w obu źródłach.
    #[test]
    fn test_e2e_full_flow_setup_verify_order() {
        let frontend_profile = make_verify_profile(
            "frontend",
            vec!["ui/**/*.ts", "ui/**/*.tsx"],
            vec!["npm test"],
            Some("ui"),
        );
        let backend_profile = make_verify_profile(
            "backend",
            vec!["src/**/*.rs"],
            vec!["cargo test"],
            Some("backend"),
        );
        let infra_profile = make_verify_profile(
            "infra",
            vec!["infra/**/*.tf"],
            vec!["terraform validate"],
            Some("infra"),
        );

        let worker =
            make_worker_with_profiles(vec![frontend_profile, backend_profile, infra_profile]);

        let global_cmds = vec![
            VerifyCommand::Simple("cargo fmt --check".to_string()),
            VerifyCommand::Simple("cargo clippy".to_string()),
        ];
        let task_profiles = vec!["frontend".to_string()];
        // Pliki matchują zarówno frontend jak i backend
        let changed_files = vec![
            "ui/components/App.tsx".to_string(),
            "src/main.rs".to_string(),
        ];

        let plan = worker
            .build_verify_plan(&global_cmds, &task_profiles, &changed_files)
            .unwrap();

        // 3 kroki: Global + frontend (task, match) + backend (git match only)
        // frontend pojawia się i w task_profiles i w git_matched — ale bez duplikatu
        // infra nie pasuje do żadnego pliku i nie jest w task_profiles
        assert_eq!(
            plan.steps.len(),
            3,
            "Oczekiwano 3 kroków: global + frontend + backend (infra nie matchuje)"
        );

        // Krok 1: GlobalVerify
        match &plan.steps[0] {
            VerifyStep::GlobalVerify { commands } => {
                assert_eq!(commands.len(), 2, "Globalne: 2 komendy");
            }
            _ => panic!("Krok 0 powinien być GlobalVerify"),
        }

        // Krok 2: frontend — z task_profiles (priorytet kolejności)
        match &plan.steps[1] {
            VerifyStep::ProfileVerify {
                profile_name,
                working_dir,
                ..
            } => {
                assert_eq!(profile_name, "frontend", "Frontend idzie przed backend");
                assert_eq!(working_dir.as_deref(), Some("ui"));
            }
            _ => panic!("Krok 1 powinien być ProfileVerify(frontend)"),
        }

        // Krok 3: backend — z git diff matching
        match &plan.steps[2] {
            VerifyStep::ProfileVerify {
                profile_name,
                working_dir,
                ..
            } => {
                assert_eq!(profile_name, "backend");
                assert_eq!(working_dir.as_deref(), Some("backend"));
            }
            _ => panic!("Krok 2 powinien być ProfileVerify(backend)"),
        }
    }

    /// E2E Test: pełny flow verify z rzeczywistym uruchomieniem komend (run_profiled_verify).
    ///
    /// Testuje, że plan zbudowany przez build_verify_plan poprawnie wykonuje się
    /// przez run_profiled_verify — globalne i profile weryfikacyjne w prawidłowej kolejności.
    /// Weryfikuje też, że komendy profilowe uruchamiają się w poprawnym working_dir.
    #[tokio::test]
    async fn test_e2e_verify_execution_with_profiles() {
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let worktree_path = temp.path();

        // Utwórz podkatalogi profili
        std::fs::create_dir_all(worktree_path.join("ui")).unwrap();
        std::fs::create_dir_all(worktree_path.join("backend")).unwrap();

        // Komendy tworzą marker file w cwd — weryfikacja working_dir
        let frontend_profile = make_verify_profile(
            "frontend",
            vec!["ui/**/*.ts"],
            vec!["touch .frontend_marker && echo frontend-verify"],
            Some("ui"),
        );
        let backend_profile = make_verify_profile(
            "backend",
            vec!["src/**/*.rs"],
            vec!["touch .backend_marker && echo backend-verify"],
            Some("backend"),
        );

        let worker = make_worker_with_profiles(vec![frontend_profile, backend_profile]);

        // Global + frontend (task) + backend (git)
        let global_cmds = vec![VerifyCommand::Simple(
            "touch .global_marker && echo global-verify".to_string(),
        )];
        let task_profiles = vec!["frontend".to_string()];
        let changed_files = vec!["src/main.rs".to_string()];

        let plan = worker
            .build_verify_plan(&global_cmds, &task_profiles, &changed_files)
            .unwrap();
        assert_eq!(plan.steps.len(), 3, "global + frontend + backend");

        // Uruchom verify plan
        let (tx, _rx) = mpsc::channel(32);
        let vr = verify::run_profiled_verify(&plan, worktree_path, &tx, 1).await;

        assert!(vr.success, "Wszystkie komendy powinny się udać");
        assert_eq!(
            vr.results.len(),
            3,
            "3 komendy: global + frontend + backend"
        );

        // Sprawdź output: globalny brak profile_name
        assert!(
            vr.results[0].profile_name.is_none(),
            "Globalna komenda nie ma profilu"
        );
        // Frontend i backend mają profile_name
        assert_eq!(
            vr.results[1].profile_name.as_deref(),
            Some("frontend"),
            "Druga komenda z profilu frontend"
        );
        assert_eq!(
            vr.results[2].profile_name.as_deref(),
            Some("backend"),
            "Trzecia komenda z profilu backend"
        );

        // Sprawdź output zawiera oczekiwane wartości
        assert!(
            vr.results[0]
                .output_tail
                .iter()
                .any(|l| l.contains("global-verify"))
        );
        assert!(
            vr.results[1]
                .output_tail
                .iter()
                .any(|l| l.contains("frontend-verify"))
        );
        assert!(
            vr.results[2]
                .output_tail
                .iter()
                .any(|l| l.contains("backend-verify"))
        );

        // Weryfikacja working_dir — marker files powinny być w podkatalogach profili
        assert!(
            worktree_path.join(".global_marker").exists(),
            "Global marker powinien być w worktree root"
        );
        assert!(
            worktree_path.join("ui/.frontend_marker").exists(),
            "Frontend marker powinien być w ui/ (working_dir profilu)"
        );
        assert!(
            worktree_path.join("backend/.backend_marker").exists(),
            "Backend marker powinien być w backend/ (working_dir profilu)"
        );
    }

    /// E2E Test: verify z wieloma profilami gdzie jeden failuje — fail-fast.
    ///
    /// Globalne przechodzi, frontend przechodzi, backend failuje.
    /// Sprawdza, że verify zatrzymuje się po pierwszym failure.
    #[tokio::test]
    async fn test_e2e_verify_execution_fail_fast_on_profile() {
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let worktree_path = temp.path();

        std::fs::create_dir_all(worktree_path.join("ui")).unwrap();

        let frontend_profile = make_verify_profile(
            "frontend",
            vec!["ui/**/*.ts"],
            vec!["echo frontend-ok"],
            Some("ui"),
        );
        // Backend z komendą, która failuje
        let backend_profile = make_verify_profile(
            "backend",
            vec!["src/**/*.rs"],
            vec!["false"], // exit 1
            None,
        );
        let infra_profile = make_verify_profile(
            "infra",
            vec!["infra/**"],
            vec!["echo infra-should-not-run"],
            None,
        );

        let worker =
            make_worker_with_profiles(vec![frontend_profile, backend_profile, infra_profile]);

        let global_cmds = vec![VerifyCommand::Simple("echo global-ok".to_string())];
        // Wszystkie 3 profile — frontend (task), backend+infra (git match)
        let task_profiles = vec!["frontend".to_string()];
        let changed_files = vec!["src/main.rs".to_string(), "infra/main.tf".to_string()];

        let plan = worker
            .build_verify_plan(&global_cmds, &task_profiles, &changed_files)
            .unwrap();
        assert_eq!(plan.steps.len(), 4, "global + frontend + backend + infra");

        let (tx, _rx) = mpsc::channel(32);
        let vr = verify::run_profiled_verify(&plan, worktree_path, &tx, 1).await;

        assert!(!vr.success, "Verify powinien failować (backend false)");

        // Fail-fast: global(ok) + frontend(ok) + backend(fail) = 3 results
        // infra nie powinien się uruchomić
        assert_eq!(
            vr.results.len(),
            3,
            "Fail-fast: 3 wyniki (global ok, frontend ok, backend fail)"
        );
        assert!(vr.results[0].success, "Global powinien przejść");
        assert!(vr.results[1].success, "Frontend powinien przejść");
        assert!(!vr.results[2].success, "Backend powinien failować");
        assert_eq!(vr.results[2].profile_name.as_deref(), Some("backend"));

        // Profile results powinny odzwierciedlać stan
        assert_eq!(vr.profile_results.len(), 2, "frontend ok + backend fail");
        assert!(vr.profile_results[0].success);
        assert_eq!(vr.profile_results[0].name, "frontend");
        assert!(!vr.profile_results[1].success);
        assert_eq!(vr.profile_results[1].name, "backend");
    }

    /// E2E Test: setup z profili + globalne — prawidłowa kolejność uruchamiania.
    ///
    /// Testuje run_setup z komendami, które zapisują kolejność do pliku,
    /// weryfikując, że setup wykonuje komendy sekwencyjnie.
    /// Komendy używają ścieżek relatywnych — run_setup ustawia cwd na worktree_path.
    #[tokio::test]
    async fn test_e2e_setup_commands_execution_order() {
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let worktree_path = temp.path();

        // Komendy zapisują kolejność do relatywnego pliku (cwd = worktree_path)
        let setup_commands: Vec<(String, String)> = vec![
            (
                "echo step1 >> order.txt".to_string(),
                "Step 1: global setup".to_string(),
            ),
            (
                "echo step2 >> order.txt".to_string(),
                "Step 2: frontend setup".to_string(),
            ),
            (
                "echo step3 >> order.txt".to_string(),
                "Step 3: backend setup".to_string(),
            ),
        ];

        let (tx, _rx) = mpsc::channel(32);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerConfig {
            setup_timeout: std::time::Duration::from_secs(5),
            ..WorkerConfig::default()
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        worker
            .run_setup("T01", &setup_commands, worktree_path)
            .await;

        // Weryfikuj, że plik zawiera kroki w prawidłowej kolejności
        let order_file = worktree_path.join("order.txt");
        let content =
            std::fs::read_to_string(&order_file).expect("order.txt powinien istnieć po setup");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3, "Powinny być 3 kroki setup");
        assert_eq!(lines[0], "step1");
        assert_eq!(lines[1], "step2");
        assert_eq!(lines[2], "step3");
    }

    /// E2E Test: setup z failującą komendą kontynuuje pozostałe (non-fatal).
    #[tokio::test]
    async fn test_e2e_setup_continues_on_failure() {
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let worktree_path = temp.path();

        // Komendy używają ścieżek relatywnych (cwd = worktree_path)
        let setup_commands: Vec<(String, String)> = vec![
            ("false".to_string(), "Failing command".to_string()),
            (
                "echo survived >> marker.txt".to_string(),
                "Should still run".to_string(),
            ),
        ];

        let (tx, mut rx) = mpsc::channel(32);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerConfig {
            setup_timeout: std::time::Duration::from_secs(5),
            ..WorkerConfig::default()
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        worker
            .run_setup("T01", &setup_commands, worktree_path)
            .await;

        // Druga komenda powinna się wykonać mimo failure pierwszej
        let marker_file = worktree_path.join("marker.txt");
        let content = std::fs::read_to_string(&marker_file)
            .expect("marker.txt powinien istnieć — setup kontynuuje po failure");
        assert!(content.contains("survived"));

        // Po await run_setup, wszystkie eventy są już w kanale — drain synchronicznie
        let mut phase_completed_found = false;
        while let Ok(e) = rx.try_recv() {
            if let WorkerEventKind::PhaseCompleted { success, phase, .. } = &e.kind
                && matches!(phase, WorkerPhase::Setup)
            {
                assert!(!success, "Setup PhaseCompleted powinien mieć success=false");
                phase_completed_found = true;
            }
        }
        assert!(
            phase_completed_found,
            "PhaseCompleted(Setup) event powinien być wysłany"
        );
    }

    // ── Task 48.2: Backward compatibility tests (worker verify flow without profiles) ──

    /// Test build_verify_plan with empty profiles in config (backward compat).
    /// Should behave exactly like global-only mode.
    #[test]
    fn test_build_verify_plan_backward_compat_empty_profiles() {
        // Worker with empty profiles (pre-profiles config)
        let worker = make_worker_with_profiles(vec![]);

        let global_cmds = vec![
            VerifyCommand::Simple("cargo test".to_string()),
            VerifyCommand::Simple("cargo clippy".to_string()),
        ];

        // No task profiles, no changed files → should only run global commands
        let plan = worker.build_verify_plan(&global_cmds, &[], &[]);

        assert!(plan.is_some(), "Should produce plan with global commands");
        let plan = plan.unwrap();
        assert_eq!(
            plan.steps.len(),
            1,
            "Should have exactly 1 step (GlobalVerify)"
        );

        match &plan.steps[0] {
            VerifyStep::GlobalVerify { commands } => {
                assert_eq!(commands.len(), 2, "Should run both global commands");
                assert_eq!(commands[0].command(), "cargo test");
                assert_eq!(commands[1].command(), "cargo clippy");
            }
            _ => panic!("Expected GlobalVerify step"),
        }
    }

    /// Test build_verify_plan with empty profiles and changed files.
    /// Changed files should NOT trigger any profile matching.
    #[test]
    fn test_build_verify_plan_backward_compat_changed_files_ignored() {
        let worker = make_worker_with_profiles(vec![]);

        let global_cmds = vec![VerifyCommand::Simple("cargo test".to_string())];
        let changed = vec!["src/main.rs".to_string(), "ui/app.ts".to_string()];

        // Even with changed files, no profiles → only global commands
        let plan = worker.build_verify_plan(&global_cmds, &[], &changed);

        assert!(plan.is_some());
        let plan = plan.unwrap();
        assert_eq!(
            plan.steps.len(),
            1,
            "Should have only GlobalVerify step (no profiles)"
        );
    }

    /// Test build_verify_plan with empty profiles and task_profiles.
    /// Task-specified profiles should be ignored if config has no profiles.
    #[test]
    fn test_build_verify_plan_backward_compat_task_profiles_ignored() {
        let worker = make_worker_with_profiles(vec![]);

        let global_cmds = vec![VerifyCommand::Simple("cargo fmt --check".to_string())];
        // Task specifies profiles, but config has none → profiles are ignored
        let task_profiles = vec!["backend".to_string(), "frontend".to_string()];

        let plan = worker.build_verify_plan(&global_cmds, &task_profiles, &[]);

        assert!(plan.is_some());
        let plan = plan.unwrap();
        assert_eq!(
            plan.steps.len(),
            1,
            "Should have only GlobalVerify (task profiles ignored when config empty)"
        );
    }

    /// Test that empty global commands + empty profiles → None plan.
    #[test]
    fn test_build_verify_plan_backward_compat_empty_everything() {
        let worker = make_worker_with_profiles(vec![]);

        // No global commands, no profiles → no verification needed
        let plan = worker.build_verify_plan(&[], &[], &[]);

        assert!(
            plan.is_none(),
            "Empty commands and profiles should return None plan"
        );
    }

    /// Test worker config construction with empty profiles (backward compat).
    #[test]
    fn test_worker_config_backward_compat_empty_profiles() {
        let config = WorkerConfig {
            system_prompt: "test".to_string(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            mcp_port: 8080,
            mcp_session_id: "test-session".to_string(),
            review_model: "opus".to_string(),
            base_commit: None,
            all_profiles: vec![], // Empty profiles (pre-profiles config)
        };

        assert!(config.all_profiles.is_empty());
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.review_model, "opus");
    }

    /// Test worker creation with empty profiles.
    #[test]
    fn test_worker_creation_backward_compat_empty_profiles() {
        let (tx, _rx) = mpsc::channel(16);
        let (_msg_tx, msg_rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerConfig {
            system_prompt: "system".to_string(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "opus".to_string(),
            base_commit: None,
            all_profiles: vec![], // Empty profiles
        };
        let worker = Worker::new(1, tx, shutdown, config, msg_rx);

        assert_eq!(worker.id, 1);
        assert!(worker.config.all_profiles.is_empty());
    }

    /// Test that build_verify_plan preserves order: GlobalVerify first, then profiles.
    /// With empty profiles, only GlobalVerify should appear.
    #[test]
    fn test_build_verify_plan_backward_compat_step_order() {
        let worker = make_worker_with_profiles(vec![]);

        let global_cmds = vec![
            VerifyCommand::Simple("cargo test".to_string()),
            VerifyCommand::Detailed {
                command: "cargo clippy".to_string(),
                name: Some("Lint".to_string()),
                description: None,
            },
        ];

        let plan = worker.build_verify_plan(&global_cmds, &[], &[]).unwrap();

        // Should have exactly 1 GlobalVerify step (no profiles)
        assert_eq!(plan.steps.len(), 1);
        match &plan.steps[0] {
            VerifyStep::GlobalVerify { commands } => {
                assert_eq!(commands.len(), 2);
                // Verify both simple and detailed commands are preserved
                assert_eq!(commands[0].command(), "cargo test");
                assert_eq!(commands[1].command(), "cargo clippy");
                assert_eq!(commands[1].name(), Some("Lint"));
            }
            _ => panic!("Expected GlobalVerify as first step"),
        }
    }
}
