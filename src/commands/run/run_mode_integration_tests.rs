//! Testy integracyjne run mode — pełny flow użytkownika.
//!
//! Testy weryfikują:
//! - Startup → splash screen → main layout transition
//! - Wrzucanie linii output → wyświetlanie w ring bufferze
//! - Scrollowanie strzałkami ↑↓ → zmiana scroll_offset
//! - Quit confirmation flow (q → quit_pending, q → shutdown)

use std::time::Duration;

use crossterm::event::KeyCode;
use ratatui::text::Line;

use crate::commands::run::app::{RunApp, RunPhase};
use crate::tui::events::EventResult;
use crate::tui::test_helpers::{TestApp, make_key};

// ── Test 1: Startup → splash screen visible ──────────────────────────────

#[test]
fn test_startup_shows_splash_screen() {
    // Utwórz RunApp z show_splash=true
    let mut app = TestApp::new(
        RunApp::new("run", "claude-sonnet-4-5", false, 5000, true),
        80,
        24,
    );

    // Renderuj i sprawdź że jesteśmy w fazie Splash
    app.assert_state(|s| s.phase == RunPhase::Splash);
    app.assert_state(|s| s.splash_start_time.is_some());

    // Renderuj i sprawdź że splash jest widoczny (brak panicu)
    let buffer = app.render();
    assert_eq!(buffer.area.width, 80);
    assert_eq!(buffer.area.height, 24);
}

// ── Test 2: Splash → Running transition po 2s tick ───────────────────────

#[test]
fn test_splash_transitions_to_running_after_2s() {
    let mut app = TestApp::new(
        RunApp::new("run", "claude-sonnet-4-5", false, 5000, true),
        80,
        24,
    );

    // Sprawdź początkowy stan
    app.assert_state(|s| s.phase == RunPhase::Splash);

    // Symuluj ticki (każdy tick to check, splash trwa 1500ms)
    // Wyślij tick co 200ms, po ~8 tickach powinno przejść do Running
    for _ in 0..10 {
        app.inject_tick();
        app.step();
        std::thread::sleep(Duration::from_millis(200));

        // Jeśli już Running, break
        if app.state().phase == RunPhase::Running {
            break;
        }
    }

    // Po 2 sekundach powinno być Running
    app.assert_state(|s| s.phase == RunPhase::Running);
    app.assert_state(|s| s.splash_start_time.is_none());

    // Renderuj main layout (bez panicu)
    let buffer = app.render();
    assert_eq!(buffer.area.width, 80);
    assert_eq!(buffer.area.height, 24);
}

// ── Test 3: Push 20 lines → assert ring buffer content ───────────────────

#[test]
fn test_push_lines_fills_ring_buffer() {
    let mut app = TestApp::new(
        RunApp::new("run", "claude-sonnet-4-5", false, 5000, false),
        80,
        24,
    );

    // Skip splash (already false w konstruktorze)
    app.assert_state(|s| s.phase == RunPhase::Running);

    // Wrzuć 20 linii do buffera
    for i in 0..20 {
        let lines = vec![Line::raw(format!("Output line {}", i))];
        app.state_mut().push_lines(lines);
    }

    // Sprawdź że buffer zawiera linie (tail_visual zwraca widoczne linie)
    let visible = app.state().ring_buffer.tail_visual(50, 80);
    assert_eq!(visible.len(), 20, "Ring buffer should contain 20 lines");

    // Renderuj i sprawdź że output jest widoczny (brak panicu)
    let buffer = app.render();
    assert_eq!(buffer.area.width, 80);
}

// ── Test 4: ↑↓ keys → scroll offset changes ──────────────────────────────

