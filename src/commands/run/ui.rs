use std::io::{self, Stdout};

use ansi_to_tui::IntoText;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::{
    Terminal, Viewport,
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
            let area = frame.area();

            if height >= 3
                && let Some(ref tp) = status.task_progress
            {
                // 3-line layout: metrics | current task | gauge
                let chunks = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ])
                .split(area);

                // Line 1: existing status metrics
                let line1 = status.to_line(nf);
                let p1 = Paragraph::new(line1);
                frame.render_widget(p1, chunks[0]);

                // Line 2: current task info + speed/ETA
                let mut task_line = tp.to_status_line();
                if let Some(ref speed) = status.speed_text {
                    task_line.spans.push(Span::raw(" │ "));
                    task_line.spans.push(Span::styled(
                        format!("{} {}", icons::status_speed(nf), speed),
                        Style::default().fg(Color::Yellow),
                    ));
                }
                if let Some(ref eta) = status.eta_text {
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
                let label = if let Some(ref eta) = status.eta_text {
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
            let line = status.to_line(nf);
            let paragraph = Paragraph::new(line);
            frame.render_widget(paragraph, area);
            strip_trailing_spaces(frame.buffer_mut());
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
