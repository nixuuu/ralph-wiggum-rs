use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::mpsc;

use crate::commands::task::orchestrate::dashboard::Dashboard;
use crate::commands::task::orchestrate::events::{EventLogger, WorkerEvent, WorkerEventKind, WorkerPhase};
use crate::commands::task::orchestrate::merge::{self, MergeResult};
use crate::commands::task::orchestrate::output::MultiplexedOutput;
use crate::commands::task::orchestrate::scheduler::TaskScheduler;
use crate::commands::task::orchestrate::state::{Lockfile, OrchestrateState};
use crate::commands::task::orchestrate::status::{
    DashboardInputThread, OrchestratorStatus, ShutdownState, WorkerState, WorkerStatus,
};
use crate::commands::task::orchestrate::summary::{self, TaskSummaryEntry};
use crate::commands::task::orchestrate::worker::{TaskResult, Worker};
use crate::commands::task::orchestrate::worktree::{WorktreeInfo, WorktreeManager};
use crate::shared::dag::TaskDag;
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::progress::{self, ProgressSummary, ProgressTask};

// ── Worker slot tracking ────────────────────────────────────────────

/// Per-worker tracking state within the orchestrator.
#[derive(Debug, Clone)]
enum WorkerSlot {
    Idle,
    Busy {
        #[allow(dead_code)]
        task_id: String,
        worktree: WorktreeInfo,
    },
}

// ── TUI context ─────────────────────────────────────────────────────

/// Groups all TUI-related mutable state to keep function signatures clean.
struct TuiContext {
    dashboard: Dashboard,
    mux_output: MultiplexedOutput,
    task_start_times: HashMap<String, Instant>,
    task_summaries: Vec<TaskSummaryEntry>,
}

// ── Resolved config ─────────────────────────────────────────────────

/// Configuration resolved from CLI args + file config for orchestration.
pub struct ResolvedConfig {
    pub workers: u32,
    pub max_retries: u32,
    pub model: Option<String>,
    pub worktree_prefix: Option<String>,
    #[allow(dead_code)]
    pub verbose: bool,
    pub resume: bool,
    #[allow(dead_code)]
    pub dry_run: bool,
    pub no_merge: bool,
    pub max_cost: Option<f64>,
    pub timeout: Option<Duration>,
    #[allow(dead_code)]
    pub task_filter: Option<Vec<String>>,
}

impl ResolvedConfig {
    /// Build resolved config from CLI args + file config.
    ///
    /// Priority: CLI flags > .ralph.toml orchestrate section > hardcoded defaults.
    pub fn from_args(
        cli: &crate::commands::task::args::OrchestrateArgs,
        file_config: &FileConfig,
    ) -> Self {
        let orch_cfg = &file_config.task.orchestrate;

        let workers = cli.workers.unwrap_or(orch_cfg.workers);
        let max_retries = cli.max_retries.unwrap_or(orch_cfg.max_retries);
        let model = cli.model.clone().or_else(|| orch_cfg.default_model.clone());
        let worktree_prefix = cli
            .worktree_prefix
            .clone()
            .or_else(|| orch_cfg.worktree_prefix.clone());
        let timeout = cli.timeout.as_deref().and_then(parse_duration);
        let task_filter = cli
            .tasks
            .as_ref()
            .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());

        Self {
            workers,
            max_retries,
            model,
            worktree_prefix,
            verbose: cli.verbose,
            resume: cli.resume,
            dry_run: cli.dry_run,
            no_merge: cli.no_merge,
            max_cost: cli.max_cost,
            timeout,
            task_filter,
        }
    }
}

/// Parse a human-readable duration string like "2h", "30m", "45s", "1h30m".
fn parse_duration(s: &str) -> Option<Duration> {
    let mut total_secs: u64 = 0;
    let mut current_num = String::new();

    for ch in s.chars() {
        if ch.is_ascii_digit() {
            current_num.push(ch);
        } else {
            let n: u64 = current_num.parse().ok()?;
            current_num.clear();
            match ch {
                'h' => total_secs += n * 3600,
                'm' => total_secs += n * 60,
                's' => total_secs += n,
                _ => return None,
            }
        }
    }

    if total_secs > 0 {
        Some(Duration::from_secs(total_secs))
    } else {
        None
    }
}

