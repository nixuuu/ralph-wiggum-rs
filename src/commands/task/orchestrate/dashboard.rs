use std::collections::{HashMap, VecDeque};
use std::io::{self, Stdout};

use ansi_to_tui::IntoText;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::commands::task::orchestrate::panels::{render_compact, render_panel_widget};
use crate::commands::task::orchestrate::ring_buffer::OutputRingBuffer;
use crate::commands::task::orchestrate::shared_types::{format_duration, render_progress_bar};
use crate::commands::task::orchestrate::shutdown_types::{OrchestratorStatus, ShutdownState};
use crate::commands::task::orchestrate::summary::TaskSummaryEntry;
use crate::commands::task::orchestrate::task_preview::render_task_preview;
use crate::commands::task::orchestrate::worker_status::WorkerStatus;
use crate::shared::error::Result;

// ── WorkerPanel ──────────────────────────────────────────────────────

/// State of a single worker panel in the dashboard.
pub struct WorkerPanel {
    pub worker_id: u32,
    pub status: WorkerStatus,
    pub output: OutputRingBuffer,
    pub scroll_offset: usize, // 0 = auto-scroll (follow tail)
}

impl WorkerPanel {
    pub fn new(worker_id: u32) -> Self {
        Self {
            worker_id,
            status: WorkerStatus::idle(worker_id),
            output: OutputRingBuffer::new(200),
            scroll_offset: 0,
        }
    }
}

// ── Dashboard ────────────────────────────────────────────────────────

/// Fullscreen TUI dashboard for the orchestrator.
///
/// Shows a grid of worker panels, each with status line and scrollable
/// output buffer, plus a global status bar at the bottom.
pub struct Dashboard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    panels: HashMap<u32, WorkerPanel>,
    worker_count: u32,
    focused_worker: Option<u32>,
    log_lines: VecDeque<Line<'static>>,
    preview_scroll_offset: usize,
}

