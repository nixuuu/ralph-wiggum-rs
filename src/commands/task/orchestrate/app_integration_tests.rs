//! Integration tests for OrchestrateApp — full interaction flows.
//!
//! These tests verify complete user interaction scenarios using TestApp:
//! - Tab cycling through active workers
//! - Direct focus with digit keys (1/2/3)
//! - Restart flow (R → y → confirm, R → n → cancel)
//! - Task preview toggle (p)
//! - Quit confirmation (q → q → shutdown)

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crossterm::event::KeyCode;

    use crate::commands::task::orchestrate::app::{OrchestrateApp, QuitState, RestartState};
    use crate::commands::task::orchestrate::worker_status::WorkerState;
    use crate::tui::test_helpers::{TestApp, TestStep, make_key};

    // ── Test helpers ────────────────────────────────────────────────

    /// Helper: create OrchestrateApp with N workers
    fn make_app(worker_count: u32) -> OrchestrateApp {
        let mut app = OrchestrateApp::new(worker_count, Arc::new(Mutex::new(None)));
        app.sidebar_focused = false;
        app
    }

    /// Helper: make workers active (Implementing state) for navigation tests
    fn activate_workers(app: &mut OrchestrateApp, ids: &[u32]) {
        for &id in ids {
            if let Some(panel) = app.panels.get_mut(&id) {
                panel.status.state = WorkerState::Implementing;
            }
        }
        app.refresh_active_worker_ids();
    }

    // ── Integration Test 1: Tab cycling through active workers ─────

    /// Test: Tab cycles forward through active workers (1→2→3→1).
    ///
    /// Scenario:
    /// - Start with 3 active workers
    /// - Press Tab 4 times
    /// - Focus should cycle: None → 1 → 2 → 3 → 1
    #[test]
    fn integration_tab_forward_cycling() {
        let state = make_app(3);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[1, 2, 3]);

        app.run_steps(vec![
            // Initial state: no focus
            TestStep::AssertState(Box::new(|s| s.focused_worker.is_none())),
            // Tab → focus worker 1
            TestStep::KeyPress(make_key(KeyCode::Tab)),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(1))),
            // Tab → focus worker 2
            TestStep::KeyPress(make_key(KeyCode::Tab)),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(2))),
            // Tab → focus worker 3
            TestStep::KeyPress(make_key(KeyCode::Tab)),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(3))),
            // Tab → wrap around to worker 1
            TestStep::KeyPress(make_key(KeyCode::Tab)),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(1))),
        ]);
    }

    /// Test: Shift+Tab cycles backward through active workers (None→3→2→1→3).
    ///
    /// Scenario:
    /// - Start with 3 active workers, no focus
    /// - Press Shift+Tab (BackTab) 4 times
    /// - Focus should cycle backward: None → 3 → 2 → 1 → 3
    #[test]
    fn integration_tab_backward_cycling() {
        let state = make_app(3);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[1, 2, 3]);

        app.run_steps(vec![
            // Initial state: no focus
            TestStep::AssertState(Box::new(|s| s.focused_worker.is_none())),
            // BackTab → focus worker 3 (last)
            TestStep::KeyPress(make_key(KeyCode::BackTab)),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(3))),
            // BackTab → focus worker 2
            TestStep::KeyPress(make_key(KeyCode::BackTab)),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(2))),
            // BackTab → focus worker 1
            TestStep::KeyPress(make_key(KeyCode::BackTab)),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(1))),
            // BackTab → wrap around to worker 3
            TestStep::KeyPress(make_key(KeyCode::BackTab)),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(3))),
        ]);
    }

    // ── Integration Test 2: Direct focus with digit keys ───────────

    /// Test: Digit keys (1/2/3) directly focus Nth active worker.
    ///
    /// Scenario:
    /// - 5 workers total, only 2/4/5 are active
    /// - Press '1' → focus worker 2 (first active)
    /// - Press '2' → focus worker 4 (second active)
    /// - Press '3' → focus worker 5 (third active)
    /// - Press '5' → no change (out of range)
    #[test]
    fn integration_direct_focus_digit_keys() {
        let state = make_app(5);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[2, 4, 5]);

        app.run_steps(vec![
            // '1' → first active worker (worker 2)
            TestStep::KeyPress(make_key(KeyCode::Char('1'))),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(2))),
            // '2' → second active worker (worker 4)
            TestStep::KeyPress(make_key(KeyCode::Char('2'))),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(4))),
            // '3' → third active worker (worker 5)
            TestStep::KeyPress(make_key(KeyCode::Char('3'))),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(5))),
            // '5' → out of range, focus unchanged (still worker 5)
            TestStep::KeyPress(make_key(KeyCode::Char('5'))),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(5))),
        ]);
    }

    /// Test: Digit keys with non-sequential active workers.
    ///
    /// Scenario:
    /// - Workers 3, 7, 9 are active (gaps in sequence)
    /// - '1' → worker 3, '2' → worker 7, '3' → worker 9
    #[test]
    fn integration_direct_focus_non_sequential() {
        let state = make_app(10);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[3, 7, 9]);

        app.run_steps(vec![
            TestStep::KeyPress(make_key(KeyCode::Char('1'))),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(3))),
            TestStep::KeyPress(make_key(KeyCode::Char('2'))),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(7))),
            TestStep::KeyPress(make_key(KeyCode::Char('3'))),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(9))),
        ]);
    }

    // ── Integration Test 3: Restart flow ────────────────────────────

    /// Test: Restart flow — R → y → confirm.
    ///
    /// Scenario:
    /// - Focus worker 2
    /// - Press 'R' → restart pending
    /// - Press 'y' → restart confirmed
    /// - Verify restart_state transitions
    #[test]
    fn integration_restart_confirm_flow() {
        let state = make_app(3);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[1, 2, 3]);
        app.state_mut().focused_worker = Some(2);

        app.run_steps(vec![
            // Initial state: no restart
            TestStep::AssertState(Box::new(|s| s.restart_state == RestartState::None)),
            // Press 'R' → restart pending for worker 2
            TestStep::KeyPress(make_key(KeyCode::Char('R'))),
            TestStep::AssertState(Box::new(|s| {
                s.restart_state == RestartState::Pending { worker_id: 2 }
            })),
            // Press 'y' → restart confirmed
            TestStep::KeyPress(make_key(KeyCode::Char('y'))),
            TestStep::AssertState(Box::new(|s| {
                s.restart_state == RestartState::Confirmed { worker_id: 2 }
            })),
        ]);
    }

    /// Test: Restart flow — R → n → cancel.
    ///
    /// Scenario:
    /// - Focus worker 1
    /// - Press 'R' → restart pending
    /// - Press 'n' → restart cancelled (state = None)
    #[test]
    fn integration_restart_cancel_with_n() {
        let state = make_app(3);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[1, 2, 3]);
        app.state_mut().focused_worker = Some(1);

        app.run_steps(vec![
            // Press 'R' → restart pending
            TestStep::KeyPress(make_key(KeyCode::Char('R'))),
            TestStep::AssertState(Box::new(|s| {
                s.restart_state == RestartState::Pending { worker_id: 1 }
            })),
            // Press 'n' → cancel restart
            TestStep::KeyPress(make_key(KeyCode::Char('n'))),
            TestStep::AssertState(Box::new(|s| s.restart_state == RestartState::None)),
        ]);
    }

    /// Test: Restart flow — R → Esc → cancel.
    ///
    /// Scenario:
    /// - Focus worker 3
    /// - Press 'R' → restart pending
    /// - Press Esc → restart cancelled
    #[test]
    fn integration_restart_cancel_with_esc() {
        let state = make_app(3);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[1, 2, 3]);
        app.state_mut().focused_worker = Some(3);

        app.run_steps(vec![
            // Press 'R' → restart pending
            TestStep::KeyPress(make_key(KeyCode::Char('R'))),
            TestStep::AssertState(Box::new(|s| {
                s.restart_state == RestartState::Pending { worker_id: 3 }
            })),
            // Press Esc → cancel restart
            TestStep::KeyPress(make_key(KeyCode::Esc)),
            TestStep::AssertState(Box::new(|s| s.restart_state == RestartState::None)),
        ]);
    }

    /// Test: Restart banner → navigation cancels pending state.
    ///
    /// Scenario:
    /// - Restart pending for worker 1
    /// - Press Tab → restart cancelled, focus shifts
    #[test]
    fn integration_restart_cancel_on_navigation() {
        let state = make_app(3);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[1, 2, 3]);
        app.state_mut().focused_worker = Some(1);
        app.state_mut().restart_state = RestartState::Pending { worker_id: 1 };

        app.run_steps(vec![
            // Verify restart is pending
            TestStep::AssertState(Box::new(|s| {
                matches!(s.restart_state, RestartState::Pending { .. })
            })),
            // Press Tab → cancels restart, shifts focus
            TestStep::KeyPress(make_key(KeyCode::Tab)),
            TestStep::AssertState(Box::new(|s| s.restart_state == RestartState::None)),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(2))), // shifted to worker 2
        ]);
    }

    // ── Integration Test 4: Task preview toggle ─────────────────────

    /// Test: 'p' key toggles task preview overlay.
    ///
    /// Scenario:
    /// - Press 'p' → show_task_preview = true
    /// - Press 'p' again → show_task_preview = false
    #[test]
    fn integration_preview_toggle() {
        let state = make_app(1);
        let mut app = TestApp::new(state, 80, 24);

        app.run_steps(vec![
            // Initial state: preview hidden
            TestStep::AssertState(Box::new(|s| !s.show_task_preview)),
            // Press 'p' → show preview
            TestStep::KeyPress(make_key(KeyCode::Char('p'))),
            TestStep::AssertState(Box::new(|s| s.show_task_preview)),
            // Press 'p' again → hide preview
            TestStep::KeyPress(make_key(KeyCode::Char('p'))),
            TestStep::AssertState(Box::new(|s| !s.show_task_preview)),
        ]);
    }

    /// Test: Esc closes task preview when open.
    ///
    /// Scenario:
    /// - Open task preview with 'p'
    /// - Press Esc → preview closed
    #[test]
    fn integration_preview_close_with_esc() {
        let state = make_app(1);
        let mut app = TestApp::new(state, 80, 24);

        app.run_steps(vec![
            // Open preview
            TestStep::KeyPress(make_key(KeyCode::Char('p'))),
            TestStep::AssertState(Box::new(|s| s.show_task_preview)),
            // Press Esc → close preview
            TestStep::KeyPress(make_key(KeyCode::Esc)),
            TestStep::AssertState(Box::new(|s| !s.show_task_preview)),
        ]);
    }

    /// Test: Preview toggle cancels quit pending.
    ///
    /// Scenario:
    /// - First 'q' → quit pending
    /// - Press 'p' → quit cancelled, preview toggled
    #[test]
    fn integration_preview_cancels_quit() {
        let state = make_app(1);
        let mut app = TestApp::new(state, 80, 24);

        app.run_steps(vec![
            // Press 'q' → quit pending
            TestStep::KeyPress(make_key(KeyCode::Char('q'))),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Pending)),
            // Press 'p' → quit cancelled, preview toggled
            TestStep::KeyPress(make_key(KeyCode::Char('p'))),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Normal)),
            TestStep::AssertState(Box::new(|s| s.show_task_preview)),
        ]);
    }

    // ── Integration Test 5: Quit confirmation flow ──────────────────

    /// Test: q → q → graceful shutdown.
    ///
    /// Scenario:
    /// - First 'q' → quit pending
    /// - Second 'q' → graceful_shutdown = true
    #[test]
    fn integration_quit_double_q_confirm() {
        let state = make_app(1);
        let mut app = TestApp::new(state, 80, 24);

        app.run_steps(vec![
            // Initial state: Normal
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Normal)),
            TestStep::AssertState(Box::new(|s| !s.graceful_shutdown)),
            // First 'q' → quit pending
            TestStep::KeyPress(make_key(KeyCode::Char('q'))),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Pending)),
            TestStep::AssertState(Box::new(|s| !s.graceful_shutdown)),
            // Second 'q' → confirm shutdown
            TestStep::KeyPress(make_key(KeyCode::Char('q'))),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Normal)), // reset to normal
            TestStep::AssertState(Box::new(|s| s.graceful_shutdown)),
        ]);
    }

    /// Test: q → Enter → graceful shutdown.
    ///
    /// Scenario:
    /// - First 'q' → quit pending
    /// - Press Enter → graceful_shutdown = true
    #[test]
    fn integration_quit_enter_confirm() {
        let state = make_app(1);
        let mut app = TestApp::new(state, 80, 24);

        app.run_steps(vec![
            // First 'q' → quit pending
            TestStep::KeyPress(make_key(KeyCode::Char('q'))),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Pending)),
            // Press Enter → confirm shutdown
            TestStep::KeyPress(make_key(KeyCode::Enter)),
            TestStep::AssertState(Box::new(|s| s.graceful_shutdown)),
        ]);
    }

    /// Test: q → Esc → cancel quit.
    ///
    /// Scenario:
    /// - First 'q' → quit pending
    /// - Press Esc → quit cancelled (state = Normal)
    #[test]
    fn integration_quit_esc_cancel() {
        let state = make_app(1);
        let mut app = TestApp::new(state, 80, 24);

        app.run_steps(vec![
            // First 'q' → quit pending
            TestStep::KeyPress(make_key(KeyCode::Char('q'))),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Pending)),
            // Press Esc → cancel quit
            TestStep::KeyPress(make_key(KeyCode::Esc)),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Normal)),
            TestStep::AssertState(Box::new(|s| !s.graceful_shutdown)),
        ]);
    }

    /// Test: q → navigation → cancel quit.
    ///
    /// Scenario:
    /// - First 'q' → quit pending
    /// - Press Tab → quit cancelled
    #[test]
    fn integration_quit_cancel_on_navigation() {
        let state = make_app(3);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[1, 2, 3]);

        app.run_steps(vec![
            // First 'q' → quit pending
            TestStep::KeyPress(make_key(KeyCode::Char('q'))),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Pending)),
            // Press Tab → quit cancelled, focus shifts
            TestStep::KeyPress(make_key(KeyCode::Tab)),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Normal)),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(1))),
        ]);
    }

    // ── Integration Test 6: Combined flows ──────────────────────────

    /// Test: Restart + Quit interaction — mutual exclusion.
    ///
    /// Scenario:
    /// - Restart pending for worker 1
    /// - Press 'q' → restart cancelled, quit pending
    /// - Press 'q' again → graceful shutdown
    #[test]
    fn integration_restart_then_quit() {
        let state = make_app(3);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[1, 2, 3]);
        app.state_mut().focused_worker = Some(1);

        app.run_steps(vec![
            // Press 'R' → restart pending
            TestStep::KeyPress(make_key(KeyCode::Char('R'))),
            TestStep::AssertState(Box::new(|s| {
                s.restart_state == RestartState::Pending { worker_id: 1 }
            })),
            // Press 'q' → restart cancelled, quit pending
            TestStep::KeyPress(make_key(KeyCode::Char('q'))),
            TestStep::AssertState(Box::new(|s| s.restart_state == RestartState::None)),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Pending)),
            // Press 'q' again → graceful shutdown
            TestStep::KeyPress(make_key(KeyCode::Char('q'))),
            TestStep::AssertState(Box::new(|s| s.graceful_shutdown)),
        ]);
    }

    /// Test: Quit + Restart interaction — mutual exclusion.
    ///
    /// Scenario:
    /// - Quit pending
    /// - Press 'R' → quit cancelled, restart pending
    /// - Press 'y' → restart confirmed
    #[test]
    fn integration_quit_then_restart() {
        let state = make_app(3);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[1, 2, 3]);
        app.state_mut().focused_worker = Some(2);

        app.run_steps(vec![
            // Press 'q' → quit pending
            TestStep::KeyPress(make_key(KeyCode::Char('q'))),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Pending)),
            // Press 'R' → quit cancelled, restart pending
            TestStep::KeyPress(make_key(KeyCode::Char('R'))),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Normal)),
            TestStep::AssertState(Box::new(|s| {
                s.restart_state == RestartState::Pending { worker_id: 2 }
            })),
            // Press 'y' → restart confirmed
            TestStep::KeyPress(make_key(KeyCode::Char('y'))),
            TestStep::AssertState(Box::new(|s| {
                s.restart_state == RestartState::Confirmed { worker_id: 2 }
            })),
        ]);
    }

    /// Test: Esc priority — restart over quit.
    ///
    /// Scenario:
    /// - Both restart and quit pending
    /// - Press Esc → restart cancelled first, quit still pending
    /// - Press Esc again → quit cancelled
    #[test]
    fn integration_esc_priority_restart_over_quit() {
        let state = make_app(3);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[1]);
        app.state_mut().focused_worker = Some(1);
        app.state_mut().restart_state = RestartState::Pending { worker_id: 1 };
        app.state_mut().quit_state = QuitState::Pending;

        app.run_steps(vec![
            // Both pending
            TestStep::AssertState(Box::new(|s| {
                matches!(s.restart_state, RestartState::Pending { .. })
            })),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Pending)),
            // Press Esc → restart cancelled first
            TestStep::KeyPress(make_key(KeyCode::Esc)),
            TestStep::AssertState(Box::new(|s| s.restart_state == RestartState::None)),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Pending)), // still pending
            // Press Esc again → quit cancelled
            TestStep::KeyPress(make_key(KeyCode::Esc)),
            TestStep::AssertState(Box::new(|s| s.quit_state == QuitState::Normal)),
        ]);
    }

    /// Test: Complex navigation flow with state changes.
    ///
    /// Scenario:
    /// - Tab through workers
    /// - Open preview
    /// - Close preview with Esc
    /// - Direct focus with digit
    /// - Restart and cancel
    #[test]
    fn integration_complex_navigation_flow() {
        let state = make_app(3);
        let mut app = TestApp::new(state, 80, 24);
        activate_workers(app.state_mut(), &[1, 2, 3]);

        app.run_steps(vec![
            // Tab to worker 1
            TestStep::KeyPress(make_key(KeyCode::Tab)),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(1))),
            // Tab to worker 2
            TestStep::KeyPress(make_key(KeyCode::Tab)),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(2))),
            // Open preview
            TestStep::KeyPress(make_key(KeyCode::Char('p'))),
            TestStep::AssertState(Box::new(|s| s.show_task_preview)),
            // Close preview with Esc
            TestStep::KeyPress(make_key(KeyCode::Esc)),
            TestStep::AssertState(Box::new(|s| !s.show_task_preview)),
            // Direct focus to worker 3 with '3'
            TestStep::KeyPress(make_key(KeyCode::Char('3'))),
            TestStep::AssertState(Box::new(|s| s.focused_worker == Some(3))),
            // Initiate restart
            TestStep::KeyPress(make_key(KeyCode::Char('R'))),
            TestStep::AssertState(Box::new(|s| {
                s.restart_state == RestartState::Pending { worker_id: 3 }
            })),
            // Cancel restart
            TestStep::KeyPress(make_key(KeyCode::Char('n'))),
            TestStep::AssertState(Box::new(|s| s.restart_state == RestartState::None)),
        ]);
    }
}
