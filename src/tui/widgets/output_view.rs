use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Paragraph, StatefulWidget, Widget, Wrap};

use crate::tui::ring_buffer::OutputRingBuffer;

/// Scroll state for OutputView, persisted between renders.
///
/// `scroll_offset` counts visual rows from the bottom (0 = pinned to bottom).
/// When `auto_follow` is true, new content auto-scrolls to bottom.
pub struct OutputViewState {
    pub scroll_offset: usize,
    pub auto_follow: bool,
}

impl Default for OutputViewState {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            auto_follow: true,
        }
    }
}

impl OutputViewState {
    /// Scroll up by `n` visual rows. Disables auto-follow.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
        self.auto_follow = false;
    }

    /// Scroll down by `n` visual rows. Re-enables auto-follow when reaching bottom.
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        if self.scroll_offset == 0 {
            self.auto_follow = true;
        }
    }

    /// Jump to the beginning of content. Disables auto-follow.
    pub fn scroll_home(&mut self, total_visual_rows: usize, viewport_height: usize) {
        self.scroll_offset = total_visual_rows.saturating_sub(viewport_height);
        self.auto_follow = false;
    }

    /// Jump to the end (bottom). Re-enables auto-follow.
    pub fn scroll_end(&mut self) {
        self.scroll_offset = 0;
        self.auto_follow = true;
    }

    /// Clamp scroll_offset to valid range based on content size and viewport.
    fn clamp_offset(&mut self, total_visual_rows: usize, viewport_height: usize) {
        let max_offset = total_visual_rows.saturating_sub(viewport_height);
        self.scroll_offset = self.scroll_offset.min(max_offset);
    }
}

/// Scrollable output widget rendering lines from an OutputRingBuffer.
///
/// Supports auto-follow (scroll to bottom on new content) and manual scrolling.
/// Visual line wrapping is accounted for via ring buffer's visual row API.
/// Optionally wraps content in a Block border with title.
pub struct OutputView<'a> {
    buffer: &'a OutputRingBuffer,
    block: Option<Block<'a>>,
    dimmed: bool,
}

impl<'a> OutputView<'a> {
    pub fn new(buffer: &'a OutputRingBuffer) -> Self {
        Self {
            buffer,
            block: None,
            dimmed: false,
        }
    }

    /// Set optional Block border/title around the widget.
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Dim the content area when another widget (e.g. ask_user) requires focus.
    pub fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self
    }
}

