#![allow(dead_code)]
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::mpsc;

use crate::commands::task::orchestrate::events::{EventLogger, WorkerEvent, WorkerEventKind};
use crate::commands::task::orchestrate::merge::{self, MergeResult};
use crate::commands::task::orchestrate::scheduler::TaskScheduler;
use crate::commands::task::orchestrate::state::{Lockfile, OrchestrateState};
use crate::commands::task::orchestrate::worker::{TaskResult, Worker};
use crate::commands::task::orchestrate::worktree::{WorktreeInfo, WorktreeManager};
use crate::shared::dag::TaskDag;
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::progress::{self, ProgressSummary, ProgressTask};

/// Per-worker tracking state within the orchestrator.
#[derive(Debug, Clone)]
enum WorkerSlot {
    Idle,
    Busy {
        task_id: String,
        worktree: WorktreeInfo,
    },
}

/// Configuration resolved from CLI args + file config for orchestration.
pub struct OrchestrateArgs {
    pub workers: u32,
    pub max_retries: u32,
    pub model: Option<String>,
    pub worktree_prefix: Option<String>,
    pub verbose: bool,
    pub resume: bool,
    pub dry_run: bool,
    pub no_merge: bool,
    pub max_cost: Option<f64>,
    pub timeout: Option<Duration>,
    pub task_filter: Option<Vec<String>>,
}

/// Main orchestrator — coordinates workers, scheduler, merges, and state.
pub struct Orchestrator {
    /// Resolved configuration
    args: OrchestrateArgs,
    /// Project root directory
    project_root: PathBuf,
    /// Path to PROGRESS.md
    progress_path: PathBuf,
    /// System prompt for workers
    system_prompt: String,
    /// Verification commands extracted from system prompt
    verification_commands: String,
}

impl Orchestrator {
    pub fn new(
        args: OrchestrateArgs,
        file_config: &FileConfig,
        project_root: PathBuf,
    ) -> Result<Self> {
        let progress_path = project_root.join(&file_config.task.progress_file);
        let system_prompt_path = project_root.join(&file_config.task.system_prompt_file);

        let system_prompt = if system_prompt_path.exists() {
            std::fs::read_to_string(&system_prompt_path)?
        } else {
            String::new()
        };

        // Extract verification commands from system prompt (look for ```bash blocks or similar)
        let verification_commands = extract_verification_commands(&system_prompt);

        Ok(Self {
            args,
            project_root,
            progress_path,
            system_prompt,
            verification_commands,
        })
    }

    /// Main entry point — run the full orchestration session.
    pub async fn execute(&self) -> Result<()> {
        // 1. Load and parse PROGRESS.md
        let progress_content = std::fs::read_to_string(&self.progress_path)
            .map_err(|e| RalphError::Orchestrate(format!("Failed to read PROGRESS.md: {e}")))?;
        let progress = progress::parse_progress(&progress_content);

        // 2. Build and validate DAG
        let frontmatter = progress
            .frontmatter
            .clone()
            .unwrap_or_default();
        let dag = TaskDag::from_frontmatter(&frontmatter);

        if let Some(cycle) = dag.detect_cycles() {
            return Err(RalphError::DagCycle(cycle));
        }

        // 3. Acquire lockfile
        let ralph_dir = self.project_root.join(".ralph");
        std::fs::create_dir_all(&ralph_dir)?;
        let lock_path = ralph_dir.join("orchestrate.lock");
        let lockfile = Lockfile::acquire(&lock_path)?;

        // 4. Initialize scheduler
        let mut scheduler =
            TaskScheduler::new(dag, &progress, self.args.max_retries);

        // 5. Initialize worktree manager
        let worktree_manager = WorktreeManager::new(
            self.project_root.clone(),
            self.args.worktree_prefix.clone(),
        );

        // 6. Initialize state
        let state_path = ralph_dir.join("orchestrate.yaml");
        let mut state = if self.args.resume && state_path.exists() {
            OrchestrateState::load(&state_path)?
        } else {
            OrchestrateState::new(
                self.args.workers,
                frontmatter.deps.clone(),
            )
        };

        // 7. Initialize event channel and logger
        let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>(256);
        let log_dir = ralph_dir.join("logs");
        let combined_log_path = ralph_dir.join("orchestrate.log");
        let event_logger = EventLogger::new(log_dir, Some(&combined_log_path))?;

        // 8. Initialize worker slots
        let worker_count = self.args.workers;
        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();
        for i in 1..=worker_count {
            worker_slots.insert(i, WorkerSlot::Idle);
        }

        // 9. Shutdown signaling
        let shutdown = Arc::new(AtomicBool::new(false));
        let graceful_shutdown = Arc::new(AtomicBool::new(false));

        // 10. Run the main orchestration loop
        self.run_loop(
            &mut scheduler,
            &worktree_manager,
            &mut state,
            &state_path,
            event_tx,
            event_rx,
            event_logger,
            &mut worker_slots,
            &progress,
            shutdown,
            graceful_shutdown,
            lockfile,
        )
        .await
    }