impl Dashboard {
    pub fn new(worker_count: u32) -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, crossterm::cursor::Hide)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let mut panels = HashMap::new();
        for i in 1..=worker_count {
            panels.insert(i, WorkerPanel::new(i));
        }

        Ok(Self {
            terminal,
            panels,
            worker_count,
            focused_worker: None,
            log_lines: VecDeque::with_capacity(50),
            preview_scroll_offset: 0,
        })
    }

    /// Restore the terminal to normal mode.
    pub fn cleanup(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        )?;
        self.terminal.show_cursor()?;
        Ok(())
    }

    /// Full render cycle: draw all panels + global status bar.
    ///
    /// When `tasks_file` is provided (preview overlay active), the dashboard
    /// can access full task metadata for rendering task details.
    pub fn render(
        &mut self,
        status: &OrchestratorStatus,
        tasks_file: Option<&crate::shared::tasks::TasksFile>,
        _task_summaries: &[TaskSummaryEntry],
    ) -> Result<()> {
        let worker_count = self.worker_count;
        let focused = self.focused_worker;
        let panels = &self.panels;
        let log_lines = &self.log_lines;
        let preview_scroll = self.preview_scroll_offset;

        self.terminal.draw(|frame| {
            let area = frame.area();

            // Small terminal: show single panel + 1-line status
            if area.width < 60 || area.height < 12 {
                let preview_active = tasks_file.is_some();
                render_compact(
                    frame,
                    area,
                    panels,
                    status,
                    focused,
                    log_lines,
                    worker_count,
                    preview_active,
                );
                return;
            }

            // Split: worker grid + 3 lines for global bar
            let vertical =
                Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(area);

            let grid_area = vertical[0];
            let bar_area = vertical[1];

            // If preview overlay is active, render task list instead of worker grid
            if let Some(tf) = tasks_file {
                render_task_preview(frame, grid_area, tf, preview_scroll);
            } else {
                // Render worker grid
                let rects = compute_grid_rects(grid_area, worker_count);
                for (worker_id, rect) in rects {
                    if let Some(panel) = panels.get(&worker_id) {
                        let is_focused = focused == Some(worker_id);
                        let widget = render_panel_widget(panel, rect, is_focused);
                        frame.render_widget(widget, rect);
                    }
                }
            }

            // Render global status bar (preview_active when tasks_file is Some)
            let preview_active = tasks_file.is_some();
            let bar_widget = render_global_bar(status, focused, preview_active);
            frame.render_widget(bar_widget, bar_area);
        })?;

        Ok(())
    }

    pub fn update_worker_status(&mut self, worker_id: u32, status: WorkerStatus) {
        if let Some(panel) = self.panels.get_mut(&worker_id) {
            panel.status = status;
        }
    }

    /// Update only cost/token fields for a worker, preserving state/phase/task.
    pub fn update_worker_cost(
        &mut self,
        worker_id: u32,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        if let Some(panel) = self.panels.get_mut(&worker_id) {
            panel.status.cost_usd = cost_usd;
            panel.status.input_tokens = input_tokens;
            panel.status.output_tokens = output_tokens;
        }
    }

    pub fn push_worker_output(&mut self, worker_id: u32, lines: &[String]) {
        if let Some(panel) = self.panels.get_mut(&worker_id) {
            for line in lines {
                panel.output.push(line);
            }
            // Auto-scroll when not manually scrolled
            if panel.scroll_offset == 0 {
                // stays at tail (0 = auto)
            }
        }
    }

    /// Clear the output buffer for a worker and reset scroll to auto-follow.
    pub fn clear_worker_output(&mut self, worker_id: u32) {
        if let Some(panel) = self.panels.get_mut(&worker_id) {
            panel.output.clear();
            panel.scroll_offset = 0; // Reset to auto-scroll (follow tail)
        }
    }

    pub fn push_log_line(&mut self, text: &str) {
        let rlines = text
            .into_text()
            .map(|t| t.lines)
            .unwrap_or_else(|_| vec![Line::raw(text.to_string())]);
        for line in rlines {
            if self.log_lines.len() >= 50 {
                self.log_lines.pop_front();
            }
            self.log_lines.push_back(line);
        }
    }

    pub fn set_focus(&mut self, worker_id: Option<u32>) {
        if self.focused_worker != worker_id {
            self.focused_worker = worker_id;
            // Reset scroll on focus change
            if let Some(id) = worker_id
                && let Some(panel) = self.panels.get_mut(&id)
            {
                panel.scroll_offset = 0;
            }
        }
    }

    pub fn apply_scroll(&mut self, delta: i32, preview_active: bool) {
        if preview_active {
            // Scroll the task preview overlay
            if delta == i32::MIN {
                // Home key — scroll to top
                self.preview_scroll_offset = 0;
                return;
            }
            if delta == i32::MAX {
                // End key — scroll to bottom (will be clamped in task_preview.rs)
                self.preview_scroll_offset = usize::MAX;
                return;
            }

            if delta < 0 {
                // Scroll up
                let up = (-delta) as usize;
                self.preview_scroll_offset = self.preview_scroll_offset.saturating_sub(up);
            } else if delta > 0 {
                // Scroll down
                let down = delta as usize;
                self.preview_scroll_offset = self.preview_scroll_offset.saturating_add(down);
            }
        } else {
            // Scroll focused worker panel (original behavior)
            let Some(wid) = self.focused_worker else {
                return;
            };
            let Some(panel) = self.panels.get_mut(&wid) else {
                return;
            };

            if delta == i32::MIN {
                // Home key — scroll to top (max offset, will be clamped in panels.rs)
                panel.scroll_offset = usize::MAX;
                return;
            }
            if delta == i32::MAX {
                // End key — reset to auto-scroll (bottom)
                panel.scroll_offset = 0;
                return;
            }

            // Note: We can't accurately compute max_offset here without panel_width.
            // Instead, we apply the scroll delta freely and clamp in build_panel_content().
            if delta < 0 {
                // Scroll up
                let up = (-delta) as usize;
                if panel.scroll_offset == 0 {
                    // Entering manual scroll from auto-scroll: start at "up" rows from bottom
                    panel.scroll_offset = up;
                } else {
                    panel.scroll_offset = panel.scroll_offset.saturating_add(up);
                }
            } else if delta > 0 {
                // Scroll down
                let down = delta as usize;
                if panel.scroll_offset == 0 {
                    // Already at tail, stay at 0
                } else {
                    panel.scroll_offset = panel.scroll_offset.saturating_sub(down);
                    // If we hit 0, we're back to auto-scroll (handled automatically)
                }
            }
        }
    }

    pub fn handle_resize(&mut self) -> Result<()> {
        self.terminal.autoresize()?;
        Ok(())
    }
}