// ── Orchestrator ────────────────────────────────────────────────────

/// Main orchestrator — coordinates workers, scheduler, merges, and state.
pub struct Orchestrator {
    config: ResolvedConfig,
    project_root: PathBuf,
    progress_path: PathBuf,
    system_prompt: String,
    verification_commands: Option<String>,
    use_nerd_font: bool,
}

impl Orchestrator {
    pub fn new(
        config: ResolvedConfig,
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

        let verification_commands = file_config
            .task
            .orchestrate
            .verify_commands
            .clone()
            .filter(|s| !s.trim().is_empty());

        Ok(Self {
            config,
            project_root,
            progress_path,
            system_prompt,
            verification_commands,
            use_nerd_font: file_config.ui.nerd_font,
        })
    }

    /// Main entry point — run the full orchestration session.
    pub async fn execute(&self) -> Result<()> {
        // 1. Load and parse PROGRESS.md
        let progress_content = std::fs::read_to_string(&self.progress_path)
            .map_err(|e| RalphError::Orchestrate(format!("Failed to read PROGRESS.md: {e}")))?;
        let progress = progress::parse_progress(&progress_content);

        // 2. Build and validate DAG
        let frontmatter = progress.frontmatter.clone().unwrap_or_default();
        let dag = TaskDag::from_frontmatter(&frontmatter);

        if let Some(cycle) = dag.detect_cycles() {
            return Err(RalphError::DagCycle(cycle));
        }

        // 2b. Dry-run mode — show DAG visualization and exit without side effects
        if self.config.dry_run {
            use crate::commands::task::orchestrate::dry_run;

            let viz = dry_run::visualize_dag(&dag, &progress);
            let dag_output = dry_run::format_dag(&viz, self.config.workers);
            println!("{dag_output}");
            println!();
            let dep_list = dry_run::format_dep_list(&dag, &progress);
            println!("Task dependency list:\n{dep_list}");
            return Ok(());
        }

        // 3. Acquire lockfile
        let ralph_dir = self.project_root.join(".ralph");
        std::fs::create_dir_all(&ralph_dir)?;
        let lock_path = ralph_dir.join("orchestrate.lock");
        let lockfile = Lockfile::acquire(&lock_path)?;

        // 4. Initialize scheduler
        let mut scheduler = TaskScheduler::new(dag, &progress, self.config.max_retries);

        // 5. Initialize worktree manager
        let worktree_manager = WorktreeManager::new(
            self.project_root.clone(),
            self.config.worktree_prefix.clone(),
        );

        // 6. Initialize state
        let state_path = ralph_dir.join("orchestrate.yaml");
        let mut state = if self.config.resume && state_path.exists() {
            OrchestrateState::load(&state_path)?
        } else {
            OrchestrateState::new(self.config.workers, frontmatter.deps.clone())
        };

        // 7. Initialize event channel and logger
        let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>(1024);
        let log_dir = ralph_dir.join("logs");
        let combined_log_path = ralph_dir.join("orchestrate.log");
        let event_logger = EventLogger::new(log_dir, Some(&combined_log_path))?;

        // 8. Initialize worker slots
        let worker_count = self.config.workers;
        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();
        for i in 1..=worker_count {
            worker_slots.insert(i, WorkerSlot::Idle);
        }

        // 9. Shutdown signaling
        let shutdown = Arc::new(AtomicBool::new(false));
        let graceful_shutdown = Arc::new(AtomicBool::new(false));
        let resize_flag = Arc::new(AtomicBool::new(false));
        let focused_worker = Arc::new(AtomicU32::new(0));
        let scroll_delta = Arc::new(AtomicI32::new(0));
        let render_notify = Arc::new(tokio::sync::Notify::new());

        // 10. Initialize TUI (fullscreen dashboard)
        let output_log_path = ralph_dir.join("orchestrate-output.log");
        let mut tui = TuiContext {
            dashboard: Dashboard::new(worker_count)?,
            mux_output: MultiplexedOutput::new(Some(&output_log_path)),
            task_start_times: HashMap::new(),
            task_summaries: Vec::new(),
        };

        // 11. Spawn input thread (dedicated OS thread for crossterm)
        let input_thread = DashboardInputThread::spawn(
            shutdown.clone(),
            graceful_shutdown.clone(),
            resize_flag.clone(),
            focused_worker.clone(),
            worker_count,
            scroll_delta.clone(),
            render_notify.clone(),
        );

        // 12. Run the main orchestration loop
        let result = self
            .run_loop(
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
                resize_flag,
                focused_worker,
                scroll_delta,
                render_notify,
                lockfile,
                &mut tui,
            )
            .await;

        // 13. Cleanup TUI
        tui.dashboard.cleanup()?;
        input_thread.stop();

        result
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
        resize_flag: Arc<AtomicBool>,
        focused_worker: Arc<AtomicU32>,
        scroll_delta: Arc<AtomicI32>,
        render_notify: Arc<tokio::sync::Notify>,
        mut lockfile: Lockfile,
        tui: &mut TuiContext,
    ) -> Result<()> {
        let mut worker_join_handles: HashMap<u32, tokio::task::JoinHandle<Result<TaskResult>>> =
            HashMap::new();
        let mut progress_mtime = get_mtime(&self.progress_path);
        let mut last_heartbeat = Instant::now();
        let mut last_hot_reload = Instant::now();
        let started_at = Instant::now();

        // SIGTERM handler — treat `kill <pid>` as force shutdown
        #[cfg(unix)]
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        ).ok();

