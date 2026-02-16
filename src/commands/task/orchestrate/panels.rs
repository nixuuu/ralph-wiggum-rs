use std::collections::{HashMap, VecDeque};

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use super::shared_types::{format_duration, format_tokens};
use super::shutdown_types::OrchestratorStatus;
use super::worker_status::{WorkerState, WorkerStatus};
use crate::commands::task::orchestrate::dashboard::WorkerPanel;
use crate::shared::tasks::reverse_model_alias;

// ── Panel rendering functions ────────────────────────────────────────

/// Build a Paragraph widget for a worker panel.
pub fn render_panel_widget<'a>(
    panel: &'a WorkerPanel,
    area: Rect,
    is_focused: bool,
) -> Paragraph<'a> {
    let ws = &panel.status;

    // Build title line
    let title_line = build_title_line(panel.worker_id, ws, is_focused);

    // Build footer line
    let footer = build_footer_line(ws);

    // Check if worker is in grace period (idle but recently)
    let in_grace_period = ws.state == WorkerState::Idle && panel.idle_since.is_some();

    // Build border style
    let (border_type, border_style) = build_border_style(is_focused, &ws.state, in_grace_period);

    // Construct block with title and footer
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(border_style)
        .title(title_line)
        .title_bottom(Span::styled(footer, Style::default().fg(Color::DarkGray)));

    // Build content lines
    let lines = build_panel_content(panel, area);

    Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
}

/// Build the title line for a worker panel.
fn build_title_line(worker_id: u32, ws: &WorkerStatus, is_focused: bool) -> Line<'static> {
    let task_str = ws.task_id.as_deref().unwrap_or("---");
    let comp_str = ws.component.as_deref().unwrap_or("");
    let focus_marker = if is_focused { "▶ " } else { "" };

    // Build base title without model
    let base_title = if comp_str.is_empty() {
        format!(" {focus_marker}W{} [{}] ", worker_id, task_str)
    } else {
        format!(
            " {focus_marker}W{} [{}: {}] ",
            worker_id, task_str, comp_str
        )
    };

    let title_style = if is_focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };

    // Append model suffix if present
    if let Some(model) = &ws.model {
        let alias = reverse_model_alias(model);
        let model_color = if ws.state == WorkerState::ResolvingConflicts {
            Color::Yellow
        } else {
            Color::DarkGray
        };

        Line::from(vec![
            Span::styled(base_title.trim_end().to_string(), title_style),
            Span::styled(format!(" ({})", alias), Style::default().fg(model_color)),
            Span::raw(" "),
        ])
    } else {
        Line::from(vec![Span::styled(base_title, title_style)])
    }
}

/// Build the footer line showing cost and tokens.
fn build_footer_line(ws: &WorkerStatus) -> String {
    format!(
        " ${:.4} │ ↓{} ↑{} ",
        ws.cost_usd.max(0.0),
        format_tokens(ws.input_tokens),
        format_tokens(ws.output_tokens)
    )
}

/// Build the border style (type, border style) based on focus, state, and grace period.
fn build_border_style(
    is_focused: bool,
    state: &WorkerState,
    in_grace_period: bool,
) -> (BorderType, Style) {
    let (border_type, border_color) = if is_focused {
        (BorderType::Double, Color::Cyan)
    } else if in_grace_period {
        // Worker in grace period — use dimmed color
        (BorderType::Rounded, Color::Gray)
    } else {
        (BorderType::Rounded, state.color())
    };

    let border_style = if is_focused {
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD)
    } else if in_grace_period {
        // Dimmed border for grace period
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(border_color)
    };

    (border_type, border_style)
}

