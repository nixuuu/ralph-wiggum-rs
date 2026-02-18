/// Header widget — top bar displaying command name, model, iteration count, and elapsed time.
///
/// Shows in format: `<command_name> | <model_info> | <iteration/max> | <elapsed_time>`
/// When running, displays animated spinner using Braille characters.
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

use crate::tui::Theme;

/// Data structure containing all header display information.
#[derive(Debug, Clone)]
pub struct HeaderData {
    /// Name of the command being executed (e.g., "run", "task orchestrate")
    pub command_name: String,

    /// Model identifier (e.g., "claude-3-5-sonnet")
    pub model: String,

    /// Current iteration number (optional for commands that don't iterate)
    pub iteration: Option<u32>,

    /// Maximum iterations (optional, shows when specified)
    pub max_iterations: Option<u32>,

    /// Elapsed time since command started
    pub elapsed: Duration,

    /// Whether the command is currently running
    pub is_running: bool,
}

/// Animated spinner frames using Braille Unicode characters for smooth animation.
/// Provides visual feedback during processing.
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Header widget that renders a single-line status bar.
///
/// Left side: command name (bold) + model info
/// Right side: iteration counter + elapsed time
/// Optional spinner when is_running is true
pub struct Header<'a> {
    data: &'a HeaderData,
    theme: &'a Theme,
}

impl<'a> Header<'a> {
    /// Creates a new Header widget with the provided data and theme.
    pub fn new(data: &'a HeaderData, theme: &'a Theme) -> Self {
        Self { data, theme }
    }

    /// Renders the left side of the header (command + model).
    fn left_spans(&self) -> Vec<Span<'a>> {
        vec![
            Span::styled(
                self.data.command_name.clone(),
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" | "),
            Span::styled(
                self.data.model.clone(),
                Style::default().fg(self.theme.muted),
            ),
        ]
    }

    /// Renders the right side of the header (iteration + elapsed + optional spinner).
    fn right_spans(&self) -> Vec<Span<'a>> {
        let mut spans = Vec::new();

        // Iteration counter
        if let Some(iter) = self.data.iteration {
            if let Some(max) = self.data.max_iterations {
                spans.push(Span::raw(format!("{}/{}", iter, max)));
            } else {
                spans.push(Span::raw(format!("Iter {}", iter)));
            }
        }

        // Elapsed time
        let elapsed_secs = self.data.elapsed.as_secs_f64();
        let elapsed_str = if elapsed_secs < 60.0 {
            format!("{:.1}s", elapsed_secs)
        } else {
            let minutes = elapsed_secs as u32 / 60;
            let seconds = (elapsed_secs as u32) % 60;
            format!("{}m{}s", minutes, seconds)
        };

        if !spans.is_empty() {
            spans.push(Span::raw(" | "));
        }
        spans.push(Span::styled(
            elapsed_str,
            Style::default().fg(self.theme.warning),
        ));

        // Optional spinner when running
        if self.data.is_running {
            spans.push(Span::raw(" "));
            let frame_idx = (self.data.elapsed.as_millis() as usize / 100) % SPINNER_FRAMES.len();
            spans.push(Span::styled(
                SPINNER_FRAMES[frame_idx].to_string(),
                Style::default().fg(self.theme.success),
            ));
        }

        spans
    }

    /// Formats the entire header as a single Line.
    /// Left content followed by right content separated by space.
    fn to_line(&self) -> Line<'a> {
        let left = self.left_spans();
        let right = self.right_spans();

        let mut all_spans = left;
        all_spans.push(Span::raw(" "));
        all_spans.extend(right);

        Line::from(all_spans)
    }
}

