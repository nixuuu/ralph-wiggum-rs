/// Widget dla inline text input w ask_user container.
///
/// Renderuje pytanie (markdown) + pole input z kursorem.
/// Obsługuje single-line input: wpisywanie, backspace, strzałki, Enter=submit, Esc=cancel.
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::Widget,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::shared::markdown::render_markdown;
use crate::tui::Theme;

/// Stan text input widget
#[derive(Debug, Clone)]
pub struct TextInputState {
    /// Buffer tekstowy (single line)
    pub buffer: String,
    /// Pozycja kursora jako char index (liczba znaków od początku)
    pub cursor_pos: usize,
    /// Placeholder text wyświetlany gdy buffer jest pusty
    pub placeholder: Option<String>,
}

impl TextInputState {
    /// Tworzy nowy stan z opcjonalnym placeholder
    pub fn new(placeholder: Option<String>) -> Self {
        Self {
            buffer: String::new(),
            cursor_pos: 0,
            placeholder,
        }
    }

    /// Wstawia znak na pozycji kursora
    pub fn insert_char(&mut self, c: char) {
        let byte_pos = char_to_byte(&self.buffer, self.cursor_pos);
        self.buffer.insert(byte_pos, c);
        self.cursor_pos += 1;
    }

    /// Usuwa znak przed kursorem (backspace)
    pub fn delete_char(&mut self) {
        if self.cursor_pos > 0 {
            let byte_pos = char_to_byte(&self.buffer, self.cursor_pos - 1);
            self.buffer.remove(byte_pos);
            self.cursor_pos -= 1;
        }
    }

    /// Przesuwa kursor w lewo
    pub fn move_cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    /// Przesuwa kursor w prawo
    pub fn move_cursor_right(&mut self) {
        let max_pos = self.buffer.chars().count();
        if self.cursor_pos < max_pos {
            self.cursor_pos += 1;
        }
    }

    /// Przesuwa kursor na początek
    pub fn move_cursor_home(&mut self) {
        self.cursor_pos = 0;
    }

    /// Przesuwa kursor na koniec
    pub fn move_cursor_end(&mut self) {
        self.cursor_pos = self.buffer.chars().count();
    }

    /// Czyści buffer
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor_pos = 0;
    }

    /// Zwraca aktualną zawartość buffera
    pub fn value(&self) -> &str {
        &self.buffer
    }
}

/// Widget text input — renderuje pytanie (markdown) + pole input z kursorem
#[allow(dead_code)] // TUI component — will be used when full TUI is integrated
pub struct TextInputWidget<'a> {
    /// Tekst pytania (markdown)
    question: &'a str,
    /// Stan inputu
    state: &'a TextInputState,
    /// Theme dla kolorów
    theme: &'a Theme,
    /// Czy widget jest aktywny (focused)
    focused: bool,
}

#[allow(dead_code)] // TUI component methods — will be used when widget is integrated
impl<'a> TextInputWidget<'a> {
    /// Tworzy nowy widget
    pub fn new(
        question: &'a str,
        state: &'a TextInputState,
        theme: &'a Theme,
        focused: bool,
    ) -> Self {
        Self {
            question,
            state,
            theme,
            focused,
        }
    }

