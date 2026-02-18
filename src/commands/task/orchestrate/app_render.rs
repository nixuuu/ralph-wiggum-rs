//! Render functions for OrchestrateApp.
//!
//! Free functions wydzielone z `app.rs` — grid layout, status bar, overlays.
//! Wywoływane przez `AppState::draw()` w `OrchestrateApp`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::commands::task::orchestrate::app::WorkerPanel;
use crate::commands::task::orchestrate::shared_types::{format_duration, render_progress_bar};
use crate::commands::task::orchestrate::shutdown_types::{OrchestratorStatus, ShutdownState};
use crate::commands::task::orchestrate::worker_panel::WorkerPanelWidget;
use crate::commands::task::orchestrate::worker_status::WorkerState;
use crate::shared::tasks::TasksFile;
use crate::tui::theme::DEFAULT_THEME;
use crate::tui::widgets::{TextInputOverlay, render_task_preview};

/// Grace period for keeping idle workers visible.
const IDLE_GRACE_PERIOD: Duration = Duration::from_secs(5);

/// Check if a worker should be visible in the grid.
///
/// Non-idle workers are always visible. When `show_idle` is true, all workers
/// (including idle) are visible. Otherwise, idle workers are visible only during
/// the grace period after becoming idle. If `idle_since` is `None` for an idle
/// worker, it is treated as expired (immediately hidden).
pub(super) fn is_worker_active(panel: &WorkerPanel, show_idle: bool) -> bool {
    if panel.status.state != WorkerState::Idle {
        return true;
    }
    if show_idle {
        return true;
    }
    panel
        .idle_since
        .is_some_and(|idle_time| idle_time.elapsed() < IDLE_GRACE_PERIOD)
}

/// Compute optimal (cols, rows) for the worker count.
///
/// Dynamic grid calculation:
/// - Terminal width < 160: max 2 columns (compact layout for narrow terminals)
/// - Terminal width >= 160: max 3 columns (full layout for wide terminals)
///
/// Grid jest dynamicznie dostosowywany do liczby workerów w ramach max_cols.
pub(super) fn compute_grid(worker_count: u32, terminal_width: u16) -> (u32, u32) {
    // Determine max columns based on terminal width
    let max_cols = if terminal_width < 160 { 2 } else { 3 };

    match worker_count {
        0 | 1 => (1, 1),
        2 if max_cols >= 2 => (2, 1),
        2 => (1, 2), // fallback for very narrow terminal
        3 | 4 if max_cols >= 2 => (2, 2),
        3 | 4 => (1, worker_count), // fallback for very narrow terminal
        5 | 6 if max_cols >= 3 => (3, 2),
        5 | 6 => (2, 3), // fallback for narrow terminal (2 cols)
        7..=9 if max_cols >= 3 => (3, 3),
        7..=9 => (2, worker_count.div_ceil(2)), // fallback for narrow terminal (2 cols)
        n => {
            // For large counts: fill max_cols first, then wrap to rows
            let cols = max_cols.min((n as f64).sqrt().ceil() as u32);
            let rows = n.div_ceil(cols);
            (cols, rows)
        }
    }
}

/// Compute screen rectangles for N workers within the grid area.
///
/// Returns `Vec<Rect>` where index 0 corresponds to the first worker slot.
/// Layout is row-major: fills each row left-to-right before wrapping.
pub(super) fn compute_grid_rects(area: Rect, worker_count: u32) -> Vec<Rect> {
    if worker_count == 0 {
        return vec![];
    }

    let (cols, rows) = compute_grid(worker_count, area.width);

    let row_constraints: Vec<Constraint> = (0..rows).map(|_| Constraint::Ratio(1, rows)).collect();
    let row_rects = Layout::vertical(row_constraints).split(area);

    let mut result = Vec::with_capacity(worker_count as usize);

    for (r, row_rect) in row_rects.iter().enumerate() {
        let workers_in_row = if r as u32 == rows - 1 {
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
            if result.len() >= worker_count as usize {
                break;
            }
            result.push(*col_rect);
        }
    }

    result
}

/// Configuration for rendering the worker grid.
pub(super) struct WorkerGridConfig<'a> {
    pub worker_count: u32,
    pub focused: Option<u32>,
    pub panels: &'a HashMap<u32, WorkerPanel>,
    pub show_preview: bool,
    pub preview_scroll: usize,
    pub tasks_file: Option<&'a TasksFile>,
    pub show_idle: bool,
}

