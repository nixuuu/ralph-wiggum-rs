use std::io::{self, Stdout};

use ansi_to_tui::IntoText;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::{
    Terminal, Viewport,
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use crate::error::Result;

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

    /// Build the status line spans
    fn to_line(&self) -> Line<'static> {
        let iter_text = if self.max_iterations > 0 {
            format!("Iter {}/{}", self.iteration, self.max_iterations)
        } else {
            format!("Iter {}", self.iteration)
        };

        let time_text = format!("{:.1}s", self.iteration_elapsed_secs);
        let tokens_in = Self::format_tokens(self.input_tokens);
        let tokens_out = Self::format_tokens(self.output_tokens);
        let cost_text = format!("${:.4}", self.cost_usd);

        Line::from(vec![
            Span::styled(iter_text, Style::default().fg(Color::Cyan)),
            Span::raw(" │ "),
            Span::styled("⏱ ", Style::default().fg(Color::Yellow)),
            Span::raw(time_text),
            Span::raw(" │ "),
            Span::styled("↓", Style::default().fg(Color::Green)),
            Span::raw(format!(" {} ", tokens_in)),
            Span::styled("↑", Style::default().fg(Color::Magenta)),
            Span::raw(format!(" {} ", tokens_out)),
            Span::raw("│ "),
            Span::styled("$", Style::default().fg(Color::Yellow)),
            Span::raw(cost_text.trim_start_matches('$').to_string()),
        ])
    }
}

/// Terminal wrapper for inline status bar rendering
pub struct StatusTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    enabled: bool,
}

impl StatusTerminal {
    /// Create a new status terminal with inline viewport (1 line)
    pub fn new() -> Result<Self> {
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
            return Ok(Self { terminal, enabled });
        }

        enable_raw_mode()?;

        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: Viewport::Inline(1),
            },
        )?;

        Ok(Self { terminal, enabled })
    }

    /// Update the status bar with new data
    pub fn update(&mut self, status: &StatusData) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        self.terminal.draw(|frame| {
            let area = frame.area();
            let line = status.to_line();
            let paragraph = Paragraph::new(line);
            frame.render_widget(paragraph, area);
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
        })?;

        Ok(())
    }

    /// Show "Shutting down..." on the status bar for immediate feedback
    pub fn show_shutting_down(&mut self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        self.terminal.draw(|frame| {
            let area = frame.area();
            let line = Line::from(vec![
                Span::styled("⏸ ", Style::default().fg(Color::Yellow)),
                Span::styled("Shutting down...", Style::default().fg(Color::Yellow)),
            ]);
            let paragraph = Paragraph::new(line);
            frame.render_widget(paragraph, area);
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
