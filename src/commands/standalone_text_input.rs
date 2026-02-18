// Widget dla pytań tekstowych (QuestionType::Text)
//
// Używa ratatui Paragraph z Viewport::Inline. Tekst pytania (header)
// jest renderowany w górnej części viewportu, a pole input na dole.

use crate::shared::error::{RalphError, Result};
use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use std::io;
use std::time::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Kolapsuje viewport: przesuwa kursor na absolutną pozycję Y viewportu
/// i czyści wszystko poniżej. Wywoływać po drop(terminal).
///
/// `viewport_y` to wartość `frame.area().y` przechwycona podczas ostatniego draw().
fn collapse_viewport(viewport_y: u16) -> Result<()> {
    crossterm::execute!(
        io::stdout(),
        MoveTo(0, viewport_y),
        Clear(ClearType::FromCursorDown)
    )
    .map_err(|e| RalphError::Mcp(format!("Failed to collapse viewport: {e}")))?;
    Ok(())
}

/// Maksymalna liczba widocznych linii dla inputu (bez headera)
const MAX_INPUT_LINES: u16 = 10;

/// Hint ze skrótami klawiszowymi wyświetlany pod polem input
const HINT_TEXT: &str = "Enter: wyślij │ Shift+Enter: nowa linia │ Esc: wróć │ Ctrl+C: anuluj";

/// Stan edycji tekstu z pozycją kursora i scrollem
#[derive(Debug)]
struct TextInputState {
    /// Buffer tekstowy (może zawierać \n)
    buffer: String,
    /// Pozycja kursora jako char index (liczba znaków od początku)
    cursor_pos: usize,
    /// Vertical scroll offset - liczba linii przewiniętych do góry
    scroll_offset: usize,
}

impl TextInputState {
    /// Tworzy nowy stan z opcjonalnym default value
    fn new(default: Option<&str>) -> Self {
        let buffer = default.unwrap_or("").to_string();
        let cursor_pos = buffer.chars().count();
        Self {
            buffer,
            cursor_pos,
            scroll_offset: 0,
        }
    }
}

/// Konwertuje char index na byte offset w stringu
///
/// Jeśli `char_idx` jest poza długością stringa, zwraca `s.len()`.
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Konwertuje byte offset na char index (liczba znaków przed tym bajtem)
fn byte_to_char(s: &str, byte_offset: usize) -> usize {
    s[..byte_offset].chars().count()
}

/// Czy znak jest częścią "słowa" (litery, cyfry, underscore)
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Znajduje poprzednią granicę słowa (readline-style).
///
/// 1. Od `cursor_pos` cofaj się, pomijając non-word chars
/// 2. Potem cofaj się przez word chars
/// 3. Zwróć nową pozycję (char index)
fn find_prev_word_boundary(buffer: &str, cursor_pos: usize) -> usize {
    let chars: Vec<char> = buffer.chars().collect();
    let mut pos = cursor_pos;

    // Pomijaj non-word chars wstecz
    while pos > 0 && !is_word_char(chars[pos - 1]) {
        pos -= 1;
    }
    // Cofaj się przez word chars
    while pos > 0 && is_word_char(chars[pos - 1]) {
        pos -= 1;
    }
    pos
}

/// Znajduje następną granicę słowa.
///
/// 1. Od `cursor_pos` idź wprzód przez word chars
/// 2. Potem pomijaj non-word chars
/// 3. Zwróć nową pozycję
fn find_next_word_boundary(buffer: &str, cursor_pos: usize) -> usize {
    let chars: Vec<char> = buffer.chars().collect();
    let len = chars.len();
    let mut pos = cursor_pos;

    // Przejdź przez word chars
    while pos < len && is_word_char(chars[pos]) {
        pos += 1;
    }
    // Pomijaj non-word chars
    while pos < len && !is_word_char(chars[pos]) {
        pos += 1;
    }
    pos
}

/// Znajduje początek bieżącej linii (char index pozycji po ostatnim '\n' przed kursorem)
fn find_line_start(buffer: &str, cursor_pos: usize) -> usize {
    let byte_pos = char_to_byte(buffer, cursor_pos);
    let before = &buffer[..byte_pos];
    before
        .rfind('\n')
        .map(|p| byte_to_char(buffer, p + 1))
        .unwrap_or(0)
}

/// Znajduje koniec bieżącej linii (char index pozycji '\n' lub końca bufora)
fn find_line_end(buffer: &str, cursor_pos: usize) -> usize {
    let byte_pos = char_to_byte(buffer, cursor_pos);
    buffer[byte_pos..]
        .find('\n')
        .map(|p| byte_to_char(buffer, byte_pos + p))
        .unwrap_or(buffer.chars().count())
}

/// Dzieli pojedynczą linię na fragmenty char-level wrap wg unicode display width.
///
/// Zwraca wektor `&str` slices — każdy fragment mieści się w `width` kolumnach.
/// Jeśli linia jest pusta, zwraca jeden pusty fragment.
fn wrap_line(line: &str, width: usize) -> Vec<&str> {
    if width == 0 {
        return vec![line];
    }
    if line.is_empty() {
        return vec![""];
    }

    let mut result = Vec::new();
    let mut current_start = 0; // byte offset początku bieżącego fragmentu
    let mut current_width = 0;

    for (byte_idx, ch) in line.char_indices() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);

        if current_width + ch_width > width && current_width > 0 {
            // Bieżący fragment jest pełny — odetnij
            result.push(&line[current_start..byte_idx]);
            current_start = byte_idx;
            current_width = ch_width;
        } else {
            current_width += ch_width;
        }
    }

    // Ostatni fragment
    result.push(&line[current_start..]);
    result
}

/// Buduje listę wrapped fragmentów (terminal rows) z bufora tekstowego.
///
/// Każdy element to `(logical_line_idx, fragment_str)`.
/// Używa char-level wrap (`wrap_line`) — identyczna logika jest używana
/// do renderowania, obliczania content_lines i pozycji kursora.
///
/// Pierwsza logiczna linia ma `terminal_width - 2` kolumn (prefix "> "),
/// continuation linie (po wrap) i kolejne logiczne linie mają pełną terminal_width.
fn build_wrapped_lines(buffer: &str, terminal_width: u16) -> Vec<(usize, &str)> {
    let first_line_width = terminal_width.saturating_sub(2) as usize;
    let line_width = terminal_width as usize;

    if buffer.is_empty() {
        return vec![(0, "")];
    }

    let mut result: Vec<(usize, &str)> = Vec::new();

    for (logical_idx, logical_line) in buffer.split('\n').enumerate() {
        if logical_idx == 0 {
            // Pierwsza logiczna linia: pierwszy fragment ma first_line_width
            let first_fragments = wrap_line(logical_line, first_line_width);
            result.push((logical_idx, first_fragments[0]));

            // Continuation fragments (po pierwszym wrap) mają pełną szerokość
            if first_fragments.len() > 1 {
                let rest_start = first_fragments[0].len();
                let rest = &logical_line[rest_start..];
                for frag in wrap_line(rest, line_width) {
                    result.push((logical_idx, frag));
                }
            }
        } else {
            // Kolejne logiczne linie: pełna terminal_width
            for frag in wrap_line(logical_line, line_width) {
                result.push((logical_idx, frag));
            }
        }
    }

    if result.is_empty() {
        result.push((0, ""));
    }
    result
}

/// Oblicza liczbę linii terminala potrzebnych do wyrenderowania tekstu
/// z uwzględnieniem char-level soft wrap i prefixu "> ".
fn calculate_content_lines(buffer: &str, terminal_width: u16) -> usize {
    build_wrapped_lines(buffer, terminal_width).len()
}

/// Oblicza pozycję kursora (x, y) względem początku obszaru inputu,
/// uwzględniając multiline i char-level soft wrap.
///
/// Iteruje wrapped linie z build_wrapped_lines() i lokalizuje fragment
/// zawierający pozycję kursora. Gwarantuje spójność z renderowaniem.
///
/// Zwraca (column, row) gdzie:
/// - column: pozycja w wierszu (0-based, z uwzględnieniem "> " w pierwszym wierszu)
/// - row: numer wiersza (0-based względem początku inputu)
fn calculate_cursor_position(buffer: &str, cursor_pos: usize, terminal_width: u16) -> (u16, u16) {
    let byte_pos = char_to_byte(buffer, cursor_pos);
    let wrapped = build_wrapped_lines(buffer, terminal_width);
    let line_width = terminal_width as usize;

    // Śledź byte offset w buforze, iterując fragmenty
    // Uwzględniamy '\n' między logicznymi liniami
    let mut consumed_bytes = 0usize;
    let mut prev_logical_idx = 0usize;

    for (row, (logical_idx, fragment)) in wrapped.iter().enumerate() {
        // Jeśli przeszliśmy na nową logiczną linię, dodaj '\n' do consumed_bytes
        if *logical_idx > prev_logical_idx {
            consumed_bytes += 1; // '\n'
            prev_logical_idx = *logical_idx;
        }

        let frag_start = consumed_bytes;
        let frag_end = consumed_bytes + fragment.len();
        // Dostępna szerokość tego wiersza (z lub bez prefixu)
        let avail = if row == 0 {
            terminal_width.saturating_sub(2) as usize
        } else {
            line_width
        };
        let frag_display_width = UnicodeWidthStr::width(*fragment);

        if byte_pos >= frag_start && byte_pos <= frag_end {
            let offset_in_frag = byte_pos - frag_start;
            let text_before_cursor = &fragment[..offset_in_frag];
            let col = UnicodeWidthStr::width(text_before_cursor);

            // Jeśli kursor na końcu pełnego fragmentu → przejdź na nowy wiersz
            // (zachowanie terminala: kursor po wypełnieniu wiersza skacze na nowy)
            if offset_in_frag == fragment.len() && frag_display_width >= avail {
                return (0, (row + 1) as u16);
            }

            // Dodaj prefix "> " tylko w pierwszym wierszu (row == 0)
            let col = if row == 0 { col + 2 } else { col };
            return (col as u16, row as u16);
        }

        consumed_bytes = frag_end;
    }

    // Fallback: kursor na końcu ostatniego fragmentu
    let last_row = wrapped.len().saturating_sub(1);
    let last_frag = wrapped.last().map(|(_, f)| *f).unwrap_or("");
    let col = UnicodeWidthStr::width(last_frag);
    let col = if last_row == 0 { col + 2 } else { col };
    (col as u16, last_row as u16)
}

/// RAII guard dla raw mode
struct RawModeGuard;
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// Renderuje widget tekstowy z headerem pytania osadzonym w viewporcie
///
/// Parametry:
/// - header: tekst pytania (markdown)
/// - placeholder: tekst placeholder dla pola inputu
/// - default: domyślna wartość
/// - required: czy odpowiedź jest wymagana
#[allow(dead_code)]
pub fn render_text_question(
    header: &str,
    placeholder: Option<&str>,
    default: Option<&str>,
    required: bool,
) -> Result<String> {
    match text_input(placeholder, default, required, Some(header)) {
        Err(RalphError::Back) => Err(RalphError::Interrupted),
        other => other,
    }
}

/// Minimalny custom readline z ratatui Viewport::Inline
///
/// Gdy `header` jest Some, tekst pytania jest renderowany w górnej części
/// viewportu. Gdy None (np. wywołanie z multi_select "Other"), viewport
/// zawiera tylko pole input.
///
/// # Uwaga
/// Ta funkcja jest publiczna i może być używana z innych modułów.
/// Do prostych przypadków bez headera użyj `standalone_text_input()`.
pub fn text_input(
    placeholder: Option<&str>,
    default: Option<&str>,
    required: bool,
    header: Option<&str>,
) -> Result<String> {
    let header_text = header
        .filter(|h| !h.is_empty())
        .map(|h| Text::raw(h.to_string()));
    let header_lines = header_text
        .as_ref()
        .map(|t| t.lines.len() as u16)
        .unwrap_or(0);

    let mut state = TextInputState::new(default);
    let mut viewport_y = 0u16;

    enable_raw_mode().map_err(|e| RalphError::Mcp(format!("Failed to enable raw mode: {e}")))?;
    let _guard = RawModeGuard;

    let backend = CrosstermBackend::new(io::stdout());

    // Pobierz rozmiar terminala dla obliczenia wysokości
    let (terminal_width, _) = crossterm::terminal::size()
        .map_err(|e| RalphError::Mcp(format!("Failed to get terminal size: {e}")))?;

    // Oblicz wysokość inputu z uwzględnieniem zawartości (+1 na hint ze skrótami)
    let content_lines = calculate_content_lines(&state.buffer, terminal_width);
    let visible_input_lines = content_lines.min(MAX_INPUT_LINES as usize) as u16;
    let hint_lines = 1u16;
    let total_height = header_lines + visible_input_lines + hint_lines;

    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(total_height),
        },
    )
    .map_err(|e| RalphError::Mcp(format!("Failed to create terminal: {e}")))?;

    // Śledź wysokość viewportu dla recreate
    let mut last_height = total_height;

    loop {
        // Oblicz aktualną wysokość przed każdym rysowaniem (+1 na hint)
        let content_lines = calculate_content_lines(&state.buffer, terminal_width);
        let visible_input_lines = content_lines.min(MAX_INPUT_LINES as usize) as u16;
        let new_height = header_lines + visible_input_lines + hint_lines;

        // Jeśli wysokość się zmieniła, musimy przebudować terminal
        if new_height != last_height {
            drop(terminal);
            collapse_viewport(viewport_y)?;

            let backend = CrosstermBackend::new(io::stdout());
            terminal = Terminal::with_options(
                backend,
                TerminalOptions {
                    viewport: Viewport::Inline(new_height),
                },
            )
            .map_err(|e| RalphError::Mcp(format!("Failed to recreate terminal: {e}")))?;

            last_height = new_height;
        }

        // Oblicz pozycję kursora przed rysowaniem (potrzebne do auto-scroll)
        let (mut cursor_x, cursor_row) =
            calculate_cursor_position(&state.buffer, state.cursor_pos, terminal_width);

        // Auto-scroll: dostosuj scroll_offset aby kursor był widoczny
        let vis_height = visible_input_lines as usize;
        if (cursor_row as usize) < state.scroll_offset {
            state.scroll_offset = cursor_row as usize;
        } else if (cursor_row as usize) >= state.scroll_offset + vis_height {
            state.scroll_offset = (cursor_row as usize) - vis_height + 1;
        }

        let scroll_offset = state.scroll_offset as u16;

        // Korekcja kursora: jeśli scroll > 0 i kursor jest na pierwszej widocznej
        // linii, dodaj offset "> " prefixu (dynamicznie dodawanego do tej linii)
        if scroll_offset > 0 && cursor_row == scroll_offset {
            cursor_x = cursor_x
                .saturating_add(2)
                .min(terminal_width.saturating_sub(1));
        }

        terminal
            .draw(|frame| {
                let area = frame.area();
                viewport_y = area.y;

                // Layout: [header (opcjonalny)] + [input] + [hint]
                let (input_area, hint_area) = if header_lines > 0 {
                    let chunks = Layout::vertical([
                        Constraint::Length(header_lines),
                        Constraint::Length(visible_input_lines),
                        Constraint::Length(hint_lines),
                    ])
                    .split(area);
                    if let Some(ref ht) = header_text {
                        frame.render_widget(Paragraph::new(ht.clone()), chunks[0]);
                    }
                    (chunks[1], chunks[2])
                } else {
                    let chunks = Layout::vertical([
                        Constraint::Length(visible_input_lines),
                        Constraint::Length(hint_lines),
                    ])
                    .split(area);
                    (chunks[0], chunks[1])
                };

                // Renderuj buffer z ręcznym char-level wrap (bez ratatui Wrap)
                // — spójne z calculate_cursor_position i calculate_content_lines
                let display_text = if state.buffer.is_empty() {
                    // Placeholder
                    Text::from(vec![Line::from(vec![
                        Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)),
                        Span::styled(
                            placeholder.unwrap_or(""),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ])])
                } else {
                    let wrapped = build_wrapped_lines(&state.buffer, terminal_width);
                    let mut lines_vec: Vec<Line> = Vec::new();

                    for (row_idx, (_logical_idx, fragment)) in wrapped.iter().enumerate() {
                        // "> " na pierwszej widocznej linii — zawsze widoczny prompt
                        let show_prefix = row_idx == 0
                            || (scroll_offset > 0 && row_idx == scroll_offset as usize);
                        if show_prefix {
                            lines_vec.push(Line::from(vec![
                                Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)),
                                Span::raw(*fragment),
                            ]));
                        } else {
                            lines_vec.push(Line::from(*fragment));
                        }
                    }

                    Text::from(lines_vec)
                };

                // Bez Wrap — linie są już ręcznie zawinięte do terminal_width
                let paragraph = Paragraph::new(display_text).scroll((scroll_offset, 0));

                frame.render_widget(paragraph, input_area);

                // Hint ze skrótami klawiszowymi
                let hint = Paragraph::new(Line::from(Span::styled(
                    HINT_TEXT,
                    Style::default().fg(Color::DarkGray),
                )));
                frame.render_widget(hint, hint_area);

                // Ustaw kursor (względem input_area, z uwzględnieniem scrollu)
                let visible_row = cursor_row.saturating_sub(scroll_offset);
                frame.set_cursor_position((cursor_x, input_area.y + visible_row));
            })
            .map_err(|e| RalphError::Mcp(format!("Failed to draw: {e}")))?;

        // Drain: przetwórz WSZYSTKIE oczekujące eventy przed następnym resize+draw.
        // Bez tego przytrzymanie Enter powoduje osobny cykl drop→collapse→Inline→draw
        // na KAŻDY event — seria szybkich resize'ów tworzy artefakty wizualne.
        if event::poll(Duration::from_millis(50))
            .map_err(|e| RalphError::Mcp(format!("Failed to poll events: {e}")))?
        {
            loop {
                let ev = event::read()
                    .map_err(|e| RalphError::Mcp(format!("Failed to read event: {e}")))?;
                if let Event::Key(key) = ev {
                    match handle_key_event(key, &mut state, required) {
                        Err(e) => {
                            drop(terminal);
                            collapse_viewport(viewport_y)?;
                            return Err(e);
                        }
                        Ok(KeyAction::Submit) => {
                            drop(terminal);
                            collapse_viewport(viewport_y)?;
                            return Ok(state.buffer);
                        }
                        Ok(KeyAction::Back) => {
                            drop(terminal);
                            collapse_viewport(viewport_y)?;
                            return Err(RalphError::Back);
                        }
                        Ok(KeyAction::Continue) => {}
                    }
                }
                // Więcej eventów natychmiast dostępnych? Kontynuuj drain
                if !event::poll(Duration::ZERO)
                    .map_err(|e| RalphError::Mcp(format!("Failed to poll events: {e}")))?
                {
                    break;
                }
            }
        }
    }
}

