use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::commands::task::orchestrate::assignment::WorkerSlot;
use crate::commands::task::orchestrate::dashboard::Dashboard;
use crate::commands::task::orchestrate::output::MultiplexedOutput;
use crate::commands::task::orchestrate::shutdown_types::{OrchestratorStatus, ShutdownState};
use crate::commands::task::orchestrate::summary::TaskSummaryEntry;
use crate::shared::error::Result;

use std::collections::HashMap;

use super::orchestrator::Orchestrator;
use super::run_loop::RunLoopContext;

// ── TUI context ─────────────────────────────────────────────────────

/// Groups all TUI-related mutable state to keep function signatures clean.
pub(super) struct TuiContext {
    pub(super) dashboard: Dashboard,
    pub(super) mux_output: MultiplexedOutput,
    pub(super) task_start_times: HashMap<String, Instant>,
    pub(super) task_summaries: Vec<TaskSummaryEntry>,
}

// ── Dashboard rendering ─────────────────────────────────────────────

impl Orchestrator {
    /// Apply focus/scroll from input thread and render the dashboard.
    pub(super) fn render_dashboard(
        &self,
        ctx: &mut RunLoopContext<'_>,
        started_at: Instant,
        graceful_shutdown_started: Option<Instant>,
    ) -> Result<()> {
        // Apply focus from input thread
        let focus = ctx.flags.focused_worker.load(Ordering::Relaxed);
        ctx.tui
            .dashboard
            .set_focus(if focus == 0 { None } else { Some(focus) });

        // Auto-shift focus to active worker if current focus is idle
        if let Some(new_focus) = ctx.tui.dashboard.auto_focus_active() {
            // Sync the new focus back to the atomic flag
            ctx.flags.focused_worker.store(new_focus, Ordering::Relaxed);
        }

        // Apply scroll delta from input thread
        let delta = ctx.flags.scroll_delta.swap(0, Ordering::Relaxed);
        let show_preview = ctx.flags.show_task_preview.load(Ordering::Relaxed);
        if delta != 0 {
            ctx.tui.dashboard.apply_scroll(delta, show_preview);
        }

        // Determine shutdown state and remaining time
        let (shutdown_state, shutdown_remaining) = if ctx.flags.shutdown.load(Ordering::SeqCst) {
            (ShutdownState::Aborting, None)
        } else if ctx.flags.graceful_shutdown.load(Ordering::SeqCst) {
            let remaining = graceful_shutdown_started.map(|start| {
                const GRACE: std::time::Duration = std::time::Duration::from_secs(120);
                GRACE.saturating_sub(start.elapsed())
            });
            (ShutdownState::Draining, remaining)
        } else {
            (ShutdownState::Running, None)
        };

        // Build status snapshot and render
        let quit_pending = ctx.flags.quit_state.load(Ordering::Relaxed) == 1;
        let completed = ctx.flags.completed.load(Ordering::Relaxed);

        // Check for pending worker restart
        let restart_worker_id = ctx.flags.restart_worker_id.load(Ordering::Relaxed);
        let restart_pending = if restart_worker_id > 0 {
            // Find task_id from worker_slots
            ctx.worker_slots
                .get(&restart_worker_id)
                .and_then(|slot| match slot {
                    WorkerSlot::Busy { task_id, .. } => Some((restart_worker_id, task_id.clone())),
                    _ => None,
                })
        } else {
            None
        };

        // Count active and idle workers
        let (active_workers, idle_workers) =
            ctx.worker_slots
                .iter()
                .fold((0u32, 0u32), |(active, idle), (_, slot)| match slot {
                    WorkerSlot::Busy { .. } => (active + 1, idle),
                    WorkerSlot::Idle => (active, idle + 1),
                });

        let orch_status = OrchestratorStatus {
            scheduler: ctx.scheduler.status(),
            total_cost: ctx.tui.mux_output.total_cost(),
            elapsed: started_at.elapsed(),
            shutdown_state,
            shutdown_remaining,
            quit_pending,
            completed,
            restart_pending,
            active_workers,
            idle_workers,
        };

        // Pass TasksFile to dashboard if preview overlay is active
        let tasks_file = if ctx
            .flags
            .show_task_preview
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            Some(ctx.cached_tasks_file.as_ref())
        } else {
            None
        };

        ctx.tui.dashboard.render(
            &orch_status,
            tasks_file,
            &ctx.tui.task_summaries,
            Some(&ctx.flags.active_worker_ids),
        )
    }
}
