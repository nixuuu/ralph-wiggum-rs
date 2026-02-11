use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::commands::task::orchestrate::dashboard::Dashboard;
use crate::commands::task::orchestrate::output::MultiplexedOutput;
use crate::commands::task::orchestrate::shared_types::{OrchestratorStatus, ShutdownState};
use crate::commands::task::orchestrate::summary::TaskSummaryEntry;
use crate::shared::error::Result;

use std::collections::HashMap;

use super::orchestrator::{Orchestrator, RunLoopContext};

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

        // Apply scroll delta from input thread
        let delta = ctx.flags.scroll_delta.swap(0, Ordering::Relaxed);
        let show_preview = ctx.flags.show_task_preview.load(Ordering::Relaxed);
        if delta != 0 {
            ctx.tui.dashboard.apply_scroll(delta, show_preview);
        }

        // Determine shutdown state and remaining time
        let (shutdown_state, shutdown_remaining) = if ctx.flags.shutdown.load(Ordering::Relaxed) {
            (ShutdownState::Aborting, None)
        } else if ctx.flags.graceful_shutdown.load(Ordering::Relaxed) {
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
        let orch_status = OrchestratorStatus {
            scheduler: ctx.scheduler.status(),
            total_cost: ctx.tui.mux_output.total_cost(),
            elapsed: started_at.elapsed(),
            shutdown_state,
            shutdown_remaining,
            quit_pending,
            completed,
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

        ctx.tui
            .dashboard
            .render(&orch_status, tasks_file, &ctx.tui.task_summaries)
    }
}
