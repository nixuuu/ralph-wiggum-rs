use termimad::{FmtText, MadSkin, crossterm::style::Color::*};

/// Create a custom skin for markdown rendering
pub fn create_skin() -> MadSkin {
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
    let skin = create_skin();
    let terminal_width = termimad::terminal_size().0 as usize;
    // Use a reasonable width, capped at terminal width
    let width = terminal_width.min(100);

    // Use FmtText with explicit width to properly wrap text
    let formatted = FmtText::from(&skin, text, Some(width));

    formatted.to_string()
}

/// Render a single line of markdown (inline formatting only)
#[allow(dead_code)]
pub fn render_inline(text: &str) -> String {
    let skin = create_skin();
    // For inline text, we just apply inline formatting
    let formatted = skin.inline(text);
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
}