/// Render worker grid with active workers only, or task preview overlay.
///
/// Gdy `show_preview == true` i `tasks_file` jest dostępny, renderuje
/// task preview zamiast worker grid.
pub(super) fn render_worker_grid(frame: &mut Frame, area: Rect, config: &WorkerGridConfig) {
    // Preview overlay: only when explicitly toggled AND tasks file available
    if config.show_preview
        && let Some(tf) = config.tasks_file
    {
        render_task_preview(frame, area, tf, config.preview_scroll);
        return;
    }

    // Filter active workers
    let active_workers: Vec<u32> = (1..=config.worker_count)
        .filter(|worker_id| {
            config
                .panels
                .get(worker_id)
                .map(|p| is_worker_active(p, config.show_idle))
                .unwrap_or(false)
        })
        .collect();

    let active_count = active_workers.len() as u32;
    if active_count > 0 {
        let rects = compute_grid_rects(area, active_count);
        for (idx, rect) in rects.into_iter().enumerate() {
            if let Some(&worker_id) = active_workers.get(idx)
                && let Some(panel) = config.panels.get(&worker_id)
            {
                let is_focused = config.focused == Some(worker_id);
                let widget = WorkerPanelWidget::new(panel, is_focused);
                frame.render_widget(widget, rect);
            }
        }
    } else {
        // All workers idle/filtered — fill with panel background for visual consistency
        let theme = &DEFAULT_THEME;
        frame.render_widget(
            Block::default().style(Style::default().bg(theme.panel_bg)),
            area,
        );
    }
}

/// Render text input overlay on top if active.
pub(super) fn render_overlay(
    frame: &mut Frame,
    area: Rect,
    shared_overlay: &Arc<Mutex<Option<TextInputOverlay>>>,
) {
    if let Ok(guard) = shared_overlay.lock()
        && let Some(ref overlay) = *guard
    {
        overlay.render(frame, area);
    }
}

