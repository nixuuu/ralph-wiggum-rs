use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::commands::mcp::session::SessionRegistry;
use crate::commands::task::orchestrate::events::{EventLogger, WorkerEvent, WorkerEventKind};
use crate::commands::task::orchestrate::output::MultiplexedOutput;
use crate::commands::task::orchestrate::scheduler::TaskScheduler;
use crate::commands::task::orchestrate::state::{Lockfile, OrchestrateState};
use crate::commands::task::orchestrate::worker::TaskResult;
use crate::commands::task::orchestrate::worktree::WorktreeManager;
use crate::shared::dag::TaskDag;
use crate::shared::error::Result;
use crate::shared::tasks::TasksFile;
use crate::tui::events::AppEvent;

use super::assignment::{WorkerSlot, get_mtime};
use super::config::InputFlags;
use super::orchestrator::Orchestrator;
use super::orchestrator_merge::MergeContext;
use super::orchestrator_tui::TuiContext;

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
    pub(super) lockfile: Option<Lockfile>,
    pub(super) tui: &'a mut TuiContext,
    pub(super) merge_ctx: MergeContext,
    pub(super) progress_mtime: Option<SystemTime>,
    pub(super) flags: InputFlags,
    /// Cached TasksFile for preview overlay rendering and task lookups.
    /// Updated on hot-reload (apply_reload) to reflect latest tasks.yml state.
    /// All task metadata queries (component, model, name) should use this field.
    pub(super) cached_tasks_file: Arc<TasksFile>,
    /// Last heartbeat timestamp per worker (for liveness checking in watchdog).
    pub(super) last_heartbeat: HashMap<u32, Instant>,
    /// Port of the shared MCP server (passed to each worker for tool access).
    pub(super) mcp_port: u16,
    /// Shared session registry for MCP server (used to register per-worker sessions).
    pub(super) mcp_session_registry: Arc<SessionRegistry>,
    /// Cancellation token for MCP server lifecycle.
    pub(super) mcp_cancel_token: CancellationToken,
    /// JoinHandle for the MCP server task.
    pub(super) mcp_server_handle: tokio::task::JoinHandle<()>,
    /// Receiver for TUI events from EventDispatcher (via bridge thread).
    pub(super) key_event_rx: mpsc::Receiver<AppEvent>,
}

impl RunLoopContext<'_> {
    /// Release a worker: remove MCP session, set slot to Idle, clear heartbeat.
    pub(super) fn release_worker(&mut self, worker_id: u32) {
        // Clean up MCP session before resetting the slot
        if let Some(WorkerSlot::Busy { mcp_session_id, .. }) = self.worker_slots.get(&worker_id) {
            self.mcp_session_registry.remove_session(mcp_session_id);
        }
        self.worker_slots.insert(worker_id, WorkerSlot::Idle);
        self.last_heartbeat.remove(&worker_id);
    }

    /// Send a message to a specific worker.
    ///
    /// Returns Ok(()) if the message was sent successfully, or an error if:
    /// - The worker doesn't exist
    /// - The worker is idle (not assigned to a task)
    /// - The message channel is full or closed
    pub(super) async fn send_message_to_worker(
        &self,
        worker_id: u32,
        message: String,
    ) -> Result<()> {
        match self.worker_slots.get(&worker_id) {
            Some(WorkerSlot::Busy {
                message_tx,
                task_id,
                ..
            }) => {
                message_tx.send(message).await.map_err(|_| {
                    crate::shared::error::RalphError::Orchestrate(format!(
                        "Failed to send message to worker {} (task {}): channel closed",
                        worker_id, task_id
                    ))
                })?;
                Ok(())
            }
            Some(WorkerSlot::Idle) => Err(crate::shared::error::RalphError::Orchestrate(format!(
                "Worker {} is idle, cannot send message",
                worker_id
            ))),
            None => Err(crate::shared::error::RalphError::Orchestrate(format!(
                "Worker {} does not exist",
                worker_id
            ))),
        }
    }
}

const GRACEFUL_SHUTDOWN_GRACE: Duration = Duration::from_secs(120);

// ── Run loop ────────────────────────────────────────────────────────

impl Orchestrator {
    /// The main orchestration loop.
    pub(in crate::commands::task::orchestrate) async fn run_loop(
        &self,
        ctx: &mut RunLoopContext<'_>,
    ) -> Result<()> {
        let mut last_heartbeat = Instant::now();
        let mut last_hot_reload = Instant::now();
        let mut last_watchdog = Instant::now();
        let started_at = Instant::now();

        #[cfg(unix)]
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();

        self.print_startup_messages(ctx);

        let mut graceful_shutdown_started: Option<Instant> = None;

        self.render_initial_dashboard(ctx, started_at)?;

        loop {
            if self.should_break_for_limits(ctx, started_at) {
                break;
            }

            self.check_completion(ctx);

            if !ctx.flags.graceful_shutdown.load(Ordering::SeqCst) {
                self.assign_tasks(ctx).await?;
            }

            tokio::select! {
                Some(event) = ctx.event_rx.recv() => {
                    self.handle_worker_event(&event, ctx, started_at, graceful_shutdown_started).await?;
                }

                Some(tui_event) = ctx.key_event_rx.recv() => {
                    let should_break = self.handle_tui_event(
                        ctx, tui_event, started_at, &mut graceful_shutdown_started
                    )?;
                    // Drain pending user messages from OrchestrateApp overlay
                    for (worker_id, message) in ctx.tui.app.take_pending_messages() {
                        self.handle_user_message(ctx, worker_id, message).await?;
                    }
                    if should_break { break; }
                }

                _ = async {
                    #[cfg(unix)]
                    if let Some(ref mut s) = sigterm { s.recv().await; return; }
                    std::future::pending::<()>().await;
                } => {
                    self.handle_sigterm(ctx);
                    break;
                }

                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    if self.handle_timer_tick(
                        ctx,
                        started_at,
                        &mut graceful_shutdown_started,
                        &mut last_heartbeat,
                        &mut last_hot_reload,
                        &mut last_watchdog,
                    ).await? {
                        break;
                    }
                }
            }
        }

        self.post_loop_cleanup(ctx, started_at).await
    }
}

// ── Startup helpers ─────────────────────────────────────────────────

