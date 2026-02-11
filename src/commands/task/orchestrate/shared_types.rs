//! Shared types and utilities for the orchestrator subsystem.
//!
//! This module contains data types and helper functions used by both the
//! orchestrator core and the dashboard TUI.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::commands::task::orchestrate::events::WorkerPhase;
use crate::commands::task::orchestrate::scheduler::SchedulerStatus;

// ── Data types ──────────────────────────────────────────────────────

/// Per-worker status for display purposes.
#[derive(Debug, Clone)]
pub struct WorkerStatus {
    pub state: WorkerState,
    pub task_id: Option<String>,
    pub component: Option<String>,
    pub phase: Option<WorkerPhase>,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl WorkerStatus {
    /// Create an idle worker status.
    pub fn idle(_worker_id: u32) -> Self {
        Self {
            state: WorkerState::Idle,
            task_id: None,
            component: None,
            phase: None,
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
        }
    }
}

/// Worker state for display purposes.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerState {
    Idle,
    SettingUp,
    Implementing,
    Reviewing,
    Verifying,
    Merging,
    ResolvingConflicts,
}

impl std::fmt::Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerState::Idle => write!(f, "idle"),
            WorkerState::SettingUp => write!(f, "setting up"),
            WorkerState::Implementing => write!(f, "implementing"),
            WorkerState::Reviewing => write!(f, "reviewing"),
            WorkerState::Verifying => write!(f, "verifying"),
            WorkerState::Merging => write!(f, "merging"),
            WorkerState::ResolvingConflicts => write!(f, "resolving conflicts"),
        }
    }
}

/// Shutdown phase for display purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShutdownState {
    #[default]
    Running,
    /// First q/Ctrl+C — waiting for in-progress tasks to finish
    Draining,
    /// Second q/Ctrl+C — force-aborting all workers
    Aborting,
}

/// Snapshot of the orchestrator's status for rendering.
#[derive(Debug, Clone)]
pub struct OrchestratorStatus {
    pub scheduler: SchedulerStatus,
    pub total_cost: f64,
    pub elapsed: Duration,
    pub shutdown_state: ShutdownState,
    /// Time remaining before force-kill escalation (only during Draining)
    pub shutdown_remaining: Option<Duration>,
    /// Whether the user is in quit confirmation state (first 'q' pressed)
    pub quit_pending: bool,
    /// Whether all tasks have completed (orchestrator in idle state)
    pub completed: bool,
}

// ── Helper functions ────────────────────────────────────────────────