/// Render the 3-line global status bar at the bottom.
pub(super) fn render_global_bar<'a>(
    status: &OrchestratorStatus,
    focused: Option<u32>,
    preview_active: bool,
    sidebar_focused: bool,
    show_idle: bool,
) -> Paragraph<'a> {
    let theme = &DEFAULT_THEME;
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

    let total_workers = status.active_workers + status.idle_workers;
    let worker_info = format!("W {}/{}", status.active_workers, total_workers);

    let progress_line = Line::from(vec![
        Span::raw(" "),
        Span::styled(bar, theme.success_style()),
        Span::raw(" "),
        Span::styled(format!("{done}/{total} ({pct}%)"), theme.primary_style()),
        Span::raw(" │ "),
        Span::styled(format!("${total_cost:.4}"), theme.warning_style()),
        Span::raw(" │ "),
        Span::styled(format!("${cost_per_task:.4}/task"), theme.warning_style()),
        Span::raw(" │ "),
        Span::styled(format!("⏱ {elapsed}"), theme.text_style()),
        Span::raw(" │ "),
        Span::styled(worker_info, theme.secondary_style()),
    ]);

    let focus_span = if let Some(wid) = focused {
        Span::styled(
            format!("Focus: W{wid}"),
            theme.primary_style().add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("No focus", theme.muted_style())
    };

    let mut queue_line_spans = vec![
        Span::raw(" Queue: "),
        Span::styled(
            format!("{} ready", status.scheduler.ready),
            theme.success_style(),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{} wip", status.scheduler.in_progress),
            theme.primary_style(),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{} blocked", status.scheduler.blocked),
            theme.error_style(),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("{} pending", status.scheduler.pending),
            theme.muted_style(),
        ),
        Span::raw("  │  "),
        focus_span,
        Span::raw("    "),
    ];

    // Keybinding hints
    if status.restart_pending.is_some() {
        queue_line_spans.push(Span::styled(
            "y=confirm n/Esc=cancel",
            theme.warning_style().add_modifier(Modifier::BOLD),
        ));
    } else if status.quit_pending {
        queue_line_spans.push(Span::styled(
            "Press q/Enter to confirm, Esc to cancel",
            theme.warning_style().add_modifier(Modifier::BOLD),
        ));
    } else if preview_active {
        queue_line_spans.push(Span::styled("p/Esc=close ↑↓=scroll", theme.muted_style()));
    } else if sidebar_focused {
        queue_line_spans.push(Span::styled(
            "Esc ↑↓ Space=expand +/-=resize t=close",
            theme.muted_style(),
        ));
    } else {
        let idle_hint = if show_idle {
            "h=hide-idle"
        } else {
            "h=show-idle"
        };
        queue_line_spans.push(Span::styled(
            format!("q Tab ↑↓ Esc p=tasks t=sidebar {idle_hint} r=reload R=restart"),
            theme.muted_style(),
        ));
    }
    let queue_line = Line::from(queue_line_spans);

    // Separator/banner line
    let separator = if let Some((worker_id, task_id)) = &status.restart_pending {
        Line::from(vec![Span::styled(
            format!(" ⟳ Restart worker {} (task {})? [y/n] ", worker_id, task_id),
            Style::default()
                .fg(theme.panel_bg)
                .bg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )])
    } else if status.quit_pending {
        Line::from(vec![Span::styled(
            " ⚠ Quit? Press q/Enter to confirm, Esc to cancel ",
            Style::default()
                .fg(theme.panel_bg)
                .bg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )])
    } else if status.completed && status.shutdown_state == ShutdownState::Running {
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
                .fg(theme.panel_bg)
                .bg(theme.success)
                .add_modifier(Modifier::BOLD),
        )])
    } else {
        match status.shutdown_state {
            ShutdownState::Running => {
                Line::from(vec![Span::styled("─".repeat(200), theme.muted_style())])
            }
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
                        .fg(theme.panel_bg)
                        .bg(theme.warning)
                        .add_modifier(Modifier::BOLD),
                )])
            }
            ShutdownState::Aborting => Line::from(vec![Span::styled(
                " ⚠ FORCE SHUTDOWN — aborting all workers... ",
                Style::default()
                    .fg(theme.text)
                    .bg(theme.error)
                    .add_modifier(Modifier::BOLD),
            )]),
        }
    };

    Paragraph::new(vec![separator, progress_line, queue_line])
        .style(Style::default().bg(theme.status_bar_bg))
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::task::orchestrate::worker_status::WorkerStatus;
    use crate::test_helpers::snap;
    use crate::tui::ring_buffer::OutputRingBuffer;
    use ratatui::{Terminal, backend::TestBackend};
    use std::time::Instant;

    // ── Test helpers ──

    /// Create a WorkerPanel with the given state and optional task/component.
    fn make_panel(
        worker_id: u32,
        state: WorkerState,
        task_id: Option<&str>,
        component: Option<&str>,
        idle_since: Option<Instant>,
    ) -> (u32, WorkerPanel) {
        let mut status = WorkerStatus::idle(worker_id);
        status.state = state;
        status.task_id = task_id.map(Into::into);
        status.component = component.map(Into::into);
        (
            worker_id,
            WorkerPanel {
                worker_id,
                status,
                output: OutputRingBuffer::with_capacity(100),
                scroll_offset: 0,
                idle_since,
            },
        )
    }

    /// Render worker grid and return snapshot string.
    fn render_grid_snap(
        width: u16,
        height: u16,
        panels: &HashMap<u32, WorkerPanel>,
        focused: Option<u32>,
    ) -> String {
        let area = Rect::new(0, 0, width, height);
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let config = WorkerGridConfig {
            worker_count: panels.len() as u32,
            focused,
            panels,
            show_preview: false,
            preview_scroll: 0,
            tasks_file: None,
            show_idle: false,
        };
        terminal
            .draw(|frame| render_worker_grid(frame, area, &config))
            .expect("draw");
        snap(terminal.backend().buffer())
    }

    // ── Grid layout tests — wide terminal (>= 160 cols) ──

    #[test]
    fn compute_grid_wide_terminal() {
        assert_eq!(compute_grid(0, 200), (1, 1));
        assert_eq!(compute_grid(1, 200), (1, 1));
        assert_eq!(compute_grid(2, 200), (2, 1));
        assert_eq!(compute_grid(3, 200), (2, 2));
        assert_eq!(compute_grid(4, 200), (2, 2));
        assert_eq!(compute_grid(5, 200), (3, 2));
        assert_eq!(compute_grid(6, 200), (3, 2));
        assert_eq!(compute_grid(7, 200), (3, 3));
        assert_eq!(compute_grid(9, 200), (3, 3));
    }

    #[test]
    fn compute_grid_wide_terminal_large_count() {
        let (cols, rows) = compute_grid(16, 200);
        assert_eq!(cols, 3);
        assert_eq!(rows, 6);
        assert!(cols * rows >= 16);
    }

    // ── Grid layout tests — narrow terminal (< 160 cols) ──

    #[test]
    fn compute_grid_narrow_terminal() {
        assert_eq!(compute_grid(0, 120), (1, 1));
        assert_eq!(compute_grid(1, 120), (1, 1));
        assert_eq!(compute_grid(2, 120), (2, 1));
        assert_eq!(compute_grid(3, 120), (2, 2));
        assert_eq!(compute_grid(4, 120), (2, 2));
        assert_eq!(compute_grid(5, 120), (2, 3));
        assert_eq!(compute_grid(6, 120), (2, 3));
        assert_eq!(compute_grid(7, 120), (2, 4));
        assert_eq!(compute_grid(9, 120), (2, 5));
    }

    #[test]
    fn compute_grid_narrow_terminal_large_count() {
        let (cols, rows) = compute_grid(16, 120);
        assert_eq!(cols, 2);
        assert_eq!(rows, 8);
        assert!(cols * rows >= 16);
    }

    // ── Grid layout tests — edge case: exactly 160 cols ──

    #[test]
    fn compute_grid_threshold_160_cols() {
        // 160 cols → wide (3 cols), 159 → narrow (2 cols)
        assert_eq!(compute_grid(5, 160), (3, 2));
        assert_eq!(compute_grid(9, 160), (3, 3));
        assert_eq!(compute_grid(5, 159), (2, 3));
        assert_eq!(compute_grid(9, 159), (2, 5));
    }

    // ── Grid rects tests ──

    #[test]
    fn compute_grid_rects_count() {
        let area = Rect::new(0, 0, 120, 40);
        let rects = compute_grid_rects(area, 4);
        assert_eq!(rects.len(), 4);
    }

    #[test]
    fn compute_grid_rects_single() {
        let area = Rect::new(0, 0, 80, 24);
        let rects = compute_grid_rects(area, 1);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0], area);
    }

    #[test]
    fn compute_grid_rects_wide_vs_narrow() {
        let rects_wide = compute_grid_rects(Rect::new(0, 0, 200, 40), 6);
        assert_eq!(rects_wide.len(), 6);

        let rects_narrow = compute_grid_rects(Rect::new(0, 0, 120, 40), 6);
        assert_eq!(rects_narrow.len(), 6);
    }

    #[test]
    fn compute_grid_rects_zero_workers() {
        let rects = compute_grid_rects(Rect::new(0, 0, 120, 40), 0);
        assert!(rects.is_empty());
    }

    // ── Snapshot tests for grid layout rendering ──

    #[test]
    fn test_snapshot_grid_2col_2workers() {
        let panels: HashMap<u32, WorkerPanel> = [
            make_panel(1, WorkerState::Implementing, Some("1.1"), Some("api"), None),
            make_panel(2, WorkerState::Reviewing, Some("2.1"), Some("ui"), None),
        ]
        .into_iter()
        .collect();

        insta::assert_snapshot!(render_grid_snap(120, 24, &panels, None));
    }

    #[test]
    fn test_snapshot_grid_3col_6workers() {
        let panels: HashMap<u32, WorkerPanel> = (1..=6)
            .map(|i| {
                make_panel(
                    i,
                    WorkerState::Implementing,
                    Some(&format!("{i}.1")),
                    Some("test"),
                    None,
                )
            })
            .collect();

        insta::assert_snapshot!(render_grid_snap(200, 40, &panels, Some(1)));
    }

    #[test]
    fn test_snapshot_grid_3col_9workers() {
        let panels: HashMap<u32, WorkerPanel> = (1..=9)
            .map(|i| {
                make_panel(
                    i,
                    WorkerState::Implementing,
                    Some(&format!("{i}.1")),
                    None,
                    None,
                )
            })
            .collect();

        insta::assert_snapshot!(render_grid_snap(200, 45, &panels, Some(5)));
    }

    #[test]
    fn test_snapshot_grid_with_grace_period() {
        let panels: HashMap<u32, WorkerPanel> = [
            // Worker 1: active
            make_panel(1, WorkerState::Implementing, Some("1.1"), None, None),
            // Worker 2: idle in grace period (just became idle)
            make_panel(2, WorkerState::Idle, None, None, Some(Instant::now())),
            // Worker 3: idle beyond grace (not visible)
            make_panel(
                3,
                WorkerState::Idle,
                None,
                None,
                Some(Instant::now() - IDLE_GRACE_PERIOD - Duration::from_secs(1)),
            ),
        ]
        .into_iter()
        .collect();

        // Tylko W1 i W2 powinny być widoczne
        insta::assert_snapshot!(render_grid_snap(120, 24, &panels, None));
    }
}
