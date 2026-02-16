// Test helpers for ratatui snapshot testing.
// Provides utilities to render widgets to a Buffer and convert them to snapshot-friendly strings.

use ratatui::{
    backend::TestBackend, buffer::Buffer, layout::Rect, prelude::Terminal, widgets::Widget,
};

/// Renders a widget to a Buffer for snapshot testing.
///
/// Creates a test backend with the specified dimensions,
/// renders the given widget to it, and returns the resulting buffer.
///
/// # Example
/// ```ignore
/// use crate::test_helpers::{render_widget_to_buffer, snap};
/// let buffer = render_widget_to_buffer(Text::raw("Hello"), 10, 5);
/// insta::assert_snapshot!(snap(&buffer));
/// ```
pub fn render_widget_to_buffer<W: Widget>(widget: W, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("Failed to create test terminal");
    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            frame.render_widget(widget, area);
        })
        .expect("Failed to draw widget");
    terminal.backend().buffer().clone()
}

/// Converts a Buffer to a human-readable string for snapshot testing.
///
/// Correctly handles wide characters (CJK, emoji) by skipping
/// cells hidden behind multi-width symbols.
///
/// # Example
/// ```ignore
/// let buffer = render_widget_to_buffer(Text::raw("Test"), 10, 3);
/// insta::assert_snapshot!(snap(&buffer));
/// ```
pub fn snap(buffer: &Buffer) -> String {
    use unicode_width::UnicodeWidthStr;

    let mut output = String::with_capacity(
        buffer.area.width as usize * buffer.area.height as usize + buffer.area.height as usize,
    );

    for y in 0..buffer.area.height {
        let mut skip: usize = 0;
        for x in 0..buffer.area.width {
            let cell = buffer.cell((x, y)).expect("Valid cell coordinates");
            if skip == 0 {
                output.push_str(cell.symbol());
            }
            // Wide chars occupy multiple cells; skip the trailing placeholder cells
            skip = std::cmp::max(skip, cell.symbol().width()).saturating_sub(1);
        }
        // Trim trailing spaces for cleaner snapshots
        let trimmed_len = output.trim_end_matches(' ').len();
        output.truncate(trimmed_len);
        output.push('\n');
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Text;

    #[test]
    fn test_render_widget_to_buffer_dimensions() {
        let buffer = render_widget_to_buffer(Text::raw("Hi"), 10, 5);
        assert_eq!(buffer.area.width, 10);
        assert_eq!(buffer.area.height, 5);
    }

    #[test]
    fn test_render_widget_to_buffer_content() {
        let buffer = render_widget_to_buffer(Text::raw("Hello"), 10, 1);
        // First 5 cells should contain "Hello"
        let content: String = (0..5)
            .map(|x| {
                buffer
                    .cell((x, 0))
                    .expect("valid cell")
                    .symbol()
                    .to_string()
            })
            .collect();
        assert_eq!(content, "Hello");
    }

    #[test]
    fn test_snap_basic() {
        let buffer = render_widget_to_buffer(Text::raw("ABC"), 5, 1);
        let snapshot = snap(&buffer);
        assert_eq!(snapshot, "ABC\n");
    }

    #[test]
    fn test_snap_multiline() {
        let buffer = render_widget_to_buffer(Text::raw("Line1\nLine2"), 10, 2);
        let snapshot = snap(&buffer);
        assert_eq!(snapshot, "Line1\nLine2\n");
    }

    #[test]
    fn test_snap_empty_buffer() {
        let buffer = render_widget_to_buffer(Text::raw(""), 5, 3);
        let snapshot = snap(&buffer);
        // Empty lines should be trimmed to just newlines
        assert_eq!(snapshot, "\n\n\n");
    }

    #[test]
    fn test_snap_with_insta() {
        let buffer = render_widget_to_buffer(Text::raw("Snapshot"), 12, 1);
        insta::assert_snapshot!(snap(&buffer), @"Snapshot");
    }
}
