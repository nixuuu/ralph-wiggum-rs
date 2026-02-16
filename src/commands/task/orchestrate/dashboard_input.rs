//! Dashboard keyboard/resize input thread for the fullscreen orchestrator TUI.
//!
//! Extracted from `shared_types.rs` to keep file sizes manageable.
//! Handles Tab/Esc/1-9/Up/Down (line scroll)/Left/Right (scroll home/end) for
//! panel focus and scroll control, plus q/Ctrl+C for shutdown.
//! Runs on a dedicated OS thread (never tokio::spawn).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

use crate::commands::task::orchestrate::events::WorkerPhase;
use crate::commands::task::orchestrate::text_input_overlay::{InputAction, TextInputOverlay};
use crate::diag_warn;

// ── Parameters ──────────────────────────────────────────────────────

/// Parameters for spawning the dashboard input thread.
pub struct DashboardInputParams {
    pub shutdown: Arc<AtomicBool>,
    pub graceful_shutdown: Arc<AtomicBool>,
    pub resize_flag: Arc<AtomicBool>,
    pub focused_worker: Arc<AtomicU32>,
    pub scroll_delta: Arc<AtomicI32>,
    pub render_notify: Arc<tokio::sync::Notify>,
    pub show_task_preview: Arc<AtomicBool>,
    pub reload_requested: Arc<AtomicBool>,
    pub quit_state: Arc<AtomicU8>,
    pub restart_worker_id: Arc<AtomicU32>,
    pub restart_confirmed: Arc<AtomicBool>,
    pub active_worker_ids: Arc<Mutex<Vec<u32>>>,
    /// Flag indicating whether text input overlay is active (task 25.3.3).
    pub overlay_active: Arc<AtomicBool>,
    /// Map of worker phases (worker_id -> phase) for validation (task 25.3.3).
    /// Allows input thread to check if focused worker is in a Claude phase (Implement or ReviewFix).
    pub worker_phases: Arc<Mutex<HashMap<u32, Option<WorkerPhase>>>>,
    /// Sender for user messages from TUI overlay to run_loop (task 25.4.3).
    /// Allows the input thread to send (worker_id, message) tuples to the orchestrator.
    pub user_message_tx: tokio::sync::mpsc::Sender<(u32, String)>,
    /// Shared overlay instance (task 25.4.3).
    /// Allows input thread to handle keyboard events and Dashboard to render overlay.
    pub shared_overlay: Arc<Mutex<Option<TextInputOverlay>>>,
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
            scroll_delta,
            render_notify,
            show_task_preview,
            reload_requested,
            quit_state,
            restart_worker_id,
            restart_confirmed,
            active_worker_ids,
            overlay_active,
            worker_phases,
            user_message_tx,
            shared_overlay,
        } = params;

        let handle = std::thread::Builder::new()
            .name("dash-input".into())
            .spawn(move || {
                let ctx = InputContext {
                    shutdown,
                    graceful_shutdown,
                    resize_flag,
                    focused_worker,
                    scroll_delta,
                    render_notify,
                    show_task_preview,
                    reload_requested,
                    quit_state,
                    restart_worker_id,
                    restart_confirmed,
                    active_worker_ids,
                    overlay_active,
                    worker_phases,
                    user_message_tx,
                    shared_overlay,
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
    scroll_delta: Arc<AtomicI32>,
    render_notify: Arc<tokio::sync::Notify>,
    show_task_preview: Arc<AtomicBool>,
    reload_requested: Arc<AtomicBool>,
    quit_state: Arc<AtomicU8>,
    restart_worker_id: Arc<AtomicU32>,
    restart_confirmed: Arc<AtomicBool>,
    active_worker_ids: Arc<Mutex<Vec<u32>>>,
    /// Flag indicating whether text input overlay is active (task 25.3.3).
    overlay_active: Arc<AtomicBool>,
    /// Map of worker phases for validation (task 25.3.3).
    worker_phases: Arc<Mutex<HashMap<u32, Option<WorkerPhase>>>>,
    /// Sender for user messages to run_loop (task 25.4.3).
    user_message_tx: tokio::sync::mpsc::Sender<(u32, String)>,
    /// Shared overlay instance (task 25.4.3).
    shared_overlay: Arc<Mutex<Option<TextInputOverlay>>>,
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

    /// Check if a worker restart is pending confirmation.
    /// restart_pending = (restart_worker_id > 0) && !restart_confirmed
    fn is_restart_pending(&self) -> bool {
        self.restart_worker_id.load(Ordering::SeqCst) > 0
            && !self.restart_confirmed.load(Ordering::SeqCst)
    }

    /// Cancel restart_pending state (set restart_worker_id = 0, restart_confirmed = false).
    /// Note: These two stores are not atomic together. In the narrow window between them,
    /// is_restart_pending() may see inconsistent state. This is acceptable because:
    /// 1. Both values are only modified from the input thread (single writer)
    /// 2. The orchestrator loop checks both values together atomically when consuming
    /// 3. The worst case is a single frame rendering inconsistency
    ///
    /// Order matters: we reset restart_confirmed first, then restart_worker_id.
    /// This matches is_restart_pending() logic (checks worker_id > 0 first), minimizing
    /// the window where we might see worker_id=0 but confirmed=true.
    fn cancel_restart_pending(&self) {
        self.restart_confirmed.store(false, Ordering::SeqCst);
        self.restart_worker_id.store(0, Ordering::SeqCst);
    }
}

// ── Handler functions ───────────────────────────────────────────────

/// Top-level key event dispatcher.
fn handle_key_event(ctx: &InputContext, key: KeyEvent) {
    // Task 25.3.3: If overlay is active, route ALL keys to overlay handler
    if ctx.overlay_active.load(Ordering::SeqCst) {
        handle_overlay_key(ctx, key);
        return;
    }

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
        KeyCode::Char('i') => handle_input_overlay_key(ctx),
        KeyCode::Up => handle_scroll(ctx, -1),
        KeyCode::Down => handle_scroll(ctx, 1),
        KeyCode::Left => handle_scroll_home(ctx),
        KeyCode::Right => handle_scroll_end(ctx),
        KeyCode::Char('r') => handle_reload(ctx),
        KeyCode::Char('R') => handle_restart_key(ctx),
        KeyCode::Char('y') => handle_confirm_restart(ctx),
        KeyCode::Char('n') => handle_cancel_restart(ctx),
        _ => {}
    }
}

/// Handle Ctrl+C: immediate shutdown bypass (no confirmation).
fn handle_ctrl_c(ctx: &InputContext) {
    // Cancel any pending restart (shutdown takes priority)
    ctx.cancel_restart_pending();

    if ctx.graceful_shutdown.load(Ordering::SeqCst) {
        ctx.shutdown.store(true, Ordering::SeqCst);
    } else {
        ctx.graceful_shutdown.store(true, Ordering::SeqCst);
    }
    ctx.render_notify.notify_one();
}

/// Handle 'q' key: quit confirmation flow.
fn handle_quit_key(ctx: &InputContext) {
    // Mutual exclusion: cancel restart_pending if active
    ctx.cancel_restart_pending();

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

/// Handle Esc: cancel restart_pending, quit_pending, or unfocus.
fn handle_esc_key(ctx: &InputContext) {
    // Priority 1: cancel restart_pending
    if ctx.is_restart_pending() {
        ctx.cancel_restart_pending();
    } else if !ctx.cancel_quit_pending() {
        // Priority 2: cancel quit_pending
        // Priority 3: normal unfocus behavior
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
///
/// New behavior (Task 21.3):
/// - Tab/Shift+Tab cycle through only active (non-idle) workers
/// - Digit keys 1-9 map to Nth visible worker (not worker ID)
/// - If no active workers, focus is set to 0
fn handle_navigation(ctx: &InputContext, action: NavigationAction) {
    ctx.cancel_restart_pending();
    ctx.cancel_quit_pending();

    // Lock active_worker_ids to read the current active set
    let active_ids = ctx
        .active_worker_ids
        .lock()
        .expect("active_worker_ids: mutex poisoned");

    let next = match action {
        NavigationAction::TabForward => {
            // Cycle forward through active workers only
            if active_ids.is_empty() {
                0 // No active workers, unfocus
            } else {
                let current = ctx.focused_worker.load(Ordering::Relaxed);
                // Find current position in active list
                if let Some(pos) = active_ids.iter().position(|&id| id == current) {
                    // Move to next active worker (wrap around)
                    let next_pos = (pos + 1) % active_ids.len();
                    active_ids[next_pos]
                } else {
                    // Current focus not in active set, jump to first active
                    active_ids[0]
                }
            }
        }
        NavigationAction::TabBackward => {
            // Cycle backward through active workers only
            if active_ids.is_empty() {
                0 // No active workers, unfocus
            } else {
                let current = ctx.focused_worker.load(Ordering::Relaxed);
                // Find current position in active list
                if let Some(pos) = active_ids.iter().position(|&id| id == current) {
                    // Move to previous active worker (wrap around)
                    let prev_pos = if pos == 0 {
                        active_ids.len() - 1
                    } else {
                        pos - 1
                    };
                    active_ids[prev_pos]
                } else {
                    // Current focus not in active set, jump to last active
                    active_ids[active_ids.len() - 1]
                }
            }
        }
        NavigationAction::DirectFocus(n) => {
            // Map digit 1-9 to Nth visible worker (1-based index)
            let index = (n as usize).saturating_sub(1);
            if index < active_ids.len() {
                active_ids[index]
            } else {
                // Out of range, ignore
                return;
            }
        }
    };

    ctx.focused_worker.store(next, Ordering::Relaxed);
    ctx.scroll_delta.store(0, Ordering::Relaxed);
    ctx.render_notify.notify_one();
}

/// Handle Up/Down arrow scroll.
fn handle_scroll(ctx: &InputContext, delta: i32) {
    ctx.cancel_restart_pending();
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
    ctx.cancel_restart_pending();
    ctx.cancel_quit_pending();
    ctx.scroll_delta.store(i32::MIN, Ordering::Relaxed);
    ctx.render_notify.notify_one();
}

/// Handle Right arrow: scroll to bottom (auto-follow).
fn handle_scroll_end(ctx: &InputContext) {
    ctx.cancel_restart_pending();
    ctx.cancel_quit_pending();
    ctx.scroll_delta.store(i32::MAX, Ordering::Relaxed);
    ctx.render_notify.notify_one();
}

/// Handle 'p' key: toggle task preview overlay.
fn handle_toggle_preview(ctx: &InputContext) {
    ctx.cancel_restart_pending();
    ctx.cancel_quit_pending();
    ctx.show_task_preview.fetch_xor(true, Ordering::Relaxed);
    ctx.render_notify.notify_one();
}

/// Handle 'r' key: request tasks.yml reload.
fn handle_reload(ctx: &InputContext) {
    ctx.reload_requested.store(true, Ordering::SeqCst);
    ctx.render_notify.notify_one();
}

/// Handle 'R' key (Shift+r): initiate worker restart confirmation flow.
///
/// Triggers a restart confirmation banner for the currently focused worker.
/// Guards:
/// - Cancels quit_pending if active (mutual exclusion)
/// - Ignores if graceful shutdown is active
/// - Ignores if no worker is focused (focused_worker == 0)
/// - Ignores if restart already pending
/// - (Task 21.3) Ignores if focused worker is not in active set
///
/// When conditions are met, sets restart_worker_id to focused worker
/// and leaves restart_confirmed = false, prompting user to confirm with 'y' or cancel with 'n'.
fn handle_restart_key(ctx: &InputContext) {
    // Mutual exclusion: cancel quit_pending if active
    ctx.cancel_quit_pending();

    // Edge case: Ignore if graceful shutdown is active.
    // During shutdown, we don't accept new restart requests.
    if ctx.graceful_shutdown.load(Ordering::SeqCst) {
        return;
    }

    let focused = ctx.focused_worker.load(Ordering::Relaxed);

    // Ignore if no worker is focused (focused_worker == 0)
    if focused == 0 {
        return;
    }

    // Ignore if restart already pending
    if ctx.is_restart_pending() {
        return;
    }

    // Task 21.3: Ignore if focused worker is not in active set
    let active_ids = ctx
        .active_worker_ids
        .lock()
        .expect("active_worker_ids: mutex poisoned");
    if !active_ids.contains(&focused) {
        return;
    }
    drop(active_ids); // Release lock

    // Initiate restart: set restart_worker_id, clear restart_confirmed
    ctx.restart_worker_id.store(focused, Ordering::SeqCst);
    ctx.restart_confirmed.store(false, Ordering::SeqCst);
    ctx.render_notify.notify_one();
}

/// Handle 'y' key: confirm pending worker restart.
///
/// If a restart is pending (restart_worker_id > 0 && !restart_confirmed),
/// sets restart_confirmed = true, which signals the orchestrator loop to
/// execute the restart (abort worker, re-queue task).
fn handle_confirm_restart(ctx: &InputContext) {
    if ctx.is_restart_pending() {
        ctx.restart_confirmed.store(true, Ordering::SeqCst);
        ctx.render_notify.notify_one();
    }
}

/// Handle 'n' key: cancel pending worker restart.
///
/// If a restart is pending (restart_worker_id > 0 && !restart_confirmed),
/// resets restart_worker_id = 0 and restart_confirmed = false, dismissing
/// the restart confirmation banner.
fn handle_cancel_restart(ctx: &InputContext) {
    if ctx.is_restart_pending() {
        ctx.cancel_restart_pending();
        ctx.render_notify.notify_one();
    }
}

/// Handle keyboard events when text input overlay is active.
///
/// Routes keys to the shared overlay instance via TextInputOverlay::handle_key().
/// - Ctrl+Enter: Send message to worker via user_message_tx channel
/// - Esc: Cancel and hide overlay
/// - Other keys: Forward to overlay for text editing
fn handle_overlay_key(ctx: &InputContext, key: KeyEvent) {
    let mut overlay_guard = ctx
        .shared_overlay
        .lock()
        .expect("shared_overlay: mutex poisoned");

    if let Some(ref mut overlay) = *overlay_guard {
        let action = overlay.handle_key(key);

        match action {
            InputAction::Send(message) => {
                let worker_id = overlay.target_worker_id();

                // Send message to run_loop via channel
                // Note: We use blocking_send here because we're in a std::thread, not tokio::task
                let tx = ctx.user_message_tx.clone();
                drop(overlay_guard); // Release lock before async operation

                // Spawn a tokio task to send the message (can't use blocking_send in std::thread)
                if let Err(e) = tx.try_send((worker_id, message)) {
                    diag_warn!("Failed to send user message: {}", e);
                }

                // Hide overlay and deactivate flag
                ctx.overlay_active.store(false, Ordering::SeqCst);
                *ctx.shared_overlay
                    .lock()
                    .expect("shared_overlay: mutex poisoned") = None;
                ctx.render_notify.notify_one();
            }
            InputAction::Cancel => {
                // Hide overlay without sending message
                ctx.overlay_active.store(false, Ordering::SeqCst);
                drop(overlay_guard);
                *ctx.shared_overlay
                    .lock()
                    .expect("shared_overlay: mutex poisoned") = None;
                ctx.render_notify.notify_one();
            }
            InputAction::Continue => {
                // Key handled by overlay — trigger re-render
                ctx.render_notify.notify_one();
            }
        }
    } else {
        // Overlay should be active but instance is missing — deactivate flag
        ctx.overlay_active.store(false, Ordering::SeqCst);
        ctx.render_notify.notify_one();
    }
}

/// Handle 'i' key: show text input overlay for focused worker.
///
/// Validates:
/// - Worker must be focused (focused_worker != 0)
/// - Worker must be active (in active_worker_ids)
/// - Worker must be in a Claude phase (Implement or ReviewFix)
///
/// If validation passes, activates overlay (sets overlay_active flag).
/// Otherwise, ignored (no-op).
fn handle_input_overlay_key(ctx: &InputContext) {
    let focused = ctx.focused_worker.load(Ordering::Relaxed);

    // Guard: no worker focused
    if focused == 0 {
        return;
    }

    // Guard: focused worker not in active set
    let active_ids = ctx
        .active_worker_ids
        .lock()
        .expect("active_worker_ids: mutex poisoned");
    if !active_ids.contains(&focused) {
        return;
    }
    drop(active_ids); // Release lock

    // Guard: focused worker not in Claude phase (Implement or ReviewFix)
    let phases = ctx
        .worker_phases
        .lock()
        .expect("worker_phases: mutex poisoned");
    let phase_opt = phases.get(&focused);
    let is_claude_phase = matches!(
        phase_opt,
        Some(Some(WorkerPhase::Implement)) | Some(Some(WorkerPhase::ReviewFix))
    );
    drop(phases); // Release lock

    if !is_claude_phase {
        // Worker is not in a Claude phase — ignore
        return;
    }

    // All guards passed — create overlay instance and activate flag
    *ctx.shared_overlay
        .lock()
        .expect("shared_overlay: mutex poisoned") = Some(TextInputOverlay::new(focused));
    ctx.overlay_active.store(true, Ordering::SeqCst);
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
    /// By default, all workers (1-3) are considered active.
    fn setup_test_params() -> DashboardInputParams {
        let (user_message_tx, _user_message_rx) = tokio::sync::mpsc::channel(16);
        DashboardInputParams {
            shutdown: Arc::new(AtomicBool::new(false)),
            graceful_shutdown: Arc::new(AtomicBool::new(false)),
            resize_flag: Arc::new(AtomicBool::new(false)),
            focused_worker: Arc::new(AtomicU32::new(0)),
            scroll_delta: Arc::new(AtomicI32::new(0)),
            render_notify: Arc::new(tokio::sync::Notify::new()),
            show_task_preview: Arc::new(AtomicBool::new(false)),
            reload_requested: Arc::new(AtomicBool::new(false)),
            quit_state: Arc::new(AtomicU8::new(0)),
            restart_worker_id: Arc::new(AtomicU32::new(0)),
            restart_confirmed: Arc::new(AtomicBool::new(false)),
            active_worker_ids: Arc::new(Mutex::new(vec![1, 2, 3])),
            overlay_active: Arc::new(AtomicBool::new(false)),
            worker_phases: Arc::new(Mutex::new(HashMap::new())),
            user_message_tx,
            shared_overlay: Arc::new(Mutex::new(None)),
        }
    }

    /// Helper: build an InputContext from DashboardInputParams.
    fn ctx_from_params(params: &DashboardInputParams) -> InputContext {
        InputContext {
            shutdown: params.shutdown.clone(),
            graceful_shutdown: params.graceful_shutdown.clone(),
            resize_flag: params.resize_flag.clone(),
            focused_worker: params.focused_worker.clone(),
            scroll_delta: params.scroll_delta.clone(),
            render_notify: params.render_notify.clone(),
            show_task_preview: params.show_task_preview.clone(),
            reload_requested: params.reload_requested.clone(),
            quit_state: params.quit_state.clone(),
            restart_worker_id: params.restart_worker_id.clone(),
            restart_confirmed: params.restart_confirmed.clone(),
            active_worker_ids: params.active_worker_ids.clone(),
            overlay_active: params.overlay_active.clone(),
            worker_phases: params.worker_phases.clone(),
            user_message_tx: params.user_message_tx.clone(),
            shared_overlay: params.shared_overlay.clone(),
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
    fn test_restart_worker_sets_restart_pending_for_focused_worker() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 2
        ctx.focused_worker.store(2, Ordering::Relaxed);

        // Press 'R' to request restart
        handle_restart_key(&ctx);

        // Verify restart_worker_id is set to focused worker
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_restart_worker_ignores_when_no_focus() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // No focus (0)
        ctx.focused_worker.store(0, Ordering::Relaxed);

        // Press 'R' to request restart
        handle_restart_key(&ctx);

        // Verify restart_worker_id remains 0 (no restart)
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_restart_confirm_sets_confirmed_flag() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Set up restart_pending state
        ctx.restart_worker_id.store(2, Ordering::SeqCst);
        ctx.restart_confirmed.store(false, Ordering::SeqCst);

        // Press 'y' to confirm restart
        handle_confirm_restart(&ctx);

        // Verify restart_confirmed is set
        assert!(ctx.restart_confirmed.load(Ordering::SeqCst));
    }

    #[test]
    fn test_restart_confirm_ignores_when_no_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // No restart pending
        ctx.restart_worker_id.store(0, Ordering::SeqCst);
        ctx.restart_confirmed.store(false, Ordering::SeqCst);

        // Press 'y'
        handle_confirm_restart(&ctx);

        // Verify restart_confirmed remains false
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));
    }

    #[test]
    fn test_restart_cancel_clears_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Set up restart_pending state
        ctx.restart_worker_id.store(3, Ordering::SeqCst);

        // Press 'n' to cancel restart
        handle_cancel_restart(&ctx);

        // Verify restart_worker_id is cleared
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_esc_cancels_restart_pending_before_quit_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Set up both restart_pending and quit_pending
        ctx.restart_worker_id.store(2, Ordering::SeqCst);
        ctx.quit_state.store(1, Ordering::SeqCst);

        // Press Esc
        handle_esc_key(&ctx);

        // Verify restart_pending is cancelled first
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        // quit_state should remain (restart has higher priority)
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_esc_cancels_quit_pending_when_no_restart() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Only quit_pending
        ctx.restart_worker_id.store(0, Ordering::SeqCst);
        ctx.quit_state.store(1, Ordering::SeqCst);

        // Press Esc
        handle_esc_key(&ctx);

        // Verify quit_pending is cancelled
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_restart_worker_cancels_quit_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Set up quit_pending and focus
        ctx.quit_state.store(1, Ordering::SeqCst);
        ctx.focused_worker.store(2, Ordering::Relaxed);

        // Press 'R' to request restart
        handle_restart_key(&ctx);

        // Verify quit_pending is cancelled
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);
        // And restart_worker_id is set
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 2);
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

        // active_worker_ids = [1, 2, 3]
        // Focus on worker 3 (last active)
        ctx.focused_worker.store(3, Ordering::Relaxed);
        handle_navigation(&ctx, NavigationAction::TabForward);

        // Should wrap back to worker 1 (first active)
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_backtab_wraps_around_from_zero() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // active_worker_ids = [1, 2, 3]
        // No focus (0)
        ctx.focused_worker.store(0, Ordering::Relaxed);
        handle_navigation(&ctx, NavigationAction::TabBackward);

        // Should jump to last active worker (3)
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn test_direct_focus_out_of_range_ignored() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        ctx.focused_worker.store(1, Ordering::Relaxed);
        // active_worker_ids = [1, 2, 3], so digit 5 is out of range
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

    // ── Worker restart tests ─────────────────────────────────────────

    #[test]
    fn test_restart_key_ignored_when_no_focus() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // No worker focused (focused_worker == 0)
        ctx.focused_worker.store(0, Ordering::Relaxed);

        handle_restart_key(&ctx);

        // restart_worker_id should remain 0
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));
    }

    #[test]
    fn test_restart_key_initiates_restart_when_focused() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 2
        ctx.focused_worker.store(2, Ordering::Relaxed);

        handle_restart_key(&ctx);

        // restart_worker_id should be set to 2, restart_confirmed = false
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 2);
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));
        assert!(ctx.is_restart_pending());
    }

    #[test]
    fn test_restart_key_ignored_when_restart_already_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 1
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);

        // Now focus worker 2 and try to restart again
        ctx.focused_worker.store(2, Ordering::Relaxed);
        handle_restart_key(&ctx);

        // restart_worker_id should still be 1 (not changed to 2)
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_confirm_restart_y_key() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 1 and initiate restart
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());

        // Confirm with 'y'
        handle_confirm_restart(&ctx);

        // restart_confirmed should be true
        assert!(ctx.restart_confirmed.load(Ordering::SeqCst));
        assert!(!ctx.is_restart_pending()); // No longer pending (confirmed)
    }

    #[test]
    fn test_cancel_restart_n_key() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 1 and initiate restart
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());

        // Cancel with 'n'
        handle_cancel_restart(&ctx);

        // restart_worker_id should be 0, restart_confirmed = false
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));
        assert!(!ctx.is_restart_pending());
    }

    #[test]
    fn test_esc_cancels_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 1 and initiate restart
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());

        // Press Esc
        handle_esc_key(&ctx);

        // restart_pending should be cancelled
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_quit_key_cancels_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 1 and initiate restart
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());

        // Press 'q'
        handle_quit_key(&ctx);

        // restart_pending should be cancelled, quit_pending should be active
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_navigation_cancels_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 1 and initiate restart
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());

        // Press Tab (navigation)
        handle_navigation(&ctx, NavigationAction::TabForward);

        // restart_pending should be cancelled
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 2); // Tab to worker 2
    }

    #[test]
    fn test_scroll_cancels_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 1 and initiate restart
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());

        // Press Up arrow
        handle_scroll(&ctx, -1);

        // restart_pending should be cancelled
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_y_key_ignored_when_no_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // No restart pending
        handle_confirm_restart(&ctx);

        // Nothing should change
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));
    }

    #[test]
    fn test_n_key_ignored_when_no_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // No restart pending
        handle_cancel_restart(&ctx);

        // Nothing should change
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));
    }

    #[test]
    fn test_scroll_home_cancels_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 1 and initiate restart
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());

        // Press Left arrow (scroll home)
        handle_scroll_home(&ctx);

        // restart_pending should be cancelled
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_scroll_end_cancels_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 1 and initiate restart
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());

        // Press Right arrow (scroll end)
        handle_scroll_end(&ctx);

        // restart_pending should be cancelled
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_toggle_preview_cancels_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 1 and initiate restart
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());

        // Press 'p' (toggle preview)
        handle_toggle_preview(&ctx);

        // restart_pending should be cancelled
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
    }

    // ── Task 18.5: Edge cases and guards tests ──────────────────────────

    #[test]
    fn test_restart_key_ignored_during_graceful_shutdown() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Activate graceful shutdown
        ctx.graceful_shutdown.store(true, Ordering::SeqCst);

        // Focus worker 1
        ctx.focused_worker.store(1, Ordering::Relaxed);

        // Try to restart
        handle_restart_key(&ctx);

        // restart_worker_id should remain 0 (restart ignored)
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));
        assert!(!ctx.is_restart_pending());
    }

    #[test]
    fn test_restart_key_works_when_not_shutdown() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Normal state (no graceful shutdown)
        ctx.graceful_shutdown.store(false, Ordering::SeqCst);

        // Focus worker 1
        ctx.focused_worker.store(1, Ordering::Relaxed);

        // Try to restart
        handle_restart_key(&ctx);

        // restart_worker_id should be set to 1
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 1);
        assert!(ctx.is_restart_pending());
    }

    #[test]
    fn test_ctrl_c_cancels_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 2 and initiate restart
        ctx.focused_worker.store(2, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 2);

        // Press Ctrl+C
        handle_ctrl_c(&ctx);

        // graceful_shutdown should be active
        assert!(ctx.graceful_shutdown.load(Ordering::SeqCst));

        // restart_pending should be cancelled
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
    }

    // ── Task 18.6: Additional restart unit tests ────────────────────────

    #[test]
    fn test_tab_cancels_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 1 and initiate restart
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 1);

        // Press Tab
        handle_navigation(&ctx, NavigationAction::TabForward);

        // restart_pending should be cancelled
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));

        // Focus should move to next worker (2)
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_digit_key_cancels_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 2 and initiate restart
        ctx.focused_worker.store(2, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 2);

        // Press digit key '1' (direct focus)
        handle_navigation(&ctx, NavigationAction::DirectFocus(1));

        // restart_pending should be cancelled
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));

        // Focus should move to worker 1
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_quit_pending_cancels_restart_pending_mutual_exclusion() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 2 and initiate restart
        ctx.focused_worker.store(2, Ordering::Relaxed);
        handle_restart_key(&ctx);

        assert!(ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 2);

        // Press 'q' to initiate quit
        handle_quit_key(&ctx);

        // restart_pending should be cancelled (mutual exclusion)
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));

        // quit_pending should be active
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_restart_pending_cancels_quit_pending_mutual_exclusion() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Initiate quit_pending first
        ctx.quit_state.store(1, Ordering::SeqCst);

        // Focus worker 1 and press R
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);

        // quit_pending should be cancelled (mutual exclusion)
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 0);

        // restart_pending should be active
        assert!(ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_y_key_only_confirms_when_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // No restart pending — 'y' should be ignored
        handle_confirm_restart(&ctx);
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));

        // Initiate restart
        ctx.focused_worker.store(2, Ordering::Relaxed);
        handle_restart_key(&ctx);
        assert!(ctx.is_restart_pending());

        // Now 'y' should confirm
        handle_confirm_restart(&ctx);
        assert!(ctx.restart_confirmed.load(Ordering::SeqCst));
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_n_key_only_cancels_when_restart_pending() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // No restart pending — 'n' should be ignored
        handle_cancel_restart(&ctx);
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);

        // Initiate restart
        ctx.focused_worker.store(3, Ordering::Relaxed);
        handle_restart_key(&ctx);
        assert!(ctx.is_restart_pending());

        // Now 'n' should cancel
        handle_cancel_restart(&ctx);
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));
    }

    #[test]
    fn test_esc_priority_restart_over_quit() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Set up both restart_pending and quit_pending
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);
        assert!(ctx.is_restart_pending());

        // Manually set quit_pending as well (simulating race condition)
        ctx.quit_state.store(1, Ordering::SeqCst);

        // Press Esc — should cancel restart first (higher priority)
        handle_esc_key(&ctx);

        // restart_pending should be cancelled
        assert!(!ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);

        // quit_pending should still be active (restart has higher priority)
        assert_eq!(ctx.quit_state.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_restart_key_with_zero_focus_is_noop() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // No focus (focused_worker == 0)
        ctx.focused_worker.store(0, Ordering::Relaxed);

        // Press R
        handle_restart_key(&ctx);

        // Nothing should happen
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert!(!ctx.restart_confirmed.load(Ordering::SeqCst));
        assert!(!ctx.is_restart_pending());
    }

    #[test]
    fn test_restart_double_r_press_ignored() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Focus worker 1 and initiate restart
        ctx.focused_worker.store(1, Ordering::Relaxed);
        handle_restart_key(&ctx);
        assert!(ctx.is_restart_pending());
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 1);

        // Focus different worker and press R again
        ctx.focused_worker.store(2, Ordering::Relaxed);
        handle_restart_key(&ctx);

        // restart_worker_id should NOT change (first restart still pending)
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 1);
        assert!(ctx.is_restart_pending());
    }

    // ── Task 21.3: Dynamic navigation tests ─────────────────────────────

    #[test]
    fn test_tab_cycles_through_active_workers_only() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Set active workers to [2, 5, 7] (non-sequential)
        *ctx.active_worker_ids.lock().unwrap() = vec![2, 5, 7];

        // Focus on worker 2
        ctx.focused_worker.store(2, Ordering::Relaxed);
        handle_navigation(&ctx, NavigationAction::TabForward);

        // Should move to next active worker (5)
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 5);

        // Tab again
        handle_navigation(&ctx, NavigationAction::TabForward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 7);

        // Tab again — should wrap to first active (2)
        handle_navigation(&ctx, NavigationAction::TabForward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_backtab_cycles_backward_through_active_workers() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Set active workers to [2, 5, 7]
        *ctx.active_worker_ids.lock().unwrap() = vec![2, 5, 7];

        // Focus on worker 5
        ctx.focused_worker.store(5, Ordering::Relaxed);
        handle_navigation(&ctx, NavigationAction::TabBackward);

        // Should move to previous active worker (2)
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 2);

        // Shift+Tab again — should wrap to last active (7)
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 7);
    }

    #[test]
    fn test_digit_key_maps_to_nth_visible_worker() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Set active workers to [3, 5, 9]
        *ctx.active_worker_ids.lock().unwrap() = vec![3, 5, 9];

        // Press '1' — should focus worker 3 (1st visible)
        handle_navigation(&ctx, NavigationAction::DirectFocus(1));
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 3);

        // Press '2' — should focus worker 5 (2nd visible)
        handle_navigation(&ctx, NavigationAction::DirectFocus(2));
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 5);

        // Press '3' — should focus worker 9 (3rd visible)
        handle_navigation(&ctx, NavigationAction::DirectFocus(3));
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 9);
    }

    #[test]
    fn test_digit_key_out_of_range_for_active_workers() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Only 2 active workers: [1, 3]
        *ctx.active_worker_ids.lock().unwrap() = vec![1, 3];

        ctx.focused_worker.store(1, Ordering::Relaxed);

        // Press '5' — out of range (only 2 active workers), should be ignored
        handle_navigation(&ctx, NavigationAction::DirectFocus(5));
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 1); // unchanged
    }

    #[test]
    fn test_tab_when_all_workers_idle() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // No active workers
        *ctx.active_worker_ids.lock().unwrap() = vec![];

        ctx.focused_worker.store(2, Ordering::Relaxed);

        // Tab should unfocus (set to 0)
        handle_navigation(&ctx, NavigationAction::TabForward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_backtab_when_all_workers_idle() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // No active workers
        *ctx.active_worker_ids.lock().unwrap() = vec![];

        ctx.focused_worker.store(1, Ordering::Relaxed);

        // Shift+Tab should unfocus (set to 0)
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_tab_from_invalid_focus_jumps_to_first_active() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Active workers: [2, 4, 6]
        *ctx.active_worker_ids.lock().unwrap() = vec![2, 4, 6];

        // Focus on worker 3 (not in active set)
        ctx.focused_worker.store(3, Ordering::Relaxed);

        // Tab should jump to first active worker
        handle_navigation(&ctx, NavigationAction::TabForward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_backtab_from_invalid_focus_jumps_to_last_active() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Active workers: [2, 4, 6]
        *ctx.active_worker_ids.lock().unwrap() = vec![2, 4, 6];

        // Focus on worker 5 (not in active set)
        ctx.focused_worker.store(5, Ordering::Relaxed);

        // Shift+Tab should jump to last active worker
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 6);
    }

    #[test]
    fn test_restart_key_ignored_when_focused_worker_not_active() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Active workers: [1, 3]
        *ctx.active_worker_ids.lock().unwrap() = vec![1, 3];

        // Focus on worker 2 (not active)
        ctx.focused_worker.store(2, Ordering::Relaxed);

        // Press R — should be ignored
        handle_restart_key(&ctx);

        // restart_worker_id should remain 0
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 0);
        assert!(!ctx.is_restart_pending());
    }

    #[test]
    fn test_restart_key_works_when_focused_worker_is_active() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Active workers: [1, 3, 5]
        *ctx.active_worker_ids.lock().unwrap() = vec![1, 3, 5];

        // Focus on worker 3 (active)
        ctx.focused_worker.store(3, Ordering::Relaxed);

        // Press R — should initiate restart
        handle_restart_key(&ctx);

        // restart_worker_id should be set to 3
        assert_eq!(ctx.restart_worker_id.load(Ordering::SeqCst), 3);
        assert!(ctx.is_restart_pending());
    }

    // ── Task 25.3.3: Text input overlay tests ──────────────────────────

    #[test]
    fn test_input_overlay_key_no_focus_ignored() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // No focus (focused_worker == 0)
        ctx.focused_worker.store(0, Ordering::Relaxed);

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // overlay_active should remain false
        assert!(!ctx.overlay_active.load(Ordering::SeqCst));
    }

    #[test]
    fn test_input_overlay_key_not_active_worker_ignored() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Active workers: [1, 3]
        *ctx.active_worker_ids.lock().unwrap() = vec![1, 3];

        // Focus on worker 2 (not in active set)
        ctx.focused_worker.store(2, Ordering::Relaxed);

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // overlay_active should remain false
        assert!(!ctx.overlay_active.load(Ordering::SeqCst));
    }

    #[test]
    fn test_input_overlay_key_not_claude_phase_ignored() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Active workers: [1]
        *ctx.active_worker_ids.lock().unwrap() = vec![1];

        // Focus on worker 1
        ctx.focused_worker.store(1, Ordering::Relaxed);

        // Set worker 1 to Setup phase (not a Claude phase)
        ctx.worker_phases
            .lock()
            .unwrap()
            .insert(1, Some(WorkerPhase::Setup));

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // overlay_active should remain false
        assert!(!ctx.overlay_active.load(Ordering::SeqCst));
    }

    #[test]
    fn test_input_overlay_key_verify_phase_ignored() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Active workers: [1]
        *ctx.active_worker_ids.lock().unwrap() = vec![1];

        // Focus on worker 1
        ctx.focused_worker.store(1, Ordering::Relaxed);

        // Set worker 1 to Verify phase (not a Claude phase)
        ctx.worker_phases
            .lock()
            .unwrap()
            .insert(1, Some(WorkerPhase::Verify));

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // overlay_active should remain false
        assert!(!ctx.overlay_active.load(Ordering::SeqCst));
    }

    #[test]
    fn test_input_overlay_key_implement_phase_activates() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Active workers: [2]
        *ctx.active_worker_ids.lock().unwrap() = vec![2];

        // Focus on worker 2
        ctx.focused_worker.store(2, Ordering::Relaxed);

        // Set worker 2 to Implement phase (Claude phase)
        ctx.worker_phases
            .lock()
            .unwrap()
            .insert(2, Some(WorkerPhase::Implement));

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // overlay_active should be true
        assert!(ctx.overlay_active.load(Ordering::SeqCst));
    }

    #[test]
    fn test_input_overlay_key_review_fix_phase_activates() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Active workers: [3]
        *ctx.active_worker_ids.lock().unwrap() = vec![3];

        // Focus on worker 3
        ctx.focused_worker.store(3, Ordering::Relaxed);

        // Set worker 3 to ReviewFix phase (Claude phase)
        ctx.worker_phases
            .lock()
            .unwrap()
            .insert(3, Some(WorkerPhase::ReviewFix));

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // overlay_active should be true
        assert!(ctx.overlay_active.load(Ordering::SeqCst));
    }

    #[test]
    fn test_input_overlay_key_no_phase_entry_ignored() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Active workers: [1]
        *ctx.active_worker_ids.lock().unwrap() = vec![1];

        // Focus on worker 1
        ctx.focused_worker.store(1, Ordering::Relaxed);

        // No phase entry for worker 1 (empty HashMap)
        // worker_phases is already empty from setup_test_params

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // overlay_active should remain false
        assert!(!ctx.overlay_active.load(Ordering::SeqCst));
    }

    #[test]
    fn test_input_overlay_key_none_phase_ignored() {
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Active workers: [1]
        *ctx.active_worker_ids.lock().unwrap() = vec![1];

        // Focus on worker 1
        ctx.focused_worker.store(1, Ordering::Relaxed);

        // Set worker 1 phase to None (idle worker)
        ctx.worker_phases.lock().unwrap().insert(1, None);

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // overlay_active should remain false
        assert!(!ctx.overlay_active.load(Ordering::SeqCst));
    }

    // ── Task 28.4: Overlay population and message pipeline tests ───────

    #[test]
    fn test_input_overlay_key_populates_shared_overlay() {
        // Test: handle_input_overlay_key should populate shared_overlay when all guards pass
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Setup: worker 2 is active and in Implement phase
        *ctx.active_worker_ids.lock().unwrap() = vec![2];
        ctx.focused_worker.store(2, Ordering::Relaxed);
        ctx.worker_phases
            .lock()
            .unwrap()
            .insert(2, Some(WorkerPhase::Implement));

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // Verify overlay_active flag is set
        assert!(
            ctx.overlay_active.load(Ordering::SeqCst),
            "overlay_active should be true after 'i' key"
        );

        // Verify shared_overlay is populated
        let overlay_guard = ctx.shared_overlay.lock().unwrap();
        assert!(
            overlay_guard.is_some(),
            "shared_overlay should contain overlay instance"
        );

        // Verify overlay targets correct worker
        if let Some(ref overlay) = *overlay_guard {
            assert_eq!(
                overlay.target_worker_id(),
                2,
                "Overlay should target worker 2"
            );
        }
    }

    #[test]
    fn test_input_overlay_key_review_fix_phase_populates_shared_overlay() {
        // Test: ReviewFix phase also allows overlay creation
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Setup: worker 3 is active and in ReviewFix phase
        *ctx.active_worker_ids.lock().unwrap() = vec![3];
        ctx.focused_worker.store(3, Ordering::Relaxed);
        ctx.worker_phases
            .lock()
            .unwrap()
            .insert(3, Some(WorkerPhase::ReviewFix));

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // Verify overlay is created
        assert!(ctx.overlay_active.load(Ordering::SeqCst));
        assert!(ctx.shared_overlay.lock().unwrap().is_some());
    }

    #[test]
    fn test_input_overlay_key_does_not_populate_when_idle() {
        // Test: 'i' key should NOT populate overlay when worker is idle
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Setup: worker 1 is NOT in active set (idle)
        *ctx.active_worker_ids.lock().unwrap() = vec![2, 3]; // Worker 1 NOT active
        ctx.focused_worker.store(1, Ordering::Relaxed);
        ctx.worker_phases
            .lock()
            .unwrap()
            .insert(1, Some(WorkerPhase::Implement));

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // Verify overlay is NOT created
        assert!(
            !ctx.overlay_active.load(Ordering::SeqCst),
            "overlay_active should remain false for idle worker"
        );
        assert!(
            ctx.shared_overlay.lock().unwrap().is_none(),
            "shared_overlay should remain None for idle worker"
        );
    }

    #[test]
    fn test_input_overlay_key_does_not_populate_when_setup_phase() {
        // Test: 'i' key should NOT populate overlay when worker is in Setup phase
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Setup: worker 1 is active but in Setup phase (not a Claude phase)
        *ctx.active_worker_ids.lock().unwrap() = vec![1];
        ctx.focused_worker.store(1, Ordering::Relaxed);
        ctx.worker_phases
            .lock()
            .unwrap()
            .insert(1, Some(WorkerPhase::Setup));

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // Verify overlay is NOT created
        assert!(!ctx.overlay_active.load(Ordering::SeqCst));
        assert!(ctx.shared_overlay.lock().unwrap().is_none());
    }

    #[test]
    fn test_input_overlay_key_does_not_populate_when_verify_phase() {
        // Test: 'i' key should NOT populate overlay when worker is in Verify phase
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // Setup: worker 1 is active but in Verify phase (not a Claude phase)
        *ctx.active_worker_ids.lock().unwrap() = vec![1];
        ctx.focused_worker.store(1, Ordering::Relaxed);
        ctx.worker_phases
            .lock()
            .unwrap()
            .insert(1, Some(WorkerPhase::Verify));

        // Press 'i'
        handle_input_overlay_key(&ctx);

        // Verify overlay is NOT created
        assert!(!ctx.overlay_active.load(Ordering::SeqCst));
        assert!(ctx.shared_overlay.lock().unwrap().is_none());
    }

    // ── Task 74.5: Focus cycling edge cases with different worker counts ──

    #[test]
    fn test_focus_cycling_0_workers_stays_unfocused() {
        // Edge case: no active workers → Tab/Shift+Tab keep focus at 0
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        *ctx.active_worker_ids.lock().unwrap() = vec![];
        ctx.focused_worker.store(0, Ordering::Relaxed);

        handle_navigation(&ctx, NavigationAction::TabForward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 0);

        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_focus_cycling_1_worker_tab_stays_on_worker() {
        // 1 worker: Tab/Shift+Tab always land on the same worker
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        *ctx.active_worker_ids.lock().unwrap() = vec![1];
        ctx.focused_worker.store(0, Ordering::Relaxed);

        // Tab → jump to worker 1
        handle_navigation(&ctx, NavigationAction::TabForward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 1);

        // Tab again → wraps to itself
        handle_navigation(&ctx, NavigationAction::TabForward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 1);

        // Shift+Tab → still worker 1 (single element wrap)
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_focus_cycling_2_workers_tab_ping_pong() {
        // Test: 2 workery, Tab→Tab → focus 0→1→2→1→2
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // 2 active workers
        *ctx.active_worker_ids.lock().unwrap() = vec![1, 2];

        // Start unfocused
        ctx.focused_worker.store(0, Ordering::Relaxed);

        // Tab → jump to worker 1
        handle_navigation(&ctx, NavigationAction::TabForward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 1);

        // Tab → move to worker 2
        handle_navigation(&ctx, NavigationAction::TabForward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 2);

        // Tab → wrap back to worker 1
        handle_navigation(&ctx, NavigationAction::TabForward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 1);

        // Tab → back to worker 2 (ping-pong)
        handle_navigation(&ctx, NavigationAction::TabForward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_focus_cycling_3_workers_shift_tab_wrap_around() {
        // 3 workery: Shift+Tab z focus=1 → wrap do 3, potem pełny cykl wstecz
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // 3 active workers: [1, 2, 3]
        *ctx.active_worker_ids.lock().unwrap() = vec![1, 2, 3];

        // Start at worker 1
        ctx.focused_worker.store(1, Ordering::Relaxed);

        // Shift+Tab → should wrap to worker 3 (last)
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 3);

        // Shift+Tab → move to worker 2
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 2);

        // Shift+Tab → move to worker 1
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 1);

        // Shift+Tab → wrap to worker 3 again
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn test_focus_cycling_3_workers_shift_tab_from_unfocused() {
        // Test: 3 workery, Shift+Tab z focus=0 → jump to last (3)
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // 3 active workers: [1, 2, 3]
        *ctx.active_worker_ids.lock().unwrap() = vec![1, 2, 3];

        // Start unfocused
        ctx.focused_worker.store(0, Ordering::Relaxed);

        // Shift+Tab → should jump to last active worker (3)
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn test_focus_cycling_5_workers_10x_tab_full_loop() {
        // Test: 5 workerów, 10x Tab → focus loops 0→1→2→3→4→5→1→2→3→4→5
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // 5 active workers: [1, 2, 3, 4, 5]
        *ctx.active_worker_ids.lock().unwrap() = vec![1, 2, 3, 4, 5];

        // Start unfocused
        ctx.focused_worker.store(0, Ordering::Relaxed);

        // Expected sequence after each Tab press
        let expected = [1, 2, 3, 4, 5, 1, 2, 3, 4, 5];

        for &expected_focus in &expected {
            handle_navigation(&ctx, NavigationAction::TabForward);
            assert_eq!(
                ctx.focused_worker.load(Ordering::Relaxed),
                expected_focus,
                "Focus should cycle through workers correctly"
            );
        }

        // After 10 Tabs, we should be at worker 5 again
        // One more Tab should wrap to worker 1
        handle_navigation(&ctx, NavigationAction::TabForward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_focus_cycling_5_workers_backward_loop() {
        // Test: 5 workerów, Shift+Tab cycling backward
        let params = setup_test_params();
        let ctx = ctx_from_params(&params);

        // 5 active workers: [1, 2, 3, 4, 5]
        *ctx.active_worker_ids.lock().unwrap() = vec![1, 2, 3, 4, 5];

        // Start at worker 1
        ctx.focused_worker.store(1, Ordering::Relaxed);

        // Shift+Tab → wrap to worker 5
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 5);

        // Shift+Tab → move to worker 4
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 4);

        // Shift+Tab → move to worker 3
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 3);

        // Shift+Tab → move to worker 2
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 2);

        // Shift+Tab → move to worker 1
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 1);

        // Shift+Tab → wrap to worker 5 again
        handle_navigation(&ctx, NavigationAction::TabBackward);
        assert_eq!(ctx.focused_worker.load(Ordering::Relaxed), 5);
    }
}
