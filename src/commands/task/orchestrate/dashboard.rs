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
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::commands::task::orchestrate::status::{
    OrchestratorStatus, ShutdownState, WorkerState, WorkerStatus, format_duration, format_tokens,
    render_progress_bar,
};
use crate::shared::error::Result;

// ── OutputRingBuffer ─────────────────────────────────────────────────

/// Ring buffer holding last N lines of worker output as ratatui Lines.
pub struct OutputRingBuffer {
    lines: VecDeque<Line<'static>>,
    capacity: usize,
}

impl OutputRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Push text (potentially with ANSI codes) as one or more Lines.
    pub fn push(&mut self, text: &str) {
        // Convert ANSI→ratatui; fallback to plain text on error
        let rlines = text
            .into_text()
            .map(|t| t.lines)
            .unwrap_or_else(|_| vec![Line::raw(text.to_string())]);

        for line in rlines {
            if self.lines.len() >= self.capacity {
                self.lines.pop_front();
            }
            self.lines.push_back(line);
        }
    }

    /// Get the last `n` lines for rendering.
    pub fn tail(&self, n: usize) -> Vec<Line<'static>> {
        let skip = self.lines.len().saturating_sub(n);
        self.lines.iter().skip(skip).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }
}

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
    pub fn render(&mut self, status: &OrchestratorStatus) -> Result<()> {
        let worker_count = self.worker_count;
        let focused = self.focused_worker;
        let panels = &self.panels;
        let log_lines = &self.log_lines;

        self.terminal.draw(|frame| {
            let area = frame.area();

            // Small terminal: show single panel + 1-line status
            if area.width < 60 || area.height < 12 {
                Self::render_compact(frame, area, panels, status, focused, log_lines, worker_count);
                return;
            }

            // Split: worker grid + 3 lines for global bar
            let vertical = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .split(area);

            let grid_area = vertical[0];
            let bar_area = vertical[1];

            // Render worker grid
            let rects = compute_grid_rects(grid_area, worker_count);
            for (worker_id, rect) in rects {
                if let Some(panel) = panels.get(&worker_id) {
                    let is_focused = focused == Some(worker_id);
                    let widget = render_panel_widget(panel, rect, is_focused);
                    frame.render_widget(widget, rect);
                }
            }

            // Render global status bar
            let bar_widget = render_global_bar(status, focused);
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

    pub fn apply_scroll(&mut self, delta: i32) {
        let Some(wid) = self.focused_worker else {
            return;
        };
        let Some(panel) = self.panels.get_mut(&wid) else {
            return;
        };

        if delta == i32::MAX {
            // End key — reset to auto-scroll
            panel.scroll_offset = 0;
            return;
        }

        let max_offset = panel.output.len();
        if delta < 0 {
            // Scroll up
            let up = (-delta) as usize;
            if panel.scroll_offset == 0 {
                // Entering manual scroll from auto-scroll
                panel.scroll_offset = max_offset.saturating_sub(up);
            } else {
                panel.scroll_offset = panel.scroll_offset.saturating_sub(up);
            }
        } else if delta > 0 {
            // Scroll down
            let down = delta as usize;
            if panel.scroll_offset == 0 {
                // Already at tail, stay
            } else {
                panel.scroll_offset += down;
                if panel.scroll_offset >= max_offset {
                    panel.scroll_offset = 0; // Back to auto-scroll
                }
            }
        }
    }

    pub fn handle_resize(&mut self) -> Result<()> {
        self.terminal.autoresize()?;
        Ok(())
    }

    /// Compact render for small terminals — single panel + tab bar.
    fn render_compact(
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        panels: &HashMap<u32, WorkerPanel>,
        status: &OrchestratorStatus,
        focused: Option<u32>,
        _log_lines: &VecDeque<Line<'static>>,
        worker_count: u32,
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
        frame.render_widget(Line::from(tab_spans), tab_area);

        // Show focused panel (or W1)
        let show_id = focused.unwrap_or(1);
        if let Some(panel) = panels.get(&show_id) {
            let widget = render_panel_widget(panel, panel_area, true);
            frame.render_widget(widget, panel_area);
        }

        // Compact status bar (1 line)
        let compact_bar = render_compact_bar(status);
        frame.render_widget(compact_bar, bar_area);
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

// ── Panel rendering ──────────────────────────────────────────────────

/// Build a Paragraph widget for a worker panel.
fn render_panel_widget<'a>(panel: &'a WorkerPanel, area: Rect, is_focused: bool) -> Paragraph<'a> {
    let ws = &panel.status;

    // Title: ▶ W{N} [{task_id}: {component}] (▶ only when focused)
    let task_str = ws.task_id.as_deref().unwrap_or("---");
    let comp_str = ws.component.as_deref().unwrap_or("");
    let focus_marker = if is_focused { "▶ " } else { "" };
    let title = if comp_str.is_empty() {
        format!(" {focus_marker}W{} [{}] ", panel.worker_id, task_str)
    } else {
        format!(
            " {focus_marker}W{} [{}: {}] ",
            panel.worker_id, task_str, comp_str
        )
    };

    // Footer: $cost | tokens
    let footer = format!(
        " ${:.4} │ ↓{} ↑{} ",
        ws.cost_usd,
        format_tokens(ws.input_tokens),
        format_tokens(ws.output_tokens)
    );

    // Border: Double+Cyan for focused, Rounded+state_color for normal
    let (border_type, border_color) = if is_focused {
        (BorderType::Double, Color::Cyan)
    } else {
        (BorderType::Rounded, state_color(&ws.state))
    };
    let border_style = if is_focused {
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(border_color)
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

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(border_style)
        .title(Span::styled(title, title_style))
        .title_bottom(Span::styled(
            footer,
            Style::default().fg(Color::DarkGray),
        ));

    // Inner area height (minus 2 for borders)
    let inner_height = area.height.saturating_sub(2) as usize;
    if inner_height == 0 {
        return Paragraph::new(Vec::<Line<'_>>::new()).block(block);
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

    // Remaining lines: output tail
    let output_height = inner_height.saturating_sub(1);
    let mut lines = vec![status_line];

    if output_height > 0 {
        let tail = if panel.scroll_offset == 0 {
            // Auto-scroll: show last N lines
            panel.output.tail(output_height)
        } else {
            // Manual scroll: show from offset
            let total = panel.output.len();
            let start = panel.scroll_offset.min(total);
            let end = (start + output_height).min(total);
            panel
                .output
                .lines
                .iter()
                .skip(start)
                .take(end - start)
                .cloned()
                .collect()
        };
        lines.extend(tail);
    }

    Paragraph::new(lines).block(block).wrap(Wrap { trim: false })
}

/// Get color for a worker state.
fn state_color(state: &WorkerState) -> Color {
    match state {
        WorkerState::Idle => Color::DarkGray,
        WorkerState::Implementing => Color::Cyan,
        WorkerState::Reviewing => Color::Yellow,
        WorkerState::Verifying => Color::Magenta,
        WorkerState::Merging => Color::Green,
    }
}

/// Get icon and color for a worker state.
fn state_icon(state: &WorkerState) -> (&'static str, Color) {
    match state {
        WorkerState::Idle => ("○", Color::DarkGray),
        WorkerState::Implementing => ("●", Color::Cyan),
        WorkerState::Reviewing => ("◎", Color::Yellow),
        WorkerState::Verifying => ("◉", Color::Magenta),
        WorkerState::Merging => ("⊕", Color::Green),
    }
}

// ── Global status bar ────────────────────────────────────────────────

/// Render the 3-line global status bar at the bottom.
fn render_global_bar<'a>(status: &OrchestratorStatus, focused: Option<u32>) -> Paragraph<'a> {
    let total = status.scheduler.total;
    let done = status.scheduler.done;
    let pct = if total > 0 { (done * 100) / total } else { 0 };
    let bar = render_progress_bar(done, total, 20);
    let elapsed = format_duration(status.elapsed);
    let cost_per_task = if done > 0 {
        status.total_cost / done as f64
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
            format!("${:.4}", status.total_cost),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("${cost_per_task:.4}/task"),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("⏱ {elapsed}"),
            Style::default().fg(Color::White),
        ),
    ]);

    let focus_span = if let Some(wid) = focused {
        Span::styled(
            format!("Focus: W{wid}"),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
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
        Span::styled(
            "q Tab ↑↓ Esc",
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    // Separator or shutdown banner
    let separator = match status.shutdown_state {
        ShutdownState::Running => Line::from(vec![Span::styled(
            "─".repeat(200),
            Style::default().fg(Color::DarkGray),
        )]),
        ShutdownState::Draining => Line::from(vec![Span::styled(
            " ⏳ SHUTTING DOWN — waiting for in-progress tasks to finish... (press q again to force) ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        ShutdownState::Aborting => Line::from(vec![Span::styled(
            " ⚠ FORCE SHUTDOWN — aborting all workers... ",
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        )]),
    };

    Paragraph::new(vec![separator, progress_line, queue_line])
}

/// Compact 1-line status bar for small terminals.
fn render_compact_bar(status: &OrchestratorStatus) -> Line<'static> {
    let total = status.scheduler.total;
    let done = status.scheduler.done;
    let pct = if total > 0 { (done * 100) / total } else { 0 };
    let elapsed = format_duration(status.elapsed);

    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{done}/{total} ({pct}%)"),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("${:.4}", status.total_cost),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("⏱ {elapsed}"),
            Style::default().fg(Color::White),
        ),
        Span::raw("  "),
        Span::styled(
            "q=quit Tab=switch",
            Style::default().fg(Color::DarkGray),
        ),
    ])
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
    fn test_output_ring_buffer_capacity() {
        let mut buf = OutputRingBuffer::new(3);
        buf.push("line 1");
        buf.push("line 2");
        buf.push("line 3");
        buf.push("line 4");
        assert_eq!(buf.len(), 3);
        let tail = buf.tail(10);
        // First element should be "line 2" (line 1 was evicted)
        assert_eq!(tail.len(), 3);
    }

    #[test]
    fn test_output_ring_buffer_tail() {
        let mut buf = OutputRingBuffer::new(10);
        for i in 0..5 {
            buf.push(&format!("line {i}"));
        }
        let tail = buf.tail(2);
        assert_eq!(tail.len(), 2);
    }

    #[test]
    fn test_state_color_mapping() {
        assert_eq!(state_color(&WorkerState::Idle), Color::DarkGray);
        assert_eq!(state_color(&WorkerState::Implementing), Color::Cyan);
        assert_eq!(state_color(&WorkerState::Reviewing), Color::Yellow);
        assert_eq!(state_color(&WorkerState::Verifying), Color::Magenta);
        assert_eq!(state_color(&WorkerState::Merging), Color::Green);
    }

    #[test]
    fn test_state_icon_mapping() {
        let (icon, _) = state_icon(&WorkerState::Idle);
        assert_eq!(icon, "○");
        let (icon, _) = state_icon(&WorkerState::Implementing);
        assert_eq!(icon, "●");
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
}