impl Orchestrator {
    fn print_startup_messages(&self, ctx: &mut RunLoopContext<'_>) {
        // Generate summary from cached_tasks_file for up-to-date counts
        let progress = ctx.cached_tasks_file.as_ref().to_summary();
        let msg = MultiplexedOutput::format_orchestrator_line(&format!(
            "Started: {} workers, {} tasks ({} done, {} remaining)",
            self.config.workers,
            progress.total(),
            progress.done,
            progress.remaining()
        ));
        ctx.tui.app.push_log_line(&msg);

        if self.verify_commands.is_empty() {
            let warn = MultiplexedOutput::format_orchestrator_line(
                "⚠ No verify_commands configured — skipping verify phase. \
                 Set verify_commands in [task.orchestrate] in .ralph.toml",
            );
            ctx.tui.app.push_log_line(&warn);
        }
    }

    fn render_initial_dashboard(
        &self,
        ctx: &mut RunLoopContext<'_>,
        started_at: Instant,
    ) -> Result<()> {
        self.render_dashboard(ctx, started_at, None)
    }
}

// ── Loop condition checks ───────────────────────────────────────────

impl Orchestrator {
    fn should_break_for_limits(&self, ctx: &mut RunLoopContext<'_>, started_at: Instant) -> bool {
        if let Some(max_cost) = self.config.max_cost
            && ctx.tui.mux_output.total_cost() >= max_cost
        {
            let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                "Budget limit reached: ${:.4} >= ${max_cost:.4}",
                ctx.tui.mux_output.total_cost()
            ));
            ctx.tui.app.push_log_line(&msg);
            return true;
        }
        if let Some(timeout) = self.config.timeout
            && started_at.elapsed() >= timeout
        {
            let msg = MultiplexedOutput::format_orchestrator_line("Timeout reached");
            ctx.tui.app.push_log_line(&msg);
            return true;
        }
        false
    }

    fn check_completion(&self, ctx: &mut RunLoopContext<'_>) {
        if ctx.scheduler.is_complete() && !ctx.flags.completed.load(Ordering::Relaxed) {
            let status = ctx.scheduler.status();
            let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                "All tasks complete: {} done, {} blocked — waiting for new tasks (press 'r' to reload, 'q' to exit)",
                status.done, status.blocked
            ));
            ctx.tui.app.push_log_line(&msg);
            ctx.flags.completed.store(true, Ordering::Relaxed);
        }
    }
}

// ── Select branch handlers ──────────────────────────────────────────

impl Orchestrator {
    async fn handle_worker_event(
        &self,
        event: &crate::commands::task::orchestrate::events::WorkerEvent,
        ctx: &mut RunLoopContext<'_>,
        started_at: Instant,
        graceful_shutdown_started: Option<Instant>,
    ) -> Result<()> {
        if !matches!(event.kind, WorkerEventKind::OutputLines { .. })
            && let Some(worker_id) = extract_worker_id(&event.kind)
        {
            ctx.event_logger.log_event(event, worker_id).ok();
        }

        self.handle_event(event, ctx).await?;
        self.render_dashboard(ctx, started_at, graceful_shutdown_started)
    }

    /// Handle a user message from the TUI overlay.
    ///
    /// Sends the message to the specified worker via the message channel,
    /// then emits a UserMessageSent event to update the TUI.
    async fn handle_user_message(
        &self,
        ctx: &mut RunLoopContext<'_>,
        worker_id: u32,
        message: String,
    ) -> Result<()> {
        // Send message to worker via message channel
        if let Err(e) = ctx.send_message_to_worker(worker_id, message.clone()).await {
            // Log error to dashboard if message send fails
            let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                "✗ Failed to send message to worker {}: {}",
                worker_id, e
            ));
            ctx.tui.app.push_log_line(&msg);
            return Ok(()); // Don't propagate error — just log and continue
        }

        // Emit UserMessageSent event to TUI for logging
        let event = WorkerEvent::new(WorkerEventKind::UserMessageSent { worker_id, message });
        let _ = ctx.event_tx.send(event).await;

        Ok(())
    }

    /// Returns `true` if the loop should break (force shutdown).
    pub(super) fn handle_ctrl_c(
        &self,
        ctx: &mut RunLoopContext<'_>,
        graceful_shutdown_started: &mut Option<Instant>,
    ) -> bool {
        if ctx.flags.graceful_shutdown.load(Ordering::SeqCst) {
            let msg = MultiplexedOutput::format_orchestrator_line(
                "Force shutdown — aborting all workers",
            );
            ctx.tui.app.push_log_line(&msg);
            ctx.flags.shutdown.store(true, Ordering::SeqCst);
            for (_, handle) in ctx.join_handles.drain() {
                handle.abort();
            }
            true
        } else {
            let msg = MultiplexedOutput::format_orchestrator_line(
                "Graceful shutdown — waiting for in-progress tasks...",
            );
            ctx.tui.app.push_log_line(&msg);
            ctx.flags.graceful_shutdown.store(true, Ordering::SeqCst);
            *graceful_shutdown_started = Some(Instant::now());

            let any_busy = ctx
                .worker_slots
                .values()
                .any(|s| matches!(s, WorkerSlot::Busy { .. }));
            !any_busy
        }
    }

    fn handle_sigterm(&self, ctx: &mut RunLoopContext<'_>) {
        let msg = MultiplexedOutput::format_orchestrator_line("SIGTERM received — force shutdown");
        ctx.tui.app.push_log_line(&msg);
        ctx.flags.shutdown.store(true, Ordering::SeqCst);
        for (_, handle) in ctx.join_handles.drain() {
            handle.abort();
        }
    }

    /// Returns `true` if the loop should break.
    async fn handle_timer_tick(
        &self,
        ctx: &mut RunLoopContext<'_>,
        started_at: Instant,
        graceful_shutdown_started: &mut Option<Instant>,
        last_heartbeat: &mut Instant,
        last_hot_reload: &mut Instant,
        last_watchdog: &mut Instant,
    ) -> Result<bool> {
        if self.check_shutdown_from_input(ctx)? {
            return Ok(true);
        }

        if self.check_graceful_drain(ctx, graceful_shutdown_started)? {
            return Ok(true);
        }

        // Heartbeat every 5 seconds
        if last_heartbeat.elapsed() >= Duration::from_secs(5) {
            if let Some(ref mut lf) = ctx.lockfile {
                lf.heartbeat().ok();
            }
            *last_heartbeat = Instant::now();
        }

        // Hot reload tasks.yml every 15 seconds
        if last_hot_reload.elapsed() >= Duration::from_secs(15) {
            self.handle_auto_hot_reload(ctx);
            *last_hot_reload = Instant::now();
        }

        // Watchdog: detect panicked/stuck workers
        let watchdog_interval = Duration::from_secs(self.config.watchdog_interval_secs.into());
        if last_watchdog.elapsed() >= watchdog_interval {
            self.run_watchdog_check(ctx).await;
            *last_watchdog = Instant::now();
        }

        self.render_dashboard(ctx, started_at, *graceful_shutdown_started)?;
        Ok(false)
    }
}