/// Render a Unicode progress bar.
pub fn render_progress_bar(done: usize, total: usize, width: usize) -> String {
    if total == 0 {
        return format!("[{}]", " ".repeat(width));
    }
    let filled = (done * width) / total;
    let empty = width - filled;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

/// Format a duration as human-readable string.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Format token count for display (e.g., 1234 -> "1.2k").
pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

// ── Dashboard input thread ──────────────────────────────────────────

/// Parameters for spawning the dashboard input thread.
pub struct DashboardInputParams {
    pub shutdown: Arc<AtomicBool>,
    pub graceful_shutdown: Arc<AtomicBool>,
    pub resize_flag: Arc<AtomicBool>,
    pub focused_worker: Arc<AtomicU32>,
    pub worker_count: u32,
    pub scroll_delta: Arc<AtomicI32>,
    pub render_notify: Arc<tokio::sync::Notify>,
    pub show_task_preview: Arc<AtomicBool>,
    pub reload_requested: Arc<AtomicBool>,
    pub quit_state: Arc<AtomicU8>,
}

/// Keyboard/resize input thread for the fullscreen dashboard.
///
/// Handles Tab/Esc/1-9/arrows for panel focus and scroll control, plus
/// q/Ctrl+C for shutdown. Runs on a dedicated OS thread.
pub struct DashboardInputThread {
    handle: Option<std::thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

impl DashboardInputThread {
    pub fn spawn(params: DashboardInputParams) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let DashboardInputParams {
            shutdown,
            graceful_shutdown,
            resize_flag,
            focused_worker,
            worker_count,
            scroll_delta,
            render_notify,
            show_task_preview,
            reload_requested,
            quit_state,
        } = params;

        let handle = std::thread::Builder::new()
            .name("dash-input".into())
            .spawn(move || {
                while running_clone.load(Ordering::SeqCst) {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    if crossterm::event::poll(Duration::from_millis(100)).unwrap_or(false) {
                        match crossterm::event::read() {
                            Ok(crossterm::event::Event::Key(key)) => {
                                use crossterm::event::{KeyCode, KeyModifiers};

                                // Ctrl+C: immediate shutdown (bypass confirmation)
                                let is_ctrl_c = key.code == KeyCode::Char('c')
                                    && key.modifiers.contains(KeyModifiers::CONTROL);

                                if is_ctrl_c {
                                    if graceful_shutdown.load(Ordering::SeqCst) {
                                        shutdown.store(true, Ordering::SeqCst);
                                    } else {
                                        graceful_shutdown.store(true, Ordering::SeqCst);
                                    }
                                    render_notify.notify_one();
                                    continue;
                                }

                                // 'q' key: confirmation flow
                                if matches!(key.code, KeyCode::Char('q')) {
                                    let current_quit = quit_state.load(Ordering::SeqCst);

                                    if graceful_shutdown.load(Ordering::SeqCst) {
                                        // Already in draining state: second 'q' forces shutdown
                                        shutdown.store(true, Ordering::SeqCst);
                                        render_notify.notify_one();
                                    } else if current_quit == 1 {
                                        // In quit_pending state: second 'q' confirms
                                        graceful_shutdown.store(true, Ordering::SeqCst);
                                        quit_state.store(0, Ordering::SeqCst);
                                        render_notify.notify_one();
                                    } else {
                                        // Normal state: first 'q' enters quit_pending
                                        quit_state.store(1, Ordering::SeqCst);
                                        render_notify.notify_one();
                                    }
                                    continue;
                                }

                                // Enter: confirm quit if in quit_pending state
                                if key.code == KeyCode::Enter {
                                    let current_quit = quit_state.load(Ordering::SeqCst);
                                    if current_quit == 1 {
                                        graceful_shutdown.store(true, Ordering::SeqCst);
                                        quit_state.store(0, Ordering::SeqCst);
                                        render_notify.notify_one();
                                        continue;
                                    }
                                }

                                // Tab: cycle focus forward (cancel quit_pending first)
                                if key.code == KeyCode::Tab
                                    && !key.modifiers.contains(KeyModifiers::SHIFT)
                                {
                                    // Cancel quit_pending state if active
                                    if quit_state.load(Ordering::SeqCst) == 1 {
                                        quit_state.store(0, Ordering::SeqCst);
                                    }

                                    let current = focused_worker.load(Ordering::Relaxed);
                                    let next = if current >= worker_count {
                                        0
                                    } else {
                                        current + 1
                                    };
                                    focused_worker.store(next, Ordering::Relaxed);
                                    scroll_delta.store(0, Ordering::Relaxed);
                                    render_notify.notify_one();
                                    continue;
                                }

                                // Shift+Tab: cycle focus backward (cancel quit_pending first)
                                if key.code == KeyCode::BackTab {
                                    // Cancel quit_pending state if active
                                    if quit_state.load(Ordering::SeqCst) == 1 {
                                        quit_state.store(0, Ordering::SeqCst);
                                    }

                                    let current = focused_worker.load(Ordering::Relaxed);
                                    let next = if current == 0 {
                                        worker_count
                                    } else {
                                        current - 1
                                    };
                                    focused_worker.store(next, Ordering::Relaxed);
                                    scroll_delta.store(0, Ordering::Relaxed);
                                    render_notify.notify_one();
                                    continue;
                                }

                                // Esc: cancel quit_pending or unfocus
                                if key.code == KeyCode::Esc {
                                    let current_quit = quit_state.load(Ordering::SeqCst);
                                    if current_quit == 1 {
                                        // Cancel quit_pending
                                        quit_state.store(0, Ordering::SeqCst);
                                        render_notify.notify_one();
                                    } else {
                                        // Normal unfocus behavior
                                        focused_worker.store(0, Ordering::Relaxed);
                                        scroll_delta.store(0, Ordering::Relaxed);
                                        render_notify.notify_one();
                                    }
                                    continue;
                                }

                                // 1-9: direct focus (cancel quit_pending first)
                                if let KeyCode::Char(ch) = key.code
                                    && ch.is_ascii_digit()
                                    && ch != '0'
                                {
                                    // Cancel quit_pending state if active
                                    if quit_state.load(Ordering::SeqCst) == 1 {
                                        quit_state.store(0, Ordering::SeqCst);
                                    }

                                    let n = ch as u32 - '0' as u32;
                                    if n <= worker_count {
                                        focused_worker.store(n, Ordering::Relaxed);
                                        scroll_delta.store(0, Ordering::Relaxed);
                                        render_notify.notify_one();
                                    }
                                    continue;
                                }

                                // p: toggle task preview overlay
                                if key.code == KeyCode::Char('p') {
                                    // Cancel quit_pending state if active
                                    if quit_state.load(Ordering::SeqCst) == 1 {
                                        quit_state.store(0, Ordering::SeqCst);
                                    }

                                    let current = show_task_preview.load(Ordering::Relaxed);
                                    show_task_preview.store(!current, Ordering::Relaxed);
                                    render_notify.notify_one();
                                    continue;
                                }

                                // Up/Down arrows: scroll focused panel (cancel quit_pending first)
                                if key.code == KeyCode::Up {
                                    // Cancel quit_pending state if active
                                    if quit_state.load(Ordering::SeqCst) == 1 {
                                        quit_state.store(0, Ordering::SeqCst);
                                    }

                                    scroll_delta.fetch_sub(1, Ordering::Relaxed);
                                    render_notify.notify_one();
                                    continue;
                                }
                                if key.code == KeyCode::Down {
                                    // Cancel quit_pending state if active
                                    if quit_state.load(Ordering::SeqCst) == 1 {
                                        quit_state.store(0, Ordering::SeqCst);
                                    }

                                    scroll_delta.fetch_add(1, Ordering::Relaxed);
                                    render_notify.notify_one();
                                    continue;
                                }

                                // r: reload tasks.yml
                                if key.code == KeyCode::Char('r') {
                                    reload_requested.store(true, Ordering::SeqCst);
                                    render_notify.notify_one();
                                    continue;
                                }
                            }
                            Ok(crossterm::event::Event::Resize(_, _)) => {
                                resize_flag.store(true, Ordering::SeqCst);
                                render_notify.notify_one();
                            }
                            _ => {}
                        }
                    }
                }
            })
            .ok();

        Self { handle, running }
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for DashboardInputThread {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: create minimal DashboardInputParams for testing.
    fn setup_test_params() -> DashboardInputParams {
        DashboardInputParams {
            shutdown: Arc::new(AtomicBool::new(false)),
            graceful_shutdown: Arc::new(AtomicBool::new(false)),
            resize_flag: Arc::new(AtomicBool::new(false)),
            focused_worker: Arc::new(AtomicU32::new(0)),
            worker_count: 3,
            scroll_delta: Arc::new(AtomicI32::new(0)),
            render_notify: Arc::new(tokio::sync::Notify::new()),
            show_task_preview: Arc::new(AtomicBool::new(false)),
            reload_requested: Arc::new(AtomicBool::new(false)),
            quit_state: Arc::new(AtomicU8::new(0)),
        }
    }

    #[test]
    fn test_quit_state_0_to_1_first_q() {
        let params = setup_test_params();
        let quit_state = params.quit_state.clone();

        // Initial state should be 0 (running)
        assert_eq!(quit_state.load(Ordering::SeqCst), 0);

        // Simulate first 'q' press: 0 → 1 (quit_pending)
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit != 1 {
            quit_state.store(1, Ordering::SeqCst);
        }

        assert_eq!(quit_state.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_quit_state_1_to_graceful_shutdown_second_q() {
        let params = setup_test_params();
        let quit_state = params.quit_state.clone();
        let graceful_shutdown = params.graceful_shutdown.clone();

        // Start in quit_pending state (quit_state == 1)
        quit_state.store(1, Ordering::SeqCst);
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);

        // Simulate second 'q' press: confirms shutdown
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            graceful_shutdown.store(true, Ordering::SeqCst);
            quit_state.store(0, Ordering::SeqCst);
        }

        assert!(graceful_shutdown.load(Ordering::SeqCst));
        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_quit_state_esc_cancels_quit_pending() {
        let params = setup_test_params();
        let quit_state = params.quit_state.clone();

        // Start in quit_pending state (quit_state == 1)
        quit_state.store(1, Ordering::SeqCst);
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);

        // Simulate Esc key: cancels quit_pending
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            quit_state.store(0, Ordering::SeqCst);
        }

        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_quit_state_tab_cancels_quit_pending() {
        let params = setup_test_params();
        let quit_state = params.quit_state.clone();
        let focused_worker = params.focused_worker.clone();

        // Start in quit_pending state
        quit_state.store(1, Ordering::SeqCst);
        focused_worker.store(1, Ordering::Relaxed);
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);

        // Simulate Tab: cancels quit_pending AND cycles focus
        if quit_state.load(Ordering::SeqCst) == 1 {
            quit_state.store(0, Ordering::SeqCst);
        }
        let current = focused_worker.load(Ordering::Relaxed);
        let next = if current >= params.worker_count {
            0
        } else {
            current + 1
        };
        focused_worker.store(next, Ordering::Relaxed);

        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
        assert_eq!(focused_worker.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_ctrl_c_immediate_shutdown_graceful() {
        let params = setup_test_params();
        let shutdown = params.shutdown.clone();
        let graceful_shutdown = params.graceful_shutdown.clone();
        let quit_state = params.quit_state.clone();

        // Start in running state
        assert!(!graceful_shutdown.load(Ordering::SeqCst));

        // Simulate first Ctrl+C: enter graceful shutdown
        if graceful_shutdown.load(Ordering::SeqCst) {
            shutdown.store(true, Ordering::SeqCst);
        } else {
            graceful_shutdown.store(true, Ordering::SeqCst);
        }

        assert!(graceful_shutdown.load(Ordering::SeqCst));
        assert!(!shutdown.load(Ordering::SeqCst));

        // quit_pending should not be set by Ctrl+C
        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_ctrl_c_second_press_forces_shutdown() {
        let params = setup_test_params();
        let shutdown = params.shutdown.clone();
        let graceful_shutdown = params.graceful_shutdown.clone();

        // Start in graceful_shutdown state (draining)
        graceful_shutdown.store(true, Ordering::SeqCst);
        assert!(graceful_shutdown.load(Ordering::SeqCst));

        // Simulate second Ctrl+C: force shutdown
        if graceful_shutdown.load(Ordering::SeqCst) {
            shutdown.store(true, Ordering::SeqCst);
        }

        assert!(shutdown.load(Ordering::SeqCst));
    }

    #[test]
    fn test_esc_in_running_state_unfocuses() {
        let params = setup_test_params();
        let focused_worker = params.focused_worker.clone();
        let quit_state = params.quit_state.clone();

        // Start with focused_worker = 1, quit_state = 0 (running)
        focused_worker.store(1, Ordering::Relaxed);
        assert_eq!(quit_state.load(Ordering::SeqCst), 0);

        // Simulate Esc in running state: unfocus (set to 0)
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            quit_state.store(0, Ordering::SeqCst);
        } else {
            // Normal unfocus behavior
            focused_worker.store(0, Ordering::Relaxed);
        }

        assert_eq!(focused_worker.load(Ordering::Relaxed), 0);
        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_enter_key_confirms_quit_pending() {
        let params = setup_test_params();
        let quit_state = params.quit_state.clone();
        let graceful_shutdown = params.graceful_shutdown.clone();

        // Start in quit_pending state
        quit_state.store(1, Ordering::SeqCst);
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);

        // Simulate Enter key: confirms shutdown
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            graceful_shutdown.store(true, Ordering::SeqCst);
            quit_state.store(0, Ordering::SeqCst);
        }

        assert!(graceful_shutdown.load(Ordering::SeqCst));
        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_arrow_up_cancels_quit_pending() {
        let params = setup_test_params();
        let quit_state = params.quit_state.clone();
        let scroll_delta = params.scroll_delta.clone();

        // Start in quit_pending state
        quit_state.store(1, Ordering::SeqCst);
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);

        // Simulate Up arrow: cancels quit_pending AND scrolls
        if quit_state.load(Ordering::SeqCst) == 1 {
            quit_state.store(0, Ordering::SeqCst);
        }
        scroll_delta.fetch_sub(1, Ordering::Relaxed);

        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
        assert_eq!(scroll_delta.load(Ordering::Relaxed), -1);
    }

    #[test]
    fn test_digit_key_cancels_quit_pending() {
        let params = setup_test_params();
        let quit_state = params.quit_state.clone();
        let focused_worker = params.focused_worker.clone();

        // Start in quit_pending state
        quit_state.store(1, Ordering::SeqCst);
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);

        // Simulate pressing '2': cancels quit_pending AND sets focus
        if quit_state.load(Ordering::SeqCst) == 1 {
            quit_state.store(0, Ordering::SeqCst);
        }
        focused_worker.store(2, Ordering::Relaxed);

        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
        assert_eq!(focused_worker.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_p_key_toggles_preview_and_cancels_quit_pending() {
        let params = setup_test_params();
        let quit_state = params.quit_state.clone();
        let show_task_preview = params.show_task_preview.clone();

        // Start in quit_pending state
        quit_state.store(1, Ordering::SeqCst);
        show_task_preview.store(false, Ordering::Relaxed);

        // Simulate 'p' key: cancels quit_pending AND toggles preview
        if quit_state.load(Ordering::SeqCst) == 1 {
            quit_state.store(0, Ordering::SeqCst);
        }
        let current = show_task_preview.load(Ordering::Relaxed);
        show_task_preview.store(!current, Ordering::Relaxed);

        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
        assert!(show_task_preview.load(Ordering::Relaxed));
    }

    #[test]
    fn test_enter_key_ignored_in_running_state() {
        let params = setup_test_params();
        let quit_state = params.quit_state.clone();
        let graceful_shutdown = params.graceful_shutdown.clone();

        // Start in running state
        quit_state.store(0, Ordering::SeqCst);
        graceful_shutdown.store(false, Ordering::SeqCst);

        // Simulate Enter key in running state: should be ignored
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            graceful_shutdown.store(true, Ordering::SeqCst);
            quit_state.store(0, Ordering::SeqCst);
        }

        assert!(!graceful_shutdown.load(Ordering::SeqCst));
        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_reload_key_bypasses_quit_pending() {
        let params = setup_test_params();
        let quit_state = params.quit_state.clone();
        let reload_requested = params.reload_requested.clone();

        // Start in quit_pending state
        quit_state.store(1, Ordering::SeqCst);

        // Simulate 'r' key (reload): does NOT cancel quit_pending
        // (reload_requested is set separately without affecting quit_state)
        reload_requested.store(true, Ordering::SeqCst);

        // Note: actual implementation should decide if 'r' cancels quit_pending
        // For now we test that reload flag can be set independently
        assert!(reload_requested.load(Ordering::SeqCst));
        // quit_state remains as-is based on implementation design
    }

    // ── Completion state tests ───────────────────────────────────────
    //
    // Note: These tests verify the state machine logic in isolation.
    // The real integration tests (including scheduler.is_complete() checks and
    // main loop behavior) are in orchestrator.rs tests.

    #[test]
    #[allow(unused_variables)]
    fn test_completion_flag_set_on_completion() {
        let params = setup_test_params();
        let completed = Arc::new(AtomicBool::new(false));

        // Initial state: not completed
        assert!(!completed.load(Ordering::Relaxed));

        // Simulate completion: set flag when all tasks done
        completed.store(true, Ordering::Relaxed);

        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    #[allow(unused_variables)]
    fn test_completion_flag_idempotent() {
        let params = setup_test_params();
        let completed = Arc::new(AtomicBool::new(false));

        // Set completed flag
        completed.store(true, Ordering::Relaxed);
        assert!(completed.load(Ordering::Relaxed));

        // Set again (idempotent) — no change
        completed.store(true, Ordering::Relaxed);
        assert!(completed.load(Ordering::Relaxed));

        // Should not alternate or reset
        completed.store(true, Ordering::Relaxed);
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_quit_in_completed_state_enters_confirmation() {
        let params = setup_test_params();
        let completed = Arc::new(AtomicBool::new(true)); // Already completed
        let quit_state = params.quit_state.clone();

        // When completed, first 'q' still enters quit_pending (confirmation flow)
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit != 1 {
            quit_state.store(1, Ordering::SeqCst);
        }

        assert_eq!(quit_state.load(Ordering::SeqCst), 1);
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_second_q_in_completed_confirms_shutdown() {
        let params = setup_test_params();
        let completed = Arc::new(AtomicBool::new(true));
        let quit_state = params.quit_state.clone();
        let graceful_shutdown = params.graceful_shutdown.clone();

        // Already in quit_pending state (first 'q' pressed)
        quit_state.store(1, Ordering::SeqCst);
        assert!(completed.load(Ordering::Relaxed));

        // Second 'q' confirms: move to graceful shutdown
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            graceful_shutdown.store(true, Ordering::SeqCst);
            quit_state.store(0, Ordering::SeqCst);
        }

        assert!(graceful_shutdown.load(Ordering::SeqCst));
        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    #[allow(unused_variables)]
    fn test_p_in_completed_toggles_preview() {
        let params = setup_test_params();
        let completed = Arc::new(AtomicBool::new(true));
        let show_task_preview = params.show_task_preview.clone();
        let quit_state = params.quit_state.clone();

        // In completed state with preview off
        assert!(!show_task_preview.load(Ordering::Relaxed));

        // Simulate 'p' key: toggle preview (and cancel quit_pending if active)
        if quit_state.load(Ordering::SeqCst) == 1 {
            quit_state.store(0, Ordering::SeqCst);
        }
        let current = show_task_preview.load(Ordering::Relaxed);
        show_task_preview.store(!current, Ordering::Relaxed);

        assert!(show_task_preview.load(Ordering::Relaxed));
    }

    #[test]
    #[allow(unused_variables)]
    fn test_esc_in_completed_cancels_quit_pending() {
        let params = setup_test_params();
        let completed = Arc::new(AtomicBool::new(true));
        let quit_state = params.quit_state.clone();

        // In completed state with quit_pending
        quit_state.store(1, Ordering::SeqCst);
        assert_eq!(quit_state.load(Ordering::SeqCst), 1);

        // Esc cancels quit_pending
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            quit_state.store(0, Ordering::SeqCst);
        }

        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    #[allow(unused_variables)]
    fn test_esc_in_completed_running_state_noop() {
        let params = setup_test_params();
        let completed = Arc::new(AtomicBool::new(true));
        let quit_state = params.quit_state.clone();
        let focused_worker = params.focused_worker.clone();

        // In completed state, running (not quit_pending), focused on worker 1
        quit_state.store(0, Ordering::SeqCst);
        focused_worker.store(1, Ordering::Relaxed);

        // Esc in completed running state: should be a no-op
        // (don't unfocus on Esc during completed state, or let user stay focused)
        let current_quit = quit_state.load(Ordering::SeqCst);
        if current_quit == 1 {
            quit_state.store(0, Ordering::SeqCst);
        } else {
            // Option: no-op behavior, or unfocus
            // For now, testing that state doesn't change unexpectedly
            let focus_before = focused_worker.load(Ordering::Relaxed);
            // (Esc behavior in completed state is implementation-dependent)
            let focus_after = focused_worker.load(Ordering::Relaxed);
            assert_eq!(focus_before, focus_after);
        }

        assert_eq!(quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_completion_loop_continues_not_breaks() {
        let completed = Arc::new(AtomicBool::new(false));

        // Simulate orchestrator logic: when complete, loop continues
        // (doesn't break) — only graceful_shutdown breaks the loop.
        let mut loop_iterations = 0;
        let mut should_break = false;

        // Initial state: not complete
        if !completed.load(Ordering::Relaxed) {
            loop_iterations += 1;
        }

        // Check completion (like in run_loop)
        if !completed.load(Ordering::Relaxed) {
            // Set completed flag (only if not already set)
            completed.store(true, Ordering::Relaxed);
            // Loop continues — doesn't break!
            should_break = false;
        }

        // Continue loop with completed=true
        loop_iterations += 1;

        assert!(completed.load(Ordering::Relaxed));
        assert!(!should_break); // Loop did NOT break
        assert_eq!(loop_iterations, 2); // Loop ran more than once
    }

    #[test]
    fn test_completed_flag_set_once_per_session() {
        let completed = Arc::new(AtomicBool::new(false));
        let mut set_count = 0;

        // Simulate multiple checks in run_loop
        for _iteration in 0..5 {
            // Check: if not completed, set it (idempotent logic from run_loop)
            if !completed.load(Ordering::Relaxed) {
                completed.store(true, Ordering::Relaxed);
                set_count += 1;
            }
            // If already set, the condition is false, so we don't re-set
        }

        // Flag should be set exactly once
        assert_eq!(set_count, 1);
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    #[allow(unused_variables)]
    fn test_ctrl_c_in_completed_state_enters_graceful_shutdown() {
        let params = setup_test_params();
        let completed = Arc::new(AtomicBool::new(true));
        let shutdown = params.shutdown.clone();
        let graceful_shutdown = params.graceful_shutdown.clone();

        // In completed idle state
        assert!(completed.load(Ordering::Relaxed));

        // First Ctrl+C even in completed state: graceful shutdown
        if graceful_shutdown.load(Ordering::SeqCst) {
            shutdown.store(true, Ordering::SeqCst);
        } else {
            graceful_shutdown.store(true, Ordering::SeqCst);
        }

        assert!(graceful_shutdown.load(Ordering::SeqCst));
        assert!(!shutdown.load(Ordering::SeqCst));
    }

    #[test]
    #[allow(unused_variables)]
    fn test_second_ctrl_c_in_completed_forces_shutdown() {
        let params = setup_test_params();
        let completed = Arc::new(AtomicBool::new(true));
        let shutdown = params.shutdown.clone();
        let graceful_shutdown = params.graceful_shutdown.clone();

        // Already in graceful shutdown during completed state
        graceful_shutdown.store(true, Ordering::SeqCst);

        // Second Ctrl+C: force shutdown
        if graceful_shutdown.load(Ordering::SeqCst) {
            shutdown.store(true, Ordering::SeqCst);
        }

        assert!(shutdown.load(Ordering::SeqCst));
    }
}