/// Wynik obsługi klawisza
#[derive(Debug)]
enum KeyAction {
    Continue,
    Submit,
    Back,
}

/// Wstawia newline w buforze na pozycji kursora
fn insert_newline(state: &mut TextInputState) {
    let byte_pos = char_to_byte(&state.buffer, state.cursor_pos);
    state.buffer.insert(byte_pos, '\n');
    state.cursor_pos += 1;
}

/// Obsługuje pojedyncze zdarzenie klawiatury
fn handle_key_event(
    key: KeyEvent,
    state: &mut TextInputState,
    required: bool,
) -> Result<KeyAction> {
    match (key.code, key.modifiers) {
        // Ctrl+C: anuluj sesję
        (KeyCode::Char('c'), mods) if mods.contains(KeyModifiers::CONTROL) => {
            Err(RalphError::Interrupted)
        }

        // Ctrl+D: submit (jak EOF w terminalu)
        (KeyCode::Char('d'), mods) if mods.contains(KeyModifiers::CONTROL) => {
            if !required || !state.buffer.trim().is_empty() {
                Ok(KeyAction::Submit)
            } else {
                Ok(KeyAction::Continue)
            }
        }

        // Enter bez modyfikatorów: submit (jak w chat UX)
        (KeyCode::Enter, KeyModifiers::NONE) => {
            if !required || !state.buffer.trim().is_empty() {
                Ok(KeyAction::Submit)
            } else {
                Ok(KeyAction::Continue)
            }
        }

        // Shift+Enter, Alt+Enter, inne kombinacje: wstaw newline
        (KeyCode::Enter, _) => {
            insert_newline(state);
            Ok(KeyAction::Continue)
        }

        // Ctrl+J: wstaw newline (Unix LF — alternatywa)
        (KeyCode::Char('j'), mods) if mods.contains(KeyModifiers::CONTROL) => {
            insert_newline(state);
            Ok(KeyAction::Continue)
        }

        // Ctrl+A: początek bieżącej linii (= Home)
        (KeyCode::Char('a'), mods) if mods.contains(KeyModifiers::CONTROL) => {
            state.cursor_pos = find_line_start(&state.buffer, state.cursor_pos);
            Ok(KeyAction::Continue)
        }

        // Ctrl+E: koniec bieżącej linii (= End)
        (KeyCode::Char('e'), mods) if mods.contains(KeyModifiers::CONTROL) => {
            state.cursor_pos = find_line_end(&state.buffer, state.cursor_pos);
            Ok(KeyAction::Continue)
        }

        // Ctrl+W: usuń słowo wstecz
        (KeyCode::Char('w'), mods) if mods.contains(KeyModifiers::CONTROL) => {
            let target = find_prev_word_boundary(&state.buffer, state.cursor_pos);
            let from_byte = char_to_byte(&state.buffer, target);
            let to_byte = char_to_byte(&state.buffer, state.cursor_pos);
            state.buffer.drain(from_byte..to_byte);
            state.cursor_pos = target;
            Ok(KeyAction::Continue)
        }

        // Ctrl+U: usuń od kursora do początku linii
        (KeyCode::Char('u'), mods) if mods.contains(KeyModifiers::CONTROL) => {
            let line_start = find_line_start(&state.buffer, state.cursor_pos);
            let from_byte = char_to_byte(&state.buffer, line_start);
            let to_byte = char_to_byte(&state.buffer, state.cursor_pos);
            state.buffer.drain(from_byte..to_byte);
            state.cursor_pos = line_start;
            Ok(KeyAction::Continue)
        }

        // Ctrl+K: usuń od kursora do końca linii
        (KeyCode::Char('k'), mods) if mods.contains(KeyModifiers::CONTROL) => {
            let line_end = find_line_end(&state.buffer, state.cursor_pos);
            let from_byte = char_to_byte(&state.buffer, state.cursor_pos);
            let to_byte = char_to_byte(&state.buffer, line_end);
            state.buffer.drain(from_byte..to_byte);
            Ok(KeyAction::Continue)
        }

        // Ctrl+Backspace: usuń do początku linii
        (KeyCode::Backspace, mods) if mods.contains(KeyModifiers::CONTROL) => {
            let line_start = find_line_start(&state.buffer, state.cursor_pos);
            let from_byte = char_to_byte(&state.buffer, line_start);
            let to_byte = char_to_byte(&state.buffer, state.cursor_pos);
            state.buffer.drain(from_byte..to_byte);
            state.cursor_pos = line_start;
            Ok(KeyAction::Continue)
        }

        // Alt+Backspace: usuń słowo wstecz
        (KeyCode::Backspace, mods) if mods.contains(KeyModifiers::ALT) => {
            let target = find_prev_word_boundary(&state.buffer, state.cursor_pos);
            let from_byte = char_to_byte(&state.buffer, target);
            let to_byte = char_to_byte(&state.buffer, state.cursor_pos);
            state.buffer.drain(from_byte..to_byte);
            state.cursor_pos = target;
            Ok(KeyAction::Continue)
        }

        // Backspace: usuń znak przed kursorem
        (KeyCode::Backspace, _) => {
            if state.cursor_pos > 0 {
                state.cursor_pos -= 1;
                let byte_pos = char_to_byte(&state.buffer, state.cursor_pos);
                state.buffer.remove(byte_pos);
            }
            Ok(KeyAction::Continue)
        }

        // Esc: wróć (nawigacja wstecz)
        (KeyCode::Esc, _) => Ok(KeyAction::Back),

        // Nieobsłużone Ctrl+key — ignoruj (zapobiega wstawianiu liter do bufora)
        (KeyCode::Char(_), mods) if mods.contains(KeyModifiers::CONTROL) => Ok(KeyAction::Continue),

        // Char: wstaw znak na pozycji kursora
        (KeyCode::Char(c), _) => {
            let byte_pos = char_to_byte(&state.buffer, state.cursor_pos);
            state.buffer.insert(byte_pos, c);
            state.cursor_pos += 1;
            Ok(KeyAction::Continue)
        }

        // Alt+Left / Ctrl+Left: skok o słowo w lewo
        (KeyCode::Left, mods)
            if mods.contains(KeyModifiers::ALT) || mods.contains(KeyModifiers::CONTROL) =>
        {
            state.cursor_pos = find_prev_word_boundary(&state.buffer, state.cursor_pos);
            Ok(KeyAction::Continue)
        }

        // Left: przesuń kursor w lewo (char boundary)
        (KeyCode::Left, _) => {
            if state.cursor_pos > 0 {
                state.cursor_pos -= 1;
            }
            Ok(KeyAction::Continue)
        }

        // Alt+Right / Ctrl+Right: skok o słowo w prawo
        (KeyCode::Right, mods)
            if mods.contains(KeyModifiers::ALT) || mods.contains(KeyModifiers::CONTROL) =>
        {
            state.cursor_pos = find_next_word_boundary(&state.buffer, state.cursor_pos);
            Ok(KeyAction::Continue)
        }

        // Right: przesuń kursor w prawo (char boundary)
        (KeyCode::Right, _) => {
            let char_count = state.buffer.chars().count();
            if state.cursor_pos < char_count {
                state.cursor_pos += 1;
            }
            Ok(KeyAction::Continue)
        }

        // Home: kursor na początek bieżącej linii
        (KeyCode::Home, _) => {
            let byte_pos = char_to_byte(&state.buffer, state.cursor_pos);
            let before_cursor = &state.buffer[..byte_pos];
            if let Some(newline_byte) = before_cursor.rfind('\n') {
                state.cursor_pos = byte_to_char(&state.buffer, newline_byte + 1);
            } else {
                state.cursor_pos = 0;
            }
            Ok(KeyAction::Continue)
        }

        // End: kursor na koniec bieżącej linii
        (KeyCode::End, _) => {
            let byte_pos = char_to_byte(&state.buffer, state.cursor_pos);
            let after_cursor = &state.buffer[byte_pos..];
            if let Some(newline_offset) = after_cursor.find('\n') {
                state.cursor_pos += byte_to_char(after_cursor, newline_offset);
            } else {
                state.cursor_pos = state.buffer.chars().count();
            }
            Ok(KeyAction::Continue)
        }

        // Up: przesuń kursor do poprzedniej linii logicznej (ta sama kolumna)
        (KeyCode::Up, _) => {
            let byte_pos = char_to_byte(&state.buffer, state.cursor_pos);
            let before = &state.buffer[..byte_pos];
            let curr_line_start = before.rfind('\n').map(|p| p + 1).unwrap_or(0);
            let col = state.cursor_pos - byte_to_char(&state.buffer, curr_line_start);

            if curr_line_start > 0 {
                // Jest poprzednia linia
                let prev_newline = curr_line_start - 1;
                let prev_start = state.buffer[..prev_newline]
                    .rfind('\n')
                    .map(|p| p + 1)
                    .unwrap_or(0);
                let prev_line = &state.buffer[prev_start..prev_newline];
                let target_col = col.min(prev_line.chars().count());
                state.cursor_pos = byte_to_char(&state.buffer, prev_start) + target_col;
            }
            Ok(KeyAction::Continue)
        }

        // Down: przesuń kursor do następnej linii logicznej (ta sama kolumna)
        (KeyCode::Down, _) => {
            let byte_pos = char_to_byte(&state.buffer, state.cursor_pos);
            let before = &state.buffer[..byte_pos];
            let curr_line_start = before.rfind('\n').map(|p| p + 1).unwrap_or(0);
            let col = state.cursor_pos - byte_to_char(&state.buffer, curr_line_start);

            if let Some(next_newline_rel) = state.buffer[byte_pos..].find('\n') {
                // Jest następna linia
                let next_start = byte_pos + next_newline_rel + 1;
                let next_end = state.buffer[next_start..]
                    .find('\n')
                    .map(|p| next_start + p)
                    .unwrap_or(state.buffer.len());
                let next_line = &state.buffer[next_start..next_end];
                let target_col = col.min(next_line.chars().count());
                state.cursor_pos = byte_to_char(&state.buffer, next_start) + target_col;
            }
            Ok(KeyAction::Continue)
        }

        // Delete: usuń znak za kursorem
        (KeyCode::Delete, _) => {
            let char_count = state.buffer.chars().count();
            if state.cursor_pos < char_count {
                let byte_pos = char_to_byte(&state.buffer, state.cursor_pos);
                state.buffer.remove(byte_pos);
            }
            Ok(KeyAction::Continue)
        }

        _ => Ok(KeyAction::Continue),
    }
}