    /// The main orchestration loop.
    #[allow(clippy::too_many_arguments)]
    async fn run_loop(
        &self,
        scheduler: &mut TaskScheduler,
        worktree_manager: &WorktreeManager,
        state: &mut OrchestrateState,
        state_path: &std::path::Path,
        event_tx: mpsc::Sender<WorkerEvent>,
        mut event_rx: mpsc::Receiver<WorkerEvent>,
        mut event_logger: EventLogger,
        worker_slots: &mut HashMap<u32, WorkerSlot>,
        progress: &ProgressSummary,
        shutdown: Arc<AtomicBool>,
        graceful_shutdown: Arc<AtomicBool>,
        mut lockfile: Lockfile,
    ) -> Result<()> {
        let mut total_cost: f64 = 0.0;
        let mut worker_join_handles: HashMap<u32, tokio::task::JoinHandle<Result<TaskResult>>> =
            HashMap::new();
        let mut progress_mtime = get_mtime(&self.progress_path);
        let mut last_heartbeat = std::time::Instant::now();
        let mut last_hot_reload = std::time::Instant::now();
        let started_at = std::time::Instant::now();

        // Build task lookup for merge
        let task_lookup: HashMap<String, ProgressTask> = progress
            .tasks
            .iter()
            .map(|t| (t.id.clone(), t.clone()))
            .collect();

        eprintln!(
            "Orchestrator started: {} workers, {} tasks ({} done, {} remaining)",
            self.args.workers,
            progress.total(),
            progress.done,
            progress.remaining()
        );

        loop {
            // Check budget limits
            if let Some(max_cost) = self.args.max_cost
                && total_cost >= max_cost
            {
                eprintln!("Budget limit reached: ${total_cost:.4} >= ${max_cost:.4}");
                break;
            }
            if let Some(timeout) = self.args.timeout
                && started_at.elapsed() >= timeout
            {
                eprintln!("Timeout reached");
                break;
            }

            // Check completion
            if scheduler.is_complete() {
                let status = scheduler.status();
                eprintln!(
                    "All tasks complete: {} done, {} blocked",
                    status.done, status.blocked
                );
                break;
            }

            // Assign tasks to free workers (unless in graceful shutdown)
            if !graceful_shutdown.load(Ordering::Relaxed) {
                self.assign_tasks(
                    scheduler,
                    worktree_manager,
                    worker_slots,
                    &mut worker_join_handles,
                    &event_tx,
                    &shutdown,
                    progress,
                )
                .await?;
            }

            // Main select loop
            tokio::select! {
                // Process worker events
                Some(event) = event_rx.recv() => {
                    // Log the event
                    if let Some(worker_id) = extract_worker_id(&event.kind) {
                        event_logger.log_event(&event, worker_id).ok();
                    }

                    self.handle_event(
                        &event,
                        scheduler,
                        worktree_manager,
                        worker_slots,
                        &mut worker_join_handles,
                        state,
                        &task_lookup,
                        &mut total_cost,
                    ).await?;
                }

                // Ctrl+C signal handling
                _ = tokio::signal::ctrl_c() => {
                    if graceful_shutdown.load(Ordering::Relaxed) {
                        // Second Ctrl+C — force shutdown
                        eprintln!("\nForce shutdown — aborting all workers");
                        shutdown.store(true, Ordering::Relaxed);
                        // Abort all join handles
                        for (_, handle) in worker_join_handles.drain() {
                            handle.abort();
                        }
                        break;
                    } else {
                        // First Ctrl+C — graceful drain
                        eprintln!("\nGraceful shutdown — waiting for in-progress tasks to finish...");
                        eprintln!("Press Ctrl+C again to force quit");
                        graceful_shutdown.store(true, Ordering::Relaxed);

                        // If no workers are busy, exit immediately
                        let any_busy = worker_slots.values().any(|s| matches!(s, WorkerSlot::Busy { .. }));
                        if !any_busy {
                            break;
                        }
                    }
                }

                // Periodic timer: heartbeat, hot reload, state save
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    // Heartbeat every 5 seconds
                    if last_heartbeat.elapsed() >= Duration::from_secs(5) {
                        lockfile.heartbeat().ok();
                        last_heartbeat = std::time::Instant::now();
                    }

                    // Hot reload PROGRESS.md every 15 seconds
                    if last_hot_reload.elapsed() >= Duration::from_secs(15) {
                        if let Some(new_mtime) = get_mtime(&self.progress_path)
                            && progress_mtime.is_none_or(|old| new_mtime > old)
                        {
                            progress_mtime = Some(new_mtime);
                            // Reload DAG
                            if let Ok(new_content) = std::fs::read_to_string(&self.progress_path) {
                                let new_progress = progress::parse_progress(&new_content);
                                if let Some(fm) = &new_progress.frontmatter {
                                    let new_dag = TaskDag::from_frontmatter(fm);
                                    scheduler.add_tasks(new_dag);
                                }
                            }
                        }
                        last_hot_reload = std::time::Instant::now();
                    }

                    // In graceful shutdown, check if all workers are done
                    if graceful_shutdown.load(Ordering::Relaxed) {
                        let any_busy = worker_slots.values().any(|s| matches!(s, WorkerSlot::Busy { .. }));
                        if !any_busy {
                            eprintln!("All workers drained — exiting");
                            break;
                        }
                    }
                }
            }
        }

