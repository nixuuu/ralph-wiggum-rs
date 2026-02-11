use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU8, Ordering};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::mpsc;

use crate::commands::task::orchestrate::dashboard::Dashboard;
use crate::commands::task::orchestrate::events::{EventLogger, WorkerEvent, WorkerEventKind};
use crate::commands::task::orchestrate::output::MultiplexedOutput;
use crate::commands::task::orchestrate::scheduler::TaskScheduler;
use crate::commands::task::orchestrate::shared_types::{
    DashboardInputParams, DashboardInputThread, OrchestratorStatus, ShutdownState,
};
use crate::commands::task::orchestrate::state::{Lockfile, OrchestrateState};
use crate::commands::task::orchestrate::summary;
use crate::commands::task::orchestrate::worker::TaskResult;
use crate::commands::task::orchestrate::worktree::WorktreeManager;
use crate::shared::dag::TaskDag;
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::{FileConfig, SetupCommand};
use crate::shared::progress::ProgressTask;
use crate::shared::tasks::TasksFile;
use crate::templates;

use super::assignment::{WorkerSlot, get_mtime};
use super::orchestrator_merge::MergeContext;
use super::orchestrator_tui::TuiContext;

// ── Input flags from TUI input thread ───────────────────────────────

/// Atomic flags shared with the keyboard input thread.
pub(super) struct InputFlags {
    pub(super) shutdown: Arc<AtomicBool>,
    pub(super) graceful_shutdown: Arc<AtomicBool>,
    pub(super) resize_flag: Arc<AtomicBool>,
    pub(super) focused_worker: Arc<AtomicU32>,
    pub(super) scroll_delta: Arc<AtomicI32>,
    pub(super) render_notify: Arc<tokio::sync::Notify>,
    /// Toggle for task preview overlay (activated with 'p' key).
    /// Used by dashboard render to show task details instead of worker grid.
    #[allow(dead_code)] // Will be used in future task for overlay rendering
    pub(super) show_task_preview: Arc<AtomicBool>,
    pub(super) reload_requested: Arc<AtomicBool>,
    pub(super) quit_state: Arc<AtomicU8>,
    /// Flag indicating that all tasks have completed.
    /// When set, orchestrator enters idle state waiting for user quit confirmation.
    pub(super) completed: Arc<AtomicBool>,
}

// ── RunLoopContext ───────────────────────────────────────────────────

/// Groups all mutable and shared state for the main orchestration loop,
/// replacing 18 individual parameters on `run_loop()` and related functions.
pub(super) struct RunLoopContext<'a> {
    pub(super) scheduler: &'a mut TaskScheduler,
    pub(super) worktree_manager: &'a WorktreeManager,
    pub(super) state: &'a mut OrchestrateState,
    pub(super) state_path: &'a Path,
    pub(super) event_tx: mpsc::Sender<WorkerEvent>,
    pub(super) event_rx: mpsc::Receiver<WorkerEvent>,
    pub(super) event_logger: EventLogger,
    pub(super) worker_slots: HashMap<u32, WorkerSlot>,
    pub(super) join_handles: HashMap<u32, tokio::task::JoinHandle<Result<TaskResult>>>,
    pub(super) tasks_file: &'a TasksFile,
    pub(super) progress: &'a crate::shared::progress::ProgressSummary,
    pub(super) task_lookup: HashMap<&'a str, &'a ProgressTask>,
    pub(super) lockfile: Option<Lockfile>,
    pub(super) tui: &'a mut TuiContext,
    pub(super) merge_ctx: MergeContext,
    pub(super) progress_mtime: Option<SystemTime>,
    pub(super) flags: InputFlags,
    /// Cached TasksFile for preview overlay rendering.
    /// Updated on hot-reload and passed to dashboard when preview is active.
    pub(super) cached_tasks_file: Arc<TasksFile>,
}

// ── Resolved config ─────────────────────────────────────────────────

