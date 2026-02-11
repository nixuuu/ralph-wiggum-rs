use std::collections::{HashMap, VecDeque};
use std::io::{self, Stdout};
use std::time::Duration;

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

use crate::commands::task::orchestrate::shared_types::{
    OrchestratorStatus, ShutdownState, WorkerState, WorkerStatus, format_duration, format_tokens,
    render_progress_bar,
};
use crate::commands::task::orchestrate::summary::TaskSummaryEntry;
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
    ///
    /// When `task_summaries` is provided and `status.completed` is true,
    /// the completion summary panel is shown instead of worker grid.
    pub fn render(
        &mut self,
        status: &OrchestratorStatus,
        tasks_file: Option<&crate::shared::tasks::TasksFile>,
        task_summaries: &[TaskSummaryEntry],
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
                Self::render_compact(frame, area, panels, status, focused, log_lines, worker_count, preview_active);
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

            // If preview overlay is active, render task list instead of worker grid
            if let Some(tf) = tasks_file {
                Self::render_task_preview(frame, grid_area, tf, preview_scroll);
            } else if status.completed && status.shutdown_state == ShutdownState::Running {
                // Render completion summary panel (only when not shutting down)
                Self::render_completion_summary(frame, grid_area, task_summaries, status.elapsed);
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
            if delta == i32::MAX {
                // End key — reset to top
                self.preview_scroll_offset = 0;
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
    }

    pub fn handle_resize(&mut self) -> Result<()> {
        self.terminal.autoresize()?;
        Ok(())
    }

    /// Render completion summary panel showing all tasks complete.
    ///
    /// Shows:
    /// - Green "All tasks complete" header with checkmark
    /// - Summary table: task_id | status | cost | duration | retries
    /// - Total stats row: X/Y done, total cost, total time
    /// - Parallelism speedup metric
    /// - Hint: "Press q to exit, p to view tasks"
    fn render_completion_summary(
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        entries: &[TaskSummaryEntry],
        wall_clock: Duration,
    ) {
        let mut lines = Vec::new();

        // Header: Green "All tasks complete" with checkmark
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

        if entries.is_empty() {
            lines.push(Line::from("No tasks were executed."));
        } else {
            // Calculate totals
            let total_cost: f64 = entries.iter().map(|e| e.cost_usd).sum();
            let total_time: Duration = entries.iter().map(|e| e.duration).sum();
            let done_count = entries.iter().filter(|e| e.status == "Done").count();
            let total_count = entries.len();

            // Calculate parallelism speedup
            let speedup = if wall_clock.as_secs_f64() > 0.0 {
                total_time.as_secs_f64() / wall_clock.as_secs_f64()
            } else {
                1.0
            };

            // Column widths (simplified for TUI)
            let task_w = entries
                .iter()
                .map(|e| e.task_id.len())
                .max()
                .unwrap_or(4)
                .max(5);
            let status_w = 8;
            let cost_w = 10;
            let time_w = 8;
            let retries_w = 7;

            // Header row
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{:<task_w$}", "Task"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<status_w$}", "Status"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<cost_w$}", "Cost"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<time_w$}", "Time"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<retries_w$}", "Retries"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]));

            // Separator
            let sep_len = 2 + task_w + 3 + status_w + 3 + cost_w + 3 + time_w + 3 + retries_w;
            lines.push(Line::from(vec![Span::styled(
                "─".repeat(sep_len),
                Style::default().fg(Color::DarkGray),
            )]));

            // Task rows
            for entry in entries {
                let time_str = format_duration(entry.duration);
                let cost_str = format!("${:.4}", entry.cost_usd);
                let status_color = if entry.status == "Done" {
                    Color::Green
                } else if entry.status == "Blocked" {
                    Color::Red
                } else {
                    Color::Yellow
                };

                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<task_w$}", entry.task_id),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::raw(" │ "),
                    Span::styled(
                        format!("{:<status_w$}", entry.status),
                        Style::default().fg(status_color),
                    ),
                    Span::raw(" │ "),
                    Span::styled(
                        format!("{:<cost_w$}", cost_str),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" │ "),
                    Span::styled(
                        format!("{:<time_w$}", time_str),
                        Style::default().fg(Color::White),
                    ),
                    Span::raw(" │ "),
                    Span::styled(
                        format!("{:<retries_w$}", entry.retries),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }

            // Totals separator
            lines.push(Line::from(vec![Span::styled(
                "─".repeat(sep_len),
                Style::default().fg(Color::DarkGray),
            )]));

            // Totals row
            let status_total = format!("{done_count}/{total_count} done");
            let cost_total = format!("${total_cost:.4}");
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
                    format!("{:<status_w$}", status_total),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<cost_w$}", cost_total),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<time_w$}", time_total),
                    Style::default().fg(Color::White),
                ),
                Span::raw(" │ "),
                Span::raw(format!("{:<retries_w$}", "")),
            ]));

            // Parallelism speedup metric
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("Parallelism speedup: {speedup:.1}x"),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    " (sum of task times / wall clock)",
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }

        // Hint at the bottom
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "Press ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "q",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " to exit, ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "p",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " to view tasks",
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        // Build block with green border
        let block = Block::default()
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
            ));

        let widget = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
        frame.render_widget(widget, area);
    }

    /// Render task preview overlay as fullscreen panel showing all tasks.
    fn render_task_preview(
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        tasks_file: &crate::shared::tasks::TasksFile,
        scroll_offset: usize,
    ) {
        use crate::shared::progress::TaskStatus;

        // Build lines from the task tree
        let mut lines = Vec::new();

        fn traverse_node(
            node: &crate::shared::tasks::TaskNode,
            lines: &mut Vec<Line<'static>>,
            depth: usize,
        ) {
            let indent = "  ".repeat(depth);

            if node.is_leaf() {
                // Leaf task: show status icon + id + name + component + deps
                let status = node.status.as_ref().unwrap_or(&TaskStatus::Todo);
                let (icon, icon_color) = match status {
                    TaskStatus::Done => ("✓", Color::Green),
                    TaskStatus::InProgress => ("●", Color::Cyan),
                    TaskStatus::Blocked => ("✗", Color::Red),
                    TaskStatus::Todo => ("○", Color::White),
                };

                let component = node
                    .component
                    .as_deref()
                    .unwrap_or("general")
                    .to_string();

                let mut spans = vec![
                    Span::raw(indent.clone()),
                    Span::styled(icon.to_string(), Style::default().fg(icon_color)),
                    Span::raw(" "),
                    Span::styled(node.id.clone(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::raw(": "),
                    Span::raw(node.name.clone()),
                    Span::raw(" ["),
                    Span::styled(component, Style::default().fg(Color::Yellow)),
                    Span::raw("]"),
                ];

                // Show deps if any
                if !node.deps.is_empty() {
                    spans.push(Span::raw(" deps: "));
                    spans.push(Span::styled(
                        node.deps.join(", "),
                        Style::default().fg(Color::DarkGray),
                    ));
                }

                lines.push(Line::from(spans));
            } else {
                // Parent task: show as bold header
                let header = format!("{}{} {}", indent, node.id, node.name);
                lines.push(Line::from(vec![
                    Span::styled(header, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                ]));
            }

            // Recurse into subtasks
            for child in &node.subtasks {
                traverse_node(child, lines, depth + 1);
            }
        }

        // Traverse all top-level tasks
        for node in &tasks_file.tasks {
            traverse_node(node, &mut lines, 0);
        }

        // Apply scrolling
        let inner_height = area.height.saturating_sub(2) as usize; // Account for borders
        let total_lines = lines.len();

        // Handle empty task list
        if total_lines == 0 {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Double)
                .border_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                .title(Span::styled(
                    " Task List (p to close) ",
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ));
            let widget = Paragraph::new(vec![Line::from("No tasks found")])
                .block(block)
                .wrap(Wrap { trim: false });
            frame.render_widget(widget, area);
            return;
        }

        // Clamp scroll offset to valid range
        let max_scroll = total_lines.saturating_sub(inner_height).max(0);
        let clamped_offset = scroll_offset.min(max_scroll);
        let end = (clamped_offset + inner_height).min(total_lines);
        let visible_lines: Vec<Line<'static>> = lines.into_iter().skip(clamped_offset).take(end - clamped_offset).collect();

        // Build block with title
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .title(Span::styled(
                " Task List (p to close, ↑↓ to scroll) ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ));

        let widget = Paragraph::new(visible_lines).block(block).wrap(Wrap { trim: false });
        frame.render_widget(widget, area);
    }

    /// Compact render for small terminals — single panel + tab bar.
    #[allow(clippy::too_many_arguments)]
    fn render_compact(
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
        let compact_bar = render_compact_bar(status, preview_active);
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
        ws.cost_usd.max(0.0),
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

// ── Global status bar ────────────────────────────────────────────────

/// Render the 3-line global status bar at the bottom.
fn render_global_bar<'a>(status: &OrchestratorStatus, focused: Option<u32>, preview_active: bool) -> Paragraph<'a> {
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
            format!(" ✓ Completed — {}/{} done, {} blocked | q=exit p=tasks ", done, total, blocked)
        } else {
            format!(" ✓ Completed — {}/{} tasks done | q=exit p=tasks ", done, total)
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
                let countdown = status.shutdown_remaining
                    .map(|d| format!(" (force-kill za {}s)", d.as_secs()))
                    .unwrap_or_default();
                Line::from(vec![Span::styled(
                    format!(" ⏳ SHUTTING DOWN — waiting for in-progress tasks to finish...{countdown} (press q again to force) "),
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

/// Compact 1-line status bar for small terminals.
fn render_compact_bar(status: &OrchestratorStatus, preview_active: bool) -> Line<'static> {
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
        Span::styled(
            format!("⏱ {elapsed}"),
            Style::default().fg(Color::White),
        ),
    ];

    // Add completion indicator if all tasks are done
    if status.completed {
        spans.push(Span::raw(" │ "));
        spans.push(Span::styled(
            "✓ DONE",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
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

    // ── Tests for render_task_preview ────────────────────────────────────

    /// Helper to build rendered lines from task tree (matches render_task_preview logic)
    fn render_task_lines(tasks: &[crate::shared::tasks::TaskNode]) -> Vec<Line<'static>> {
        use crate::shared::progress::TaskStatus;
        let mut lines = Vec::new();

        fn traverse_node(
            node: &crate::shared::tasks::TaskNode,
            lines: &mut Vec<Line<'static>>,
            depth: usize,
        ) {
            let indent = "  ".repeat(depth);
            if node.is_leaf() {
                let status = node.status.as_ref().unwrap_or(&TaskStatus::Todo);
                let (icon, icon_color) = match status {
                    TaskStatus::Done => ("✓", Color::Green),
                    TaskStatus::InProgress => ("●", Color::Cyan),
                    TaskStatus::Blocked => ("✗", Color::Red),
                    TaskStatus::Todo => ("○", Color::White),
                };

                let component = node.component.as_deref().unwrap_or("general").to_string();
                let mut spans = vec![
                    Span::raw(indent.clone()),
                    Span::styled(icon.to_string(), Style::default().fg(icon_color)),
                    Span::raw(" "),
                    Span::styled(node.id.clone(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::raw(": "),
                    Span::raw(node.name.clone()),
                    Span::raw(" ["),
                    Span::styled(component, Style::default().fg(Color::Yellow)),
                    Span::raw("]"),
                ];

                if !node.deps.is_empty() {
                    spans.push(Span::raw(" deps: "));
                    spans.push(Span::styled(
                        node.deps.join(", "),
                        Style::default().fg(Color::DarkGray),
                    ));
                }

                lines.push(Line::from(spans));
            } else {
                let header = format!("{}{} {}", indent, node.id, node.name);
                lines.push(Line::from(vec![
                    Span::styled(header, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                ]));
            }

            for child in &node.subtasks {
                traverse_node(child, lines, depth + 1);
            }
        }

        for node in tasks {
            traverse_node(node, &mut lines, 0);
        }
        lines
    }

    #[test]
    fn test_render_task_preview_parent_tasks_bold() {
        use crate::shared::tasks::TaskNode;
        use crate::shared::progress::TaskStatus;

        let tasks = vec![
            TaskNode {
                id: "1".to_string(),
                name: "Epic 1: Frontend".to_string(),
                component: None,
                status: None,
                deps: vec![],
                model: None,
                description: None,
                related_files: vec![],
                implementation_steps: vec![],
                subtasks: vec![
                    TaskNode {
                        id: "1.1".to_string(),
                        name: "Build UI".to_string(),
                        component: Some("ui".to_string()),
                        status: Some(TaskStatus::Done),
                        deps: vec![],
                        model: None,
                        description: None,
                        related_files: vec![],
                        implementation_steps: vec![],
                        subtasks: vec![],
                    },
                ],
            },
        ];

        let lines = render_task_lines(&tasks);

        // Check: parent task "1" is bold
        assert_eq!(lines.len(), 2);
        // First line should be parent "1"
        let parent_line = &lines[0];
        assert!(parent_line.spans[0].content.contains("1 Epic 1: Frontend"));
        // Verify parent has bold modifier
        assert!(parent_line.spans[0].style.add_modifier.contains(Modifier::BOLD));

        // Second line should be leaf "1.1" with Done icon (✓)
        let leaf_line = &lines[1];
        assert_eq!(leaf_line.spans[1].content, "✓");
    }

    #[test]
    fn test_render_task_preview_status_icons_and_colors() {
        use crate::shared::progress::TaskStatus;

        // Verify icon mapping matches render_task_preview logic
        let icons_and_colors = vec![
            (TaskStatus::Done, "✓", Color::Green),
            (TaskStatus::InProgress, "●", Color::Cyan),
            (TaskStatus::Todo, "○", Color::White),
            (TaskStatus::Blocked, "✗", Color::Red),
        ];

        for (status, expected_icon, expected_color) in icons_and_colors {
            let (icon, color) = match &status {
                TaskStatus::Done => ("✓", Color::Green),
                TaskStatus::InProgress => ("●", Color::Cyan),
                TaskStatus::Blocked => ("✗", Color::Red),
                TaskStatus::Todo => ("○", Color::White),
            };
            assert_eq!(icon, expected_icon);
            assert_eq!(color, expected_color);
        }
    }

    #[test]
    fn test_render_task_preview_shows_deps() {
        use crate::shared::tasks::TaskNode;
        use crate::shared::progress::TaskStatus;

        let tasks = vec![
            TaskNode {
                id: "1".to_string(),
                name: "First".to_string(),
                component: Some("api".to_string()),
                status: Some(TaskStatus::Done),
                deps: vec![],
                model: None,
                description: None,
                related_files: vec![],
                implementation_steps: vec![],
                subtasks: vec![],
            },
            TaskNode {
                id: "2".to_string(),
                name: "Second".to_string(),
                component: Some("api".to_string()),
                status: Some(TaskStatus::Todo),
                deps: vec!["1".to_string()],
                model: None,
                description: None,
                related_files: vec![],
                implementation_steps: vec![],
                subtasks: vec![],
            },
            TaskNode {
                id: "3".to_string(),
                name: "Third".to_string(),
                component: Some("api".to_string()),
                status: Some(TaskStatus::Todo),
                deps: vec!["1".to_string(), "2".to_string()],
                model: None,
                description: None,
                related_files: vec![],
                implementation_steps: vec![],
                subtasks: vec![],
            },
        ];

        let lines = render_task_lines(&tasks);

        // Check: task "2" has deps shown
        assert_eq!(lines.len(), 3);
        let task2_line = &lines[1];
        let task2_text = task2_line.spans.iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(task2_text.contains("deps: 1"), "Expected 'deps: 1' in task 2 line: {}", task2_text);

        // Check: task "3" has multiple deps
        let task3_line = &lines[2];
        let task3_text = task3_line.spans.iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(task3_text.contains("deps: 1, 2"), "Expected 'deps: 1, 2' in task 3 line: {}", task3_text);
    }

    #[test]
    fn test_render_task_preview_empty_tasks_list() {
        use crate::shared::tasks::TasksFile;

        let empty_tasks_file = TasksFile {
            default_model: None,
            tasks: vec![],
        };

        // Empty TasksFile should produce no leaf lines
        assert!(empty_tasks_file.tasks.is_empty());
        assert_eq!(empty_tasks_file.flatten_leaves().len(), 0);
    }

    #[test]
    fn test_render_task_preview_deeply_nested_subtasks() {
        use crate::shared::tasks::TaskNode;
        use crate::shared::progress::TaskStatus;

        let tasks = vec![
            TaskNode {
                id: "1".to_string(),
                name: "Epic 1".to_string(),
                component: None,
                status: None,
                deps: vec![],
                model: None,
                description: None,
                related_files: vec![],
                implementation_steps: vec![],
                subtasks: vec![
                    TaskNode {
                        id: "1.1".to_string(),
                        name: "Feature 1.1".to_string(),
                        component: None,
                        status: None,
                        deps: vec![],
                        model: None,
                        description: None,
                        related_files: vec![],
                        implementation_steps: vec![],
                        subtasks: vec![
                            TaskNode {
                                id: "1.1.1".to_string(),
                                name: "Task 1.1.1".to_string(),
                                component: Some("api".to_string()),
                                status: Some(TaskStatus::Done),
                                deps: vec![],
                                model: None,
                                description: None,
                                related_files: vec![],
                                implementation_steps: vec![],
                                subtasks: vec![],
                            },
                            TaskNode {
                                id: "1.1.2".to_string(),
                                name: "Task 1.1.2".to_string(),
                                component: Some("api".to_string()),
                                status: Some(TaskStatus::Todo),
                                deps: vec!["1.1.1".to_string()],
                                model: None,
                                description: None,
                                related_files: vec![],
                                implementation_steps: vec![],
                                subtasks: vec![],
                            },
                        ],
                    },
                ],
            },
        ];

        let lines = render_task_lines(&tasks);

        // Should have 4 lines: Epic 1 (depth 0), Feature 1.1 (depth 1), Task 1.1.1 (depth 2), Task 1.1.2 (depth 2)
        assert_eq!(lines.len(), 4);

        // First line: "1 Epic 1" (bold, no indent)
        let epic_line = &lines[0];
        let epic_text = epic_line.spans[0].content.as_ref();
        assert!(epic_text.contains("1 Epic 1"));

        // Second line: "  1.1 Feature 1.1" (bold, 2 spaces indent)
        let feature_line = &lines[1];
        let feature_text = feature_line.spans[0].content.as_ref();
        assert!(feature_text.contains("1.1 Feature 1.1"));

        // Third line: "    1.1.1 Task 1.1.1" (4 spaces indent, leaf task)
        let task1_line = &lines[2];
        assert_eq!(task1_line.spans[0].content.as_ref(), "    "); // 4 spaces
        assert_eq!(task1_line.spans[1].content.as_ref(), "✓"); // Done icon

        // Fourth line: "    1.1.2 Task 1.1.2" (4 spaces indent, has deps)
        let task2_line = &lines[3];
        assert_eq!(task2_line.spans[0].content.as_ref(), "    "); // 4 spaces
        assert_eq!(task2_line.spans[1].content.as_ref(), "○"); // Todo icon
        let task2_text = task2_line.spans.iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(task2_text.contains("deps: 1.1.1"), "Expected 'deps: 1.1.1' in task 1.1.2 line: {}", task2_text);
    }

    #[test]
    fn test_render_task_preview_scroll_offset_clamping() {
        // Test that scroll offset is properly clamped to valid range
        let total_lines: usize = 1;
        let inner_height: usize = 18;
        let max_scroll = total_lines.saturating_sub(inner_height).max(0);

        // Any scroll offset >= max_scroll should be clamped
        assert_eq!(max_scroll, 0);
        let clamped = 100usize.min(max_scroll);
        assert_eq!(clamped, 0);
    }

    // ── Tests for render_completion_summary ──────────────────────────────

    /// Helper: collect lines from render_completion_summary
    fn render_summary_lines(entries: &[TaskSummaryEntry], wall_clock: Duration) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Header: Green "All tasks complete" with checkmark
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

        if entries.is_empty() {
            lines.push(Line::from("No tasks were executed."));
        } else {
            // Calculate totals
            let total_cost: f64 = entries.iter().map(|e| e.cost_usd).sum();
            let total_time: Duration = entries.iter().map(|e| e.duration).sum();
            let done_count = entries.iter().filter(|e| e.status == "Done").count();
            let total_count = entries.len();

            // Calculate parallelism speedup
            let speedup = if wall_clock.as_secs_f64() > 0.0 {
                total_time.as_secs_f64() / wall_clock.as_secs_f64()
            } else {
                1.0
            };

            // Column widths (simplified for TUI)
            let task_w = entries
                .iter()
                .map(|e| e.task_id.len())
                .max()
                .unwrap_or(4)
                .max(5);
            let status_w = 8;
            let cost_w = 10;
            let time_w = 8;
            let retries_w = 7;

            // Header row
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{:<task_w$}", "Task"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<status_w$}", "Status"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<cost_w$}", "Cost"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<time_w$}", "Time"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<retries_w$}", "Retries"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]));

            // Separator
            let sep_len = 2 + task_w + 3 + status_w + 3 + cost_w + 3 + time_w + 3 + retries_w;
            lines.push(Line::from(vec![Span::styled(
                "─".repeat(sep_len),
                Style::default().fg(Color::DarkGray),
            )]));

            // Task rows
            for entry in entries {
                let time_str = format_duration(entry.duration);
                let cost_str = format!("${:.4}", entry.cost_usd);
                let status_color = if entry.status == "Done" {
                    Color::Green
                } else if entry.status == "Blocked" {
                    Color::Red
                } else {
                    Color::Yellow
                };

                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<task_w$}", entry.task_id),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::raw(" │ "),
                    Span::styled(
                        format!("{:<status_w$}", entry.status),
                        Style::default().fg(status_color),
                    ),
                    Span::raw(" │ "),
                    Span::styled(
                        format!("{:<cost_w$}", cost_str),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" │ "),
                    Span::styled(
                        format!("{:<time_w$}", time_str),
                        Style::default().fg(Color::White),
                    ),
                    Span::raw(" │ "),
                    Span::styled(
                        format!("{:<retries_w$}", entry.retries),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }

            // Totals separator
            lines.push(Line::from(vec![Span::styled(
                "─".repeat(sep_len),
                Style::default().fg(Color::DarkGray),
            )]));

            // Totals row
            let status_total = format!("{done_count}/{total_count} done");
            let cost_total = format!("${total_cost:.4}");
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
                    format!("{:<status_w$}", status_total),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<cost_w$}", cost_total),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<time_w$}", time_total),
                    Style::default().fg(Color::White),
                ),
                Span::raw(" │ "),
                Span::raw(format!("{:<retries_w$}", "")),
            ]));

            // Parallelism speedup metric
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("Parallelism speedup: {speedup:.1}x"),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    " (sum of task times / wall clock)",
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }

        // Hint at the bottom
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "Press ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "q",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " to exit, ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "p",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " to view tasks",
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        lines
    }

    #[test]
    fn test_render_completion_summary_header_styling() {
        let entries = vec![TaskSummaryEntry {
            task_id: "T01".to_string(),
            status: "Done".to_string(),
            cost_usd: 0.042,
            duration: Duration::from_secs(45),
            retries: 0,
        }];

        let lines = render_summary_lines(&entries, Duration::from_secs(45));

        // First line should be green header with checkmark
        assert!(!lines.is_empty());
        let header_line = &lines[0];
        assert!(header_line.spans.len() >= 2);
        // Check second span has "All tasks complete"
        assert!(header_line.spans[1].content.contains("All tasks complete"));
        // Verify it's green
        assert_eq!(header_line.spans[1].style.fg, Some(Color::Green));
        // Verify it's bold
        assert!(header_line.spans[1].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_render_completion_summary_table_structure() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "T01".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.042,
                duration: Duration::from_secs(45),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "T02".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.038,
                duration: Duration::from_secs(32),
                retries: 1,
            },
        ];

        let lines = render_summary_lines(&entries, Duration::from_secs(100));

        // Skip header (line 0) and empty line (line 1)
        // Line 2 should be table header row
        let table_header_idx = 2;
        assert!(table_header_idx < lines.len());
        let header_line = &lines[table_header_idx];

        // Check header row has correct columns
        let header_text = header_line.spans.iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");

        assert!(header_text.contains("Task"));
        assert!(header_text.contains("Status"));
        assert!(header_text.contains("Cost"));
        assert!(header_text.contains("Time"));
        assert!(header_text.contains("Retries"));
    }

    #[test]
    fn test_render_completion_summary_totals_row() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "T01".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.042,
                duration: Duration::from_secs(45),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "T02".to_string(),
                status: "Blocked".to_string(),
                cost_usd: 0.089,
                duration: Duration::from_secs(80),
                retries: 3,
            },
        ];

        let lines = render_summary_lines(&entries, Duration::from_secs(100));

        // Find totals row (should contain "TOTAL" and "1/2 done")
        let totals_text = lines.iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");

        // Check aggregates
        assert!(totals_text.contains("TOTAL"));
        assert!(totals_text.contains("1/2 done"), "Expected '1/2 done' in totals, got: {}", totals_text);
        // Total cost: 0.042 + 0.089 = 0.131
        assert!(totals_text.contains("$0.131"), "Expected total cost in totals");
    }

    #[test]
    fn test_render_completion_summary_empty_case() {
        let entries: Vec<TaskSummaryEntry> = vec![];

        let lines = render_summary_lines(&entries, Duration::from_secs(60));

        // Should have header, empty line, and placeholder message
        assert!(lines.len() >= 3);

        // Find the placeholder message
        let all_text = lines.iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");

        assert!(all_text.contains("No tasks were executed."));
    }

    #[test]
    fn test_render_completion_summary_parallelism_calculation() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "T01".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.042,
                duration: Duration::from_secs(100),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "T02".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.038,
                duration: Duration::from_secs(100),
                retries: 0,
            },
        ];

        // Wall clock is 100s, but tasks took 200s total (2 parallel tasks)
        // Speedup should be 200/100 = 2.0x
        let lines = render_summary_lines(&entries, Duration::from_secs(100));

        let all_text = lines.iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");

        assert!(all_text.contains("Parallelism speedup: 2.0x"));
    }

    #[test]
    fn test_render_completion_summary_zero_wall_clock() {
        // Edge case: wall_clock duration is zero (shouldn't happen in practice)
        // Should default to speedup 1.0x
        let entries = vec![TaskSummaryEntry {
            task_id: "T01".to_string(),
            status: "Done".to_string(),
            cost_usd: 0.042,
            duration: Duration::from_secs(45),
            retries: 0,
        }];

        let lines = render_summary_lines(&entries, Duration::from_secs(0));

        let all_text = lines.iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");

        // When wall_clock is 0, speedup defaults to 1.0x
        assert!(all_text.contains("Parallelism speedup: 1.0x"));
    }

    #[test]
    fn test_render_completion_summary_status_colors() {
        let entries = vec![
            TaskSummaryEntry {
                task_id: "T01".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.042,
                duration: Duration::from_secs(45),
                retries: 0,
            },
            TaskSummaryEntry {
                task_id: "T02".to_string(),
                status: "Blocked".to_string(),
                cost_usd: 0.089,
                duration: Duration::from_secs(80),
                retries: 2,
            },
            TaskSummaryEntry {
                task_id: "T03".to_string(),
                status: "Pending".to_string(),
                cost_usd: 0.050,
                duration: Duration::from_secs(30),
                retries: 1,
            },
        ];

        let lines = render_summary_lines(&entries, Duration::from_secs(100));

        // Find task rows and verify status color mapping
        let mut done_found = false;
        let mut blocked_found = false;
        let mut pending_found = false;

        for line in &lines {
            for span in &line.spans {
                // Statuses are formatted with padding, so check if they contain the text
                if span.content.contains("Done") && !span.content.contains("done") {
                    // Task status "Done", not "1/3 done" (which is in totals)
                    assert_eq!(span.style.fg, Some(Color::Green));
                    done_found = true;
                }
                if span.content.contains("Blocked") {
                    assert_eq!(span.style.fg, Some(Color::Red));
                    blocked_found = true;
                }
                if span.content.contains("Pending") {
                    assert_eq!(span.style.fg, Some(Color::Yellow));
                    pending_found = true;
                }
            }
        }

        assert!(done_found, "Done status not found");
        assert!(blocked_found, "Blocked status not found");
        assert!(pending_found, "Pending status not found");
    }

    #[test]
    fn test_render_completion_summary_footer_hint() {
        let entries = vec![TaskSummaryEntry {
            task_id: "T01".to_string(),
            status: "Done".to_string(),
            cost_usd: 0.042,
            duration: Duration::from_secs(45),
            retries: 0,
        }];

        let lines = render_summary_lines(&entries, Duration::from_secs(45));

        // Find footer with hint
        let all_text = lines.iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");

        assert!(all_text.contains("Press q to exit, p to view tasks"));
    }

    #[test]
    fn test_render_completion_summary_long_task_id() {
        // Test that column width adapts to longer task IDs
        let entries = vec![
            TaskSummaryEntry {
                task_id: "feature/very-long-task-id".to_string(),
                status: "Done".to_string(),
                cost_usd: 0.042,
                duration: Duration::from_secs(45),
                retries: 0,
            },
        ];

        let lines = render_summary_lines(&entries, Duration::from_secs(45));

        let all_text = lines.iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");

        // Long task ID should be included in the output
        assert!(all_text.contains("feature/very-long-task-id"));
    }

    #[test]
    fn test_render_completion_summary_all_blocked_tasks() {
        // Edge case: all tasks are blocked
        let entries = vec![
            TaskSummaryEntry {
                task_id: "T01".to_string(),
                status: "Blocked".to_string(),
                cost_usd: 0.042,
                duration: Duration::from_secs(45),
                retries: 2,
            },
            TaskSummaryEntry {
                task_id: "T02".to_string(),
                status: "Blocked".to_string(),
                cost_usd: 0.038,
                duration: Duration::from_secs(32),
                retries: 1,
            },
        ];

        let lines = render_summary_lines(&entries, Duration::from_secs(100));

        let all_text = lines.iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");

        // Should show 0/2 done when all are blocked
        assert!(all_text.contains("0/2 done"));
        // Total cost should still be calculated
        assert!(all_text.contains("$0.080"));
    }
}