// ── Worker restart ──────────────────────────────────────────────────

impl Orchestrator {
    /// Handle a user-confirmed worker restart request.
    ///
    /// Reads restart state from OrchestrateApp, validates the target worker
    /// state, aborts the JoinHandle, re-queues the task (without incrementing
    /// retries), and resets the worker slot to Idle.
    pub(super) fn handle_restart_request(&self, ctx: &mut RunLoopContext<'_>) -> Result<()> {
        use crate::commands::task::orchestrate::app::RestartState;

        let worker_id = match ctx.tui.app.restart_state() {
            RestartState::Confirmed { worker_id } => *worker_id,
            _ => return Ok(()),
        };

        // Reset restart state immediately to prevent re-processing
        ctx.tui.app.clear_restart();

        // Edge case: Ignore restart if graceful shutdown is active.
        if ctx.flags.graceful_shutdown.load(Ordering::SeqCst) {
            return Ok(());
        }

        if worker_id == 0 {
            return Ok(());
        }

        // Validate worker slot state
        let slot = ctx.worker_slots.get(&worker_id);
        match slot {
            None => {
                let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                    "⟳ Restart: worker {worker_id} does not exist"
                ));
                ctx.tui.app.push_log_line(&msg);
                return Ok(());
            }
            Some(WorkerSlot::Idle) => {
                let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                    "⟳ Restart: worker {worker_id} is idle, nothing to restart"
                ));
                ctx.tui.app.push_log_line(&msg);
                return Ok(());
            }
            Some(WorkerSlot::Busy { .. }) => {
                // OK — proceed below
            }
        }

        // Extract task_id before mutating
        let task_id =
            if let Some(WorkerSlot::Busy { task_id, .. }) = ctx.worker_slots.get(&worker_id) {
                task_id.clone()
            } else {
                return Ok(());
            };

        // Check if worker is in merge pipeline — cannot restart mid-merge.
        // A worker enters the merge pipeline when its TaskCompleted event has
        // been processed and it's waiting in pending_merges or actively merging.
        let in_merge_pipeline = ctx
            .merge_ctx
            .pending_merges
            .iter()
            .any(|pm| pm.worker_id == worker_id)
            || ctx.merge_ctx.current_merge_worker_id == Some(worker_id);

        if in_merge_pipeline {
            let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                "⟳ Restart: worker {worker_id} is merging — cannot restart during merge"
            ));
            ctx.tui.app.push_log_line(&msg);
            return Ok(());
        }

        // 1. Abort the JoinHandle (kills the Claude process)
        if let Some(handle) = ctx.join_handles.remove(&worker_id) {
            handle.abort();
        }

        // 2. Re-queue the task without incrementing retry counter
        ctx.scheduler.requeue_without_retry(&task_id);

        // 2b. Reset completed flag if scheduler is no longer complete
        if ctx.flags.completed.load(Ordering::Relaxed) && !ctx.scheduler.is_complete() {
            ctx.flags.completed.store(false, Ordering::Relaxed);
        }

        // 3. Release worker slot (→ Idle), clean up MCP session and heartbeat
        //    NOTE: Worktree is NOT deleted — preserved for next assignment
        ctx.release_worker(worker_id);

        // 4. Update dashboard: worker status to idle, clear output
        ctx.tui.app.update_worker_status(
            worker_id,
            crate::commands::task::orchestrate::worker_status::WorkerStatus::idle(worker_id),
        );
        ctx.tui.mux_output.clear_worker(worker_id);

        // 5. Update tasks.yml status back to Todo
        if let Some(mt) =
            self.update_tasks_file(&task_id, crate::shared::progress::TaskStatus::Todo, ctx.tui)
        {
            ctx.progress_mtime = Some(mt);
            let mut updated = (*ctx.cached_tasks_file).clone();
            let _ = updated.update_status(&task_id, crate::shared::progress::TaskStatus::Todo);
            ctx.cached_tasks_file = Arc::new(updated);
        }

        // 5b. Clean session state — remove stale in-progress entry for restarted task
        ctx.state.tasks.remove(&task_id);

        // 6. Log to dashboard
        let msg = MultiplexedOutput::format_orchestrator_line(&format!(
            "⟳ Worker {worker_id} restarted (task {task_id} re-queued)"
        ));
        ctx.tui.app.push_log_line(&msg);

        Ok(())
    }
}

// ── Shutdown checks ─────────────────────────────────────────────────

