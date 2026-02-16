use std::io::{self, Stdout};

use ansi_to_tui::IntoText;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::{
    Frame, Terminal, Viewport,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::Rect,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Gauge, Paragraph, Wrap},
};

use crate::shared::error::Result;
use crate::shared::icons;
use crate::updater::version_checker::{UpdateInfo, UpdateState};

use super::output::TaskProgress;

/// Data for the status bar display
#[derive(Debug, Clone, Default)]
pub struct StatusData {
    pub iteration: u32,
    pub min_iterations: u32,
    pub max_iterations: u32,
    pub iteration_elapsed_secs: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub update_info: Option<UpdateInfo>,
    pub update_state: UpdateState,
    pub task_progress: Option<TaskProgress>,
    /// Formatted speed string (e.g., "1.2/h")
    pub speed_text: Option<String>,
    /// Formatted ETA string (e.g., "~23m")
    pub eta_text: Option<String>,
}

impl StatusData {
    /// Format tokens for display (e.g., 1234 -> "1.2k")
    fn format_tokens(tokens: u64) -> String {
        if tokens >= 1_000_000 {
            format!("{:.1}M", tokens as f64 / 1_000_000.0)
        } else if tokens >= 1_000 {
            format!("{:.1}k", tokens as f64 / 1_000.0)
        } else {
            tokens.to_string()
        }
    }

    /// Build the status line spans (version + metrics in single column)
    fn to_line(&self, nerd_font: bool) -> Line<'static> {
        let iter_text = match (self.min_iterations > 0, self.max_iterations > 0) {
            (true, true) => format!(
                "Iter {} ({}..{})",
                self.iteration, self.min_iterations, self.max_iterations
            ),
            (true, false) => format!("Iter {} (min {})", self.iteration, self.min_iterations),
            (false, true) => format!("Iter {}/{}", self.iteration, self.max_iterations),
            (false, false) => format!("Iter {}", self.iteration),
        };

        let time_text = format!("{:.1}s", self.iteration_elapsed_secs);
        let tokens_in = Self::format_tokens(self.input_tokens);
        let tokens_out = Self::format_tokens(self.output_tokens);
        let cost_text = format!("${:.4}", self.cost_usd);

        let mut spans = self.version_spans();
        if self.iteration > 0 {
            spans.push(Span::raw(" │ "));
            spans.push(Span::styled(iter_text, Style::default().fg(Color::Cyan)));
        }
        spans.push(Span::raw(" │ "));
        spans.push(Span::styled(
            format!("{} ", icons::status_clock(nerd_font)),
            Style::default().fg(Color::Yellow),
        ));
        spans.push(Span::raw(time_text));
        spans.push(Span::raw(" │ "));
        spans.push(Span::styled("↓", Style::default().fg(Color::Green)));
        spans.push(Span::raw(format!(" {} ", tokens_in)));
        spans.push(Span::styled("↑", Style::default().fg(Color::Magenta)));
        spans.push(Span::raw(format!(" {} ", tokens_out)));
        spans.push(Span::raw("│ "));
        spans.push(Span::styled("$", Style::default().fg(Color::Yellow)));
        spans.push(Span::raw(cost_text.trim_start_matches('$').to_string()));