        // Build task lookup for merge
        let task_lookup: HashMap<String, ProgressTask> = progress
            .tasks
            .iter()
            .map(|t| (t.id.clone(), t.clone()))
            .collect();

        // Print startup message
        let msg = MultiplexedOutput::format_orchestrator_line(&format!(
            "Started: {} workers, {} tasks ({} done, {} remaining)",
            self.config.workers,
            progress.total(),
            progress.done,
            progress.remaining()
        ));
        tui.dashboard.push_log_line(&msg);

        // Warn if no verification commands configured
        if self.verification_commands.is_none() {
            let warn = MultiplexedOutput::format_orchestrator_line(
                "⚠ No verify_commands configured — skipping verify phase. \
                 Set verify_commands in [task.orchestrate] in .ralph.toml",
            );
            tui.dashboard.push_log_line(&warn);
        }

        // Initial dashboard render
        {
            let orch_status = OrchestratorStatus {
                scheduler: scheduler.status(),
                workers: Vec::new(),
                total_cost: tui.mux_output.total_cost(),
                elapsed: started_at.elapsed(),
                shutdown_state: ShutdownState::Running,
            };
            tui.dashboard.render(&orch_status)?;
        }

        loop {
            // Check budget limits
            if let Some(max_cost) = self.config.max_cost
                && tui.mux_output.total_cost() >= max_cost
            {
                let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                    "Budget limit reached: ${:.4} >= ${max_cost:.4}",
                    tui.mux_output.total_cost()
                ));
                tui.dashboard.push_log_line(&msg);
                break;
            }
            if let Some(timeout) = self.config.timeout
                && started_at.elapsed() >= timeout
            {
                let msg =
                    MultiplexedOutput::format_orchestrator_line("Timeout reached");
                tui.dashboard.push_log_line(&msg);
                break;
            }

            // Check completion
            if scheduler.is_complete() {
                let status = scheduler.status();
                let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                    "All tasks complete: {} done, {} blocked",
                    status.done, status.blocked
                ));
                tui.dashboard.push_log_line(&msg);
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
                    tui,
                )
                .await?;
            }

            // Main select loop
            tokio::select! {
                // Process worker events
                Some(event) = event_rx.recv() => {
                    // Log structural events (skip verbose OutputLines)
                    if !matches!(event.kind, WorkerEventKind::OutputLines { .. })
                        && let Some(worker_id) = extract_worker_id(&event.kind)
                    {
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
                        tui,
                    ).await?;

                    // Handle resize after event
                    if resize_flag.swap(false, Ordering::SeqCst) {
                        tui.dashboard.handle_resize()?;
                    }
                    self.render_dashboard(tui, scheduler, started_at, &focused_worker, &scroll_delta, &graceful_shutdown, &shutdown)?;
                }

                // Ctrl+C signal handling (backup — input thread handles q/Ctrl+C too)
                _ = tokio::signal::ctrl_c() => {
                    if graceful_shutdown.load(Ordering::Relaxed) {
                        // Second Ctrl+C — force shutdown
                        let msg = MultiplexedOutput::format_orchestrator_line(
                            "Force shutdown — aborting all workers"
                        );
                        tui.dashboard.push_log_line(&msg);
                        shutdown.store(true, Ordering::Relaxed);
                        for (_, handle) in worker_join_handles.drain() {
                            handle.abort();
                        }
                        break;
                    } else {
                        // First Ctrl+C — graceful drain
                        let msg = MultiplexedOutput::format_orchestrator_line(
                            "Graceful shutdown — waiting for in-progress tasks..."
                        );
                        tui.dashboard.push_log_line(&msg);
                        graceful_shutdown.store(true, Ordering::Relaxed);

                        let any_busy = worker_slots.values().any(|s| matches!(s, WorkerSlot::Busy { .. }));
                        if !any_busy {
                            break;
                        }
                    }
                }

                // SIGTERM — immediate force shutdown (from `kill <pid>`)
                _ = async {
                    #[cfg(unix)]
                    if let Some(ref mut s) = sigterm { s.recv().await; return; }
                    // On non-unix, never resolves
                    std::future::pending::<()>().await;
                } => {
                    let msg = MultiplexedOutput::format_orchestrator_line(
                        "SIGTERM received — force shutdown"
                    );
                    tui.dashboard.push_log_line(&msg);
                    shutdown.store(true, Ordering::Relaxed);
                    for (_, handle) in worker_join_handles.drain() {
                        handle.abort();
                    }
                    break;
                }

                // Immediate re-render on keypress (input thread notifies)
                _ = render_notify.notified() => {
                    if resize_flag.swap(false, Ordering::SeqCst) {
                        tui.dashboard.handle_resize()?;
                    }
                    self.render_dashboard(tui, scheduler, started_at, &focused_worker, &scroll_delta, &graceful_shutdown, &shutdown)?;
                }

                // Periodic timer: heartbeat, hot reload, status refresh
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    // Check if input thread triggered shutdown
                    if shutdown.load(Ordering::SeqCst) {
                        let msg = MultiplexedOutput::format_orchestrator_line(
                            "Force shutdown — aborting all workers"
                        );
                        tui.dashboard.push_log_line(&msg);
                        for (_, handle) in worker_join_handles.drain() {
                            handle.abort();
                        }
                        break;
                    }
                    if graceful_shutdown.load(Ordering::SeqCst)
                        && !worker_slots.values().any(|s| matches!(s, WorkerSlot::Busy { .. }))
                    {
                        let msg = MultiplexedOutput::format_orchestrator_line(
                            "All workers drained — exiting"
                        );
                        tui.dashboard.push_log_line(&msg);
                        break;
                    }

                    // Heartbeat every 5 seconds
                    if last_heartbeat.elapsed() >= Duration::from_secs(5) {
                        lockfile.heartbeat().ok();
                        last_heartbeat = Instant::now();
                    }

                    // Hot reload PROGRESS.md every 15 seconds
                    if last_hot_reload.elapsed() >= Duration::from_secs(15) {
                        if let Some(new_mtime) = get_mtime(&self.progress_path)
                            && progress_mtime.is_none_or(|old| new_mtime > old)
                        {
                            progress_mtime = Some(new_mtime);
                            if let Ok(new_content) = std::fs::read_to_string(&self.progress_path) {
                                let new_progress = progress::parse_progress(&new_content);
                                if let Some(fm) = &new_progress.frontmatter {
                                    let new_dag = TaskDag::from_frontmatter(fm);
                                    scheduler.add_tasks(new_dag);
                                }
                            }
                        }
                        last_hot_reload = Instant::now();
                    }

                    // Handle resize + render dashboard
                    if resize_flag.swap(false, Ordering::SeqCst) {
                        tui.dashboard.handle_resize()?;
                    }
                    self.render_dashboard(tui, scheduler, started_at, &focused_worker, &scroll_delta, &graceful_shutdown, &shutdown)?;
                }
            }
        }

        // Save final state
        state.save(state_path).ok();

        // Release lockfile
        lockfile.release().ok();

        // Print summary
        let elapsed = started_at.elapsed();
        let summary_text = summary::format_summary(&tui.task_summaries, elapsed);
        println!("\n{summary_text}");

        Ok(())
    }

    /// Apply focus/scroll from input thread and render the dashboard.
    #[allow(clippy::too_many_arguments)]
    fn render_dashboard(
        &self,
        tui: &mut TuiContext,
        scheduler: &TaskScheduler,
        started_at: Instant,
        focused_worker: &Arc<AtomicU32>,
        scroll_delta: &Arc<AtomicI32>,
        graceful_shutdown: &Arc<AtomicBool>,
        shutdown: &Arc<AtomicBool>,
    ) -> Result<()> {
        // Apply focus from input thread
        let focus = focused_worker.load(Ordering::Relaxed);
        tui.dashboard
            .set_focus(if focus == 0 { None } else { Some(focus) });

        // Apply scroll delta from input thread
        let delta = scroll_delta.swap(0, Ordering::Relaxed);
        if delta != 0 {
            tui.dashboard.apply_scroll(delta);
        }

        // Determine shutdown state
        let shutdown_state = if shutdown.load(Ordering::Relaxed) {
            ShutdownState::Aborting
        } else if graceful_shutdown.load(Ordering::Relaxed) {
            ShutdownState::Draining
        } else {
            ShutdownState::Running
        };

        // Build status snapshot and render
        let orch_status = OrchestratorStatus {
            scheduler: scheduler.status(),
            workers: Vec::new(),
            total_cost: tui.mux_output.total_cost(),
            elapsed: started_at.elapsed(),
            shutdown_state,
        };
        tui.dashboard.render(&orch_status)
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
        tui: &mut TuiContext,
    ) -> Result<()> {
        // Find idle workers
        let idle_workers: Vec<u32> = worker_slots
            .iter()
            .filter(|(_, slot)| matches!(slot, WorkerSlot::Idle))
            .map(|(id, _)| *id)
            .collect();

        for worker_id in idle_workers {
            let Some(task_id) = scheduler.next_ready_task() else {
                break;
            };

            // Find task info from progress
            let task_info = progress.tasks.iter().find(|t| t.id == task_id);
            let task_desc = task_info
                .map(|t| format!("{} [{}] {}", t.id, t.component, t.name))
                .unwrap_or_else(|| task_id.clone());

            // Resolve model for this task
            let model = self.resolve_model(&task_id, progress);

            // Create worktree
            let worktree = worktree_manager
                .create_worktree(worker_id, &task_id)
                .await?;

            // Print assignment via TUI
            let msg = tui.mux_output.format_worker_line(
                worker_id,
                &format!("Assigned: {task_id} → {}", worktree.branch),
            );
            tui.dashboard.push_log_line(&msg);

            // Mark task as started in scheduler
            scheduler.mark_started(&task_id);

            // Update TUI worker status
            tui.mux_output.assign_worker(worker_id, &task_id);
            let ws = WorkerStatus {
                worker_id,
                state: WorkerState::Implementing,
                task_id: Some(task_id.clone()),
                component: task_info.map(|t| t.component.clone()),
                phase: Some(WorkerPhase::Implement),
                cost_usd: 0.0,
                input_tokens: 0,
                output_tokens: 0,
            };
            tui.dashboard.update_worker_status(worker_id, ws);
            tui.task_start_times
                .insert(task_id.clone(), Instant::now());

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
                self.config.max_retries,
                self.use_nerd_font,
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
                        verification_cmds.as_deref(),
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
        tui: &mut TuiContext,
    ) -> Result<()> {
        match &event.kind {
            WorkerEventKind::TaskStarted {
                worker_id,
                task_id,
            } => {
                let ws = WorkerStatus {
                    worker_id: *worker_id,
                    state: WorkerState::Implementing,
                    task_id: Some(task_id.clone()),
                    component: task_lookup.get(task_id).map(|t| t.component.clone()),
                    phase: Some(WorkerPhase::Implement),
                    cost_usd: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                };
                tui.dashboard.update_worker_status(*worker_id, ws);
                let msg = tui
                    .mux_output
                    .format_worker_line(*worker_id, &format!("Started: {task_id}"));
                tui.dashboard.push_log_line(&msg);
            }

            WorkerEventKind::PhaseStarted {
                worker_id,
                task_id,
                phase,
            } => {
                let new_state = match phase {
                    WorkerPhase::Implement => WorkerState::Implementing,
                    WorkerPhase::ReviewFix => WorkerState::Reviewing,
                    WorkerPhase::Verify => WorkerState::Verifying,
                };
                // Update just the phase/state fields via a fresh status
                let (cost, input, output) = tui.mux_output.worker_cost(*worker_id);
                let ws = WorkerStatus {
                    worker_id: *worker_id,
                    state: new_state,
                    task_id: Some(task_id.clone()),
                    component: task_lookup.get(task_id).map(|t| t.component.clone()),
                    phase: Some(phase.clone()),
                    cost_usd: cost,
                    input_tokens: input,
                    output_tokens: output,
                };
                tui.dashboard.update_worker_status(*worker_id, ws);
                let msg = tui
                    .mux_output
                    .format_worker_line(*worker_id, &format!("{task_id} → phase: {phase}"));
                tui.dashboard.push_log_line(&msg);
            }

            WorkerEventKind::PhaseCompleted {
                worker_id,
                task_id,
                phase,
                success,
            } => {
                let status_text = if *success { "ok" } else { "FAILED" };
                let msg = tui.mux_output.format_worker_line(
                    *worker_id,
                    &format!("{task_id} ← phase: {phase} [{status_text}]"),
                );
                tui.dashboard.push_log_line(&msg);
            }

            WorkerEventKind::CostUpdate {
                worker_id,
                cost_usd,
                input_tokens,
                output_tokens,
            } => {
                tui.mux_output
                    .update_cost(*worker_id, *cost_usd, *input_tokens, *output_tokens);
                let (total_cost, total_in, total_out) =
                    tui.mux_output.worker_cost(*worker_id);
                tui.dashboard
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
                if let Some(handle) = join_handles.remove(worker_id) {
                    let _ = handle.await;
                }

                // Record summary entry
                let duration = tui
                    .task_start_times
                    .remove(task_id)
                    .map(|s| s.elapsed())
                    .unwrap_or_default();

                if *success && !self.config.no_merge {
                    // Attempt squash merge
                    if let Some(WorkerSlot::Busy { worktree, .. }) = worker_slots.get(worker_id)
                        && let Some(task) = task_lookup.get(task_id)
                    {
                        // Show merging state
                        let (cost, input, output) = tui.mux_output.worker_cost(*worker_id);
                        let ws = WorkerStatus {
                            worker_id: *worker_id,
                            state: WorkerState::Merging,
                            task_id: Some(task_id.clone()),
                            component: Some(task.component.clone()),
                            phase: None,
                            cost_usd: cost,
                            input_tokens: input,
                            output_tokens: output,
                        };
                        tui.dashboard.update_worker_status(*worker_id, ws);

                        let merge_result =
                            merge::squash_merge(&self.project_root, worktree, task).await?;

                        match merge_result {
                            MergeResult::Success { commit_hash } => {
                                let msg = tui.mux_output.format_worker_line(
                                    *worker_id,
                                    &format!("Merged: {task_id} → {commit_hash}"),
                                );
                                tui.dashboard.push_log_line(&msg);
                                scheduler.mark_done(task_id);

                                state.tasks.insert(
                                    task_id.clone(),
                                    crate::commands::task::orchestrate::state::TaskState {
                                        status: "done".to_string(),
                                        worker: Some(*worker_id),
                                        retries: scheduler.retry_count(task_id),
                                        cost: *cost_usd,
                                    },
                                );

                                worktree_manager.remove_worktree(&worktree.path).await.ok();
                                worktree_manager.remove_branch(&worktree.branch).await.ok();

                                tui.task_summaries.push(TaskSummaryEntry {
                                    task_id: task_id.clone(),
                                    status: "Done".to_string(),
                                    cost_usd: *cost_usd,
                                    duration,
                                    retries: scheduler.retry_count(task_id),
                                });
                            }
                            MergeResult::Conflict { files } => {
                                let msg = tui.mux_output.format_worker_line(
                                    *worker_id,
                                    &format!("Merge conflict in {task_id}: {files:?}"),
                                );
                                tui.dashboard.push_log_line(&msg);
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

                                tui.task_summaries.push(TaskSummaryEntry {
                                    task_id: task_id.clone(),
                                    status: "Blocked".to_string(),
                                    cost_usd: *cost_usd,
                                    duration,
                                    retries: scheduler.retry_count(task_id),
                                });
                            }
                            MergeResult::Failed { error } => {
                                let msg = tui.mux_output.format_worker_line(
                                    *worker_id,
                                    &format!("Merge failed for {task_id}: {error}"),
                                );
                                tui.dashboard.push_log_line(&msg);
                                scheduler.mark_blocked(task_id);

                                tui.task_summaries.push(TaskSummaryEntry {
                                    task_id: task_id.clone(),
                                    status: "Blocked".to_string(),
                                    cost_usd: *cost_usd,
                                    duration,
                                    retries: scheduler.retry_count(task_id),
                                });
                            }
                        }
                    }
                } else if *success {
                    // --no-merge mode
                    scheduler.mark_done(task_id);
                    let msg = tui
                        .mux_output
                        .format_worker_line(*worker_id, &format!("Done (no merge): {task_id}"));
                    tui.dashboard.push_log_line(&msg);

                    tui.task_summaries.push(TaskSummaryEntry {
                        task_id: task_id.clone(),
                        status: "Done".to_string(),
                        cost_usd: *cost_usd,
                        duration,
                        retries: scheduler.retry_count(task_id),
                    });
                } else {
                    // Task failed
                    let requeued = scheduler.mark_failed(task_id);
                    if requeued {
                        let msg = tui.mux_output.format_worker_line(
                            *worker_id,
                            &format!("Task {task_id} failed, re-queued for retry"),
                        );
                        tui.dashboard.push_log_line(&msg);
                    } else {
                        let msg = tui.mux_output.format_worker_line(
                            *worker_id,
                            &format!("Task {task_id} blocked after max retries"),
                        );
                        tui.dashboard.push_log_line(&msg);

                        tui.task_summaries.push(TaskSummaryEntry {
                            task_id: task_id.clone(),
                            status: "Blocked".to_string(),
                            cost_usd: *cost_usd,
                            duration,
                            retries: scheduler.retry_count(task_id),
                        });
                    }
                }

                // Free the worker slot and reset status
                worker_slots.insert(*worker_id, WorkerSlot::Idle);
                tui.dashboard
                    .update_worker_status(*worker_id, WorkerStatus::idle(*worker_id));
                tui.mux_output.clear_worker(*worker_id);
            }

            WorkerEventKind::TaskFailed {
                worker_id,
                task_id,
                error,
                retries_left,
            } => {
                let msg = if *retries_left == 0 {
                    tui.mux_output.format_worker_line(
                        *worker_id,
                        &format!("Task {task_id} failed permanently: {error}"),
                    )
                } else {
                    tui.mux_output.format_worker_line(
                        *worker_id,
                        &format!(
                            "Task {task_id} failed ({retries_left} retries left): {error}"
                        ),
                    )
                };
                tui.dashboard.push_log_line(&msg);
            }

            WorkerEventKind::MergeStarted {
                worker_id,
                task_id,
            } => {
                let (cost, input, output) = tui.mux_output.worker_cost(*worker_id);
                let ws = WorkerStatus {
                    worker_id: *worker_id,
                    state: WorkerState::Merging,
                    task_id: Some(task_id.clone()),
                    component: task_lookup.get(task_id).map(|t| t.component.clone()),
                    phase: None,
                    cost_usd: cost,
                    input_tokens: input,
                    output_tokens: output,
                };
                tui.dashboard.update_worker_status(*worker_id, ws);
                let msg = tui
                    .mux_output
                    .format_worker_line(*worker_id, &format!("Merging: {task_id}"));
                tui.dashboard.push_log_line(&msg);
            }

            WorkerEventKind::MergeCompleted {
                worker_id,
                task_id,
                success,
                commit_hash,
            } => {
                let hash = commit_hash.as_deref().unwrap_or("???");
                let status_text = if *success { "ok" } else { "FAILED" };
                let msg = tui.mux_output.format_worker_line(
                    *worker_id,
                    &format!("Merge {task_id}: {status_text} ({hash})"),
                );
                tui.dashboard.push_log_line(&msg);
            }

            WorkerEventKind::MergeConflict {
                worker_id,
                task_id,
                conflicting_files,
            } => {
                let msg = tui.mux_output.format_worker_line(
                    *worker_id,
                    &format!("Merge conflict in {task_id}: {conflicting_files:?}"),
                );
                tui.dashboard.push_log_line(&msg);
            }

            WorkerEventKind::OutputLines { worker_id, lines } => {
                tui.dashboard.push_worker_output(*worker_id, lines);
            }
        }

        Ok(())
    }

    /// Resolve the model to use for a specific task.
    fn resolve_model(&self, task_id: &str, progress: &ProgressSummary) -> Option<String> {
        if let Some(fm) = &progress.frontmatter {
            if let Some(model) = fm.models.get(task_id) {
                return Some(model.clone());
            }
            if let Some(model) = &fm.default_model {
                return Some(model.clone());
            }
        }
        self.config.model.clone()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

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
        | WorkerEventKind::MergeConflict { worker_id, .. }
        | WorkerEventKind::OutputLines { worker_id, .. } => Some(*worker_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let config = ResolvedConfig {
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
            config,
            project_root: PathBuf::from("/tmp/test"),
            progress_path: PathBuf::from("/tmp/test/PROGRESS.md"),
            system_prompt: String::new(),
            verification_commands: None,
            use_nerd_font: false,
        };

        assert_eq!(
            orch.resolve_model("T01", &progress),
            Some("claude-opus-4-6".to_string())
        );
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

        let config = ResolvedConfig {
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
            config,
            project_root: PathBuf::from("/tmp/test"),
            progress_path: PathBuf::from("/tmp/test/PROGRESS.md"),
            system_prompt: String::new(),
            verification_commands: None,
            use_nerd_font: false,
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

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("30m"), Some(Duration::from_secs(1800)));
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("45s"), Some(Duration::from_secs(45)));
    }

    #[test]
    fn test_parse_duration_combined() {
        assert_eq!(
            parse_duration("1h30m"),
            Some(Duration::from_secs(3600 + 1800))
        );
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("abc"), None);
    }

    #[test]
    fn test_resolved_config_from_args() {
        use crate::commands::task::args::OrchestrateArgs;

        let cli_args = OrchestrateArgs {
            workers: Some(4),
            model: Some("cli-model".to_string()),
            max_retries: None,
            verbose: true,
            resume: false,
            dry_run: false,
            worktree_prefix: None,
            no_merge: false,
            max_cost: Some(5.0),
            timeout: Some("1h".to_string()),
            tasks: Some("T01,T03".to_string()),
        };

        let file_config = FileConfig::default();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        assert_eq!(config.workers, 4);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.model.as_deref(), Some("cli-model"));
        assert!(config.verbose);
        assert_eq!(config.max_cost, Some(5.0));
        assert_eq!(config.timeout, Some(Duration::from_secs(3600)));
        assert_eq!(
            config.task_filter,
            Some(vec!["T01".to_string(), "T03".to_string()])
        );
    }
}