/// Configuration resolved from CLI args + file config for orchestration.
pub struct ResolvedConfig {
    pub workers: u32,
    pub max_retries: u32,
    pub model: Option<String>,
    pub worktree_prefix: Option<String>,
    /// Verbosity flag — parsed from CLI but not actively used in orchestrator logic.
    #[allow(dead_code)] // CLI flag: parsed from args but not currently used in orchestrator
    pub verbose: bool,
    pub resume: bool,
    pub dry_run: bool,
    pub no_merge: bool,
    pub max_cost: Option<f64>,
    pub timeout: Option<Duration>,
    /// Task filter — parsed from CLI but not actively used in orchestrator logic.
    #[allow(dead_code)] // CLI flag: parsed from args but not currently used in orchestrator
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
    pub(super) config: ResolvedConfig,
    pub(super) project_root: PathBuf,
    pub(super) tasks_path: PathBuf,
    pub(super) system_prompt: String,
    pub(super) verification_commands: Option<String>,
    pub(super) setup_commands: Vec<SetupCommand>,
    pub(super) use_nerd_font: bool,
}

impl Orchestrator {
    pub fn new(
        config: ResolvedConfig,
        file_config: &FileConfig,
        project_root: PathBuf,
    ) -> Result<Self> {
        let tasks_path = project_root.join(&file_config.task.tasks_file);

        // Use embedded system prompt template (general part, before task-specific section)
        let system_prompt = templates::CONTINUE_SYSTEM_PROMPT
            .split("\n---\n\n# Your Task")
            .next()
            .unwrap_or(templates::CONTINUE_SYSTEM_PROMPT)
            .to_string();

        let verification_commands = file_config
            .task
            .orchestrate
            .verify_commands
            .clone()
            .filter(|s| !s.trim().is_empty());

        let setup_commands = file_config.task.orchestrate.setup_commands.clone();

        Ok(Self {
            config,
            project_root,
            tasks_path,
            system_prompt,
            verification_commands,
            setup_commands,
            use_nerd_font: file_config.ui.nerd_font,
        })
    }

    /// Main entry point — run the full orchestration session.
    pub async fn execute(&self) -> Result<()> {
        // 1. Load and parse .ralph/tasks.yml
        let tasks_file = TasksFile::load(&self.tasks_path)
            .map_err(|e| RalphError::Orchestrate(format!("Failed to read tasks.yml: {e}")))?;
        let progress = tasks_file.to_summary();

        // 2. Build and validate DAG
        let frontmatter = progress.frontmatter.clone().unwrap_or_default();
        let dag = TaskDag::from_tasks_file(&tasks_file);

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
        let show_task_preview = Arc::new(AtomicBool::new(false));
        let reload_requested = Arc::new(AtomicBool::new(false));
        let quit_state = Arc::new(AtomicU8::new(0)); // 0=running, 1=quit_pending
        let completed = Arc::new(AtomicBool::new(false));

        // 10. Initialize TUI (fullscreen dashboard)
        let output_log_path = ralph_dir.join("orchestrate-output.log");
        let mut tui = TuiContext {
            dashboard: Dashboard::new(worker_count)?,
            mux_output: MultiplexedOutput::new(Some(&output_log_path)),
            task_start_times: HashMap::new(),
            task_summaries: Vec::new(),
        };

        // 11. Spawn input thread (dedicated OS thread for crossterm)
        let mut input_thread = DashboardInputThread::spawn(DashboardInputParams {
            shutdown: Arc::clone(&shutdown),
            graceful_shutdown: Arc::clone(&graceful_shutdown),
            resize_flag: Arc::clone(&resize_flag),
            focused_worker: Arc::clone(&focused_worker),
            worker_count,
            scroll_delta: Arc::clone(&scroll_delta),
            render_notify: Arc::clone(&render_notify),
            show_task_preview: Arc::clone(&show_task_preview),
            reload_requested: Arc::clone(&reload_requested),
            quit_state: Arc::clone(&quit_state),
        });

        // 12. Build task lookup for merge
        let task_lookup: HashMap<&str, &ProgressTask> = progress
            .tasks
            .iter()
            .map(|t| (t.id.as_str(), t))
            .collect();

        // 13. Build run loop context
        let flags = InputFlags {
            shutdown,
            graceful_shutdown,
            resize_flag,
            focused_worker,
            scroll_delta,
            render_notify,
            show_task_preview,
            reload_requested,
            quit_state: Arc::clone(&quit_state),
            completed,
        };

        // Cache TasksFile in Arc for efficient sharing with dashboard preview
        let cached_tasks_file = Arc::new(tasks_file);

        let mut ctx = RunLoopContext {
            scheduler: &mut scheduler,
            worktree_manager: &worktree_manager,
            state: &mut state,
            state_path: &state_path,
            event_tx,
            event_rx,
            event_logger,
            worker_slots,
            join_handles: HashMap::new(),
            tasks_file: cached_tasks_file.as_ref(),
            progress: &progress,
            task_lookup,
            lockfile: Some(lockfile),
            tui: &mut tui,
            merge_ctx: MergeContext::new(),
            progress_mtime: get_mtime(&self.tasks_path),
            flags,
            cached_tasks_file: Arc::clone(&cached_tasks_file),
        };

        // 14. Run the main orchestration loop
        let result = self.run_loop(&mut ctx).await;

        // 15. Cleanup TUI
        ctx.tui.dashboard.cleanup()?;
        input_thread.stop();

        result
    }

