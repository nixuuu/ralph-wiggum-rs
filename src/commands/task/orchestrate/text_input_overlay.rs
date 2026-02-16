//! Multi-line text input overlay widget for sending messages to workers.
//!
//! Displays a centered modal overlay with a text input field for composing
//! messages to workers. Supports:
//! - Multi-line text input with line wrapping
//! - Enter key for new lines
//! - Ctrl+Enter to send message
//! - Esc to cancel
//! - Cursor positioning and backspace
//! - Vertical scrolling when content exceeds visible area

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// Action returned by handle_key to signal what should happen next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    /// Continue editing (key was handled, no special action).
    Continue,
    /// Send the message (Ctrl+Enter pressed).
    Send(String),
    /// Cancel input (Esc pressed).
    Cancel,
}

/// Convert char index to byte offset in the string.
///
/// If `char_idx` is beyond the string length, returns `s.len()`.
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Convert byte offset to char index (number of chars before that byte).
fn byte_to_char(s: &str, byte_offset: usize) -> usize {
    s[..byte_offset].chars().count()
}

/// Multi-line text input overlay widget.
///
/// Displays a centered modal with title "Message to Worker N",
/// text input field with scrolling, and hint about Ctrl+Enter/Esc.
pub struct TextInputOverlay {
    /// The text content being edited (may contain newlines).
    content: String,
    /// Cursor position as character index (number of chars from start of content).
    cursor_pos: usize,
    /// Vertical scroll offset (number of lines scrolled from top).
    scroll_offset: usize,
    /// Target worker ID for the message.
    target_worker_id: u32,
}

impl TextInputOverlay {
    /// Create a new text input overlay for sending a message to the specified worker.
    pub fn new(worker_id: u32) -> Self {
        Self {
            content: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            target_worker_id: worker_id,
        }
    }

    /// Handle a keyboard event and return the action to take.
    ///
    /// - Ctrl+Enter: Send message (returns InputAction::Send)
    /// - Esc: Cancel (returns InputAction::Cancel)
    /// - Enter: Insert newline
    /// - Backspace: Delete character before cursor
    /// - Char input: Insert character at cursor
    /// - Left/Right/Home/End: Move cursor (TODO: not yet implemented)
    /// - Up/Down: Scroll (TODO: not yet implemented)
    pub fn handle_key(&mut self, key: KeyEvent) -> InputAction {
        // Ctrl+Enter: send message (or cancel if empty)
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
            // Don't send empty messages
            if self.content.trim().is_empty() {
                return InputAction::Cancel;
            }
            return InputAction::Send(self.content.clone());
        }