impl<'a> Widget for Header<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let line = self.to_line();

        // Render the header line at the top of the area
        let x = area.x;
        let y = area.y;

        // Reset the area first
        for x_pos in x..x.saturating_add(area.width) {
            if let Some(cell) = buf.cell_mut((x_pos, y)) {
                cell.reset();
            }
        }

        // Render spans from left to right using display width (not byte length)
        let mut current_x = x;
        for span in line.spans {
            let span_width = UnicodeWidthStr::width(span.content.as_ref()) as u16;
            if current_x + span_width > x + area.width {
                // Truncate at character boundary that fits remaining width
                let remaining = (x + area.width - current_x) as usize;
                let mut truncated_width = 0;
                let mut truncated_end = 0;
                for (idx, ch) in span.content.char_indices() {
                    let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    if truncated_width + ch_width > remaining {
                        break;
                    }
                    truncated_width += ch_width;
                    truncated_end = idx + ch.len_utf8();
                }
                buf.set_string(current_x, y, &span.content[..truncated_end], span.style);
                break;
            }
            buf.set_string(current_x, y, &span.content, span.style);
            current_x += span_width;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::DEFAULT_THEME;
    use ratatui::style::Color;

    fn default_theme() -> &'static Theme {
        &DEFAULT_THEME
    }

    #[test]
    fn header_data_creation() {
        let data = HeaderData {
            command_name: "task run".to_string(),
            model: "claude-3-5-sonnet".to_string(),
            iteration: Some(5),
            max_iterations: Some(10),
            elapsed: Duration::from_secs(120),
            is_running: true,
        };

        assert_eq!(data.command_name, "task run");
        assert_eq!(data.model, "claude-3-5-sonnet");
        assert_eq!(data.iteration, Some(5));
        assert_eq!(data.max_iterations, Some(10));
        assert_eq!(data.elapsed.as_secs(), 120);
        assert!(data.is_running);
    }

    #[test]
    fn header_without_iteration() {
        let data = HeaderData {
            command_name: "init".to_string(),
            model: "claude-3-5-sonnet".to_string(),
            iteration: None,
            max_iterations: None,
            elapsed: Duration::from_secs(5),
            is_running: false,
        };

        let header = Header::new(&data, default_theme());
        let line = header.to_line();

        let content = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(content.contains("init"));
        assert!(content.contains("claude-3-5-sonnet"));
    }

    #[test]
    fn header_spinner_hidden_when_not_running() {
        let data = HeaderData {
            command_name: "test".to_string(),
            model: "model-v1".to_string(),
            iteration: Some(1),
            max_iterations: Some(3),
            elapsed: Duration::from_secs(10),
            is_running: false,
        };

        let header = Header::new(&data, default_theme());
        let line = header.to_line();
        let content = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");

        for frame in SPINNER_FRAMES {
            assert!(
                !content.contains(frame),
                "Spinner should not appear when is_running=false"
            );
        }
    }

    #[test]
    fn header_spinner_shows_when_running() {
        let data = HeaderData {
            command_name: "test".to_string(),
            model: "model-v1".to_string(),
            iteration: Some(1),
            max_iterations: Some(3),
            elapsed: Duration::from_millis(500),
            is_running: true,
        };

        let header = Header::new(&data, default_theme());
        let line = header.to_line();
        let content = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");

        let has_spinner = SPINNER_FRAMES.iter().any(|frame| content.contains(frame));
        assert!(
            has_spinner,
            "Spinner should appear when is_running=true, content: {}",
            content
        );
    }

    #[test]
    fn header_elapsed_time_format_seconds() {
        let data = HeaderData {
            command_name: "cmd".to_string(),
            model: "model".to_string(),
            iteration: None,
            max_iterations: None,
            elapsed: Duration::from_secs_f64(45.5),
            is_running: false,
        };

        let header = Header::new(&data, default_theme());
        let line = header.to_line();
        let content = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");

        assert!(
            content.contains("45.5s"),
            "Should display seconds with one decimal"
        );
    }

    #[test]
    fn header_elapsed_time_format_minutes() {
        let data = HeaderData {
            command_name: "cmd".to_string(),
            model: "model".to_string(),
            iteration: None,
            max_iterations: None,
            elapsed: Duration::from_secs(125),
            is_running: false,
        };

        let header = Header::new(&data, default_theme());
        let line = header.to_line();
        let content = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");

        assert!(
            content.contains("2m5s"),
            "Should display minutes and seconds, got: {}",
            content
        );
    }

    #[test]
    fn header_iteration_with_max() {
        let data = HeaderData {
            command_name: "cmd".to_string(),
            model: "model".to_string(),
            iteration: Some(3),
            max_iterations: Some(10),
            elapsed: Duration::from_secs(5),
            is_running: false,
        };

        let header = Header::new(&data, default_theme());
        let line = header.to_line();
        let content = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");

        assert!(content.contains("3/10"), "Should show iteration/max format");
    }

    #[test]
    fn header_iteration_without_max() {
        let data = HeaderData {
            command_name: "cmd".to_string(),
            model: "model".to_string(),
            iteration: Some(3),
            max_iterations: None,
            elapsed: Duration::from_secs(5),
            is_running: false,
        };

        let header = Header::new(&data, default_theme());
        let line = header.to_line();
        let content = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");

        assert!(
            content.contains("Iter 3"),
            "Should show Iter prefix without max"
        );
    }

    #[test]
    fn header_spinner_frames_cycle() {
        let base_data = HeaderData {
            command_name: "test".to_string(),
            model: "model".to_string(),
            iteration: None,
            max_iterations: None,
            elapsed: Duration::from_millis(0),
            is_running: true,
        };

        // Frame 0: 0ms
        let header = Header::new(&base_data, default_theme());
        let line = header.to_line();
        let content = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(content.contains(SPINNER_FRAMES[0]));

        // Frame 2: 200ms (100ms per frame)
        let data_200ms = HeaderData {
            elapsed: Duration::from_millis(200),
            ..base_data
        };
        let header = Header::new(&data_200ms, default_theme());
        let line = header.to_line();
        let content = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(content.contains(SPINNER_FRAMES[2]));
    }

    #[test]
    fn header_with_custom_theme() {
        let data = HeaderData {
            command_name: "test".to_string(),
            model: "model".to_string(),
            iteration: None,
            max_iterations: None,
            elapsed: Duration::from_secs(1),
            is_running: false,
        };

        let mut custom_theme = DEFAULT_THEME;
        custom_theme.primary = Color::Magenta;

        let header = Header::new(&data, &custom_theme);
        let line = header.to_line();

        if let Some(first_span) = line.spans.first() {
            assert_eq!(
                first_span.style.fg,
                Some(Color::Magenta),
                "Header should use custom theme color"
            );
        }
    }

    #[test]
    fn spinner_frames_are_valid_unicode() {
        for frame in SPINNER_FRAMES {
            assert!(!frame.is_empty(), "Spinner frame should not be empty");
            assert_eq!(
                frame.chars().count(),
                1,
                "Spinner frame should be a single character: {}",
                frame
            );
            // Each Braille char should have display width of 1
            assert_eq!(
                UnicodeWidthStr::width(*frame),
                1,
                "Spinner frame display width should be 1: {}",
                frame
            );
        }
    }

    #[test]
    fn header_render_truncates_when_too_wide() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let data = HeaderData {
            command_name: "very_long_command_name_that_exceeds_area".to_string(),
            model: "very_long_model_name".to_string(),
            iteration: Some(999),
            max_iterations: Some(9999),
            elapsed: Duration::from_secs(9999),
            is_running: true,
        };

        let header = Header::new(&data, default_theme());

        let area = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 1,
        };
        let mut buf = Buffer::empty(area);

        header.render(area, &mut buf);

        assert_eq!(buf.area().width, 20);
    }

    #[test]
    fn header_render_empty_area() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let data = HeaderData {
            command_name: "test".to_string(),
            model: "model".to_string(),
            iteration: None,
            max_iterations: None,
            elapsed: Duration::from_secs(1),
            is_running: false,
        };

        let header = Header::new(&data, default_theme());

        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 0,
        };
        let mut buf = Buffer::empty(Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 1,
        });

        header.render(area, &mut buf);
    }

    #[test]
    fn header_uses_theme_colors_for_model_and_elapsed() {
        let data = HeaderData {
            command_name: "run".to_string(),
            model: "sonnet".to_string(),
            iteration: Some(1),
            max_iterations: None,
            elapsed: Duration::from_secs(5),
            is_running: true,
        };

        let header = Header::new(&data, default_theme());
        let line = header.to_line();

        // Model span (index 2) should use theme.muted
        assert_eq!(line.spans[2].style.fg, Some(DEFAULT_THEME.muted));

        // Find elapsed span — it's styled with theme.warning
        let elapsed_span = line.spans.iter().find(|s| s.content.contains("5.0s"));
        assert!(elapsed_span.is_some());
        assert_eq!(elapsed_span.unwrap().style.fg, Some(DEFAULT_THEME.warning));

        // Find spinner span — it's styled with theme.success
        let spinner_span = line
            .spans
            .iter()
            .find(|s| SPINNER_FRAMES.contains(&s.content.as_ref()));
        assert!(spinner_span.is_some());
        assert_eq!(spinner_span.unwrap().style.fg, Some(DEFAULT_THEME.success));
    }

    // ── Snapshot tests ─────────────────────────────────────────────────

    use crate::test_helpers::{render_widget_to_buffer, snap};

    /// Helper: renderuje header do bufora testowego
    fn render_header(data: &HeaderData, width: u16, height: u16) -> Buffer {
        render_widget_to_buffer(Header::new(data, default_theme()), width, height)
    }

    #[test]
    fn test_snapshot_header_running_with_iteration() {
        let data = HeaderData {
            command_name: "task orchestrate".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            iteration: Some(3),
            max_iterations: Some(10),
            elapsed: Duration::from_secs(42),
            is_running: true,
        };
        let buffer = render_header(&data, 80, 1);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_header_stopped_no_iteration() {
        let data = HeaderData {
            command_name: "run".to_string(),
            model: "claude-opus-4".to_string(),
            iteration: None,
            max_iterations: None,
            elapsed: Duration::from_secs(5),
            is_running: false,
        };
        let buffer = render_header(&data, 80, 1);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_header_running_no_max() {
        let data = HeaderData {
            command_name: "test".to_string(),
            model: "haiku-4".to_string(),
            iteration: Some(7),
            max_iterations: None,
            elapsed: Duration::from_secs(125),
            is_running: true,
        };
        let buffer = render_header(&data, 80, 1);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_header_long_elapsed_time() {
        let data = HeaderData {
            command_name: "task plan".to_string(),
            model: "sonnet".to_string(),
            iteration: Some(1),
            max_iterations: Some(5),
            elapsed: Duration::from_secs(3665), // 1h 1m 5s
            is_running: false,
        };
        let buffer = render_header(&data, 80, 1);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_header_narrow_width() {
        let data = HeaderData {
            command_name: "orchestrate".to_string(),
            model: "claude-sonnet-4-5-20250929".to_string(),
            iteration: Some(99),
            max_iterations: Some(100),
            elapsed: Duration::from_secs(999),
            is_running: true,
        };
        let buffer = render_header(&data, 40, 1);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_header_minimal_info() {
        let data = HeaderData {
            command_name: "cmd".to_string(),
            model: "m".to_string(),
            iteration: None,
            max_iterations: None,
            elapsed: Duration::from_millis(500),
            is_running: false,
        };
        let buffer = render_header(&data, 80, 1);
        insta::assert_snapshot!(snap(&buffer));
    }
}
