use std::sync::atomic::Ordering;
use std::time::Instant;

use std::io::Stdout;

use crossterm::event::KeyEventKind;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::commands::task::orchestrate::app::{OrchestrateApp, RestartState};
use crate::commands::task::orchestrate::assignment::WorkerSlot;
use crate::commands::task::orchestrate::output::MultiplexedOutput;
use crate::commands::task::orchestrate::shutdown_types::{OrchestratorStatus, ShutdownState};
use crate::commands::task::orchestrate::summary::TaskSummaryEntry;
use crate::shared::error::Result;
use crate::tui::app::AppState;
use crate::tui::events::{AppEvent, EventResult, is_ctrl_c};
use crate::tui::keybindings::KeybindingResolver;

use std::collections::HashMap;

use super::orchestrator::Orchestrator;
use super::run_loop::RunLoopContext;

// ── TUI context ─────────────────────────────────────────────────────

/// Groups all TUI-related mutable state to keep function signatures clean.
pub(super) struct TuiContext {
    pub(super) app: OrchestrateApp,
    pub(super) terminal: Terminal<CrosstermBackend<Stdout>>,
    pub(super) mux_output: MultiplexedOutput,
    pub(super) task_start_times: HashMap<String, Instant>,
    pub(super) task_summaries: Vec<TaskSummaryEntry>,
    /// Resolver keybindingów — inicjalizowany raz przy starcie, reużywany per-event.
    /// Tworzone w orchestrator::run_tui_loop() z opcjonalną konfiguracją z .ralph.toml.
    pub(super) resolver: KeybindingResolver,
}

// ── TUI event handling ──────────────────────────────────────────────

impl Orchestrator {
    /// Handle a TUI event from the EventDispatcher.
    ///
    /// Routes key events to `OrchestrateApp::handle_event()`, then processes
    /// the result (quit, shutdown, user messages, reload, restart).
    /// Returns `true` if the loop should break (force shutdown).
    pub(super) fn handle_tui_event(
        &self,
        ctx: &mut RunLoopContext<'_>,
        event: AppEvent,
        started_at: Instant,
        graceful_shutdown_started: &mut Option<Instant>,
    ) -> Result<bool> {
        // Only process key press events (ignore Release/Repeat)
        if let AppEvent::Key(key) = &event {
            if key.kind != KeyEventKind::Press {
                return Ok(false);
            }

            // Ctrl+C bypasses OrchestrateApp — goes straight to shutdown logic
            if is_ctrl_c(key) {
                let should_break = self.handle_ctrl_c(ctx, graceful_shutdown_started);
                self.render_dashboard(ctx, started_at, *graceful_shutdown_started)?;
                return Ok(should_break);
            }
        }

        // TODO(11.4): zamienić hardcoded KeyCode checks w OrchestrateApp na resolver.resolve()
        let result = ctx.tui.app.handle_event(event, &ctx.tui.resolver);

        match result {
            EventResult::Quit => {
                // OrchestrateApp sets graceful_shutdown=true in its quit flow
                if ctx.tui.app.is_graceful_shutdown() {
                    ctx.flags.graceful_shutdown.store(true, Ordering::SeqCst);
                    if graceful_shutdown_started.is_none() {
                        *graceful_shutdown_started = Some(Instant::now());
                    }
                    let msg = MultiplexedOutput::format_orchestrator_line(
                        "Graceful shutdown — waiting for in-progress tasks...",
                    );
                    ctx.tui.app.push_log_line(&msg);

                    // If no busy workers, break immediately
                    let any_busy = ctx
                        .worker_slots
                        .values()
                        .any(|s| matches!(s, WorkerSlot::Busy { .. }));
                    if !any_busy {
                        self.render_dashboard(ctx, started_at, *graceful_shutdown_started)?;
                        return Ok(true);
                    }
                }
            }
            EventResult::Shutdown => {
                // Force shutdown
                let msg = MultiplexedOutput::format_orchestrator_line(
                    "Force shutdown — aborting all workers",
                );
                ctx.tui.app.push_log_line(&msg);
                ctx.flags.shutdown.store(true, Ordering::SeqCst);
                for (_, handle) in ctx.join_handles.drain() {
                    handle.abort();
                }
                self.render_dashboard(ctx, started_at, *graceful_shutdown_started)?;
                return Ok(true);
            }
            EventResult::Consumed | EventResult::Ignored => {}
        }

        // Check reload requested (klawisz 'r')
        if ctx.tui.app.take_reload_requested() {
            self.handle_manual_reload(ctx);
        }

        // Check restart request from OrchestrateApp
        if let RestartState::Confirmed { .. } = ctx.tui.app.restart_state() {
            self.handle_restart_request(ctx)?;
        }

        self.render_dashboard(ctx, started_at, *graceful_shutdown_started)?;
        Ok(false)
    }
}

// ── Dashboard rendering ─────────────────────────────────────────────

impl Orchestrator {
    /// Render the dashboard using OrchestrateApp state directly (no InputFlags).
    pub(super) fn render_dashboard(
        &self,
        ctx: &mut RunLoopContext<'_>,
        started_at: Instant,
        graceful_shutdown_started: Option<Instant>,
    ) -> Result<()> {
        // Auto-shift focus to active worker if current focus is idle
        ctx.tui.app.auto_focus_active();

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

        // Build status snapshot — read quit/restart state from OrchestrateApp
        let quit_pending =
            ctx.tui.app.quit_state == crate::commands::task::orchestrate::app::QuitState::Pending;
        let completed = ctx.flags.completed.load(Ordering::Relaxed);

        // Check for pending worker restart from app state
        let restart_pending = match ctx.tui.app.restart_state() {
            RestartState::Pending { worker_id } => {
                let wid = *worker_id;
                ctx.worker_slots.get(&wid).and_then(|slot| match slot {
                    WorkerSlot::Busy { task_id, .. } => Some((wid, task_id.clone())),
                    _ => None,
                })
            }
            _ => None,
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

        // Update app state with latest status
        ctx.tui.app.update_status(orch_status);

        // Update tasks file for preview overlay and sidebar
        if ctx.tui.app.show_task_preview || ctx.tui.app.sidebar_state.visible {
            ctx.tui
                .app
                .update_tasks_file(Some((*ctx.cached_tasks_file).clone()));
        } else {
            ctx.tui.app.update_tasks_file(None);
        }

        // Render using terminal.draw() + app.draw()
        ctx.tui.terminal.draw(|frame| {
            let area = frame.area();
            ctx.tui.app.draw(frame, area);
        })?;

        Ok(())
    }
}
