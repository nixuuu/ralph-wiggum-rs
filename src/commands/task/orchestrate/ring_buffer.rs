use std::collections::VecDeque;

use ansi_to_tui::IntoText;
use ratatui::text::Line;

/// Ring buffer holding last N lines of worker output as ratatui Lines.
pub struct OutputRingBuffer {
    lines: VecDeque<Line<'static>>,
    capacity: usize,
}

impl OutputRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Push text (potentially with ANSI codes) as one or more Lines.
    pub fn push(&mut self, text: &str) {
        // Convert ANSI→ratatui; fallback to plain text on error
        let rlines = text
            .into_text()
            .map(|t| t.lines)
            .unwrap_or_else(|_| vec![Line::raw(text.to_string())]);

        for line in rlines {
            if self.lines.len() >= self.capacity {
                self.lines.pop_front();
            }
            self.lines.push_back(line);
        }
    }

    /// Get the last `n` lines for rendering.
    pub fn tail(&self, n: usize) -> Vec<Line<'static>> {
        let skip = self.lines.len().saturating_sub(n);
        self.lines.iter().skip(skip).cloned().collect()
    }

    /// Get a slice of lines from `start` to `end` (exclusive).
    /// Used for manual scrolling in the dashboard.
    pub fn slice(&self, start: usize, end: usize) -> Vec<Line<'static>> {
        self.lines
            .iter()
            .skip(start)
            .take(end.saturating_sub(start))
            .cloned()
            .collect()
    }

    #[allow(dead_code)] // Used in tests
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Get the last lines that fit within `visual_rows` visual rows,
    /// accounting for line wrapping at `panel_width`.
    ///
    /// Each logical line occupies ceil(line.width() / panel_width) visual rows
    /// (minimum 1). Iterates from the end (newest) and collects lines until
    /// the visual row budget is exhausted.
    pub fn tail_visual(&self, visual_rows: usize, panel_width: u16) -> Vec<Line<'static>> {
        if visual_rows == 0 {
            return Vec::new();
        }
        // Fallback: if panel_width is 0, we can't compute wrapping
        if panel_width == 0 {
            return self.tail(visual_rows);
        }

        let pw = panel_width as usize;
        let mut result = VecDeque::new();
        let mut used = 0usize;

        for line in self.lines.iter().rev() {
            let w = line.width();
            let visual_h = if w == 0 { 1 } else { w.div_ceil(pw) };

            // Stop if adding this line would exceed budget (unless result is empty)
            if used + visual_h > visual_rows && !result.is_empty() {
                break;
            }

            result.push_front(line.clone());
            used += visual_h;

            // Stop if we've filled the budget
            if used >= visual_rows {
                break;
            }
        }

        result.into()
    }

    /// Compute total visual rows occupied by all lines in the buffer,
    /// accounting for wrapping at `panel_width`.
    ///
    /// Each logical line occupies ceil(line.width() / panel_width) visual rows
    /// (minimum 1). Returns the sum of all visual heights.
    pub fn total_visual_rows(&self, panel_width: u16) -> usize {
        if panel_width == 0 {
            return self.lines.len();
        }
        let pw = panel_width as usize;
        self.lines
            .iter()
            .map(|line| {
                let w = line.width();
                if w == 0 { 1 } else { w.div_ceil(pw) }
            })
            .sum()
    }

    /// Get lines from a visual scroll offset, fitting within `visual_rows` rows,
    /// accounting for wrapping at `panel_width`.
    ///
    /// `visual_offset` counts visual rows from the bottom (0 = no scroll).
    /// Returns the logical lines whose visual rows fill the viewport.
    pub fn slice_visual(
        &self,
        visual_offset: usize,
        visual_rows: usize,
        panel_width: u16,
    ) -> Vec<Line<'static>> {
        if visual_rows == 0 {
            return Vec::new();
        }

        // Optimization: offset=0 → same as tail_visual
        if visual_offset == 0 {
            return self.tail_visual(visual_rows, panel_width);
        }

        if panel_width == 0 {
            // Fallback: offset from bottom (like tail_visual but with offset)
            let total = self.lines.len();
            // visual_offset from bottom: skip last N lines
            let end = total.saturating_sub(visual_offset);
            let start = end.saturating_sub(visual_rows);
            return self.slice(start, end);
        }

        let pw = panel_width as usize;

        // First: compute visual heights for all lines from the end,
        // and find where the offset lands.
        // visual_offset = how many visual rows to skip from the bottom.
        let mut skip_visual = 0usize;
        let mut end_logical = self.lines.len(); // exclusive end index

        // Skip `visual_offset` visual rows from the bottom
        for (i, line) in self.lines.iter().rev().enumerate() {
            if skip_visual >= visual_offset {
                end_logical = self.lines.len() - i;
                break;
            }
            let w = line.width();
            let vh = if w == 0 { 1 } else { w.div_ceil(pw) };
            skip_visual += vh;
        }

        // If we consumed all lines while skipping, start from beginning
        if skip_visual < visual_offset {
            end_logical = 0;
        }

        // Now collect lines going backwards from end_logical until we fill visual_rows
        let mut result = VecDeque::new();
        let mut used = 0usize;

        for idx in (0..end_logical).rev() {
            let line = &self.lines[idx];
            let w = line.width();
            let vh = if w == 0 { 1 } else { w.div_ceil(pw) };

            // Stop if adding this line would exceed budget (unless result is empty)
            if used + vh > visual_rows && !result.is_empty() {
                break;
            }

            result.push_front(line.clone());
            used += vh;

            // Stop if we've filled the budget
            if used >= visual_rows {
                break;
            }
        }

        result.into()
    }

    /// Clear all lines from the buffer.
    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_ring_buffer_empty() {
        let buf = OutputRingBuffer::new(10);
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
        let tail = buf.tail(10);
        assert_eq!(tail.len(), 0);
    }

    #[test]
    fn test_output_ring_buffer_capacity() {
        let mut buf = OutputRingBuffer::new(3);
        buf.push("line 1");
        buf.push("line 2");
        buf.push("line 3");
        buf.push("line 4");
        assert_eq!(buf.len(), 3);
        assert!(!buf.is_empty());
        let tail = buf.tail(10);
        // First element should be "line 2" (line 1 was evicted)
        assert_eq!(tail.len(), 3);
    }

    #[test]
    fn test_output_ring_buffer_tail() {
        let mut buf = OutputRingBuffer::new(10);
        for i in 0..5 {
            buf.push(&format!("line {i}"));
        }
        let tail = buf.tail(2);
        assert_eq!(tail.len(), 2);
    }

    #[test]
    fn test_output_ring_buffer_slice() {
        let mut buf = OutputRingBuffer::new(10);
        for i in 0..5 {
            buf.push(&format!("line {i}"));
        }
        // Get middle slice (indices 1-3)
        let slice = buf.slice(1, 3);
        assert_eq!(slice.len(), 2);

        // Slice beyond buffer length
        let slice = buf.slice(3, 10);
        assert_eq!(slice.len(), 2); // Only lines 3-4 exist

        // Invalid range (start >= end)
        let slice = buf.slice(3, 2);
        assert_eq!(slice.len(), 0);
    }

    #[test]
    fn test_output_ring_buffer_clear() {
        let mut buf = OutputRingBuffer::new(10);
        for i in 0..5 {
            buf.push(&format!("line {i}"));
        }
        assert_eq!(buf.len(), 5);

        buf.clear();
        assert_eq!(buf.len(), 0);

        let tail = buf.tail(10);
        assert_eq!(tail.len(), 0);
    }

    #[test]
    fn test_push_markdown_formatted_text() {
        use crate::shared::markdown::render_markdown;

        let mut buf = OutputRingBuffer::new(10);
        let raw_markdown = "## Header\n- item 1\n- item 2\n```rust\nfn main() {}\n```";
        let formatted = render_markdown(raw_markdown);

        buf.push(&formatted);

        // Verify that Lines have styling (non-empty spans with colors)
        let lines = buf.tail(20);
        assert!(!lines.is_empty(), "Buffer should contain formatted lines");

        // At least some lines should have styled spans (not just plain text)
        let has_styled = lines.iter().any(|line| {
            !line.spans.is_empty()
                && line.spans.iter().any(|span| {
                    // Check if span has style attributes (color, modifiers, etc.)
                    span.style.fg.is_some()
                        || span.style.bg.is_some()
                        || !span.style.add_modifier.is_empty()
                })
        });

        assert!(
            has_styled,
            "Markdown-formatted text should produce Lines with styling"
        );
    }

    #[test]
    fn test_push_plain_text_status_unmodified() {
        let mut buf = OutputRingBuffer::new(10);
        let plain_status = "✓ Conflicts resolved";

        buf.push(plain_status);

        let lines = buf.tail(10);
        assert_eq!(lines.len(), 1, "Should have exactly one line");

        // Plain text without ANSI codes should produce a single span
        // with the original text
        let line = &lines[0];
        assert!(!line.spans.is_empty(), "Line should have at least one span");

        // The text content should be preserved
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, plain_status);

        // Plain text should NOT have styling (no colors/modifiers)
        let has_styling = line.spans.iter().any(|span| {
            span.style.fg.is_some()
                || span.style.bg.is_some()
                || !span.style.add_modifier.is_empty()
        });
        assert!(
            !has_styling,
            "Plain text status should not have styling added"
        );
    }

    #[test]
    fn test_end_to_end_pipeline() {
        use crate::shared::markdown::render_markdown;

        let mut buf = OutputRingBuffer::new(20);

        // Simulate the orchestrator_merge.rs pipeline:
        // 1. render_markdown() on text from Claude
        let markdown_text = "**Bold text**\n\n- List item 1\n- List item 2\n\n`inline code`";
        let formatted = render_markdown(markdown_text);

        // 2. Push to ring buffer
        buf.push(&formatted);

        // 3. Retrieve with tail()
        let lines = buf.tail(10);

        // Verify:
        // - Lines were extracted successfully
        assert!(!lines.is_empty(), "Pipeline should produce output lines");

        // - Some styling is present
        let has_any_style = lines.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.style.fg.is_some()
                    || span.style.bg.is_some()
                    || !span.style.add_modifier.is_empty()
            })
        });

        assert!(
            has_any_style,
            "End-to-end pipeline should preserve markdown styling"
        );
    }

    #[test]
    fn test_capacity_with_formatted_text() {
        use crate::shared::markdown::render_markdown;

        let capacity = 5;
        let mut buf = OutputRingBuffer::new(capacity);

        // Push single-line markdown snippets
        for i in 0..10 {
            let md = format!("**Item {i}**");
            let formatted = render_markdown(&md);
            buf.push(&formatted);
        }

        // Buffer should respect capacity
        assert!(
            buf.len() <= capacity,
            "Buffer should evict old lines when capacity is exceeded (got {}, expected <= {})",
            buf.len(),
            capacity
        );

        // Tail should work correctly
        let tail = buf.tail(3);
        assert!(!tail.is_empty());
    }

    #[test]
    fn test_multiline_markdown_produces_multiple_lines() {
        use crate::shared::markdown::render_markdown;

        let mut buf = OutputRingBuffer::new(20);

        // Multi-line markdown input
        let markdown = "Line 1\n\nLine 2\n\nLine 3";
        let formatted = render_markdown(markdown);
        buf.push(&formatted);

        // Should produce multiple Lines (at least 3, possibly more with spacing)
        let lines = buf.tail(20);
        assert!(
            lines.len() >= 3,
            "Multi-line markdown should produce multiple Lines (got {})",
            lines.len()
        );
    }

    // ── tail_visual tests ──────────────────────────────────────────────

    #[test]
    fn test_tail_visual_short_lines_same_as_tail() {
        // Lines shorter than panel width → tail_visual == tail
        let mut buf = OutputRingBuffer::new(20);
        for i in 0..5 {
            buf.push(&format!("line {i}")); // ~6 chars each
        }
        let panel_width = 40;
        let visual = buf.tail_visual(3, panel_width);
        let logical = buf.tail(3);
        assert_eq!(visual.len(), logical.len());
    }

    #[test]
    fn test_tail_visual_long_line_reduces_count() {
        // A single line of 80 chars in a 40-wide panel = 2 visual rows
        // So requesting 3 visual rows should yield 2 logical lines max
        let mut buf = OutputRingBuffer::new(20);
        buf.push("short");
        buf.push("short");
        buf.push("short");
        buf.push(&"X".repeat(80)); // 2 visual rows in 40-wide panel

        let result = buf.tail_visual(3, 40);
        // The long line (80 chars) takes 2 visual rows, then 1 short line takes 1 row = 3 total
        assert_eq!(result.len(), 2);
        // The last line should be the long one (most recent)
        let last_text: String = result
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(last_text.len(), 80);
    }

    #[test]
    fn test_tail_visual_empty_lines_count_as_one() {
        let mut buf = OutputRingBuffer::new(20);
        buf.push("line1");
        buf.push(""); // empty → 1 visual row
        buf.push("line3");

        let result = buf.tail_visual(3, 40);
        // 3 lines, each 1 visual row → fits exactly
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_tail_visual_zero_rows() {
        let mut buf = OutputRingBuffer::new(10);
        buf.push("line1");
        assert_eq!(buf.tail_visual(0, 40).len(), 0);
    }

    #[test]
    fn test_tail_visual_zero_width_fallback() {
        let mut buf = OutputRingBuffer::new(10);
        buf.push("line1");
        buf.push("line2");
        buf.push("line3");
        // panel_width=0 → fallback to tail()
        let result = buf.tail_visual(2, 0);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_tail_visual_very_long_line_exceeds_budget() {
        // A single line of 200 chars in 40-wide panel = 5 visual rows
        // Requesting only 3 visual rows — the line still gets included (at least 1 line)
        let mut buf = OutputRingBuffer::new(10);
        buf.push(&"A".repeat(200));

        let result = buf.tail_visual(3, 40);
        assert_eq!(result.len(), 1); // Still returns the line even though it overflows
    }

    #[test]
    fn test_tail_visual_mixed_widths() {
        let mut buf = OutputRingBuffer::new(20);
        buf.push("short1"); // 1 visual row
        buf.push(&"M".repeat(80)); // 2 visual rows (in 40-wide)
        buf.push("short2"); // 1 visual row
        buf.push(&"L".repeat(120)); // 3 visual rows (in 40-wide)
        buf.push("short3"); // 1 visual row

        // 8 visual rows available: should fit short3(1)+L(3)+short2(1)+M(2) = 7, next would add short1(1)=8
        let result = buf.tail_visual(8, 40);
        assert_eq!(result.len(), 5); // All lines fit

        // 4 visual rows: short3(1)+L(3)=4
        let result = buf.tail_visual(4, 40);
        assert_eq!(result.len(), 2);
    }

    // ── slice_visual tests ─────────────────────────────────────────────

    #[test]
    fn test_slice_visual_no_offset() {
        let mut buf = OutputRingBuffer::new(20);
        for i in 0..5 {
            buf.push(&format!("line {i}"));
        }
        // offset=0 should behave like tail_visual
        let result = buf.slice_visual(0, 3, 40);
        let tail = buf.tail_visual(3, 40);
        assert_eq!(result.len(), tail.len());
    }

    #[test]
    fn test_slice_visual_with_offset() {
        let mut buf = OutputRingBuffer::new(20);
        for i in 0..10 {
            buf.push(&format!("line {i}"));
        }
        // offset=2 visual rows from bottom, show 3 visual rows
        let result = buf.slice_visual(2, 3, 40);
        assert_eq!(result.len(), 3);
        // Should show lines 5,6,7 (skipping last 2: lines 8,9)
        let first_text: String = result[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(first_text, "line 5");
    }

    #[test]
    fn test_slice_visual_zero_width_fallback() {
        let mut buf = OutputRingBuffer::new(10);
        for i in 0..5 {
            buf.push(&format!("line {i}"));
        }
        let result = buf.slice_visual(1, 2, 0);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_slice_visual_offset_beyond_buffer() {
        let mut buf = OutputRingBuffer::new(10);
        buf.push("line1");
        buf.push("line2");
        // offset=100 is way beyond buffer
        let result = buf.slice_visual(100, 5, 40);
        assert!(result.is_empty());
    }

    #[test]
    fn test_slice_visual_no_offset_with_wrapped_lines() {
        // Verify slice_visual(0, ...) matches tail_visual with long lines
        let mut buf = OutputRingBuffer::new(20);
        buf.push("short");
        buf.push(&"X".repeat(80)); // 2 visual rows at width 40
        buf.push("end");

        let tail = buf.tail_visual(4, 40);
        let slice = buf.slice_visual(0, 4, 40);
        assert_eq!(tail.len(), slice.len());
    }

    #[test]
    fn test_slice_visual_with_offset_and_wrapped_lines() {
        let mut buf = OutputRingBuffer::new(20);
        buf.push("a"); // 1 visual row
        buf.push("b"); // 1 visual row
        buf.push(&"C".repeat(80)); // 2 visual rows at width 40
        buf.push("d"); // 1 visual row

        // Total visual rows: 1+1+2+1=5
        // offset=1 → skip "d" (1 visual row from bottom)
        // visual_rows=3 → should fit "C"(2) + "b"(1) = 3
        let result = buf.slice_visual(1, 3, 40);
        assert_eq!(result.len(), 2); // "b" and "CCCC..."
    }

    // ── total_visual_rows tests ────────────────────────────────────────

    #[test]
    fn test_total_visual_rows_empty_buffer() {
        let buf = OutputRingBuffer::new(10);
        assert_eq!(buf.total_visual_rows(40), 0);
    }

    #[test]
    fn test_total_visual_rows_short_lines() {
        let mut buf = OutputRingBuffer::new(10);
        buf.push("short1");
        buf.push("short2");
        buf.push("short3");
        // Each line < 40 chars → 1 visual row each
        assert_eq!(buf.total_visual_rows(40), 3);
    }

    #[test]
    fn test_total_visual_rows_with_wrapping() {
        let mut buf = OutputRingBuffer::new(10);
        buf.push("short"); // 1 visual row
        buf.push(&"X".repeat(80)); // 2 visual rows (80/40 = 2)
        buf.push(&"Y".repeat(120)); // 3 visual rows (120/40 = 3)
        // Total: 1 + 2 + 3 = 6
        assert_eq!(buf.total_visual_rows(40), 6);
    }

    #[test]
    fn test_total_visual_rows_empty_lines() {
        let mut buf = OutputRingBuffer::new(10);
        buf.push("");
        buf.push("text");
        buf.push("");
        // Empty lines count as 1 visual row
        assert_eq!(buf.total_visual_rows(40), 3);
    }

    #[test]
    fn test_total_visual_rows_zero_width_fallback() {
        let mut buf = OutputRingBuffer::new(10);
        buf.push("line1");
        buf.push("line2");
        buf.push("line3");
        // panel_width=0 → fallback to logical count
        assert_eq!(buf.total_visual_rows(0), 3);
    }

    #[test]
    fn test_total_visual_rows_exact_width_boundary() {
        let mut buf = OutputRingBuffer::new(10);
        buf.push(&"A".repeat(40)); // Exactly 40 → 1 visual row
        buf.push(&"B".repeat(41)); // 41 → 2 visual rows (ceil(41/40) = 2)
        buf.push(&"C".repeat(80)); // 80 → 2 visual rows
        // Total: 1 + 2 + 2 = 5
        assert_eq!(buf.total_visual_rows(40), 5);
    }

    #[test]
    fn test_tail_visual_panel_width_one() {
        // Edge case: panel_width=1 → każdy znak to osobny wizualny wiersz
        let mut buf = OutputRingBuffer::new(10);
        buf.push("abc"); // 3 chars → 3 visual rows at width 1
        buf.push("de"); // 2 chars → 2 visual rows at width 1

        let result = buf.tail_visual(4, 1);
        // "de" = 2 visual rows, "abc" = 3 visual rows → total 5
        // Przy budżecie 4, powinno zwrócić tylko "de" (2 rows < 4)
        // Dodanie "abc" dałoby 2+3=5 > 4, więc stop
        assert_eq!(result.len(), 1);

        // Budżet 5 lub więcej → obie linie
        let result = buf.tail_visual(5, 1);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_total_visual_rows_panel_width_one() {
        let mut buf = OutputRingBuffer::new(10);
        buf.push("abc"); // 3 visual rows
        buf.push("de"); // 2 visual rows
        assert_eq!(buf.total_visual_rows(1), 5);
    }

    #[test]
    fn test_slice_visual_panel_width_one() {
        let mut buf = OutputRingBuffer::new(10);
        buf.push("abc"); // 3 visual rows
        buf.push("de"); // 2 visual rows

        // offset=1 → skip 1 visual row from bottom ("d")
        // visual_rows=2 → show 2 visual rows ("e" + first char of "abc")
        // Ale zwracamy całe linie, więc:
        // skip 1 row from bottom → end at visual row 4 (from top)
        // show 2 rows → should show "abc" (rows 2,3,4 from bottom)
        let result = buf.slice_visual(1, 2, 1);
        assert_eq!(result.len(), 1); // tylko "abc"
    }
}
