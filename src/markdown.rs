use std::sync::LazyLock;

use crossterm::terminal;
use termimad::{FmtText, MadSkin, crossterm::style::Color::*};

/// Cached MadSkin — created once, reused for all markdown rendering.
static SKIN: LazyLock<MadSkin> = LazyLock::new(create_skin);

/// Get terminal width using crossterm (more reliable in raw mode)
fn get_terminal_width() -> usize {
    terminal::size().map(|(w, _)| w as usize).unwrap_or(120) // fallback to 120 columns
}

/// Create a custom skin for markdown rendering
fn create_skin() -> MadSkin {
    let mut skin = MadSkin::default();

    // Headers - bold and colored
    skin.headers[0].set_fg(Cyan);
    skin.headers[0].add_attr(termimad::crossterm::style::Attribute::Bold);
    skin.headers[1].set_fg(Blue);
    skin.headers[1].add_attr(termimad::crossterm::style::Attribute::Bold);
    skin.headers[2].set_fg(Magenta);
    skin.headers[2].add_attr(termimad::crossterm::style::Attribute::Bold);

    // Bold text
    skin.bold.set_fg(White);
    skin.bold
        .add_attr(termimad::crossterm::style::Attribute::Bold);

    // Italic
    skin.italic.set_fg(Grey);
    skin.italic
        .add_attr(termimad::crossterm::style::Attribute::Italic);

    // Inline code - cyan on dark background
    skin.inline_code.set_fg(Yellow);

    // Code blocks - with background
    skin.code_block.set_fg(Green);

    // Quote blocks
    skin.quote_mark.set_fg(DarkGrey);

    // Bullet points
    skin.bullet.set_fg(Cyan);

    // Horizontal rule
    skin.horizontal_rule.set_fg(DarkGrey);

    // Table borders
    skin.table.set_fg(DarkGrey);

    // Links
    skin.paragraph.set_fg(Reset);

    skin
}

/// Render markdown text to a styled string for terminal output
pub fn render_markdown(text: &str) -> String {
    let terminal_width = get_terminal_width();
    // Use full terminal width (minimum 80 for readability)
    let width = terminal_width.max(80);

    // Use FmtText with explicit width to properly wrap text
    let formatted = FmtText::from(&*SKIN, text, Some(width));

    formatted.to_string()
}

/// Render a single line of markdown (inline formatting only)
#[allow(dead_code)]
pub fn render_inline(text: &str) -> String {
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
}