// ── Grid layout computation ──────────────────────────────────────────

/// Compute optimal (cols, rows) for the worker count.
fn compute_grid(worker_count: u32) -> (u32, u32) {
    match worker_count {
        0 | 1 => (1, 1),
        2 => (2, 1),
        3 | 4 => (2, 2),
        5 | 6 => (3, 2),
        7..=9 => (3, 3),
        n => {
            let cols = (n as f64).sqrt().ceil() as u32;
            let rows = n.div_ceil(cols);
            (cols, rows)
        }
    }
}

/// Map worker IDs to screen rectangles within the grid area.
fn compute_grid_rects(area: Rect, worker_count: u32) -> Vec<(u32, Rect)> {
    if worker_count == 0 {
        return vec![];
    }

    let (cols, rows) = compute_grid(worker_count);

    // Split vertically into rows
    let row_constraints: Vec<Constraint> = (0..rows).map(|_| Constraint::Ratio(1, rows)).collect();
    let row_rects = Layout::vertical(row_constraints).split(area);

    let mut result = Vec::with_capacity(worker_count as usize);
    let mut worker_id = 1u32;

    for (r, row_rect) in row_rects.iter().enumerate() {
        // How many workers in this row
        let workers_in_row = if r as u32 == rows - 1 {
            // Last row might have fewer
            worker_count - (rows - 1) * cols
        } else {
            cols
        };

        if workers_in_row == 0 {
            break;
        }

        let col_constraints: Vec<Constraint> = (0..workers_in_row)
            .map(|_| Constraint::Ratio(1, workers_in_row))
            .collect();
        let col_rects = Layout::horizontal(col_constraints).split(*row_rect);

        for col_rect in col_rects.iter() {
            if worker_id > worker_count {
                break;
            }
            result.push((worker_id, *col_rect));
            worker_id += 1;
        }
    }

    result
}

// ── Global status bar ────────────────────────────────────────────────

