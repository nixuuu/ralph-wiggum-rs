use std::io::{self, Stdout};

use ansi_to_tui::IntoText;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::{
    Terminal, Viewport,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use crate::error::Result;
use crate::icons;
use crate::updater::version_checker::UpdateInfo;

/// Data for the status bar display
#[derive(Debug, Clone, Default)]
pub struct StatusData {
    pub iteration: u32,
    pub max_iterations: u32,
    #[allow(dead_code)]
    pub elapsed_secs: f64,
    pub iteration_elapsed_secs: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub update_info: Option<UpdateInfo>,
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
        let iter_text = if self.max_iterations > 0 {
            format!("Iter {}/{}", self.iteration, self.max_iterations)
        } else {
            format!("Iter {}", self.iteration)
        };

        let time_text = format!("{:.1}s", self.iteration_elapsed_secs);
        let tokens_in = Self::format_tokens(self.input_tokens);
        let tokens_out = Self::format_tokens(self.output_tokens);
        let cost_text = format!("${:.4}", self.cost_usd);

        let mut spans = self.version_spans();
        spans.push(Span::raw(" │ "));
        spans.push(Span::styled(iter_text, Style::default().fg(Color::Cyan)));
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
            return vec![
                Span::styled(format!("v{current}"), Style::default().fg(Color::DarkGray)),
                Span::styled(" -> ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    info.latest_version.clone(),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
            ];
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
}

impl StatusTerminal {
    /// Create a new status terminal with inline viewport (1 line)
    pub fn new(use_nerd_font: bool) -> Result<Self> {
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
            });
        }

        enable_raw_mode()?;

        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: Viewport::Inline(1),
            },
        )?;

        Ok(Self {
            terminal,
            enabled,
            use_nerd_font,
        })
    }

    /// Update the status bar with new data
    pub fn update(&mut self, status: &StatusData) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let nf = self.use_nerd_font;
        self.terminal.draw(|frame| {
            let area = frame.area();
            let line = status.to_line(nf);
            let paragraph = Paragraph::new(line);
            frame.render_widget(paragraph, area);
            strip_trailing_spaces(frame.buffer_mut());
        })?;

        Ok(())
    }

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

    /// Clear the status bar and restore terminal
    pub fn cleanup(&mut self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        disable_raw_mode()?;
        // Clear the inline viewport area
        self.terminal.clear()?;
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