/// Build the content lines for the panel (status + output).
fn build_panel_content<'a>(panel: &'a WorkerPanel, area: Rect) -> Vec<Line<'a>> {
    let ws = &panel.status;

    // Inner area height (minus 2 for borders)
    let inner_height = area.height.saturating_sub(2) as usize;
    if inner_height == 0 {
        return Vec::new();
    }

    // First line: phase icon + name + profiles (if verify phase)
    let (icon, icon_color) = ws.state.icon();
    let phase_str = ws
        .phase
        .as_ref()
        .map(|p| p.to_string())
        .unwrap_or_else(|| ws.state.to_string());

    let mut status_spans = vec![
        Span::styled(icon.to_string(), Style::default().fg(icon_color)),
        Span::raw(" "),
        Span::styled(phase_str, Style::default().fg(icon_color)),
    ];

    // Add profile info for Verify phase
    if ws.state == WorkerState::Verifying && !ws.verify_profiles.is_empty() {
        let profile_info: Vec<String> = ws
            .verify_profiles
            .iter()
            .map(|(name, success)| {
                let icon = match success {
                    Some(true) => "✓",
                    Some(false) => "✗",
                    None => "⏳",
                };
                format!("{} {}", name, icon)
            })
            .collect();
        let profiles_str = format!(" [{}]", profile_info.join(", "));
        status_spans.push(Span::styled(
            profiles_str,
            Style::default().fg(Color::DarkGray),
        ));
    }

    let status_line = Line::from(status_spans);

    // Remaining lines: output tail (visual-wrap aware)
    let output_height = inner_height.saturating_sub(1);
    let inner_width = area.width.saturating_sub(2); // subtract borders
    let mut lines = vec![status_line];

    if output_height > 0 {
        let tail = if panel.scroll_offset == 0 {
            // Auto-scroll: show last N lines fitting visual rows
            panel.output.tail_visual(output_height, inner_width)
        } else {
            // Manual scroll: offset counted in visual rows from bottom
            // Clamp offset to valid range: max = total_visual_rows - output_height
            let total_visual = panel.output.total_visual_rows(inner_width);
            let max_offset = total_visual.saturating_sub(output_height);
            let clamped_offset = panel.scroll_offset.min(max_offset);

            panel
                .output
                .slice_visual(clamped_offset, output_height, inner_width)
        };
        lines.extend(tail);
    }

    lines
}

// ── Compact render ───────────────────────────────────────────────────

/// Compact render for small terminals — single panel + tab bar.
/// Only shows non-idle workers in the tab bar and auto-focuses the next active worker
/// when the focused worker becomes idle. Shows a placeholder when all workers are idle.
// Too many arguments: grouped rendering context (frame, area, panels, status, etc.)
#[allow(clippy::too_many_arguments)]
pub fn render_compact(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    panels: &HashMap<u32, WorkerPanel>,
    status: &OrchestratorStatus,
    focused: Option<u32>,
    _log_lines: &VecDeque<Line<'static>>,
    worker_count: u32,
    preview_active: bool,
) {
    // Tab bar at top (1 line)
    let vertical = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);

    let tab_area = vertical[0];
    let panel_area = vertical[1];
    let bar_area = vertical[2];

    // Filter panels to only non-idle workers
    let active_workers: Vec<u32> = (1..=worker_count)
        .filter(|&id| {
            panels
                .get(&id)
                .map(|p| p.status.state != WorkerState::Idle)
                .unwrap_or(false)
        })
        .collect();

    // Tab bar — only show active workers
    let tab_spans = build_tab_bar_filtered(&active_workers, focused);
    frame.render_widget(Line::from(tab_spans), tab_area);

    // Determine which worker to show
    let show_id = if active_workers.is_empty() {
        // All workers idle — show placeholder
        None
    } else if let Some(fid) = focused {
        // Auto-shift focus to first active worker if focused worker is idle
        if active_workers.contains(&fid) {
            Some(fid)
        } else {
            // Focused worker is idle, pick first active
            active_workers.first().copied()
        }
    } else {
        // No focus, pick first active
        active_workers.first().copied()
    };

    if let Some(wid) = show_id {
        if let Some(panel) = panels.get(&wid) {
            let widget = render_panel_widget(panel, panel_area, true);
            frame.render_widget(widget, panel_area);
        }
    } else {
        // Render placeholder when all workers are idle
        render_idle_placeholder(frame, panel_area);
    }

    // Compact status bar (1 line)
    let compact_bar = build_compact_bar(status, preview_active);
    frame.render_widget(compact_bar, bar_area);
}

/// Build tab bar spans for only the provided active workers.
/// Used in compact mode to show only non-idle workers.
fn build_tab_bar_filtered(active_workers: &[u32], focused: Option<u32>) -> Vec<Span<'static>> {
    if active_workers.is_empty() {
        return vec![Span::styled(
            " All workers idle ",
            Style::default().fg(Color::DarkGray),
        )];
    }

    let mut tab_spans = Vec::new();
    for &wid in active_workers {
        let is_active = focused == Some(wid);
        let style = if is_active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        tab_spans.push(Span::styled(format!(" W{wid} "), style));
        tab_spans.push(Span::raw(" "));
    }
    tab_spans
}

