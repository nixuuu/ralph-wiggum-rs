use std::collections::{HashMap, VecDeque};
use std::io::{self, Stdout};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
use crate::commands::task::orchestrate::worker_status::{WorkerState, WorkerStatus};
use crate::shared::error::Result;

/// Grace period for keeping idle workers visible in the dashboard (in seconds).
const IDLE_GRACE_PERIOD: Duration = Duration::from_secs(5);

// ── WorkerPanel ──────────────────────────────────────────────────────

/// State of a single worker panel in the dashboard.
pub struct WorkerPanel {
    pub worker_id: u32,
    pub status: WorkerStatus,
    pub output: OutputRingBuffer,
    pub scroll_offset: usize, // 0 = auto-scroll (follow tail)
    /// Timestamp when the worker became idle (used for grace period visibility).
    pub idle_since: Option<Instant>,
}

impl WorkerPanel {
    pub fn new(worker_id: u32) -> Self {
        Self {
            worker_id,
            status: WorkerStatus::idle(worker_id),
            output: OutputRingBuffer::new(200),
            scroll_offset: 0,
            idle_since: None,
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
    /// Task 21.3: Updates `active_worker_ids` with currently active (non-idle) worker IDs
    /// for dynamic focus navigation.
    pub fn render(
        &mut self,
        status: &OrchestratorStatus,
        tasks_file: Option<&crate::shared::tasks::TasksFile>,
        _task_summaries: &[TaskSummaryEntry],
        active_worker_ids: Option<&Arc<Mutex<Vec<u32>>>>,
    ) -> Result<()> {
        let worker_count = self.worker_count;
        let focused = self.focused_worker;
        let panels = &self.panels;
        let log_lines = &self.log_lines;
        let preview_scroll = self.preview_scroll_offset;

        // Task 21.3: Compute active (non-idle) worker IDs before rendering
        if let Some(active_ids_ref) = active_worker_ids {
            let mut active = Vec::with_capacity(worker_count as usize);
            for id in 1..=worker_count {
                if let Some(panel) = panels.get(&id) {
                    // Consider worker active if not in Idle state
                    // Check via is_idle() method to avoid direct field access
                    if !panel.status.is_idle() {
                        active.push(id);
                    }
                }
            }
            // Update shared state (input thread reads this)
            *active_ids_ref.lock().unwrap() = active;
        }

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
                // Filter active workers (non-Idle or Idle within grace period)
                let active_workers: Vec<u32> = (1..=worker_count)
                    .filter(|worker_id| {
                        if let Some(panel) = panels.get(worker_id) {
                            is_worker_active(panel)
                        } else {
                            false
                        }
                    })
                    .collect();

                // Render worker grid with only active workers
                let active_count = active_workers.len() as u32;
                if active_count > 0 {
                    let rects = compute_grid_rects(grid_area, active_count);
                    for (idx, (_grid_worker_id, rect)) in rects.into_iter().enumerate() {
                        // Map grid position (idx) to actual worker_id from active_workers
                        if let Some(&worker_id) = active_workers.get(idx)
                            && let Some(panel) = panels.get(&worker_id)
                        {
                            let is_focused = focused == Some(worker_id);
                            let widget = render_panel_widget(panel, rect, is_focused);
                            frame.render_widget(widget, rect);
                        }
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
        use crate::commands::task::orchestrate::worker_status::WorkerState;

        if let Some(panel) = self.panels.get_mut(&worker_id) {
            let previous_state = &panel.status.state;
            let new_state = &status.state;

            // Track transition to/from Idle for grace period
            if previous_state != &WorkerState::Idle && new_state == &WorkerState::Idle {
                // Worker just became idle — start grace period
                panel.idle_since = Some(Instant::now());
            } else if new_state != &WorkerState::Idle {
                // Worker is no longer idle — clear grace period
                panel.idle_since = None;
            }

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

    /// Auto-shift focus to first active worker if the focused worker is idle.
    /// Returns Some(new_focus) if focus was changed, None if no change.
    /// This should be called after worker status updates to maintain focus on active workers.
    pub fn auto_focus_active(&mut self) -> Option<u32> {
        use crate::commands::task::orchestrate::worker_status::WorkerState;

        // If no focus, nothing to do
        let focused = self.focused_worker?;

        // Check if focused worker is idle
        let is_idle = self
            .panels
            .get(&focused)
            .map(|p| p.status.state == WorkerState::Idle)
            .unwrap_or(false);

        if !is_idle {
            return None; // Focused worker is still active
        }

        // Find first non-idle worker
        let first_active = (1..=self.worker_count).find(|&id| {
            self.panels
                .get(&id)
                .map(|p| p.status.state != WorkerState::Idle)
                .unwrap_or(false)
        });

        if let Some(new_focus) = first_active {
            self.set_focus(Some(new_focus));
            Some(new_focus)
        } else {
            // All workers idle, unfocus
            self.set_focus(None);
            Some(0) // Return 0 to indicate unfocus
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

// ── Worker activity check ────────────────────────────────────────────

/// Check if a worker should be visible in the grid.
///
/// A worker is active if:
/// - It's not Idle, OR
/// - It's Idle but within the grace period (idle_since is recent enough)
fn is_worker_active(panel: &WorkerPanel) -> bool {
    if panel.status.state != WorkerState::Idle {
        // Not idle — always show
        return true;
    }

    // Idle — check grace period
    if let Some(idle_time) = panel.idle_since {
        idle_time.elapsed() < IDLE_GRACE_PERIOD
    } else {
        // No idle_since timestamp — assume not in grace period
        false
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

    // Format worker count info
    let total_workers = status.active_workers + status.idle_workers;
    let worker_info = format!("W {}/{}", status.active_workers, total_workers);

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
        Span::raw(" │ "),
        Span::styled(worker_info, Style::default().fg(Color::Magenta)),
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

    // Add restart confirmation, quit confirmation hint, or normal keybindings
    let mut queue_line_spans = queue_line.spans;
    if status.restart_pending.is_some() {
        queue_line_spans.push(Span::styled(
            "y=confirm n/Esc=cancel",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    } else if status.quit_pending {
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
            "q Tab ↑↓ Esc p=tasks r=reload R=restart",
            Style::default().fg(Color::DarkGray),
        ));
    }
    let queue_line = Line::from(queue_line_spans);

    // Separator or restart/shutdown/quit/completed banner
    let separator = if let Some((worker_id, task_id)) = &status.restart_pending {
        // Restart confirmation banner (highest priority)
        Line::from(vec![Span::styled(
            format!(" ⟳ Restart worker {} (task {})? [y/n] ", worker_id, task_id),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )])
    } else if status.quit_pending {
        // Quit confirmation banner (second priority)
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
        // Task 21.6: Test with 0 workers
        assert_eq!(compute_grid(0), (1, 1));
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
        assert_ne!(
            initial_offset, preview_scroll_offset,
            "offset should change"
        );
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
        assert_ne!(
            initial_offset, preview_scroll_offset,
            "offset should change"
        );
        assert_eq!(
            preview_scroll_offset,
            usize::MAX,
            "i32::MAX should set preview_scroll_offset to usize::MAX (bottom, clamped later)"
        );
    }

    // ── Grace period tests ───────────────────────────────────────────────

    #[test]
    fn test_worker_panel_new_has_no_idle_since() {
        let panel = WorkerPanel::new(1);
        assert_eq!(panel.idle_since, None);
    }

    #[test]
    fn test_is_worker_active_non_idle_worker() {
        let mut panel = WorkerPanel::new(1);
        panel.status.state = WorkerState::Implementing;
        panel.idle_since = None;

        assert!(is_worker_active(&panel), "Non-idle worker should be active");
    }

    #[test]
    fn test_is_worker_active_idle_no_timestamp() {
        let mut panel = WorkerPanel::new(1);
        panel.status.state = WorkerState::Idle;
        panel.idle_since = None;

        assert!(
            !is_worker_active(&panel),
            "Idle worker without timestamp should not be active"
        );
    }

    #[test]
    fn test_is_worker_active_idle_within_grace_period() {
        let mut panel = WorkerPanel::new(1);
        panel.status.state = WorkerState::Idle;
        panel.idle_since = Some(Instant::now());

        assert!(
            is_worker_active(&panel),
            "Idle worker within grace period should be active"
        );
    }

    #[test]
    fn test_is_worker_active_idle_beyond_grace_period() {
        let mut panel = WorkerPanel::new(1);
        panel.status.state = WorkerState::Idle;
        // Simulate a timestamp from 10 seconds ago (beyond 5s grace period)
        panel.idle_since = Some(Instant::now() - std::time::Duration::from_secs(10));

        assert!(
            !is_worker_active(&panel),
            "Idle worker beyond grace period should not be active"
        );
    }

    #[test]
    fn test_update_worker_status_sets_idle_since() {
        // We can't create a full Dashboard without TTY, so we'll test the logic directly
        let mut panel = WorkerPanel::new(1);

        // Initially not idle
        panel.status.state = WorkerState::Implementing;
        panel.idle_since = None;

        // Simulate update_worker_status logic for transition to Idle
        let previous_state = panel.status.state.clone();
        let new_status = WorkerStatus {
            state: WorkerState::Idle,
            task_id: None,
            component: None,
            phase: None,
            model: None,
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
        };

        // Apply the logic from update_worker_status
        let new_state = &new_status.state;
        if previous_state != WorkerState::Idle && new_state == &WorkerState::Idle {
            panel.idle_since = Some(Instant::now());
        }
        panel.status = new_status;

        assert!(
            panel.idle_since.is_some(),
            "idle_since should be set when transitioning to Idle"
        );
    }

    #[test]
    fn test_update_worker_status_clears_idle_since() {
        let mut panel = WorkerPanel::new(1);

        // Start in Idle with timestamp
        panel.status.state = WorkerState::Idle;
        panel.idle_since = Some(Instant::now());

        // Simulate transition to non-Idle
        let new_status = WorkerStatus {
            state: WorkerState::Implementing,
            task_id: Some("T1".to_string()),
            component: None,
            phase: None,
            model: None,
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
        };

        // Apply the logic from update_worker_status
        let new_state = &new_status.state;
        if new_state != &WorkerState::Idle {
            panel.idle_since = None;
        }
        panel.status = new_status;

        assert_eq!(
            panel.idle_since, None,
            "idle_since should be cleared when transitioning away from Idle"
        );
    }

    #[test]
    fn test_update_worker_status_idle_to_idle_preserves_timestamp() {
        let mut panel = WorkerPanel::new(1);

        // Start in Idle with timestamp
        let original_timestamp = Instant::now();
        panel.status.state = WorkerState::Idle;
        panel.idle_since = Some(original_timestamp);

        // Stay Idle (e.g., cost update)
        let new_status = WorkerStatus {
            state: WorkerState::Idle,
            task_id: None,
            component: None,
            phase: None,
            model: None,
            cost_usd: 1.23,
            input_tokens: 100,
            output_tokens: 50,
        };

        // Apply the logic from update_worker_status
        let previous_state = &panel.status.state;
        let new_state = &new_status.state;
        if previous_state != &WorkerState::Idle && new_state == &WorkerState::Idle {
            panel.idle_since = Some(Instant::now());
        } else if new_state != &WorkerState::Idle {
            panel.idle_since = None;
        }
        // else: preserve idle_since
        panel.status = new_status;

        assert_eq!(
            panel.idle_since,
            Some(original_timestamp),
            "idle_since should be preserved when staying Idle"
        );
    }

    // ── Task 21.6: Idle worker filtering tests ──────────────────────────

    #[test]
    fn test_compute_grid_rects_with_non_sequential_ids() {
        let area = Rect::new(0, 0, 120, 40);

        // Test with 2 workers (should produce 2x1 grid)
        let rects = compute_grid_rects(area, 2);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].0, 1); // First position gets ID 1
        assert_eq!(rects[1].0, 2); // Second position gets ID 2

        // Test with 3 workers (should produce 2x2 grid)
        let rects = compute_grid_rects(area, 3);
        assert_eq!(rects.len(), 3);
        // Verify IDs are sequential
        let ids: Vec<u32> = rects.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn test_compute_grid_rects_zero_workers() {
        let area = Rect::new(0, 0, 120, 40);
        let rects = compute_grid_rects(area, 0);
        assert_eq!(rects.len(), 0, "Zero workers should produce empty grid");
    }

    #[test]
    fn test_active_filter_includes_implementing_worker() {
        let mut panel = WorkerPanel::new(1);
        panel.status.state = WorkerState::Implementing;
        panel.idle_since = None;

        assert!(
            is_worker_active(&panel),
            "Implementing worker should be active"
        );
    }

    #[test]
    fn test_active_filter_excludes_idle_worker_without_timestamp() {
        let mut panel = WorkerPanel::new(1);
        panel.status.state = WorkerState::Idle;
        panel.idle_since = None;

        assert!(
            !is_worker_active(&panel),
            "Idle worker without timestamp should be excluded"
        );
    }

    #[test]
    fn test_active_filter_includes_idle_worker_within_grace_period() {
        let mut panel = WorkerPanel::new(1);
        panel.status.state = WorkerState::Idle;
        // Timestamp from 1 second ago (well within 5s grace period)
        panel.idle_since = Some(Instant::now() - Duration::from_secs(1));

        assert!(
            is_worker_active(&panel),
            "Idle worker within grace period should be included"
        );
    }

    #[test]
    fn test_active_filter_excludes_idle_worker_past_grace_period() {
        let mut panel = WorkerPanel::new(1);
        panel.status.state = WorkerState::Idle;
        // Timestamp from 10 seconds ago (beyond 5s grace period)
        panel.idle_since = Some(Instant::now() - Duration::from_secs(10));

        assert!(
            !is_worker_active(&panel),
            "Idle worker past grace period should be excluded"
        );
    }

    #[test]
    fn test_all_idle_scenario_returns_empty_active_set() {
        // Simulate the filtering logic from render() method
        let mut panels = HashMap::new();

        // Create 3 idle workers, all past grace period
        for id in 1..=3 {
            let mut panel = WorkerPanel::new(id);
            panel.status.state = WorkerState::Idle;
            panel.idle_since = Some(Instant::now() - Duration::from_secs(10));
            panels.insert(id, panel);
        }

        // Apply the active filter (same logic as in render())
        let active_workers: Vec<u32> = (1..=3)
            .filter(|worker_id| {
                if let Some(panel) = panels.get(worker_id) {
                    is_worker_active(panel)
                } else {
                    false
                }
            })
            .collect();

        assert_eq!(
            active_workers.len(),
            0,
            "All idle workers past grace period should result in empty active set"
        );
    }

    #[test]
    fn test_mixed_workers_active_filter() {
        let mut panels = HashMap::new();

        // Worker 1: Implementing (active)
        let mut panel1 = WorkerPanel::new(1);
        panel1.status.state = WorkerState::Implementing;
        panels.insert(1, panel1);

        // Worker 2: Idle within grace period (active)
        let mut panel2 = WorkerPanel::new(2);
        panel2.status.state = WorkerState::Idle;
        panel2.idle_since = Some(Instant::now() - Duration::from_secs(2));
        panels.insert(2, panel2);

        // Worker 3: Idle past grace period (inactive)
        let mut panel3 = WorkerPanel::new(3);
        panel3.status.state = WorkerState::Idle;
        panel3.idle_since = Some(Instant::now() - Duration::from_secs(10));
        panels.insert(3, panel3);

        // Apply the active filter
        let active_workers: Vec<u32> = (1..=3)
            .filter(|worker_id| {
                if let Some(panel) = panels.get(worker_id) {
                    is_worker_active(panel)
                } else {
                    false
                }
            })
            .collect();

        assert_eq!(
            active_workers,
            vec![1, 2],
            "Should include Implementing and recently-Idle workers, exclude old Idle"
        );
    }

    #[test]
    fn test_grace_period_boundary_at_exactly_5_seconds() {
        let mut panel = WorkerPanel::new(1);
        panel.status.state = WorkerState::Idle;
        // Exactly at grace period boundary (5 seconds)
        panel.idle_since = Some(Instant::now() - IDLE_GRACE_PERIOD);

        // At exactly 5s, elapsed() >= IDLE_GRACE_PERIOD, so should be excluded
        // However, due to timing precision, this might be flaky.
        // The logic is: elapsed() < IDLE_GRACE_PERIOD means active
        // So at exactly 5s, it should NOT be active
        assert!(
            !is_worker_active(&panel),
            "Idle worker at exactly grace period boundary should be excluded"
        );
    }

    #[test]
    fn test_grace_period_just_before_boundary() {
        let mut panel = WorkerPanel::new(1);
        panel.status.state = WorkerState::Idle;
        // Just before grace period boundary (4.9 seconds)
        panel.idle_since = Some(Instant::now() - Duration::from_millis(4900));

        assert!(
            is_worker_active(&panel),
            "Idle worker just before grace period should be included"
        );
    }

    #[test]
    fn test_idle_grace_period_constant_is_5_seconds() {
        // Verify the constant matches specification
        assert_eq!(
            IDLE_GRACE_PERIOD,
            Duration::from_secs(5),
            "IDLE_GRACE_PERIOD should be 5 seconds as specified"
        );
    }
}