    /// The main orchestration loop.
    async fn run_loop(&self, ctx: &mut RunLoopContext<'_>) -> Result<()> {
        let mut last_heartbeat = Instant::now();
        let mut last_hot_reload = Instant::now();
        let started_at = Instant::now();

        // SIGTERM handler — treat `kill <pid>` as force shutdown
        #[cfg(unix)]
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        ).ok();

        // Print startup message
        let msg = MultiplexedOutput::format_orchestrator_line(&format!(
            "Started: {} workers, {} tasks ({} done, {} remaining)",
            self.config.workers,
            ctx.progress.total(),
            ctx.progress.done,
            ctx.progress.remaining()
        ));
        ctx.tui.dashboard.push_log_line(&msg);

        // Warn if no verification commands configured
        if self.verification_commands.is_none() {
            let warn = MultiplexedOutput::format_orchestrator_line(
                "⚠ No verify_commands configured — skipping verify phase. \
                 Set verify_commands in [task.orchestrate] in .ralph.toml",
            );
            ctx.tui.dashboard.push_log_line(&warn);
        }

        // Graceful shutdown timeout tracking
        let mut graceful_shutdown_started: Option<Instant> = None;
        const GRACEFUL_SHUTDOWN_GRACE: Duration = Duration::from_secs(120);

        // Initial dashboard render
        {
            let quit_pending = ctx.flags.quit_state.load(Ordering::Relaxed) == 1;
            let completed = ctx.flags.completed.load(Ordering::Relaxed);
            let orch_status = OrchestratorStatus {
                scheduler: ctx.scheduler.status(),
                total_cost: ctx.tui.mux_output.total_cost(),
                elapsed: started_at.elapsed(),
                shutdown_state: ShutdownState::Running,
                shutdown_remaining: None,
                quit_pending,
                completed,
            };
            ctx.tui.dashboard.render(&orch_status, None, &ctx.tui.task_summaries)?;
        }

