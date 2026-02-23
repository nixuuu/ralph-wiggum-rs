/// Multiline text input widget state z visual wrapping awareness.
///
/// Buffer przechowuje tekst z `\n` jako separator logicznych linii.
/// Cursor to `(row, col)` w logical lines.
/// Visual wrapping: linie zawijane na widget width, kursor up/down
/// porusza się po visual rows (uwzględnia wrap).
///
/// Shift+Enter = newline, Enter = submit (zwraca Some(String)).
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

// ── Helpers ──────────────────────────────────────────────────────────────

/// Konwertuje char index na byte offset w stringu.
/// Jeśli `char_idx` poza zakresem → zwraca `s.len()`.
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Display width znaku (uwzględnia wide chars, np. CJK).
fn char_display_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

// ── Visual line ──────────────────────────────────────────────────────────

/// Fragment logicznej linii po wrappingu na daną szerokość.
///
/// `char_offset` to char index w logical line, od którego zaczyna się fragment.
/// `char_count` to liczba znaków w fragmencie.
#[derive(Debug, Clone, PartialEq)]
pub struct VisualLine {
    /// Indeks logicznej linii (0-based)
    pub logical_row: usize,
    /// Char offset w logical line (0-based)
    pub char_offset: usize,
    /// Liczba znaków w tym fragmencie
    pub char_count: usize,
}

/// Wrapuje jedną logiczną linię na fragmenty o max display width `width`.
///
/// Zwraca wektor VisualLine (minimum 1 element — pusta linia → 1 fragment).
fn wrap_logical_line(line: &str, width: usize, logical_row: usize) -> Vec<VisualLine> {
    if width == 0 {
        // Nie da się wrapować — zwróć całą linię jako 1 fragment
        return vec![VisualLine {
            logical_row,
            char_offset: 0,
            char_count: line.chars().count(),
        }];
    }

    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return vec![VisualLine {
            logical_row,
            char_offset: 0,
            char_count: 0,
        }];
    }

    let mut result = Vec::new();
    let mut frag_start = 0;
    let mut current_width = 0;

    for (i, &ch) in chars.iter().enumerate() {
        let ch_w = char_display_width(ch);

        if current_width + ch_w > width && current_width > 0 {
            // Fragment pełny — odetnij
            result.push(VisualLine {
                logical_row,
                char_offset: frag_start,
                char_count: i - frag_start,
            });
            frag_start = i;
            current_width = ch_w;
        } else {
            current_width += ch_w;
        }
    }

    // Ostatni fragment
    result.push(VisualLine {
        logical_row,
        char_offset: frag_start,
        char_count: chars.len() - frag_start,
    });

    result
}

/// Buduje listę visual lines z bufora. Każda logiczna linia jest wrapowana
/// na fragmenty o max `wrap_width` display columns.
fn build_visual_lines(buffer: &str, wrap_width: usize) -> Vec<VisualLine> {
    let mut visual_lines = Vec::new();

    if buffer.is_empty() {
        // Pusty buffer — 1 visual line (pusta)
        visual_lines.push(VisualLine {
            logical_row: 0,
            char_offset: 0,
            char_count: 0,
        });
        return visual_lines;
    }

    for (logical_row, line) in buffer.split('\n').enumerate() {
        visual_lines.extend(wrap_logical_line(line, wrap_width, logical_row));
    }

    visual_lines
}

// ── State ────────────────────────────────────────────────────────────────

/// Stan multiline text input z visual wrapping awareness.
///
/// Buffer: `String` z `\n` jako separator logicznych linii.
/// Cursor: `(row, col)` — row to indeks logicznej linii, col to char index w tej linii.
/// `wrap_width` określa szerokość wrappingu (ustawiany przez widget).
#[derive(Debug, Clone)]
pub struct MultilineTextInputState {
    /// Buffer tekstowy (multiline, `\n` jako separator)
    buffer: String,
    /// Wiersz kursora (indeks logicznej linii, 0-based)
    cursor_row: usize,
    /// Kolumna kursora (char index w logicznej linii, 0-based)
    cursor_col: usize,
    /// Szerokość wrappingu w kolumnach (display width). 0 = brak wrappingu.
    wrap_width: usize,
}

