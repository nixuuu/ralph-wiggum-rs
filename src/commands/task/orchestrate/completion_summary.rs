#![allow(dead_code)]

use std::time::Duration;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::commands::task::orchestrate::shared_types::format_duration;
use crate::commands::task::orchestrate::summary::TaskSummaryEntry;

// Column widths for the summary table
const STATUS_W: usize = 8;
const COST_W: usize = 10;
const TIME_W: usize = 8;
const RETRIES_W: usize = 7;

/// Render completion summary panel showing all tasks complete.
///
/// Shows:
/// - Green "All tasks complete" header with checkmark
/// - Summary table: task_id | status | cost | duration | retries
/// - Total stats row: X/Y done, total cost, total time
/// - Parallelism speedup metric
/// - Hint: "Press q to exit, p to view tasks"
pub fn render_completion_summary(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    entries: &[TaskSummaryEntry],
    wall_clock: Duration,
) {
    let mut lines = Vec::new();

    add_header(&mut lines);

    if entries.is_empty() {
        lines.push(Line::from("No tasks were executed."));
    } else {
        render_stats_section(&mut lines, entries, wall_clock);
    }

    add_footer(&mut lines);

    let block = build_summary_block();
    let widget = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, area);
}

/// Add header with checkmark.
fn add_header(lines: &mut Vec<Line<'static>>) {
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "✓ All tasks complete",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));
}

/// Add footer hint.
fn add_footer(lines: &mut Vec<Line<'static>>) {
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Press ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "q",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to exit, ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "p",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to view tasks", Style::default().fg(Color::DarkGray)),
    ]));
}

/// Build block with green border.
fn build_summary_block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
        .title(Span::styled(
            " Completion Summary ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ))
}

/// Render stats section: table + totals + metrics.
fn render_stats_section(
    lines: &mut Vec<Line<'static>>,
    entries: &[TaskSummaryEntry],
    wall_clock: Duration,
) {
    // Calculate totals
    let total_cost: f64 = entries.iter().map(|e| e.cost_usd).sum();
    let total_time: Duration = entries.iter().map(|e| e.duration).sum();
    let done_count = entries.iter().filter(|e| e.status == "Done").count();
    let total_count = entries.len();

    // Column widths
    let task_w = entries
        .iter()
        .map(|e| e.task_id.len())
        .max()
        .unwrap_or(4)
        .max(5);

    render_task_table(lines, entries, task_w);
    render_totals_row(
        lines,
        task_w,
        done_count,
        total_count,
        total_cost,
        wall_clock,
    );
    render_metrics(lines, total_time, wall_clock);
}

/// Render task table: header + separator + rows.
fn render_task_table(lines: &mut Vec<Line<'static>>, entries: &[TaskSummaryEntry], task_w: usize) {
    // Header row
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<task_w$}", "Task"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{:<STATUS_W$}", "Status"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{:<COST_W$}", "Cost"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{:<TIME_W$}", "Time"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{:<RETRIES_W$}", "Retries"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]));

    // Separator
    let sep_len = 2 + task_w + 3 + STATUS_W + 3 + COST_W + 3 + TIME_W + 3 + RETRIES_W;
    lines.push(Line::from(vec![Span::styled(
        "─".repeat(sep_len),
        Style::default().fg(Color::DarkGray),
    )]));

    // Task rows
    for entry in entries {
        lines.push(render_task_row(entry, task_w));
    }
}

/// Render a single task row.
fn render_task_row(entry: &TaskSummaryEntry, task_w: usize) -> Line<'static> {
    let time_str = format_duration(entry.duration);
    let cost_str = format!("${:.4}", entry.cost_usd);
    let status_color = if entry.status == "Done" {
        Color::Green
    } else if entry.status == "Blocked" {
        Color::Red
    } else {
        Color::Yellow
    };

    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<task_w$}", entry.task_id),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{:<STATUS_W$}", entry.status),
            Style::default().fg(status_color),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{:<COST_W$}", cost_str),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{:<TIME_W$}", time_str),
            Style::default().fg(Color::White),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{:<RETRIES_W$}", entry.retries),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

/// Render totals row at the bottom of the table.
fn render_totals_row(
    lines: &mut Vec<Line<'static>>,
    task_w: usize,
    done_count: usize,
    total_count: usize,
    total_cost: f64,
    wall_clock: Duration,
) {
    let sep_len = 2 + task_w + 3 + STATUS_W + 3 + COST_W + 3 + TIME_W + 3 + RETRIES_W;

    // Separator
    lines.push(Line::from(vec![Span::styled(
        "─".repeat(sep_len),
        Style::default().fg(Color::DarkGray),
    )]));

    // Build formatted strings
    use std::fmt::Write;
    let mut status_total = String::with_capacity(16);
    let _ = write!(status_total, "{done_count}/{total_count} done");
    let mut cost_total = String::with_capacity(12);
    let _ = write!(cost_total, "${total_cost:.4}");
    let time_total = format_duration(wall_clock);

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<task_w$}", "TOTAL"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{:<STATUS_W$}", status_total),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{:<COST_W$}", cost_total),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{:<TIME_W$}", time_total),
            Style::default().fg(Color::White),
        ),
        Span::raw(" │ "),
        Span::raw(format!("{:<RETRIES_W$}", "")),
    ]));
}

/// Render parallelism speedup metric.
fn render_metrics(lines: &mut Vec<Line<'static>>, total_time: Duration, wall_clock: Duration) {
    let speedup = if wall_clock.as_secs_f64() > 0.0 {
        total_time.as_secs_f64() / wall_clock.as_secs_f64()
    } else {
        1.0
    };

    lines.push(Line::from(""));

    let mut speedup_text = String::with_capacity(32);
    use std::fmt::Write;
    let _ = write!(speedup_text, "Parallelism speedup: {speedup:.1}x");

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(speedup_text, Style::default().fg(Color::Cyan)),
        Span::styled(
            " (sum of task times / wall clock)",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
}
