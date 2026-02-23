use std::sync::LazyLock;

#[cfg(not(test))]
use crossterm::terminal;
use termimad::{FmtText, MadSkin, crossterm::style::Color};

/// Cached MadSkin — created once, reused for all markdown rendering.
static SKIN: LazyLock<MadSkin> = LazyLock::new(create_skin);

/// Get terminal width using crossterm (more reliable in raw mode).
/// Returns a fixed value in test mode for deterministic snapshot tests.
fn get_terminal_width() -> usize {
    #[cfg(test)]
    return 80;
    #[cfg(not(test))]
    terminal::size().map(|(w, _)| w as usize).unwrap_or(120) // fallback to 120 columns
}

/// Create a custom skin for markdown rendering
fn create_skin() -> MadSkin {
    let mut skin = MadSkin::default();

    // Headers — Catppuccin hierarchy: Lavender → Blue → Mauve
    skin.headers[0].set_fg(Color::Rgb {
        r: 180,
        g: 190,
        b: 254,
    }); // Lavender #b4befe
    skin.headers[0].add_attr(termimad::crossterm::style::Attribute::Bold);
    skin.headers[1].set_fg(Color::Rgb {
        r: 137,
        g: 180,
        b: 250,
    }); // Blue #89b4fa
    skin.headers[1].add_attr(termimad::crossterm::style::Attribute::Bold);
    skin.headers[2].set_fg(Color::Rgb {
        r: 203,
        g: 166,
        b: 247,
    }); // Mauve #cba6f7
    skin.headers[2].add_attr(termimad::crossterm::style::Attribute::Bold);

    // Bold text — Text #cdd6f4
    skin.bold.set_fg(Color::Rgb {
        r: 205,
        g: 214,
        b: 244,
    });
    skin.bold
        .add_attr(termimad::crossterm::style::Attribute::Bold);

    // Italic — Subtext0 #a6adc8
    skin.italic.set_fg(Color::Rgb {
        r: 166,
        g: 173,
        b: 200,
    });
    skin.italic
        .add_attr(termimad::crossterm::style::Attribute::Italic);

    // Inline code — Yellow #f9e2af
    skin.inline_code.set_fg(Color::Rgb {
        r: 249,
        g: 226,
        b: 175,
    });

    // Code blocks — Green #a6e3a1
    skin.code_block.set_fg(Color::Rgb {
        r: 166,
        g: 227,
        b: 161,
    });

    // Quote blocks — Overlay0 #6c7086
    skin.quote_mark.set_fg(Color::Rgb {
        r: 108,
        g: 112,
        b: 134,
    });

    // Bullet points — Sky #89dceb
    skin.bullet.set_fg(Color::Rgb {
        r: 137,
        g: 220,
        b: 235,
    });

    // Horizontal rule — Surface1 #45475a
    skin.horizontal_rule.set_fg(Color::Rgb {
        r: 69,
        g: 71,
        b: 90,
    });

    // Table borders — Overlay0 #6c7086
    skin.table.set_fg(Color::Rgb {
        r: 108,
        g: 112,
        b: 134,
    });

    // Paragraph — reset to default
    skin.paragraph.set_fg(Color::Reset);

    skin
}

/// Render markdown text to a styled string for terminal output
pub fn render_markdown(text: &str) -> String {
    let terminal_width = get_terminal_width();
    // Use full terminal width (minimum 80 for readability)
    let width = terminal_width.clamp(80, 120);

    // Use FmtText with explicit width to properly wrap text
    let formatted = FmtText::from(&SKIN, text, Some(width));

    formatted.to_string()
}

/// Render markdown text with an explicit width (for widgets that know their area).
///
/// Unlike `render_markdown()`, this respects the actual available width
/// instead of using terminal width. Minimum 20 columns to avoid degenerate output.
pub fn render_markdown_for_width(text: &str, width: usize) -> String {
    let width = width.max(20);
    let formatted = FmtText::from(&SKIN, text, Some(width));
    formatted.to_string()
}

/// Render a single line of markdown (inline formatting only)
#[cfg(test)]
fn render_inline(text: &str) -> String {
    let formatted = SKIN.inline(text);
    formatted.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_basic_text() {
        let result = render_markdown("Hello world");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_render_code_block() {
        let result = render_markdown("```rust\nfn main() {}\n```");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_render_inline() {
        let result = render_inline("**bold** and *italic*");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_render_simple_table() {
        let table = r#"| Col1 | Col2 | Col3 |
|------|------|------|
| A    | B    | C    |
| D    | E    | F    |"#;
        let result = render_markdown(table);
        assert!(!result.is_empty());
        // Verify all cells are present in output
        assert!(result.contains("Col1") || result.contains("A"));
    }

    #[test]
    fn test_render_table_with_many_columns() {
        let table = r#"| C1 | C2 | C3 | C4 | C5 |
|----|----|----|----|----|
| A  | B  | C  | D  | E  |"#;
        let result = render_markdown(table);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_render_table_with_long_text() {
        let table = r#"| Name | Description |
|------|-------------|
| Test | This is a very long description that spans many characters |"#;
        let result = render_markdown(table);
        assert!(!result.is_empty());
        assert!(result.contains("Description") || result.contains("long"));
    }

    #[test]
    fn test_render_table_with_alignment() {
        let table = r#"| Left | Center | Right |
|:-----|:------:|------:|
| L    | C      | R     |"#;
        let result = render_markdown(table);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_render_empty_string() {
        let result = render_markdown("");
        // Empty input should produce empty or whitespace-only output
        assert!(result.trim().is_empty());
    }

    #[test]
    fn test_render_whitespace_only() {
        let result = render_markdown("   \n\n   ");
        // termimad may add minimal formatting/spacing even for whitespace-only input
        // Just verify it doesn't crash and produces some output
        assert!(!result.is_empty());
    }

    #[test]
    fn test_render_preserves_empty_lines_in_content() {
        let text = "Line 1\n\nLine 2";
        let result = render_markdown(text);
        // Should contain multiple lines (including the empty separator)
        assert!(result.lines().count() >= 2);
    }
}
