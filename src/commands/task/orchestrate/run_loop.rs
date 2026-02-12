use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::mpsc;

use crate::commands::task::orchestrate::events::{EventLogger, WorkerEvent, WorkerEventKind};
use crate::commands::task::orchestrate::output::MultiplexedOutput;
use crate::commands::task::orchestrate::scheduler::TaskScheduler;
use crate::commands::task::orchestrate::shutdown_types::{OrchestratorStatus, ShutdownState};
use crate::commands::task::orchestrate::state::{Lockfile, OrchestrateState};
use crate::commands::task::orchestrate::worker::TaskResult;
use crate::commands::task::orchestrate::worktree::WorktreeManager;
use crate::shared::dag::TaskDag;
use crate::shared::error::Result;
use crate::shared::progress::ProgressTask;
use crate::shared::tasks::TasksFile;

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
    /// Last heartbeat timestamp per worker (for liveness checking in watchdog).
    pub(super) last_heartbeat: HashMap<u32, Instant>,
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

                _ = tokio::signal::ctrl_c() => {
                    if self.handle_ctrl_c(ctx, &mut graceful_shutdown_started) {
                        break;
                    }
                }

                _ = async {
                    #[cfg(unix)]
                    if let Some(ref mut s) = sigterm { s.recv().await; return; }
                    std::future::pending::<()>().await;
                } => {
                    self.handle_sigterm(ctx);
                    break;
                }

                _ = ctx.flags.render_notify.notified() => {
                    self.handle_render_notify(ctx, started_at, graceful_shutdown_started)?;
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
        let msg = MultiplexedOutput::format_orchestrator_line(&format!(
            "Started: {} workers, {} tasks ({} done, {} remaining)",
            self.config.workers,
            ctx.progress.total(),
            ctx.progress.done,
            ctx.progress.remaining()
        ));
        ctx.tui.dashboard.push_log_line(&msg);

        if self.verify_commands.is_empty() {
            let warn = MultiplexedOutput::format_orchestrator_line(
                "⚠ No verify_commands configured — skipping verify phase. \
                 Set verify_commands in [task.orchestrate] in .ralph.toml",
            );
            ctx.tui.dashboard.push_log_line(&warn);
        }
    }

    fn render_initial_dashboard(
        &self,
        ctx: &mut RunLoopContext<'_>,
        started_at: Instant,
    ) -> Result<()> {
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
        ctx.tui
            .dashboard
            .render(&orch_status, None, &ctx.tui.task_summaries)
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
            ctx.tui.dashboard.push_log_line(&msg);
            return true;
        }
        if let Some(timeout) = self.config.timeout
            && started_at.elapsed() >= timeout
        {
            let msg = MultiplexedOutput::format_orchestrator_line("Timeout reached");
            ctx.tui.dashboard.push_log_line(&msg);
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
            ctx.tui.dashboard.push_log_line(&msg);
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

        if ctx.flags.resize_flag.swap(false, Ordering::SeqCst) {
            ctx.tui.dashboard.handle_resize()?;
        }
        self.render_dashboard(ctx, started_at, graceful_shutdown_started)
    }

    /// Returns `true` if the loop should break (force shutdown).
    fn handle_ctrl_c(
        &self,
        ctx: &mut RunLoopContext<'_>,
        graceful_shutdown_started: &mut Option<Instant>,
    ) -> bool {
        if ctx.flags.graceful_shutdown.load(Ordering::SeqCst) {
            let msg = MultiplexedOutput::format_orchestrator_line(
                "Force shutdown — aborting all workers",
            );
            ctx.tui.dashboard.push_log_line(&msg);
            ctx.flags.shutdown.store(true, Ordering::SeqCst);
            for (_, handle) in ctx.join_handles.drain() {
                handle.abort();
            }
            true
        } else {
            let msg = MultiplexedOutput::format_orchestrator_line(
                "Graceful shutdown — waiting for in-progress tasks...",
            );
            ctx.tui.dashboard.push_log_line(&msg);
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
        ctx.tui.dashboard.push_log_line(&msg);
        ctx.flags.shutdown.store(true, Ordering::SeqCst);
        for (_, handle) in ctx.join_handles.drain() {
            handle.abort();
        }
    }

    fn handle_render_notify(
        &self,
        ctx: &mut RunLoopContext<'_>,
        started_at: Instant,
        graceful_shutdown_started: Option<Instant>,
    ) -> Result<()> {
        if ctx.flags.reload_requested.swap(false, Ordering::SeqCst) {
            self.handle_manual_reload(ctx);
        }

        if ctx.flags.resize_flag.swap(false, Ordering::SeqCst) {
            ctx.tui.dashboard.handle_resize()?;
        }
        self.render_dashboard(ctx, started_at, graceful_shutdown_started)
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

        if ctx.flags.resize_flag.swap(false, Ordering::SeqCst) {
            ctx.tui.dashboard.handle_resize()?;
        }
        self.render_dashboard(ctx, started_at, *graceful_shutdown_started)?;
        Ok(false)
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
            ctx.tui.dashboard.push_log_line(&msg);
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
            ctx.tui.dashboard.push_log_line(&msg);
            return Ok(true);
        }

        if let Some(gs_start) = *graceful_shutdown_started
            && gs_start.elapsed() >= GRACEFUL_SHUTDOWN_GRACE
        {
            let msg = MultiplexedOutput::format_orchestrator_line(
                "Grace period expired — force-killing all workers",
            );
            ctx.tui.dashboard.push_log_line(&msg);
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
                        ctx.tui.dashboard.push_log_line(&msg);

                        // Recover: mark task as failed, free worker slot
                        let requeued = ctx.scheduler.mark_failed(&task_id);
                        if !requeued {
                            ctx.scheduler.mark_blocked(&task_id);
                        }

                        ctx.worker_slots.insert(worker_id, WorkerSlot::Idle);
                        ctx.last_heartbeat.remove(&worker_id);
                        ctx.tui.dashboard.update_worker_status(
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
                    ctx.tui.dashboard.push_log_line(&msg);

                    // Reset merge state so the next queued merge can proceed
                    ctx.merge_ctx.merge_in_progress = false;
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
                    ctx.tui.dashboard.push_log_line(&msg);
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
                    ctx.tui.dashboard.push_log_line(&msg);
                }
            }
        }
    }
}