/// Render placeholder panel when all workers are idle.
fn render_idle_placeholder(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " Orchestrator ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ));

    let message = vec![
        Line::from(""),
        Line::from(Span::styled(
            "○ All workers idle",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Waiting for tasks to be assigned...",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let widget = Paragraph::new(message)
        .block(block)
        .alignment(ratatui::layout::Alignment::Center);

    frame.render_widget(widget, area);
}

/// Build compact status bar (1 line) for small terminals.
fn build_compact_bar(status: &OrchestratorStatus, preview_active: bool) -> Line<'static> {
    let total = status.scheduler.total;
    let done = status.scheduler.done;
    let pct = if total > 0 { (done * 100) / total } else { 0 };
    let elapsed = format_duration(status.elapsed);
    let total_cost = status.total_cost.max(0.0);

    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            format!("{done}/{total} ({pct}%)"),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("${total_cost:.4}"),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" │ "),
        Span::styled(format!("⏱ {elapsed}"), Style::default().fg(Color::White)),
    ];

    // Add completion indicator if all tasks are done
    if status.completed {
        spans.push(Span::raw(" │ "));
        spans.push(Span::styled(
            "✓ DONE",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));
    }

    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        if preview_active {
            "p/Esc=close ↑↓=scroll"
        } else {
            "q=quit Tab=switch p=tasks"
        },
        Style::default().fg(Color::DarkGray),
    ));

    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_panel_content_with_verify_profiles() {
        use crate::commands::task::orchestrate::dashboard::WorkerPanel;
        use crate::commands::task::orchestrate::events::WorkerPhase;
        use crate::commands::task::orchestrate::ring_buffer::OutputRingBuffer;

        let mut status = WorkerStatus::idle(1);
        status.state = WorkerState::Verifying;
        status.phase = Some(WorkerPhase::Verify);
        status.verify_profiles = vec![
            ("frontend".to_string(), Some(true)),
            ("backend".to_string(), None),
            ("database".to_string(), Some(false)),
        ];

        let panel = WorkerPanel {
            worker_id: 1,
            status,
            output: OutputRingBuffer::new(10),
            scroll_offset: 0,
            idle_since: None,
        };

        let area = Rect::new(0, 0, 80, 10);
        let lines = build_panel_content(&panel, area);

        // First line should contain phase and profiles
        assert!(!lines.is_empty());
        let first_line_text = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();

        // Should contain phase name
        assert!(first_line_text.contains("verify"));
        // Should contain profile names with status icons
        assert!(first_line_text.contains("frontend"));
        assert!(first_line_text.contains("backend"));
        assert!(first_line_text.contains("database"));
        // Should contain status icons
        assert!(first_line_text.contains("✓")); // frontend success
        assert!(first_line_text.contains("⏳")); // backend in progress
        assert!(first_line_text.contains("✗")); // database failed
    }

    #[test]
    fn test_build_footer_line() {
        let ws = WorkerStatus {
            state: WorkerState::Idle,
            phase: None,
            task_id: None,
            component: None,
            model: None,
            cost_usd: 0.1234,
            input_tokens: 5000,
            output_tokens: 3000,
            verify_profiles: Vec::new(),
        };

        let footer = build_footer_line(&ws);
        assert!(footer.contains("$0.1234"));
        assert!(footer.contains("5.0k"));
        assert!(footer.contains("3.0k"));
    }

    #[test]
    fn test_build_title_line_basic() {
        let ws = WorkerStatus {
            state: WorkerState::Implementing,
            phase: None,
            task_id: Some("T01".to_string()),
            component: Some("api".to_string()),
            model: None,
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            verify_profiles: Vec::new(),
        };

        let line = build_title_line(1, &ws, false);
        let text = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();

        assert!(text.contains("W1"));
        assert!(text.contains("T01"));
        assert!(text.contains("api"));
    }

    #[test]
    fn test_build_title_line_with_model() {
        let ws = WorkerStatus {
            state: WorkerState::Implementing,
            phase: None,
            task_id: Some("T02".to_string()),
            component: None,
            model: Some("claude-sonnet-4-5-20250929".to_string()),
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            verify_profiles: Vec::new(),
        };

        let line = build_title_line(2, &ws, false);
        let text = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();

        assert!(text.contains("(sonnet)"));
    }

    #[test]
    fn test_build_title_line_focused() {
        let ws = WorkerStatus {
            state: WorkerState::Idle,
            phase: None,
            task_id: None,
            component: None,
            model: None,
            verify_profiles: Vec::new(),
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
        };

        let line = build_title_line(3, &ws, true);
        let text = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();

        assert!(text.contains("▶"));
    }

    #[test]
    fn test_build_compact_bar_completed() {
        use super::super::scheduler::SchedulerStatus;
        use super::super::shutdown_types::ShutdownState;
        use std::time::Duration;

        let status = OrchestratorStatus {
            scheduler: SchedulerStatus {
                total: 5,
                done: 5,
                ready: 0,
                in_progress: 0,
                blocked: 0,
                pending: 0,
            },
            completed: true,
            shutdown_state: ShutdownState::Running,
            shutdown_remaining: None,
            quit_pending: false,
            total_cost: 1.2345,
            elapsed: Duration::from_secs(120),
            restart_pending: None,
            active_workers: 0,
            idle_workers: 3,
        };

        let line = build_compact_bar(&status, false);
        let text = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();

        assert!(text.contains("5/5"));
        assert!(text.contains("100%"));
        assert!(text.contains("✓ DONE"));
    }

    #[test]
    fn test_build_panel_content_scroll_offset_clamping() {
        use super::super::ring_buffer::OutputRingBuffer;
        use crate::commands::task::orchestrate::dashboard::WorkerPanel;
        use crate::commands::task::orchestrate::worker_status::{WorkerState, WorkerStatus};

        let mut output = OutputRingBuffer::new(100);
        // Add some lines with wrapping behavior at width 40
        output.push("short1"); // 1 visual row
        output.push(&"X".repeat(80)); // 2 visual rows
        output.push("short2"); // 1 visual row
        output.push(&"Y".repeat(120)); // 3 visual rows
        output.push("short3"); // 1 visual row
        // Total: 8 visual rows

        let status = WorkerStatus {
            state: WorkerState::Idle,
            phase: None,
            task_id: None,
            component: None,
            model: None,
            cost_usd: 0.0,
            input_tokens: 0,
            verify_profiles: Vec::new(),
            output_tokens: 0,
        };

        let panel = WorkerPanel {
            worker_id: 1,
            status,
            output,
            scroll_offset: 1000, // Deliberately excessive offset
            idle_since: None,
        };

        // Inner height = 10, minus 1 for status line = 9 output rows
        // Inner width = 40
        let area = Rect {
            x: 0,
            y: 0,
            width: 44,  // +2 for borders = 44
            height: 12, // +2 for borders = 12
        };

        let lines = build_panel_content(&panel, area);

        // Should have 1 status line + output lines (up to 9)
        // With 8 total visual rows and 9 available, all content fits
        assert!(!lines.is_empty(), "Should have at least status line");
        assert!(lines.len() <= 10, "Should not exceed inner_height");

        // Test with scroll_offset that should show partial content
        let panel_scrolled = WorkerPanel {
            scroll_offset: 3, // Skip 3 visual rows from bottom
            ..panel
        };

        let lines_scrolled = build_panel_content(&panel_scrolled, area);
        assert!(
            !lines_scrolled.is_empty(),
            "Should have at least status line"
        );
    }

    #[test]
    fn test_build_tab_bar_filtered_with_active_workers() {
        let active_workers = vec![1, 3, 4];
        let spans = build_tab_bar_filtered(&active_workers, Some(3));

        // Should have 3 workers * 2 spans each (worker + space) = 6 spans
        assert_eq!(spans.len(), 6);

        // W3 should be highlighted (focused)
        let w3_span = &spans[2]; // Third worker tab (index 2)
        assert!(w3_span.content.contains("W3"));
        assert_eq!(w3_span.style.bg, Some(Color::Cyan));

        // W1 and W4 should not be highlighted
        assert_eq!(spans[0].style.bg, None); // W1
        assert_eq!(spans[4].style.bg, None); // W4
    }

    #[test]
    fn test_build_tab_bar_filtered_all_idle() {
        let active_workers = vec![];
        let spans = build_tab_bar_filtered(&active_workers, None);

        // Should show single "All workers idle" span
        assert_eq!(spans.len(), 1);
        assert!(spans[0].content.contains("All workers idle"));
        assert_eq!(spans[0].style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn test_build_tab_bar_filtered_single_worker() {
        let active_workers = vec![2];
        let spans = build_tab_bar_filtered(&active_workers, Some(2));

        // Should have 1 worker * 2 spans = 2 spans
        assert_eq!(spans.len(), 2);
        assert!(spans[0].content.contains("W2"));
        assert_eq!(spans[0].style.bg, Some(Color::Cyan));
    }

    #[test]
    fn test_build_tab_bar_filtered_no_focus() {
        let active_workers = vec![1, 2];
        let spans = build_tab_bar_filtered(&active_workers, None);

        // Should have 2 workers * 2 spans each = 4 spans
        assert_eq!(spans.len(), 4);

        // None should be highlighted
        assert_eq!(spans[0].style.bg, None);
        assert_eq!(spans[2].style.bg, None);
    }

    // ── Snapshot tests ─────────────────────────────────────────────────

    /// Helper: builds a WorkerPanel and renders it via render_panel_widget + snap.
    /// Delegates to `render_panel_with_output` with empty output.
    fn render_panel_snapshot(
        status: WorkerStatus,
        worker_id: u32,
        is_focused: bool,
        idle_since: Option<std::time::Instant>,
        width: u16,
        height: u16,
    ) -> String {
        render_panel_with_output(
            status,
            worker_id,
            is_focused,
            idle_since,
            &[],
            width,
            height,
        )
    }

    /// Helper: builds a WorkerPanel with output lines and renders it.
    fn render_panel_with_output(
        status: WorkerStatus,
        worker_id: u32,
        is_focused: bool,
        idle_since: Option<std::time::Instant>,
        output_lines: &[&str],
        width: u16,
        height: u16,
    ) -> String {
        use crate::commands::task::orchestrate::ring_buffer::OutputRingBuffer;
        use crate::test_helpers::snap;
        use ratatui::{Terminal, backend::TestBackend, layout::Rect};

        let mut output = OutputRingBuffer::new(100);
        for line in output_lines {
            output.push(line);
        }

        let panel = WorkerPanel {
            worker_id,
            status,
            output,
            scroll_offset: 0,
            idle_since,
        };

        let area = Rect::new(0, 0, width, height);
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let widget = render_panel_widget(&panel, area, is_focused);
                frame.render_widget(widget, area);
            })
            .expect("draw");

        snap(terminal.backend().buffer())
    }

    /// Helper: creates a default WorkerStatus with common fields.
    fn make_status(state: WorkerState) -> WorkerStatus {
        WorkerStatus {
            state,
            phase: None,
            task_id: None,
            component: None,
            model: None,
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            verify_profiles: Vec::new(),
        }
    }

    // ── 1. Border type: focused (Double) vs unfocused (Rounded) ──────

    #[test]
    fn test_snapshot_panel_focused_border() {
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("2.1".into());
        status.component = Some("api".into());
        status.cost_usd = 0.042;
        status.input_tokens = 1500;
        status.output_tokens = 2300;

        let snapshot = render_panel_snapshot(status, 1, true, None, 50, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_panel_unfocused_border() {
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("2.1".into());
        status.component = Some("api".into());
        status.cost_usd = 0.042;
        status.input_tokens = 1500;
        status.output_tokens = 2300;

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 6);
        insta::assert_snapshot!(snapshot);
    }

    // ── 2–3. Title line variants ─────────────────────────────────────

    #[test]
    fn test_snapshot_title_with_component_and_model() {
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("3.2".into());
        status.component = Some("frontend".into());
        status.model = Some("claude-sonnet-4-5-20250929".into());
        status.cost_usd = 0.015;
        status.input_tokens = 800;
        status.output_tokens = 400;

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_title_no_component_no_model() {
        let mut status = make_status(WorkerState::Idle);
        status.task_id = None;
        status.component = None;
        status.model = None;

        let snapshot = render_panel_snapshot(status, 3, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── 4. Footer line ───────────────────────────────────────────────

    #[test]
    fn test_snapshot_footer_cost_tokens() {
        let mut status = make_status(WorkerState::Reviewing);
        status.task_id = Some("1.1".into());
        status.cost_usd = 0.042;
        status.input_tokens = 1500;
        status.output_tokens = 2300;

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── 5–8. Panel content with different WorkerStates ───────────────

    #[test]
    fn test_snapshot_state_implementing() {
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("5.1".into());
        status.phase = Some(super::super::events::WorkerPhase::Implement);

        let snapshot = render_panel_snapshot(status, 2, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_state_resolving_conflicts() {
        let mut status = make_status(WorkerState::ResolvingConflicts);
        status.task_id = Some("6.1".into());
        status.model = Some("claude-opus-4-6".into());

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_state_idle() {
        let status = make_status(WorkerState::Idle);

        let snapshot = render_panel_snapshot(status, 4, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_state_verifying() {
        let mut status = make_status(WorkerState::Verifying);
        status.task_id = Some("7.1".into());
        status.phase = Some(super::super::events::WorkerPhase::Verify);

        let snapshot = render_panel_snapshot(status, 3, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── 9. Grace period dimming ──────────────────────────────────────

    #[test]
    fn test_snapshot_grace_period_dimming() {
        let status = make_status(WorkerState::Idle);
        // idle_since = Some(Instant::now()) simulates grace period
        let idle_since = Some(std::time::Instant::now());

        let snapshot = render_panel_snapshot(status, 2, false, idle_since, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── 10. Profile status rows (verify phase) ──────────────────────

    #[test]
    fn test_snapshot_verify_profiles() {
        let mut status = make_status(WorkerState::Verifying);
        status.task_id = Some("8.1".into());
        status.phase = Some(super::super::events::WorkerPhase::Verify);
        status.verify_profiles = vec![
            ("lint".into(), Some(true)),
            ("test".into(), Some(false)),
            ("build".into(), None),
        ];

        let snapshot = render_panel_snapshot(status, 1, false, None, 60, 6);
        insta::assert_snapshot!(snapshot);
    }

    // ── 11. Panel with output buffer tail ────────────────────────────

    #[test]
    fn test_snapshot_panel_with_output_lines() {
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("9.1".into());
        status.component = Some("core".into());
        status.phase = Some(super::super::events::WorkerPhase::Implement);

        let output_lines = &[
            "Compiling ralph-wiggum v0.1.0",
            "  Running `target/debug/ralph`",
            "warning: unused variable `x`",
            "  --> src/main.rs:42:9",
            "Build completed successfully",
        ];

        let snapshot = render_panel_with_output(status, 1, false, None, output_lines, 55, 10);
        insta::assert_snapshot!(snapshot);
    }

    // ── Missing state snapshots: SettingUp, Reviewing, Merging ─────────

    #[test]
    fn test_snapshot_state_setting_up() {
        let mut status = make_status(WorkerState::SettingUp);
        status.task_id = Some("10.1".into());
        status.phase = Some(super::super::events::WorkerPhase::Setup);

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_state_reviewing() {
        let mut status = make_status(WorkerState::Reviewing);
        status.task_id = Some("11.1".into());
        status.phase = Some(super::super::events::WorkerPhase::ReviewFix);

        let snapshot = render_panel_snapshot(status, 2, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_state_merging() {
        let mut status = make_status(WorkerState::Merging);
        status.task_id = Some("12.1".into());

        let snapshot = render_panel_snapshot(status, 3, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── Focused panel with model ────────────────────────────────────────

    #[test]
    fn test_snapshot_focused_with_model() {
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("4.1".into());
        status.component = Some("cli".into());
        status.model = Some("claude-sonnet-4-5-20250929".into());
        status.cost_usd = 0.025;
        status.input_tokens = 3000;
        status.output_tokens = 1200;

        let snapshot = render_panel_snapshot(status, 1, true, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── Grace period rendering tests ─────────────────────────────────────

    #[test]
    fn test_build_border_style_grace_period() {
        // Test that grace period workers get dimmed border
        let (border_type, border_style) = build_border_style(false, &WorkerState::Idle, true);

        assert_eq!(border_type, BorderType::Rounded);
        assert_eq!(border_style.fg, Some(Color::Gray));
        assert!(border_style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn test_build_border_style_normal_idle() {
        // Test that normal idle workers (not in grace period) get DarkGray border
        let (border_type, border_style) = build_border_style(false, &WorkerState::Idle, false);

        assert_eq!(border_type, BorderType::Rounded);
        assert_eq!(border_style.fg, Some(Color::DarkGray));
        assert!(!border_style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn test_build_border_style_focused_overrides_grace_period() {
        // Test that focus takes precedence over grace period styling
        let (border_type, border_style) = build_border_style(true, &WorkerState::Idle, true);

        assert_eq!(border_type, BorderType::Double);
        assert_eq!(border_style.fg, Some(Color::Cyan));
        assert!(border_style.add_modifier.contains(Modifier::BOLD));
        assert!(!border_style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn test_build_border_style_active_worker_not_in_grace() {
        // Test that active (non-idle) workers ignore grace period flag
        let (border_type, border_style) =
            build_border_style(false, &WorkerState::Implementing, false);

        assert_eq!(border_type, BorderType::Rounded);
        assert_eq!(border_style.fg, Some(Color::Cyan));
        assert!(!border_style.add_modifier.contains(Modifier::DIM));
    }

    // ── Narrow terminal tests ────────────────────────────────────────────

    #[test]
    fn test_snapshot_narrow_width_20() {
        // Test minimal width (20) — title/footer truncation, bordering preserved
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("13.1".into());
        status.component = Some("backend".into());
        status.model = Some("claude-sonnet-4-5-20250929".into());
        status.cost_usd = 0.0123;
        status.input_tokens = 1234;
        status.output_tokens = 567;

        let output_lines = &[
            "Short line",
            "This is a much longer line that should wrap",
            "OK",
        ];

        let snapshot = render_panel_with_output(status, 1, false, None, output_lines, 20, 8);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_narrow_width_30() {
        // Test narrow but usable width (30) — better layout
        let mut status = make_status(WorkerState::Verifying);
        status.task_id = Some("14.2".into());
        status.component = Some("api".into());
        status.model = Some("claude-opus-4-6".into());
        status.cost_usd = 0.456;
        status.input_tokens = 5000;
        status.output_tokens = 3000;
        status.phase = Some(super::super::events::WorkerPhase::Verify);
        status.verify_profiles = vec![("lint".into(), Some(true)), ("test".into(), None)];

        let output_lines = &[
            "Running verification...",
            "Profile: lint — passed",
            "Profile: test — in progress",
        ];

        let snapshot = render_panel_with_output(status, 2, false, None, output_lines, 30, 10);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_minimal_height_3() {
        // Test minimal height (3) — border + 1 line content
        let mut status = make_status(WorkerState::Idle);
        status.task_id = None;
        status.cost_usd = 0.0;
        status.input_tokens = 0;
        status.output_tokens = 0;

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 3);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_narrow_width_20_focused() {
        // Test focused panel at minimal width
        let mut status = make_status(WorkerState::Reviewing);
        status.task_id = Some("15".into());
        status.component = Some("ui".into());
        status.cost_usd = 0.001;
        status.input_tokens = 100;
        status.output_tokens = 50;

        let snapshot = render_panel_snapshot(status, 3, true, None, 20, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_narrow_width_30_with_long_output() {
        // Test narrow panel with wrapping output lines
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("16.1".into());
        status.component = Some("core".into());

        let output_lines = &[
            "Line 1 normal",
            "This is a very long line that will definitely wrap in a 30-char wide terminal",
            "Short",
            "Another long line with many words that should cause wrapping behavior",
        ];

        let snapshot = render_panel_with_output(status, 1, false, None, output_lines, 30, 12);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_minimal_height_4() {
        // Test height=4 — border + status + 1 output line
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("17".into());
        status.phase = Some(super::super::events::WorkerPhase::Implement);

        let output_lines = &["First output line", "Second line (hidden)"];

        let snapshot = render_panel_with_output(status, 2, false, None, output_lines, 50, 4);
        insta::assert_snapshot!(snapshot);
    }

    // ── Unicode tests ─────────────────────────────────────────────────────

    #[test]
    fn test_snapshot_unicode_polish_task_id_component() {
        // Test Polish characters in task_id and component
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("zażółć".into());
        status.component = Some("środowisko".into());
        status.model = Some("claude-sonnet-4-5-20250929".into());
        status.cost_usd = 0.042;
        status.input_tokens = 1500;
        status.output_tokens = 2300;

        let snapshot = render_panel_snapshot(status, 1, false, None, 60, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_unicode_emoji_output() {
        // Test emoji in output lines
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("18.1".into());
        status.component = Some("core".into());

        let output_lines = &[
            "🚀 Starting build process",
            "✅ Tests passed successfully",
            "❌ Linter found issues",
            "⚠️  Warning: deprecated API",
        ];

        let snapshot = render_panel_with_output(status, 1, false, None, output_lines, 60, 10);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_unicode_cjk_output() {
        // Test CJK double-width characters in output
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("19.1".into());
        status.component = Some("i18n".into());

        let output_lines = &[
            "中文测试 Chinese test",
            "日本語テスト Japanese test",
            "한글 테스트 Korean test",
            "Mixed: 中文 and English",
        ];

        let snapshot = render_panel_with_output(status, 1, false, None, output_lines, 60, 10);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_unicode_mixed_polish_emoji_cjk() {
        // Test mixed Unicode: Polish + emoji + CJK
        let mut status = make_status(WorkerState::Verifying);
        status.task_id = Some("zażółć".into());
        status.component = Some("środowisko".into());
        status.phase = Some(super::super::events::WorkerPhase::Verify);

        let output_lines = &[
            "🚀 Uruchamianie testów",
            "✅ Test został zakończony pomyślnie",
            "中文: 测试通过",
            "❌ Błąd: nie znaleziono pliku",
        ];

        let snapshot = render_panel_with_output(status, 1, false, None, output_lines, 70, 10);
        insta::assert_snapshot!(snapshot);
    }

    // ── Extremely long task_id and component tests ────────────────────────

    #[test]
    fn test_snapshot_extremely_long_task_id_width_60() {
        // Test task_id with 100 characters on width=60
        // Title line should be truncated to fit panel width
        let mut status = make_status(WorkerState::Implementing);
        // 100-char task_id
        status.task_id = Some(
            "1.2.3.4.5.6.7.8.9.10.11.12.13.14.15.16.17.18.19.20.21.22.23.24.25.26.27.28.29.30.31.32.33.34.35.36.37.38.39.40".into()
        );
        status.component = Some("api".into());
        status.model = Some("claude-sonnet-4-5-20250929".into());
        status.cost_usd = 0.042;
        status.input_tokens = 1500;
        status.output_tokens = 2300;

        let snapshot = render_panel_snapshot(status, 1, false, None, 60, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_extremely_long_component_width_40() {
        // Test component with 50 characters on width=40
        // Title line should be truncated to fit panel width
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("2.1".into());
        // 50-char component
        status.component = Some("authentication-and-authorization-backend-service".into());
        status.model = Some("claude-opus-4-6".into());
        status.cost_usd = 0.123;
        status.input_tokens = 2000;
        status.output_tokens = 1000;

        let snapshot = render_panel_snapshot(status, 1, false, None, 40, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_extremely_high_cost_footer() {
        // Test footer with extremely high cost ($999999.99)
        // Footer should display large cost without panic
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("3.1".into());
        status.component = Some("core".into());
        status.cost_usd = 999999.99;
        status.input_tokens = 999_999_999;
        status.output_tokens = 999_999_999;

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_extremely_long_both_task_id_and_component() {
        // Test both task_id and component extremely long
        // Should verify truncation behavior with both long values
        let mut status = make_status(WorkerState::Reviewing);
        // 100-char task_id
        status.task_id = Some(
            "feature.authentication.oauth2.implementation.backend.api.endpoints.v2.user.profile.settings.update.handler".into()
        );
        // 50-char component
        status.component = Some("authentication-backend-microservice-orchestrator".into());
        status.model = Some("claude-sonnet-4-5-20250929".into());
        status.cost_usd = 12.3456;
        status.input_tokens = 50_000;
        status.output_tokens = 25_000;

        let snapshot = render_panel_snapshot(status, 1, false, None, 60, 6);
        insta::assert_snapshot!(snapshot);
    }

    // ── Empty/minimal data tests ──────────────────────────────────────────

    #[test]
    fn test_snapshot_empty_worker_with_task_id_no_component_no_model() {
        // Test worker with task_id but without component and model
        // Should render task_id without component or model suffix
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("42.1".into());
        status.component = None;
        status.model = None;
        status.cost_usd = 0.0;
        status.input_tokens = 0;
        status.output_tokens = 0;

        let snapshot = render_panel_snapshot(status, 2, false, None, 50, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_empty_worker_zero_cost_zero_tokens() {
        // Test worker with cost=0.0 and tokens=0/0
        // Footer should display $0.0000 and ↓0 ↑0 without errors
        let mut status = make_status(WorkerState::Idle);
        status.task_id = Some("1.1".into());
        status.component = Some("test".into());

        let snapshot = render_panel_snapshot(status, 3, false, None, 50, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_empty_worker_empty_output_buffer() {
        // Test worker with empty output buffer
        // Should render only status line, no output lines
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("5.2".into());
        status.component = Some("core".into());
        status.phase = Some(super::super::events::WorkerPhase::Implement);

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 8);
        insta::assert_snapshot!(snapshot);
    }
}