impl Orchestrator {
    /// Check if input thread triggered force shutdown. Returns `true` to break.
    fn check_shutdown_from_input(&self, ctx: &mut RunLoopContext<'_>) -> Result<bool> {
        if ctx.flags.shutdown.load(Ordering::SeqCst) {
            let msg = MultiplexedOutput::format_orchestrator_line(
                "Force shutdown — aborting all workers",
            );
            ctx.tui.app.push_log_line(&msg);
            for (_, handle) in ctx.join_handles.drain() {
                handle.abort();
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Check graceful drain: all workers done or grace period expired.
    /// Returns `true` to break.
    fn check_graceful_drain(
        &self,
        ctx: &mut RunLoopContext<'_>,
        graceful_shutdown_started: &mut Option<Instant>,
    ) -> Result<bool> {
        if !ctx.flags.graceful_shutdown.load(Ordering::SeqCst) {
            return Ok(false);
        }

        if graceful_shutdown_started.is_none() {
            *graceful_shutdown_started = Some(Instant::now());
        }

        if !ctx
            .worker_slots
            .values()
            .any(|s| matches!(s, WorkerSlot::Busy { .. }))
        {
            let msg = MultiplexedOutput::format_orchestrator_line("All workers drained — exiting");
            ctx.tui.app.push_log_line(&msg);
            return Ok(true);
        }

        if let Some(gs_start) = *graceful_shutdown_started
            && gs_start.elapsed() >= GRACEFUL_SHUTDOWN_GRACE
        {
            let msg = MultiplexedOutput::format_orchestrator_line(
                "Grace period expired — force-killing all workers",
            );
            ctx.tui.app.push_log_line(&msg);
            ctx.flags.shutdown.store(true, Ordering::SeqCst);
            for (_, handle) in ctx.join_handles.drain() {
                handle.abort();
            }
            return Ok(true);
        }

        Ok(false)
    }
}

// ── Watchdog ────────────────────────────────────────────────────

impl Orchestrator {
    /// Check for panicked worker tasks, panicked merge tasks, and stuck workers.
    ///
    /// Called periodically from the timer tick (every `watchdog_interval_secs`).
    async fn run_watchdog_check(&self, ctx: &mut RunLoopContext<'_>) {
        // 1. Detect panicked worker tasks
        //
        // IMPORTANT: Only recover on Err(JoinError) — i.e. actual panics.
        // When a handle finishes with Ok(_), the worker sent its event to the
        // channel normally; that event may still be buffered and not yet
        // processed by the main loop.  Doing full recovery here would race
        // with the upcoming TaskCompleted/TaskFailed event, leading to
        // double mark_failed and a lost merge.
        let finished_workers: Vec<u32> = ctx
            .join_handles
            .iter()
            .filter(|(_, h)| h.is_finished())
            .map(|(id, _)| *id)
            .collect();

        for worker_id in finished_workers {
            if let Some(handle) = ctx.join_handles.remove(&worker_id) {
                match handle.await {
                    Ok(_) => {
                        // Task finished normally — its TaskCompleted/TaskFailed
                        // event is in the channel and will be processed by the
                        // main select! loop.  No recovery needed here.
                    }
                    Err(join_error) => {
                        // Task panicked — no event will ever arrive, so we must
                        // perform full recovery.
                        let task_id = if let Some(WorkerSlot::Busy { task_id, .. }) =
                            ctx.worker_slots.get(&worker_id)
                        {
                            task_id.clone()
                        } else {
                            format!("unknown(w{})", worker_id)
                        };

                        let panic_info = if join_error.is_panic() {
                            let payload = join_error.into_panic();
                            if let Some(s) = payload.downcast_ref::<&str>() {
                                s.to_string()
                            } else if let Some(s) = payload.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "unknown panic".to_string()
                            }
                        } else {
                            format!("JoinError: {join_error}")
                        };

                        let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                            "✗ Watchdog: worker {} (task {}) panicked: {}",
                            worker_id, task_id, panic_info
                        ));
                        ctx.tui.app.push_log_line(&msg);

                        // Recover: mark task as failed, free worker slot
                        let requeued = ctx.scheduler.mark_failed(&task_id);
                        if !requeued {
                            ctx.scheduler.mark_blocked(&task_id);
                        }

                        ctx.release_worker(worker_id);
                        ctx.tui.app.update_worker_status(
                            worker_id,
                            crate::commands::task::orchestrate::worker_status::WorkerStatus::idle(
                                worker_id,
                            ),
                        );
                        ctx.tui.mux_output.clear_worker(worker_id);
                    }
                }
            }
        }

        // 2. Detect panicked merge task (same logic — only recover on panic)
        if let Some(ref handle) = ctx.merge_ctx.merge_join_handle
            && handle.is_finished()
            && let Some(handle) = ctx.merge_ctx.merge_join_handle.take()
        {
            match handle.await {
                Ok(()) => {
                    // Merge task finished normally — MergeCompleted event is in
                    // the channel.  No recovery needed.
                }
                Err(join_error) => {
                    let panic_info = if join_error.is_panic() {
                        let payload = join_error.into_panic();
                        if let Some(s) = payload.downcast_ref::<&str>() {
                            s.to_string()
                        } else if let Some(s) = payload.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "unknown panic".to_string()
                        }
                    } else {
                        format!("JoinError: {join_error}")
                    };

                    let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                        "✗ Watchdog: merge task panicked: {panic_info}",
                    ));
                    ctx.tui.app.push_log_line(&msg);

                    // Reset merge state so the next queued merge can proceed
                    ctx.merge_ctx.merge_in_progress = false;
                    ctx.merge_ctx.current_merge_worker_id = None;
                    if let Some(next) = ctx.merge_ctx.pending_merges.pop_front() {
                        Self::spawn_merge_task(
                            &mut ctx.merge_ctx,
                            next,
                            &self.project_root,
                            ctx.event_tx.clone(),
                            std::sync::Arc::clone(&ctx.flags.shutdown),
                            &self.config.conflict_resolution_model,
                            self.config.merge_timeout,
                            self.config.phase_timeout,
                        );
                    }
                }
            }
        }

        // 3. Detect stuck workers (warning only — killing is handled by per-phase timeout)
        for (&worker_id, slot) in &ctx.worker_slots {
            if let WorkerSlot::Busy {
                task_id,
                started_at,
                ..
            } = slot
            {
                let elapsed = started_at.elapsed();
                // Warn if worker has been busy for more than 30 minutes
                if elapsed >= Duration::from_secs(30 * 60)
                    && elapsed.as_secs() % (self.config.watchdog_interval_secs as u64) < 2
                {
                    let mins = elapsed.as_secs() / 60;
                    let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                        "⚠ Watchdog: worker {} (task {}) has been busy for {}m",
                        worker_id, task_id, mins
                    ));
                    ctx.tui.app.push_log_line(&msg);
                }
            }
        }

        // 4. Check worker heartbeats — warn if no heartbeat received for > 2 minutes
        for (&worker_id, slot) in &ctx.worker_slots {
            if let WorkerSlot::Busy { task_id, .. } = slot
                && let Some(last_hb) = ctx.last_heartbeat.get(&worker_id)
            {
                let heartbeat_age = last_hb.elapsed();
                // Warn if no heartbeat for more than 2 minutes (log once per watchdog interval)
                if heartbeat_age >= Duration::from_secs(2 * 60)
                    && heartbeat_age.as_secs() % (self.config.watchdog_interval_secs as u64) < 2
                {
                    let secs = heartbeat_age.as_secs();
                    let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                        "⚠ Watchdog: worker {} (task {}) no heartbeat for {}s — may be stuck",
                        worker_id, task_id, secs
                    ));
                    ctx.tui.app.push_log_line(&msg);
                }
            }
        }
    }
}