impl<'a> StatefulWidget for OutputView<'a> {
    type State = OutputViewState;

    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer, state: &mut Self::State) {
        // Compute inner area (accounting for block borders)
        let inner = if let Some(ref block) = self.block {
            block.inner(area)
        } else {
            area
        };

        // Render block border if present
        if let Some(block) = self.block {
            block.render(area, buf);
        }

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let viewport_height = inner.height as usize;
        let panel_width = inner.width;
        let total_visual = self.buffer.total_visual_rows(panel_width);

        // Auto-follow: reset offset to 0 (bottom)
        if state.auto_follow {
            state.scroll_offset = 0;
        }

        // Clamp offset to valid range
        state.clamp_offset(total_visual, viewport_height);

        // Fetch visible lines from ring buffer
        let lines = if state.scroll_offset == 0 {
            self.buffer.tail_visual(viewport_height, panel_width)
        } else {
            self.buffer
                .slice_visual(state.scroll_offset, viewport_height, panel_width)
        };

        // Render lines as a Paragraph with wrapping
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        paragraph.render(inner, buf);

        // Dim the entire area when another widget requires focus
        if self.dimmed {
            let dim_style = Style::default().add_modifier(Modifier::DIM);
            for y in inner.y..inner.y + inner.height {
                for x in inner.x..inner.x + inner.width {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_style(dim_style);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::snap;
    use ratatui::backend::TestBackend;
    use ratatui::prelude::Terminal;
    use ratatui::widgets::{Block, Borders};

    /// Helper: render OutputView with state to a snapshot string.
    fn render_output_view(
        buffer: &OutputRingBuffer,
        state: &mut OutputViewState,
        block: Option<Block<'_>>,
        width: u16,
        height: u16,
    ) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, height);
                let mut widget = OutputView::new(buffer);
                if let Some(b) = block {
                    widget = widget.block(b);
                }
                frame.render_stateful_widget(widget, area, state);
            })
            .expect("draw");
        snap(terminal.backend().buffer())
    }

    // ── Auto-follow tests ────────────────────────────────────────────

    #[test]
    fn test_snapshot_auto_follow_basic() {
        let mut buf = OutputRingBuffer::with_capacity(20);
        for i in 1..=10 {
            buf.push_str(&format!("Line {i}"));
        }
        let mut state = OutputViewState::default();
        // Viewport of 5 rows — should show last 5 lines
        let snapshot = render_output_view(&buf, &mut state, None, 40, 5);
        insta::assert_snapshot!(snapshot);
        assert!(state.auto_follow);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_snapshot_auto_follow_with_block() {
        let mut buf = OutputRingBuffer::with_capacity(20);
        for i in 1..=8 {
            buf.push_str(&format!("Line {i}"));
        }
        let mut state = OutputViewState::default();
        let block = Block::default().borders(Borders::ALL).title(" Output ");
        // Height 7 = 2 (borders) + 5 (inner) — should show last 5 lines
        let snapshot = render_output_view(&buf, &mut state, Some(block), 40, 7);
        insta::assert_snapshot!(snapshot);
    }

    // ── Manual scroll tests ──────────────────────────────────────────

    #[test]
    fn test_snapshot_manual_scroll_up() {
        let mut buf = OutputRingBuffer::with_capacity(20);
        for i in 1..=10 {
            buf.push_str(&format!("Line {i}"));
        }
        let mut state = OutputViewState::default();
        // Scroll up 3 visual rows
        state.scroll_up(3);
        assert!(!state.auto_follow);
        assert_eq!(state.scroll_offset, 3);

        let snapshot = render_output_view(&buf, &mut state, None, 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_manual_scroll_to_top() {
        let mut buf = OutputRingBuffer::with_capacity(20);
        for i in 1..=10 {
            buf.push_str(&format!("Line {i}"));
        }
        let mut state = OutputViewState::default();
        // Scroll to home (top of content)
        // total_visual_rows = 10, viewport = 5
        state.scroll_home(10, 5);
        assert!(!state.auto_follow);
        assert_eq!(state.scroll_offset, 5);

        let snapshot = render_output_view(&buf, &mut state, None, 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_scroll_end_re_enables_auto_follow() {
        let mut buf = OutputRingBuffer::with_capacity(20);
        for i in 1..=10 {
            buf.push_str(&format!("Line {i}"));
        }
        let mut state = OutputViewState::default();
        state.scroll_up(5);
        assert!(!state.auto_follow);

        state.scroll_end();
        assert!(state.auto_follow);
        assert_eq!(state.scroll_offset, 0);

        let snapshot = render_output_view(&buf, &mut state, None, 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── Empty buffer tests ───────────────────────────────────────────

    #[test]
    fn test_snapshot_empty_buffer() {
        let buf = OutputRingBuffer::with_capacity(10);
        let mut state = OutputViewState::default();
        let snapshot = render_output_view(&buf, &mut state, None, 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_empty_buffer_with_block() {
        let buf = OutputRingBuffer::with_capacity(10);
        let mut state = OutputViewState::default();
        let block = Block::default().borders(Borders::ALL).title(" Empty ");
        let snapshot = render_output_view(&buf, &mut state, Some(block), 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── Overflow / capacity tests ────────────────────────────────────

    #[test]
    fn test_snapshot_overflow_buffer() {
        let mut buf = OutputRingBuffer::with_capacity(5);
        for i in 1..=20 {
            buf.push_str(&format!("Line {i}"));
        }
        let mut state = OutputViewState::default();
        // Buffer capacity=5, pushed 20 lines → only last 5 remain
        let snapshot = render_output_view(&buf, &mut state, None, 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_overflow_scroll_clamped() {
        let mut buf = OutputRingBuffer::with_capacity(5);
        for i in 1..=20 {
            buf.push_str(&format!("Line {i}"));
        }
        let mut state = OutputViewState::default();
        // Scroll up way past buffer content — should be clamped
        state.scroll_up(1000);
        let snapshot = render_output_view(&buf, &mut state, None, 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── Visual wrapping tests ────────────────────────────────────────

    #[test]
    fn test_snapshot_visual_wrapping() {
        let mut buf = OutputRingBuffer::with_capacity(20);
        buf.push_str("Short");
        buf.push_str(&"X".repeat(80)); // 2 visual rows at width 40
        buf.push_str("End");

        let mut state = OutputViewState::default();
        let snapshot = render_output_view(&buf, &mut state, None, 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_visual_wrapping_scroll_up() {
        let mut buf = OutputRingBuffer::with_capacity(20);
        buf.push_str("First");
        buf.push_str("Second");
        buf.push_str(&"L".repeat(80)); // 2 visual rows at width 40
        buf.push_str("After long");
        buf.push_str("Last");

        let mut state = OutputViewState::default();
        // Scroll up 2 visual rows — should skip "Last" and "After long"
        state.scroll_up(2);
        let snapshot = render_output_view(&buf, &mut state, None, 40, 4);
        insta::assert_snapshot!(snapshot);
    }

    // ── Scroll method unit tests ─────────────────────────────────────

    #[test]
    fn test_scroll_down_re_enables_auto_follow_at_bottom() {
        let mut state = OutputViewState::default();
        state.scroll_up(3);
        assert!(!state.auto_follow);

        state.scroll_down(2);
        assert_eq!(state.scroll_offset, 1);
        assert!(!state.auto_follow);

        state.scroll_down(1);
        assert_eq!(state.scroll_offset, 0);
        assert!(state.auto_follow); // re-enabled at bottom
    }

    #[test]
    fn test_scroll_down_saturates_at_zero() {
        let mut state = OutputViewState::default();
        state.scroll_up(2);
        state.scroll_down(10); // more than offset
        assert_eq!(state.scroll_offset, 0);
        assert!(state.auto_follow);
    }

    #[test]
    fn test_scroll_home_with_small_content() {
        let mut state = OutputViewState::default();
        // Content fits in viewport — home should set offset to 0
        state.scroll_home(3, 5);
        assert_eq!(state.scroll_offset, 0);
        assert!(!state.auto_follow);
    }

    #[test]
    fn test_clamp_offset() {
        let mut state = OutputViewState {
            scroll_offset: 100,
            auto_follow: false,
        };
        // total_visual=10, viewport=5 → max_offset=5
        state.clamp_offset(10, 5);
        assert_eq!(state.scroll_offset, 5);
    }

    #[test]
    fn test_clamp_offset_no_change_when_valid() {
        let mut state = OutputViewState {
            scroll_offset: 3,
            auto_follow: false,
        };
        state.clamp_offset(10, 5);
        assert_eq!(state.scroll_offset, 3);
    }

    #[test]
    fn test_default_state() {
        let state = OutputViewState::default();
        assert_eq!(state.scroll_offset, 0);
        assert!(state.auto_follow);
    }

    // ── Dimmed mode tests ────────────────────────────────────────────

    fn render_to_buffer(
        buffer: &OutputRingBuffer,
        state: &mut OutputViewState,
        dimmed: bool,
        width: u16,
        height: u16,
    ) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, height);
                let widget = OutputView::new(buffer).dimmed(dimmed);
                frame.render_stateful_widget(widget, area, state);
            })
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    #[test]
    fn test_dimmed_true_applies_dim_modifier() {
        let mut buf = OutputRingBuffer::with_capacity(10);
        buf.push_str("Hello world");
        let mut state = OutputViewState::default();

        let buffer = render_to_buffer(&buf, &mut state, true, 20, 3);

        // Every rendered cell should have DIM modifier
        for y in 0..3 {
            for x in 0..20 {
                let cell = buffer.cell((x, y)).expect("cell exists");
                assert!(
                    cell.modifier.contains(ratatui::style::Modifier::DIM),
                    "cell ({x},{y}) missing DIM modifier"
                );
            }
        }
    }

    #[test]
    fn test_dimmed_false_no_dim_modifier() {
        let mut buf = OutputRingBuffer::with_capacity(10);
        buf.push_str("Hello world");
        let mut state = OutputViewState::default();

        let buffer = render_to_buffer(&buf, &mut state, false, 20, 3);

        // No cell should have DIM modifier when dimmed=false
        for y in 0..3 {
            for x in 0..20 {
                let cell = buffer.cell((x, y)).expect("cell exists");
                assert!(
                    !cell.modifier.contains(ratatui::style::Modifier::DIM),
                    "cell ({x},{y}) unexpectedly has DIM modifier"
                );
            }
        }
    }
}
