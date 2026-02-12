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

    // Build border style
    let (border_type, border_style) = build_border_style(is_focused, &ws.state);

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

/// Build the border style (type, border style) based on focus and state.
fn build_border_style(is_focused: bool, state: &WorkerState) -> (BorderType, Style) {
    let (border_type, border_color) = if is_focused {
        (BorderType::Double, Color::Cyan)
    } else {
        (BorderType::Rounded, state_color(state))
    };

    let border_style = if is_focused {
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD)
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

    // First line: phase icon + name
    let (icon, icon_color) = state_icon(&ws.state);
    let phase_str = ws
        .phase
        .as_ref()
        .map(|p| p.to_string())
        .unwrap_or_else(|| ws.state.to_string());

    let status_line = Line::from(vec![
        Span::styled(icon.to_string(), Style::default().fg(icon_color)),
        Span::raw(" "),
        Span::styled(phase_str, Style::default().fg(icon_color)),
    ]);

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

/// Get color for a worker state.
fn state_color(state: &WorkerState) -> Color {
    match state {
        WorkerState::Idle => Color::DarkGray,
        WorkerState::SettingUp => Color::Blue,
        WorkerState::Implementing => Color::Cyan,
        WorkerState::Reviewing => Color::Yellow,
        WorkerState::Verifying => Color::Magenta,
        WorkerState::Merging => Color::Green,
        WorkerState::ResolvingConflicts => Color::Red,
    }
}

/// Get icon and color for a worker state.
fn state_icon(state: &WorkerState) -> (&'static str, Color) {
    match state {
        WorkerState::Idle => ("○", Color::DarkGray),
        WorkerState::SettingUp => ("⚙", Color::Blue),
        WorkerState::Implementing => ("●", Color::Cyan),
        WorkerState::Reviewing => ("◎", Color::Yellow),
        WorkerState::Verifying => ("◉", Color::Magenta),
        WorkerState::Merging => ("⊕", Color::Green),
        WorkerState::ResolvingConflicts => ("⚡", Color::Red),
    }
}

// ── Compact render ───────────────────────────────────────────────────

/// Compact render for small terminals — single panel + tab bar.
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

    // Tab bar
    let tab_spans = build_tab_bar(worker_count, focused);
    frame.render_widget(Line::from(tab_spans), tab_area);

    // Show focused panel (or W1)
    let show_id = focused.unwrap_or(1);
    if let Some(panel) = panels.get(&show_id) {
        let widget = render_panel_widget(panel, panel_area, true);
        frame.render_widget(widget, panel_area);
    }

    // Compact status bar (1 line)
    let compact_bar = build_compact_bar(status, preview_active);
    frame.render_widget(compact_bar, bar_area);
}

/// Build tab bar spans for worker selection.
fn build_tab_bar(worker_count: u32, focused: Option<u32>) -> Vec<Span<'static>> {
    let mut tab_spans = Vec::new();
    for i in 1..=worker_count {
        let is_active = focused == Some(i) || (focused.is_none() && i == 1);
        let style = if is_active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        tab_spans.push(Span::styled(format!(" W{i} "), style));
        tab_spans.push(Span::raw(" "));
    }
    tab_spans
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
    fn test_state_color_mapping() {
        assert_eq!(state_color(&WorkerState::Idle), Color::DarkGray);
        assert_eq!(state_color(&WorkerState::SettingUp), Color::Blue);
        assert_eq!(state_color(&WorkerState::Implementing), Color::Cyan);
        assert_eq!(state_color(&WorkerState::Reviewing), Color::Yellow);
        assert_eq!(state_color(&WorkerState::Verifying), Color::Magenta);
        assert_eq!(state_color(&WorkerState::Merging), Color::Green);
        assert_eq!(state_color(&WorkerState::ResolvingConflicts), Color::Red);
    }

    #[test]
    fn test_state_icon_mapping() {
        let (icon, _) = state_icon(&WorkerState::Idle);
        assert_eq!(icon, "○");
        let (icon, color) = state_icon(&WorkerState::SettingUp);
        assert_eq!(icon, "⚙");
        assert_eq!(color, Color::Blue);
        let (icon, _) = state_icon(&WorkerState::Implementing);
        assert_eq!(icon, "●");
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
            output_tokens: 0,
        };

        let panel = WorkerPanel {
            worker_id: 1,
            status,
            output,
            scroll_offset: 1000, // Deliberately excessive offset
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
    fn test_build_tab_bar() {
        let spans = build_tab_bar(3, Some(2));

        // Should have 3 workers * 2 spans each = 6 spans
        assert_eq!(spans.len(), 6);

        // W2 should be highlighted (focused)
        let w2_text = spans[2].content.as_ref();
        assert!(w2_text.contains("W2"));
        assert_eq!(spans[2].style.bg, Some(Color::Cyan));
    }
}