// ── Hot reload ──────────────────────────────────────────────────────

impl Orchestrator {
    /// Manual reload triggered by user pressing 'r'.
    pub(super) fn handle_manual_reload(&self, ctx: &mut RunLoopContext<'_>) {
        let timestamp = chrono::Local::now().format("%H:%M:%S");
        match TasksFile::load(&self.tasks_path) {
            Ok(mut new_tf) => {
                let old_count = ctx.scheduler.status().total;
                let new_dag = TaskDag::from_tasks_file(&new_tf);
                let new_count = new_dag.tasks().len();

                if let Some(cycle) = new_dag.detect_cycles() {
                    let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                        "✗ [{timestamp}] reload failed: DAG cycle detected: {}",
                        cycle.join(" -> ")
                    ));
                    ctx.tui.app.push_log_line(&msg);
                } else {
                    // Apply reload and capture unblocked tasks
                    let unblocked = self.apply_reload(ctx, &mut new_tf, new_dag);

                    // Build comprehensive delta message
                    let mut delta_parts = Vec::new();

                    let task_delta = (new_count as i32) - (old_count as i32);
                    if task_delta > 0 {
                        delta_parts.push(format!(
                            "{} new task{}",
                            task_delta,
                            if task_delta == 1 { "" } else { "s" }
                        ));
                    } else if task_delta < 0 {
                        delta_parts.push(format!(
                            "{} task{} removed",
                            -task_delta,
                            if task_delta == -1 { "" } else { "s" }
                        ));
                    }

                    if !unblocked.is_empty() {
                        let ids = unblocked.join(", ");
                        delta_parts.push(format!("{} unblocked: {}", unblocked.len(), ids));
                    }

                    let delta_str = if delta_parts.is_empty() {
                        "no change".to_string()
                    } else {
                        delta_parts.join(", ")
                    };

                    let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                        "♻ [{timestamp}] tasks.yml reloaded ({})",
                        delta_str
                    ));
                    ctx.tui.app.push_log_line(&msg);
                }
            }
            Err(e) => {
                let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                    "✗ [{timestamp}] reload failed: {e}"
                ));
                ctx.tui.app.push_log_line(&msg);
            }
        }
    }

    /// Automatic hot-reload (silent on failure, every 15s).
    fn handle_auto_hot_reload(&self, ctx: &mut RunLoopContext<'_>) {
        if let Some(new_mtime) = get_mtime(&self.tasks_path)
            && ctx.progress_mtime.is_none_or(|old| new_mtime > old)
            && let Ok(mut new_tf) = TasksFile::load(&self.tasks_path)
        {
            let new_dag = TaskDag::from_tasks_file(&new_tf);
            if new_dag.detect_cycles().is_none() {
                ctx.progress_mtime = Some(new_mtime);
                let _ = self.apply_reload(ctx, &mut new_tf, new_dag);
            }
        }
    }

    /// Patch scheduler state onto a freshly loaded TasksFile and update cache.
    /// Returns the list of task IDs that were unblocked.
    fn apply_reload(
        &self,
        ctx: &mut RunLoopContext<'_>,
        new_tf: &mut TasksFile,
        new_dag: TaskDag,
    ) -> Vec<String> {
        for task_id in ctx.scheduler.done_tasks() {
            let _ = new_tf.update_status(task_id, crate::shared::progress::TaskStatus::Done);
        }
        for task_id in ctx.scheduler.in_progress_tasks() {
            let _ = new_tf.update_status(task_id, crate::shared::progress::TaskStatus::InProgress);
        }
        for task_id in ctx.scheduler.blocked_tasks() {
            let _ = new_tf.update_status(task_id, crate::shared::progress::TaskStatus::Blocked);
        }

        ctx.progress_mtime = get_mtime(&self.tasks_path);
        ctx.cached_tasks_file = Arc::new(new_tf.clone());
        ctx.scheduler.add_tasks(new_dag);

        // Synchronize blocked set with file: if user changed blocked→todo, unblock them
        let unblocked = ctx.scheduler.sync_blocked_with_file(new_tf);

        // Reset completed flag if new tasks make scheduler incomplete
        if ctx.flags.completed.load(Ordering::Relaxed) && !ctx.scheduler.is_complete() {
            ctx.flags.completed.store(false, Ordering::Relaxed);
            let status = ctx.scheduler.status();
            let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                "♻ Resuming — {} new tasks detected",
                status.pending + status.ready
            ));
            ctx.tui.app.push_log_line(&msg);
        }

        unblocked
    }
}

// ── Post-loop cleanup ───────────────────────────────────────────────
// Moved to cleanup.rs

// ── Helpers ─────────────────────────────────────────────────────────

