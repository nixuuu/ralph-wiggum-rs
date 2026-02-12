//! Dashboard keyboard/resize input thread for the fullscreen orchestrator TUI.
//!
//! Extracted from `shared_types.rs` to keep file sizes manageable.
//! Handles Tab/Esc/1-9/Up/Down (line scroll)/Left/Right (scroll home/end) for
//! panel focus and scroll control, plus q/Ctrl+C for shutdown.
//! Runs on a dedicated OS thread (never tokio::spawn).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, Ordering};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

// ── Parameters ──────────────────────────────────────────────────────

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

// ── Input thread ────────────────────────────────────────────────────

/// Keyboard/resize input thread for the fullscreen dashboard.
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
                let ctx = InputContext {
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
                };

                while running_clone.load(Ordering::SeqCst) {
                    if ctx.shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                        match event::read() {
                            Ok(Event::Key(key)) => handle_key_event(&ctx, key),
                            Ok(Event::Resize(_, _)) => handle_resize(&ctx),
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

// ── Shared context for handler functions ────────────────────────────

/// Shared references passed to all key-event handler functions.
struct InputContext {
    shutdown: Arc<AtomicBool>,
    graceful_shutdown: Arc<AtomicBool>,
    resize_flag: Arc<AtomicBool>,
    focused_worker: Arc<AtomicU32>,
    worker_count: u32,
    scroll_delta: Arc<AtomicI32>,
    render_notify: Arc<tokio::sync::Notify>,
    show_task_preview: Arc<AtomicBool>,
    reload_requested: Arc<AtomicBool>,
    quit_state: Arc<AtomicU8>,
}

impl InputContext {
    /// Cancel quit_pending state if active. Returns true if it was cancelled.
    fn cancel_quit_pending(&self) -> bool {
        if self.quit_state.load(Ordering::SeqCst) == 1 {
            self.quit_state.store(0, Ordering::SeqCst);
            true
        } else {
            false
        }
    }
}

// ── Handler functions ───────────────────────────────────────────────

/// Top-level key event dispatcher.
fn handle_key_event(ctx: &InputContext, key: KeyEvent) {
    let is_ctrl_c = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);

    if is_ctrl_c {
        handle_ctrl_c(ctx);
        return;
    }

    match key.code {
        KeyCode::Char('q') => handle_quit_key(ctx),
        KeyCode::Enter => handle_enter_key(ctx),
        KeyCode::Tab if !key.modifiers.contains(KeyModifiers::SHIFT) => {
            handle_navigation(ctx, NavigationAction::TabForward)
        }
        KeyCode::BackTab => handle_navigation(ctx, NavigationAction::TabBackward),
        KeyCode::Esc => handle_esc_key(ctx),
        KeyCode::Char(ch) if ch.is_ascii_digit() && ch != '0' => {
            handle_navigation(ctx, NavigationAction::DirectFocus(ch as u32 - '0' as u32))
        }
        KeyCode::Char('p') => handle_toggle_preview(ctx),
        KeyCode::Up => handle_scroll(ctx, -1),
        KeyCode::Down => handle_scroll(ctx, 1),
        KeyCode::Left => handle_scroll_home(ctx),
        KeyCode::Right => handle_scroll_end(ctx),
        KeyCode::Char('r') => handle_reload(ctx),
        _ => {}
    }
}

/// Handle Ctrl+C: immediate shutdown bypass (no confirmation).
fn handle_ctrl_c(ctx: &InputContext) {
    if ctx.graceful_shutdown.load(Ordering::SeqCst) {
        ctx.shutdown.store(true, Ordering::SeqCst);
    } else {
        ctx.graceful_shutdown.store(true, Ordering::SeqCst);
    }
    ctx.render_notify.notify_one();
}

/// Handle 'q' key: quit confirmation flow.
fn handle_quit_key(ctx: &InputContext) {
    let current_quit = ctx.quit_state.load(Ordering::SeqCst);

    if ctx.graceful_shutdown.load(Ordering::SeqCst) {
        // Already draining: second 'q' forces shutdown
        ctx.shutdown.store(true, Ordering::SeqCst);
    } else if current_quit == 1 {
        // In quit_pending: second 'q' confirms
        ctx.graceful_shutdown.store(true, Ordering::SeqCst);
        ctx.quit_state.store(0, Ordering::SeqCst);
    } else {
        // Normal state: first 'q' enters quit_pending
        ctx.quit_state.store(1, Ordering::SeqCst);
    }
    ctx.render_notify.notify_one();
}

/// Handle Enter key: confirm quit if in quit_pending state.
fn handle_enter_key(ctx: &InputContext) {
    if ctx.quit_state.load(Ordering::SeqCst) == 1 {
        ctx.graceful_shutdown.store(true, Ordering::SeqCst);
        ctx.quit_state.store(0, Ordering::SeqCst);
        ctx.render_notify.notify_one();
    }
}

/// Handle Esc: cancel quit_pending or unfocus.
fn handle_esc_key(ctx: &InputContext) {
    if !ctx.cancel_quit_pending() {
        // Normal unfocus behavior
        ctx.focused_worker.store(0, Ordering::Relaxed);
        ctx.scroll_delta.store(0, Ordering::Relaxed);
    }
    ctx.render_notify.notify_one();
}

/// Navigation actions for Tab/Shift+Tab/digit keys.
enum NavigationAction {
    TabForward,
    TabBackward,
    DirectFocus(u32),
}

/// Handle navigation: Tab/Shift+Tab/1-9 for panel focus.
fn handle_navigation(ctx: &InputContext, action: NavigationAction) {
    ctx.cancel_quit_pending();

    let next = match action {
        NavigationAction::TabForward => {
            let current = ctx.focused_worker.load(Ordering::Relaxed);
            if current >= ctx.worker_count {
                0
            } else {
                current + 1
            }
        }
        NavigationAction::TabBackward => {
            let current = ctx.focused_worker.load(Ordering::Relaxed);
            if current == 0 {
                ctx.worker_count
            } else {
                current - 1
            }
        }
        NavigationAction::DirectFocus(n) => {
            if n > ctx.worker_count {
                return;
            }
            n
        }
    };

    ctx.focused_worker.store(next, Ordering::Relaxed);
    ctx.scroll_delta.store(0, Ordering::Relaxed);
    ctx.render_notify.notify_one();
}

/// Handle Up/Down arrow scroll.
fn handle_scroll(ctx: &InputContext, delta: i32) {
    ctx.cancel_quit_pending();
    if delta < 0 {
        ctx.scroll_delta.fetch_sub(1, Ordering::Relaxed);
    } else {
        ctx.scroll_delta.fetch_add(1, Ordering::Relaxed);
    }
    ctx.render_notify.notify_one();
}

/// Handle Left arrow: scroll to top.
fn handle_scroll_home(ctx: &InputContext) {
    ctx.cancel_quit_pending();
    ctx.scroll_delta.store(i32::MIN, Ordering::Relaxed);
    ctx.render_notify.notify_one();
}

/// Handle Right arrow: scroll to bottom (auto-follow).
fn handle_scroll_end(ctx: &InputContext) {
    ctx.cancel_quit_pending();
    ctx.scroll_delta.store(i32::MAX, Ordering::Relaxed);
    ctx.render_notify.notify_one();
}

/// Handle 'p' key: toggle task preview overlay.
fn handle_toggle_preview(ctx: &InputContext) {
    ctx.cancel_quit_pending();
    ctx.show_task_preview.fetch_xor(true, Ordering::Relaxed);
    ctx.render_notify.notify_one();
}

/// Handle 'r' key: request tasks.yml reload.
fn handle_reload(ctx: &InputContext) {
    ctx.reload_requested.store(true, Ordering::SeqCst);
    ctx.render_notify.notify_one();
}

/// Handle terminal resize event.
fn handle_resize(ctx: &InputContext) {
    ctx.resize_flag.store(true, Ordering::SeqCst);
    ctx.render_notify.notify_one();
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

    /// Helper: build an InputContext from DashboardInputParams.
    fn ctx_from_params(params: &DashboardInputParams) -> InputContext {
        InputContext {
            shutdown: params.shutdown.clone(),
            graceful_shutdown: params.graceful_shutdown.clone(),
            resize_flag: params.resize_flag.clone(),
            focused_worker: params.focused_worker.clone(),
            worker_count: params.worker_count,
            scroll_delta: params.scroll_delta.clone(),
            render_notify: params.render_notify.clone(),
            show_task_preview: params.show_task_preview.clone(),
            reload_requested: params.reload_requested.clone(),
            quit_state: params.quit_state.clone(),
        }
    }

    #[test]
    fn test_quit_state_0_to_1_first_q() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
        handle_quit_key(&ctx);
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_quit_state_1_to_graceful_shutdown_second_q() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.quit_state.store(1, Ordering::SeqCst);
        handle_quit_key(&ctx);

        assert!(ctx.graceful_shutdown.load(Ordering::SeqCst));
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_quit_state_esc_cancels_quit_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.quit_state.store(1, Ordering::SeqCst);
        handle_esc_key(&ctx);
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_quit_state_tab_cancels_quit_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.quit_state.store(1, Ordering::SeqCst);
        ctx.focused_worker.store(1, Ordering::Relaxed);

        handle_navigation(&ctx, NavigationAction::TabForward);

        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_ctrl_c_immediate_shutdown_graceful() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        assert!(!ctx.graceful_shutdown.load(Ordering::SeqCst));
        handle_ctrl_c(&ctx);

        assert!(ctx.graceful_shutdown.load(Ordering::SeqCst));
        assert!(!ctx.shutdown.load(Ordering::SeqCst));
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_ctrl_c_second_press_forces_shutdown() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.graceful_shutdown.store(true, Ordering::SeqCst);
        handle_ctrl_c(&ctx);

        assert!(ctx.shutdown.load(Ordering::SeqCst));
    }

    #[test]
    fn test_esc_in_running_state_unfocuses() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_esc_key(&ctx);

        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 0);
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_enter_key_confirms_quit_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.quit_state.store(1, Ordering::SeqCst);
        handle_enter_key(&ctx);

        assert!(ctx.graceful_shutdown.load(Ordering::SeqCst));
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_arrow_up_cancels_quit_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.quit_state.store(1, Ordering::SeqCst);
        handle_scroll(&ctx, -1);

        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
        assert_eq!(ctx.scroll_delta.load(Ordering::Relaxed), -1);
    }

    #[test]
    fn test_digit_key_cancels_quit_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.quit_state.store(1, Ordering::SeqCst);
        handle_navigation(&ctx, NavigationAction::DirectFocus(2));

        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_p_key_toggles_preview_and_cancels_quit_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.quit_state.store(1, Ordering::SeqCst);
        ctx.show_task_preview.store(false, Ordering::Relaxed);

        handle_toggle_preview(&ctx);

        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
        assert!(ctx.show_task_preview.load(Ordering::Relaxed));
    }

    #[test]
    fn test_enter_key_ignored_in_running_state() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.quit_state.store(0, Ordering::SeqCst);
        ctx.graceful_shutdown.store(false, Ordering::SeqCst);

        handle_enter_key(&ctx);

        assert!(!ctx.graceful_shutdown.load(Ordering::SeqCst));
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_reload_key() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        handle_reload(&ctx);

        assert!(ctx.reload_requested.load(Ordering::SeqCst));
    }

    #[test]
    fn test_resize_sets_flag() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        handle_resize(&ctx);

        assert!(ctx.resize_flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_tab_wraps_around_past_last_worker() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // worker_count=3, so valid range is 0..=3
        ctx.focused_worker.store(3, Ordering::Relaxed);
        handle_navigation(&ctx, NavigationAction::TabForward);

        // Should wrap back to 0 (overview)
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_backtab_wraps_around_from_zero() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.focused_worker.store(0, Ordering::Relaxed);
        handle_navigation(&ctx, NavigationAction::TabBackward);

        // Should wrap to worker_count (last worker)
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn test_direct_focus_out_of_range_ignored() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.focused_worker.store(1, Ordering::Relaxed);
        // worker_count=3, so 5 is out of range
        handle_navigation(&ctx, NavigationAction::DirectFocus(5));

        // Should remain at 1 (unchanged)
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_scroll_home_sets_min_delta() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.quit_state.store(1, Ordering::SeqCst);
        ctx.scroll_delta.store(42, Ordering::Relaxed);

        handle_scroll_home(&ctx);

        // Should set scroll_delta to i32::MIN (signal: scroll to top)
        assert_eq!(ctx.scroll_delta.load(Ordering::Relaxed), i32::MIN);
        // Should cancel quit_pending
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_scroll_end_sets_max_delta() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.quit_state.store(1, Ordering::SeqCst);
        ctx.scroll_delta.store(-42, Ordering::Relaxed);

        handle_scroll_end(&ctx);

        // Should set scroll_delta to i32::MAX (signal: scroll to bottom)
        assert_eq!(ctx.scroll_delta.load(Ordering::Relaxed), i32::MAX);
        // Should cancel quit_pending
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_left_arrow_cancels_quit_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.quit_state.store(1, Ordering::SeqCst);

        handle_scroll_home(&ctx);

        // Left arrow should cancel quit_pending
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
        assert_eq!(ctx.scroll_delta.load(Ordering::Relaxed), i32::MIN);
    }

    #[test]
    fn test_right_arrow_cancels_quit_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.quit_state.store(1, Ordering::SeqCst);

        handle_scroll_end(&ctx);

        // Right arrow should cancel quit_pending
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
        assert_eq!(ctx.scroll_delta.load(Ordering::Relaxed), i32::MAX);
    }
}