        match key.code {
            KeyCode::Esc => InputAction::Cancel,
            KeyCode::Enter => {
                // Insert newline at cursor position (convert char index → byte offset)
                let byte_pos = char_to_byte(&self.content, self.cursor_pos);
                self.content.insert(byte_pos, '\n');
                self.cursor_pos += 1;
                InputAction::Continue
            }
            KeyCode::Backspace => {
                // Delete character before cursor (char index → byte offset)
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    let byte_pos = char_to_byte(&self.content, self.cursor_pos);
                    self.content.remove(byte_pos);
                }
                InputAction::Continue
            }
            KeyCode::Char(c) => {
                // Insert character at cursor position (char index → byte offset)
                let byte_pos = char_to_byte(&self.content, self.cursor_pos);
                self.content.insert(byte_pos, c);
                self.cursor_pos += 1;
                InputAction::Continue
            }
            KeyCode::Left => {
                // Move cursor left
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
                InputAction::Continue
            }
            KeyCode::Right => {
                // Move cursor right (char-based)
                let char_count = self.content.chars().count();
                if self.cursor_pos < char_count {
                    self.cursor_pos += 1;
                }
                InputAction::Continue
            }
            KeyCode::Home => {
                // Move cursor to start of current line (char-based)
                let byte_pos = char_to_byte(&self.content, self.cursor_pos);
                let before_cursor = &self.content[..byte_pos];
                if let Some(newline_byte) = before_cursor.rfind('\n') {
                    self.cursor_pos = byte_to_char(&self.content, newline_byte + 1);
                } else {
                    self.cursor_pos = 0;
                }
                InputAction::Continue
            }
            KeyCode::End => {
                // Move cursor to end of current line (char-based)
                let byte_pos = char_to_byte(&self.content, self.cursor_pos);
                let after_cursor = &self.content[byte_pos..];
                if let Some(newline_offset) = after_cursor.find('\n') {
                    self.cursor_pos += byte_to_char(after_cursor, newline_offset);
                } else {
                    self.cursor_pos = self.content.chars().count();
                }
                InputAction::Continue
            }
            KeyCode::Up => {
                // Scroll up (decrease offset)
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                InputAction::Continue
            }
            KeyCode::Down => {
                // Scroll down (increase offset)
                self.scroll_offset = self.scroll_offset.saturating_add(1);
                InputAction::Continue
            }
            _ => InputAction::Continue,
        }
    }

    /// Get the current text content.
    #[allow(dead_code)] // Used in tests; available for future interactive messaging
    pub fn content(&self) -> String {
        self.content.clone()
    }

    /// Get the target worker ID for this overlay.
    pub fn target_worker_id(&self) -> u32 {
        self.target_worker_id
    }

    /// Render the overlay widget onto the given frame at the specified area.
    ///
    /// The overlay is centered within the area and has a fixed size
    /// (60% width, 50% height, minimum 40x10, maximum 80x20).
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        // Compute overlay size (centered, 60% width, 50% height)
        let overlay_width = (area.width * 6 / 10).clamp(40, 80);
        let overlay_height = (area.height / 2).clamp(10, 20);

        // Center the overlay
        let overlay_area = centered_rect(overlay_width, overlay_height, area);

        // Clear background with semi-transparent effect (render a blank block)
        let backdrop = Block::default().style(Style::default().bg(Color::Reset));
        frame.render_widget(backdrop, area);

        // Build the overlay widget
        let overlay_widget = self.build_overlay_widget(overlay_area.width, overlay_area.height);
        frame.render_widget(overlay_widget, overlay_area);
    }

    /// Build the overlay widget (block with title, content, and hint).
    fn build_overlay_widget(&self, width: u16, height: u16) -> Paragraph<'_> {
        // Title: "Message to Worker N"
        let title = format!(" Message to Worker {} ", self.target_worker_id);

        // Hint: "Ctrl+Enter to send, Esc to cancel"
        let hint = " Ctrl+Enter=send | Esc=cancel ";

        // Block with title and borders
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                title,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Span::styled(hint, Style::default().fg(Color::DarkGray)))
            .border_style(Style::default().fg(Color::Cyan));

        // Render content with cursor
        let content_lines = self.render_content_with_cursor(width.saturating_sub(2));

        // Apply scrolling
        let visible_lines: Vec<Line> = content_lines
            .into_iter()
            .skip(self.scroll_offset)
            .take((height.saturating_sub(2)) as usize) // Leave space for borders
            .collect();

        Paragraph::new(visible_lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Left)
    }

    /// Render content with cursor indicator.
    ///
    /// Returns a list of `Line` objects representing the text content
    /// with the cursor rendered as an inverted block on the character under cursor,
    /// or a '|' pipe when cursor is at the end of content.
    ///
    /// Uses char indices throughout to correctly handle multi-byte UTF-8 characters.
    fn render_content_with_cursor(&self, max_width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();
        let _max_width = max_width as usize;

        // If content is empty, show placeholder with cursor
        if self.content.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "|",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )]));
            return lines;
        }

        // Split content into lines and render with cursor.
        // Track position using char indices (not bytes).
        let text_lines: Vec<&str> = self.content.split('\n').collect();
        let mut char_offset = 0;

        for (line_idx, line_text) in text_lines.iter().enumerate() {
            let line_char_count = line_text.chars().count();
            let line_start = char_offset;
            let line_end = char_offset + line_char_count;

            // Check if cursor is on this line
            let cursor_in_line = self.cursor_pos >= line_start && self.cursor_pos <= line_end;

            if cursor_in_line {
                // Cursor offset within this line (in chars)
                let cursor_char_offset = self.cursor_pos - line_start;
                let cursor_byte = char_to_byte(line_text, cursor_char_offset);
                let before = &line_text[..cursor_byte];
                let after = &line_text[cursor_byte..];

                let mut spans = Vec::new();
                if !before.is_empty() {
                    spans.push(Span::raw(before.to_string()));
                }

                // Cursor indicator (inverted block on char under cursor, or '|' at end)
                if after.is_empty() {
                    spans.push(Span::styled(
                        "|",
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else {
                    // Take exactly the first character (may be multi-byte)
                    let first_char = after.chars().next().unwrap();
                    let first_char_len = first_char.len_utf8();
                    spans.push(Span::styled(
                        after[..first_char_len].to_string(),
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ));

                    if after.len() > first_char_len {
                        spans.push(Span::raw(after[first_char_len..].to_string()));
                    }
                }

                lines.push(Line::from(spans));
            } else {
                // Render line without cursor
                lines.push(Line::from(line_text.to_string()));
            }

            // Account for newline character (+1 char) except on last line
            char_offset = line_end
                + if line_idx < text_lines.len() - 1 {
                    1
                } else {
                    0
                };
        }

        // Apply line wrapping if needed (simple character-based wrapping)
        let mut wrapped_lines = Vec::new();
        for line in lines {
            wrapped_lines.extend(wrap_line(line, _max_width));
        }

        wrapped_lines
    }
}

// ── Helper functions ────────────────────────────────────────────────────

/// Create a centered rectangle with the given width and height within the area.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

/// Wrap a line into multiple lines if it exceeds max_width.
///
/// Simple character-based wrapping (does not handle grapheme clusters).
fn wrap_line(line: Line, _max_width: usize) -> Vec<Line> {
    // Simplified: just return the line as-is (wrapping is handled by Paragraph widget)
    // For now, we rely on ratatui's built-in wrapping via Wrap { trim: false }
    vec![line]
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_overlay_empty_content() {
        let overlay = TextInputOverlay::new(3);
        assert_eq!(overlay.content(), "");
        assert_eq!(overlay.cursor_pos, 0);
        assert_eq!(overlay.target_worker_id, 3);
    }

    #[test]
    fn test_handle_char_input() {
        let mut overlay = TextInputOverlay::new(1);
        let action = overlay.handle_key(KeyEvent::from(KeyCode::Char('h')));
        assert_eq!(action, InputAction::Continue);
        assert_eq!(overlay.content(), "h");
        assert_eq!(overlay.cursor_pos, 1);
    }

    #[test]
    fn test_handle_multiple_chars() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('h')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('i')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('!')));
        assert_eq!(overlay.content(), "hi!");
        assert_eq!(overlay.cursor_pos, 3);
    }

    #[test]
    fn test_handle_enter_inserts_newline() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        let action = overlay.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(action, InputAction::Continue);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        assert_eq!(overlay.content(), "a\nb");
        assert_eq!(overlay.cursor_pos, 3);
    }

    #[test]
    fn test_handle_backspace() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('c')));
        assert_eq!(overlay.content(), "abc");

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "ab");
        assert_eq!(overlay.cursor_pos, 2);
    }

    #[test]
    fn test_handle_backspace_at_start_is_noop() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "");
        assert_eq!(overlay.cursor_pos, 0);
    }

    #[test]
    fn test_handle_ctrl_enter_sends_message() {
        let mut overlay = TextInputOverlay::new(2);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('t')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('e')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('s')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('t')));

        let mut key = KeyEvent::from(KeyCode::Enter);
        key.modifiers = KeyModifiers::CONTROL;
        let action = overlay.handle_key(key);

        assert_eq!(action, InputAction::Send("test".to_string()));
    }

    #[test]
    fn test_handle_esc_cancels() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        let action = overlay.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(action, InputAction::Cancel);
    }

    #[test]
    fn test_handle_ctrl_enter_empty_content_cancels() {
        let mut overlay = TextInputOverlay::new(1);
        // Empty content
        assert_eq!(overlay.content(), "");

        let mut key = KeyEvent::from(KeyCode::Enter);
        key.modifiers = KeyModifiers::CONTROL;
        let action = overlay.handle_key(key);

        // Should cancel instead of sending empty message
        assert_eq!(action, InputAction::Cancel);
    }

    #[test]
    fn test_handle_ctrl_enter_whitespace_only_cancels() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "   \n\t  ".to_string();
        overlay.cursor_pos = overlay.content.chars().count();

        let mut key = KeyEvent::from(KeyCode::Enter);
        key.modifiers = KeyModifiers::CONTROL;
        let action = overlay.handle_key(key);

        // Should cancel whitespace-only content
        assert_eq!(action, InputAction::Cancel);
    }

    #[test]
    fn test_cursor_movement_left() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        assert_eq!(overlay.cursor_pos, 2);

        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(overlay.cursor_pos, 1);
    }

    #[test]
    fn test_cursor_movement_right() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(overlay.cursor_pos, 1);

        overlay.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(overlay.cursor_pos, 2);
    }

    #[test]
    fn test_cursor_movement_home() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "hello\nworld".to_string();
        overlay.cursor_pos = 8; // middle of "world"

        overlay.handle_key(KeyEvent::from(KeyCode::Home));
        assert_eq!(overlay.cursor_pos, 6); // start of "world" (after '\n')
    }

    #[test]
    fn test_cursor_movement_end() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "hello\nworld".to_string();
        overlay.cursor_pos = 7; // start of "world"

        overlay.handle_key(KeyEvent::from(KeyCode::End));
        assert_eq!(overlay.cursor_pos, 11); // end of "world"
    }

    #[test]
    fn test_scroll_up() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.scroll_offset = 5;
        overlay.handle_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(overlay.scroll_offset, 4);
    }

    #[test]
    fn test_scroll_down() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.scroll_offset = 0;
        overlay.handle_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(overlay.scroll_offset, 1);
    }

    #[test]
    fn test_centered_rect_basic() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        let rect = centered_rect(40, 20, area);

        assert_eq!(rect.width, 40);
        assert_eq!(rect.height, 20);
        // Centered: x = (100 - 40) / 2 = 30, y = (50 - 20) / 2 = 15
        assert_eq!(rect.x, 30);
        assert_eq!(rect.y, 15);
    }

    #[test]
    fn test_centered_rect_clamps_to_area() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 10,
        };
        let rect = centered_rect(100, 50, area);

        // Should clamp to area size
        assert_eq!(rect.width, 30);
        assert_eq!(rect.height, 10);
    }

    #[test]
    fn test_multi_line_content() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "line1\nline2\nline3".to_string();
        overlay.cursor_pos = 0;

        let lines = overlay.render_content_with_cursor(80);
        // Should have at least 3 lines (may be more if wrapping occurs)
        assert!(lines.len() >= 3);
    }

    #[test]
    fn test_cursor_at_end_of_content() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "test".to_string();
        overlay.cursor_pos = 4; // after 't'

        let lines = overlay.render_content_with_cursor(80);
        assert_eq!(lines.len(), 1);
        // Cursor should be rendered (as '|' at the end)
        let spans = &lines[0].spans;
        assert!(spans.len() > 1); // Should have text + cursor
    }

    #[test]
    fn test_empty_content_shows_cursor() {
        let overlay = TextInputOverlay::new(1);
        let lines = overlay.render_content_with_cursor(80);
        assert_eq!(lines.len(), 1);
        // Should have a cursor indicator
        assert!(!lines[0].spans.is_empty());
    }

    #[test]
    fn test_insert_at_middle() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "ac".to_string();
        overlay.cursor_pos = 1; // between 'a' and 'c'

        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        assert_eq!(overlay.content(), "abc");
        assert_eq!(overlay.cursor_pos, 2);
    }

    #[test]
    fn test_backspace_at_middle() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "abc".to_string();
        overlay.cursor_pos = 2; // after 'b'

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "ac");
        assert_eq!(overlay.cursor_pos, 1);
    }

    #[test]
    fn test_newline_at_middle() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "ac".to_string();
        overlay.cursor_pos = 1; // between 'a' and 'c'

        overlay.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(overlay.content(), "a\nc");
        assert_eq!(overlay.cursor_pos, 2); // after '\n'
    }

    #[test]
    fn test_render_overlay_widget_title() {
        let overlay = TextInputOverlay::new(5);
        let widget = overlay.build_overlay_widget(60, 20);

        // Widget should be created without panicking
        // (We can't easily inspect the widget internals, but we test it doesn't crash)
        drop(widget);
    }

    // ── Unicode / multi-byte character tests ──────────────────────

    #[test]
    fn test_char_to_byte_ascii() {
        assert_eq!(char_to_byte("hello", 0), 0);
        assert_eq!(char_to_byte("hello", 3), 3);
        assert_eq!(char_to_byte("hello", 5), 5); // past end → len
    }

    #[test]
    fn test_char_to_byte_unicode() {
        // 'ą' is 2 bytes in UTF-8
        let s = "ąę";
        assert_eq!(char_to_byte(s, 0), 0); // start of 'ą'
        assert_eq!(char_to_byte(s, 1), 2); // start of 'ę'
        assert_eq!(char_to_byte(s, 2), 4); // past end
    }

    #[test]
    fn test_byte_to_char_unicode() {
        let s = "ąę";
        assert_eq!(byte_to_char(s, 0), 0);
        assert_eq!(byte_to_char(s, 2), 1);
        assert_eq!(byte_to_char(s, 4), 2);
    }

    #[test]
    fn test_unicode_char_input() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        assert_eq!(overlay.content(), "ą");
        assert_eq!(overlay.cursor_pos, 1); // char index, not byte offset
    }

    #[test]
    fn test_unicode_multiple_chars() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ś')));
        assert_eq!(overlay.content(), "ąęś");
        assert_eq!(overlay.cursor_pos, 3);
    }

    #[test]
    fn test_unicode_backspace() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ś')));
        assert_eq!(overlay.content(), "ąęś");

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "ąę");
        assert_eq!(overlay.cursor_pos, 2);
    }

    #[test]
    fn test_unicode_left_right_navigation() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ć')));
        assert_eq!(overlay.cursor_pos, 3);

        // Left twice
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(overlay.cursor_pos, 2);
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(overlay.cursor_pos, 1);

        // Insert at position 1 (between 'ą' and 'b')
        overlay.handle_key(KeyEvent::from(KeyCode::Char('x')));
        assert_eq!(overlay.content(), "ąxbć");
        assert_eq!(overlay.cursor_pos, 2);

        // Right back to end
        overlay.handle_key(KeyEvent::from(KeyCode::Right));
        overlay.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(overlay.cursor_pos, 4);
    }

    #[test]
    fn test_unicode_home_end() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "ąę\nść".to_string();
        overlay.cursor_pos = 4; // middle of second line ('ć')

        overlay.handle_key(KeyEvent::from(KeyCode::Home));
        assert_eq!(overlay.cursor_pos, 3); // start of "ść" line

        overlay.handle_key(KeyEvent::from(KeyCode::End));
        assert_eq!(overlay.cursor_pos, 5); // end of "ść" line
    }

    #[test]
    fn test_unicode_render_no_panic() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "ąęść".to_string();
        overlay.cursor_pos = 2; // on 'ś'

        // Should not panic when rendering cursor on multi-byte char
        let lines = overlay.render_content_with_cursor(80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_unicode_mixed_ascii_and_polish() {
        let mut overlay = TextInputOverlay::new(1);
        // Type: "aąb"
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        assert_eq!(overlay.content(), "aąb");
        assert_eq!(overlay.cursor_pos, 3);

        // Backspace removes 'b'
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "aą");
        assert_eq!(overlay.cursor_pos, 2);

        // Backspace removes 'ą'
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "a");
        assert_eq!(overlay.cursor_pos, 1);
    }

    #[test]
    fn test_unicode_insert_in_middle() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "ąć".to_string();
        overlay.cursor_pos = 1; // between 'ą' and 'ć'

        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        assert_eq!(overlay.content(), "ąęć");
        assert_eq!(overlay.cursor_pos, 2);
    }

    #[test]
    fn test_unicode_backspace_in_middle() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "ąęć".to_string();
        overlay.cursor_pos = 2; // after 'ę', before 'ć'

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "ąć");
        assert_eq!(overlay.cursor_pos, 1);
    }

    #[test]
    fn test_scroll_offset_does_not_go_negative() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.scroll_offset = 0;

        overlay.handle_key(KeyEvent::from(KeyCode::Up));
        overlay.handle_key(KeyEvent::from(KeyCode::Up));
        overlay.handle_key(KeyEvent::from(KeyCode::Up));

        // Should saturate at 0
        assert_eq!(overlay.scroll_offset, 0);
    }

    #[test]
    fn test_multiple_newlines() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Enter));
        overlay.handle_key(KeyEvent::from(KeyCode::Enter));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));

        assert_eq!(overlay.content(), "a\n\nb");
        assert_eq!(overlay.cursor_pos, 4);
    }

    #[test]
    fn test_backspace_across_newline() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "a\nb".to_string();
        overlay.cursor_pos = 2; // after '\n', before 'b'

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));

        assert_eq!(overlay.content(), "ab");
        assert_eq!(overlay.cursor_pos, 1);
    }

    #[test]
    fn test_render_with_scrolling() {
        let mut overlay = TextInputOverlay::new(1);
        // Create content with many lines
        for i in 0..20 {
            overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
            if i < 19 {
                overlay.handle_key(KeyEvent::from(KeyCode::Enter));
            }
        }

        overlay.scroll_offset = 5;
        let lines = overlay.render_content_with_cursor(80);

        // Should have at least 20 lines (one per iteration)
        assert!(lines.len() >= 20);
    }

    #[test]
    fn test_centered_rect_offset() {
        let area = Rect {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        };
        let rect = centered_rect(40, 20, area);

        // Should be centered relative to area's position
        assert_eq!(rect.x, 10 + (100 - 40) / 2);
        assert_eq!(rect.y, 20 + (50 - 20) / 2);
    }

    // ── Snapshot testy dla TextInputOverlay modal (zadanie 64.3) ──

    use crate::test_helpers::{render_widget_to_buffer, snap};

    /// Wrapper Widget dla TextInputOverlay — renderuje build_overlay_widget w pełnym area.
    ///
    /// Testuje treść i layout widgetu (tytuł, border, tekst, hint) bez centrowania.
    /// Centrowanie i clamping rozmiarów są testowane osobno w testach centered_rect.
    struct TextInputOverlayWidget {
        overlay: TextInputOverlay,
    }

    impl ratatui::widgets::Widget for TextInputOverlayWidget {
        fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
            let overlay_widget = self.overlay.build_overlay_widget(area.width, area.height);
            overlay_widget.render(area, buf);
        }
    }

    #[test]
    fn test_snapshot_empty_overlay() {
        // Pusty overlay z hint text — worker ID 1
        let overlay = TextInputOverlay::new(1);
        let widget = TextInputOverlayWidget { overlay };

        // Renderujemy w obszarze 60x10 (typowy rozmiar overlay)
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 1 ─────────────────────────────────────┐
        │|                                                         │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_single_line_text() {
        // Overlay z jedną linią tekstu
        let mut overlay = TextInputOverlay::new(2);
        overlay.content = "Hello Worker!".to_string();
        overlay.cursor_pos = 13; // na końcu tekstu

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 2 ─────────────────────────────────────┐
        │Hello Worker!|                                            │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_multiline_text() {
        // Overlay z wieloma liniami tekstu
        let mut overlay = TextInputOverlay::new(3);
        overlay.content = "Line one\nLine two\nLine three".to_string();
        overlay.cursor_pos = overlay.content.chars().count(); // na końcu ostatniej linii

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 3 ─────────────────────────────────────┐
        │Line one                                                  │
        │Line two                                                  │
        │Line three|                                               │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_cursor_at_start() {
        // Kursor na początku tekstu
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "test message".to_string();
        overlay.cursor_pos = 0; // na początku

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 1 ─────────────────────────────────────┐
        │test message                                              │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_cursor_in_middle() {
        // Kursor w środku tekstu
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "test message".to_string();
        overlay.cursor_pos = 5; // między "test" a "message"

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 1 ─────────────────────────────────────┐
        │test message                                              │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_cursor_multiline_positions() {
        // Kursor na różnych pozycjach w wieloliniowym tekście
        let mut overlay = TextInputOverlay::new(2);
        overlay.content = "abc\ndef\nghi".to_string();
        overlay.cursor_pos = 4; // początek drugiej linii (po '\n')

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 2 ─────────────────────────────────────┐
        │abc                                                       │
        │def                                                       │
        │ghi                                                       │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_with_scrolling() {
        // Overlay z długim tekstem i scrolling offsetem
        let mut overlay = TextInputOverlay::new(5);
        // Tworzę 15 linii tekstu
        let lines: Vec<String> = (1..=15).map(|i| format!("Line {}", i)).collect();
        overlay.content = lines.join("\n");
        overlay.cursor_pos = overlay.content.chars().count();
        overlay.scroll_offset = 5; // przewinięcie o 5 linii

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        // Po scrolling offset=5 widzimy linie 6-13 (8 linii w viewport 10-2=8)
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 5 ─────────────────────────────────────┐
        │Line 6                                                    │
        │Line 7                                                    │
        │Line 8                                                    │
        │Line 9                                                    │
        │Line 10                                                   │
        │Line 11                                                   │
        │Line 12                                                   │
        │Line 13                                                   │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_narrow_terminal_40x10() {
        // Wąski area 40x10 — weryfikuje layout przy minimalnym rozmiarze
        let mut overlay = TextInputOverlay::new(7);
        overlay.content = "Short text".to_string();
        overlay.cursor_pos = 10;

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 40, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 7 ─────────────────┐
        │Short text|                           │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        └ Ctrl+Enter=send | Esc=cancel ────────┘
        ");
    }

    #[test]
    fn test_snapshot_medium_terminal_80x15() {
        // Średni area 80x15 — weryfikuje layout przy standardowym rozmiarze
        let mut overlay = TextInputOverlay::new(10);
        overlay.content = "Medium terminal test".to_string();
        overlay.cursor_pos = 20;

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 80, 15);
        // Overlay będzie 80*6/10=48 szerokości (clamped to 80), wysokość 15/2=7 (clamped to 20)
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 10 ────────────────────────────────────────────────────────┐
        │Medium terminal test|                                                         │
        │                                                                              │
        │                                                                              │
        │                                                                              │
        │                                                                              │
        │                                                                              │
        │                                                                              │
        │                                                                              │
        │                                                                              │
        │                                                                              │
        │                                                                              │
        │                                                                              │
        │                                                                              │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_wide_terminal_120x20() {
        // Szerokie area 120x20 — weryfikuje layout przy dużym rozmiarze
        let mut overlay = TextInputOverlay::new(15);
        overlay.content = "Wide terminal test message".to_string();
        overlay.cursor_pos = 26;

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 120, 20);
        // Overlay: width = 120*6/10=72, clamped max 80 → 72
        // height = 20/2=10, clamped max 20 → 10
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 15 ────────────────────────────────────────────────────────────────────────────────────────────────┐
        │Wide terminal test message|                                                                                           │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────────────────────────────────────────────────────────────────┘
        ");
    }

    #[test]
    fn test_unicode_ctrl_enter_send() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ś')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ć')));

        let mut key = KeyEvent::from(KeyCode::Enter);
        key.modifiers = KeyModifiers::CONTROL;

        let action = overlay.handle_key(key);

        assert_eq!(action, InputAction::Send("ąęść".to_string()));
    }

    // ── Snapshot testy dla Unicode input — zadanie 70.5 ──

    #[test]
    fn test_snapshot_unicode_input_aes() {
        // Test 1: wpisanie 'ąęś' — cursor_pos==3, content length==6 bytes
        let mut overlay = TextInputOverlay::new(1);
        overlay.content = "ąęś".to_string();
        overlay.cursor_pos = 3; // 3 znaki (każdy 2 bajty)

        // Weryfikacja założeń: cursor_pos to char count, content.len() to bajty
        assert_eq!(overlay.cursor_pos, 3); // 3 znaki
        assert_eq!(overlay.content.len(), 6); // 6 bajtów (ą=2B, ę=2B, ś=2B)
        assert_eq!(overlay.content.chars().count(), 3); // potwierdzenie char count

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 1 ─────────────────────────────────────┐
        │ąęś|                                                      │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_unicode_backspace_removal() {
        // Test 2: backspace po polskim znaku — symulacja pełnego flow
        // Wpisujemy 'ąęś', potem backspace → powinno być 'ąę'
        let mut overlay = TextInputOverlay::new(2);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ś')));
        assert_eq!(overlay.cursor_pos, 3);

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.cursor_pos, 2);
        assert_eq!(overlay.content.len(), 4); // 4 bajty (ą=2B, ę=2B)
        assert_eq!(overlay.content.chars().count(), 2);

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 2 ─────────────────────────────────────┐
        │ąę|                                                       │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_unicode_cursor_left_middle() {
        // Test 3a: cursor left przez polskie znaki — symulacja nawigacji
        let mut overlay = TextInputOverlay::new(3);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ś')));
        assert_eq!(overlay.cursor_pos, 3);

        // Left dwukrotnie: 3 → 2 → 1 (kursor na 'ę')
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(overlay.cursor_pos, 1);

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 3 ─────────────────────────────────────┐
        │ąęś                                                       │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_unicode_cursor_right_middle() {
        // Test 3b: cursor right przez polskie znaki — symulacja nawigacji
        let mut overlay = TextInputOverlay::new(4);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ś')));

        // Left do początku: 3 → 0
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(overlay.cursor_pos, 0);

        // Right dwukrotnie: 0 → 1 → 2 (kursor na 'ś')
        overlay.handle_key(KeyEvent::from(KeyCode::Right));
        overlay.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(overlay.cursor_pos, 2);

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 4 ─────────────────────────────────────┐
        │ąęś                                                       │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_unicode_home_key() {
        // Test 4a: Home z unicode content — wieloliniowy tekst
        // Kursor w środku drugiej linii "śćź", Home przenosi na początek linii
        let mut overlay = TextInputOverlay::new(5);
        overlay.content = "ąę\nśćź".to_string();
        overlay.cursor_pos = 5; // na 'ź' (środek drugiej linii)

        overlay.handle_key(KeyEvent::from(KeyCode::Home));
        assert_eq!(overlay.cursor_pos, 3); // początek "śćź"

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 5 ─────────────────────────────────────┐
        │ąę                                                        │
        │śćź                                                       │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_unicode_end_key() {
        // Test 4b: End z unicode content — wieloliniowy tekst
        // Kursor na początku drugiej linii "śćź", End przenosi na koniec
        let mut overlay = TextInputOverlay::new(6);
        overlay.content = "ąę\nśćź".to_string();
        overlay.cursor_pos = 3; // początek drugiej linii (na 'ś')

        overlay.handle_key(KeyEvent::from(KeyCode::End));
        assert_eq!(overlay.cursor_pos, 6); // koniec "śćź"

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 6 ─────────────────────────────────────┐
        │ąę                                                        │
        │śćź|                                                      │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_unicode_mixed_ascii_polish() {
        // Test mieszanych znaków ASCII i polskich — kursor w środku
        let mut overlay = TextInputOverlay::new(7);
        for c in "abc ąęś xyz".chars() {
            overlay.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(overlay.content(), "abc ąęś xyz");
        assert_eq!(overlay.cursor_pos, 11); // na końcu

        // Left 4x: kursor na ' ' przed "xyz" (pos=7)
        for _ in 0..4 {
            overlay.handle_key(KeyEvent::from(KeyCode::Left));
        }
        assert_eq!(overlay.cursor_pos, 7);

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 7 ─────────────────────────────────────┐
        │abc ąęś xyz                                               │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_unicode_cursor_at_start() {
        // Test: kursor na pierwszym polskim znaku (pos=0, podświetla 'ą')
        let mut overlay = TextInputOverlay::new(8);
        for c in "ąęś".chars() {
            overlay.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }

        // Home przenosi na początek
        overlay.handle_key(KeyEvent::from(KeyCode::Home));
        assert_eq!(overlay.cursor_pos, 0);

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        ┌ Message to Worker 8 ─────────────────────────────────────┐
        │ąęś                                                       │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        │                                                          │
        └ Ctrl+Enter=send | Esc=cancel ────────────────────────────┘
        ");
    }

    // ── Snapshot testy renderowania pełnego overlay modal z centrowaniem (zadanie 72.1) ──

    /// Pomocnicza funkcja do renderowania overlay z pełnym centrowaniem.
    ///
    /// Używa TestBackend i Terminal::draw() aby wywołać pełną metodę render()
    /// która zawiera logikę centrowania overlaya w area.
    fn render_overlay_full(
        overlay: TextInputOverlay,
        width: u16,
        height: u16,
    ) -> ratatui::buffer::Buffer {
        use ratatui::backend::TestBackend;
        use ratatui::prelude::Terminal;

        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("Failed to create terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, height);
                overlay.render(frame, area);
            })
            .expect("Failed to draw");
        terminal.backend().buffer().clone()
    }

    #[test]
    fn test_snapshot_full_render_empty_80x24() {
        // Test 1: Pusty overlay modal — centrowanie na terminalu 80x24
        let overlay = TextInputOverlay::new(1);
        let buffer = render_overlay_full(overlay, 80, 24);
        insta::assert_snapshot!(snap(&buffer), @"






        ┌ Message to Worker 1 ─────────────────────────┐
        │|                                             │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        └ Ctrl+Enter=send | Esc=cancel ────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_full_render_single_line_80x24() {
        // Test 2: Overlay z jedną linią tekstu i kursorem — centrowanie 80x24
        let mut overlay = TextInputOverlay::new(2);
        overlay.content = "Hello from worker!".to_string();
        overlay.cursor_pos = 18; // kursor na końcu

        let buffer = render_overlay_full(overlay, 80, 24);
        insta::assert_snapshot!(snap(&buffer), @"






        ┌ Message to Worker 2 ─────────────────────────┐
        │Hello from worker!|                           │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        └ Ctrl+Enter=send | Esc=cancel ────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_full_render_five_lines_80x24() {
        // Test 3: Overlay z 5 liniami tekstu (multi-line) — centrowanie 80x24
        let mut overlay = TextInputOverlay::new(3);
        let lines = [
            "First line",
            "Second line",
            "Third line",
            "Fourth line",
            "Fifth line",
        ];
        overlay.content = lines.join("\n");
        overlay.cursor_pos = overlay.content.chars().count(); // kursor na końcu

        let buffer = render_overlay_full(overlay, 80, 24);
        insta::assert_snapshot!(snap(&buffer), @"






        ┌ Message to Worker 3 ─────────────────────────┐
        │First line                                    │
        │Second line                                   │
        │Third line                                    │
        │Fourth line                                   │
        │Fifth line|                                   │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        │                                              │
        └ Ctrl+Enter=send | Esc=cancel ────────────────┘
        ");
    }

    #[test]
    fn test_snapshot_full_render_empty_40x15() {
        // Test 5: Pusty overlay modal — centrowanie na małym terminalu 40x15
        let overlay = TextInputOverlay::new(5);
        let buffer = render_overlay_full(overlay, 40, 15);
        insta::assert_snapshot!(snap(&buffer), @"


        ┌ Message to Worker 5 ─────────────────┐
        │|                                     │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        └ Ctrl+Enter=send | Esc=cancel ────────┘
        ");
    }

    #[test]
    fn test_snapshot_full_render_text_40x15() {
        // Test 6: Overlay z tekstem — centrowanie na małym terminalu 40x15
        let mut overlay = TextInputOverlay::new(6);
        overlay.content = "Short text\nAnother line".to_string();
        overlay.cursor_pos = overlay.content.chars().count();

        let buffer = render_overlay_full(overlay, 40, 15);
        insta::assert_snapshot!(snap(&buffer), @"


        ┌ Message to Worker 6 ─────────────────┐
        │Short text                            │
        │Another line|                         │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        └ Ctrl+Enter=send | Esc=cancel ────────┘
        ");
    }

    #[test]
    fn test_snapshot_full_render_hint_display() {
        // Test 7: Weryfikacja wyświetlania hint text w różnych konfiguracjach
        // (hint jest zawsze widoczny jako bottom title)
        let mut overlay = TextInputOverlay::new(10);
        overlay.content = "Testing hints".to_string();
        overlay.cursor_pos = overlay.content.chars().count();

        let buffer = render_overlay_full(overlay, 60, 12);

        // Weryfikujemy że hint jest na dole overlay
        let snapshot = snap(&buffer);
        assert!(snapshot.contains("Ctrl+Enter=send | Esc=cancel"));

        insta::assert_snapshot!(snapshot, @"

        ┌ Message to Worker 10 ────────────────┐
        │Testing hints|                        │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        │                                      │
        └ Ctrl+Enter=send | Esc=cancel ────────┘
        ");
    }

    #[test]
    fn test_snapshot_full_render_border_display() {
        // Test 8: Weryfikacja border rendering i tytułu
        let overlay = TextInputOverlay::new(42);
        let buffer = render_overlay_full(overlay, 70, 14);
        let snapshot = snap(&buffer);

        // Weryfikujemy że tytuł zawiera worker ID
        assert!(snapshot.contains("Message to Worker 42"));
        // Weryfikujemy że border jest obecny (┌ ┐ └ ┘ │ ─)
        assert!(snapshot.contains("┌"));
        assert!(snapshot.contains("┐"));
        assert!(snapshot.contains("└"));
        assert!(snapshot.contains("┘"));

        insta::assert_snapshot!(snapshot, @"


        ┌ Message to Worker 42 ──────────────────┐
        │|                                       │
        │                                        │
        │                                        │
        │                                        │
        │                                        │
        │                                        │
        │                                        │
        │                                        │
        └ Ctrl+Enter=send | Esc=cancel ──────────┘
        ");
    }

    // ── Testy backspace na granicy unicode char (zadanie 74.1) ──

    #[test]
    fn test_backspace_after_multibyte_unicode_char() {
        // Test 1: wpisz 'aąb', cursor_pos=3, backspace → 'aą', cursor_pos=2
        let mut overlay = TextInputOverlay::new(1);

        // Wpisz 'a', 'ą', 'b'
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));

        // Sprawdź stan przed backspace
        assert_eq!(overlay.content(), "aąb");
        assert_eq!(overlay.cursor_pos, 3); // 3 znaki
        assert_eq!(overlay.content.len(), 4); // 4 bajty: 'a'=1B, 'ą'=2B, 'b'=1B

        // Backspace usuwa 'b'
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));

        // Weryfikacja
        assert_eq!(overlay.content(), "aą");
        assert_eq!(overlay.cursor_pos, 2);
        assert_eq!(overlay.content.len(), 3); // 3 bajty: 'a'=1B, 'ą'=2B

        // Drugi backspace usuwa 'ą' (wielobajtowy znak)
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));

        // Weryfikacja
        assert_eq!(overlay.content(), "a");
        assert_eq!(overlay.cursor_pos, 1);
        assert_eq!(overlay.content.len(), 1); // 1 bajt
    }

    #[test]
    fn test_backspace_after_multibyte_unicode_from_middle() {
        // Test 2: wpisz 'aąb', left, backspace → 'ab', cursor_pos=1
        // Testuje usuwanie wielobajtowego znaku 'ą' ze środka stringa
        let mut overlay = TextInputOverlay::new(1);

        // Wpisz 'a', 'ą', 'b'
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));

        assert_eq!(overlay.content(), "aąb");
        assert_eq!(overlay.cursor_pos, 3);
        assert_eq!(overlay.content.len(), 4); // a=1B, ą=2B, b=1B

        // Left — kursor na pozycji 2 (po 'ą', przed 'b')
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(overlay.cursor_pos, 2);

        // Backspace usuwa 'ą' (wielobajtowy znak ze środka)
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));

        // Weryfikacja: 'ą' usunięte, pozostaje 'ab'
        assert_eq!(overlay.content(), "ab");
        assert_eq!(overlay.cursor_pos, 1);
        assert_eq!(overlay.content.len(), 2); // 'a'=1B, 'b'=1B
    }

    #[test]
    fn test_backspace_emoji_multibyte() {
        // Test 3: wpisz emoji '🎉', backspace → '', cursor_pos=0
        let mut overlay = TextInputOverlay::new(1);

        // Emoji '🎉' to 4 bajty w UTF-8
        overlay.handle_key(KeyEvent::from(KeyCode::Char('🎉')));

        assert_eq!(overlay.content(), "🎉");
        assert_eq!(overlay.cursor_pos, 1); // 1 znak
        assert_eq!(overlay.content.len(), 4); // 4 bajty

        // Backspace usuwa cały emoji
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));

        // Weryfikacja
        assert_eq!(overlay.content(), "");
        assert_eq!(overlay.cursor_pos, 0);
        assert_eq!(overlay.content.len(), 0);
    }

    #[test]
    fn test_insert_unicode_in_middle_of_ascii() {
        // Test 4: wpisz 'abc', left, wpisz 'ą', sprawdź content='abąc', cursor_pos=3
        let mut overlay = TextInputOverlay::new(1);

        // Wpisz 'abc'
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('c')));

        assert_eq!(overlay.content(), "abc");
        assert_eq!(overlay.cursor_pos, 3);

        // Left — kursor na pozycji 2 (po 'b', przed 'c')
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(overlay.cursor_pos, 2);

        // Wpisz 'ą'
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));

        // Weryfikacja
        assert_eq!(overlay.content(), "abąc");
        assert_eq!(overlay.cursor_pos, 3); // 3 znaki przed kursorem
        assert_eq!(overlay.content.len(), 5); // 'a'=1B, 'b'=1B, 'ą'=2B, 'c'=1B
    }

    #[test]
    fn test_backspace_mixed_unicode_sequence() {
        // Test kompleksowy: mieszanka ASCII i unicode, backspace w różnych miejscach
        let mut overlay = TextInputOverlay::new(1);

        // Wpisz 'aąbęc'
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('c')));

        assert_eq!(overlay.content(), "aąbęc");
        assert_eq!(overlay.cursor_pos, 5);
        assert_eq!(overlay.content.len(), 7); // a=1B, ą=2B, b=1B, ę=2B, c=1B

        // Backspace 1: usuń 'c'
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "aąbę");
        assert_eq!(overlay.cursor_pos, 4);
        assert_eq!(overlay.content.len(), 6);

        // Backspace 2: usuń 'ę' (wielobajtowy)
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "aąb");
        assert_eq!(overlay.cursor_pos, 3);
        assert_eq!(overlay.content.len(), 4);

        // Backspace 3: usuń 'b'
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "aą");
        assert_eq!(overlay.cursor_pos, 2);
        assert_eq!(overlay.content.len(), 3);

        // Backspace 4: usuń 'ą' (wielobajtowy)
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "a");
        assert_eq!(overlay.cursor_pos, 1);
        assert_eq!(overlay.content.len(), 1);

        // Backspace 5: usuń 'a'
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "");
        assert_eq!(overlay.cursor_pos, 0);
        assert_eq!(overlay.content.len(), 0);
    }

    #[test]
    fn test_backspace_unicode_at_string_boundaries() {
        // Test backspace na wielobajtowych znakach na początku i końcu stringa
        let mut overlay = TextInputOverlay::new(1);

        // Wpisz 'ąb'
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));

        assert_eq!(overlay.content(), "ąb");
        assert_eq!(overlay.cursor_pos, 2);

        // Left — kursor na pozycji 1 (po 'ą', przed 'b')
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(overlay.cursor_pos, 1);

        // Backspace usuwa 'ą' (wielobajtowy na początku)
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "b");
        assert_eq!(overlay.cursor_pos, 0);
        assert_eq!(overlay.content.len(), 1);
    }

    #[test]
    fn test_multiple_emoji_backspace() {
        // Test backspace z wieloma emoji
        let mut overlay = TextInputOverlay::new(1);

        // Wpisz '🎉🚀🌟'
        overlay.handle_key(KeyEvent::from(KeyCode::Char('🎉')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('🚀')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('🌟')));

        assert_eq!(overlay.content(), "🎉🚀🌟");
        assert_eq!(overlay.cursor_pos, 3); // 3 znaki
        // Każde emoji to 4 bajty
        assert_eq!(overlay.content.len(), 12);

        // Backspace usuwa '🌟'
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "🎉🚀");
        assert_eq!(overlay.cursor_pos, 2);
        assert_eq!(overlay.content.len(), 8);

        // Backspace usuwa '🚀'
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "🎉");
        assert_eq!(overlay.cursor_pos, 1);
        assert_eq!(overlay.content.len(), 4);
    }
}