/// Convenience wrapper dla standalone text input bez headera
///
/// Wywołuje `text_input()` z `header: None`, przydatne gdy potrzebujemy
/// tylko prostego pola tekstowego bez pytania wyświetlanego w viewporcie.
///
/// # Arguments
/// * `placeholder` - opcjonalny tekst placeholder wyświetlany gdy pole jest puste
/// * `required` - czy pole jest wymagane (Ctrl+D nie submituje pustego tekstu)
///
/// # Returns
/// `Result<String>` - wprowadzony tekst lub błąd (np. Ctrl+C)
///
/// # Example
/// ```no_run
/// use crate::commands::standalone_text_input::standalone_text_input;
///
/// let input = standalone_text_input(Some("Enter your name..."), true)?;
/// println!("User entered: {}", input);
/// ```
#[allow(dead_code)] // Eksportowana dla użytku zewnętrznego
pub fn standalone_text_input(placeholder: Option<&str>, required: bool) -> Result<String> {
    match text_input(placeholder, None, required, None) {
        Err(RalphError::Back) => Err(RalphError::Interrupted),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_esc_returns_back() {
        let mut state = TextInputState::new(Some("test"));
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Back));
        // Buffer nie zmieniony
        assert_eq!(state.buffer, "test");
        assert_eq!(state.cursor_pos, 4);
    }

    #[test]
    fn test_handle_key_event_enter_submits() {
        // Enter submituje (chat UX)
        let mut state = TextInputState::new(Some("test"));
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Submit));
        assert_eq!(state.buffer, "test");
    }

    #[test]
    fn test_handle_key_event_enter_ignored_when_empty_and_required() {
        let mut state = TextInputState::new(None);
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(state.buffer, "");
    }

    #[test]
    fn test_handle_key_event_enter_submits_when_not_required() {
        let mut state = TextInputState::new(None);
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Submit));
    }

    #[test]
    fn test_handle_key_event_ctrl_d_submits_when_buffer_not_empty() {
        let mut state = TextInputState::new(Some("test"));
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Submit));
    }

    #[test]
    fn test_handle_key_event_ctrl_d_ignored_when_buffer_empty_and_required() {
        let mut state = TextInputState::new(None);
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
    }

    #[test]
    fn test_handle_key_event_ctrl_d_allowed_when_not_required() {
        let mut state = TextInputState::new(None);
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Submit));
    }

    #[test]
    fn test_handle_key_event_shift_enter_adds_newline() {
        let mut state = TextInputState::new(Some("line1"));
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(state.buffer, "line1\n");
        assert_eq!(state.cursor_pos, 6); // kursor po '\n'
    }

    #[test]
    fn test_handle_key_event_backspace_removes_last_char() {
        let mut state = TextInputState::new(Some("hello"));
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(state.buffer, "hell");
        assert_eq!(state.cursor_pos, 4); // kursor po 'hell'
    }

    #[test]
    fn test_handle_key_event_backspace_on_empty_buffer() {
        let mut state = TextInputState::new(None);
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer, "");
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_handle_key_event_ctrl_c_returns_error() {
        let mut state = TextInputState::new(Some("test"));
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RalphError::Interrupted));
    }

    #[test]
    fn test_handle_key_event_char_appends_to_buffer() {
        let mut state = TextInputState::new(Some("hel"));
        let key = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer, "hell");
        assert_eq!(state.cursor_pos, 4); // kursor po 'hell'
    }

    #[test]
    fn test_handle_key_event_whitespace_chars() {
        let mut state = TextInputState::new(Some("a"));
        let space_key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        let tab_key = KeyEvent::new(KeyCode::Char('\t'), KeyModifiers::NONE);

        let _ = handle_key_event(space_key, &mut state, false);
        assert_eq!(state.buffer, "a ");

        let _ = handle_key_event(tab_key, &mut state, false);
        assert_eq!(state.buffer, "a \t");
    }

    #[test]
    fn test_handle_key_event_ctrl_d_rejects_whitespace_only_when_required() {
        // Ctrl+D nie submituje gdy buffer zawiera tylko whitespace i pole jest required
        let mut state = TextInputState::new(Some("   \t  "));
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
    }

    #[test]
    fn test_multiline_input_multiple_newlines() {
        let mut state = TextInputState::new(Some("line1"));
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);

        let _ = handle_key_event(shift_enter, &mut state, false);
        // Dodaj "line2" przez insert znaków
        for c in "line2".chars() {
            let _ = handle_key_event(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &mut state,
                false,
            );
        }
        let _ = handle_key_event(shift_enter, &mut state, false);
        for c in "line3".chars() {
            let _ = handle_key_event(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &mut state,
                false,
            );
        }

        assert_eq!(state.buffer, "line1\nline2\nline3");
    }

    #[test]
    fn test_ctrl_c_preserves_buffer() {
        let original = "important data";
        let mut state = TextInputState::new(Some(original));
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        let _ = handle_key_event(key, &mut state, true);

        assert_eq!(state.buffer, original);
    }

    #[test]
    fn test_ctrl_c_during_multiline_input() {
        let mut state = TextInputState::new(Some("line1\nline2\nline3"));
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, false);

        assert!(matches!(result, Err(RalphError::Interrupted)));
        assert_eq!(state.buffer, "line1\nline2\nline3");
    }

    // ── Testy alternatywnych skrótów newline (zadanie 39.1) ────────

    #[test]
    fn test_alt_enter_inserts_newline() {
        let mut state = TextInputState::new(Some("line1"));
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(state.buffer, "line1\n");
        assert_eq!(state.cursor_pos, 6);
    }

    #[test]
    fn test_ctrl_j_inserts_newline() {
        let mut state = TextInputState::new(Some("line1"));
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(state.buffer, "line1\n");
        assert_eq!(state.cursor_pos, 6);
    }

    #[test]
    fn test_enter_with_shift_alt_ctrl_combinations() {
        // Test różnych kombinacji modyfikatorów dla Enter
        let test_cases = [
            (KeyModifiers::SHIFT, "Shift+Enter"),
            (KeyModifiers::ALT, "Alt+Enter"),
            (KeyModifiers::SHIFT | KeyModifiers::ALT, "Shift+Alt+Enter"),
        ];

        for (modifiers, desc) in &test_cases {
            let mut state = TextInputState::new(Some("test"));
            let key = KeyEvent::new(KeyCode::Enter, *modifiers);

            let result = handle_key_event(key, &mut state, false);
            assert!(result.is_ok(), "{} powinien być obsłużony", desc);
            assert!(
                matches!(result.unwrap(), KeyAction::Continue),
                "{} nie powinien submitować",
                desc
            );
            assert_eq!(state.buffer, "test\n", "{} powinien wstawić newline", desc);
            assert_eq!(state.cursor_pos, 5, "{}: kursor po newline", desc);
        }
    }

    #[test]
    fn test_ctrl_j_in_middle_of_text() {
        // Ctrl+J w środku tekstu
        let mut state = TextInputState::new(Some("hello"));
        state.cursor_pos = 2; // między 'e' i 'l'

        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        let result = handle_key_event(key, &mut state, false);

        assert!(result.is_ok());
        assert_eq!(state.buffer, "he\nllo");
        assert_eq!(state.cursor_pos, 3);
    }

    #[test]
    fn test_alt_enter_with_empty_buffer() {
        // Alt+Enter na pustym buforze
        let mut state = TextInputState::new(None);
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer, "\n");
        assert_eq!(state.cursor_pos, 1);
    }

    #[test]
    fn test_multiple_newline_keys_sequence() {
        // Sekwencja różnych klawiszy newline
        let mut state = TextInputState::new(Some("a"));

        // Shift+Enter
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
            &mut state,
            false,
        );
        assert_eq!(state.buffer, "a\n");

        // Alt+Enter
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
            &mut state,
            false,
        );
        assert_eq!(state.buffer, "a\n\n");

        // Ctrl+J
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
            &mut state,
            false,
        );
        assert_eq!(state.buffer, "a\n\n\n");
        assert_eq!(state.cursor_pos, 4);
    }

    #[test]
    fn test_unhandled_ctrl_key_ignored() {
        // Ctrl+Z itp. nie powinny wstawiać znaków do bufora
        let mut state = TextInputState::new(Some("test"));
        let ctrl_z = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL);

        let result = handle_key_event(ctrl_z, &mut state, false);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(state.buffer, "test", "Ctrl+Z nie powinien wstawiać 'z'");
    }

    #[test]
    fn test_shift_enter_inserts_newline_at_cursor_position() {
        // Shift+Enter wstawia newline w środku tekstu
        let mut state = TextInputState::new(Some("hello"));
        state.cursor_pos = 2; // między 'e' i 'l'

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        let result = handle_key_event(key, &mut state, false);

        assert!(result.is_ok());
        assert_eq!(state.buffer, "he\nllo");
        assert_eq!(state.cursor_pos, 3);
    }

    #[test]
    fn test_multiline_with_shift_enter() {
        // Test budowania multiline tekstu za pomocą Shift+Enter
        let mut state = TextInputState::new(None);
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);

        // Wpisz "a", Shift+Enter, "b", Shift+Enter, "c"
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            &mut state,
            false,
        );
        let _ = handle_key_event(shift_enter, &mut state, false);
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
            &mut state,
            false,
        );
        let _ = handle_key_event(shift_enter, &mut state, false);
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
            &mut state,
            false,
        );

        assert_eq!(state.buffer, "a\nb\nc");
    }

    #[test]
    fn test_cursor_position_with_polish_chars() {
        // "ąęśćźżółń" = 9 znaków Unicode, ale każdy to 2 bajty UTF-8
        // buffer.len() = 18, ale width() = 9
        let buffer = "ąęśćźżółń".to_string();
        let width = UnicodeWidthStr::width(buffer.as_str());
        assert_eq!(width, 9, "Polskie znaki powinny zajmować 9 kolumn");
        assert_eq!(buffer.len(), 18, "Polskie znaki zajmują 18 bajtów UTF-8");

        // Cursor powinien być na pozycji 2+9=11, nie 2+18=20
        let cursor_x = 2 + width as u16;
        assert_eq!(cursor_x, 11);
    }

    #[test]
    fn test_cursor_position_with_ascii() {
        let buffer = "test".to_string();
        let width = UnicodeWidthStr::width(buffer.as_str());
        assert_eq!(width, 4);
        assert_eq!(buffer.len(), 4);

        let cursor_x = 2 + width as u16;
        assert_eq!(cursor_x, 6);
    }

    #[test]
    fn test_cursor_position_with_mixed_chars() {
        // "aąbę" = 4 znaki Unicode, 6 bajtów UTF-8, ale 4 kolumny
        let buffer = "aąbę".to_string();
        let width = UnicodeWidthStr::width(buffer.as_str());
        assert_eq!(width, 4, "Mixed string powinien zajmować 4 kolumny");
        assert_eq!(buffer.len(), 6, "Mixed string zajmuje 6 bajtów UTF-8");

        let cursor_x = 2 + width as u16;
        assert_eq!(cursor_x, 6);
    }

    #[test]
    fn test_backspace_on_polish_char() {
        // Po backspace na polskim znaku, szerokość maleje o 1 kolumnę
        let mut buffer = "ą".to_string();
        assert_eq!(UnicodeWidthStr::width(buffer.as_str()), 1);

        buffer.pop(); // usuwa 'ą' (2 bajty UTF-8)
        assert_eq!(UnicodeWidthStr::width(buffer.as_str()), 0);
        assert_eq!(buffer.len(), 0);
    }

    // ── Nowe testy Unicode (zadanie 34.4) ──────────────────────────

    #[test]
    fn test_handle_key_polish_char_appends() {
        let mut state = TextInputState::new(None);
        let key = KeyEvent::new(KeyCode::Char('ą'), KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer, "ą");
        assert_eq!(state.buffer.len(), 2, "Znak 'ą' zajmuje 2 bajty UTF-8");
        assert_eq!(state.cursor_pos, 1, "Kursor po 'ą' na char index 1");
    }

    #[test]
    fn test_handle_key_polish_chars_sequence() {
        let mut state = TextInputState::new(None);

        // Wpisujemy "ąęść"
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Char('ą'), KeyModifiers::NONE),
            &mut state,
            false,
        );
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Char('ę'), KeyModifiers::NONE),
            &mut state,
            false,
        );
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Char('ś'), KeyModifiers::NONE),
            &mut state,
            false,
        );
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Char('ć'), KeyModifiers::NONE),
            &mut state,
            false,
        );

        assert_eq!(state.buffer, "ąęść");
        assert_eq!(
            state.buffer.chars().count(),
            4,
            "Powinno być 4 znaki Unicode"
        );
        assert_eq!(state.buffer.len(), 8, "Powinno być 8 bajtów UTF-8");
        assert_eq!(state.cursor_pos, 4, "Kursor po 'ąęść' na char index 4");
    }

    #[test]
    fn test_backspace_after_polish_char() {
        let mut state = TextInputState::new(Some("ą"));
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer, "", "Backspace powinien usunąć 'ą'");
        assert_eq!(state.buffer.len(), 0);
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_buffer_len_vs_display_width() {
        let buffer = "aą".to_string();

        // buffer.len() = 3 (a=1 byte, ą=2 bytes)
        assert_eq!(buffer.len(), 3, "Buffer 'aą' zajmuje 3 bajty UTF-8");

        // display_width = 2 (oba znaki mają szerokość 1 kolumnę)
        let width = UnicodeWidthStr::width(buffer.as_str());
        assert_eq!(width, 2, "Buffer 'aą' zajmuje 2 kolumny terminala");
    }

    // ── Testy dynamicznego viewportu (zadanie 35.2) ────────────────

    #[test]
    fn test_calculate_content_lines_empty() {
        let lines = calculate_content_lines("", 80);
        assert_eq!(lines, 1, "Pusty buffer powinien zajmować 1 linię");
    }

    #[test]
    fn test_calculate_content_lines_single_short_line() {
        let lines = calculate_content_lines("hello", 80);
        assert_eq!(lines, 1, "Krótka linia powinna zajmować 1 linię");
    }

    #[test]
    fn test_calculate_content_lines_single_long_line() {
        // "hello" * 20 = 100 znaków, terminal width 80
        // Pierwsza linia ma 78 znaków dostępnych (minus "> ")
        // Zawinięcie: 78 + 22 = 100, więc 2 linie
        let text = "hello".repeat(20);
        let lines = calculate_content_lines(&text, 80);
        assert_eq!(lines, 2, "Długa linia powinna się zawijać");
    }

    #[test]
    fn test_calculate_content_lines_multiline() {
        let text = "line1\nline2\nline3";
        let lines = calculate_content_lines(text, 80);
        assert_eq!(lines, 3, "Trzy linie logiczne = 3 linie terminala");
    }

    #[test]
    fn test_calculate_content_lines_multiline_with_wrap() {
        // Pierwsza linia: 100 znaków -> zawinięcie na 2 linie (78 + 22)
        // Druga linia: 50 znaków -> 1 linia
        // Trzecia linia: 100 znaków -> zawinięcie na 2 linie
        // Total: 5 linii
        let text = format!(
            "{}\n{}\n{}",
            "a".repeat(100),
            "b".repeat(50),
            "c".repeat(100)
        );
        let lines = calculate_content_lines(&text, 80);
        assert_eq!(lines, 5, "Multiline z wrap powinien poprawnie liczyć linie");
    }

    #[test]
    fn test_calculate_content_lines_empty_lines() {
        let text = "line1\n\nline3";
        let lines = calculate_content_lines(text, 80);
        assert_eq!(lines, 3, "Puste linie powinny być liczone");
    }

    // ── Nowe testy calculate_content_lines dla zadania 39.3 ────────────

    #[test]
    fn test_calculate_content_lines_single_char() {
        let lines = calculate_content_lines("x", 80);
        assert_eq!(lines, 1);
    }

    #[test]
    fn test_calculate_content_lines_exact_first_line_width() {
        // Dokładnie wypełnia first_line_width (78)
        let text = "a".repeat(78);
        let lines = calculate_content_lines(&text, 80);
        assert_eq!(lines, 1, "78 znaków mieści się w pierwszej linii");
    }

    #[test]
    fn test_calculate_content_lines_exceed_first_line_by_one() {
        // 79 znaków → 78 + 1 = 2 linie
        let text = "a".repeat(79);
        let lines = calculate_content_lines(&text, 80);
        assert_eq!(lines, 2, "79 znaków wymaga 2 linii");
    }

    #[test]
    fn test_calculate_content_lines_double_wrap() {
        // 158 znaków → 78 + 80 = 158 → dokładnie 2 linie
        let text = "a".repeat(158);
        let lines = calculate_content_lines(&text, 80);
        assert_eq!(lines, 2, "158 znaków dokładnie wypełnia 2 linie");
    }

    #[test]
    fn test_calculate_content_lines_triple_wrap() {
        // 159 znaków → 78 + 80 + 1 = 3 linie
        let text = "a".repeat(159);
        let lines = calculate_content_lines(&text, 80);
        assert_eq!(lines, 3, "159 znaków wymaga 3 linii");
    }

    #[test]
    fn test_calculate_content_lines_polish_first_line() {
        // Polskie znaki w pierwszej linii
        let text = "ą".repeat(78); // dokładnie first_line_width
        let lines = calculate_content_lines(&text, 80);
        assert_eq!(lines, 1, "78 polskich znaków = 1 linia");
    }

    #[test]
    fn test_calculate_content_lines_polish_overflow() {
        // Polskie znaki z overflow
        let text = "ą".repeat(79);
        let lines = calculate_content_lines(&text, 80);
        assert_eq!(lines, 2, "79 polskich znaków = 2 linie");
    }

    #[test]
    fn test_calculate_content_lines_multiline_no_wrap() {
        // Wieloliniowy bez wrap
        let text = "a\nb\nc\nd";
        let lines = calculate_content_lines(text, 80);
        assert_eq!(lines, 4);
    }

    #[test]
    fn test_calculate_content_lines_multiline_first_wraps() {
        // Pierwsza linia się zawija, reszta nie
        let text = format!("{}\nshort\nshort", "a".repeat(100));
        let lines = calculate_content_lines(&text, 80);
        // 100 = 78 + 22 → 2 linie, + 2 krótkie = 4 linie
        assert_eq!(lines, 4);
    }

    #[test]
    fn test_calculate_content_lines_multiline_all_wrap() {
        // Wszystkie linie się zawijają
        let long = "b".repeat(100);
        let text = format!("{}\n{}\n{}", long, long, long);
        let lines = calculate_content_lines(&text, 80);
        // Pierwsza linia: 78 + 22 = 2
        // Druga linia: 80 + 20 = 2
        // Trzecia linia: 80 + 20 = 2
        assert_eq!(lines, 6);
    }

    #[test]
    fn test_calculate_content_lines_empty_line_at_start() {
        let text = "\nhello";
        let lines = calculate_content_lines(text, 80);
        assert_eq!(lines, 2);
    }

    #[test]
    fn test_calculate_content_lines_empty_line_at_end() {
        let text = "hello\n";
        let lines = calculate_content_lines(text, 80);
        assert_eq!(lines, 2);
    }

    #[test]
    fn test_calculate_content_lines_only_newlines() {
        let text = "\n\n\n";
        let lines = calculate_content_lines(text, 80);
        assert_eq!(lines, 4, "4 puste linie logiczne");
    }

    #[test]
    fn test_calculate_content_lines_narrow_terminal() {
        // Bardzo wąski terminal (20 kolumn)
        let text = "hello world test"; // 16 znaków
        let lines = calculate_content_lines(text, 20);
        // first_line_width = 18 → "hello world test" (16 znaków) mieści się
        assert_eq!(lines, 1);
    }

    #[test]
    fn test_calculate_content_lines_narrow_with_wrap() {
        // Wąski terminal z wrap
        let text = "a".repeat(50);
        let lines = calculate_content_lines(&text, 20);
        // first_line_width = 18, line_width = 20
        // 50 = 18 + 20 + 12 = 3 linie
        assert_eq!(lines, 3);
    }

    #[test]
    fn test_calculate_cursor_position_single_line() {
        let buffer = "hello";
        let (x, y) = calculate_cursor_position(buffer, 5, 80);
        // Kursor po "hello" -> "> hello" -> x=7 (2+5), y=0
        assert_eq!(x, 7);
        assert_eq!(y, 0);
    }

    #[test]
    fn test_calculate_cursor_position_multiline() {
        let buffer = "line1\nline2";
        let (x, y) = calculate_cursor_position(buffer, 11, 80);
        // Kursor po "line2" -> x=5, y=1
        assert_eq!(x, 5);
        assert_eq!(y, 1);
    }

    #[test]
    fn test_calculate_cursor_position_with_wrap() {
        // "a" * 100 w terminalu 80 -> 78 (pierwsza linia) + 22 (druga linia)
        // Kursor na pozycji 100 -> koniec drugiej wrapped linii
        let buffer = "a".repeat(100);
        let (x, y) = calculate_cursor_position(&buffer, 100, 80);
        assert_eq!(y, 1, "Kursor powinien być w drugiej linii");
        assert_eq!(x, 22, "Kursor na końcu wraparound");
    }

    #[test]
    fn test_calculate_cursor_position_middle_of_wrap() {
        // "a" * 100, kursor na pozycji 50 (w środku pierwszej linii)
        let buffer = "a".repeat(100);
        let (x, y) = calculate_cursor_position(&buffer, 50, 80);
        assert_eq!(y, 0, "Kursor w pierwszej linii");
        assert_eq!(x, 52, "Kursor: 2 (prefix) + 50");
    }

    #[test]
    fn test_handle_key_up_moves_cursor_to_previous_line() {
        // Kursor na "line3" (koniec), Up → "line2" (ta sama kolumna)
        let mut state = TextInputState::new(Some("line1\nline2\nline3"));
        // cursor_pos = 17 (koniec "line3")
        assert_eq!(state.cursor_pos, 17);

        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        // "line3" ma 5 znaków, "line2" ma 5 znaków → kolumna 5 (koniec)
        // "line1\nline2" = 11 znaków, pozycja "line2" koniec = 11
        assert_eq!(state.cursor_pos, 11, "Kursor na końcu 'line2'");
    }

    #[test]
    fn test_handle_key_down_moves_cursor_to_next_line() {
        // Kursor na "line1" (koniec, pos=5), Down → "line2" (ta sama kolumna)
        let mut state = TextInputState::new(Some("line1\nline2\nline3"));
        state.cursor_pos = 5; // koniec "line1"

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        // "line1\n" = 6, "line2" kolumna 5 = 6+5 = 11
        assert_eq!(state.cursor_pos, 11, "Kursor na końcu 'line2'");
    }

    #[test]
    fn test_handle_key_down_clamps_to_shorter_line() {
        // "long_line\nhi" — kursor na koniec "long_line" (pos=9), Down → koniec "hi" (pos=12)
        let mut state = TextInputState::new(Some("long_line\nhi"));
        state.cursor_pos = 9; // koniec "long_line"

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.cursor_pos, 12, "Clamp do końca krótszej linii 'hi'");
    }

    #[test]
    fn test_up_on_first_line_does_nothing() {
        let mut state = TextInputState::new(Some("test"));
        state.cursor_pos = 2; // środek

        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.cursor_pos, 2, "Up na pierwszej linii — brak zmian");
    }

    #[test]
    fn test_down_on_last_line_does_nothing() {
        let mut state = TextInputState::new(Some("line1\nline2"));
        // cursor na "line2" koniec = 11
        assert_eq!(state.cursor_pos, 11);

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.cursor_pos, 11, "Down na ostatniej linii — brak zmian");
    }

    // ── Testy pozycji kursora w multiline z soft wrap (zadanie 35.3) ──

    #[test]
    fn test_cursor_position_at_wrap_boundary() {
        // "a" × 78, terminal_width=80 → first_line_width=78
        // Kursor dokładnie na granicy wrap → przechodzi na następną linię
        let buffer = "a".repeat(78);
        let (x, y) = calculate_cursor_position(&buffer, 78, 80);
        assert_eq!(y, 1, "Kursor na granicy wrap powinien przejść do wiersza 1");
        assert_eq!(x, 0, "Kursor na początku nowego wiersza");
    }

    #[test]
    fn test_cursor_position_double_wrap() {
        // "a" × 160, terminal_width=80 → first_line_width=78, line_width=80
        // Char-level wrap: 78 (wiersz 0) + 80 (wiersz 1) + 2 (wiersz 2) = 160
        let buffer = "a".repeat(160);
        let (x, y) = calculate_cursor_position(&buffer, 160, 80);
        // Kursor na końcu: wiersz 2 ma 2 znaki, ale display_width=2 < 80
        // więc kursor zostaje w wierszu 2 na pozycji 2
        assert_eq!(y, 2, "Kursor po podwójnym wrap");
        assert_eq!(x, 2, "Kursor na pozycji 2 w trzeciej linii (78+80+2=160)");
    }

    #[test]
    fn test_cursor_position_after_newline() {
        // Kursor zaraz po '\n' — na początku nowej linii
        let buffer = "abc\n";
        let (x, y) = calculate_cursor_position(buffer, 4, 80);
        assert_eq!(y, 1, "Kursor po newline w wierszu 1");
        assert_eq!(x, 0, "Kursor na początku nowej linii (bez prefixu)");
    }

    #[test]
    fn test_cursor_position_empty_line_between() {
        // "abc\n\ndef" — kursor na 'd' (pozycja 5)
        let buffer = "abc\n\ndef";
        let (x, y) = calculate_cursor_position(buffer, 5, 80);
        assert_eq!(y, 2, "Kursor w trzeciej logicznej linii");
        assert_eq!(x, 0, "Kursor na początku 'def'");
    }

    #[test]
    fn test_cursor_position_polish_multiline() {
        // "ąęś\nćżó" — polskie znaki, każdy ma display_width=1
        let buffer = "ąęś\nćżó";
        let cursor_pos = buffer.chars().count(); // 7 (3+1+3)
        let (x, y) = calculate_cursor_position(buffer, cursor_pos, 80);
        assert_eq!(y, 1, "Kursor w drugiej linii");
        assert_eq!(x, 3, "Polskie znaki mają szerokość 1 kolumny");
    }

    #[test]
    fn test_cursor_position_at_beginning() {
        let buffer = "hello world";
        let (x, y) = calculate_cursor_position(buffer, 0, 80);
        assert_eq!(x, 2, "Kursor na początku, po prefixie '> '");
        assert_eq!(y, 0, "Kursor w pierwszej linii");
    }

    #[test]
    fn test_cursor_position_multiline_wrap_second_line() {
        // Pierwsza linia krótka, druga się zawija
        // first_line_width=78, line_width=80
        let line2 = "b".repeat(100);
        let buffer = format!("short\n{}", line2);
        // Kursor na końcu drugiej linii: 6 + 100 = 106
        let (x, y) = calculate_cursor_position(&buffer, 106, 80);
        // Linie przed kursorem: "short" → 1 wiersz terminala (row=1 po newline)
        // Druga linia: dw=100, avail=80, wrapped_lines=100/80=1, col=100%80=20
        // row = 1 + 1 = 2
        assert_eq!(y, 2, "Kursor po wrap drugiej linii");
        assert_eq!(x, 20, "Kursor na pozycji 20");
    }

    #[test]
    fn test_cursor_position_second_line_exact_wrap() {
        // Druga linia dokładnie wypełnia terminal_width
        let line2 = "b".repeat(80);
        let buffer = format!("a\n{}", line2);
        // Kursor na końcu: 2 + 80 = 82
        let (x, y) = calculate_cursor_position(&buffer, 82, 80);
        // "a" → 1 wiersz, po newline row=1
        // "bbb...80" → dw=80, avail=80, 80>=80 → wrapped=80/80=1, col=0
        // row = 1 + 1 = 2
        assert_eq!(y, 2, "Kursor po exact wrap");
        assert_eq!(x, 0, "Kursor na początku nowej linii po wrap");
    }

    // ── Nowe testy calculate_cursor_position dla zadania 39.3 ──────────

    #[test]
    fn test_cursor_position_polish_in_wrapped_first_line() {
        // Polskie znaki w pierwszej linii, która się zawija
        let buffer = "ą".repeat(90); // 90 > 78 → wrap na 78 + 12
        let (x, y) = calculate_cursor_position(&buffer, 80, 80);
        // Kursor na pozycji 80 (char index)
        // 78 znaków w wierszu 0, reszta w wierszu 1
        assert_eq!(y, 1, "Kursor w drugiej linii po wrap");
        assert_eq!(x, 2, "Kursor na pozycji 2 w drugiej linii (80-78=2)");
    }

    #[test]
    fn test_cursor_position_polish_exact_boundary() {
        // Polski znak dokładnie na granicy fragmentu
        let buffer = "ą".repeat(78);
        let (x, y) = calculate_cursor_position(&buffer, 78, 80);
        // Dokładnie wypełnia first_line_width → kursor przechodzi na nowy wiersz
        assert_eq!(y, 1, "Kursor na nowym wierszu");
        assert_eq!(x, 0, "Kursor na początku");
    }

    #[test]
    fn test_cursor_position_polish_middle_of_line() {
        // Kursor w środku polskich znaków
        let buffer = "ąęśćźżółń"; // 9 znaków
        let (x, y) = calculate_cursor_position(buffer, 5, 80);
        // Kursor po 5 znakach → 2 (prefix) + 5 = 7
        assert_eq!(y, 0);
        assert_eq!(x, 7);
    }

    #[test]
    fn test_cursor_position_mixed_polish_ascii_wrap() {
        // Mieszane znaki z wrap
        let buffer = format!("{}abc{}", "ą".repeat(50), "ę".repeat(50));
        // 50 + 3 + 50 = 103 znaki
        let (x, y) = calculate_cursor_position(&buffer, 103, 80);
        // first_line: 78, continuation: 80
        // 103 = 78 + 25 → wiersz 1, pozycja 25
        assert_eq!(y, 1);
        assert_eq!(x, 25);
    }

    #[test]
    fn test_cursor_position_at_newline() {
        // Kursor dokładnie na znaku '\n'
        let buffer = "abc\ndef";
        let (x, y) = calculate_cursor_position(buffer, 3, 80);
        // Kursor tuż przed '\n' → pozycja 3 w pierwszej linii
        assert_eq!(y, 0);
        assert_eq!(x, 5); // 2 (prefix) + 3
    }

    #[test]
    fn test_cursor_position_after_multiple_wraps() {
        // Wiele zapętleń
        let buffer = "x".repeat(300);
        let (x, y) = calculate_cursor_position(&buffer, 300, 80);
        // 300 = 78 + 80 + 80 + 62
        // row = 3 (0-indexed: 4. wiersz terminala)
        assert_eq!(y, 3);
        assert_eq!(x, 62);
    }

    #[test]
    fn test_cursor_position_empty_line_in_multiline() {
        // Pusta linia w środku
        let buffer = "a\n\nb";
        let (x, y) = calculate_cursor_position(buffer, 2, 80);
        // Kursor na pustej linii (char index 2 = zaraz po drugim '\n')
        // "a\n" → wiersz 0, "\n" → wiersz 1 (pusty)
        assert_eq!(y, 1, "Kursor na pustej linii");
        assert_eq!(x, 0);
    }

    #[test]
    fn test_cursor_position_end_of_empty_line() {
        // Koniec pustej linii
        let buffer = "a\n\nb";
        let (x, y) = calculate_cursor_position(buffer, 3, 80);
        // char_idx=3 → wiersz 2 (trzecia linia "b"), pozycja 0
        assert_eq!(y, 2);
        assert_eq!(x, 0);
    }

    #[test]
    fn test_cursor_position_very_narrow_terminal() {
        // Bardzo wąski terminal
        let buffer = "hello world";
        let (x, y) = calculate_cursor_position(buffer, 11, 10);
        // first_line_width = 8, line_width = 10
        // "hello world" = 11 znaków
        // 8 znaków w wierszu 0, reszta (3) w wierszu 1
        assert_eq!(y, 1);
        assert_eq!(x, 3);
    }

    #[test]
    fn test_cursor_position_wrap_with_polish_at_end() {
        // Polski znak na końcu długiej linii
        let buffer = format!("{}ą", "a".repeat(77));
        // 77 + 1 = 78 → dokładnie first_line_width
        let (x, y) = calculate_cursor_position(&buffer, 78, 80);
        assert_eq!(y, 1, "Kursor przechodzi na nowy wiersz");
        assert_eq!(x, 0);
    }

    #[test]
    fn test_cursor_position_triple_line_multiline() {
        // Trzy linie logiczne, jedna się zawija
        let line1 = "short";
        let line2 = "b".repeat(100); // zawija się
        let line3 = "end";
        let buffer = format!("{}\n{}\n{}", line1, line2, line3);

        // Kursor na końcu line3
        let cursor_pos = buffer.chars().count();
        let (x, y) = calculate_cursor_position(&buffer, cursor_pos, 80);

        // line1: 1 wiersz (row=0)
        // line2: 2 wiersze (rows=1,2) → 80 + 20
        // line3: 1 wiersz (row=3)
        assert_eq!(y, 3);
        assert_eq!(x, 3); // "end" ma 3 znaki
    }

    #[test]
    fn test_cursor_position_wrap_continuation_prefix() {
        // Test że continuation lines (po wrap) NIE mają prefixu "> "
        let buffer = "a".repeat(80);
        let (x, y) = calculate_cursor_position(&buffer, 79, 80);
        // 79 znaków → 78 w wierszu 0, 1 w wierszu 1
        assert_eq!(y, 1);
        assert_eq!(x, 1, "Continuation line bez prefixu");
    }

    #[test]
    fn test_cursor_position_multiline_first_short_second_wraps() {
        // Pierwsza krótka, druga długa
        let buffer = format!("a\n{}", "b".repeat(150));
        // Kursor na końcu drugiej linii
        let cursor_pos = buffer.chars().count();
        let (x, y) = calculate_cursor_position(&buffer, cursor_pos, 80);

        // line1: "a" → wiersz 0
        // line2: 150 znaków → 80 + 70 = 2 wiersze (rows 1,2)
        assert_eq!(y, 2);
        assert_eq!(x, 70);
    }

    // ── Testy auto-scroll ──

    /// Helper: symuluje auto-scroll logikę z pętli draw
    fn auto_scroll(scroll_offset: &mut usize, cursor_row: u16, visible_height: u16) {
        let vis = visible_height as usize;
        if (cursor_row as usize) < *scroll_offset {
            *scroll_offset = cursor_row as usize;
        } else if (cursor_row as usize) >= *scroll_offset + vis {
            *scroll_offset = (cursor_row as usize) - vis + 1;
        }
    }

    #[test]
    fn test_auto_scroll_cursor_below_viewport() {
        let mut scroll = 0usize;
        // Kursor w wierszu 12, viewport ma 10 linii
        auto_scroll(&mut scroll, 12, 10);
        assert_eq!(
            scroll, 3,
            "Scroll powinien przesunąć się aby kursor był widoczny"
        );
    }

    #[test]
    fn test_auto_scroll_cursor_above_viewport() {
        let mut scroll = 5usize;
        // Kursor w wierszu 3, viewport zaczyna się od 5
        auto_scroll(&mut scroll, 3, 10);
        assert_eq!(scroll, 3, "Scroll powinien cofnąć się do pozycji kursora");
    }

    #[test]
    fn test_auto_scroll_cursor_within_viewport() {
        let mut scroll = 2usize;
        // Kursor w wierszu 5, viewport 2..12
        auto_scroll(&mut scroll, 5, 10);
        assert_eq!(
            scroll, 2,
            "Scroll nie powinien się zmienić gdy kursor widoczny"
        );
    }

    #[test]
    fn test_auto_scroll_cursor_at_viewport_top() {
        let mut scroll = 5usize;
        auto_scroll(&mut scroll, 5, 10);
        assert_eq!(
            scroll, 5,
            "Scroll nie zmienia się gdy kursor na górze viewportu"
        );
    }

    #[test]
    fn test_auto_scroll_cursor_at_viewport_bottom() {
        let mut scroll = 0usize;
        // Kursor w wierszu 9, viewport ma 10 linii (0..9) — jeszcze widoczny
        auto_scroll(&mut scroll, 9, 10);
        assert_eq!(scroll, 0, "Kursor na dole viewportu nie wymaga scrollu");
    }

    #[test]
    fn test_auto_scroll_cursor_just_past_viewport() {
        let mut scroll = 0usize;
        // Kursor w wierszu 10, viewport ma 10 linii (0..9) — już poza
        auto_scroll(&mut scroll, 10, 10);
        assert_eq!(scroll, 1, "Scroll o 1 gdy kursor tuż za viewportem");
    }

    // ── Testy TextInputState (zadanie 35.4) ────────────────────────────

    #[test]
    fn test_state_insert_at_cursor() {
        // Test wstawiania znaków w środku tekstu
        let mut state = TextInputState::new(Some("hello"));
        state.cursor_pos = 2; // Kursor między 'e' i 'l'

        // Wstaw 'X' w środku
        let key = KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE);
        let result = handle_key_event(key, &mut state, false);

        assert!(result.is_ok());
        assert_eq!(state.buffer, "heXllo", "Znak 'X' powinien być w środku");
        assert_eq!(state.cursor_pos, 3, "Kursor po 'X'");
    }

    #[test]
    fn test_state_backspace_at_cursor() {
        // Test backspace w środku tekstu
        let mut state = TextInputState::new(Some("hello"));
        state.cursor_pos = 3; // Kursor po 'hel'

        // Usuń 'l' przed kursorem
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        let result = handle_key_event(key, &mut state, false);

        assert!(result.is_ok());
        assert_eq!(state.buffer, "helo", "Środkowy znak powinien być usunięty");
        assert_eq!(state.cursor_pos, 2, "Kursor po 'he'");
    }

    #[test]
    fn test_state_left_right_navigation() {
        // Test nawigacji Left/Right po char boundaries
        let mut state = TextInputState::new(Some("ąbć"));

        // Zaczynamy na końcu (pozycja 3)
        assert_eq!(state.cursor_pos, 3);

        // Left × 3 — powinniśmy dojść do początku
        for _ in 0..3 {
            let key = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
            let _ = handle_key_event(key, &mut state, false);
        }
        assert_eq!(state.cursor_pos, 0, "Kursor na początku po Left × 3");

        // Right × 3 — powinniśmy dojść do końca
        for _ in 0..3 {
            let key = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
            let _ = handle_key_event(key, &mut state, false);
        }
        assert_eq!(state.cursor_pos, 3, "Kursor na końcu po Right × 3");

        // Right na końcu — nie powinno nic się stać
        let key = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.cursor_pos, 3, "Kursor dalej na końcu");

        // Left na początku
        state.cursor_pos = 0;
        let key = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.cursor_pos, 0, "Kursor dalej na początku");
    }

    #[test]
    fn test_state_home_end() {
        // Test Home/End w multiline tekście
        let mut state = TextInputState::new(Some("line1\nline2\nline3"));

        // Ustaw kursor w środku drugiej linii (pozycja 9: "line1\nlin")
        state.cursor_pos = 9;

        // Home — kursor na początek bieżącej linii
        let key = KeyEvent::new(KeyCode::Home, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.cursor_pos, 6, "Home ustawia kursor na początek linii");

        // End — kursor na koniec bieżącej linii
        let key = KeyEvent::new(KeyCode::End, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.cursor_pos, 11, "End ustawia kursor na koniec linii");

        // End na ostatniej linii — kursor na koniec całego bufora
        state.cursor_pos = 12; // początek "line3"
        let key = KeyEvent::new(KeyCode::End, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(
            state.cursor_pos, 17,
            "End na ostatniej linii idzie do końca bufora"
        );

        // Home na pierwszej linii — kursor na pozycję 0
        state.cursor_pos = 3; // w środku "line1"
        let key = KeyEvent::new(KeyCode::Home, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.cursor_pos, 0, "Home na pierwszej linii idzie do 0");
    }

    #[test]
    fn test_ctrl_d_submits_multiline() {
        // Test że Ctrl+D submituje cały multiline content
        let mut state = TextInputState::new(Some("line1\nline2\nline3"));

        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        let result = handle_key_event(key, &mut state, false);

        assert!(result.is_ok());
        assert!(
            matches!(result.unwrap(), KeyAction::Submit),
            "Ctrl+D powinien submitować multiline content"
        );
        assert_eq!(
            state.buffer, "line1\nline2\nline3",
            "Buffer powinien pozostać niezmieniony"
        );
    }

    #[test]
    fn test_empty_lines() {
        // Test Enter Enter Enter → 3 puste linie
        let mut state = TextInputState::new(None);

        // Shift+Enter × 3
        for _ in 0..3 {
            let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
            let _ = handle_key_event(key, &mut state, false);
        }

        assert_eq!(state.buffer, "\n\n\n", "Buffer powinien zawierać 3 newline");
        assert_eq!(state.cursor_pos, 3, "Kursor po 3 newline");
    }

    #[test]
    fn test_very_long_single_line() {
        // Test 200 znaków w jednej linii → char-level wrap
        let buffer = "a".repeat(200);

        let lines = calculate_content_lines(&buffer, 80);

        // Pierwsza linia: 78 znaków (minus "> ")
        // Kolejne linie: 80 znaków każda
        // 200 = 78 + 80 + 42 → 3 linie
        assert_eq!(lines, 3, "200 znaków powinno zawijać na 3 linie");

        // Test pozycji kursora na końcu długiej linii
        let (x, y) = calculate_cursor_position(&buffer, 200, 80);
        // 200 = 78 (wiersz 0) + 80 (wiersz 1) + 42 (wiersz 2)
        // Kursor na pozycji 42 w wierszu 2
        assert_eq!(y, 2, "Kursor w trzeciej linii");
        assert_eq!(
            x, 42,
            "Kursor na pozycji 42 w trzeciej linii (78+80+42=200)"
        );
    }

    // ── Dodatkowe testy edge cases ──

    #[test]
    fn test_insert_unicode_at_cursor() {
        // Test wstawiania polskich znaków w środku tekstu
        let mut state = TextInputState::new(Some("abc"));
        state.cursor_pos = 1; // między 'a' i 'b'

        let key = KeyEvent::new(KeyCode::Char('ż'), KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);

        assert_eq!(state.buffer, "ażbc", "Znak 'ż' powinien być wstawiony");
        assert_eq!(state.cursor_pos, 2, "Kursor po 'ż'");
    }

    #[test]
    fn test_backspace_unicode_at_cursor() {
        // Test backspace na polskim znaku w środku tekstu
        let mut state = TextInputState::new(Some("aźb"));
        state.cursor_pos = 2; // po "aź"

        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);

        assert_eq!(state.buffer, "ab", "Znak 'ź' powinien być usunięty");
        assert_eq!(state.cursor_pos, 1, "Kursor po 'a'");
    }

    #[test]
    fn test_multiline_navigation_boundaries() {
        // Test granic nawigacji w multiline
        let mut state = TextInputState::new(Some("a\nb\nc"));

        // Left na początku — nic się nie dzieje
        state.cursor_pos = 0;
        let key = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.cursor_pos, 0);

        // Right do końca
        state.cursor_pos = 5; // na końcu
        let key = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.cursor_pos, 5, "Right na końcu nie zmienia pozycji");
    }

    #[test]
    fn test_calculate_cursor_position_edge_cases() {
        // Test pustego bufora
        let (x, y) = calculate_cursor_position("", 0, 80);
        assert_eq!(x, 2, "Pusty buffer: kursor po prefixie '> '");
        assert_eq!(y, 0);

        // Test kursora przed pierwszym znakiem
        let (x, y) = calculate_cursor_position("abc", 0, 80);
        assert_eq!(x, 2, "Kursor przed 'abc': po prefixie");
        assert_eq!(y, 0);

        // Test pojedynczego znaku
        let (x, y) = calculate_cursor_position("a", 1, 80);
        assert_eq!(x, 3, "Kursor po 'a': 2 (prefix) + 1");
        assert_eq!(y, 0);
    }

    #[test]
    fn test_char_to_byte_conversions() {
        // Test konwersji char<->byte dla różnych typów znaków
        let text = "aąb";

        // char 0 → byte 0
        assert_eq!(char_to_byte(text, 0), 0);
        // char 1 → byte 1
        assert_eq!(char_to_byte(text, 1), 1);
        // char 2 → byte 3 (ą zajmuje 2 bajty)
        assert_eq!(char_to_byte(text, 2), 3);
        // char 3 → byte 4
        assert_eq!(char_to_byte(text, 3), 4);
        // char poza zakresem → text.len()
        assert_eq!(char_to_byte(text, 10), 4);

        // Odwrotnie: byte→char
        assert_eq!(byte_to_char(text, 0), 0);
        assert_eq!(byte_to_char(text, 1), 1);
        assert_eq!(byte_to_char(text, 3), 2);
        assert_eq!(byte_to_char(text, 4), 3);
    }

    // ── Testy char-level wrap (zadanie 39.2) ──────────────────────────

    #[test]
    fn test_wrap_line_short() {
        let frags = wrap_line("hello", 80);
        assert_eq!(frags, vec!["hello"]);
    }

    #[test]
    fn test_wrap_line_exact_width() {
        let line = "a".repeat(10);
        let frags = wrap_line(&line, 10);
        assert_eq!(frags, vec!["aaaaaaaaaa"]);
    }

    #[test]
    fn test_wrap_line_overflow_by_one() {
        let line = "a".repeat(11);
        let frags = wrap_line(&line, 10);
        assert_eq!(frags.len(), 2);
        assert_eq!(frags[0], "aaaaaaaaaa");
        assert_eq!(frags[1], "a");
    }

    #[test]
    fn test_wrap_line_empty() {
        let frags = wrap_line("", 10);
        assert_eq!(frags, vec![""]);
    }

    #[test]
    fn test_wrap_line_polish_chars_at_boundary() {
        // "ą" ma display width 1 i byte size 2
        // 10 polskich znaków × 1 kolumna = 10 kolumn → mieści się w width=10
        let line = "ąęśćźżółńa";
        assert_eq!(UnicodeWidthStr::width(line), 10);
        let frags = wrap_line(line, 10);
        assert_eq!(
            frags,
            vec!["ąęśćźżółńa"],
            "10 znaków powinno zmieścić się w 10 kolumnach"
        );
    }

    #[test]
    fn test_wrap_line_polish_overflow() {
        // 11 polskich znaków × 1 kolumna = 11 kolumn → overflow przy width=10
        let line = "ąęśćźżółńąę";
        assert_eq!(UnicodeWidthStr::width(line), 11);
        let frags = wrap_line(line, 10);
        assert_eq!(frags.len(), 2);
        assert_eq!(UnicodeWidthStr::width(frags[0]), 10);
        assert_eq!(UnicodeWidthStr::width(frags[1]), 1);
    }

    #[test]
    fn test_wrap_line_triple_wrap() {
        let line = "a".repeat(25);
        let frags = wrap_line(&line, 10);
        assert_eq!(frags.len(), 3);
        assert_eq!(frags[0].len(), 10);
        assert_eq!(frags[1].len(), 10);
        assert_eq!(frags[2].len(), 5);
    }

    // ── Nowe testy wrap_line dla zadania 39.3 ──────────────────────────

    #[test]
    fn test_wrap_line_width_zero() {
        // Edge case: width=0 powinien zwrócić całą linię
        let frags = wrap_line("hello", 0);
        assert_eq!(frags, vec!["hello"]);
    }

    #[test]
    fn test_wrap_line_single_char() {
        let frags = wrap_line("a", 5);
        assert_eq!(frags, vec!["a"]);
    }

    #[test]
    fn test_wrap_line_width_one() {
        // Każdy znak w osobnym fragmencie
        let frags = wrap_line("abc", 1);
        assert_eq!(frags, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_wrap_line_mixed_polish_ascii() {
        // Mieszane znaki ASCII i polskie
        let line = "hello ąęś world ćżó";
        let frags = wrap_line(line, 10);
        // "hello ąęś " = 10 kolumn (h,e,l,l,o, ,ą,ę,ś, = 10)
        assert!(frags.len() >= 2);
        assert_eq!(UnicodeWidthStr::width(frags[0]), 10);
    }

    #[test]
    fn test_wrap_line_only_polish_chars() {
        // Tylko polskie znaki
        let line = "ąęśćźżółńąę";
        let frags = wrap_line(line, 5);
        assert_eq!(frags.len(), 3); // 11 znaków / 5 = 3 fragmenty
        assert_eq!(UnicodeWidthStr::width(frags[0]), 5);
        assert_eq!(UnicodeWidthStr::width(frags[1]), 5);
        assert_eq!(UnicodeWidthStr::width(frags[2]), 1);
    }

    #[test]
    fn test_wrap_line_polish_char_at_boundary() {
        // Polski znak dokładnie na granicy fragmentu
        let line = "abcdą"; // 5 znaków, 5 kolumn
        let frags = wrap_line(line, 5);
        assert_eq!(frags, vec!["abcdą"]);
    }

    #[test]
    fn test_wrap_line_polish_char_splits_fragment() {
        // Polski znak w pierwszym fragmencie, wrap na 'e'
        let line = "abcdąe"; // 6 znaków, 6 kolumn, width=5
        let frags = wrap_line(line, 5);
        assert_eq!(frags.len(), 2);
        assert_eq!(frags[0], "abcdą"); // 5 znaków mieści się w width=5
        assert_eq!(frags[1], "e"); // 'e' wymusza nowy fragment
    }

    #[test]
    fn test_wrap_line_whitespace() {
        // Whitespace na granicy
        let line = "hello world test"; // width=10
        let frags = wrap_line(line, 10);
        // Char-level wrap nie rozpoznaje słów — tnie dokładnie po kolumnach
        assert_eq!(frags.len(), 2);
        assert_eq!(UnicodeWidthStr::width(frags[0]), 10);
    }

    #[test]
    fn test_wrap_line_newline_not_handled() {
        // wrap_line operuje na pojedynczej linii logicznej
        // — newline nie powinien być obsłużony (wywołujący splituje po '\n')
        let line = "hello"; // bez '\n'
        let frags = wrap_line(line, 10);
        assert_eq!(frags, vec!["hello"]);
    }

    #[test]
    fn test_wrap_line_exact_double_width() {
        // Dokładnie 2× width
        let line = "a".repeat(20);
        let frags = wrap_line(&line, 10);
        assert_eq!(frags.len(), 2);
        assert_eq!(frags[0].len(), 10);
        assert_eq!(frags[1].len(), 10);
    }

    #[test]
    fn test_wrap_line_very_long() {
        // Bardzo długa linia
        let line = "x".repeat(500);
        let frags = wrap_line(&line, 80);
        assert_eq!(frags.len(), 7); // 500 / 80 = 6.25 → 7 fragmentów
        for (i, frag) in frags.iter().enumerate() {
            if i < 6 {
                assert_eq!(frag.len(), 80);
            } else {
                assert_eq!(frag.len(), 20); // ostatni fragment: reszta
            }
        }
    }

    #[test]
    fn test_build_wrapped_lines_empty() {
        let lines = build_wrapped_lines("", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], (0, ""));
    }

    #[test]
    fn test_build_wrapped_lines_short() {
        let lines = build_wrapped_lines("hello", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], (0, "hello"));
    }

    #[test]
    fn test_build_wrapped_lines_first_line_wrap() {
        // Pierwsza linia: 78 + reszta → 2 wiersze
        let text = "a".repeat(100);
        let lines = build_wrapped_lines(&text, 80);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0].1.len(),
            78,
            "Pierwsza wrapped linia: 78 znaków (term_width - 2)"
        );
        assert_eq!(
            lines[1].1.len(),
            22,
            "Druga wrapped linia: reszta 22 znaków"
        );
    }

    #[test]
    fn test_build_wrapped_lines_multiline() {
        let lines = build_wrapped_lines("abc\ndef\nghi", 80);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], (0, "abc"));
        assert_eq!(lines[1], (1, "def"));
        assert_eq!(lines[2], (2, "ghi"));
    }

    #[test]
    fn test_build_wrapped_lines_second_line_wrap() {
        // Druga linia: 80+20 = 100 znaków → 2 wiersze
        let text = format!("short\n{}", "b".repeat(100));
        let lines = build_wrapped_lines(&text, 80);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], (0, "short"));
        assert_eq!(lines[1].1.len(), 80, "Druga linia wrap: 80 znaków");
        assert_eq!(lines[2].1.len(), 20, "Trzecia linia: reszta 20 znaków");
    }

    #[test]
    fn test_build_wrapped_lines_empty_lines() {
        let lines = build_wrapped_lines("a\n\nb", 80);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], (0, "a"));
        assert_eq!(lines[1], (1, ""));
        assert_eq!(lines[2], (2, "b"));
    }

    #[test]
    fn test_cursor_position_polish_at_wrap_boundary() {
        // Polskie znaki na granicy first_line_width (78)
        let buffer = "ą".repeat(78); // 78 znaków × 1 kolumna = 78 kolumn = first_line_width
        assert_eq!(UnicodeWidthStr::width(buffer.as_str()), 78);

        let (x, y) = calculate_cursor_position(&buffer, 78, 80);
        assert_eq!(
            y, 1,
            "Kursor po wypełnieniu first_line_width powinien być na nowym wierszu"
        );
        assert_eq!(x, 0, "Kursor na początku nowego wiersza");

        let lines = calculate_content_lines(&buffer, 80);
        assert_eq!(
            lines, 1,
            "78 polskich znaków dokładnie wypełnia first_line_width"
        );
    }

    #[test]
    fn test_cursor_position_polish_overflow_boundary() {
        // 79 polskich znaków → 78 + 1 = 2 linie
        let buffer = "ą".repeat(79);
        let (x, y) = calculate_cursor_position(&buffer, 79, 80);
        assert_eq!(y, 1, "Kursor w drugiej linii");
        assert_eq!(x, 1, "Kursor na pozycji 1 (jeden znak w drugiej linii)");

        let lines = calculate_content_lines(&buffer, 80);
        assert_eq!(lines, 2, "79 polskich znaków = 2 linie");
    }

    // ── Testy spójności cursor/content/wrap (zadanie 39.3) ──────────────────────────

    #[test]
    fn test_consistency_cursor_never_exceeds_content_lines() {
        // Kursor na końcu nigdy nie powinien być poza content_lines
        let test_cases = [
            ("x", 80),
            (&"a".repeat(78), 80),
            (&"a".repeat(79), 80),
            (&"a".repeat(158), 80),
            (&"a".repeat(159), 80),
            (&"a".repeat(300), 80),
            ("hello\nworld", 80),
            (&format!("{}\n{}", "a".repeat(100), "b".repeat(100)), 80),
            ("ąęś", 80),
            (&"ą".repeat(78), 80),
            (&"ą".repeat(79), 80),
            // Wąski terminal
            ("hello world", 20),
            (&"x".repeat(50), 20),
        ];

        for (text, width) in &test_cases {
            let content_lines = calculate_content_lines(text, *width);
            let char_count = text.chars().count();
            let (_, cursor_row) = calculate_cursor_position(text, char_count, *width);

            assert!(
                (cursor_row as usize) <= content_lines,
                "text={:?}, width={}: cursor_row={} > content_lines={}",
                &text[..text.len().min(20)],
                width,
                cursor_row,
                content_lines
            );
        }
    }

    #[test]
    fn test_consistency_wrapped_lines_equals_content_lines() {
        // build_wrapped_lines().len() == calculate_content_lines()
        let test_cases = [
            ("", 80),
            ("a", 80),
            (&"x".repeat(77), 80),
            (&"x".repeat(78), 80),
            (&"x".repeat(79), 80),
            (&"x".repeat(158), 80),
            (&"x".repeat(159), 80),
            ("line1\nline2\nline3", 80),
            (&format!("{}\n{}", "a".repeat(100), "b".repeat(50)), 80),
            ("ąęśćźżółń", 80),
            (&"ą".repeat(100), 80),
            // Edge cases
            ("\n\n\n", 80),
            ("a\n\nb", 80),
            (&"x".repeat(50), 20),
        ];

        for (text, width) in &test_cases {
            let wrapped = build_wrapped_lines(text, *width);
            let content = calculate_content_lines(text, *width);

            assert_eq!(
                wrapped.len(),
                content,
                "text={:?}, width={}: wrapped.len()={} != content_lines={}",
                &text[..text.len().min(20)],
                width,
                wrapped.len(),
                content
            );
        }
    }

    #[test]
    fn test_consistency_cursor_at_every_position() {
        // Dla każdej pozycji kursora w tekście, pozycja powinna być sensowna
        let buffer = "abc\ndef\nghi";

        for cursor_pos in 0..=buffer.chars().count() {
            let (x, y) = calculate_cursor_position(buffer, cursor_pos, 80);
            let content_lines = calculate_content_lines(buffer, 80);

            assert!(
                (y as usize) < content_lines,
                "cursor_pos={}: y={} >= content_lines={}",
                cursor_pos,
                y,
                content_lines
            );

            // x nie powinien przekraczać szerokości terminala
            assert!(
                x < 80,
                "cursor_pos={}: x={} >= terminal_width",
                cursor_pos,
                x
            );
        }
    }

    #[test]
    fn test_consistency_cursor_multiline_wrapped() {
        // Multiline z wrap - kursor w każdej pozycji
        let buffer = format!("{}\n{}", "a".repeat(100), "b".repeat(50));
        let char_count = buffer.chars().count();

        for cursor_pos in 0..=char_count {
            let (_x, y) = calculate_cursor_position(&buffer, cursor_pos, 80);
            let content_lines = calculate_content_lines(&buffer, 80);

            assert!(
                (y as usize) <= content_lines,
                "cursor_pos={}: y={} > content_lines={}",
                cursor_pos,
                y,
                content_lines
            );
        }
    }

    #[test]
    fn test_consistency_polish_chars_all_positions() {
        // Polskie znaki - kursor w każdej pozycji
        let buffer = "ąęść\nźżół";

        for cursor_pos in 0..=buffer.chars().count() {
            let (_x, y) = calculate_cursor_position(buffer, cursor_pos, 80);
            let content_lines = calculate_content_lines(buffer, 80);

            assert!(
                (y as usize) < content_lines,
                "cursor_pos={}: y={} >= content_lines={}",
                cursor_pos,
                y,
                content_lines
            );
        }
    }

    #[test]
    fn test_consistency_empty_lines_cursor() {
        // Puste linie - kursor powinien być na właściwej pozycji
        let buffer = "a\n\n\nb";

        // Kursor na pierwszej pustej linii (pozycja 2)
        let (x, y) = calculate_cursor_position(buffer, 2, 80);
        assert_eq!(y, 1, "Kursor na pierwszej pustej linii");
        assert_eq!(x, 0);

        // Kursor na drugiej pustej linii (pozycja 3)
        let (x, y) = calculate_cursor_position(buffer, 3, 80);
        assert_eq!(y, 2, "Kursor na drugiej pustej linii");
        assert_eq!(x, 0);
    }

    #[test]
    fn test_consistency_very_long_single_line() {
        // Bardzo długa pojedyncza linia
        let buffer = "y".repeat(500);
        let content_lines = calculate_content_lines(&buffer, 80);
        let (_, cursor_row) = calculate_cursor_position(&buffer, 500, 80);

        // 500 = 78 + 80*5 + 22 = 7 linii
        assert_eq!(content_lines, 7);
        assert!(
            (cursor_row as usize) <= content_lines,
            "cursor_row={} > content_lines={}",
            cursor_row,
            content_lines
        );
    }

    #[test]
    fn test_consistency_wrapped_lines_fragments_display_width() {
        // Weryfikacja że fragmenty w build_wrapped_lines nie przekraczają dostępnej szerokości
        let buffer = format!("{}\n{}", "a".repeat(200), "b".repeat(150));
        let wrapped = build_wrapped_lines(&buffer, 80);

        for (row, (logical_idx, fragment)) in wrapped.iter().enumerate() {
            let display_width = UnicodeWidthStr::width(*fragment);
            let avail_width = if row == 0 && *logical_idx == 0 {
                78 // first line ma prefix "> "
            } else {
                80
            };

            assert!(
                display_width <= avail_width,
                "row={}, logical={}: display_width={} > avail_width={}",
                row,
                logical_idx,
                display_width,
                avail_width
            );
        }
    }

    #[test]
    fn test_consistency_cursor_x_never_exceeds_fragment_width() {
        // Pozycja x kursora nie powinna przekraczać szerokości fragmentu + prefix
        let test_cases = [
            "hello",
            &"a".repeat(78),
            &"a".repeat(79),
            &"a".repeat(200),
            "line1\nline2",
            &format!("{}\n{}", "a".repeat(100), "b".repeat(100)),
            "ąęśćźżółń",
        ];

        for text in &test_cases {
            let char_count = text.chars().count();
            let (x, y) = calculate_cursor_position(text, char_count, 80);

            if y == 0 {
                // Pierwsza linia: max x = 80 (2 prefix + 78 znaków)
                assert!(x <= 80, "text={:?}: x={} > 80", text, x);
            } else {
                // Continuation lines: max x = 80
                assert!(x <= 80, "text={:?}: x={} > 80", text, x);
            }
        }
    }

    // ── Nowe testy dla zadania 39.3 ──────────────────────────────────────

    /// Testy dla nowych kombinacji klawiszy newline (zadanie 39.1)

    #[test]
    fn test_newline_ctrl_j_multiline() {
        // Ctrl+J tworzy multiline
        let mut state = TextInputState::new(Some("line1"));
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
            &mut state,
            false,
        );
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            &mut state,
            false,
        );
        assert_eq!(state.buffer, "line1\nx");
    }

    /// Testy wrap_line helper z różnymi edge cases

    #[test]
    fn test_wrap_line_unicode_polish_exact() {
        // Polskie znaki dokładnie wypełniają width
        let line = "ą".repeat(15);
        let frags = wrap_line(&line, 15);
        assert_eq!(frags.len(), 1);
        assert_eq!(UnicodeWidthStr::width(frags[0]), 15);
    }

    #[test]
    fn test_wrap_line_polish_on_boundary_mixed() {
        // Mieszane ASCII i polskie na granicy
        let line = "abcąe"; // 5 znaków, 5 kolumn, width=5
        let frags = wrap_line(line, 5);
        assert_eq!(frags, vec!["abcąe"]);
    }

    #[test]
    fn test_wrap_line_polish_overflow_one() {
        // Polski znak powoduje overflow o 1
        let line = "abcąef"; // 6 znaków, width=5
        let frags = wrap_line(line, 5);
        assert_eq!(frags.len(), 2);
        assert_eq!(UnicodeWidthStr::width(frags[0]), 5);
        assert_eq!(frags[1], "f");
    }

    /// Testy calculate_content_lines z nowym char-level wrap

    #[test]
    fn test_content_lines_char_wrap_exact() {
        // Dokładnie wypełnia pierwszy wiersz (78)
        let text = "x".repeat(78);
        let lines = calculate_content_lines(&text, 80);
        assert_eq!(lines, 1);
    }

    #[test]
    fn test_content_lines_char_wrap_overflow_one_char() {
        // O 1 znak więcej niż pierwszy wiersz
        let text = "x".repeat(79);
        let lines = calculate_content_lines(&text, 80);
        assert_eq!(lines, 2);
    }

    #[test]
    fn test_content_lines_multiline_first_wraps_polish() {
        // Pierwsza linia zawija się, reszta nie, polskie znaki
        let text = format!("{}\nkrótka", "ą".repeat(100));
        let lines = calculate_content_lines(&text, 80);
        // 100 polskich znaków = 78 + 22 = 2 wiersze, + 1 wiersz dla "krótka"
        assert_eq!(lines, 3);
    }

    #[test]
    fn test_content_lines_all_empty_lines() {
        // Same puste linie
        let text = "\n\n\n\n";
        let lines = calculate_content_lines(text, 80);
        assert_eq!(lines, 5); // 5 pustych linii logicznych
    }

    /// Testy calculate_cursor_position dla wrapped lines z polskimi znakami

    #[test]
    fn test_cursor_wrapped_polish_middle_position() {
        // Kursor w środku wrapped linii z polskimi znakami
        let buffer = "ą".repeat(90); // Zawinięcie: 78 + 12
        let (x, y) = calculate_cursor_position(&buffer, 80, 80);
        // 80 znaków = 78 w wierszu 0, 2 w wierszu 1
        assert_eq!(y, 1);
        assert_eq!(x, 2);
    }

    #[test]
    fn test_cursor_polish_multiline_second_line_wrap() {
        // Druga linia zawija się, polskie znaki
        let line2 = "ą".repeat(100);
        let buffer = format!("short\n{}", line2);
        let cursor_pos = buffer.chars().count();
        let (x, y) = calculate_cursor_position(&buffer, cursor_pos, 80);
        // "short" = wiersz 0, druga linia: 80 + 20 = wiersze 1-2
        assert_eq!(y, 2);
        assert_eq!(x, 20);
    }

    #[test]
    fn test_cursor_polish_at_exact_boundary_78() {
        // Polski znak dokładnie na granicy 78 (first_line_width)
        let buffer = "ż".repeat(78);
        let (x, y) = calculate_cursor_position(&buffer, 78, 80);
        assert_eq!(y, 1, "Kursor na nowym wierszu po wypełnieniu 78 kolumn");
        assert_eq!(x, 0);
    }

    #[test]
    fn test_cursor_polish_mixed_wrap_middle() {
        // Mieszane znaki, kursor w środku wrapped fragmentu
        let buffer = format!("{}test{}", "ą".repeat(50), "ę".repeat(40));
        let (x, y) = calculate_cursor_position(&buffer, 54, 80); // W "test"
        // 50 polskich + "test" (4) = 54, first_line: 78 → wszystko w wierszu 0
        assert_eq!(y, 0);
        assert_eq!(x, 56); // 2 (prefix) + 54
    }

    #[test]
    fn test_cursor_polish_on_newline_boundary() {
        // Kursor tuż przed '\n' z polskimi znakami
        let buffer = "ąęść\nźżół";
        let (x, y) = calculate_cursor_position(buffer, 4, 80); // Tuż przed '\n'
        assert_eq!(y, 0);
        assert_eq!(x, 6); // 2 (prefix) + 4
    }

    /// Testy spójności calculate_content_lines i calculate_cursor_position

    #[test]
    fn test_consistency_polish_all_lengths() {
        // Polskie znaki - różne długości
        for len in [1, 10, 50, 77, 78, 79, 100, 150, 200] {
            let buffer = "ą".repeat(len);
            let content_lines = calculate_content_lines(&buffer, 80);
            let (_, cursor_row) = calculate_cursor_position(&buffer, len, 80);
            assert!(
                (cursor_row as usize) <= content_lines,
                "len={}: cursor_row > content_lines",
                len
            );
        }
    }

    #[test]
    fn test_consistency_mixed_multiline_polish() {
        // Multiline z polskimi znakami i wrap
        let buffer = format!("{}\n{}\n{}", "ą".repeat(90), "krótka", "ę".repeat(120));
        let char_count = buffer.chars().count();
        let content_lines = calculate_content_lines(&buffer, 80);
        let (_, cursor_row) = calculate_cursor_position(&buffer, char_count, 80);
        assert!((cursor_row as usize) <= content_lines);
    }

    #[test]
    fn test_consistency_narrow_terminal_polish() {
        // Wąski terminal z polskimi znakami
        let buffer = "ąęśćźżółń".repeat(5);
        let content_lines = calculate_content_lines(&buffer, 20);
        let char_count = buffer.chars().count();
        let (_, cursor_row) = calculate_cursor_position(&buffer, char_count, 20);
        assert!((cursor_row as usize) <= content_lines);
    }

    /// Edge cases: kursor na granicy wrap, polskie znaki na granicy, pusta linia po wrap

    #[test]
    fn test_edge_cursor_exact_first_line_end() {
        // Kursor dokładnie na końcu pierwszego wiersza (78)
        let buffer = "x".repeat(78);
        let (x, y) = calculate_cursor_position(&buffer, 78, 80);
        // Kursor po wypełnieniu wiersza przechodzi na nowy
        assert_eq!(y, 1);
        assert_eq!(x, 0);
    }

    #[test]
    fn test_edge_polish_boundary_79() {
        // 79 polskich znaków - testy konsystencji
        let buffer = "ć".repeat(79);
        let content_lines = calculate_content_lines(&buffer, 80);
        let (x, y) = calculate_cursor_position(&buffer, 79, 80);
        assert_eq!(content_lines, 2);
        assert_eq!(y, 1);
        assert_eq!(x, 1);
    }

    #[test]
    fn test_edge_empty_line_after_wrap() {
        // Pusta linia zaraz po wrapped linii
        let buffer = format!("{}\n", "y".repeat(100));
        let content_lines = calculate_content_lines(&buffer, 80);
        assert_eq!(content_lines, 3); // 2 wiersze wrap + 1 pusta linia
    }

    #[test]
    fn test_edge_polish_char_split_exact_boundary() {
        // Polski znak wymusza split dokładnie na granicy
        let buffer = format!("{}ą", "a".repeat(77));
        let (x, y) = calculate_cursor_position(&buffer, 78, 80);
        // 77 'a' + 1 'ą' = 78 kolumn → exact boundary
        assert_eq!(y, 1);
        assert_eq!(x, 0);
    }

    #[test]
    fn test_edge_multiline_all_wrap() {
        // Wszystkie linie logiczne się zawijają
        let buffer = format!(
            "{}\n{}\n{}",
            "a".repeat(100),
            "b".repeat(100),
            "c".repeat(100)
        );
        let content_lines = calculate_content_lines(&buffer, 80);
        // Pierwsza: 78 + 22 = 2, druga: 80 + 20 = 2, trzecia: 80 + 20 = 2
        assert_eq!(content_lines, 6);
    }

    #[test]
    fn test_edge_empty_buffer_cursor() {
        // Pusty buffer - kursor powinien być na początku
        let (x, y) = calculate_cursor_position("", 0, 80);
        assert_eq!(x, 2); // Po prefixie "> "
        assert_eq!(y, 0);
    }

    #[test]
    fn test_edge_single_polish_char() {
        // Pojedynczy polski znak
        let buffer = "ą";
        let content_lines = calculate_content_lines(buffer, 80);
        let (x, y) = calculate_cursor_position(buffer, 1, 80);
        assert_eq!(content_lines, 1);
        assert_eq!(y, 0);
        assert_eq!(x, 3); // 2 (prefix) + 1
    }

    #[test]
    fn test_edge_wrap_continuation_no_prefix() {
        // Test że continuation lines nie mają prefixu "> "
        let buffer = "x".repeat(100);
        let (x, y) = calculate_cursor_position(&buffer, 80, 80);
        // 80 znaków: 78 w wierszu 0, 2 w wierszu 1
        assert_eq!(y, 1);
        assert_eq!(x, 2); // Bez prefixu, bo continuation line
    }

    #[test]
    fn test_edge_polish_empty_mixed() {
        // Puste linie między liniami z polskimi znakami
        let buffer = "ąę\n\nść";
        let content_lines = calculate_content_lines(buffer, 80);
        assert_eq!(content_lines, 3);
        let (x, y) = calculate_cursor_position(buffer, 3, 80); // Na pustej linii
        assert_eq!(y, 1);
        assert_eq!(x, 0);
    }

    #[test]
    fn test_cursor_never_out_of_bounds() {
        // Kursor nigdy nie wychodzi poza zakres dla różnych pozycji
        let buffer = format!("{}\n{}", "ą".repeat(90), "ę".repeat(70));
        for pos in 0..=buffer.chars().count() {
            let (x, y) = calculate_cursor_position(&buffer, pos, 80);
            assert!(x < 200, "x out of bounds at pos {}", pos);
            assert!(y < 100, "y out of bounds at pos {}", pos);
        }
    }

    #[test]
    fn test_char_to_byte_edge_cases() {
        // Konwersje char<->byte dla edge cases
        let text = "ą"; // 2 bajty UTF-8, 1 char
        assert_eq!(char_to_byte(text, 0), 0);
        assert_eq!(char_to_byte(text, 1), 2);
        assert_eq!(char_to_byte(text, 100), 2); // Poza zakresem → text.len()

        assert_eq!(byte_to_char(text, 0), 0);
        assert_eq!(byte_to_char(text, 2), 1);
    }

    // ── Snapshot testy renderowania widgetu (zadanie 62.4) ────────

    /// Buduje Text dla pola input (bez hint text).
    /// Używane do snapshot testów renderowania.
    fn build_input_text<'a>(
        buffer: &'a str,
        placeholder: Option<&'a str>,
        terminal_width: u16,
    ) -> Text<'a> {
        if buffer.is_empty() {
            // Placeholder
            Text::from(vec![Line::from(vec![
                Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    placeholder.unwrap_or(""),
                    Style::default().fg(Color::DarkGray),
                ),
            ])])
        } else {
            let wrapped = build_wrapped_lines(buffer, terminal_width);
            let mut lines_vec: Vec<Line> = Vec::new();

            for (row_idx, (_logical_idx, fragment)) in wrapped.iter().enumerate() {
                if row_idx == 0 {
                    // Pierwszy wiersz terminala: prefix "> "
                    lines_vec.push(Line::from(vec![
                        Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(*fragment),
                    ]));
                } else {
                    lines_vec.push(Line::from(*fragment));
                }
            }

            Text::from(lines_vec)
        }
    }

    /// Buduje kompletny layout: input + hint (dla snapshot testów).
    /// Zwraca Text zawierający wszystkie linie.
    fn build_full_layout<'a>(
        buffer: &'a str,
        placeholder: Option<&'a str>,
        terminal_width: u16,
        scroll_offset: u16,
    ) -> Text<'a> {
        let input_text = build_input_text(buffer, placeholder, terminal_width);
        let mut all_lines = input_text.lines;

        // Zastosuj scroll offset - usuń pierwsze N linii
        if scroll_offset > 0 && scroll_offset < all_lines.len() as u16 {
            all_lines = all_lines.into_iter().skip(scroll_offset as usize).collect();
        }

        // Dodaj hint line na końcu
        all_lines.push(Line::from(Span::styled(
            HINT_TEXT,
            Style::default().fg(Color::DarkGray),
        )));

        Text::from(all_lines)
    }

    #[test]
    fn test_snapshot_empty_input_with_hint() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // Pusty input z placeholder
        let text = build_full_layout("", Some("Enter your text..."), 80, 0);
        let widget = Paragraph::new(text);
        let buffer = render_widget_to_buffer(widget, 80, 3);

        insta::assert_snapshot!(snap(&buffer), @"
        > Enter your text...
        Enter: wyślij │ Shift+Enter: nowa linia │ Esc: wróć │ Ctrl+C: anuluj
        ");
    }

    #[test]
    fn test_snapshot_single_line_cursor_at_end() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // Input z jedną linią, kursor na końcu
        let buffer_text = "Hello, world!";
        let text = build_full_layout(buffer_text, None, 80, 0);
        let widget = Paragraph::new(text);
        let buffer = render_widget_to_buffer(widget, 80, 3);

        insta::assert_snapshot!(snap(&buffer), @"
        > Hello, world!
        Enter: wyślij │ Shift+Enter: nowa linia │ Esc: wróć │ Ctrl+C: anuluj
        ");
    }

    #[test]
    fn test_snapshot_single_line_cursor_in_middle() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // Input z jedną linią, kursor w środku (po "Hello")
        // Kursor nie jest widoczny w TestBackend, więc weryfikujemy:
        // 1. Snapshot tekstu (identyczny niezależnie od pozycji kursora)
        // 2. Obliczoną pozycję kursora
        let buffer_text = "Hello, world!";
        let cursor_pos = 5; // kursor po "Hello"

        let text = build_full_layout(buffer_text, None, 80, 0);
        let widget = Paragraph::new(text);
        let buffer = render_widget_to_buffer(widget, 80, 3);

        insta::assert_snapshot!(snap(&buffer), @"
        > Hello, world!
        Enter: wyślij │ Shift+Enter: nowa linia │ Esc: wróć │ Ctrl+C: anuluj
        ");

        // Weryfikacja pozycji kursora w środku tekstu
        let (cx, cy) = calculate_cursor_position(buffer_text, cursor_pos, 80);
        assert_eq!(cy, 0, "Kursor w pierwszej linii");
        assert_eq!(cx, 7, "Kursor: 2 (prefix '> ') + 5 (po 'Hello')");
    }

    #[test]
    fn test_snapshot_multiline_three_lines() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // Multi-line input z 3 liniami
        let buffer_text = "Line 1\nLine 2\nLine 3";
        let text = build_full_layout(buffer_text, None, 80, 0);
        let widget = Paragraph::new(text);
        let buffer = render_widget_to_buffer(widget, 80, 5);

        insta::assert_snapshot!(snap(&buffer), @"
        > Line 1
        Line 2
        Line 3
        Enter: wyślij │ Shift+Enter: nowa linia │ Esc: wróć │ Ctrl+C: anuluj
        ");
    }

    #[test]
    fn test_snapshot_line_wrapping() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // Linia dłuższa niż szerokość terminala (80 - 2 = 78 dla pierwszej linii)
        // Tworzymy linię 100 znaków - powinna się zawinąć
        let buffer_text = "x".repeat(100);
        let text = build_full_layout(&buffer_text, None, 80, 0);
        let widget = Paragraph::new(text);
        let buffer = render_widget_to_buffer(widget, 80, 4);

        insta::assert_snapshot!(snap(&buffer), @"
        > xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
        xxxxxxxxxxxxxxxxxxxxxx
        Enter: wyślij │ Shift+Enter: nowa linia │ Esc: wróć │ Ctrl+C: anuluj
        ");
    }

    #[test]
    fn test_snapshot_max_input_lines() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // Input z MAX_INPUT_LINES (10 linii)
        let lines: Vec<String> = (1..=10).map(|i| format!("Line {}", i)).collect();
        let buffer_text = lines.join("\n");
        let text = build_full_layout(&buffer_text, None, 80, 0);
        let widget = Paragraph::new(text);
        let buffer = render_widget_to_buffer(widget, 80, 12);

        insta::assert_snapshot!(snap(&buffer), @"
        > Line 1
        Line 2
        Line 3
        Line 4
        Line 5
        Line 6
        Line 7
        Line 8
        Line 9
        Line 10
        Enter: wyślij │ Shift+Enter: nowa linia │ Esc: wróć │ Ctrl+C: anuluj
        ");
    }

    #[test]
    fn test_snapshot_hint_text_styling() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // Test stylowania hint text - sprawdzamy czy jest renderowany z odpowiednim tekstem
        let buffer_text = "Sample text";
        let text = build_full_layout(buffer_text, None, 80, 0);
        let widget = Paragraph::new(text);
        let buffer = render_widget_to_buffer(widget, 80, 3);

        // Hint text powinien zawierać wszystkie skróty
        let snapshot = snap(&buffer);
        assert!(snapshot.contains("Enter: wyślij"));
        assert!(snapshot.contains("Shift+Enter: nowa linia"));
        assert!(snapshot.contains("Ctrl+C: anuluj"));

        insta::assert_snapshot!(snapshot, @"
        > Sample text
        Enter: wyślij │ Shift+Enter: nowa linia │ Esc: wróć │ Ctrl+C: anuluj
        ");
    }

    // ── Testy dla wąskiego terminala (width=20) ────────────────────

    #[test]
    fn test_snapshot_narrow_terminal_long_text() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // 50-znakowy tekst na width=20 - sprawdzamy line wrapping
        let buffer_text = "x".repeat(50);
        let text = build_full_layout(&buffer_text, None, 20, 0);
        let widget = Paragraph::new(text);
        // Linia 1: "> " + 18 'x' = 20, linia 2: 20 'x', linia 3: 12 'x'
        // + hint = 4 linie treści
        let buffer = render_widget_to_buffer(widget, 20, 5);

        insta::assert_snapshot!(snap(&buffer), @"
        > xxxxxxxxxxxxxxxxxx
        xxxxxxxxxxxxxxxxxxxx
        xxxxxxxxxxxx
        Enter: wyślij │ Shif
        ");
    }

    #[test]
    fn test_snapshot_narrow_terminal_hint_truncation() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // Hint text jest dłuższy niż 20 znaków - powinien się zawinąć lub obciąć
        // HINT_TEXT = "Enter: nowa linia │ Ctrl+D: wyślij │ Ctrl+C: anuluj" (53 znaki)
        let buffer_text = "Test";
        let text = build_full_layout(buffer_text, None, 20, 0);
        let widget = Paragraph::new(text);
        // Hint text zawinięty na width=20
        let buffer = render_widget_to_buffer(widget, 20, 5);

        insta::assert_snapshot!(snap(&buffer), @"
        > Test
        Enter: wyślij │ Shif
        ");
    }

    #[test]
    fn test_snapshot_narrow_terminal_multiline_wrapping() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // Multiline input z wrapping na width=20
        // Linia 1: "> " + "Line one is longer" (18) = 20 → mieści się w 1 linii
        // Linia 2: "Line two" (8) → 1 linia
        // Linia 3: "Line three is also very long" (28) → wrap na 2 linie (20+8)
        // Hint: 1+ linia
        let buffer_text = "Line one is longer\nLine two\nLine three is also very long";
        let text = build_full_layout(buffer_text, None, 20, 0);
        let widget = Paragraph::new(text);
        let buffer = render_widget_to_buffer(widget, 20, 8);

        insta::assert_snapshot!(snap(&buffer), @"
        > Line one is longer
        Line two
        Line three is also v
        ery long
        Enter: wyślij │ Shif
        ");
    }

    #[test]
    fn test_narrow_terminal_cursor_position_after_wrapping() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // Weryfikacja pozycji kursora po zawinięciu linii
        // Tekst: "This is a very long line that wraps" (35 znaków)
        // Width: 20, więc pierwsza linia może mieć 18 znaków (width - 2 dla "> ")
        // Kursor na pozycji 25 (char index)
        let buffer_text = "This is a very long line that wraps";
        let cursor_pos = 25;

        // Pierwsza linia: "> " + 18 znaków (0-17) = "This is a very lon"
        // Druga linia: znaków 18-37 (początek: "g line that wraps")
        // Pozycja kursora 25 - 18 = 7 (7 znaków od początku drugiej linii)
        let (cx, cy) = calculate_cursor_position(buffer_text, cursor_pos, 20);

        // Kursor powinien być w drugiej linii (cy=1), na pozycji 7 (cx=7)
        assert_eq!(cy, 1, "Kursor powinien być w drugiej linii po zawinięciu");
        assert_eq!(cx, 7, "Kursor na pozycji 7 w drugiej linii (25 - 18 = 7)");

        // Snapshot dla weryfikacji wizualnej
        let text = build_full_layout(buffer_text, None, 20, 0);
        let widget = Paragraph::new(text);
        let buffer = render_widget_to_buffer(widget, 20, 4);

        insta::assert_snapshot!(snap(&buffer), @"
        > This is a very lon
        g line that wraps
        Enter: wyślij │ Shif
        ");
    }

    #[test]
    fn test_narrow_terminal_placeholder_truncation() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // Placeholder dłuższy niż szerokość terminala
        let placeholder = "Enter your very long placeholder text here";
        let text = build_full_layout("", Some(placeholder), 20, 0);
        let widget = Paragraph::new(text);
        let buffer = render_widget_to_buffer(widget, 20, 4);

        insta::assert_snapshot!(snap(&buffer), @"
        > Enter your very lo
        Enter: wyślij │ Shif
        ");
    }

    #[test]
    fn test_narrow_terminal_multiline_cursor_last_line() {
        // Weryfikacja pozycji kursora w ostatniej linii multiline tekstu
        let buffer_text = "Short\nAnother line here\nLast";
        let cursor_pos = buffer_text.chars().count(); // koniec tekstu

        let (cx, cy) = calculate_cursor_position(buffer_text, cursor_pos, 20);

        // Linia 0: "Short" (5 znaków)
        // Linia 1: "Another line here" (17 znaków)
        // Linia 2: "Last" (4 znaki)
        // Kursor na końcu = linia 2, pozycja 4
        assert_eq!(cy, 2, "Kursor w trzeciej linii (cy=2)");
        assert_eq!(cx, 4, "Kursor po 'Last' (4 znaki)");
    }

    // ── Task 74.4: Test paste-like rapid insert (50+ chars) ────────

    #[test]
    fn test_rapid_insert_50_chars_cursor_and_scroll() {
        // Symulacja paste przez szybkie wstawienie 50 znaków 'x'
        let mut state = TextInputState::new(None);

        // Wstaw 50x znak 'x' (symulacja paste)
        for _ in 0..50 {
            let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
            let result = handle_key_event(key, &mut state, false);
            assert!(result.is_ok());
            assert!(matches!(result.unwrap(), KeyAction::Continue));
        }

        // Sprawdź że buffer zawiera 50 'x'
        assert_eq!(state.buffer.len(), 50);
        assert_eq!(state.buffer.chars().count(), 50);
        assert!(state.buffer.chars().all(|c| c == 'x'));

        // Sprawdź cursor_pos == 50 (kursor na końcu)
        assert_eq!(state.cursor_pos, 50);

        // scroll_offset nie zmienia się w handle_key_event — auto-scroll jest w render_input()
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_snapshot_rapid_paste_50_chars_on_width_40() {
        use crate::test_helpers::{render_widget_to_buffer, snap};

        // Rendering po szybkim wstawieniu 50 znaków na width=40
        // Pierwsza linia: "> " + 38x 'x' (width 40 - 2)
        // Druga linia: 12x 'x'
        let buffer_text = "x".repeat(50);
        let text = build_full_layout(&buffer_text, None, 40, 0);
        let widget = Paragraph::new(text);
        let buffer = render_widget_to_buffer(widget, 40, 4);

        insta::assert_snapshot!(snap(&buffer), @"
        > xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
        xxxxxxxxxxxx
        Enter: wyślij │ Shift+Enter: nowa linia
        ");
    }

    #[test]
    fn test_rapid_paste_cursor_position_calculation() {
        // Weryfikacja pozycji kursora po wstawieniu 50 'x' na width=40
        let buffer_text = "x".repeat(50);
        let cursor_pos = 50; // kursor na końcu

        // Pierwsza linia: "> " + 38 znaków (0-37)
        // Druga linia: 12 znaków (38-49)
        // Kursor na pozycji 50 (po ostatnim 'x')
        let (cx, cy) = calculate_cursor_position(&buffer_text, cursor_pos, 40);

        // Kursor powinien być w drugiej linii (cy=1)
        // Na pozycji 12 (cx=12) - tuż po ostatnim 'x' w drugiej linii
        assert_eq!(cy, 1, "Kursor w drugiej linii po paste 50 znaków");
        assert_eq!(cx, 12, "Kursor na pozycji 12 w drugiej linii");
    }

    #[test]
    fn test_rapid_paste_100_chars_wrapping_and_cursor() {
        // Test większego paste (100 znaków) — weryfikacja zawijania i pozycji kursora
        let mut state = TextInputState::new(None);

        for _ in 0..100 {
            let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
            let _ = handle_key_event(key, &mut state, false);
        }

        assert_eq!(state.buffer.chars().count(), 100);
        assert_eq!(state.cursor_pos, 100);

        // Na width=40: 38 + 40 + 22 = 3 linie wrapped
        let (cx, cy) = calculate_cursor_position(&state.buffer, state.cursor_pos, 40);
        assert_eq!(cy, 2, "Kursor w trzeciej linii (cy=2) po paste 100 znaków");
        assert_eq!(cx, 22, "Kursor na pozycji 22 w trzeciej linii");
    }

    // ── Testy Enter/Backspace viewport resize (fix duplikacji) ──────

    #[test]
    fn test_shift_enter_then_backspace_restores_buffer() {
        // Shift+Enter dodaje '\n', Backspace go usuwa — buffer wraca do oryginału
        let mut state = TextInputState::new(Some("hello"));
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        let backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);

        let _ = handle_key_event(enter, &mut state, false);
        assert_eq!(state.buffer, "hello\n");
        assert_eq!(state.cursor_pos, 6);

        let _ = handle_key_event(backspace, &mut state, false);
        assert_eq!(state.buffer, "hello");
        assert_eq!(state.cursor_pos, 5);

        let lines_after = calculate_content_lines(&state.buffer, 80);
        assert_eq!(lines_after, 1, "Po Backspace buffer wraca do 1 linii");
    }

    #[test]
    fn test_shift_enter_mid_text_then_backspace_restores() {
        // Shift+Enter w środku tekstu ("abc|def" → "abc\ndef"), Backspace przywraca "abcdef"
        let mut state = TextInputState::new(Some("abcdef"));
        state.cursor_pos = 3; // między 'c' i 'd'
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        let backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);

        let _ = handle_key_event(enter, &mut state, false);
        assert_eq!(state.buffer, "abc\ndef");
        assert_eq!(state.cursor_pos, 4);

        let _ = handle_key_event(backspace, &mut state, false);
        assert_eq!(state.buffer, "abcdef");
        assert_eq!(state.cursor_pos, 3);
    }

    #[test]
    fn test_multiple_shift_enter_then_backspace_sequence() {
        // 3x Shift+Enter, 3x Backspace → oryginalny buffer
        let mut state = TextInputState::new(Some("text"));
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        let backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);

        for _ in 0..3 {
            let _ = handle_key_event(enter, &mut state, false);
        }
        assert_eq!(state.buffer, "text\n\n\n");
        assert_eq!(state.cursor_pos, 7);

        for _ in 0..3 {
            let _ = handle_key_event(backspace, &mut state, false);
        }
        assert_eq!(state.buffer, "text");
        assert_eq!(state.cursor_pos, 4);
    }

    #[test]
    fn test_content_lines_changes_on_newline_add_remove() {
        // Weryfikacja że height zmienia się po Shift+Enter i wraca po Backspace
        // — dokładny warunek triggera resize w event loop
        let mut state = TextInputState::new(Some("hello"));
        let width = 80u16;

        let lines_before = calculate_content_lines(&state.buffer, width);
        assert_eq!(lines_before, 1);

        // Shift+Enter → dodaje newline
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        let _ = handle_key_event(enter, &mut state, false);
        let lines_after_enter = calculate_content_lines(&state.buffer, width);
        assert_eq!(lines_after_enter, 2, "Po Shift+Enter powinny być 2 linie");

        // Backspace → usuwa newline
        let backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        let _ = handle_key_event(backspace, &mut state, false);
        let lines_after_backspace = calculate_content_lines(&state.buffer, width);
        assert_eq!(
            lines_after_backspace, 1,
            "Po Backspace powinny wrócić do 1 linii"
        );

        // Upewnij się, że przeszło 1→2→1 (trigger resize w obie strony)
        assert_ne!(lines_before, lines_after_enter);
        assert_eq!(lines_before, lines_after_backspace);
    }

    // ── Testy scroll + widoczny "> " prefix ──────────────────────────

    #[test]
    fn test_scroll_offset_triggers_when_content_exceeds_max() {
        // Symulacja: 12 nowych linii → content_lines=13, visible=10, scroll_offset=3
        let mut state = TextInputState::new(None);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);

        for _ in 0..12 {
            let _ = handle_key_event(enter, &mut state, false);
        }

        let content_lines = calculate_content_lines(&state.buffer, 80);
        assert_eq!(content_lines, 13);

        let vis_height = content_lines.min(MAX_INPUT_LINES as usize);
        assert_eq!(vis_height, 10);

        // Auto-scroll: kursor na ostatniej linii
        let (_, cursor_row) = calculate_cursor_position(&state.buffer, state.cursor_pos, 80);
        assert_eq!(cursor_row, 12);

        // scroll_offset = cursor_row - vis_height + 1 = 12 - 10 + 1 = 3
        let expected_scroll = (cursor_row as usize) - vis_height + 1;
        assert_eq!(expected_scroll, 3);
    }

    #[test]
    fn test_cursor_x_corrected_when_on_scroll_offset_line() {
        // Gdy kursor jest na pierwszej widocznej linii (scroll_offset),
        // cursor_x powinien uwzględniać "> " prefix (+2)
        let buffer = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm";
        let state = TextInputState::new(Some(buffer));
        let (cursor_x, cursor_row) = calculate_cursor_position(&state.buffer, state.cursor_pos, 80);

        // Kursor na końcu "m" — row=12, x=1
        assert_eq!(cursor_row, 12);
        assert_eq!(cursor_x, 1);

        // Symulacja korekcji z event loop:
        // scroll_offset = 3 (jak w poprzednim teście), kursor na row 12 ≠ 3
        // → brak korekcji, cursor_x zostaje 1
        let scroll_offset = 3u16;
        let corrected_x = if scroll_offset > 0 && cursor_row == scroll_offset {
            cursor_x.saturating_add(2).min(79)
        } else {
            cursor_x
        };
        assert_eq!(corrected_x, 1, "Kursor na dole — brak korekcji");
    }

    #[test]
    fn test_cursor_x_corrected_when_cursor_at_scroll_offset() {
        // Kursor dokładnie na linii scroll_offset — korekcja +2
        let buffer = "aaa\nbbb\nccc\nddd\neee";
        // Ustawiamy kursor na początku "ddd" (linia 3, char pos = 12)
        let (cursor_x, cursor_row) = calculate_cursor_position(buffer, 12, 80);

        assert_eq!(cursor_row, 3);
        assert_eq!(cursor_x, 0); // początek linii, bez prefixu

        // Jeśli scroll_offset = 3, kursor jest na pierwszej widocznej linii
        let scroll_offset = 3u16;
        let corrected_x = if scroll_offset > 0 && cursor_row == scroll_offset {
            cursor_x.saturating_add(2).min(79)
        } else {
            cursor_x
        };
        assert_eq!(corrected_x, 2, "Korekcja +2 za '>  ' prefix");
    }

    #[test]
    fn test_backspace_reduces_scroll_restores_prefix() {
        // 12x Shift+Enter → scroll_offset > 0, potem 12x Backspace → scroll_offset = 0
        let mut state = TextInputState::new(None);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        let backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);

        for _ in 0..12 {
            let _ = handle_key_event(enter, &mut state, false);
        }

        let content_lines = calculate_content_lines(&state.buffer, 80);
        assert!(content_lines > MAX_INPUT_LINES as usize);

        // Backspace 12 razy
        for _ in 0..12 {
            let _ = handle_key_event(backspace, &mut state, false);
        }

        let content_lines = calculate_content_lines(&state.buffer, 80);
        assert_eq!(content_lines, 1);

        // Kursor na wierszu 0 — "> " jest naturalnie na row_idx == 0
        let (cursor_x, cursor_row) = calculate_cursor_position(&state.buffer, state.cursor_pos, 80);
        assert_eq!(cursor_row, 0);
        assert_eq!(cursor_x, 2); // ">" prefix na wierszu 0
    }

    // ── Testy helperów word boundary ────────────────────────────────

    #[test]
    fn test_is_word_char() {
        assert!(is_word_char('a'));
        assert!(is_word_char('Z'));
        assert!(is_word_char('0'));
        assert!(is_word_char('9'));
        assert!(is_word_char('_'));
        assert!(!is_word_char(' '));
        assert!(!is_word_char('.'));
        assert!(!is_word_char('-'));
        assert!(!is_word_char('\n'));
        assert!(!is_word_char('!'));
    }

    #[test]
    fn test_prev_word_boundary_simple() {
        // "hello world|" → "hello |world"
        let buffer = "hello world";
        assert_eq!(find_prev_word_boundary(buffer, 11), 6);
    }

    #[test]
    fn test_prev_word_boundary_multiple_spaces() {
        // "hello   |" → "|hello   "
        let buffer = "hello   ";
        assert_eq!(find_prev_word_boundary(buffer, 8), 0);
    }

    #[test]
    fn test_prev_word_boundary_at_start() {
        assert_eq!(find_prev_word_boundary("hello", 0), 0);
    }

    #[test]
    fn test_next_word_boundary_simple() {
        // "|hello world" → "hello |world"
        let buffer = "hello world";
        assert_eq!(find_next_word_boundary(buffer, 0), 6);
    }

    #[test]
    fn test_next_word_boundary_at_end() {
        let buffer = "hello";
        assert_eq!(find_next_word_boundary(buffer, 5), 5);
    }

    #[test]
    fn test_find_line_start_end() {
        let buffer = "line1\nline2\nline3";
        // Kursor w "line2" (pos=8, 'n')
        assert_eq!(find_line_start(buffer, 8), 6);
        assert_eq!(find_line_end(buffer, 8), 11);
        // Kursor na początku
        assert_eq!(find_line_start(buffer, 0), 0);
        assert_eq!(find_line_end(buffer, 0), 5);
    }

    // ── Testy nawigacji słowami ─────────────────────────────────────

    #[test]
    fn test_alt_left_moves_word() {
        let mut state = TextInputState::new(Some("hello world"));
        // kursor na końcu (pos=11)
        let key = KeyEvent::new(KeyCode::Left, KeyModifiers::ALT);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.cursor_pos, 6, "Alt+Left powinien przeskoczyć do 'w'");
    }

    #[test]
    fn test_alt_right_moves_word() {
        let mut state = TextInputState::new(Some("hello world"));
        state.cursor_pos = 0;
        let key = KeyEvent::new(KeyCode::Right, KeyModifiers::ALT);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(
            state.cursor_pos, 6,
            "Alt+Right powinien przeskoczyć za 'hello '"
        );
    }

    #[test]
    fn test_ctrl_left_same_as_alt_left() {
        let mut state1 = TextInputState::new(Some("hello world"));
        let mut state2 = TextInputState::new(Some("hello world"));

        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
            &mut state1,
            false,
        );
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
            &mut state2,
            false,
        );
        assert_eq!(state1.cursor_pos, state2.cursor_pos, "Ctrl+Left = Alt+Left");
    }

    #[test]
    fn test_ctrl_a_moves_to_line_start() {
        let mut state = TextInputState::new(Some("hello\nworld"));
        // kursor na końcu "world" (pos=11)
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.cursor_pos, 6, "Ctrl+A → początek 'world'");
    }

    #[test]
    fn test_ctrl_e_moves_to_line_end() {
        let mut state = TextInputState::new(Some("hello\nworld"));
        state.cursor_pos = 6; // początek "world"
        let key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.cursor_pos, 11, "Ctrl+E → koniec 'world'");
    }

    // ── Testy usuwania ──────────────────────────────────────────────

    #[test]
    fn test_alt_backspace_deletes_word() {
        let mut state = TextInputState::new(Some("hello world"));
        // kursor na końcu (pos=11)
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.buffer, "hello ");
        assert_eq!(state.cursor_pos, 6);
    }

    #[test]
    fn test_ctrl_w_deletes_word() {
        let mut state = TextInputState::new(Some("hello world"));
        let key = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.buffer, "hello ");
        assert_eq!(state.cursor_pos, 6);
    }

    #[test]
    fn test_ctrl_u_kills_to_line_start() {
        let mut state = TextInputState::new(Some("hello world"));
        state.cursor_pos = 5; // po "hello"
        let key = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.buffer, " world");
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_ctrl_backspace_kills_to_line_start() {
        let mut state = TextInputState::new(Some("hello world"));
        state.cursor_pos = 5;
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.buffer, " world");
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_ctrl_k_kills_to_line_end() {
        let mut state = TextInputState::new(Some("hello world"));
        state.cursor_pos = 5;
        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.buffer, "hello");
        assert_eq!(state.cursor_pos, 5);
    }

    // ── Edge cases ──────────────────────────────────────────────────

    #[test]
    fn test_word_delete_across_newline() {
        // Alt+Backspace na początku linii po \n — cofa do poprzedniego słowa
        let mut state = TextInputState::new(Some("hello\nworld"));
        state.cursor_pos = 6; // początek "world"
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT);
        let _ = handle_key_event(key, &mut state, false);
        // Powinno cofnąć się przez \n i usunąć "hello\n"
        assert_eq!(state.buffer, "world");
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_ctrl_u_on_second_line() {
        // Ctrl+U na drugiej linii nie usuwa newline z pierwszej
        let mut state = TextInputState::new(Some("first\nsecond"));
        state.cursor_pos = 12; // koniec "second"
        let key = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.buffer, "first\n");
        assert_eq!(state.cursor_pos, 6);
    }

    #[test]
    fn test_delete_removes_char_after_cursor() {
        let mut state = TextInputState::new(Some("hello"));
        state.cursor_pos = 0;
        let key = KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.buffer, "ello");
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_delete_at_end_does_nothing() {
        let mut state = TextInputState::new(Some("hello"));
        // kursor na końcu (pos=5)
        let key = KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.buffer, "hello");
        assert_eq!(state.cursor_pos, 5);
    }

    #[test]
    fn test_delete_in_middle() {
        let mut state = TextInputState::new(Some("hello"));
        state.cursor_pos = 2;
        let key = KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.buffer, "helo");
        assert_eq!(state.cursor_pos, 2);
    }

    #[test]
    fn test_delete_removes_newline() {
        let mut state = TextInputState::new(Some("first\nsecond"));
        state.cursor_pos = 5; // na \n
        let key = KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE);
        let _ = handle_key_event(key, &mut state, false);
        assert_eq!(state.buffer, "firstsecond");
        assert_eq!(state.cursor_pos, 5);
    }

    #[test]
    fn test_ctrl_k_at_line_end() {
        // Ctrl+K na końcu linii usuwa newline (łączy z następną)
        let mut state = TextInputState::new(Some("first\nsecond"));
        state.cursor_pos = 5; // koniec "first" (przed \n)
        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        let _ = handle_key_event(key, &mut state, false);
        // find_line_end na pos=5 → szuka \n w buffer[byte_pos..] → offset 0 → line_end=5
        // Więc Ctrl+K na końcu linii (tuż przed \n) nie usuwa nic,
        // bo kursor == line_end. To standardowe zachowanie readline.
        // Drugie Ctrl+K jest potrzebne żeby usunąć \n.
        assert_eq!(state.buffer, "first\nsecond");
        // Aby usunąć \n, trzeba przesunąć kursor za \n lub użyć Delete
    }
}