impl MultilineTextInputState {
    /// Tworzy nowy stan z pustym buforem.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor_row: 0,
            cursor_col: 0,
            wrap_width: 0,
        }
    }

    /// Tworzy stan z initial content.
    pub fn with_content(content: &str) -> Self {
        let lines: Vec<&str> = content.split('\n').collect();
        let last_row = lines.len().saturating_sub(1);
        let last_col = lines.last().map_or(0, |l| l.chars().count());

        Self {
            buffer: content.to_string(),
            cursor_row: last_row,
            cursor_col: last_col,
            wrap_width: 0,
        }
    }

    // ── Accessors ────────────────────────────────────────────────────

    /// Zwraca referencję do bufora.
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    /// Zwraca pozycję kursora: (row, col) w logical lines.
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    /// Ustawia szerokość wrappingu (wywoływane przez widget przy renderze).
    pub fn set_wrap_width(&mut self, width: usize) {
        self.wrap_width = width;
    }

    /// Zwraca aktualny wrap width.
    pub fn wrap_width(&self) -> usize {
        self.wrap_width
    }

    /// Zwraca logiczne linie bufora.
    fn logical_lines(&self) -> Vec<&str> {
        if self.buffer.is_empty() {
            vec![""]
        } else {
            self.buffer.split('\n').collect()
        }
    }

    /// Zwraca liczbę znaków w danej logicznej linii.
    fn line_char_count(&self, row: usize) -> usize {
        self.logical_lines()
            .get(row)
            .map_or(0, |l| l.chars().count())
    }

    /// Zwraca liczbę logicznych linii.
    fn line_count(&self) -> usize {
        if self.buffer.is_empty() {
            1
        } else {
            self.buffer.split('\n').count()
        }
    }

    /// Clamps cursor position — zapewnia, że cursor jest w granicach bufora.
    fn clamp_cursor(&mut self) {
        let line_count = self.line_count();
        if self.cursor_row >= line_count {
            self.cursor_row = line_count.saturating_sub(1);
        }
        let max_col = self.line_char_count(self.cursor_row);
        if self.cursor_col > max_col {
            self.cursor_col = max_col;
        }
    }

    /// Zwraca byte offset w buforze dla bieżącej pozycji kursora.
    fn cursor_byte_offset(&self) -> usize {
        let lines = self.logical_lines();
        let mut offset = 0;
        for (i, line) in lines.iter().enumerate() {
            if i == self.cursor_row {
                return offset + char_to_byte(line, self.cursor_col);
            }
            offset += line.len() + 1; // +1 for '\n'
        }
        self.buffer.len()
    }

    // ── Edit operations ──────────────────────────────────────────────

    /// Wstawia znak na pozycji kursora.
    pub fn insert_char(&mut self, c: char) {
        let byte_pos = self.cursor_byte_offset();
        self.buffer.insert(byte_pos, c);
        self.cursor_col += 1;
    }

    /// Wstawia nową linię (Shift+Enter).
    pub fn insert_newline(&mut self) {
        let byte_pos = self.cursor_byte_offset();
        self.buffer.insert(byte_pos, '\n');
        self.cursor_row += 1;
        self.cursor_col = 0;
    }

    /// Usuwa znak przed kursorem (backspace).
    ///
    /// Jeśli kursor na początku linii i jest poprzednia linia,
    /// łączy bieżącą linię z poprzednią.
    pub fn delete_char(&mut self) {
        if self.cursor_col > 0 {
            // Oblicz byte offset poprzedniego znaku bezpośrednio
            let lines = self.logical_lines();
            let line = lines[self.cursor_row];
            let mut line_byte_start = 0;
            for (i, l) in lines.iter().enumerate() {
                if i == self.cursor_row {
                    break;
                }
                line_byte_start += l.len() + 1; // +1 for '\n'
            }
            let absolute_byte = line_byte_start + char_to_byte(line, self.cursor_col - 1);
            self.buffer.remove(absolute_byte);
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            // Kursor na początku linii — łącz z poprzednią (usuń '\n')
            let prev_line_len = self.line_char_count(self.cursor_row - 1);
            // Byte offset '\n' kończącego poprzednią linię
            let byte_pos = self.cursor_byte_offset();
            // '\n' jest bajtem tuż przed bieżącą pozycją
            self.buffer.remove(byte_pos - 1);
            self.cursor_row -= 1;
            self.cursor_col = prev_line_len;
        }
    }

    /// Usuwa znak pod kursorem (forward delete).
    ///
    /// Jeśli kursor na końcu linii i jest następna linia,
    /// łączy następną linię z bieżącą (usuwa '\n').
    pub fn delete_char_forward(&mut self) {
        let line_len = self.line_char_count(self.cursor_row);
        if self.cursor_col < line_len {
            // Usuń znak pod kursorem
            let byte_pos = self.cursor_byte_offset();
            self.buffer.remove(byte_pos);
        } else if self.cursor_row < self.line_count() - 1 {
            // Kursor na końcu linii — łącz z następną (usuń '\n')
            let byte_pos = self.cursor_byte_offset();
            self.buffer.remove(byte_pos);
        }
    }

    /// Czyści cały bufor.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    // ── Cursor movement ──────────────────────────────────────────────

    /// Przesuwa kursor w lewo o 1 znak.
    /// Jeśli na początku linii, przechodzi na koniec poprzedniej.
    pub fn move_cursor_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.line_char_count(self.cursor_row);
        }
    }

    /// Przesuwa kursor w prawo o 1 znak.
    /// Jeśli na końcu linii, przechodzi na początek następnej.
    pub fn move_cursor_right(&mut self) {
        let line_len = self.line_char_count(self.cursor_row);
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_row < self.line_count() - 1 {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    /// Przesuwa kursor na początek bieżącej linii.
    pub fn move_cursor_home(&mut self) {
        self.cursor_col = 0;
    }

    /// Przesuwa kursor na koniec bieżącej linii.
    pub fn move_cursor_end(&mut self) {
        self.cursor_col = self.line_char_count(self.cursor_row);
    }

    /// Przesuwa kursor w górę.
    ///
    /// Z visual wrapping awareness: jeśli kursor jest w wrapped line,
    /// move_up przenosi na poprzedni visual row (nie logical row).
    pub fn move_cursor_up(&mut self) {
        let wrap = self.effective_wrap_width();
        if wrap == 0 {
            // Brak wrappingu — proste przeniesienie na poprzednią logical line
            if self.cursor_row > 0 {
                self.cursor_row -= 1;
                self.clamp_cursor();
            }
            return;
        }

        let visual_lines = build_visual_lines(&self.buffer, wrap);
        if let Some(current_vl_idx) = self.find_visual_line_index(&visual_lines) {
            if current_vl_idx == 0 {
                return; // Już na samej górze
            }

            let prev_vl = &visual_lines[current_vl_idx - 1];
            // Oblicz target col: display column kursora w bieżącym visual row
            let current_vl = &visual_lines[current_vl_idx];
            let col_in_visual = self.cursor_col - current_vl.char_offset;
            let target_col_display = self.char_col_to_display_col(current_vl, col_in_visual);

            self.cursor_row = prev_vl.logical_row;
            self.cursor_col =
                prev_vl.char_offset + self.display_col_to_char_col(prev_vl, target_col_display);
            self.clamp_cursor();
        }
    }

    /// Przesuwa kursor w dół.
    ///
    /// Z visual wrapping awareness: jeśli kursor jest w wrapped line,
    /// move_down przenosi na następny visual row (nie logical row).
    pub fn move_cursor_down(&mut self) {
        let wrap = self.effective_wrap_width();
        if wrap == 0 {
            // Brak wrappingu — proste przeniesienie na następną logical line
            if self.cursor_row < self.line_count() - 1 {
                self.cursor_row += 1;
                self.clamp_cursor();
            }
            return;
        }

        let visual_lines = build_visual_lines(&self.buffer, wrap);
        if let Some(current_vl_idx) = self.find_visual_line_index(&visual_lines) {
            if current_vl_idx >= visual_lines.len() - 1 {
                return; // Już na samym dole
            }

            let next_vl = &visual_lines[current_vl_idx + 1];
            let current_vl = &visual_lines[current_vl_idx];
            let col_in_visual = self.cursor_col - current_vl.char_offset;
            let target_col_display = self.char_col_to_display_col(current_vl, col_in_visual);

            self.cursor_row = next_vl.logical_row;
            self.cursor_col =
                next_vl.char_offset + self.display_col_to_char_col(next_vl, target_col_display);
            self.clamp_cursor();
        }
    }

    // ── Visual wrapping helpers ──────────────────────────────────────

    /// Effective wrap width — 0 oznacza brak wrappingu.
    fn effective_wrap_width(&self) -> usize {
        self.wrap_width
    }

    /// Znajduje indeks visual line zawierającego aktualną pozycję kursora.
    fn find_visual_line_index(&self, visual_lines: &[VisualLine]) -> Option<usize> {
        for (i, vl) in visual_lines.iter().enumerate() {
            if vl.logical_row != self.cursor_row {
                continue;
            }
            let col_in_range = self.cursor_col >= vl.char_offset
                && self.cursor_col <= vl.char_offset + vl.char_count;

            // Kursor na końcu fragmentu (boundary) — jeśli jest następny fragment
            // w tej samej logical line, to kursor należy do następnego fragmentu.
            // Wyjątek: kursor na końcu linii (cursor_col == line_char_count).
            if col_in_range {
                let at_fragment_end = self.cursor_col == vl.char_offset + vl.char_count;
                let is_line_end = self.cursor_col == self.line_char_count(self.cursor_row);
                if at_fragment_end && !is_line_end {
                    // Sprawdź czy następny fragment jest w tej samej logical line
                    if let Some(next_vl) = visual_lines.get(i + 1)
                        && next_vl.logical_row == self.cursor_row
                    {
                        continue; // Kursor należy do następnego fragmentu
                    }
                }
                return Some(i);
            }
        }
        // Fallback — ostatni visual line
        Some(visual_lines.len().saturating_sub(1))
    }

    /// Konwertuje char col offset w visual line na display column.
    fn char_col_to_display_col(&self, vl: &VisualLine, char_col: usize) -> usize {
        let lines = self.logical_lines();
        let line = lines.get(vl.logical_row).unwrap_or(&"");
        let start_char = vl.char_offset;
        let mut display_col = 0;
        for (i, ch) in line.chars().enumerate() {
            if i < start_char {
                continue;
            }
            if i >= start_char + char_col {
                break;
            }
            display_col += char_display_width(ch);
        }
        display_col
    }

    /// Konwertuje display column na char col offset w visual line.
    /// Zwraca char offset od `vl.char_offset` (relatywny do visual line).
    fn display_col_to_char_col(&self, vl: &VisualLine, target_display_col: usize) -> usize {
        let lines = self.logical_lines();
        let line = lines.get(vl.logical_row).unwrap_or(&"");
        let start_char = vl.char_offset;
        let mut display_col = 0;
        let mut char_col = 0;

        for (i, ch) in line.chars().enumerate() {
            if i < start_char {
                continue;
            }
            if char_col >= vl.char_count {
                break;
            }
            let ch_w = char_display_width(ch);
            if display_col + ch_w > target_display_col {
                break;
            }
            display_col += ch_w;
            char_col += 1;
        }

        char_col
    }

    /// Zwraca visual lines dla aktualnego bufora i wrap width.
    pub fn visual_lines(&self) -> Vec<VisualLine> {
        let wrap = self.effective_wrap_width();
        if wrap == 0 {
            // Bez wrappingu — 1 visual line per logical line
            self.logical_lines()
                .iter()
                .enumerate()
                .map(|(i, l)| VisualLine {
                    logical_row: i,
                    char_offset: 0,
                    char_count: l.chars().count(),
                })
                .collect()
        } else {
            build_visual_lines(&self.buffer, wrap)
        }
    }

    // ── Key event handling ───────────────────────────────────────────

    /// Obsługuje key event.
    ///
    /// - **Enter** → submit: zwraca `Some(buffer_content)`
    /// - **Shift+Enter** → wstawia newline
    /// - **Backspace** → delete_char (backward)
    /// - **Delete** → delete_char_forward
    /// - **Strzałki** → nawigacja
    /// - **Home/End, Ctrl+A/Ctrl+E** → początek/koniec linii
    /// - **Char(c)** → insert_char
    ///
    /// Zwraca `Some(String)` jeśli użytkownik submituje (Enter),
    /// `None` jeśli klawisz obsłużony bez submita.
    pub fn handle_key_event(&mut self, key: KeyEvent) -> Option<String> {
        // Shift+Enter → newline (nie submit)
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
            self.insert_newline();
            return None;
        }

        // Enter → submit
        if key.code == KeyCode::Enter {
            return Some(self.buffer.clone());
        }

        match key.code {
            KeyCode::Backspace => self.delete_char(),
            KeyCode::Delete => self.delete_char_forward(),
            KeyCode::Left => self.move_cursor_left(),
            KeyCode::Right => self.move_cursor_right(),
            KeyCode::Up => self.move_cursor_up(),
            KeyCode::Down => self.move_cursor_down(),
            KeyCode::Home => self.move_cursor_home(),
            KeyCode::End => self.move_cursor_end(),
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_cursor_home();
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_cursor_end();
            }
            KeyCode::Char(c) => self.insert_char(c),
            _ => {}
        }

        None
    }
}