        // Save final state
        state.save(state_path).ok();

        // Release lockfile
        lockfile.release().ok();

        // Summary
        let elapsed = started_at.elapsed();
        eprintln!(
            "\nSession complete: ${total_cost:.4} total | {:.0}s elapsed",
            elapsed.as_secs_f64()
        );

        Ok(())
    }

    /// Assign ready tasks to idle workers.
    #[allow(clippy::too_many_arguments)]
    async fn assign_tasks(
        &self,
        scheduler: &mut TaskScheduler,
        worktree_manager: &WorktreeManager,
        worker_slots: &mut HashMap<u32, WorkerSlot>,
        join_handles: &mut HashMap<u32, tokio::task::JoinHandle<Result<TaskResult>>>,
        event_tx: &mpsc::Sender<WorkerEvent>,
        shutdown: &Arc<AtomicBool>,
        progress: &ProgressSummary,
    ) -> Result<()> {
        // Find idle workers
        let idle_workers: Vec<u32> = worker_slots
            .iter()
            .filter(|(_, slot)| matches!(slot, WorkerSlot::Idle))
            .map(|(id, _)| *id)
            .collect();

        for worker_id in idle_workers {
            let Some(task_id) = scheduler.next_ready_task() else {
                break; // No more ready tasks
            };

            // Find task info from progress
            let task_desc = progress
                .tasks
                .iter()
                .find(|t| t.id == task_id)
                .map(|t| format!("{} [{}] {}", t.id, t.component, t.name))
                .unwrap_or_else(|| task_id.clone());

            // Resolve model for this task
            let model = self.resolve_model(&task_id, progress);

            // Create worktree
            let worktree = worktree_manager
                .create_worktree(worker_id, &task_id)
                .await?;

            eprintln!(
                "[W{worker_id}] Assigned: {task_id} → {}",
                worktree.branch
            );

            // Mark task as started in scheduler
            scheduler.mark_started(&task_id);

            // Update worker slot
            worker_slots.insert(
                worker_id,
                WorkerSlot::Busy {
                    task_id: task_id.clone(),
                    worktree: worktree.clone(),
                },
            );

            // Spawn worker as tokio task
            let worker = Worker::new(
                worker_id,
                event_tx.clone(),
                shutdown.clone(),
                self.system_prompt.clone(),
                self.args.max_retries,
            );

            let worktree_path = worktree.path.clone();
            let verification_cmds = self.verification_commands.clone();
            let task_id_owned = task_id.clone();
            let task_desc_owned = task_desc.clone();
            let model_owned = model.clone();

            let handle = tokio::spawn(async move {
                worker
                    .execute_task(
                        &task_id_owned,
                        &task_desc_owned,
                        model_owned.as_deref(),
                        &worktree_path,
                        &verification_cmds,
                    )
                    .await
            });

            join_handles.insert(worker_id, handle);
        }

        Ok(())
    }

    /// Handle a worker event from the mpsc channel.
    #[allow(clippy::too_many_arguments)]
    async fn handle_event(
        &self,
        event: &WorkerEvent,
        scheduler: &mut TaskScheduler,
        worktree_manager: &WorktreeManager,
        worker_slots: &mut HashMap<u32, WorkerSlot>,
        join_handles: &mut HashMap<u32, tokio::task::JoinHandle<Result<TaskResult>>>,
        state: &mut OrchestrateState,
        task_lookup: &HashMap<String, ProgressTask>,
        total_cost: &mut f64,
    ) -> Result<()> {
        match &event.kind {
            WorkerEventKind::TaskCompleted {
                worker_id,
                task_id,
                success,
                cost_usd,
                ..
            } => {
                *total_cost += cost_usd;

                // Wait for the worker's join handle to complete
                if let Some(handle) = join_handles.remove(worker_id) {
                    let _ = handle.await;
                }

                if *success && !self.args.no_merge {
                    // Attempt squash merge
                    if let Some(WorkerSlot::Busy { worktree, .. }) =
                        worker_slots.get(worker_id)
                        && let Some(task) = task_lookup.get(task_id)
                    {
                        let merge_result =
                            merge::squash_merge(&self.project_root, worktree, task).await?;

                        match merge_result {
                            MergeResult::Success { commit_hash } => {
                                eprintln!(
                                    "[W{worker_id}] Merged: {task_id} → {commit_hash}"
                                );
                                scheduler.mark_done(task_id);

                                // Update state
                                state.tasks.insert(
                                    task_id.clone(),
                                    crate::commands::task::orchestrate::state::TaskState {
                                        status: "done".to_string(),
                                        worker: Some(*worker_id),
                                        retries: scheduler.retry_count(task_id),
                                        cost: *cost_usd,
                                    },
                                );

                                // Cleanup worktree
                                worktree_manager
                                    .remove_worktree(&worktree.path)
                                    .await
                                    .ok();
                                worktree_manager
                                    .remove_branch(&worktree.branch)
                                    .await
                                    .ok();
                            }
                            MergeResult::Conflict { files } => {
                                eprintln!(
                                    "[W{worker_id}] Merge conflict in {task_id}: {files:?}"
                                );
                                // Abort merge and block the task for now
                                merge::abort_merge(&self.project_root).await.ok();
                                scheduler.mark_blocked(task_id);

                                state.tasks.insert(
                                    task_id.clone(),
                                    crate::commands::task::orchestrate::state::TaskState {
                                        status: "blocked".to_string(),
                                        worker: Some(*worker_id),
                                        retries: scheduler.retry_count(task_id),
                                        cost: *cost_usd,
                                    },
                                );
                            }
                            MergeResult::Failed { error } => {
                                eprintln!(
                                    "[W{worker_id}] Merge failed for {task_id}: {error}"
                                );
                                scheduler.mark_blocked(task_id);
                            }
                        }
                    }
                } else if *success {
                    // --no-merge mode: mark done without merging
                    scheduler.mark_done(task_id);
                    eprintln!("[W{worker_id}] Done (no merge): {task_id}");
                } else {
                    // Task failed
                    let requeued = scheduler.mark_failed(task_id);
                    if requeued {
                        eprintln!(
                            "[W{worker_id}] Task {task_id} failed, re-queued for retry"
                        );
                    } else {
                        eprintln!("[W{worker_id}] Task {task_id} blocked after max retries");
                    }
                }

                // Free the worker slot
                worker_slots.insert(*worker_id, WorkerSlot::Idle);
            }

            WorkerEventKind::TaskFailed {
                worker_id,
                task_id,
                error,
                retries_left,
            } => {
                if *retries_left == 0 {
                    eprintln!("[W{worker_id}] Task {task_id} failed permanently: {error}");
                } else {
                    eprintln!(
                        "[W{worker_id}] Task {task_id} failed ({retries_left} retries left): {error}"
                    );
                }
            }

            WorkerEventKind::CostUpdate {
                worker_id: _,
                cost_usd,
                ..
            } => {
                *total_cost += cost_usd;
            }

            WorkerEventKind::PhaseStarted {
                worker_id,
                task_id,
                phase,
            } => {
                eprintln!("[W{worker_id}] {task_id} → phase: {phase}");
            }

            WorkerEventKind::PhaseCompleted {
                worker_id,
                task_id,
                phase,
                success,
            } => {
                let status = if *success { "ok" } else { "FAILED" };
                eprintln!("[W{worker_id}] {task_id} ← phase: {phase} [{status}]");
            }

            // Other events: log only (MergeStarted, MergeCompleted, MergeConflict, TaskStarted)
            _ => {}
        }

        Ok(())
    }

    /// Resolve the model to use for a specific task.
    fn resolve_model(&self, task_id: &str, progress: &ProgressSummary) -> Option<String> {
        // Priority: per-task model from frontmatter > CLI --model > config default
        if let Some(fm) = &progress.frontmatter {
            if let Some(model) = fm.models.get(task_id) {
                return Some(model.clone());
            }
            if let Some(model) = &fm.default_model {
                return Some(model.clone());
            }
        }
        self.args.model.clone()
    }
}