#[test]
fn test_arrow_keys_scroll_output() {
    let mut app = TestApp::new(
        RunApp::new("run", "claude-sonnet-4-5", false, 5000, false),
        120,
        30,
    );

    app.assert_state(|s| s.phase == RunPhase::Running);

    // Wrzuć 50 linii żeby scrollowanie miało sens
    for i in 0..50 {
        app.state_mut()
            .push_lines(vec![Line::raw(format!("Line {}", i))]);
    }

    // Renderuj żeby wypełnić last_output_area cache (potrzebne dla Home)
    let _ = app.render();

    // Sprawdź początkowy stan scrollu
    app.assert_state(|s| s.output_view_state.auto_follow);
    app.assert_state(|s| s.output_view_state.scroll_offset == 0);

    // Strzałka w górę → scroll_offset += 1, auto_follow = false
    app.inject_key(make_key(KeyCode::Up));
    app.step();

    app.assert_state(|s| !s.output_view_state.auto_follow);
    app.assert_state(|s| s.output_view_state.scroll_offset == 1);

    // Strzałka w górę ponownie → scroll_offset += 1
    app.inject_key(make_key(KeyCode::Up));
    app.step();

    app.assert_state(|s| s.output_view_state.scroll_offset == 2);

    // Strzałka w dół → scroll_offset -= 1
    app.inject_key(make_key(KeyCode::Down));
    app.step();

    app.assert_state(|s| s.output_view_state.scroll_offset == 1);

    // End → auto_follow = true, scroll_offset = 0
    app.inject_key(make_key(KeyCode::End));
    app.step();

    app.assert_state(|s| s.output_view_state.auto_follow);
    app.assert_state(|s| s.output_view_state.scroll_offset == 0);
}

// ── Test 5: q → quit_pending, q → quit ───────────────────────────────────

#[test]
fn test_quit_confirmation_flow() {
    let mut app = TestApp::new(
        RunApp::new("run", "claude-sonnet-4-5", false, 5000, false),
        80,
        24,
    );

    app.assert_state(|s| s.phase == RunPhase::Running);
    app.assert_state(|s| !s.quit_pending);

    // Pierwszy 'q' → quit_pending = true, EventResult::Consumed
    app.inject_key(make_key(KeyCode::Char('q')));
    let result = app.step();
    assert_eq!(result, Some(EventResult::Consumed));
    app.assert_state(|s| s.quit_pending);

    // Drugi 'q' → EventResult::Quit (potwierdza zamknięcie)
    app.inject_key(make_key(KeyCode::Char('q')));
    let result = app.step();
    assert_eq!(result, Some(EventResult::Quit));
}

// ── Test 6: Scroll stress test ────────────────────────────────────────────

#[test]
fn test_scroll_stress() {
    let mut app = TestApp::new(
        RunApp::new("run", "claude-sonnet-4-5", false, 5000, false),
        120,
        30,
    );

    // Wrzuć 100 linii
    for i in 0..100 {
        app.state_mut()
            .push_lines(vec![Line::raw(format!("Line {}", i))]);
    }

    // Renderuj żeby wypełnić last_output_area
    let _ = app.render();

    // Scroll w górę 50 razy → offset == 50
    for _ in 0..50 {
        app.inject_key(make_key(KeyCode::Up));
        app.step();
    }

    app.assert_state(|s| s.output_view_state.scroll_offset == 50);

    // Scroll w dół 25 razy → offset == 25
    for _ in 0..25 {
        app.inject_key(make_key(KeyCode::Down));
        app.step();
    }

    app.assert_state(|s| s.output_view_state.scroll_offset == 25);

    // Home → max scroll
    app.inject_key(make_key(KeyCode::Home));
    app.step();

    app.assert_state(|s| !s.output_view_state.auto_follow);
    app.assert_state(|s| s.output_view_state.scroll_offset > 0);

    // End → auto_follow + scroll=0
    app.inject_key(make_key(KeyCode::End));
    app.step();

    app.assert_state(|s| s.output_view_state.auto_follow);
    app.assert_state(|s| s.output_view_state.scroll_offset == 0);
}
