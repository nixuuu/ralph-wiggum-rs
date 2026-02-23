/// Multiline text input widget state z visual wrapping awareness.
///
/// Buffer przechowuje tekst z `\n` jako separator logicznych linii.
/// Cursor to `(row, col)` w logical lines.
/// Visual wrapping: linie zawijane na widget width, kursor up/down
/// porusza się po visual rows (uwzględnia wrap).
///
/// Shift+Enter = newline, Enter = submit (zwraca Some(String)).
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    widgets::{StatefulWidget, Widget},
};
use unicode_width::UnicodeWidthChar;

use crate::tui::{DEFAULT_THEME, Theme};

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
///
/// Viewport: `scroll_offset` (visual line index) + `viewport_height`.
/// Auto-follow: `clamp_scroll()` po każdej mutacji kursora gwarantuje,
/// że kursor jest zawsze widoczny. Manual scroll z poziomu widgetu jest wyłączony.
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
    /// Indeks pierwszej widocznej visual line (scroll offset). 0-based.
    scroll_offset: usize,
    /// Wysokość viewportu w wierszach visual (ustawiana przez widget przy renderze).
    /// 0 oznacza brak viewportu — clamp_scroll() jest wtedy no-op.
    viewport_height: usize,
}

impl MultilineTextInputState {
    /// Tworzy nowy stan z pustym buforem.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor_row: 0,
            cursor_col: 0,
            wrap_width: 0,
            scroll_offset: 0,
            viewport_height: 0,
        }
    }

    /// Tworzy stan z initial content. Kursor ustawiany na koniec tekstu.
    pub fn with_content(content: &str) -> Self {
        let lines: Vec<&str> = content.split('\n').collect();
        let last_row = lines.len().saturating_sub(1);
        let last_col = lines.last().map_or(0, |l| l.chars().count());

        Self {
            buffer: content.to_string(),
            cursor_row: last_row,
            cursor_col: last_col,
            wrap_width: 0,
            scroll_offset: 0,
            viewport_height: 0,
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
        // Wrapping zmienia układ visual lines — przelicz scroll.
        self.clamp_scroll();
    }

    /// Zwraca aktualny wrap width.
    pub fn wrap_width(&self) -> usize {
        self.wrap_width
    }

    /// Ustawia wysokość viewportu (wywoływane przez widget przy renderze).
    ///
    /// Po zmianie wysokości przelicza scroll_offset tak, by kursor był widoczny.
    pub fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = height;
        self.clamp_scroll();
    }

    /// Zwraca aktualną wysokość viewportu.
    pub fn viewport_height(&self) -> usize {
        self.viewport_height
    }

    /// Zwraca aktualny scroll offset (indeks pierwszej widocznej visual line).
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Ustawia pozycję kursora (row, col) z automatycznym clampem do granic bufora.
    ///
    /// Używane w testach i przy integracji z zewnętrznymi kontrolerami stanu.
    /// Po ustawieniu wywołuje `clamp_cursor()` i `clamp_scroll()`.
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor_row = row;
        self.cursor_col = col;
        self.clamp_cursor();
        self.clamp_scroll();
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

    // ── Viewport / scroll ────────────────────────────────────────────

    /// Dostosowuje `scroll_offset` tak, by kursor był zawsze widoczny w viewporcie.
    ///
    /// Viewport podąża za kursorem (auto-follow). Nigdy nie pozwala na manual scroll —
    /// po każdej mutacji kursora viewport jest natychmiast przeliczany.
    ///
    /// Jeśli `viewport_height == 0`, metoda jest no-op (viewport nieznany).
    pub fn clamp_scroll(&mut self) {
        if self.viewport_height == 0 {
            return;
        }

        // TODO: cache visual lines if perf becomes concern (build_visual_lines jest O(n),
        //       a clamp_scroll() + visible_lines_viewport() każde wołają visual_lines() osobno)
        let vl = self.visual_lines();
        // Znajdź visual line index kursora.
        let cursor_vl_idx = self.find_visual_line_index(&vl).unwrap_or(0);

        // Kursor powyżej widocznego obszaru → scroll up.
        if cursor_vl_idx < self.scroll_offset {
            self.scroll_offset = cursor_vl_idx;
        }

        // Kursor poniżej widocznego obszaru → scroll down.
        let last_visible_exclusive = self.scroll_offset + self.viewport_height;
        if cursor_vl_idx >= last_visible_exclusive {
            self.scroll_offset = cursor_vl_idx + 1 - self.viewport_height;
        }

        // Clamp scroll_offset do zakresu valid linii.
        let max_offset = vl.len().saturating_sub(self.viewport_height);
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }
    }

    /// Zwraca widoczny zakres visual lines dla viewportu o danej `height`.
    ///
    /// Używa aktualnego `scroll_offset` do wyznaczenia zakresu.
    /// Jeśli `scroll_offset + height > total_visual_lines`, zwraca tyle ile jest.
    pub fn visible_lines(&self, height: usize) -> Vec<VisualLine> {
        if height == 0 {
            return Vec::new();
        }
        let vl = self.visual_lines();
        let start = self.scroll_offset.min(vl.len().saturating_sub(1));
        let end = (start + height).min(vl.len());
        vl[start..end].to_vec()
    }

    /// Zwraca indeks visual line zawierającego kursor (globalny, przed scroll).
    ///
    /// Używany przez widget renderujący do zlokalizowania kursora wśród widocznych linii.
    pub fn cursor_visual_line_index(&self) -> Option<usize> {
        let vl = self.visual_lines();
        self.find_visual_line_index(&vl)
    }

    /// Zwraca widoczne visual lines dla bieżącego viewportu.
    ///
    /// Używa `self.viewport_height` i `self.scroll_offset` do wyznaczenia zakresu.
    /// Spójny z `clamp_scroll()` — oba operują na `self.viewport_height`.
    pub fn visible_lines_viewport(&self) -> Vec<VisualLine> {
        self.visible_lines(self.viewport_height)
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
        self.clamp_scroll();
    }

    /// Wstawia nową linię (Shift+Enter).
    pub fn insert_newline(&mut self) {
        let byte_pos = self.cursor_byte_offset();
        self.buffer.insert(byte_pos, '\n');
        self.cursor_row += 1;
        self.cursor_col = 0;
        self.clamp_scroll();
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
        self.clamp_scroll();
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
        self.clamp_scroll();
    }

    /// Czyści cały bufor i resetuje scroll.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.scroll_offset = 0;
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
        self.clamp_scroll();
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
        self.clamp_scroll();
    }

    /// Przesuwa kursor na początek bieżącej linii.
    pub fn move_cursor_home(&mut self) {
        self.cursor_col = 0;
        self.clamp_scroll();
    }

    /// Przesuwa kursor na koniec bieżącej linii.
    pub fn move_cursor_end(&mut self) {
        self.cursor_col = self.line_char_count(self.cursor_row);
        self.clamp_scroll();
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
            self.clamp_scroll();
            return;
        }

        let visual_lines = build_visual_lines(&self.buffer, wrap);
        if let Some(current_vl_idx) = self.find_visual_line_index(&visual_lines) {
            if current_vl_idx == 0 {
                self.clamp_scroll();
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
        self.clamp_scroll();
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
            self.clamp_scroll();
            return;
        }

        let visual_lines = build_visual_lines(&self.buffer, wrap);
        if let Some(current_vl_idx) = self.find_visual_line_index(&visual_lines) {
            if current_vl_idx >= visual_lines.len() - 1 {
                self.clamp_scroll();
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
        self.clamp_scroll();
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

// ── Rendering helpers ─────────────────────────────────────────────────────

/// Renderuje znaki visual line z opcjonalnym kursorem REVERSED.
///
/// Wspólna logika używana zarówno przez [`MultilineTextInputWidget`] (StatefulWidget)
/// jak i przez [`MultilineTextInput`] (Widget). Eliminuje duplikację kodu renderowania.
///
/// # Parametry
/// - `start_x` — pierwsza kolumna dla znaków (po prefiksie lub na początku obszaru)
/// - `max_x` — granica prawa (area.x + area.width), znaki poza nią są pomijane
/// - `y` — wiersz ekranu
/// - `chars` — slice znaków tego fragmentu visual line
/// - `cursor_col_in_fragment` — indeks znaku kursora w `chars`, lub `None` gdy brak kursora
///   (caller przekazuje `None` gdy widget nie jest focused)
/// - `cursor_base_style` — styl bazowy kursora; `REVERSED` jest dodawany do znaku pod kursorem,
///   styl niezmieniony używany jest dla blokowego `█` za ostatnim znakiem
fn render_line_chars(
    buf: &mut Buffer,
    start_x: u16,
    max_x: u16,
    y: u16,
    chars: &[char],
    cursor_col_in_fragment: Option<usize>,
    cursor_base_style: Style,
) {
    let cursor_style = cursor_base_style.add_modifier(Modifier::REVERSED);
    let mut x = start_x;

    for (i, &ch) in chars.iter().enumerate() {
        let ch_w = char_display_width(ch) as u16;
        if x + ch_w > max_x {
            break;
        }
        let style = if cursor_col_in_fragment == Some(i) {
            cursor_style
        } else {
            Style::default()
        };
        buf.set_string(x, y, ch.to_string(), style);
        x += ch_w;
    }

    // Kursor blokowy za ostatnim znakiem (kursor na końcu linii/fragmentu)
    if cursor_col_in_fragment == Some(chars.len()) && x < max_x {
        buf.set_string(x, y, "█", cursor_base_style);
    }
}

// ── Widget ────────────────────────────────────────────────────────────────

/// Widget renderujący multiline text input z auto-scroll viewportem.
///
/// Implementuje `StatefulWidget` — aktualizuje stan przy każdym render:
/// - Wywołuje `state.set_wrap_width(area.width)` — dostosowuje wrapping do szerokości
/// - Wywołuje `state.set_viewport_height(area.height)` — aktualizuje viewport height
/// - Używa `state.visible_lines_viewport()` do renderowania tylko widocznych linii
///
/// Kursor jest zawsze widoczny dzięki auto-follow w `clamp_scroll()`.
/// Manual scroll wyłączony — viewport zawsze podąża za kursorem.
#[allow(dead_code)] // TUI component — will be used when full TUI is integrated
pub struct MultilineTextInputWidget<'a> {
    /// Theme dla kolorów i stylów
    theme: &'a Theme,
    /// Czy widget jest aktywny (focused) — determinuje renderowanie kursora
    focused: bool,
    /// Placeholder wyświetlany gdy bufor jest pusty
    placeholder: Option<&'a str>,
}

#[allow(dead_code)] // TUI component methods — will be used when widget is integrated
impl<'a> MultilineTextInputWidget<'a> {
    /// Tworzy nowy widget.
    pub fn new(theme: &'a Theme, focused: bool) -> Self {
        Self {
            theme,
            focused,
            placeholder: None,
        }
    }

    /// Ustawia placeholder wyświetlany gdy bufor jest pusty.
    pub fn placeholder(mut self, text: &'a str) -> Self {
        self.placeholder = Some(text);
        self
    }

    /// Renderuje pusty bufor — kursor blokowy i/lub placeholder.
    fn render_empty(&self, area: Rect, buf: &mut Buffer) {
        let x = area.x;
        let y = area.y;
        if self.focused {
            // Kursor blokowy na pierwszej pozycji
            buf.set_string(x, y, "█", self.theme.primary_style());
            if let Some(ph) = self.placeholder {
                // Reszta placeholdera za kursorem (pomijamy pierwszy znak)
                let rest: String = ph.chars().skip(1).collect();
                if !rest.is_empty() {
                    buf.set_string(
                        x + 1,
                        y,
                        &rest,
                        self.theme.muted_style().add_modifier(Modifier::ITALIC),
                    );
                }
            }
        } else if let Some(ph) = self.placeholder {
            // Unfocused — pokaż placeholder bez kursora
            buf.set_string(
                x,
                y,
                ph,
                self.theme.muted_style().add_modifier(Modifier::ITALIC),
            );
        }
        // Pusty, unfocused, brak placeholder → nie renderuj nic
    }

    /// Renderuje pojedynczą visual line z kursorem na zadanej pozycji ekranu.
    ///
    /// `cursor_col_in_fragment` to pozycja kursora w obrębie tego fragmentu
    /// (relatywna do `vl.char_offset`), lub `None` jeśli kursor nie jest tutaj
    /// albo widget nie jest focused.
    fn render_visual_line(
        &self,
        area: Rect,
        buf: &mut Buffer,
        y: u16,
        vl: &VisualLine,
        logical_line: &str,
        cursor_col_in_fragment: Option<usize>,
    ) {
        let chars: Vec<char> = logical_line
            .chars()
            .skip(vl.char_offset)
            .take(vl.char_count)
            .collect();
        // cursor_col_in_fragment już uwzględnia focused (None gdy !focused) — patrz caller
        render_line_chars(
            buf,
            area.x,
            area.x + area.width,
            y,
            &chars,
            cursor_col_in_fragment,
            self.theme.primary_style(),
        );
    }
}

impl<'a> StatefulWidget for MultilineTextInputWidget<'a> {
    type State = MultilineTextInputState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Kluczowy krok integracji: aktualizuj wrap width i viewport height ze stanu.
        // clamp_scroll() jest wywoływane wewnątrz obu setterów — gwarantuje auto-follow.
        state.set_wrap_width(area.width as usize);
        state.set_viewport_height(area.height as usize);

        // Obsługa pustego bufora
        if state.buffer().is_empty() {
            self.render_empty(area, buf);
            return;
        }

        // Indeks visual line kursora (globalny, przed scroll)
        let cursor_vl_idx = state.cursor_visual_line_index().unwrap_or(0);
        let scroll_offset = state.scroll_offset();
        let (cursor_row, cursor_col) = state.cursor();

        // Pobierz widoczne visual lines (po uwzględnieniu scroll_offset)
        let visible = state.visible_lines_viewport();

        // Skopiuj zawartość bufora — unikamy confliktu borrows na state
        let buffer_content = state.buffer().to_string();
        let logical_lines: Vec<&str> = buffer_content.split('\n').collect();

        for (screen_y_idx, vl) in visible.iter().enumerate() {
            let y = area.y + screen_y_idx as u16;
            if y >= area.y + area.height {
                break;
            }

            let logical_line = logical_lines.get(vl.logical_row).copied().unwrap_or("");
            // Globalny indeks tej visual line = scroll_offset + pozycja w visible slice
            let global_vl_idx = scroll_offset + screen_y_idx;

            // Kursor jest na tej visual line gdy globalny indeks się zgadza i widget focused.
            // Przekazujemy None gdy !focused — render_line_chars nie renderuje wtedy kursora.
            let cursor_col_in_fragment =
                if self.focused && global_vl_idx == cursor_vl_idx && vl.logical_row == cursor_row {
                    cursor_col.checked_sub(vl.char_offset)
                } else {
                    None
                };

            self.render_visual_line(area, buf, y, vl, logical_line, cursor_col_in_fragment);
        }
    }
}

// ── MultilineTextInput (Widget) ───────────────────────────────────────────

/// Widget z prefixem `> ` implementujący ratatui `Widget`.
///
/// W odróżnieniu od [`MultilineTextInputWidget`] (StatefulWidget), ten widget
/// owni stan (`MultilineTextInputState`) i jest przeznaczony do jednorazowego
/// renderowania per-frame.
///
/// Prefix `> ` jest wyświetlany na pierwszej widocznej linii.
/// Kolejne visual linie (po zawinięciu) mają wcięcie `  ` (2 spacje),
/// aby content był wyrównany do pierwszej linii.
///
/// `content_width = area.width - 2` (prefix zajmuje 2 kolumny).
///
/// Kursor renderowany jako `Modifier::REVERSED` na znaku pod kursorem,
/// lub jako blokowy `█` gdy kursor jest za ostatnim znakiem.
/// Kolory z `DEFAULT_THEME` (primary dla prefiksu i kursora).
#[allow(dead_code)] // TUI component — will be used when full TUI is integrated
pub struct MultilineTextInput {
    state: MultilineTextInputState,
    theme: Theme,
    /// Czy widget jest aktywny (focused) — determinuje renderowanie kursora.
    focused: bool,
    /// Placeholder wyświetlany gdy bufor jest pusty (opcjonalny).
    placeholder: Option<String>,
}

#[allow(dead_code)] // TUI component methods — will be used when widget is integrated
impl MultilineTextInput {
    /// Prefix wyświetlany na początku pierwszej widocznej linii.
    const PREFIX: &'static str = "> ";

    /// Szerokość prefiksu w kolumnach ekranu.
    const PREFIX_WIDTH: u16 = 2;

    /// Tworzy widget z domyślnym theme (`DEFAULT_THEME`) i `focused=true`.
    pub fn new(state: MultilineTextInputState) -> Self {
        Self {
            state,
            theme: DEFAULT_THEME,
            focused: true,
            placeholder: None,
        }
    }

    /// Ustawia stan focused widgetu (domyślnie `true`).
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Ustawia theme widgetu (domyślnie `DEFAULT_THEME`).
    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// Ustawia placeholder wyświetlany gdy bufor jest pusty.
    pub fn with_placeholder(mut self, placeholder: String) -> Self {
        self.placeholder = Some(placeholder);
        self
    }
}

impl Widget for MultilineTextInput {
    fn render(mut self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let prefix_style = self.theme.primary_style();

        // Content width: odejmujemy 2 kolumny zajmowane przez prefix
        let content_width = area.width.saturating_sub(Self::PREFIX_WIDTH) as usize;

        // Zaktualizuj wrap_width i viewport_height w stanie — clamp_scroll() wywoływany wewnętrznie
        self.state.set_wrap_width(content_width);
        self.state.set_viewport_height(area.height as usize);

        // Pusty bufor — renderuj prefix + kursor + placeholder (gdy ustawiony)
        if self.state.buffer().is_empty() {
            buf.set_string(area.x, area.y, Self::PREFIX, prefix_style);
            let cx = area.x + Self::PREFIX_WIDTH;
            if cx < area.x + area.width {
                if self.focused {
                    // Kursor blokowy, placeholder jako muted italic za kursorem
                    buf.set_string(cx, area.y, "█", self.theme.primary_style());
                    if let Some(ref ph) = self.placeholder {
                        let rest: String = ph.chars().skip(1).collect();
                        if !rest.is_empty() {
                            let px = cx + 1;
                            if px < area.x + area.width {
                                buf.set_string(
                                    px,
                                    area.y,
                                    &rest,
                                    self.theme.muted_style().add_modifier(Modifier::ITALIC),
                                );
                            }
                        }
                    }
                } else if let Some(ref ph) = self.placeholder {
                    // Unfocused — pokaż cały placeholder bez kursora
                    buf.set_string(
                        cx,
                        area.y,
                        ph.as_str(),
                        self.theme.muted_style().add_modifier(Modifier::ITALIC),
                    );
                }
            }
            return;
        }

        let cursor_vl_idx = self.state.cursor_visual_line_index().unwrap_or(0);
        let scroll_offset = self.state.scroll_offset();
        let (cursor_row, cursor_col) = self.state.cursor();
        let visible = self.state.visible_lines_viewport();

        // Kopiujemy buffer content — unikamy konfliktu borrows na state
        let buffer_content = self.state.buffer().to_string();
        let logical_lines: Vec<&str> = buffer_content.split('\n').collect();

        for (screen_y_idx, vl) in visible.iter().enumerate() {
            let y = area.y + screen_y_idx as u16;
            if y >= area.y + area.height {
                break;
            }

            // Pierwsza widoczna linia: prefix "> ", kolejne: wcięcie "  "
            let line_prefix = if screen_y_idx == 0 {
                Self::PREFIX
            } else {
                "  "
            };
            buf.set_string(area.x, y, line_prefix, prefix_style);

            let content_x = area.x + Self::PREFIX_WIDTH;
            let logical_line = logical_lines.get(vl.logical_row).copied().unwrap_or("");
            let global_vl_idx = scroll_offset + screen_y_idx;

            // Kursor jest na tej visual line gdy globalny indeks się zgadza i widget focused.
            // Przekazujemy None gdy !focused — render_line_chars nie renderuje wtedy kursora.
            let cursor_col_in_fragment =
                if self.focused && global_vl_idx == cursor_vl_idx && vl.logical_row == cursor_row {
                    cursor_col.checked_sub(vl.char_offset)
                } else {
                    None
                };

            // Renderuj znaki visual line
            let chars: Vec<char> = logical_line
                .chars()
                .skip(vl.char_offset)
                .take(vl.char_count)
                .collect();
            render_line_chars(
                buf,
                content_x,
                area.x + area.width,
                y,
                &chars,
                cursor_col_in_fragment,
                self.theme.primary_style(),
            );
        }
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

    // ── Scroll / viewport ────────────────────────────────────────────

    #[test]
    fn test_scroll_initial_zero() {
        let state = MultilineTextInputState::with_content("abc\ndef");
        assert_eq!(state.scroll_offset(), 0);
        assert_eq!(state.viewport_height(), 0);
    }

    #[test]
    fn test_clamp_scroll_noop_without_viewport() {
        let mut state = MultilineTextInputState::with_content("a\nb\nc\nd\ne");
        // viewport_height == 0 → clamp_scroll nie zmienia offset
        state.clamp_scroll();
        assert_eq!(state.scroll_offset(), 0);
    }

    #[test]
    fn test_set_viewport_height_triggers_clamp() {
        // Tworzymy bufor z 5 linii, ustawiamy viewport 2 — kursor na linii 4 (ostatnia)
        let mut state = MultilineTextInputState::with_content("a\nb\nc\nd\ne");
        // cursor at (4, 1) — ostatnia linia
        assert_eq!(state.cursor(), (4, 1));

        state.set_viewport_height(2);
        // Kursor musi być widoczny: scroll_offset = 4 + 1 - 2 = 3
        assert_eq!(state.scroll_offset(), 3);
    }

    #[test]
    fn test_scroll_follows_cursor_down() {
        // 10 linii, viewport 3, kursor startuje na linii 0
        let content = "0\n1\n2\n3\n4\n5\n6\n7\n8\n9";
        let mut state = MultilineTextInputState::with_content(content);
        // Przesuń na początek
        state.cursor_row = 0;
        state.cursor_col = 0;
        state.set_viewport_height(3);
        assert_eq!(state.scroll_offset(), 0);

        // Przesuń kursor w dół — po każdym kroku scroll powinien podążać
        state.move_cursor_down(); // row 1
        assert_eq!(state.scroll_offset(), 0); // nadal widoczny

        state.move_cursor_down(); // row 2
        assert_eq!(state.scroll_offset(), 0); // ostatnia widoczna w viewport 0-2

        state.move_cursor_down(); // row 3 → wychodzi poza viewport
        assert_eq!(state.scroll_offset(), 1); // scroll = 3 + 1 - 3 = 1

        state.move_cursor_down(); // row 4
        assert_eq!(state.scroll_offset(), 2);
    }

    #[test]
    fn test_scroll_follows_cursor_up() {
        let content = "0\n1\n2\n3\n4\n5";
        let mut state = MultilineTextInputState::with_content(content);
        // Cursor na końcu (row 5), viewport 3
        assert_eq!(state.cursor(), (5, 1));
        state.set_viewport_height(3);
        // scroll = 5 + 1 - 3 = 3
        assert_eq!(state.scroll_offset(), 3);

        state.move_cursor_up(); // row 4
        assert_eq!(state.scroll_offset(), 3); // nadal widoczny (3,4,5)

        state.move_cursor_up(); // row 3
        assert_eq!(state.scroll_offset(), 3); // nadal widoczny

        state.move_cursor_up(); // row 2 → wychodzi poza górę
        assert_eq!(state.scroll_offset(), 2); // scroll = 2

        state.move_cursor_up(); // row 1
        assert_eq!(state.scroll_offset(), 1);

        state.move_cursor_up(); // row 0
        assert_eq!(state.scroll_offset(), 0);
    }

    #[test]
    fn test_scroll_cursor_on_first_visible_line() {
        // Kursor na pierwszej widocznej linii — scroll nie zmienia się
        let content = "0\n1\n2\n3\n4";
        let mut state = MultilineTextInputState::with_content(content);
        state.cursor_row = 3;
        state.cursor_col = 0;
        state.set_viewport_height(3);
        // scroll = max(3 + 1 - 3, 0) = 1; viewport shows rows 1,2,3
        assert_eq!(state.scroll_offset(), 1);

        // Cursor at row 3 = last visible → scroll stays
        assert_eq!(state.scroll_offset(), 1);
    }

    #[test]
    fn test_scroll_cursor_on_last_visible_line() {
        let content = "0\n1\n2\n3\n4";
        let mut state = MultilineTextInputState::with_content(content);
        state.cursor_row = 2;
        state.cursor_col = 0;
        state.set_viewport_height(3);
        // cursor at 2, viewport 3 → scroll = 0 (widoczne 0,1,2)
        assert_eq!(state.scroll_offset(), 0);
    }

    #[test]
    fn test_visible_lines_basic() {
        let content = "a\nb\nc\nd\ne";
        let mut state = MultilineTextInputState::with_content(content);
        state.cursor_row = 0;
        state.cursor_col = 0;
        state.set_viewport_height(3);
        assert_eq!(state.scroll_offset(), 0);

        let visible = state.visible_lines(3);
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].logical_row, 0);
        assert_eq!(visible[1].logical_row, 1);
        assert_eq!(visible[2].logical_row, 2);
    }

    #[test]
    fn test_visible_lines_after_scroll() {
        let content = "0\n1\n2\n3\n4";
        let mut state = MultilineTextInputState::with_content(content);
        // cursor at (4, 1) — ostatnia linia
        state.set_viewport_height(3);
        // scroll = 4 + 1 - 3 = 2
        assert_eq!(state.scroll_offset(), 2);

        let visible = state.visible_lines(3);
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].logical_row, 2);
        assert_eq!(visible[1].logical_row, 3);
        assert_eq!(visible[2].logical_row, 4);
    }

    #[test]
    fn test_visible_lines_height_zero() {
        let state = MultilineTextInputState::with_content("abc");
        let visible = state.visible_lines(0);
        assert!(visible.is_empty());
    }

    #[test]
    fn test_visible_lines_height_exceeds_content() {
        let content = "a\nb";
        let state = MultilineTextInputState::with_content(content);
        let visible = state.visible_lines(10);
        // Tylko 2 linie
        assert_eq!(visible.len(), 2);
    }

    #[test]
    fn test_insert_char_scrolls_into_view() {
        // Wpisujemy znaki do nowej linii poza viewportem — scroll podąża
        let content = "0\n1\n2\n3\n4";
        let mut state = MultilineTextInputState::with_content(content);
        // cursor at (4, 1)
        state.set_viewport_height(2);
        // scroll = 4 + 1 - 2 = 3; viewport shows rows 3,4
        assert_eq!(state.scroll_offset(), 3);

        state.insert_char('X');
        // Cursor at (4, 2), scroll pozostaje 3
        assert_eq!(state.scroll_offset(), 3);
    }

    #[test]
    fn test_insert_newline_scrolls_into_view() {
        let content = "0\n1\n2";
        let mut state = MultilineTextInputState::with_content(content);
        state.set_viewport_height(2);
        // cursor at (2, 1), scroll = 2+1-2 = 1; viewport shows rows 1,2
        assert_eq!(state.scroll_offset(), 1);

        state.insert_newline();
        // Nowa linia 3 — cursor at (3, 0)
        // scroll = 3+1-2 = 2; viewport shows rows 2,3
        assert_eq!(state.scroll_offset(), 2);
    }

    #[test]
    fn test_scroll_with_visual_wrap() {
        // Jedna długa linia wrapping 5: "abcdeXXXXX" → 2 visual rows
        // Viewport 1 → scroll powinien podążać za kursorem w visual rows
        let mut state = MultilineTextInputState::with_content("abcdeXXXXX");
        state.set_wrap_width(5);
        state.set_viewport_height(1);

        // cursor at (0, 10) — końcu linii, w visual row 1
        // find_visual_line_index powinien dać 1, scroll = 1+1-1 = 1
        assert_eq!(state.scroll_offset(), 1);

        // Przesuń kursor na początek linii (visual row 0)
        state.move_cursor_home();
        // cursor at (0, 0) — visual row 0
        assert_eq!(state.scroll_offset(), 0);
    }

    #[test]
    fn test_clear_resets_scroll() {
        let content = "0\n1\n2\n3\n4";
        let mut state = MultilineTextInputState::with_content(content);
        state.set_viewport_height(2);
        assert!(state.scroll_offset() > 0); // scroll był przesunięty

        state.clear();
        assert_eq!(state.scroll_offset(), 0);
        assert_eq!(state.cursor(), (0, 0));
    }

    // ── cursor_visual_line_index ─────────────────────────────────────

    #[test]
    fn test_cursor_visual_line_index_single_line() {
        let state = MultilineTextInputState::with_content("abc");
        // 1 visual line (no wrap) → cursor at row 0 → idx 0
        assert_eq!(state.cursor_visual_line_index(), Some(0));
    }

    #[test]
    fn test_cursor_visual_line_index_multiline() {
        let state = MultilineTextInputState::with_content("a\nb\nc");
        // cursor at (2, 1) — ostatnia linia → visual line idx 2
        assert_eq!(state.cursor_visual_line_index(), Some(2));
    }

    #[test]
    fn test_cursor_visual_line_index_with_wrap() {
        let mut state = MultilineTextInputState::with_content("abcdeXXXXX");
        state.set_wrap_width(5);
        // cursor at (0, 10) — końiec linii, visual row 1
        assert_eq!(state.cursor_visual_line_index(), Some(1));
    }

    // ── visible_lines_viewport ───────────────────────────────────────

    #[test]
    fn test_visible_lines_viewport_same_as_visible_lines() {
        let content = "a\nb\nc\nd\ne";
        let mut state = MultilineTextInputState::with_content(content);
        state.cursor_row = 0;
        state.cursor_col = 0;
        state.set_viewport_height(3);

        // visible_lines_viewport() powinno zwrócić to samo co visible_lines(3)
        let via_viewport = state.visible_lines_viewport();
        let via_arg = state.visible_lines(3);
        assert_eq!(via_viewport.len(), via_arg.len());
        for (a, b) in via_viewport.iter().zip(via_arg.iter()) {
            assert_eq!(a.logical_row, b.logical_row);
            assert_eq!(a.char_offset, b.char_offset);
            assert_eq!(a.char_count, b.char_count);
        }
    }

    #[test]
    fn test_visible_lines_viewport_zero_height() {
        let state = MultilineTextInputState::with_content("abc");
        // viewport_height = 0 (domyślne) → pusta lista
        let visible = state.visible_lines_viewport();
        assert!(visible.is_empty());
    }

    // ── MultilineTextInputWidget ─────────────────────────────────────

    #[test]
    fn test_widget_updates_viewport_on_render() {
        use ratatui::{backend::TestBackend, prelude::Terminal};

        use crate::tui::Theme;

        let theme = Theme::default();
        let mut state = MultilineTextInputState::new();
        assert_eq!(state.viewport_height(), 0);
        assert_eq!(state.wrap_width(), 0);

        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 20, 5);
                let widget = MultilineTextInputWidget::new(&theme, true);
                frame.render_stateful_widget(widget, area, &mut state);
            })
            .expect("draw");

        // Widget musi zaktualizować viewport_height i wrap_width ze state
        assert_eq!(state.viewport_height(), 5);
        assert_eq!(state.wrap_width(), 20);
    }

    #[test]
    fn test_widget_renders_content() {
        use ratatui::{backend::TestBackend, prelude::Terminal};

        use crate::tui::Theme;

        let theme = Theme::default();
        let mut state = MultilineTextInputState::with_content("hello");
        // Przesuń kursor na początek (żeby nie było kursora na pierwszych znakach)
        state.cursor_col = 0;

        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 20, 3);
                let widget = MultilineTextInputWidget::new(&theme, false);
                frame.render_stateful_widget(widget, area, &mut state);
            })
            .expect("draw");

        let buf = terminal.backend().buffer();
        // Sprawdź że tekst "hello" jest w wierszu 0
        let row: String = (0..5)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert_eq!(row, "hello");
    }

    #[test]
    fn test_widget_scroll_shows_visible_lines_only() {
        use ratatui::{backend::TestBackend, prelude::Terminal};

        use crate::tui::Theme;

        let theme = Theme::default();
        // 5 linii, cursor na ostatniej
        let mut state = MultilineTextInputState::with_content("a\nb\nc\nd\ne");
        // cursor at (4, 1) — koniec "e"

        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 20, 3);
                let widget = MultilineTextInputWidget::new(&theme, false);
                frame.render_stateful_widget(widget, area, &mut state);
            })
            .expect("draw");

        // scroll_offset = 4 + 1 - 3 = 2 → widoczne linie: c(row2), d(row3), e(row4)
        assert_eq!(state.scroll_offset(), 2);

        let buf = terminal.backend().buffer();
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "c");
        assert_eq!(buf.cell((0, 1)).unwrap().symbol(), "d");
        assert_eq!(buf.cell((0, 2)).unwrap().symbol(), "e");
    }

    #[test]
    fn test_widget_cursor_uses_reversed_style() {
        use ratatui::{backend::TestBackend, prelude::Terminal, style::Modifier};

        use crate::tui::Theme;

        let theme = Theme::default();
        // Kursor na znaku 'b' (col=1)
        let mut state = MultilineTextInputState::with_content("abc");
        state.cursor_col = 1; // kursor na 'b'

        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 20, 3);
                let widget = MultilineTextInputWidget::new(&theme, true); // focused!
                frame.render_stateful_widget(widget, area, &mut state);
            })
            .expect("draw");

        let buf = terminal.backend().buffer();
        // 'a' — brak kursora
        assert!(
            !buf.cell((0, 0))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED)
        );
        // 'b' — kursor (REVERSED)
        assert_eq!(buf.cell((1, 0)).unwrap().symbol(), "b");
        assert!(
            buf.cell((1, 0))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED)
        );
        // 'c' — brak kursora
        assert!(
            !buf.cell((2, 0))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn test_widget_empty_focused_shows_cursor() {
        use ratatui::{backend::TestBackend, prelude::Terminal};

        use crate::tui::Theme;

        let theme = Theme::default();
        let mut state = MultilineTextInputState::new();

        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 20, 3);
                let widget = MultilineTextInputWidget::new(&theme, true);
                frame.render_stateful_widget(widget, area, &mut state);
            })
            .expect("draw");

        let buf = terminal.backend().buffer();
        // Pusty bufor + focused → kursor blokowy ("█")
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "█");
    }

    #[test]
    fn test_widget_empty_unfocused_no_content() {
        use ratatui::{backend::TestBackend, prelude::Terminal};

        use crate::tui::Theme;

        let theme = Theme::default();
        let mut state = MultilineTextInputState::new();

        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 20, 3);
                let widget = MultilineTextInputWidget::new(&theme, false); // unfocused, no placeholder
                frame.render_stateful_widget(widget, area, &mut state);
            })
            .expect("draw");

        let buf = terminal.backend().buffer();
        // Pusty bufor, unfocused, brak placeholder → puste komórki
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), " ");
    }

    #[test]
    fn test_widget_zero_area_no_panic() {
        use ratatui::{backend::TestBackend, prelude::Terminal};

        use crate::tui::Theme;

        let theme = Theme::default();
        let mut state = MultilineTextInputState::with_content("hello");

        let backend = TestBackend::new(1, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        // Nie powinno panikować przy minimalnym/zerowym area
        terminal
            .draw(|frame| {
                let widget = MultilineTextInputWidget::new(&theme, true);
                frame.render_stateful_widget(widget, Rect::new(0, 0, 0, 0), &mut state);
            })
            .expect("draw");
    }

    // ── MultilineTextInput (Widget) snapshot tests ────────────────────

    /// Helper do renderowania MultilineTextInput do bufora testowego.
    fn render_multiline_input(widget: MultilineTextInput, width: u16, height: u16) -> String {
        use ratatui::{backend::TestBackend, prelude::Terminal};

        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, height);
                frame.render_widget(widget, area);
            })
            .expect("draw");
        crate::test_helpers::snap(terminal.backend().buffer())
    }

    #[test]
    fn snapshot_multiline_input_empty_focused() {
        // Pusty bufor, focused — kursor blokowy za prefixem
        let state = MultilineTextInputState::new();
        let widget = MultilineTextInput::new(state);
        let snapshot = render_multiline_input(widget, 20, 3);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_multiline_input_empty_unfocused() {
        // Pusty bufor, unfocused — tylko prefix, brak kursora
        let state = MultilineTextInputState::new();
        let widget = MultilineTextInput::new(state).focused(false);
        let snapshot = render_multiline_input(widget, 20, 3);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_multiline_input_single_line_cursor_at_end() {
        // Jedna linia tekstu, kursor na końcu
        let state = MultilineTextInputState::with_content("hello");
        let widget = MultilineTextInput::new(state);
        let snapshot = render_multiline_input(widget, 20, 3);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_multiline_input_cursor_in_middle() {
        // Kursor w środku linii ("hello" col=2 → na 'l')
        let mut state = MultilineTextInputState::with_content("hello");
        state.cursor_col = 2;
        let widget = MultilineTextInput::new(state);
        let snapshot = render_multiline_input(widget, 20, 3);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_multiline_input_multiline() {
        // Dwie logiczne linie, kursor na końcu drugiej
        let state = MultilineTextInputState::with_content("first line\nsecond line");
        let widget = MultilineTextInput::new(state);
        let snapshot = render_multiline_input(widget, 20, 3);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_multiline_input_visual_wrap() {
        // Linia dłuższa niż content_width (20-2=18) — wymusza visual wrap
        let state = MultilineTextInputState::with_content("this is a long wrap line!");
        let widget = MultilineTextInput::new(state);
        let snapshot = render_multiline_input(widget, 20, 3);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_multiline_input_min_width() {
        // width=2 → content_width=0 → prefiks zajmuje całą szerokość, brak miejsca na tekst
        let state = MultilineTextInputState::with_content("hello");
        let widget = MultilineTextInput::new(state);
        let snapshot = render_multiline_input(widget, 2, 3);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_multiline_input_narrow_width() {
        // width=5 → content_width=3 → jeden/dwa znaki na visual line
        let state = MultilineTextInputState::with_content("hello");
        let widget = MultilineTextInput::new(state);
        let snapshot = render_multiline_input(widget, 5, 5);
        insta::assert_snapshot!(snapshot);
    }

    /// Test buffer-level: weryfikuje REVERSED modifier na znaku pod kursorem
    /// dla MultilineTextInput (Widget z prefixem).
    ///
    /// `test_widget_cursor_uses_reversed_style` pokrywa ten styl dla StatefulWidget.
    /// Ten test robi to samo dla Widget (z prefixem `> `).
    #[test]
    fn test_multiline_input_cursor_uses_reversed_style() {
        use ratatui::{backend::TestBackend, prelude::Terminal, style::Modifier};

        // "hello", kursor na col=2 → na znaku 'l' (0-indexed: h=0, e=1, l=2)
        let mut state = MultilineTextInputState::with_content("hello");
        state.cursor_col = 2;

        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 20, 3);
                let widget = MultilineTextInput::new(state);
                frame.render_widget(widget, area);
            })
            .expect("draw");

        let buf = terminal.backend().buffer();
        // Prefix "> " zajmuje kolumny x=0 i x=1.
        // Znaki "hello" zaczynają się od x=2: h=2, e=3, l=4, l=5, o=6.
        // cursor_col=2 → kursor na indeksie 2 w fragmencie → znak 'l' na x=4.
        assert_eq!(buf.cell((4, 0)).unwrap().symbol(), "l");
        assert!(
            buf.cell((4, 0))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED),
            "znak pod kursorem powinien mieć REVERSED modifier"
        );
        // Sąsiednie znaki nie mają REVERSED
        assert!(
            !buf.cell((3, 0))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED),
            "'e' przed kursorem nie powinno mieć REVERSED"
        );
        assert!(
            !buf.cell((5, 0))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED),
            "'l' za kursorem nie powinno mieć REVERSED"
        );
    }
}