        Line::from(spans)
    }

    /// Render status data to a ratatui Frame.
    ///
    /// Shared rendering logic used by both StatusTerminal::update() and tests.
    /// For height >= 3 with task_progress: 3-line layout (metrics | task | gauge).
    /// Otherwise: single-line metrics.
    pub(crate) fn draw(&self, frame: &mut Frame, nerd_font: bool, height: u16) {
        let area = frame.area();

        if height >= 3
            && let Some(ref tp) = self.task_progress
        {
            // 3-line layout: metrics | current task | gauge
            let chunks = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(area);

            // Line 1: existing status metrics
            let line1 = self.to_line(nerd_font);
            let p1 = Paragraph::new(line1);
            frame.render_widget(p1, chunks[0]);

            // Line 2: current task info + speed/ETA
            let mut task_line = tp.to_status_line();
            if let Some(ref speed) = self.speed_text {
                task_line.spans.push(Span::raw(" │ "));
                task_line.spans.push(Span::styled(
                    format!("{} {}", icons::status_speed(nerd_font), speed),
                    Style::default().fg(Color::Yellow),
                ));
            }
            if let Some(ref eta) = self.eta_text {
                task_line.spans.push(Span::raw(" "));
                task_line.spans.push(Span::styled(
                    format!("ETA {}", eta),
                    Style::default().fg(Color::Cyan),
                ));
            }
            let p2 = Paragraph::new(task_line);
            frame.render_widget(p2, chunks[1]);

            // Line 3: gauge progress bar
            let ratio = if tp.total > 0 {
                tp.done as f64 / tp.total as f64
            } else {
                0.0
            };
            let label = if let Some(ref eta) = self.eta_text {
                format!(
                    "{}/{} ({}%) | ETA {}",
                    tp.done,
                    tp.total,
                    (ratio * 100.0).round() as u32,
                    eta
                )
            } else {
                format!(
                    "{}/{} ({}%)",
                    tp.done,
                    tp.total,
                    (ratio * 100.0).round() as u32
                )
            };
            let gauge = Gauge::default()
                .ratio(ratio)
                .label(label)
                .gauge_style(Style::default().fg(Color::Green).bg(Color::DarkGray))
                .style(Style::default().fg(Color::White));
            frame.render_widget(gauge, chunks[2]);

            strip_trailing_spaces(frame.buffer_mut());
            return;
        }

        // Default: single line
        let line = self.to_line(nerd_font);
        let paragraph = Paragraph::new(line);
        frame.render_widget(paragraph, area);
        strip_trailing_spaces(frame.buffer_mut());
    }

    /// Build version spans for inline display
    fn version_spans(&self) -> Vec<Span<'static>> {
        let current = env!("CARGO_PKG_VERSION");

        if let Some(ref info) = self.update_info
            && info.update_available
        {
            let base = vec![
                Span::styled(format!("v{current}"), Style::default().fg(Color::DarkGray)),
                Span::styled(" -> ", Style::default().fg(Color::DarkGray)),
            ];

            let (version_style, suffix) = match self.update_state {
                UpdateState::Downloading => (
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                    Some((" downloading...", Style::default().fg(Color::Yellow))),
                ),
                UpdateState::Completed => (
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                    Some((" restart to apply", Style::default().fg(Color::Green))),
                ),
                UpdateState::Failed => (
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    Some((" update failed [Ctrl+U]", Style::default().fg(Color::Red))),
                ),
                _ => (
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                    Some((" [Ctrl+U]", Style::default().fg(Color::DarkGray))),
                ),
            };

            let mut spans = base;
            spans.push(Span::styled(info.latest_version.clone(), version_style));
            if let Some((text, style)) = suffix {
                spans.push(Span::styled(text, style));
            }
            return spans;
        }

        vec![Span::styled(
            format!("v{current}"),
            Style::default().fg(Color::DarkGray),
        )]
    }
}

/// Terminal wrapper for inline status bar rendering
pub struct StatusTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    enabled: bool,
    use_nerd_font: bool,
    height: u16,
}

impl StatusTerminal {
    /// Create a new status terminal with inline viewport (1 line)
    pub fn new(use_nerd_font: bool) -> Result<Self> {
        Self::with_height(use_nerd_font, 1)
    }

    /// Create a status terminal with custom viewport height
    pub fn with_height(use_nerd_font: bool, height: u16) -> Result<Self> {
        // Check if we're in a TTY
        let enabled = atty::is(atty::Stream::Stdout);

        if !enabled {
            // Create a dummy terminal for non-TTY environments
            let backend = CrosstermBackend::new(io::stdout());
            let terminal = Terminal::with_options(
                backend,
                ratatui::TerminalOptions {
                    viewport: Viewport::Inline(0),
                },
            )?;
            return Ok(Self {
                terminal,
                enabled,
                use_nerd_font,
                height: 0,
            });
        }

        enable_raw_mode()?;

        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        )?;

