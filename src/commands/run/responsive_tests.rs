//! Testy integracyjne responsywności run mode — resize event → breakpoint switch → layout change.
//!
//! Testy weryfikują:
//! - Detekcję breakpointów (Large/Medium/Small) przy różnych rozmiarach terminala
//! - Przełączanie layoutu przy resize evencie
//! - Właściwe renderowanie sidebar w zależności od breakpointu
//! - Przywracanie layoutu po powrocie do większego rozmiaru

use ratatui::layout::Rect;

use crate::commands::run::app::RunApp;
use crate::tui::responsive::{Breakpoint, LayoutAreas};
use crate::tui::test_helpers::TestApp;

// ── Helper: render and get sidebar width ──────────────────────────────

/// Renderuje app i zwraca szerokość sidebara (jeśli jest widoczny).
/// Zwraca None jeśli sidebar jest niewidoczny (breakpoint Small lub sidebar.visible == false).
///
/// Używa `LayoutAreas::for_breakpoint` żeby nie duplikować logiki layoutu z kodu produkcyjnego.
/// Faktyczna szerokość renderowana to `min(sidebar_area.width, sidebar.width())`.
fn get_rendered_sidebar_width(app: &mut TestApp<RunApp>) -> Option<u16> {
    let buffer = app.render();
    let state = app.state();

    let bp = state.current_breakpoint();
    let area = Rect::new(0, 0, buffer.area.width, buffer.area.height);
    let layout = LayoutAreas::for_breakpoint(bp, area);

    match layout.sidebar {
        Some(sidebar_area) if state.sidebar.visible => {
            // Faktyczna szerokość = min(layout allocation, sidebar.width())
            Some(sidebar_area.width.min(state.sidebar.width()))
        }
        _ => None,
    }
}

// ── Test: Large → Medium → Small → Large ──────────────────────────────

#[test]
fn test_responsive_resize_sequence() {
    // 1. Start at 120x40 (Large) → assert sidebar visible
    let mut app = TestApp::new(
        RunApp::new("run", "claude-sonnet-4-5", false, 5000, false),
        120,
        40,
    );

    // Force phase to Running (skip splash)
    app.state_mut().phase = crate::commands::run::app::RunPhase::Running;

    // Renderuj początkowy stan
    let buffer = app.render();
    assert_eq!(buffer.area.width, 120);
    assert_eq!(buffer.area.height, 40);

    // Sprawdź breakpoint
    assert_eq!(app.state().current_breakpoint(), Breakpoint::Large);

    // Sprawdź że sidebar jest widoczny (Large breakpoint, sidebar.visible domyślnie true)
    let sidebar_width = get_rendered_sidebar_width(&mut app);
    assert!(
        sidebar_width.is_some(),
        "Sidebar should be visible at Large breakpoint (120x40)"
    );
    // W Large breakpoint sidebar width to 20% z 120 = 24
    assert_eq!(sidebar_width.unwrap(), 24);

    // 2. Resize to 90x30 (Medium) → assert collapsed sidebar (width=3)
    app.resize_terminal(90, 30);
    app.step();

    let buffer = app.render();
    assert_eq!(buffer.area.width, 90);
    assert_eq!(buffer.area.height, 30);

    // Sprawdź breakpoint
    assert_eq!(app.state().current_breakpoint(), Breakpoint::Medium);

    // Sprawdź że sidebar jest widoczny ale skrócony (collapsed, width=3)
    let sidebar_width = get_rendered_sidebar_width(&mut app);
    assert!(
        sidebar_width.is_some(),
        "Sidebar should be visible at Medium breakpoint (90x30)"
    );
    assert_eq!(
        sidebar_width.unwrap(),
        3,
        "Sidebar should be collapsed (width=3) at Medium breakpoint"
    );

    // 3. Resize to 60x24 (Small) → assert no sidebar
    app.resize_terminal(60, 24);
    app.step();

    let buffer = app.render();
    assert_eq!(buffer.area.width, 60);
    assert_eq!(buffer.area.height, 24);

    // Sprawdź breakpoint
    assert_eq!(app.state().current_breakpoint(), Breakpoint::Small);

    // Sprawdź że sidebar jest niewidoczny
    let sidebar_width = get_rendered_sidebar_width(&mut app);
    assert!(
        sidebar_width.is_none(),
        "Sidebar should NOT be visible at Small breakpoint (60x24)"
    );

    // 4. Resize back to 120x40 → assert sidebar restored
    app.resize_terminal(120, 40);
    app.step();

    let buffer = app.render();
    assert_eq!(buffer.area.width, 120);
    assert_eq!(buffer.area.height, 40);

    // Sprawdź breakpoint
    assert_eq!(app.state().current_breakpoint(), Breakpoint::Large);

    // Sprawdź że sidebar wrócił (Large breakpoint, sidebar.visible powinno być true)
    let sidebar_width = get_rendered_sidebar_width(&mut app);
    assert!(
        sidebar_width.is_some(),
        "Sidebar should be restored after returning to Large breakpoint (120x40)"
    );
    assert_eq!(
        sidebar_width.unwrap(),
        24,
        "Sidebar width should be 24 (20% of 120) after returning to Large breakpoint"
    );
}

// ── Test: Focus switch on Small breakpoint ────────────────────────────