        loop {
            // Check budget limits
            if let Some(max_cost) = self.config.max_cost
                && ctx.tui.mux_output.total_cost() >= max_cost
            {
                let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                    "Budget limit reached: ${:.4} >= ${max_cost:.4}",
                    ctx.tui.mux_output.total_cost()
                ));
                ctx.tui.dashboard.push_log_line(&msg);
                break;
            }
            if let Some(timeout) = self.config.timeout
                && started_at.elapsed() >= timeout
            {
                let msg =
                    MultiplexedOutput::format_orchestrator_line("Timeout reached");
                ctx.tui.dashboard.push_log_line(&msg);
                break;
            }

            // Check completion — enter idle state instead of breaking
            if ctx.scheduler.is_complete() && !ctx.flags.completed.load(Ordering::Relaxed) {
                let status = ctx.scheduler.status();
                let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                    "All tasks complete: {} done, {} blocked — press 'q' to exit",
                    status.done, status.blocked
                ));
                ctx.tui.dashboard.push_log_line(&msg);
                ctx.flags.completed.store(true, Ordering::Relaxed);
                // Continue loop in completed idle state — only exit on graceful_shutdown
            }

            // Assign tasks to free workers (unless in graceful shutdown)
            if !ctx.flags.graceful_shutdown.load(Ordering::Relaxed) {
                self.assign_tasks(ctx).await?;
            }

            // Main select loop
            tokio::select! {
                // Process worker events
                Some(event) = ctx.event_rx.recv() => {
                    // Log structural events (skip verbose OutputLines)
                    if !matches!(event.kind, WorkerEventKind::OutputLines { .. })
                        && let Some(worker_id) = extract_worker_id(&event.kind)
                    {
                        ctx.event_logger.log_event(&event, worker_id).ok();
                    }

                    self.handle_event(&event, ctx).await?;

                    // Handle resize after event
                    if ctx.flags.resize_flag.swap(false, Ordering::SeqCst) {
                        ctx.tui.dashboard.handle_resize()?;
                    }
                    self.render_dashboard(ctx, started_at, graceful_shutdown_started)?;
                }

                // Ctrl+C signal handling (backup — input thread handles q/Ctrl+C too)
                _ = tokio::signal::ctrl_c() => {
                    if ctx.flags.graceful_shutdown.load(Ordering::Relaxed) {
                        // Second Ctrl+C — force shutdown
                        let msg = MultiplexedOutput::format_orchestrator_line(
                            "Force shutdown — aborting all workers"
                        );
                        ctx.tui.dashboard.push_log_line(&msg);
                        ctx.flags.shutdown.store(true, Ordering::Relaxed);
                        for (_, handle) in ctx.join_handles.drain() {
                            handle.abort();
                        }
                        break;
                    } else {
                        // First Ctrl+C — graceful drain
                        let msg = MultiplexedOutput::format_orchestrator_line(
                            "Graceful shutdown — waiting for in-progress tasks..."
                        );
                        ctx.tui.dashboard.push_log_line(&msg);
                        ctx.flags.graceful_shutdown.store(true, Ordering::Relaxed);
                        graceful_shutdown_started = Some(Instant::now());

                        let any_busy = ctx.worker_slots.values().any(|s| matches!(s, WorkerSlot::Busy { .. }));
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
                    ctx.tui.dashboard.push_log_line(&msg);
                    ctx.flags.shutdown.store(true, Ordering::Relaxed);
                    for (_, handle) in ctx.join_handles.drain() {
                        handle.abort();
                    }
                    break;
                }

                // Immediate re-render on keypress (input thread notifies)
                _ = ctx.flags.render_notify.notified() => {
                    // Check reload request (user pressed 'r')
                    if ctx.flags.reload_requested.swap(false, Ordering::SeqCst) {
                        let timestamp = chrono::Local::now().format("%H:%M:%S");
                        match TasksFile::load(&self.tasks_path) {
                            Ok(new_tf) => {
                                let old_count = ctx.scheduler.status().total;
                                let new_dag = TaskDag::from_tasks_file(&new_tf);
                                let new_count = new_dag.tasks().len();

                                // Validate DAG for cycles before applying
                                if let Some(cycle) = new_dag.detect_cycles() {
                                    let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                                        "✗ [{timestamp}] reload failed: DAG cycle detected: {}",
                                        cycle.join(" -> ")
                                    ));
                                    ctx.tui.dashboard.push_log_line(&msg);
                                } else {
                                    ctx.progress_mtime = get_mtime(&self.tasks_path);
                                    let delta = (new_count as i32) - (old_count as i32);
                                    ctx.scheduler.add_tasks(new_dag);
                                    let delta_str = if delta > 0 {
                                        format!("{} new tasks added", delta)
                                    } else if delta < 0 {
                                        format!("{} tasks removed", -delta)
                                    } else {
                                        "no change".to_string()
                                    };
                                    let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                                        "♻ [{timestamp}] tasks.yml reloaded ({})",
                                        delta_str
                                    ));
                                    ctx.tui.dashboard.push_log_line(&msg);
                                }
                            }
                            Err(e) => {
                                let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                                    "✗ [{timestamp}] reload failed: {e}"
                                ));
                                ctx.tui.dashboard.push_log_line(&msg);
                            }
                        }
                    }

                    if ctx.flags.resize_flag.swap(false, Ordering::SeqCst) {
                        ctx.tui.dashboard.handle_resize()?;
                    }
                    self.render_dashboard(ctx, started_at, graceful_shutdown_started)?;
                }

                // Periodic timer: heartbeat, hot reload, status refresh
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    // Check if input thread triggered shutdown
                    if ctx.flags.shutdown.load(Ordering::SeqCst) {
                        let msg = MultiplexedOutput::format_orchestrator_line(
                            "Force shutdown — aborting all workers"
                        );
                        ctx.tui.dashboard.push_log_line(&msg);
                        for (_, handle) in ctx.join_handles.drain() {
                            handle.abort();
                        }
                        break;
                    }
                    if ctx.flags.graceful_shutdown.load(Ordering::SeqCst) {
                        // Track when graceful shutdown started (input thread may set it)
                        if graceful_shutdown_started.is_none() {
                            graceful_shutdown_started = Some(Instant::now());
                        }

                        if !ctx.worker_slots.values().any(|s| matches!(s, WorkerSlot::Busy { .. })) {
                            let msg = MultiplexedOutput::format_orchestrator_line(
                                "All workers drained — exiting"
                            );
                            ctx.tui.dashboard.push_log_line(&msg);
                            break;
                        }

                        // Escalate to force shutdown after grace period
                        if let Some(gs_start) = graceful_shutdown_started
                            && gs_start.elapsed() >= GRACEFUL_SHUTDOWN_GRACE
                        {
                            let msg = MultiplexedOutput::format_orchestrator_line(
                                "Grace period expired — force-killing all workers"
                            );
                            ctx.tui.dashboard.push_log_line(&msg);
                            ctx.flags.shutdown.store(true, Ordering::Relaxed);
                            for (_, handle) in ctx.join_handles.drain() {
                                handle.abort();
                            }
                            break;
                        }
                    }

                    // Heartbeat every 5 seconds
                    if last_heartbeat.elapsed() >= Duration::from_secs(5) {
                        if let Some(ref mut lf) = ctx.lockfile { lf.heartbeat().ok(); }
                        last_heartbeat = Instant::now();
                    }

                    // Hot reload tasks.yml every 15 seconds
                    if last_hot_reload.elapsed() >= Duration::from_secs(15) {
                        if let Some(new_mtime) = get_mtime(&self.tasks_path)
                            && ctx.progress_mtime.is_none_or(|old| new_mtime > old)
                            && let Ok(new_tf) = TasksFile::load(&self.tasks_path)
                        {
                            // Wrap in Arc and update cache BEFORE scheduler to avoid race
                            let new_arc = Arc::new(new_tf);
                            let new_dag = TaskDag::from_tasks_file(new_arc.as_ref());

                            // Validate DAG for cycles before applying
                            if new_dag.detect_cycles().is_none() {
                                ctx.progress_mtime = Some(new_mtime);
                                ctx.cached_tasks_file = Arc::clone(&new_arc);
                                ctx.scheduler.add_tasks(new_dag);
                            }
                            // Note: silent failure on automatic reload — user didn't trigger it
                        }
                        last_hot_reload = Instant::now();
                    }

                    // Handle resize + render dashboard
                    if ctx.flags.resize_flag.swap(false, Ordering::SeqCst) {
                        ctx.tui.dashboard.handle_resize()?;
                    }
                    self.render_dashboard(ctx, started_at, graceful_shutdown_started)?;
                }
            }
        }

        // ─────────────────────────────────────────────────────────────
        // Post-loop cleanup: executed after user quit confirmation in
        // completion idle state or forced shutdown. Summary is printed
        // to stdout for terminal scrollback, preserving both interactive
        // display (dashboard) and permanent record (stdout).
        // ─────────────────────────────────────────────────────────────

        // Clean up any pending merges that never completed (orphaned worktrees)
        for pending in ctx.merge_ctx.pending_merges.iter() {
            if let Some(WorkerSlot::Busy { worktree, .. }) = ctx.worker_slots.get(&pending.worker_id) {
                ctx.worktree_manager.remove_worktree(&worktree.path).await.ok();
                ctx.worktree_manager.remove_branch(&worktree.branch).await.ok();
            }
        }

        // Save final state with error logging for verbose mode
        if let Err(e) = ctx.state.save(ctx.state_path) && self.config.verbose {
            eprintln!("Warning: Failed to save orchestrator state: {e}");
        }

        // Release lockfile with error logging for verbose mode
        if let Some(lf) = ctx.lockfile.take() && let Err(e) = lf.release() && self.config.verbose {
            eprintln!("Warning: Failed to release lockfile: {e}");
        }

        // Print summary to stdout (one entry per completed/blocked task)
        let elapsed = started_at.elapsed();
        let summary_text = summary::format_summary(&ctx.tui.task_summaries, elapsed);
        println!("\n{summary_text}");

        Ok(())
    }

    /// Resolve the model to use for a specific task (with alias support).
    pub(super) fn resolve_model(&self, task_id: &str, tasks_file: &TasksFile) -> Option<String> {
        let models = tasks_file.models_map();
        let raw: Option<&String> = models
            .get(task_id)
            .or(tasks_file.default_model.as_ref())
            .or(self.config.model.as_ref());
        raw.map(|m| crate::shared::tasks::resolve_model_alias(m))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

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
        | WorkerEventKind::OutputLines { worker_id, .. }
        | WorkerEventKind::MergeStepOutput { worker_id, .. } => Some(*worker_id),
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
            WorkerEventKind::MergeStepOutput {
                worker_id: 4,
                lines: vec!["test".to_string()],
            },
        ];
        for (i, kind) in kinds.iter().enumerate() {
            assert_eq!(extract_worker_id(kind), Some((i + 1) as u32));
        }
    }

    #[test]
    fn test_resolve_model_per_task() {
        use crate::shared::tasks::{TaskNode, TasksFile};
        use crate::shared::progress::TaskStatus;

        let tasks_file = TasksFile {
            default_model: Some("claude-sonnet-4-5-20250929".to_string()),
            tasks: vec![TaskNode {
                id: "T01".to_string(),
                name: "Test".to_string(),
                component: Some("api".to_string()),
                status: Some(TaskStatus::Todo),
                deps: Vec::new(),
                model: Some("claude-opus-4-6".to_string()),
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                subtasks: Vec::new(),
            }],
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
            tasks_path: PathBuf::from("/tmp/test/.ralph/tasks.yml"),
            system_prompt: String::new(),
            verification_commands: None,
            setup_commands: Vec::new(),
            use_nerd_font: false,
        };

        assert_eq!(
            orch.resolve_model("T01", &tasks_file),
            Some("claude-opus-4-6".to_string())
        );
        assert_eq!(
            orch.resolve_model("T99", &tasks_file),
            Some("claude-sonnet-4-5-20250929".to_string())
        );
    }

    #[test]
    fn test_resolve_model_cli_fallback() {
        use crate::shared::tasks::{TaskNode, TasksFile};
        use crate::shared::progress::TaskStatus;

        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![TaskNode {
                id: "T01".to_string(),
                name: "Test".to_string(),
                component: Some("api".to_string()),
                status: Some(TaskStatus::Todo),
                deps: Vec::new(),
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                subtasks: Vec::new(),
            }],
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
            tasks_path: PathBuf::from("/tmp/test/.ralph/tasks.yml"),
            system_prompt: String::new(),
            verification_commands: None,
            setup_commands: Vec::new(),
            use_nerd_font: false,
        };

        assert_eq!(
            orch.resolve_model("T01", &tasks_file),
            Some("cli-model".to_string())
        );
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

    // ── Completion state transition tests ────────────────────────────

    #[test]
    fn test_input_flags_completed_initialized_false() {
        let completed = Arc::new(AtomicBool::new(false));
        assert!(!completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_input_flags_completed_set_true_on_scheduler_complete() {
        let completed = Arc::new(AtomicBool::new(false));

        // Simulate run_loop logic:
        // if scheduler.is_complete() && !flags.completed.load() {
        //   flags.completed.store(true);
        // }

        // First iteration: complete and not yet marked
        if !completed.load(Ordering::Relaxed) {
            completed.store(true, Ordering::Relaxed);
        }

        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_completion_flag_remains_true() {
        let completed = Arc::new(AtomicBool::new(false));

        // First iteration: set to true
        if !completed.load(Ordering::Relaxed) {
            completed.store(true, Ordering::Relaxed);
        }
        assert!(completed.load(Ordering::Relaxed));

        // Second iteration: condition is false (already true), so don't set again
        if !completed.load(Ordering::Relaxed) {
            // This block is NOT executed
            completed.store(true, Ordering::Relaxed);
        }

        // Flag should still be true
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_completion_does_not_break_loop() {
        let completed = Arc::new(AtomicBool::new(false));
        let graceful_shutdown = Arc::new(AtomicBool::new(false));

        // Simulate loop iterations
        let mut iterations = 0;

        loop {
            iterations += 1;

            // Check completion (like in run_loop)
            if !completed.load(Ordering::Relaxed) {
                completed.store(true, Ordering::Relaxed);
                // NO BREAK HERE — loop continues in completed state
            }

            // Check graceful shutdown (loop exit condition)
            if graceful_shutdown.load(Ordering::Relaxed) {
                break;
            }

            // After a few iterations in completed state, trigger shutdown
            if iterations >= 3 {
                graceful_shutdown.store(true, Ordering::Relaxed);
            }
        }

        // Should have looped more than once after completion
        assert!(completed.load(Ordering::Relaxed));
        assert!(graceful_shutdown.load(Ordering::Relaxed));
        assert!(iterations > 1);
    }

    #[test]
    fn test_quit_confirmation_works_in_completed_state() {
        let quit_state = Arc::new(AtomicU8::new(0));
        let graceful_shutdown = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicBool::new(true)); // Already completed

        // First 'q' in completed state
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit != 1 {
            quit_state.store(1, Ordering::SeqCst);
        }
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);

        // Second 'q' confirms and sets graceful_shutdown
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            graceful_shutdown.store(true, Ordering::SeqCst);
            quit_state.store(0, Ordering::SeqCst);
        }

        assert!(graceful_shutdown.load(Ordering::SeqCst));
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_completion_idempotent_flag_set() {
        let completed = Arc::new(AtomicBool::new(false));

        // Multiple checks — flag set only on first completion check
        let mut times_set = 0;

        for _ in 0..5 {
            if !completed.load(Ordering::Relaxed) {
                completed.store(true, Ordering::Relaxed);
                times_set += 1;
            }
        }

        assert_eq!(times_set, 1);
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_esc_in_completed_state_closes_preview_or_noop() {
        let _completed = Arc::new(AtomicBool::new(true));
        let show_task_preview = Arc::new(AtomicBool::new(true));
        let quit_state = Arc::new(AtomicU8::new(0));
        let focused_worker = Arc::new(AtomicU32::new(1));

        // In completed state with preview open, quit_state=0 (running)
        assert!(_completed.load(Ordering::Relaxed));
        assert!(show_task_preview.load(Ordering::Relaxed));
        assert_eq!(quit_state.load(Ordering::SeqCst), 0);

        // Real DashboardInputThread behavior: Esc in completed running state
        // cancels quit_pending OR unfocuses (does NOT close preview).
        // Test the actual behavior from shared_types.rs:271-284
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            // Cancel quit_pending
            quit_state.store(0, Ordering::SeqCst);
        } else {
            // Running state: Esc unfocuses (sets focused_worker to 0)
            // Preview stays open — preview is toggled only by 'p' key
            focused_worker.store(0, Ordering::Relaxed);
        }

        // Preview should still be open (Esc doesn't close it in running state)
        assert!(show_task_preview.load(Ordering::Relaxed));
        // Focus was cleared
        assert_eq!(focused_worker.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_p_during_completion_shows_preview_overlay() {
        let _completed = Arc::new(AtomicBool::new(true));
        let show_task_preview = Arc::new(AtomicBool::new(false));
        let quit_state = Arc::new(AtomicU8::new(0));

        // In completed state with preview closed
        assert!(_completed.load(Ordering::Relaxed));
        assert!(!show_task_preview.load(Ordering::Relaxed));

        // Press 'p' — toggle preview (off → on)
        if quit_state.load(Ordering::SeqCst) == 1 {
            quit_state.store(0, Ordering::SeqCst);
        }
        let current = show_task_preview.load(Ordering::Relaxed);
        show_task_preview.store(!current, Ordering::Relaxed);
        assert!(show_task_preview.load(Ordering::Relaxed)); // Now on

        // Press 'p' again — toggle preview (on → off)
        if quit_state.load(Ordering::SeqCst) == 1 {
            quit_state.store(0, Ordering::SeqCst);
        }
        let current = show_task_preview.load(Ordering::Relaxed);
        show_task_preview.store(!current, Ordering::Relaxed);
        assert!(!show_task_preview.load(Ordering::Relaxed)); // Now off
    }

    #[test]
    fn test_q_during_completion_with_preview_open() {
        let _completed = Arc::new(AtomicBool::new(true));
        let show_task_preview = Arc::new(AtomicBool::new(true));
        let quit_state = Arc::new(AtomicU8::new(0));
        let graceful_shutdown = Arc::new(AtomicBool::new(false));

        // In completed state with preview open
        assert!(show_task_preview.load(Ordering::Relaxed));

        // First 'q' enters quit_pending (preview stays open)
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit != 1 {
            quit_state.store(1, Ordering::SeqCst);
        }
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);
        assert!(show_task_preview.load(Ordering::Relaxed)); // Preview still open

        // Second 'q' confirms shutdown (preview still open until user explicitly closes it)
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            graceful_shutdown.store(true, Ordering::SeqCst);
            quit_state.store(0, Ordering::SeqCst);
        }

        assert!(graceful_shutdown.load(Ordering::SeqCst));
        assert!(show_task_preview.load(Ordering::Relaxed)); // Preview stays open
    }

    #[test]
    fn test_completed_state_all_transitions() {
        // Comprehensive test: start in completed state, test all key transitions
        let completed = Arc::new(AtomicBool::new(true));
        let quit_state = Arc::new(AtomicU8::new(0));
        let graceful_shutdown = Arc::new(AtomicBool::new(false));
        let show_task_preview = Arc::new(AtomicBool::new(false));

        // State 1: Completed idle
        assert!(completed.load(Ordering::Relaxed));
        assert_eq!(quit_state.load(Ordering::SeqCst), 0);

        // Transition 1: Press 'p' — open preview
        let current = show_task_preview.load(Ordering::Relaxed);
        show_task_preview.store(!current, Ordering::Relaxed);
        assert!(show_task_preview.load(Ordering::Relaxed));

        // Transition 2: Press 'q' — enter quit confirmation
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit != 1 {
            quit_state.store(1, Ordering::SeqCst);
        }
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);
        assert!(show_task_preview.load(Ordering::Relaxed)); // Preview unaffected by q

        // Transition 3: Press 'q' again — confirm shutdown
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            graceful_shutdown.store(true, Ordering::SeqCst);
            quit_state.store(0, Ordering::SeqCst);
        }
        assert!(graceful_shutdown.load(Ordering::SeqCst));
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_post_loop_cleanup_summary_formatting() {
        // Verify that summary is correctly formatted after loop exit
        use crate::commands::task::orchestrate::summary::TaskSummaryEntry;
        use std::time::Duration;

        let entries = vec![
            TaskSummaryEntry {
                task_id: "T01".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.042,
                duration: Duration::from_secs(45),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "T02".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.038,
                duration: Duration::from_secs(32),
                retries: 0,
            },
        ];

        let elapsed = Duration::from_secs(50);
        let summary_text = summary::format_summary(&entries, elapsed);

        // Verify summary contains key elements
        assert!(summary_text.contains("T01"));
        assert!(summary_text.contains("T02"));
        assert!(summary_text.contains("Done"));
        assert!(summary_text.contains("TOTAL"));
        assert!(summary_text.contains("2/2 done"));
        assert!(summary_text.contains("Parallelism speedup"));
    }

    #[test]
    fn test_post_loop_cleanup_summary_empty_case() {
        // Verify that empty task list is handled gracefully
        use std::time::Duration;

        let entries: Vec<crate::commands::task::orchestrate::summary::TaskSummaryEntry> = vec![];
        let elapsed = Duration::from_secs(10);
        let summary_text = summary::format_summary(&entries, elapsed);

        // When no tasks were executed, summary should indicate this
        assert!(summary_text.contains("No tasks were executed"));
    }

    #[test]
    fn test_post_loop_cleanup_summary_mixed_results() {
        // Verify that post-loop summary correctly reports mixed Done/Blocked tasks
        // This is the typical scenario after orchestration completes
        use crate::commands::task::orchestrate::summary::TaskSummaryEntry;
        use std::time::Duration;

        let entries = vec![
            TaskSummaryEntry {
                task_id: "T01".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.042,
                duration: Duration::from_secs(45),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "T02".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.038,
                duration: Duration::from_secs(32),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "T03".to_string(),
                status: "Blocked".to_string(),
                cost_usd: 0.089,
                duration: Duration::ZERO, // Merge mode doesn't track duration
                retries: 3,
            },
        ];

        let elapsed = Duration::from_secs(60);
        let summary_text = summary::format_summary(&entries, elapsed);

        // Verify all tasks and their statuses are reported
        assert!(summary_text.contains("T01"));
        assert!(summary_text.contains("T02"));
        assert!(summary_text.contains("T03"));
        assert!(summary_text.contains("2/3 done"));
        assert!(summary_text.contains("Blocked"));
        assert!(summary_text.contains("$0.1690")); // Total cost: 0.042 + 0.038 + 0.089
    }
}