/// Extract verification commands from the system prompt.
/// Looks for ```bash or ```sh fenced code blocks, or lines starting with `$`.
fn extract_verification_commands(system_prompt: &str) -> String {
    let mut commands = Vec::new();
    let mut in_code_block = false;
    let mut is_bash_block = false;

    for line in system_prompt.lines() {
        if line.starts_with("```bash") || line.starts_with("```sh") {
            in_code_block = true;
            is_bash_block = true;
            continue;
        }
        if line.starts_with("```") && in_code_block {
            in_code_block = false;
            is_bash_block = false;
            continue;
        }
        if in_code_block && is_bash_block {
            commands.push(line.to_string());
        }
    }

    if commands.is_empty() {
        // Fallback: look for common verification commands
        "cargo check && cargo test && cargo clippy --all-targets -- -D warnings".to_string()
    } else {
        commands.join("\n")
    }
}

/// Get file modification time.
fn get_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Extract worker_id from a WorkerEventKind.
fn extract_worker_id(kind: &WorkerEventKind) -> Option<u32> {
    match kind {
        WorkerEventKind::TaskStarted { worker_id, .. }
        | WorkerEventKind::PhaseStarted { worker_id, .. }
        | WorkerEventKind::PhaseCompleted { worker_id, .. }
        | WorkerEventKind::TaskCompleted { worker_id, .. }
        | WorkerEventKind::TaskFailed { worker_id, .. }
        | WorkerEventKind::CostUpdate { worker_id, .. }
        | WorkerEventKind::MergeStarted { worker_id, .. }
        | WorkerEventKind::MergeCompleted { worker_id, .. }
        | WorkerEventKind::MergeConflict { worker_id, .. } => Some(*worker_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_verification_commands_bash_block() {
        let prompt = "# Build\n\n```bash\ncargo build\ncargo test\n```\n\nSome text.";
        let cmds = extract_verification_commands(prompt);
        assert!(cmds.contains("cargo build"));
        assert!(cmds.contains("cargo test"));
    }

    #[test]
    fn test_extract_verification_commands_no_block() {
        let prompt = "No code blocks here.";
        let cmds = extract_verification_commands(prompt);
        assert!(cmds.contains("cargo check"));
    }

    #[test]
    fn test_extract_verification_commands_sh_block() {
        let prompt = "```sh\nmake test\n```";
        let cmds = extract_verification_commands(prompt);
        assert!(cmds.contains("make test"));
    }

    #[test]
    fn test_extract_worker_id() {
        let kind = WorkerEventKind::TaskStarted {
            worker_id: 42,
            task_id: "T01".to_string(),
        };
        assert_eq!(extract_worker_id(&kind), Some(42));
    }

    #[test]
    fn test_extract_worker_id_all_variants() {
        let kinds = [
            WorkerEventKind::CostUpdate {
                worker_id: 1,
                cost_usd: 0.0,
                input_tokens: 0,
                output_tokens: 0,
            },
            WorkerEventKind::MergeStarted {
                worker_id: 2,
                task_id: "T01".to_string(),
            },
            WorkerEventKind::MergeConflict {
                worker_id: 3,
                task_id: "T01".to_string(),
                conflicting_files: vec![],
            },
        ];
        for (i, kind) in kinds.iter().enumerate() {
            assert_eq!(extract_worker_id(kind), Some((i + 1) as u32));
        }
    }

    #[test]
    fn test_get_mtime_nonexistent() {
        assert!(get_mtime(std::path::Path::new("/nonexistent/file")).is_none());
    }

    #[test]
    fn test_resolve_model_per_task() {
        use crate::shared::progress::{ProgressFrontmatter, ProgressTask, TaskStatus};

        let fm = ProgressFrontmatter {
            deps: HashMap::new(),
            models: HashMap::from([("T01".to_string(), "claude-opus-4-6".to_string())]),
            default_model: Some("claude-sonnet-4-5-20250929".to_string()),
        };
        let progress = ProgressSummary {
            tasks: vec![ProgressTask {
                id: "T01".to_string(),
                component: "api".to_string(),
                name: "Test".to_string(),
                status: TaskStatus::Todo,
            }],
            done: 0,
            in_progress: 0,
            blocked: 0,
            todo: 1,
            frontmatter: Some(fm),
        };

        let args = OrchestrateArgs {
            workers: 2,
            max_retries: 3,
            model: Some("fallback-model".to_string()),
            worktree_prefix: None,
            verbose: false,
            resume: false,
            dry_run: false,
            no_merge: false,
            max_cost: None,
            timeout: None,
            task_filter: None,
        };

        let orch = Orchestrator {
            args,
            project_root: PathBuf::from("/tmp/test"),
            progress_path: PathBuf::from("/tmp/test/PROGRESS.md"),
            system_prompt: String::new(),
            verification_commands: String::new(),
        };

        // Per-task model takes priority
        assert_eq!(
            orch.resolve_model("T01", &progress),
            Some("claude-opus-4-6".to_string())
        );
        // Frontmatter default for unknown task
        assert_eq!(
            orch.resolve_model("T99", &progress),
            Some("claude-sonnet-4-5-20250929".to_string())
        );
    }

    #[test]
    fn test_resolve_model_cli_fallback() {
        use crate::shared::progress::{ProgressTask, TaskStatus};

        let progress = ProgressSummary {
            tasks: vec![ProgressTask {
                id: "T01".to_string(),
                component: "api".to_string(),
                name: "Test".to_string(),
                status: TaskStatus::Todo,
            }],
            done: 0,
            in_progress: 0,
            blocked: 0,
            todo: 1,
            frontmatter: None,
        };

        let args = OrchestrateArgs {
            workers: 2,
            max_retries: 3,
            model: Some("cli-model".to_string()),
            worktree_prefix: None,
            verbose: false,
            resume: false,
            dry_run: false,
            no_merge: false,
            max_cost: None,
            timeout: None,
            task_filter: None,
        };

        let orch = Orchestrator {
            args,
            project_root: PathBuf::from("/tmp/test"),
            progress_path: PathBuf::from("/tmp/test/PROGRESS.md"),
            system_prompt: String::new(),
            verification_commands: String::new(),
        };

        assert_eq!(
            orch.resolve_model("T01", &progress),
            Some("cli-model".to_string())
        );
    }

    #[test]
    fn test_worker_slot_states() {
        let idle = WorkerSlot::Idle;
        assert!(matches!(idle, WorkerSlot::Idle));

        let busy = WorkerSlot::Busy {
            task_id: "T01".to_string(),
            worktree: WorktreeInfo {
                path: PathBuf::from("/tmp/wt1"),
                branch: "ralph/w1/T01".to_string(),
                worker_id: 1,
                task_id: "T01".to_string(),
            },
        };
        assert!(matches!(busy, WorkerSlot::Busy { .. }));
    }
}