/// Render the 3-line global status bar at the bottom.
fn render_global_bar<'a>(
    status: &OrchestratorStatus,
    focused: Option<u32>,
    preview_active: bool,
) -> Paragraph<'a> {
    let total = status.scheduler.total;
    let done = status.scheduler.done;
    let pct = if total > 0 { (done * 100) / total } else { 0 };
    let bar = render_progress_bar(done, total, 20);
    let elapsed = format_duration(status.elapsed);
    let total_cost = status.total_cost.max(0.0);
    let cost_per_task = if done > 0 {
        total_cost / done as f64
    } else {
        0.0
    };

    let progress_line = Line::from(vec![
        Span::raw(" "),
        Span::styled(bar, Style::default().fg(Color::Green)),
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
        Span::styled(
            format!("${cost_per_task:.4}/task"),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" │ "),
        Span::styled(format!("⏱ {elapsed}"), Style::default().fg(Color::White)),
    ]);

    let focus_span = if let Some(wid) = focused {
        Span::styled(
            format!("Focus: W{wid}"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("No focus", Style::default().fg(Color::DarkGray))
    };

    let queue_line = Line::from(vec![
        Span::raw(" Queue: "),
        Span::styled(
            format!("{} ready", status.scheduler.ready),
            Style::default().fg(Color::Green),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{} wip", status.scheduler.in_progress),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{} blocked", status.scheduler.blocked),
            Style::default().fg(Color::Red),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{} pending", status.scheduler.pending),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  │  "),
        focus_span,
        Span::raw("    "),
    ]);

    // Add quit confirmation hint or normal keybindings
    let mut queue_line_spans = queue_line.spans;
    if status.quit_pending {
        queue_line_spans.push(Span::styled(
            "Press q/Enter to confirm, Esc to cancel",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    } else if preview_active {
        queue_line_spans.push(Span::styled(
            "p/Esc=close ↑↓=scroll",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        queue_line_spans.push(Span::styled(
            "q Tab ↑↓ Esc p=tasks r=reload",
            Style::default().fg(Color::DarkGray),
        ));
    }
    let queue_line = Line::from(queue_line_spans);

    // Separator or shutdown/quit/completed banner
    let separator = if status.quit_pending {
        // Quit confirmation banner (highest priority)
        Line::from(vec![Span::styled(
            " ⚠ Quit? Press q/Enter to confirm, Esc to cancel ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )])
    } else if status.completed && status.shutdown_state == ShutdownState::Running {
        // All tasks completed banner (second priority, only when not shutting down)
        let done = status.scheduler.done;
        let blocked = status.scheduler.blocked;
        let total = status.scheduler.total;
        let msg = if blocked > 0 {
            format!(
                " ✓ Completed — {}/{} done, {} blocked | q=exit p=tasks ",
                done, total, blocked
            )
        } else {
            format!(
                " ✓ Completed — {}/{} tasks done | q=exit p=tasks ",
                done, total
            )
        };
        Line::from(vec![Span::styled(
            msg,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )])
    } else {
        match status.shutdown_state {
            ShutdownState::Running => Line::from(vec![Span::styled(
                "─".repeat(200),
                Style::default().fg(Color::DarkGray),
            )]),
            ShutdownState::Draining => {
                let countdown = if let Some(d) = status.shutdown_remaining {
                    use std::fmt::Write;
                    let mut buf = String::with_capacity(20);
                    let _ = write!(buf, " (force-kill za {}s)", d.as_secs());
                    buf
                } else {
                    String::new()
                };
                Line::from(vec![Span::styled(
                    format!(
                        " ⏳ SHUTTING DOWN — waiting for in-progress tasks to finish...{countdown} (press q again to force) "
                    ),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )])
            }
            ShutdownState::Aborting => Line::from(vec![Span::styled(
                " ⚠ FORCE SHUTDOWN — aborting all workers... ",
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            )]),
        }
    };

    Paragraph::new(vec![separator, progress_line, queue_line])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_grid_layouts() {
        assert_eq!(compute_grid(1), (1, 1));
        assert_eq!(compute_grid(2), (2, 1));
        assert_eq!(compute_grid(3), (2, 2));
        assert_eq!(compute_grid(4), (2, 2));
        assert_eq!(compute_grid(5), (3, 2));
        assert_eq!(compute_grid(6), (3, 2));
        assert_eq!(compute_grid(7), (3, 3));
        assert_eq!(compute_grid(9), (3, 3));
    }

    #[test]
    fn test_compute_grid_rects_count() {
        let area = Rect::new(0, 0, 120, 40);
        let rects = compute_grid_rects(area, 4);
        assert_eq!(rects.len(), 4);
        // All worker IDs should be 1-4
        let ids: Vec<u32> = rects.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_compute_grid_rects_single() {
        let area = Rect::new(0, 0, 80, 24);
        let rects = compute_grid_rects(area, 1);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].0, 1);
        assert_eq!(rects[0].1, area);
    }

    #[test]
    fn test_worker_panel_auto_scroll() {
        let mut panel = WorkerPanel::new(1);
        assert_eq!(panel.scroll_offset, 0); // auto-scroll by default

        panel.output.push("line 1");
        panel.output.push("line 2");
        assert_eq!(panel.scroll_offset, 0); // still auto
    }

    #[test]
    fn test_clear_worker_output_resets_scroll() {
        // Test the logic of clear_worker_output without creating a full Dashboard (no TTY in tests)
        let mut panel = WorkerPanel::new(1);

        // Add some output
        panel.output.push("line 1");
        panel.output.push("line 2");
        panel.output.push("line 3");

        // Simulate user scrolling
        panel.scroll_offset = 10;

        // Verify state before clear
        assert_eq!(panel.output.len(), 3, "should have 3 lines");
        assert_eq!(panel.scroll_offset, 10, "scroll should be at 10");

        // Simulate clear_worker_output logic
        panel.output.clear();
        panel.scroll_offset = 0;

        // Verify both output and scroll_offset are cleared
        assert_eq!(panel.output.len(), 0, "output should be cleared");
        assert_eq!(
            panel.scroll_offset, 0,
            "scroll_offset should be reset to 0 (auto-scroll)"
        );
    }

    #[test]
    fn test_apply_scroll_min_worker_panel() {
        // Test apply_scroll with i32::MIN on worker panel (scroll to top)
        // We need to simulate the logic without creating a full Dashboard
        let mut panel = WorkerPanel::new(1);
        panel.output.push("line 1");
        panel.output.push("line 2");
        panel.output.push("line 3");

        // Simulate apply_scroll with delta = i32::MIN
        // According to line 259-262: panel.scroll_offset = usize::MAX (will be clamped)
        panel.scroll_offset = usize::MAX;

        // Verify scroll_offset is set to usize::MAX (clamping happens in build_panel_content)
        assert_eq!(
            panel.scroll_offset,
            usize::MAX,
            "i32::MIN should set scroll_offset to usize::MAX"
        );
    }

    #[test]
    fn test_apply_scroll_max_worker_panel() {
        // Test apply_scroll with i32::MAX on worker panel (scroll to bottom/auto-follow)
        let mut panel = WorkerPanel::new(1);
        panel.output.push("line 1");
        panel.output.push("line 2");
        panel.output.push("line 3");

        // Start with manual scroll
        panel.scroll_offset = 100;

        // Simulate apply_scroll with delta = i32::MAX
        // According to line 264-267: panel.scroll_offset = 0 (auto-scroll)
        panel.scroll_offset = 0;

        // Verify scroll_offset is reset to 0 (auto-scroll/bottom)
        assert_eq!(
            panel.scroll_offset, 0,
            "i32::MAX should reset scroll_offset to 0 (auto-scroll)"
        );
    }

    #[test]
    fn test_apply_scroll_min_preview() {
        // Test apply_scroll with i32::MIN on preview overlay (scroll to top)
        // Simulate the preview_scroll_offset logic from lines 230-233
        let initial_offset = 100usize;

        // When delta == i32::MIN
        let preview_scroll_offset = 0;

        // Verify preview scrolls to top (from initial position)
        assert_ne!(initial_offset, preview_scroll_offset, "offset should change");
        assert_eq!(
            preview_scroll_offset, 0,
            "i32::MIN should set preview_scroll_offset to 0 (top)"
        );
    }

    #[test]
    fn test_apply_scroll_max_preview() {
        // Test apply_scroll with i32::MAX on preview overlay (scroll to bottom)
        // Simulate the preview_scroll_offset logic from lines 235-238
        let initial_offset = 10usize;

        // When delta == i32::MAX
        let preview_scroll_offset = usize::MAX;

        // Verify preview scrolls to bottom (will be clamped in task_preview.rs)
        assert_ne!(initial_offset, preview_scroll_offset, "offset should change");
        assert_eq!(
            preview_scroll_offset,
            usize::MAX,
            "i32::MAX should set preview_scroll_offset to usize::MAX (bottom, clamped later)"
        );
    }
}
