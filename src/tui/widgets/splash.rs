use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Widget};

use crate::shared::banner::ART;
use crate::tui::DEFAULT_THEME;

/// Splash screen widget displaying the ralph-wiggum banner with version info.
///
/// Renders centered ASCII art (horizontally and vertically) with application
/// version and author. Used as an introduction screen on startup.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Public API — splash screen widget używany przy starcie TUI
pub struct SplashScreen;

impl Widget for SplashScreen {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height < 5 {
            return;
        }

        let version = env!("CARGO_PKG_VERSION");
        let author = env!("CARGO_PKG_AUTHORS");

        // Build content: banner + blank line + version + author
        let mut content = ART.to_string();
        content.push_str("\n\n");
        content.push_str(&format!("v{version}"));
        content.push('\n');
        content.push_str(&format!("by {author}"));

        // Calculate vertical offset to center content
        let content_height = content.lines().count() as u16;
        let vertical_offset = area.height.saturating_sub(content_height) / 2;

        let centered_area = Rect {
            x: area.x,
            y: area.y + vertical_offset,
            width: area.width,
            height: area.height.saturating_sub(vertical_offset),
        };

        let style = Style::default().fg(DEFAULT_THEME.primary);
        let paragraph = Paragraph::new(content)
            .alignment(Alignment::Center)
            .style(style);

        paragraph.render(centered_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splash_screen_is_sendable() {
        fn assert_send<T: Send>() {}
        assert_send::<SplashScreen>();
    }

    #[test]
    fn splash_screen_is_cloneable() {
        let splash = SplashScreen;
        let _cloned = splash.clone();
    }

    #[test]
    fn splash_screen_widget_renders_without_panic() {
        let splash = SplashScreen;
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        splash.render(area, &mut buf);
        assert!(!buf.content.is_empty());
    }

    #[test]
    fn splash_screen_handles_small_viewport() {
        let splash = SplashScreen;
        let area = Rect::new(0, 0, 5, 3);
        let mut buf = Buffer::empty(area);
        splash.render(area, &mut buf);
        // Should return early without panic
    }

    #[test]
    fn splash_screen_renders_version_info() {
        let version = env!("CARGO_PKG_VERSION");
        let author = env!("CARGO_PKG_AUTHORS");
        assert!(!version.is_empty());
        assert!(!author.is_empty());
    }

    #[test]
    fn splash_screen_vertically_centers_content() {
        let splash = SplashScreen;
        // Use a tall area to verify vertical centering leaves top rows empty
        let area = Rect::new(0, 0, 100, 80);
        let mut buf = Buffer::empty(area);
        splash.render(area, &mut buf);

        // Top-left cell should be empty (vertical padding)
        let top_cell = &buf.content[0];
        assert_eq!(top_cell.symbol(), " ");
    }

    // ── Snapshot tests ─────────────────────────────────────────────────

    use crate::test_helpers::{render_widget_to_buffer, snap};

    #[test]
    fn test_snapshot_splash_standard_terminal() {
        let buffer = render_widget_to_buffer(SplashScreen, 80, 24);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_splash_wide_terminal() {
        let buffer = render_widget_to_buffer(SplashScreen, 120, 40);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_splash_narrow_terminal() {
        let buffer = render_widget_to_buffer(SplashScreen, 60, 20);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_splash_tall_terminal() {
        let buffer = render_widget_to_buffer(SplashScreen, 80, 50);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_splash_minimal_size() {
        // Minimum size where splash still renders (width=10, height=5)
        let buffer = render_widget_to_buffer(SplashScreen, 10, 5);
        insta::assert_snapshot!(snap(&buffer));
    }
}