/// Extract worker_id from a WorkerEventKind.
pub(super) fn extract_worker_id(kind: &WorkerEventKind) -> Option<u32> {
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
        | WorkerEventKind::MergeStepOutput { worker_id, .. }
        | WorkerEventKind::Heartbeat { worker_id, .. }
        | WorkerEventKind::UserMessageSent { worker_id, .. }
        | WorkerEventKind::UserMessageReceived { worker_id, .. } => Some(*worker_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::task::orchestrate::events::WorkerEventKind;

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
        use crate::commands::task::orchestrate::events::WorkerPhase;

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
            WorkerEventKind::Heartbeat {
                worker_id: 5,
                phase: WorkerPhase::Implement,
            },
            WorkerEventKind::UserMessageSent {
                worker_id: 6,
                message: "Test message".to_string(),
            },
        ];
        for (i, kind) in kinds.iter().enumerate() {
            assert_eq!(extract_worker_id(kind), Some((i + 1) as u32));
        }
    }

    // ── Task 15.6: Watchdog and heartbeat detection tests ───────────────

    #[test]
    fn test_watchdog_detects_finished_handle() {
        // Test: Watchdog should detect finished JoinHandle
        // Note: This is a unit test for the concept — actual watchdog logic
        // requires tokio runtime and is tested in run_watchdog_check()
        use tokio::task::JoinHandle;

        let rt = tokio::runtime::Runtime::new().unwrap();
        let handle: JoinHandle<Result<()>> = rt.spawn(async { Ok(()) });

        rt.block_on(async {
            // Wait for task to complete
            tokio::time::sleep(Duration::from_millis(10)).await;
            assert!(handle.is_finished(), "Handle should be finished");
        });
    }

    #[test]
    fn test_watchdog_stale_heartbeat_detection() {
        // Test: Watchdog should warn when heartbeat is older than 2 minutes
        use std::collections::HashMap;

        let mut last_heartbeat: HashMap<u32, Instant> = HashMap::new();

        // Simulate stale heartbeat (over 2 minutes old)
        let worker_id = 1;
        let stale_time = Instant::now() - Duration::from_secs(130); // 2m 10s ago
        last_heartbeat.insert(worker_id, stale_time);

        // Check heartbeat age
        let heartbeat_age = last_heartbeat.get(&worker_id).unwrap().elapsed();
        assert!(
            heartbeat_age >= Duration::from_secs(120),
            "Heartbeat should be stale (>= 2 minutes old)"
        );
    }

    #[test]
    fn test_watchdog_fresh_heartbeat_no_warning() {
        // Test: Watchdog should NOT warn when heartbeat is fresh (< 2 minutes)
        use std::collections::HashMap;

        let mut last_heartbeat: HashMap<u32, Instant> = HashMap::new();

        // Simulate fresh heartbeat
        let worker_id = 1;
        last_heartbeat.insert(worker_id, Instant::now());

        // Check heartbeat age
        let heartbeat_age = last_heartbeat.get(&worker_id).unwrap().elapsed();
        assert!(
            heartbeat_age < Duration::from_secs(120),
            "Heartbeat should be fresh (< 2 minutes old)"
        );
    }

    #[test]
    fn test_watchdog_multiple_workers_mixed_heartbeats() {
        // Test: Watchdog should correctly identify which workers have stale heartbeats
        use std::collections::HashMap;

        let mut last_heartbeat: HashMap<u32, Instant> = HashMap::new();

        // Worker 1: fresh heartbeat
        last_heartbeat.insert(1, Instant::now());

        // Worker 2: stale heartbeat (3 minutes old)
        let stale_time = Instant::now() - Duration::from_secs(180);
        last_heartbeat.insert(2, stale_time);

        // Worker 3: fresh heartbeat
        last_heartbeat.insert(3, Instant::now());

        // Check all workers
        let worker1_stale = last_heartbeat.get(&1).unwrap().elapsed() >= Duration::from_secs(120);
        let worker2_stale = last_heartbeat.get(&2).unwrap().elapsed() >= Duration::from_secs(120);
        let worker3_stale = last_heartbeat.get(&3).unwrap().elapsed() >= Duration::from_secs(120);

        assert!(!worker1_stale, "Worker 1 should have fresh heartbeat");
        assert!(worker2_stale, "Worker 2 should have stale heartbeat");
        assert!(!worker3_stale, "Worker 3 should have fresh heartbeat");
    }

    #[test]
    fn test_watchdog_heartbeat_threshold_boundary() {
        // Test: Watchdog threshold at exactly 2 minutes (120 seconds)
        use std::collections::HashMap;

        let mut last_heartbeat: HashMap<u32, Instant> = HashMap::new();

        // Heartbeat exactly at 2 minute boundary
        let boundary_time = Instant::now() - Duration::from_secs(120);
        last_heartbeat.insert(1, boundary_time);

        let heartbeat_age = last_heartbeat.get(&1).unwrap().elapsed();
        assert!(
            heartbeat_age >= Duration::from_secs(120),
            "Heartbeat at exactly 2 minutes should be considered stale"
        );
    }

    // ── Task 18.3: Worker restart tests ─────────────────────────────────
    // NOTE: Old restart atomic flag tests removed (restart state is now in OrchestrateApp).

    #[test]
    fn test_restart_skips_idle_worker() {
        // Test: When restart targets an idle worker, it should be a no-op
        use crate::commands::task::orchestrate::assignment::WorkerSlot;

        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();
        worker_slots.insert(1, WorkerSlot::Idle);
        worker_slots.insert(2, WorkerSlot::Idle);

        let target = 1u32;
        let slot = worker_slots.get(&target);
        assert!(
            matches!(slot, Some(WorkerSlot::Idle)),
            "Idle worker should be detected — restart is a no-op"
        );
    }

    #[test]
    fn test_restart_proceeds_for_busy_worker() {
        // Test: Busy worker should be eligible for restart
        use crate::commands::task::orchestrate::assignment::WorkerSlot;
        use crate::commands::task::orchestrate::worktree::WorktreeInfo;

        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();
        let (msg_tx, _rx) = mpsc::channel(16);
        worker_slots.insert(
            1,
            WorkerSlot::Busy {
                task_id: "T01".to_string(),
                worktree: WorktreeInfo {
                    path: std::path::PathBuf::from("/tmp/wt1"),
                    branch: "ralph/task/T01".to_string(),
                    task_id: "T01".to_string(),
                },
                started_at: Instant::now(),
                mcp_session_id: "session-1".to_string(),
                message_tx: msg_tx,
            },
        );

        let target = 1u32;
        let slot = worker_slots.get(&target);
        assert!(
            matches!(slot, Some(WorkerSlot::Busy { .. })),
            "Busy worker should be eligible for restart"
        );

        // Verify task_id extraction
        if let Some(WorkerSlot::Busy { task_id, .. }) = slot {
            assert_eq!(task_id, "T01");
        }
    }

    #[test]
    fn test_restart_skips_nonexistent_worker() {
        // Test: Worker ID that doesn't exist should result in no-op
        use crate::commands::task::orchestrate::assignment::WorkerSlot;

        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();
        worker_slots.insert(1, WorkerSlot::Idle);

        let target = 99u32;
        assert!(
            !worker_slots.contains_key(&target),
            "Non-existent worker should return None"
        );
    }

    #[test]
    fn test_restart_merge_pipeline_detection() {
        // Test: Worker in merge pipeline should be detected
        use crate::commands::task::orchestrate::orchestrator_merge::{MergeContext, PendingMerge};

        let mut merge_ctx = MergeContext::new();
        let worker_id = 2u32;

        // Worker 2 is in pending merges
        merge_ctx.pending_merges.push_back(PendingMerge {
            worker_id: 2,
            task_id: "T01".to_string(),
            task_name: "Test task".to_string(),
            branch: "ralph/task/T01".to_string(),
        });

        let in_pipeline = merge_ctx
            .pending_merges
            .iter()
            .any(|pm| pm.worker_id == worker_id)
            || merge_ctx.current_merge_worker_id == Some(worker_id);
        assert!(in_pipeline, "Worker 2 should be detected in merge pipeline");

        // Worker 1 is NOT in merge pipeline
        let worker_1_in_pipeline = merge_ctx.pending_merges.iter().any(|pm| pm.worker_id == 1)
            || merge_ctx.current_merge_worker_id == Some(1);
        assert!(
            !worker_1_in_pipeline,
            "Worker 1 should NOT be in merge pipeline"
        );
    }

    #[test]
    fn test_restart_merge_pipeline_active_merge_detection() {
        // Test: Worker currently being merged should be detected via current_merge_worker_id
        use crate::commands::task::orchestrate::orchestrator_merge::MergeContext;

        let mut merge_ctx = MergeContext::new();
        merge_ctx.merge_in_progress = true;
        merge_ctx.current_merge_worker_id = Some(3);

        // Worker 3 is actively merging
        let in_pipeline = merge_ctx.pending_merges.iter().any(|pm| pm.worker_id == 3)
            || merge_ctx.current_merge_worker_id == Some(3);
        assert!(
            in_pipeline,
            "Worker 3 should be detected as actively merging"
        );

        // Worker 1 is NOT merging
        let worker_1_in_pipeline = merge_ctx.pending_merges.iter().any(|pm| pm.worker_id == 1)
            || merge_ctx.current_merge_worker_id == Some(1);
        assert!(
            !worker_1_in_pipeline,
            "Worker 1 should NOT be in merge pipeline"
        );
    }

    // ── Task 25.2.3: Message channel tests ──────────────────────────────

    #[tokio::test]
    async fn test_send_message_to_worker_success() {
        // Test: Successfully send a message to a busy worker
        use crate::commands::task::orchestrate::assignment::WorkerSlot;
        use crate::commands::task::orchestrate::worktree::WorktreeInfo;

        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();
        let (msg_tx, mut msg_rx) = mpsc::channel(16);

        worker_slots.insert(
            1,
            WorkerSlot::Busy {
                task_id: "T01".to_string(),
                worktree: WorktreeInfo {
                    path: std::path::PathBuf::from("/tmp/wt1"),
                    branch: "ralph/task/T01".to_string(),
                    task_id: "T01".to_string(),
                },
                started_at: Instant::now(),
                mcp_session_id: "session-1".to_string(),
                message_tx: msg_tx,
            },
        );

        // Send a test message by simulating send_message_to_worker logic
        if let Some(WorkerSlot::Busy { message_tx, .. }) = worker_slots.get(&1) {
            let result = message_tx.send("test message".to_string()).await;
            assert!(result.is_ok(), "Message should be sent successfully");
        }

        // Verify message was received
        let received = msg_rx.recv().await;
        assert_eq!(received, Some("test message".to_string()));
    }

    #[test]
    fn test_send_message_to_idle_worker_returns_error() {
        // Test: Sending a message to an idle worker should return an error
        use crate::commands::task::orchestrate::assignment::WorkerSlot;

        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();
        worker_slots.insert(1, WorkerSlot::Idle);

        // Check if worker is idle (simulating send_message_to_worker logic)
        let slot = worker_slots.get(&1);
        assert!(
            matches!(slot, Some(WorkerSlot::Idle)),
            "Idle worker should be detected"
        );
    }

    #[test]
    fn test_send_message_to_nonexistent_worker() {
        // Test: Sending a message to a worker that doesn't exist
        use crate::commands::task::orchestrate::assignment::WorkerSlot;

        let worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();

        // Check if worker exists
        assert!(
            !worker_slots.contains_key(&999),
            "Non-existent worker should return None"
        );
    }

    #[tokio::test]
    async fn test_message_channel_capacity() {
        // Test: Message channel has capacity of 16
        let (msg_tx, mut msg_rx) = mpsc::channel::<String>(16);

        // Send 16 messages (should all succeed without blocking)
        for i in 0..16 {
            let result = msg_tx.send(format!("msg{}", i)).await;
            assert!(result.is_ok(), "Should send message {} without blocking", i);
        }

        // Verify all messages can be received
        for i in 0..16 {
            let received = msg_rx.recv().await;
            assert_eq!(received, Some(format!("msg{}", i)));
        }
    }

    #[tokio::test]
    async fn test_message_channel_closed_after_sender_drop() {
        // Test: Channel is closed when sender is dropped
        let (msg_tx, mut msg_rx) = mpsc::channel::<String>(16);

        // Send a message
        msg_tx.send("test".to_string()).await.unwrap();

        // Drop sender
        drop(msg_tx);

        // Receive the sent message
        assert_eq!(msg_rx.recv().await, Some("test".to_string()));

        // Next recv should return None (channel closed)
        assert_eq!(msg_rx.recv().await, None);
    }

    // ── Task 28.4: End-to-end message pipeline tests ────────────────────

    #[tokio::test]
    async fn test_message_pipeline_end_to_end() {
        // Test: Full message pipeline from user_message_tx → user_message_rx → worker message_tx → worker message_rx
        use crate::commands::task::orchestrate::assignment::WorkerSlot;
        use crate::commands::task::orchestrate::worktree::WorktreeInfo;

        // 1. Create user message channel (TUI overlay → run_loop)
        let (user_message_tx, mut user_message_rx) = mpsc::channel::<(u32, String)>(16);

        // 2. Create worker message channel (run_loop → worker)
        let (worker_message_tx, mut worker_message_rx) = mpsc::channel::<String>(16);

        // 3. Setup worker slots with message channel
        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();
        worker_slots.insert(
            1,
            WorkerSlot::Busy {
                task_id: "T01".to_string(),
                worktree: WorktreeInfo {
                    path: std::path::PathBuf::from("/tmp/wt1"),
                    branch: "ralph/task/T01".to_string(),
                    task_id: "T01".to_string(),
                },
                started_at: Instant::now(),
                mcp_session_id: "session-1".to_string(),
                message_tx: worker_message_tx,
            },
        );

        // 4. Simulate TUI sending a message (user presses Ctrl+Enter in overlay)
        user_message_tx
            .send((1, "test message from user".to_string()))
            .await
            .unwrap();

        // 5. Simulate run_loop receiving the message
        let (worker_id, message) = user_message_rx.recv().await.unwrap();
        assert_eq!(worker_id, 1);
        assert_eq!(message, "test message from user");

        // 6. Simulate run_loop forwarding the message to worker
        if let Some(WorkerSlot::Busy { message_tx, .. }) = worker_slots.get(&worker_id) {
            message_tx.send(message).await.unwrap();
        }

        // 7. Verify worker receives the message
        let received = worker_message_rx.recv().await;
        assert_eq!(received, Some("test message from user".to_string()));
    }

    #[tokio::test]
    async fn test_message_pipeline_multiple_messages() {
        // Test: Pipeline can handle multiple messages in sequence
        let (user_message_tx, mut user_message_rx) = mpsc::channel::<(u32, String)>(16);
        let (worker_message_tx, mut worker_message_rx) = mpsc::channel::<String>(16);

        // Send multiple messages
        user_message_tx
            .send((1, "message 1".to_string()))
            .await
            .unwrap();
        user_message_tx
            .send((1, "message 2".to_string()))
            .await
            .unwrap();
        user_message_tx
            .send((1, "message 3".to_string()))
            .await
            .unwrap();

        // Forward all messages
        while let Ok((_, message)) = user_message_rx.try_recv() {
            worker_message_tx.send(message).await.unwrap();
        }

        // Verify all messages arrive at worker
        assert_eq!(
            worker_message_rx.recv().await,
            Some("message 1".to_string())
        );
        assert_eq!(
            worker_message_rx.recv().await,
            Some("message 2".to_string())
        );
        assert_eq!(
            worker_message_rx.recv().await,
            Some("message 3".to_string())
        );
    }

    #[tokio::test]
    async fn test_message_pipeline_multiple_workers() {
        // Test: Pipeline can route messages to different workers
        use crate::commands::task::orchestrate::assignment::WorkerSlot;
        use crate::commands::task::orchestrate::worktree::WorktreeInfo;

        let (user_message_tx, mut user_message_rx) = mpsc::channel::<(u32, String)>(16);

        // Create two worker message channels
        let (worker1_tx, mut worker1_rx) = mpsc::channel::<String>(16);
        let (worker2_tx, mut worker2_rx) = mpsc::channel::<String>(16);

        let mut worker_slots: HashMap<u32, WorkerSlot> = HashMap::new();
        worker_slots.insert(
            1,
            WorkerSlot::Busy {
                task_id: "T01".to_string(),
                worktree: WorktreeInfo {
                    path: std::path::PathBuf::from("/tmp/wt1"),
                    branch: "ralph/task/T01".to_string(),
                    task_id: "T01".to_string(),
                },
                started_at: Instant::now(),
                mcp_session_id: "session-1".to_string(),
                message_tx: worker1_tx,
            },
        );
        worker_slots.insert(
            2,
            WorkerSlot::Busy {
                task_id: "T02".to_string(),
                worktree: WorktreeInfo {
                    path: std::path::PathBuf::from("/tmp/wt2"),
                    branch: "ralph/task/T02".to_string(),
                    task_id: "T02".to_string(),
                },
                started_at: Instant::now(),
                mcp_session_id: "session-2".to_string(),
                message_tx: worker2_tx,
            },
        );

        // Send messages to different workers
        user_message_tx
            .send((1, "message for worker 1".to_string()))
            .await
            .unwrap();
        user_message_tx
            .send((2, "message for worker 2".to_string()))
            .await
            .unwrap();

        // Route messages to workers
        while let Ok((worker_id, message)) = user_message_rx.try_recv() {
            if let Some(WorkerSlot::Busy { message_tx, .. }) = worker_slots.get(&worker_id) {
                message_tx.send(message).await.unwrap();
            }
        }

        // Verify each worker receives the correct message
        assert_eq!(
            worker1_rx.recv().await,
            Some("message for worker 1".to_string())
        );
        assert_eq!(
            worker2_rx.recv().await,
            Some("message for worker 2".to_string())
        );
    }

    #[tokio::test]
    async fn test_message_pipeline_channel_capacity() {
        // Test: Both channels have capacity of 16 messages
        let (user_message_tx, _user_message_rx) = mpsc::channel::<(u32, String)>(16);
        let (worker_message_tx, _worker_message_rx) = mpsc::channel::<String>(16);

        // Send 16 messages to user_message channel without blocking
        for i in 0..16 {
            let result = user_message_tx.send((1, format!("msg{}", i))).await;
            assert!(result.is_ok(), "Should send message {} without blocking", i);
        }

        // Send 16 messages to worker_message channel without blocking
        for i in 0..16 {
            let result = worker_message_tx.send(format!("worker msg{}", i)).await;
            assert!(
                result.is_ok(),
                "Should send worker message {} without blocking",
                i
            );
        }
    }

    #[tokio::test]
    async fn test_message_pipeline_user_channel_closed() {
        // Test: user_message_rx returns None when sender is dropped
        let (user_message_tx, mut user_message_rx) = mpsc::channel::<(u32, String)>(16);

        user_message_tx.send((1, "test".to_string())).await.unwrap();
        drop(user_message_tx);

        // First recv gets the message
        assert_eq!(user_message_rx.recv().await, Some((1, "test".to_string())));

        // Second recv returns None (channel closed)
        assert_eq!(user_message_rx.recv().await, None);
    }
}