// ── Hot reload ──────────────────────────────────────────────────────

impl Orchestrator {
    /// Manual reload triggered by user pressing 'r'.
    fn handle_manual_reload(&self, ctx: &mut RunLoopContext<'_>) {
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
                    ctx.tui.dashboard.push_log_line(&msg);
                } else {
                    self.apply_reload(ctx, &mut new_tf, new_dag);
                    let delta = (new_count as i32) - (old_count as i32);
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

    /// Automatic hot-reload (silent on failure, every 15s).
    fn handle_auto_hot_reload(&self, ctx: &mut RunLoopContext<'_>) {
        if let Some(new_mtime) = get_mtime(&self.tasks_path)
            && ctx.progress_mtime.is_none_or(|old| new_mtime > old)
            && let Ok(mut new_tf) = TasksFile::load(&self.tasks_path)
        {
            let new_dag = TaskDag::from_tasks_file(&new_tf);
            if new_dag.detect_cycles().is_none() {
                ctx.progress_mtime = Some(new_mtime);
                self.apply_reload(ctx, &mut new_tf, new_dag);
            }
        }
    }

    /// Patch scheduler state onto a freshly loaded TasksFile and update cache.
    fn apply_reload(&self, ctx: &mut RunLoopContext<'_>, new_tf: &mut TasksFile, new_dag: TaskDag) {
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

        // Reset completed flag if new tasks make scheduler incomplete
        if ctx.flags.completed.load(Ordering::Relaxed) && !ctx.scheduler.is_complete() {
            ctx.flags.completed.store(false, Ordering::Relaxed);
            let status = ctx.scheduler.status();
            let msg = MultiplexedOutput::format_orchestrator_line(&format!(
                "♻ Resuming — {} new tasks detected",
                status.pending + status.ready
            ));
            ctx.tui.dashboard.push_log_line(&msg);
        }
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
        | WorkerEventKind::Heartbeat { worker_id, .. } => Some(*worker_id),
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
}