        Ok(Self {
            terminal,
            enabled,
            use_nerd_font,
            height,
        })
    }

    /// Update the status bar with new data
    pub fn update(&mut self, status: &StatusData) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let nf = self.use_nerd_font;
        let height = self.height;
        self.terminal.draw(|frame| {
            status.draw(frame, nf, height);
        })?;

        Ok(())
    }

    // Removed: draw_lines() method (task 2.4)
    // Never called in production code. Content rendering is handled via
    // print_line() and print_styled_lines() instead.

    /// Print a line above the status bar
    pub fn print_line(&mut self, text: &str) -> Result<()> {
        if !self.enabled {
            println!("{}", text);
            return Ok(());
        }

        // Convert ANSI escape codes to ratatui Text
        let ratatui_text = text.into_text().unwrap_or_default();

        self.terminal.insert_before(1, |buf| {
            let area = Rect::new(0, 0, buf.area.width, 1);
            let paragraph = Paragraph::new(ratatui_text.clone());
            paragraph.render(area, buf);
            strip_trailing_spaces(buf);
        })?;

        Ok(())
    }

    /// Print multiple lines above the status bar
    pub fn print_lines(&mut self, lines: &[String]) -> Result<()> {
        if !self.enabled {
            for line in lines {
                println!("{}", line);
            }
            return Ok(());
        }

        // Convert all lines to ratatui Text, properly handling ANSI escape codes
        let combined = lines.join("\n");
        let ratatui_text = combined.into_text().unwrap_or_default();
        let text_height = ratatui_text.lines.len() as u16;

        // Calculate required height with wrapping
        let terminal_width = self.terminal.size()?.width;
        let mut total_height = 0u16;
        for line in &ratatui_text.lines {
            let line_width: usize = line.spans.iter().map(|s| s.content.len()).sum();
            let wrapped_lines = if line_width == 0 {
                1
            } else {
                ((line_width as u16).saturating_sub(1) / terminal_width + 1).max(1)
            };
            total_height += wrapped_lines;
        }

        // Use the larger of actual lines or calculated wrapped height
        let height = total_height.max(text_height);

        self.terminal.insert_before(height, |buf| {
            let area = Rect::new(0, 0, buf.area.width, height);
            let paragraph = Paragraph::new(ratatui_text.clone()).wrap(Wrap { trim: false });
            paragraph.render(area, buf);
            strip_trailing_spaces(buf);
        })?;

        Ok(())
    }

    /// Handle terminal resize by clearing viewport and redrawing status bar
    pub fn handle_resize(&mut self, status: &StatusData) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        self.terminal.clear()?;
        self.update(status)?;
        Ok(())
    }

    /// Show "Shutting down..." on the status bar for immediate feedback
    pub fn show_shutting_down(&mut self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let nf = self.use_nerd_font;
        self.terminal.draw(|frame| {
            let area = frame.area();
            let line = Line::from(vec![
                Span::styled(
                    format!("{} ", icons::status_pause(nf)),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("Shutting down...", Style::default().fg(Color::Yellow)),
            ]);
            let paragraph = Paragraph::new(line);
            frame.render_widget(paragraph, area);
            strip_trailing_spaces(frame.buffer_mut());
        })?;

        Ok(())
    }

    /// Clear the status bar and restore terminal.
    ///
    /// Kolapsuje inline viewport: przesuwa kursor na pozycję viewportu
    /// i czyści wszystko poniżej, eliminując puste linie.
    pub fn cleanup(&mut self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        // Capture current viewport Y via one last draw
        let mut viewport_y = 0u16;
        self.terminal.draw(|frame| {
            viewport_y = frame.area().y;
        })?;

        // Collapse the inline viewport while still in raw mode
        crossterm::execute!(
            io::stdout(),
            crossterm::cursor::MoveTo(0, viewport_y),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown)
        )?;

        disable_raw_mode()?;
        Ok(())
    }

    /// Re-enable raw mode after temporary cleanup (e.g. after TUI question widgets).
    ///
    /// Creates a fresh terminal with new Viewport::Inline to replace the stale one.
    /// The old viewport was collapsed in `cleanup()`, so a new one is needed.
    pub fn reinit(&mut self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        enable_raw_mode()?;

        // Create a fresh terminal — the old viewport was collapsed in cleanup()
        let backend = CrosstermBackend::new(io::stdout());
        self.terminal = Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: Viewport::Inline(self.height),
            },
        )?;

        Ok(())
    }
}

impl Drop for StatusTerminal {
    fn drop(&mut self) {
        if self.enabled {
            let _ = disable_raw_mode();
        }
    }
}