    /// Renderuje pytanie jako markdown text
    fn render_question(&self, max_lines: usize) -> Vec<Line<'a>> {
        let rendered = render_markdown(self.question);
        rendered
            .lines()
            .take(max_lines)
            .map(|line| Line::from(line.to_string()))
            .collect()
    }

    /// Renderuje linię inputu z kursorem
    fn render_input_line(&self, width: u16) -> Line<'a> {
        let mut spans = Vec::new();

        // Prompt prefix
        let prompt = "> ";
        spans.push(Span::styled(
            prompt,
            if self.focused {
                self.theme.header_style()
            } else {
                self.theme.muted_style()
            },
        ));

        let available_width = (width as usize).saturating_sub(prompt.width());

        if self.state.buffer.is_empty() {
            // Pokaż placeholder lub kursor gdy buffer pusty
            if self.focused {
                spans.push(Span::styled("█", self.theme.primary_style()));
                if let Some(placeholder) = &self.state.placeholder {
                    // Placeholder za kursorem (pomijamy pierwszy znak)
                    let rest: String = placeholder.chars().skip(1).collect();
                    if !rest.is_empty() {
                        spans.push(Span::styled(
                            rest,
                            self.theme.muted_style().add_modifier(Modifier::ITALIC),
                        ));
                    }
                }
            } else if let Some(placeholder) = &self.state.placeholder {
                spans.push(Span::styled(
                    placeholder.clone(),
                    self.theme.muted_style().add_modifier(Modifier::ITALIC),
                ));
            }
        } else {
            // Renderuj buffer z kursorem
            let chars: Vec<char> = self.state.buffer.chars().collect();
            let cursor_pos = self.state.cursor_pos;

            // Oblicz scroll offset jeśli tekst nie mieści się w width
            // Rezerwuj 1 kolumnę na kursor na końcu
            let scroll_width = if cursor_pos == chars.len() {
                available_width.saturating_sub(1)
            } else {
                available_width
            };
            let mut visible_start = 0;
            let mut current_width = 0;

            // Znajdź początek widocznego fragmentu tak, żeby kursor był widoczny
            if cursor_pos > 0 {
                for i in (0..cursor_pos).rev() {
                    let char_width = chars[i].width().unwrap_or(1);
                    if current_width + char_width > scroll_width {
                        visible_start = i + 1;
                        break;
                    }
                    current_width += char_width;
                }
            }

            // Renderuj widoczną część tekstu
            let cursor_style = self.theme.primary_style().add_modifier(Modifier::REVERSED);
            let mut rendered_width = 0;
            for (i, &ch) in chars.iter().enumerate().skip(visible_start) {
                let char_width = ch.width().unwrap_or(1);
                if rendered_width + char_width > available_width {
                    break;
                }

                if i == cursor_pos && self.focused {
                    // Kursor na znaku — renderuj znak z odwróconym stylem
                    spans.push(Span::styled(ch.to_string(), cursor_style));
                } else {
                    spans.push(Span::raw(ch.to_string()));
                }
                rendered_width += char_width;
            }

            // Kursor na końcu — block cursor za ostatnim znakiem
            if cursor_pos == chars.len() && self.focused {
                spans.push(Span::styled("█", self.theme.primary_style()));
            }
        }

        Line::from(spans)
    }
}

impl<'a> Widget for TextInputWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Renderuj pytanie (max_lines = wysokość - separator - input line)
        let max_question_lines = area.height.saturating_sub(2) as usize;
        let question_lines = self.render_question(max_question_lines);
        let question_height = question_lines.len();

        for (i, line) in question_lines.iter().take(question_height).enumerate() {
            let y = area.y + i as u16;
            if y < area.y + area.height {
                buf.set_line(area.x, y, line, area.width);
            }
        }

        // Pusta linia separator
        let separator_y = area.y + question_height as u16;
        if separator_y < area.y + area.height {
            buf.set_line(area.x, separator_y, &Line::from(""), area.width);
        }

        // Renderuj input line
        let input_y = separator_y + 1;
        if input_y < area.y + area.height {
            let input_line = self.render_input_line(area.width);
            buf.set_line(area.x, input_y, &input_line, area.width);
        }
    }
}