#[test]
fn test_focus_switches_to_output_on_small_breakpoint() {
    // Start at 120x40 (Large) with focus on Sidebar
    let mut app = TestApp::new(
        RunApp::new("run", "claude-sonnet-4-5", false, 5000, false),
        120,
        40,
    );

    // Force phase to Running
    app.state_mut().phase = crate::commands::run::app::RunPhase::Running;

    // Set focus to Sidebar
    app.state_mut().focus = crate::commands::run::app::FocusArea::Sidebar;

    // Render
    app.render();

    // Verify focus is on Sidebar
    assert_eq!(
        app.state().focus,
        crate::commands::run::app::FocusArea::Sidebar
    );

    // Resize to 60x24 (Small) → focus should switch to Output
    app.resize_terminal(60, 24);
    app.step();

    app.render();

    // Verify breakpoint
    assert_eq!(app.state().current_breakpoint(), Breakpoint::Small);

    // Sidebar is now an overlay in Small mode — focus preserved
    assert_eq!(
        app.state().focus,
        crate::commands::run::app::FocusArea::Sidebar,
        "Focus should be preserved when resizing to Small breakpoint (sidebar is overlay)"
    );
}

// ── Test: Large → Medium maintains sidebar visibility ─────────────────

#[test]
fn test_large_to_medium_maintains_sidebar_visibility() {
    // Start at 120x40 (Large)
    let mut app = TestApp::new(
        RunApp::new("run", "claude-sonnet-4-5", false, 5000, false),
        120,
        40,
    );

    // Force phase to Running
    app.state_mut().phase = crate::commands::run::app::RunPhase::Running;

    // Render
    app.render();

    // Sidebar should be visible
    assert!(app.state().sidebar.visible);
    assert_eq!(app.state().current_breakpoint(), Breakpoint::Large);

    // Resize to 100x30 (Medium)
    app.resize_terminal(100, 30);
    app.step();

    app.render();

    // Sidebar should still be visible (just collapsed)
    assert!(
        app.state().sidebar.visible,
        "Sidebar should remain visible when transitioning from Large to Medium"
    );
    assert_eq!(app.state().current_breakpoint(), Breakpoint::Medium);
}

// ── Test: Hidden sidebar stays hidden across breakpoints ──────────────

#[test]
fn test_hidden_sidebar_stays_hidden_across_breakpoints() {
    // Start at 120x40 (Large) with hidden sidebar
    let mut app = TestApp::new(
        RunApp::new("run", "claude-sonnet-4-5", false, 5000, false),
        120,
        40,
    );

    // Force phase to Running
    app.state_mut().phase = crate::commands::run::app::RunPhase::Running;

    // Hide sidebar
    app.state_mut().sidebar.visible = false;

    // Render
    app.render();

    // Verify sidebar is hidden
    assert!(!app.state().sidebar.visible);

    // Resize to 100x30 (Medium)
    app.resize_terminal(100, 30);
    app.step();
    app.render();

    // Sidebar should still be hidden
    assert!(
        !app.state().sidebar.visible,
        "Hidden sidebar should stay hidden at Medium breakpoint"
    );

    // Resize to 60x24 (Small)
    app.resize_terminal(60, 24);
    app.step();
    app.render();

    // Sidebar should still be hidden
    assert!(
        !app.state().sidebar.visible,
        "Hidden sidebar should stay hidden at Small breakpoint"
    );

    // Resize back to 120x40 (Large)
    app.resize_terminal(120, 40);
    app.step();
    app.render();

    // Sidebar should still be hidden
    assert!(
        !app.state().sidebar.visible,
        "Hidden sidebar should stay hidden when returning to Large breakpoint"
    );
}

// ── Test: Multiple rapid resizes ──────────────────────────────────────

#[test]
fn test_multiple_rapid_resizes() {
    let mut app = TestApp::new(
        RunApp::new("run", "claude-sonnet-4-5", false, 5000, false),
        120,
        40,
    );

    // Force phase to Running
    app.state_mut().phase = crate::commands::run::app::RunPhase::Running;

    // Rapid resize sequence: Large → Medium → Small → Medium → Large
    let sizes = [(120, 40), (90, 30), (60, 24), (100, 30), (130, 50)];
    let expected_breakpoints = [
        Breakpoint::Large,
        Breakpoint::Medium,
        Breakpoint::Small,
        Breakpoint::Medium,
        Breakpoint::Large,
    ];

    for ((width, height), expected_bp) in sizes.iter().zip(expected_breakpoints.iter()) {
        app.resize_terminal(*width, *height);
        app.step();
        app.render();

        assert_eq!(
            app.state().current_breakpoint(),
            *expected_bp,
            "Breakpoint mismatch at size {}x{}",
            width,
            height
        );
    }
}

// ── Test: Breakpoint boundaries ───────────────────────────────────────

#[test]
fn test_breakpoint_boundaries() {
    // Test boundary values: 79, 80, 119, 120
    let test_cases = [
        (79, Breakpoint::Small),
        (80, Breakpoint::Medium),
        (119, Breakpoint::Medium),
        (120, Breakpoint::Large),
    ];

    for (width, expected_bp) in test_cases {
        let mut app = TestApp::new(
            RunApp::new("run", "claude-sonnet-4-5", false, 5000, false),
            width,
            40,
        );

        // Force phase to Running
        app.state_mut().phase = crate::commands::run::app::RunPhase::Running;

        // Inject resize to ensure breakpoint detection
        app.resize_terminal(width, 40);
        app.step();
        app.render();

        assert_eq!(
            app.state().current_breakpoint(),
            expected_bp,
            "Breakpoint mismatch at width {}",
            width
        );
    }
}