// Need to add atty dependency for TTY detection
// For now, we'll assume TTY is available
mod atty {
    pub enum Stream {
        Stdout,
    }

    pub fn is(_stream: Stream) -> bool {
        // Simple check - assume TTY for now
        // Could use std::io::IsTerminal in Rust 1.70+
        true
    }
}

use ratatui::widgets::Widget;

/// Mark trailing space cells as skip to prevent terminal wrapping on resize.
///
/// ratatui's Paragraph fills the entire buffer width with spaces.
/// When the terminal is resized smaller, those trailing spaces cause line wrapping
/// and garbled output. Marking them as skip prevents the backend from writing them.
fn strip_trailing_spaces(buf: &mut Buffer) {
    let area = buf.area;
    for y in area.y..area.y + area.height {
        for x in (area.x..area.x + area.width).rev() {
            let cell = &buf[(x, y)];
            if cell.symbol() == " "
                && cell.fg == Color::Reset
                && cell.bg == Color::Reset
                && cell.modifier.is_empty()
            {
                buf[(x, y)].set_skip(true);
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::snap;
    use ratatui::backend::TestBackend;

    /// Helper do renderowania statusu do bufora testowego.
    ///
    /// Wywołuje StatusData::draw() — tę samą logikę co produkcyjne StatusTerminal::update().
    /// Dla height=1: single-line metryki. Dla height=3: metryki + task progress + gauge.
    fn render_status_to_buffer(
        status: &StatusData,
        width: u16,
        height: u16,
        nerd_font: bool,
    ) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("Failed to create test terminal");

        terminal
            .draw(|frame| {
                status.draw(frame, nerd_font, height);
            })
            .expect("Failed to draw widget");

        terminal.backend().buffer().clone()
    }

    /// Test snapshot: status bar z domyślnymi metrykami (iteracja 1, 0 tokenów)
    #[test]
    fn test_snapshot_default_metrics() {
        let status = StatusData {
            iteration: 1,
            min_iterations: 0,
            max_iterations: 0,
            iteration_elapsed_secs: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: None,
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 80, 1, false);
        insta::assert_snapshot!(snap(&buffer), @"v[VERSION] │ Iter 1 │ [t] 0.0s │ ↓ 0 ↑ 0 │ $0.0000");
    }

    /// Test snapshot: status bar z aktywnymi metrykami (iteracja 5, 15k tokenów, $0.42)
    #[test]
    fn test_snapshot_active_metrics() {
        let status = StatusData {
            iteration: 5,
            min_iterations: 0,
            max_iterations: 0,
            iteration_elapsed_secs: 42.7,
            input_tokens: 15234,
            output_tokens: 8976,
            cost_usd: 0.4231,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: None,
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 80, 1, false);
        insta::assert_snapshot!(snap(&buffer), @"v[VERSION] │ Iter 5 │ [t] 42.7s │ ↓ 15.2k ↑ 9.0k │ $0.4231");
    }

    /// Test snapshot: status bar z UpdateState::Completed (update gotowy do użycia)
    #[test]
    fn test_snapshot_update_completed() {
        let status = StatusData {
            iteration: 3,
            min_iterations: 0,
            max_iterations: 0,
            iteration_elapsed_secs: 12.3,
            input_tokens: 5000,
            output_tokens: 3000,
            cost_usd: 0.15,
            update_info: Some(UpdateInfo {
                update_available: true,
                latest_version: "0.2.0".to_string(),
            }),
            update_state: UpdateState::Completed,
            task_progress: None,
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 100, 1, false);
        insta::assert_snapshot!(snap(&buffer), @"v[VERSION] -> 0.2.0 restart to apply │ Iter 3 │ [t] 12.3s │ ↓ 5.0k ↑ 3.0k │ $0.1500");
    }

    /// Test snapshot: status bar z UpdateState::Failed (błąd podczas update)
    #[test]
    fn test_snapshot_update_failed() {
        let status = StatusData {
            iteration: 2,
            min_iterations: 0,
            max_iterations: 0,
            iteration_elapsed_secs: 8.5,
            input_tokens: 2000,
            output_tokens: 1500,
            cost_usd: 0.08,
            update_info: Some(UpdateInfo {
                update_available: true,
                latest_version: "0.3.0".to_string(),
            }),
            update_state: UpdateState::Failed,
            task_progress: None,
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 100, 1, false);
        insta::assert_snapshot!(snap(&buffer), @"v[VERSION] -> 0.3.0 update failed [Ctrl+U] │ Iter 2 │ [t] 8.5s │ ↓ 2.0k ↑ 1.5k │ $0.0800");
    }

    /// Test snapshot: status bar z UpdateState::Downloading (pobieranie aktualizacji)
    #[test]
    fn test_snapshot_update_downloading() {
        let status = StatusData {
            iteration: 1,
            min_iterations: 0,
            max_iterations: 0,
            iteration_elapsed_secs: 3.2,
            input_tokens: 1000,
            output_tokens: 500,
            cost_usd: 0.03,
            update_info: Some(UpdateInfo {
                update_available: true,
                latest_version: "0.4.0".to_string(),
            }),
            update_state: UpdateState::Downloading,
            task_progress: None,
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 100, 1, false);
        insta::assert_snapshot!(snap(&buffer), @"v[VERSION] -> 0.4.0 downloading... │ Iter 1 │ [t] 3.2s │ ↓ 1.0k ↑ 500 │ $0.0300");
    }

    /// Test snapshot: status bar z gauge postępu (50%) i task progress
    #[test]
    fn test_snapshot_gauge_progress_50_percent() {
        let status = StatusData {
            iteration: 4,
            min_iterations: 0,
            max_iterations: 0,
            iteration_elapsed_secs: 25.6,
            input_tokens: 10000,
            output_tokens: 6000,
            cost_usd: 0.25,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: Some(TaskProgress {
                done: 5,
                total: 10,
                in_progress: 0,
                blocked: 0,
                todo: 5,
                current_task_id: Some("1.2".to_string()),
                current_task_name: Some("Test task".to_string()),
                current_task_component: Some("tests".to_string()),
            }),
            speed_text: Some("1.2/h".to_string()),
            eta_text: Some("~23m".to_string()),
        };

        let buffer = render_status_to_buffer(&status, 80, 3, false);
        insta::assert_snapshot!(snap(&buffer), @"
        v[VERSION] │ Iter 4 │ [t] 25.6s │ ↓ 10.0k ↑ 6.0k │ $0.2500
        ▶ 1.2 [tests] Test task │ ✓5 ~0 !0 ○5 │ ^ 1.2/h ETA ~23m
        █████████████████████████████5/10 (50%) | ETA ~23m
        ");
    }

    /// Test snapshot: status bar z task_progress (0/10, 0%)
    #[test]
    fn test_snapshot_task_progress_0_percent() {
        let status = StatusData {
            iteration: 1,
            min_iterations: 0,
            max_iterations: 0,
            iteration_elapsed_secs: 5.2,
            input_tokens: 2000,
            output_tokens: 1000,
            cost_usd: 0.05,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: Some(TaskProgress {
                done: 0,
                total: 10,
                in_progress: 1,
                blocked: 0,
                todo: 9,
                current_task_id: Some("1.1".to_string()),
                current_task_name: Some("First task".to_string()),
                current_task_component: Some("core".to_string()),
            }),
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 80, 3, false);
        insta::assert_snapshot!(snap(&buffer), @"
        v[VERSION] │ Iter 1 │ [t] 5.2s │ ↓ 2.0k ↑ 1.0k │ $0.0500
        ▶ 1.1 [core] First task │ ✓0 ~1 !0 ○9
                                           0/10 (0%)
        ");
    }

    /// Test snapshot: status bar z task_progress (10/10, 100%)
    #[test]
    fn test_snapshot_task_progress_100_percent() {
        let status = StatusData {
            iteration: 12,
            min_iterations: 0,
            max_iterations: 0,
            iteration_elapsed_secs: 120.5,
            input_tokens: 50000,
            output_tokens: 30000,
            cost_usd: 1.25,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: Some(TaskProgress {
                done: 10,
                total: 10,
                in_progress: 0,
                blocked: 0,
                todo: 0,
                current_task_id: Some("10.5".to_string()),
                current_task_name: Some("Final task".to_string()),
                current_task_component: Some("finalize".to_string()),
            }),
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 80, 3, false);
        insta::assert_snapshot!(snap(&buffer), @"
        v[VERSION] │ Iter 12 │ [t] 120.5s │ ↓ 50.0k ↑ 30.0k │ $1.2500
        ▶ 10.5 [finalize] Final task │ ✓10 ~0 !0 ○0
        ██████████████████████████████████10/10 (100%) █████████████████████████████████
        ");
    }

    /// Test snapshot: speed_text i eta_text ignorowane bez task_progress nawet przy height=3.
    ///
    /// Logika draw() wymaga `height >= 3 && task_progress.is_some()` aby wejść w 3-liniowy layout.
    /// Bez task_progress fallback do single-line — speed/eta nie mają gdzie się pojawić.
    #[test]
    fn test_snapshot_speed_eta_ignored_without_task_progress() {
        let status = StatusData {
            iteration: 3,
            min_iterations: 0,
            max_iterations: 0,
            iteration_elapsed_secs: 18.3,
            input_tokens: 8000,
            output_tokens: 4500,
            cost_usd: 0.18,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: None,
            speed_text: Some("5.2 tok/s".to_string()),
            eta_text: Some("~2m 30s".to_string()),
        };

        // height=3 ale brak task_progress → single-line fallback, speed/eta pominięte
        let buffer = render_status_to_buffer(&status, 80, 3, false);
        insta::assert_snapshot!(snap(&buffer), @"v[VERSION] │ Iter 3 │ [t] 18.3s │ ↓ 8.0k ↑ 4.5k │ $0.1800");
    }

    /// Test snapshot: status bar z task_progress + speed_text + eta_text (wszystkie pola)
    #[test]
    fn test_snapshot_all_fields_combined() {
        let status = StatusData {
            iteration: 8,
            min_iterations: 0,
            max_iterations: 0,
            iteration_elapsed_secs: 65.4,
            input_tokens: 25000,
            output_tokens: 15000,
            cost_usd: 0.55,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: Some(TaskProgress {
                done: 7,
                total: 12,
                in_progress: 1,
                blocked: 1,
                todo: 3,
                current_task_id: Some("5.3".to_string()),
                current_task_name: Some("Complex integration test".to_string()),
                current_task_component: Some("integration".to_string()),
            }),
            speed_text: Some("3.8/h".to_string()),
            eta_text: Some("~1h 15m".to_string()),
        };

        let buffer = render_status_to_buffer(&status, 100, 3, false);
        insta::assert_snapshot!(snap(&buffer), @"
        v[VERSION] │ Iter 8 │ [t] 65.4s │ ↓ 25.0k ↑ 15.0k │ $0.5500
        ▶ 5.3 [integration] Complex integration test │ ✓7 ~1 !1 ○3 │ ^ 3.8/h ETA ~1h 15m
        ██████████████████████████████████████7/12 (58%) | ETA ~1h 15m
        ");
    }

    /// Test snapshot: task_progress z total=0 — gauge ratio=0.0, brak dzielenia przez zero
    #[test]
    fn test_snapshot_task_progress_total_zero() {
        let status = StatusData {
            iteration: 1,
            min_iterations: 0,
            max_iterations: 0,
            iteration_elapsed_secs: 0.5,
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.01,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: Some(TaskProgress {
                done: 0,
                total: 0,
                in_progress: 0,
                blocked: 0,
                todo: 0,
                current_task_id: None,
                current_task_name: None,
                current_task_component: None,
            }),
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 80, 3, false);
        insta::assert_snapshot!(snap(&buffer), @"
        v[VERSION] │ Iter 1 │ [t] 0.5s │ ↓ 100 ↑ 50 │ $0.0100
         │ ✓0 ~0 !0 ○0
                                            0/0 (0%)
        ");
    }

    /// Test formatowania tokenów: małe liczby (0-999) bez suffiksu
    #[test]
    fn test_format_tokens_small() {
        assert_eq!(StatusData::format_tokens(0), "0");
        assert_eq!(StatusData::format_tokens(123), "123");
        assert_eq!(StatusData::format_tokens(999), "999");
    }

    /// Test formatowania tokenów: tysiące z suffiksem 'k' (1.2k, 15.2k)
    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(StatusData::format_tokens(1_000), "1.0k");
        assert_eq!(StatusData::format_tokens(1_234), "1.2k");
        assert_eq!(StatusData::format_tokens(9_876), "9.9k");
        assert_eq!(StatusData::format_tokens(15_234), "15.2k");
        assert_eq!(StatusData::format_tokens(999_999), "1000.0k");
    }

    /// Test formatowania tokenów: miliony z suffiksem 'M' (1.2M, 15.2M)
    #[test]
    fn test_format_tokens_millions() {
        assert_eq!(StatusData::format_tokens(1_000_000), "1.0M");
        assert_eq!(StatusData::format_tokens(1_234_567), "1.2M");
        assert_eq!(StatusData::format_tokens(9_876_543), "9.9M");
        assert_eq!(StatusData::format_tokens(15_234_567), "15.2M");
    }

    /// Test snapshot: status bar z min_iterations=5, max_iterations=10, iteration=3
    /// Oczekiwany format: "Iter 3 (5..10)"
    #[test]
    fn test_snapshot_min_max_iterations() {
        let status = StatusData {
            iteration: 3,
            min_iterations: 5,
            max_iterations: 10,
            iteration_elapsed_secs: 15.2,
            input_tokens: 8000,
            output_tokens: 4000,
            cost_usd: 0.18,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: None,
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 80, 1, false);
        insta::assert_snapshot!(snap(&buffer), @"v[VERSION] │ Iter 3 (5..10) │ [t] 15.2s │ ↓ 8.0k ↑ 4.0k │ $0.1800");
    }

    /// Test snapshot: status bar z min_iterations=0, max_iterations=1 (single shot)
    /// Oczekiwany format: "Iter 1/1"
    #[test]
    fn test_snapshot_single_shot() {
        let status = StatusData {
            iteration: 1,
            min_iterations: 0,
            max_iterations: 1,
            iteration_elapsed_secs: 5.7,
            input_tokens: 3000,
            output_tokens: 1500,
            cost_usd: 0.07,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: None,
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 80, 1, false);
        insta::assert_snapshot!(snap(&buffer), @"v[VERSION] │ Iter 1/1 │ [t] 5.7s │ ↓ 3.0k ↑ 1.5k │ $0.0700");
    }

    /// Test snapshot: status bar z iteration=999 (duża liczba iteracji)
    /// Oczekiwany format: "Iter 999"
    #[test]
    fn test_snapshot_large_iteration() {
        let status = StatusData {
            iteration: 999,
            min_iterations: 0,
            max_iterations: 0,
            iteration_elapsed_secs: 1234.5,
            input_tokens: 500_000,
            output_tokens: 250_000,
            cost_usd: 12.34,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: None,
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 100, 1, false);
        insta::assert_snapshot!(snap(&buffer), @"v[VERSION] │ Iter 999 │ [t] 1234.5s │ ↓ 500.0k ↑ 250.0k │ $12.3400");
    }

    /// Test snapshot: status bar z min_iterations=5 bez max (tylko minimum)
    /// Oczekiwany format: "Iter 3 (min 5)"
    #[test]
    fn test_snapshot_min_iterations_only() {
        let status = StatusData {
            iteration: 3,
            min_iterations: 5,
            max_iterations: 0,
            iteration_elapsed_secs: 10.0,
            input_tokens: 5000,
            output_tokens: 2500,
            cost_usd: 0.12,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: None,
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 80, 1, false);
        insta::assert_snapshot!(snap(&buffer), @"v[VERSION] │ Iter 3 (min 5) │ [t] 10.0s │ ↓ 5.0k ↑ 2.5k │ $0.1200");
    }

    /// Test snapshot: status bar z max_iterations=20 bez min (tylko maximum)
    /// Oczekiwany format: "Iter 15/20"
    #[test]
    fn test_snapshot_max_iterations_only() {
        let status = StatusData {
            iteration: 15,
            min_iterations: 0,
            max_iterations: 20,
            iteration_elapsed_secs: 67.3,
            input_tokens: 45_000,
            output_tokens: 23_000,
            cost_usd: 1.15,
            update_info: None,
            update_state: UpdateState::None,
            task_progress: None,
            speed_text: None,
            eta_text: None,
        };

        let buffer = render_status_to_buffer(&status, 80, 1, false);
        insta::assert_snapshot!(snap(&buffer), @"v[VERSION] │ Iter 15/20 │ [t] 67.3s │ ↓ 45.0k ↑ 23.0k │ $1.1500");
    }
}