impl Default for MultilineTextInputState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Construction ─────────────────────────────────────────────────

    #[test]
    fn test_new_empty() {
        let state = MultilineTextInputState::new();
        assert_eq!(state.buffer(), "");
        assert_eq!(state.cursor(), (0, 0));
    }

    #[test]
    fn test_with_content() {
        let state = MultilineTextInputState::with_content("hello\nworld");
        assert_eq!(state.buffer(), "hello\nworld");
        assert_eq!(state.cursor(), (1, 5)); // koniec "world"
    }

    #[test]
    fn test_with_content_single_line() {
        let state = MultilineTextInputState::with_content("abc");
        assert_eq!(state.cursor(), (0, 3));
    }

    #[test]
    fn test_default() {
        let state = MultilineTextInputState::default();
        assert_eq!(state.buffer(), "");
        assert_eq!(state.cursor(), (0, 0));
    }

    // ── insert_char ──────────────────────────────────────────────────

    #[test]
    fn test_insert_char_empty() {
        let mut state = MultilineTextInputState::new();
        state.insert_char('a');
        assert_eq!(state.buffer(), "a");
        assert_eq!(state.cursor(), (0, 1));
    }

    #[test]
    fn test_insert_char_multiple() {
        let mut state = MultilineTextInputState::new();
        state.insert_char('a');
        state.insert_char('b');
        state.insert_char('c');
        assert_eq!(state.buffer(), "abc");
        assert_eq!(state.cursor(), (0, 3));
    }

    #[test]
    fn test_insert_char_at_middle() {
        let mut state = MultilineTextInputState::with_content("ac");
        state.cursor_col = 1; // między 'a' i 'c'
        state.insert_char('b');
        assert_eq!(state.buffer(), "abc");
        assert_eq!(state.cursor(), (0, 2));
    }

    #[test]
    fn test_insert_char_unicode() {
        let mut state = MultilineTextInputState::new();
        state.insert_char('ą');
        state.insert_char('ę');
        state.insert_char('ś');
        assert_eq!(state.buffer(), "ąęś");
        assert_eq!(state.cursor(), (0, 3));
    }

    #[test]
    fn test_insert_char_second_line() {
        let mut state = MultilineTextInputState::with_content("hello\n");
        // cursor at (1, 0) — pusta druga linia
        assert_eq!(state.cursor(), (1, 0));
        state.insert_char('w');
        assert_eq!(state.buffer(), "hello\nw");
        assert_eq!(state.cursor(), (1, 1));
    }

    // ── insert_newline ───────────────────────────────────────────────

    #[test]
    fn test_insert_newline_empty() {
        let mut state = MultilineTextInputState::new();
        state.insert_newline();
        assert_eq!(state.buffer(), "\n");
        assert_eq!(state.cursor(), (1, 0));
    }

    #[test]
    fn test_insert_newline_after_text() {
        let mut state = MultilineTextInputState::new();
        state.insert_char('a');
        state.insert_char('b');
        state.insert_newline();
        assert_eq!(state.buffer(), "ab\n");
        assert_eq!(state.cursor(), (1, 0));
    }

    #[test]
    fn test_insert_newline_at_middle() {
        let mut state = MultilineTextInputState::with_content("ac");
        state.cursor_col = 1; // między 'a' i 'c'
        state.insert_newline();
        assert_eq!(state.buffer(), "a\nc");
        assert_eq!(state.cursor(), (1, 0));
    }

    // ── delete_char ──────────────────────────────────────────────────

    #[test]
    fn test_delete_char_at_end() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.delete_char();
        assert_eq!(state.buffer(), "ab");
        assert_eq!(state.cursor(), (0, 2));
    }

    #[test]
    fn test_delete_char_at_start_noop() {
        let mut state = MultilineTextInputState::new();
        state.delete_char(); // Nie powinno crashować
        assert_eq!(state.buffer(), "");
        assert_eq!(state.cursor(), (0, 0));
    }

    #[test]
    fn test_delete_char_at_middle() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.cursor_col = 2; // po 'b'
        state.delete_char();
        assert_eq!(state.buffer(), "ac");
        assert_eq!(state.cursor(), (0, 1));
    }

    #[test]
    fn test_delete_char_unicode() {
        let mut state = MultilineTextInputState::with_content("ąęś");
        state.delete_char();
        assert_eq!(state.buffer(), "ąę");
        assert_eq!(state.cursor(), (0, 2));
    }

    #[test]
    fn test_delete_char_joins_lines() {
        let mut state = MultilineTextInputState::with_content("ab\ncd");
        state.cursor_row = 1;
        state.cursor_col = 0; // początek drugiej linii
        state.delete_char(); // Usuwa '\n', łączy linie
        assert_eq!(state.buffer(), "abcd");
        assert_eq!(state.cursor(), (0, 2));
    }

    #[test]
    fn test_delete_char_first_line_start_noop() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.cursor_col = 0;
        state.delete_char();
        assert_eq!(state.buffer(), "abc");
        assert_eq!(state.cursor(), (0, 0));
    }

    // ── move_cursor_left ─────────────────────────────────────────────

    #[test]
    fn test_move_left_basic() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.move_cursor_left();
        assert_eq!(state.cursor(), (0, 2));
    }

    #[test]
    fn test_move_left_start_of_line_wraps() {
        let mut state = MultilineTextInputState::with_content("ab\ncd");
        state.cursor_row = 1;
        state.cursor_col = 0;
        state.move_cursor_left(); // Przechodzi na koniec pierwszej linii
        assert_eq!(state.cursor(), (0, 2));
    }

    #[test]
    fn test_move_left_start_of_buffer_noop() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.cursor_col = 0;
        state.move_cursor_left();
        assert_eq!(state.cursor(), (0, 0));
    }

    // ── move_cursor_right ────────────────────────────────────────────

    #[test]
    fn test_move_right_basic() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.cursor_col = 0;
        state.move_cursor_right();
        assert_eq!(state.cursor(), (0, 1));
    }

    #[test]
    fn test_move_right_end_of_line_wraps() {
        let mut state = MultilineTextInputState::with_content("ab\ncd");
        state.cursor_row = 0;
        state.cursor_col = 2; // koniec "ab"
        state.move_cursor_right(); // Przechodzi na początek "cd"
        assert_eq!(state.cursor(), (1, 0));
    }

    #[test]
    fn test_move_right_end_of_buffer_noop() {
        let mut state = MultilineTextInputState::with_content("abc");
        // cursor at (0, 3) — end
        state.move_cursor_right();
        assert_eq!(state.cursor(), (0, 3));
    }

    // ── move_cursor_home/end ─────────────────────────────────────────

    #[test]
    fn test_move_home() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.move_cursor_home();
        assert_eq!(state.cursor(), (0, 0));
    }

    #[test]
    fn test_move_end() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.cursor_col = 0;
        state.move_cursor_end();
        assert_eq!(state.cursor(), (0, 3));
    }

    #[test]
    fn test_move_home_second_line() {
        let mut state = MultilineTextInputState::with_content("abc\ndefgh");
        // cursor at (1, 5)
        state.move_cursor_home();
        assert_eq!(state.cursor(), (1, 0));
    }

    #[test]
    fn test_move_end_second_line() {
        let mut state = MultilineTextInputState::with_content("abc\ndefgh");
        state.cursor_col = 0;
        state.move_cursor_end();
        assert_eq!(state.cursor(), (1, 5));
    }

    // ── move_cursor_up/down (no wrap) ────────────────────────────────

    #[test]
    fn test_move_up_no_wrap() {
        let mut state = MultilineTextInputState::with_content("abc\ndef");
        // cursor at (1, 3) — end of "def"
        state.move_cursor_up();
        assert_eq!(state.cursor(), (0, 3)); // end of "abc"
    }

    #[test]
    fn test_move_up_clamps_col() {
        let mut state = MultilineTextInputState::with_content("ab\ndefgh");
        // cursor at (1, 5) — end of "defgh"
        state.move_cursor_up();
        assert_eq!(state.cursor(), (0, 2)); // clamped to end of "ab"
    }

    #[test]
    fn test_move_up_first_line_noop() {
        let mut state = MultilineTextInputState::with_content("abc\ndef");
        state.cursor_row = 0;
        state.cursor_col = 1;
        state.move_cursor_up();
        assert_eq!(state.cursor(), (0, 1));
    }

    #[test]
    fn test_move_down_no_wrap() {
        let mut state = MultilineTextInputState::with_content("abc\ndef");
        state.cursor_row = 0;
        state.cursor_col = 2;
        state.move_cursor_down();
        assert_eq!(state.cursor(), (1, 2));
    }

    #[test]
    fn test_move_down_clamps_col() {
        let mut state = MultilineTextInputState::with_content("abcde\nfg");
        state.cursor_row = 0;
        state.cursor_col = 4;
        state.move_cursor_down();
        assert_eq!(state.cursor(), (1, 2)); // clamped to end of "fg"
    }

    #[test]
    fn test_move_down_last_line_noop() {
        let mut state = MultilineTextInputState::with_content("abc\ndef");
        // cursor at (1, 3)
        state.move_cursor_down();
        assert_eq!(state.cursor(), (1, 3));
    }

    // ── move_cursor_up/down with visual wrap ─────────────────────────

    #[test]
    fn test_move_up_with_wrap_within_same_logical_line() {
        // Logical line "abcdefghij" wraps at width=5 →
        // visual row 0: "abcde"
        // visual row 1: "fghij"
        let mut state = MultilineTextInputState::with_content("abcdefghij");
        state.set_wrap_width(5);
        state.cursor_row = 0;
        state.cursor_col = 7; // 'h' w visual row 1

        state.move_cursor_up();
        // Powinien przejść do visual row 0, col 2 (display col 2)
        assert_eq!(state.cursor(), (0, 2));
    }

    #[test]
    fn test_move_down_with_wrap_within_same_logical_line() {
        let mut state = MultilineTextInputState::with_content("abcdefghij");
        state.set_wrap_width(5);
        state.cursor_row = 0;
        state.cursor_col = 2; // 'c' w visual row 0

        state.move_cursor_down();
        // Powinien przejść do visual row 1, col offset=5, display col 2 → char 7
        assert_eq!(state.cursor(), (0, 7));
    }

    #[test]
    fn test_move_up_across_logical_lines_with_wrap() {
        // Line 0: "abcde" (1 visual row)
        // Line 1: "fghij" (1 visual row)
        let mut state = MultilineTextInputState::with_content("abcde\nfghij");
        state.set_wrap_width(10);
        state.cursor_row = 1;
        state.cursor_col = 3;

        state.move_cursor_up();
        assert_eq!(state.cursor(), (0, 3));
    }

    #[test]
    fn test_move_down_across_logical_lines_with_wrap() {
        let mut state = MultilineTextInputState::with_content("abcde\nfghij");
        state.set_wrap_width(10);
        state.cursor_row = 0;
        state.cursor_col = 3;

        state.move_cursor_down();
        assert_eq!(state.cursor(), (1, 3));
    }

    #[test]
    fn test_move_up_from_wrapped_second_row_to_first_logical() {
        // Line 0: "ab"
        // Line 1: "cdefghijkl" wraps at 5: "cdefg" "hijkl"
        let mut state = MultilineTextInputState::with_content("ab\ncdefghijkl");
        state.set_wrap_width(5);
        state.cursor_row = 1;
        state.cursor_col = 2; // 'e' in visual "cdefg" (first wrap row of line 1)

        state.move_cursor_up();
        assert_eq!(state.cursor(), (0, 2)); // end of "ab"
    }

    #[test]
    fn test_move_up_from_wrapped_third_row() {
        // Line 1: "cdefghijkl" wraps at 5: "cdefg" "hijkl"
        // Kursor w "hijkl" (visual row 2), col=7 ('h'), move_up → "cdefg" col=2
        let mut state = MultilineTextInputState::with_content("ab\ncdefghijkl");
        state.set_wrap_width(5);
        state.cursor_row = 1;
        state.cursor_col = 7; // 'j' in "hijkl" (visual row 2, display col 2)

        state.move_cursor_up();
        // Powinien przejść do "cdefg" (visual row 1 of logical line 1), display col 2
        assert_eq!(state.cursor(), (1, 2));
    }

    // ── handle_key_event ─────────────────────────────────────────────

    #[test]
    fn test_handle_enter_submits() {
        let mut state = MultilineTextInputState::new();
        state.insert_char('a');
        state.insert_char('b');

        let result = state.handle_key_event(KeyEvent::from(KeyCode::Enter));
        assert_eq!(result, Some("ab".to_string()));
    }

    #[test]
    fn test_handle_shift_enter_inserts_newline() {
        let mut state = MultilineTextInputState::new();
        state.insert_char('a');

        let mut key = KeyEvent::from(KeyCode::Enter);
        key.modifiers = KeyModifiers::SHIFT;
        let result = state.handle_key_event(key);
        assert_eq!(result, None);
        assert_eq!(state.buffer(), "a\n");
        assert_eq!(state.cursor(), (1, 0));
    }

    #[test]
    fn test_handle_char_input() {
        let mut state = MultilineTextInputState::new();
        let result = state.handle_key_event(KeyEvent::from(KeyCode::Char('x')));
        assert_eq!(result, None);
        assert_eq!(state.buffer(), "x");
    }

    #[test]
    fn test_handle_backspace() {
        let mut state = MultilineTextInputState::with_content("abc");
        let result = state.handle_key_event(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(result, None);
        assert_eq!(state.buffer(), "ab");
    }

    #[test]
    fn test_handle_arrows() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.handle_key_event(KeyEvent::from(KeyCode::Left));
        assert_eq!(state.cursor(), (0, 2));
        state.handle_key_event(KeyEvent::from(KeyCode::Right));
        assert_eq!(state.cursor(), (0, 3));
        state.handle_key_event(KeyEvent::from(KeyCode::Home));
        assert_eq!(state.cursor(), (0, 0));
        state.handle_key_event(KeyEvent::from(KeyCode::End));
        assert_eq!(state.cursor(), (0, 3));
    }

    #[test]
    fn test_handle_up_down() {
        let mut state = MultilineTextInputState::with_content("abc\ndef");
        state.cursor_row = 0;
        state.cursor_col = 1;
        state.handle_key_event(KeyEvent::from(KeyCode::Down));
        assert_eq!(state.cursor(), (1, 1));
        state.handle_key_event(KeyEvent::from(KeyCode::Up));
        assert_eq!(state.cursor(), (0, 1));
    }

    #[test]
    fn test_handle_unknown_key_noop() {
        let mut state = MultilineTextInputState::with_content("abc");
        let result = state.handle_key_event(KeyEvent::from(KeyCode::F(1)));
        assert_eq!(result, None);
        assert_eq!(state.buffer(), "abc");
    }

    // ── clear ────────────────────────────────────────────────────────

    #[test]
    fn test_clear() {
        let mut state = MultilineTextInputState::with_content("abc\ndef");
        state.clear();
        assert_eq!(state.buffer(), "");
        assert_eq!(state.cursor(), (0, 0));
    }

    // ── Wrapping helpers ─────────────────────────────────────────────

    #[test]
    fn test_wrap_logical_line_no_wrap() {
        let result = wrap_logical_line("abcde", 10, 0);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].char_count, 5);
    }

    #[test]
    fn test_wrap_logical_line_exact() {
        let result = wrap_logical_line("abcde", 5, 0);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].char_count, 5);
    }

    #[test]
    fn test_wrap_logical_line_overflow() {
        let result = wrap_logical_line("abcdefgh", 5, 0);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].char_offset, 0);
        assert_eq!(result[0].char_count, 5);
        assert_eq!(result[1].char_offset, 5);
        assert_eq!(result[1].char_count, 3);
    }

    #[test]
    fn test_wrap_logical_line_empty() {
        let result = wrap_logical_line("", 5, 0);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].char_count, 0);
    }

    #[test]
    fn test_wrap_logical_line_zero_width() {
        let result = wrap_logical_line("abc", 0, 0);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].char_count, 3);
    }

    #[test]
    fn test_build_visual_lines_multiline() {
        let lines = build_visual_lines("abc\ndefgh", 5);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].logical_row, 0);
        assert_eq!(lines[0].char_count, 3);
        assert_eq!(lines[1].logical_row, 1);
        assert_eq!(lines[1].char_count, 5);
    }

    #[test]
    fn test_build_visual_lines_with_wrap() {
        let lines = build_visual_lines("abcdefgh\nij", 5);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].logical_row, 0);
        assert_eq!(lines[0].char_count, 5); // "abcde"
        assert_eq!(lines[1].logical_row, 0);
        assert_eq!(lines[1].char_offset, 5);
        assert_eq!(lines[1].char_count, 3); // "fgh"
        assert_eq!(lines[2].logical_row, 1);
        assert_eq!(lines[2].char_count, 2); // "ij"
    }

    #[test]
    fn test_build_visual_lines_empty() {
        let lines = build_visual_lines("", 5);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].logical_row, 0);
        assert_eq!(lines[0].char_count, 0);
    }

    // ── Complex scenarios ────────────────────────────────────────────

    #[test]
    fn test_multiline_editing_flow() {
        let mut state = MultilineTextInputState::new();

        // Wpisz "Hello\nWorld"
        for c in "Hello".chars() {
            state.insert_char(c);
        }
        state.insert_newline();
        for c in "World".chars() {
            state.insert_char(c);
        }

        assert_eq!(state.buffer(), "Hello\nWorld");
        assert_eq!(state.cursor(), (1, 5));

        // Przesuń na początek drugiej linii
        state.move_cursor_home();
        assert_eq!(state.cursor(), (1, 0));

        // Wstaw "Dear " na początku drugiej linii
        for c in "Dear ".chars() {
            state.insert_char(c);
        }
        assert_eq!(state.buffer(), "Hello\nDear World");
        assert_eq!(state.cursor(), (1, 5));

        // Up → pierwsza linia
        state.move_cursor_up();
        assert_eq!(state.cursor(), (0, 5));

        // Home → początek
        state.move_cursor_home();
        assert_eq!(state.cursor(), (0, 0));

        // Wstaw "Oh, " na początku
        for c in "Oh, ".chars() {
            state.insert_char(c);
        }
        assert_eq!(state.buffer(), "Oh, Hello\nDear World");
    }

    #[test]
    fn test_delete_across_multiple_lines() {
        let mut state = MultilineTextInputState::with_content("a\nb\nc");
        // cursor at (2, 1) — end of 'c'

        // Delete 'c'
        state.delete_char();
        assert_eq!(state.buffer(), "a\nb\n");
        assert_eq!(state.cursor(), (2, 0));

        // Delete newline — joins with line 1
        state.delete_char();
        assert_eq!(state.buffer(), "a\nb");
        assert_eq!(state.cursor(), (1, 1));

        // Delete 'b'
        state.delete_char();
        assert_eq!(state.buffer(), "a\n");
        assert_eq!(state.cursor(), (1, 0));

        // Delete newline — joins with line 0
        state.delete_char();
        assert_eq!(state.buffer(), "a");
        assert_eq!(state.cursor(), (0, 1));
    }

    #[test]
    fn test_wrap_width_setter() {
        let mut state = MultilineTextInputState::new();
        assert_eq!(state.wrap_width(), 0);
        state.set_wrap_width(80);
        assert_eq!(state.wrap_width(), 80);
    }

    #[test]
    fn test_visual_lines_no_wrap() {
        let state = MultilineTextInputState::with_content("abc\ndef");
        let vl = state.visual_lines();
        assert_eq!(vl.len(), 2);
    }

    #[test]
    fn test_visual_lines_with_wrap() {
        let mut state = MultilineTextInputState::with_content("abcdefgh");
        state.set_wrap_width(5);
        let vl = state.visual_lines();
        assert_eq!(vl.len(), 2);
        assert_eq!(vl[0].char_count, 5);
        assert_eq!(vl[1].char_count, 3);
    }

    // ── Edge cases ───────────────────────────────────────────────────

    #[test]
    fn test_insert_emoji() {
        let mut state = MultilineTextInputState::new();
        state.insert_char('🎉');
        assert_eq!(state.buffer(), "🎉");
        assert_eq!(state.cursor(), (0, 1));
    }

    #[test]
    fn test_delete_emoji() {
        let mut state = MultilineTextInputState::with_content("🎉");
        state.delete_char();
        assert_eq!(state.buffer(), "");
        assert_eq!(state.cursor(), (0, 0));
    }

    #[test]
    fn test_consecutive_newlines() {
        let mut state = MultilineTextInputState::new();
        state.insert_newline();
        state.insert_newline();
        state.insert_char('x');
        assert_eq!(state.buffer(), "\n\nx");
        assert_eq!(state.cursor(), (2, 1));
    }

    #[test]
    fn test_move_left_right_across_newlines() {
        let mut state = MultilineTextInputState::with_content("a\nb");
        state.cursor_row = 0;
        state.cursor_col = 1;

        // Right → przechodzi na następną linię
        state.move_cursor_right();
        assert_eq!(state.cursor(), (1, 0));

        // Left → wraca na poprzednią linię
        state.move_cursor_left();
        assert_eq!(state.cursor(), (0, 1));
    }

    #[test]
    fn test_enter_submits_empty_buffer() {
        let mut state = MultilineTextInputState::new();
        let result = state.handle_key_event(KeyEvent::from(KeyCode::Enter));
        assert_eq!(result, Some("".to_string()));
    }

    #[test]
    fn test_enter_submits_multiline() {
        let mut state = MultilineTextInputState::with_content("hello\nworld");
        let result = state.handle_key_event(KeyEvent::from(KeyCode::Enter));
        assert_eq!(result, Some("hello\nworld".to_string()));
    }

    // ── delete_char_forward ─────────────────────────────────────────

    #[test]
    fn test_delete_forward_at_cursor() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.cursor_col = 1; // kursor na 'b'
        state.delete_char_forward();
        assert_eq!(state.buffer(), "ac");
        assert_eq!(state.cursor(), (0, 1));
    }

    #[test]
    fn test_delete_forward_at_start() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.cursor_col = 0;
        state.delete_char_forward();
        assert_eq!(state.buffer(), "bc");
        assert_eq!(state.cursor(), (0, 0));
    }

    #[test]
    fn test_delete_forward_at_end_noop() {
        let mut state = MultilineTextInputState::with_content("abc");
        // cursor at (0, 3) — koniec jedynej linii, brak następnej
        state.delete_char_forward();
        assert_eq!(state.buffer(), "abc");
    }

    #[test]
    fn test_delete_forward_joins_lines() {
        let mut state = MultilineTextInputState::with_content("ab\ncd");
        state.cursor_row = 0;
        state.cursor_col = 2; // koniec "ab"
        state.delete_char_forward(); // Usuwa '\n'
        assert_eq!(state.buffer(), "abcd");
        assert_eq!(state.cursor(), (0, 2));
    }

    #[test]
    fn test_delete_forward_unicode() {
        let mut state = MultilineTextInputState::with_content("ąęś");
        state.cursor_col = 1; // kursor na 'ę'
        state.delete_char_forward();
        assert_eq!(state.buffer(), "ąś");
        assert_eq!(state.cursor(), (0, 1));
    }

    #[test]
    fn test_delete_forward_emoji() {
        let mut state = MultilineTextInputState::with_content("a🎉b");
        state.cursor_col = 1; // kursor na '🎉'
        state.delete_char_forward();
        assert_eq!(state.buffer(), "ab");
        assert_eq!(state.cursor(), (0, 1));
    }

    #[test]
    fn test_handle_delete_key() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.cursor_col = 0;
        let result = state.handle_key_event(KeyEvent::from(KeyCode::Delete));
        assert_eq!(result, None);
        assert_eq!(state.buffer(), "bc");
    }

    // ── Ctrl+A / Ctrl+E ─────────────────────────────────────────────

    #[test]
    fn test_ctrl_a_moves_home() {
        let mut state = MultilineTextInputState::with_content("abc");
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        let result = state.handle_key_event(key);
        assert_eq!(result, None);
        assert_eq!(state.cursor(), (0, 0));
    }

    #[test]
    fn test_ctrl_e_moves_end() {
        let mut state = MultilineTextInputState::with_content("abc");
        state.cursor_col = 0;
        let key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        let result = state.handle_key_event(key);
        assert_eq!(result, None);
        assert_eq!(state.cursor(), (0, 3));
    }

    #[test]
    fn test_ctrl_a_second_line() {
        let mut state = MultilineTextInputState::with_content("abc\ndefgh");
        // cursor at (1, 5) — koniec "defgh"
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        state.handle_key_event(key);
        assert_eq!(state.cursor(), (1, 0));
    }

    #[test]
    fn test_ctrl_e_second_line() {
        let mut state = MultilineTextInputState::with_content("abc\ndefgh");
        state.cursor_col = 0;
        let key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        state.handle_key_event(key);
        assert_eq!(state.cursor(), (1, 5));
    }
}