/// Konwertuje char index na byte offset w stringu
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::Theme;

    #[test]
    fn test_text_input_state_new() {
        let state = TextInputState::new(Some("Enter text...".to_string()));
        assert_eq!(state.buffer, "");
        assert_eq!(state.cursor_pos, 0);
        assert_eq!(state.placeholder, Some("Enter text...".to_string()));
    }

    #[test]
    fn test_insert_char() {
        let mut state = TextInputState::new(None);
        state.insert_char('H');
        state.insert_char('i');
        assert_eq!(state.buffer, "Hi");
        assert_eq!(state.cursor_pos, 2);
    }

    #[test]
    fn test_delete_char() {
        let mut state = TextInputState::new(None);
        state.insert_char('H');
        state.insert_char('i');
        state.delete_char();
        assert_eq!(state.buffer, "H");
        assert_eq!(state.cursor_pos, 1);
    }

    #[test]
    fn test_delete_char_empty() {
        let mut state = TextInputState::new(None);
        state.delete_char(); // Nie powinno crashować
        assert_eq!(state.buffer, "");
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_move_cursor() {
        let mut state = TextInputState::new(None);
        state.insert_char('H');
        state.insert_char('e');
        state.insert_char('l');
        state.insert_char('l');
        state.insert_char('o');

        assert_eq!(state.cursor_pos, 5);

        state.move_cursor_left();
        assert_eq!(state.cursor_pos, 4);

        state.move_cursor_left();
        state.move_cursor_left();
        assert_eq!(state.cursor_pos, 2);

        state.move_cursor_right();
        assert_eq!(state.cursor_pos, 3);

        state.move_cursor_home();
        assert_eq!(state.cursor_pos, 0);

        state.move_cursor_end();
        assert_eq!(state.cursor_pos, 5);
    }

    #[test]
    fn test_cursor_bounds() {
        let mut state = TextInputState::new(None);
        state.move_cursor_left(); // Nie powinno zejść poniżej 0
        assert_eq!(state.cursor_pos, 0);

        state.insert_char('A');
        state.move_cursor_right(); // Nie powinno przekroczyć długości
        assert_eq!(state.cursor_pos, 1);
    }

    #[test]
    fn test_clear() {
        let mut state = TextInputState::new(None);
        state.insert_char('T');
        state.insert_char('e');
        state.insert_char('s');
        state.insert_char('t');
        state.clear();
        assert_eq!(state.buffer, "");
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_insert_at_middle() {
        let mut state = TextInputState::new(None);
        state.insert_char('H');
        state.insert_char('l');
        state.insert_char('o');
        // Buffer: "Hlo", cursor at 3

        state.move_cursor_left();
        state.move_cursor_left();
        // Cursor at 1 (między H i l)

        state.insert_char('e');
        // Buffer powinien być "Helo"
        assert_eq!(state.buffer, "Helo");
        assert_eq!(state.cursor_pos, 2);
    }

    #[test]
    fn test_char_to_byte() {
        let s = "Hello";
        assert_eq!(char_to_byte(s, 0), 0);
        assert_eq!(char_to_byte(s, 1), 1);
        assert_eq!(char_to_byte(s, 5), 5);
        assert_eq!(char_to_byte(s, 10), 5); // Poza zakresem -> len
    }

    #[test]
    fn test_char_to_byte_unicode() {
        let s = "🔥Hello";
        // 🔥 zajmuje 4 bajty
        assert_eq!(char_to_byte(s, 0), 0); // Przed emoji
        assert_eq!(char_to_byte(s, 1), 4); // Po emoji
        assert_eq!(char_to_byte(s, 2), 5); // Po 'H'
    }

    #[test]
    fn test_widget_render_basic() {
        let theme = Theme::default();
        let state = TextInputState::new(Some("Type here...".to_string()));
        let widget = TextInputWidget::new("What is your name?", &state, &theme, true);

        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 10));
        widget.render(Rect::new(0, 0, 80, 10), &mut buf);

        // Sprawdź że prompt "> " jest renderowany w linii inputu
        // (pytanie + separator + input = linia 2 = y=2)
        let input_y = 2;
        let cell = &buf[(0, input_y)];
        assert_eq!(cell.symbol(), ">");
    }

    #[test]
    fn test_widget_render_zero_area() {
        let theme = Theme::default();
        let state = TextInputState::new(None);
        let widget = TextInputWidget::new("Q?", &state, &theme, true);

        // Nie powinno panikować przy zerowym area
        let mut buf = Buffer::empty(Rect::new(0, 0, 0, 0));
        widget.render(Rect::new(0, 0, 0, 0), &mut buf);
    }

    #[test]
    fn test_widget_render_with_text_focused() {
        let theme = Theme::default();
        let mut state = TextInputState::new(None);
        state.insert_char('A');
        state.insert_char('B');
        state.move_cursor_home(); // Kursor na 'A'

        let widget = TextInputWidget::new("Q?", &state, &theme, true);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 5));
        widget.render(Rect::new(0, 0, 80, 5), &mut buf);

        // Kursor na znaku 'A' powinien mieć styl REVERSED (nie dodatkowy █)
        let input_y = 2;
        // "> " prefix (2 znaki) + kursor na 'A' na pozycji 2
        let cell_a = &buf[(2, input_y)];
        assert_eq!(cell_a.symbol(), "A");
        assert!(cell_a.modifier.contains(Modifier::REVERSED));

        // 'B' nie ma kursora — zwykły styl
        let cell_b = &buf[(3, input_y)];
        assert_eq!(cell_b.symbol(), "B");
        assert!(!cell_b.modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn test_unicode_insert_and_delete() {
        let mut state = TextInputState::new(None);
        // Polskie znaki
        state.insert_char('ą');
        state.insert_char('ę');
        state.insert_char('ś');
        assert_eq!(state.value(), "ąęś");
        assert_eq!(state.cursor_pos, 3);

        state.delete_char();
        assert_eq!(state.value(), "ąę");
        assert_eq!(state.cursor_pos, 2);

        // Wstaw w środek
        state.move_cursor_left();
        state.insert_char('ź');
        assert_eq!(state.value(), "ąźę");
    }

    #[test]
    fn test_value_returns_buffer_ref() {
        let mut state = TextInputState::new(None);
        state.insert_char('x');
        assert_eq!(state.value(), "x");
        assert!(std::ptr::eq(
            state.value().as_bytes(),
            state.buffer.as_bytes()
        ));
    }
}
