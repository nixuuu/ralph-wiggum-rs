/// Standalone text input widget oparty na MultilineTextInputState.
///
/// Wyświetlany gdy user nie poda --prompt/--file/stdin.
/// Używa ratatui Viewport::Inline — osadzony w bieżącym terminalu.
///
/// Shift+Enter = nowa linia, Enter = wyślij.
/// Visual wrapping: content_width = terminal_width - 2 (dla prefixu "> ").
/// Vertical scroll: auto-follow za kursorem (MultilineTextInputState).
use crate::shared::error::{RalphError, Result};
use crate::tui::widgets::multiline_text_input::MultilineTextInputState;
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
use unicode_width::UnicodeWidthChar;

// ── Stałe ─────────────────────────────────────────────────────────────────

/// Maksymalna liczba widocznych linii inputu (bez headera i hinta).
const MAX_INPUT_LINES: u16 = 10;

/// Hint ze skrótami klawiszowymi wyświetlany pod polem input.
const HINT_TEXT: &str = "Enter: wyślij │ Shift+Enter: nowa linia │ Esc: wróć │ Ctrl+C: anuluj";

// ── Pomocnicze funkcje ─────────────────────────────────────────────────────

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

/// RAII guard dla raw mode — wyłącza raw mode przy drop.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

// ── Word operation helpers ─────────────────────────────────────────────────

/// Czy znak jest częścią "słowa" (litery, cyfry, underscore).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Usuwa słowo wstecz od kursora (readline-style Ctrl+W).
/// Nie przekracza granicy bieżącej linii.
fn delete_word_backward(state: &mut MultilineTextInputState) {
    let (row, col) = state.cursor();
    if col == 0 {
        return; // Na początku linii — nic do usunięcia w bieżącej linii
    }

    let buf = state.buffer().to_string();
    let line = buf.split('\n').nth(row).unwrap_or("");
    let chars: Vec<char> = line.chars().take(col).collect();
    let mut pos = chars.len();

    // Pomiń non-word chars wstecz
    while pos > 0 && !is_word_char(chars[pos - 1]) {
        pos -= 1;
    }
    // Usuń word chars wstecz
    while pos > 0 && is_word_char(chars[pos - 1]) {
        pos -= 1;
    }

    let count = chars.len() - pos;
    for _ in 0..count {
        state.delete_char();
    }
}

/// Usuwa od kursora do początku bieżącej linii (Ctrl+U).
fn delete_to_line_start(state: &mut MultilineTextInputState) {
    let (_, col) = state.cursor();
    for _ in 0..col {
        state.delete_char();
    }
}

/// Usuwa od kursora do końca bieżącej linii (Ctrl+K).
fn delete_to_line_end(state: &mut MultilineTextInputState) {
    let (row, col) = state.cursor();
    let buf = state.buffer().to_string();
    let line_len = buf.split('\n').nth(row).map_or(0, |l| l.chars().count());
    let count = line_len.saturating_sub(col);
    for _ in 0..count {
        state.delete_char_forward();
    }
}

/// Przesuwa kursor o słowo w lewo (readline Alt+Left / Ctrl+Left).
/// Może przekraczać granice linii.
fn move_word_backward(state: &mut MultilineTextInputState) {
    let buf = state.buffer().to_string();
    let (row, col) = state.cursor();

    // Oblicz flat char position kursora w całym buforze
    let flat_pos: usize = buf
        .split('\n')
        .take(row)
        .map(|l| l.chars().count() + 1)
        .sum::<usize>()
        + col;

    let chars: Vec<char> = buf.chars().collect();
    let mut pos = flat_pos;

    // Pomiń non-word chars wstecz
    while pos > 0 && !is_word_char(chars[pos - 1]) {
        pos -= 1;
    }
    // Cofaj przez word chars
    while pos > 0 && is_word_char(chars[pos - 1]) {
        pos -= 1;
    }

    let steps = flat_pos - pos;
    for _ in 0..steps {
        state.move_cursor_left();
    }
}

/// Przesuwa kursor o słowo w prawo (readline Alt+Right / Ctrl+Right).
/// Może przekraczać granice linii.
fn move_word_forward(state: &mut MultilineTextInputState) {
    let buf = state.buffer().to_string();
    let (row, col) = state.cursor();

    // Oblicz flat char position kursora w całym buforze
    let flat_pos: usize = buf
        .split('\n')
        .take(row)
        .map(|l| l.chars().count() + 1)
        .sum::<usize>()
        + col;

    let chars: Vec<char> = buf.chars().collect();
    let len = chars.len();
    let mut pos = flat_pos;

    // Przejdź przez word chars
    while pos < len && is_word_char(chars[pos]) {
        pos += 1;
    }
    // Pomiń non-word chars
    while pos < len && !is_word_char(chars[pos]) {
        pos += 1;
    }

    let steps = pos - flat_pos;
    for _ in 0..steps {
        state.move_cursor_right();
    }
}

// ── Key event handling ─────────────────────────────────────────────────────

/// Wynik obsługi klawisza.
#[derive(Debug)]
enum KeyAction {
    Continue,
    Submit,
    Back,
}

/// Obsługuje pojedyncze zdarzenie klawiatury.
///
/// Deleguje podstawową nawigację i edycję do [`MultilineTextInputState::handle_key_event`].
/// Obsługuje dodatkowo: Ctrl+C, Ctrl+D, Esc, Alt+Enter, Ctrl+J, Ctrl+W/U/K,
/// word navigation (Alt+Left, Ctrl+Left, Alt+Right, Ctrl+Right).
fn handle_key_event(
    key: KeyEvent,
    state: &mut MultilineTextInputState,
    required: bool,
) -> Result<KeyAction> {
    match (key.code, key.modifiers) {
        // Ctrl+C: anuluj sesję
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
            Err(RalphError::Interrupted)
        }

        // Ctrl+D: submit (jak EOF w terminalu)
        (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => {
            if !required || !state.buffer().trim().is_empty() {
                Ok(KeyAction::Submit)
            } else {
                Ok(KeyAction::Continue)
            }
        }

        // Esc: wróć (nawigacja wstecz)
        (KeyCode::Esc, _) => Ok(KeyAction::Back),

        // Alt+Enter: wstaw newline (alternatywny skrót)
        (KeyCode::Enter, m)
            if m.contains(KeyModifiers::ALT) && !m.contains(KeyModifiers::SHIFT) =>
        {
            state.insert_newline();
            Ok(KeyAction::Continue)
        }

        // Ctrl+J: wstaw newline (Unix LF — alternatywa dla Shift+Enter)
        (KeyCode::Char('j'), m) if m.contains(KeyModifiers::CONTROL) => {
            state.insert_newline();
            Ok(KeyAction::Continue)
        }

        // Ctrl+W: usuń słowo wstecz
        (KeyCode::Char('w'), m) if m.contains(KeyModifiers::CONTROL) => {
            delete_word_backward(state);
            Ok(KeyAction::Continue)
        }

        // Ctrl+U: usuń do początku linii
        (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => {
            delete_to_line_start(state);
            Ok(KeyAction::Continue)
        }

        // Ctrl+K: usuń do końca linii
        (KeyCode::Char('k'), m) if m.contains(KeyModifiers::CONTROL) => {
            delete_to_line_end(state);
            Ok(KeyAction::Continue)
        }

        // Ctrl+Backspace / Alt+Backspace: usuń słowo wstecz
        (KeyCode::Backspace, m)
            if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::ALT) =>
        {
            delete_word_backward(state);
            Ok(KeyAction::Continue)
        }

        // Alt+Left / Ctrl+Left: skok o słowo w lewo
        (KeyCode::Left, m)
            if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL) =>
        {
            move_word_backward(state);
            Ok(KeyAction::Continue)
        }

        // Alt+Right / Ctrl+Right: skok o słowo w prawo
        (KeyCode::Right, m)
            if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL) =>
        {
            move_word_forward(state);
            Ok(KeyAction::Continue)
        }

        // Pozostałe Ctrl+key — ignoruj (zapobiega wstawianiu liter do bufora)
        (KeyCode::Char(_), m) if m.contains(KeyModifiers::CONTROL) => Ok(KeyAction::Continue),

        // Pozostałe klawisze: deleguj do MultilineTextInputState
        // Enter → Submit (sprawdź required), Shift+Enter → newline, inne → Continue
        _ => match state.handle_key_event(key) {
            Some(_) => {
                // Enter naciśnięty — sprawdź required
                if !required || !state.buffer().trim().is_empty() {
                    Ok(KeyAction::Submit)
                } else {
                    Ok(KeyAction::Continue)
                }
            }
            None => Ok(KeyAction::Continue),
        },
    }
}

// ── Rendering helpers ──────────────────────────────────────────────────────

/// Oblicza display column kursora w wierszu terminala.
///
/// `char_offset` to char offset początku tego fragmentu visual line w logical line.
/// `cursor_log_col` to kolumna kursora w logical line (char index).
/// `_is_first_visible_row`: zachowany dla kompatybilności; wszystkie linie mają
/// 2-kolumnowy prefix (`"> "` lub `"  "`), więc zawsze dodajemy 2.
fn cursor_display_col(
    logical_line: &str,
    char_offset: usize,
    cursor_log_col: usize,
    _is_first_visible_row: bool,
) -> u16 {
    let col_in_frag = cursor_log_col.saturating_sub(char_offset);
    let disp_x: u16 = logical_line
        .chars()
        .skip(char_offset)
        .take(col_in_frag)
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0) as u16)
        .sum();
    disp_x + 2 // prefix "> " (pierwsza linia) lub "  " (continuation) — zawsze 2 kolumny
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Renderuje pytanie tekstowe z headerem osadzonym w viewporcie.
///
/// Wrapper dla [`text_input`] z headerem. Błąd Back jest konwertowany na Interrupted.
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

/// Minimalny text input z ratatui Viewport::Inline.
///
/// Obsługuje:
/// - **Enter**: submit
/// - **Shift+Enter, Alt+Enter, Ctrl+J**: nowa linia
/// - **Ctrl+C**: przerwanie (Interrupted)
/// - **Ctrl+D**: submit (jak EOF)
/// - **Esc**: powrót (Back)
/// - **Ctrl+W/U/K**: usuwanie słów/linii
/// - **Alt+Left/Right, Ctrl+Left/Right**: word navigation
/// - **Strzałki, Home/End, Ctrl+A/E**: nawigacja
///
/// Visual wrapping: `content_width = terminal_width - 2`.
/// Vertical scroll: auto-follow za kursorem (MultilineTextInputState).
///
/// Gdy `header` jest Some, tekst pytania jest renderowany w górnej części viewport.
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

    // Inicjalizuj stan z opcjonalnym default value
    let mut state = match default {
        Some(d) if !d.is_empty() => MultilineTextInputState::with_content(d),
        _ => MultilineTextInputState::new(),
    };

    enable_raw_mode().map_err(|e| RalphError::Mcp(format!("Failed to enable raw mode: {e}")))?;
    let _guard = RawModeGuard;

    let (mut terminal_width, _) = crossterm::terminal::size()
        .map_err(|e| RalphError::Mcp(format!("Failed to get terminal size: {e}")))?;

    // content_width = terminal_width - 2 (dla prefixu "> " i "  ")
    let mut content_width = terminal_width.saturating_sub(2) as usize;

    // Wstępna konfiguracja stanu
    state.set_wrap_width(content_width);
    let hint_lines = 1u16;
    let initial_vis_lines = state.visual_lines().len().min(MAX_INPUT_LINES as usize) as u16;
    let initial_height = header_lines + initial_vis_lines + hint_lines;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(initial_height),
        },
    )
    .map_err(|e| RalphError::Mcp(format!("Failed to create terminal: {e}")))?;

    let mut last_height = initial_height;
    let mut viewport_y = 0u16;

    loop {
        // Zaktualizuj wrap_width (spójność z terminal_width)
        state.set_wrap_width(content_width);

        // Oblicz nową wysokość viewportu
        let total_visual = state.visual_lines().len();
        let visible_input_lines = total_visual.min(MAX_INPUT_LINES as usize) as u16;
        // Ustaw viewport height w stanie — wywołuje clamp_scroll()
        state.set_viewport_height(visible_input_lines as usize);
        let new_height = header_lines + visible_input_lines + hint_lines;

        // Przebuduj terminal jeśli wysokość się zmieniła
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

        // ── Snapshot danych do renderowania (przed draw closure) ──────────

        let (cursor_log_row, cursor_log_col) = state.cursor();
        let cursor_vl_idx = state.cursor_visual_line_index().unwrap_or(0);
        let scroll_off = state.scroll_offset();
        let visible_cursor_row = cursor_vl_idx.saturating_sub(scroll_off);

        // Widoczne visual lines dla bieżącego viewport
        let visible_lines_snap = state.visible_lines_viewport();

        // Owned buffer snapshot — unikamy borrow konfliktu w closure
        let buf_snap = state.buffer().to_string();
        let is_empty_buffer = buf_snap.is_empty();

        // Oblicz display column kursora
        let all_visual_lines = state.visual_lines();
        let vl_char_offset = all_visual_lines
            .get(cursor_vl_idx)
            .map(|vl| vl.char_offset)
            .unwrap_or(0);
        // Logiczna linia dla obliczenia kursora
        let logical_line_for_cursor = if is_empty_buffer {
            String::new()
        } else {
            buf_snap
                .split('\n')
                .nth(cursor_log_row)
                .unwrap_or("")
                .to_string()
        };
        let cursor_x = cursor_display_col(
            &logical_line_for_cursor,
            vl_char_offset,
            cursor_log_col,
            visible_cursor_row == 0,
        );

        // ── Draw ──────────────────────────────────────────────────────────

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

                let bold_style = Style::default().add_modifier(Modifier::BOLD);
                let muted_style = Style::default().fg(Color::DarkGray);

                // Zbuduj tekst do wyświetlenia z widocznych visual lines
                let display_text = if is_empty_buffer {
                    // Placeholder gdy bufor pusty
                    Text::from(vec![Line::from(vec![
                        Span::styled("> ", bold_style),
                        Span::styled(placeholder.unwrap_or(""), muted_style),
                    ])])
                } else {
                    // Linie z bufora — każda to fragment visual line z opcjonalnym prefixem
                    let logical_lines: Vec<&str> = buf_snap.split('\n').collect();
                    let mut lines_vec: Vec<Line> = Vec::new();

                    for (screen_idx, vl) in visible_lines_snap.iter().enumerate() {
                        let ll = logical_lines.get(vl.logical_row).copied().unwrap_or("");
                        let frag: String = ll
                            .chars()
                            .skip(vl.char_offset)
                            .take(vl.char_count)
                            .collect();

                        // Pierwsza widoczna linia: prefix "> ", continuation: "  " (2 spacje)
                        // Obie opcje zajmują 2 kolumny — wyrównanie wizualne.
                        if screen_idx == 0 {
                            lines_vec.push(Line::from(vec![
                                Span::styled("> ", bold_style),
                                Span::raw(frag),
                            ]));
                        } else {
                            lines_vec.push(Line::from(vec![
                                Span::raw("  "),
                                Span::raw(frag),
                            ]));
                        }
                    }
                    Text::from(lines_vec)
                };

                // Renderuj input (linie już zawinięte do content_width — bez Wrap)
                frame.render_widget(Paragraph::new(display_text), input_area);

                // Hint ze skrótami klawiszowymi
                let hint = Paragraph::new(Line::from(Span::styled(HINT_TEXT, muted_style)));
                frame.render_widget(hint, hint_area);

                // Ustaw pozycję kursora
                frame.set_cursor_position((cursor_x, input_area.y + visible_cursor_row as u16));
            })
            .map_err(|e| RalphError::Mcp(format!("Failed to draw: {e}")))?;

        // ── Event drain ───────────────────────────────────────────────────
        // Przetwórz WSZYSTKIE oczekujące eventy przed następnym resize+draw.
        // Zapobiega artefaktom wizualnym przy szybkim wpisywaniu.
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
                            return Ok(state.buffer().to_string());
                        }
                        Ok(KeyAction::Back) => {
                            drop(terminal);
                            collapse_viewport(viewport_y)?;
                            return Err(RalphError::Back);
                        }
                        Ok(KeyAction::Continue) => {}
                    }
                } else if let Event::Resize(new_width, _) = ev {
                    // Zaktualizuj content_width po zmianie rozmiaru terminala.
                    // state.set_wrap_width() zostanie wywołany na początku następnej iteracji.
                    terminal_width = new_width;
                    content_width = terminal_width.saturating_sub(2) as usize;
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

/// Convenience wrapper dla standalone text input bez headera.
///
/// Wywołuje [`text_input`] bez header i default value.
/// Błąd Back konwertowany na Interrupted (użytkownik nie może "cofnąć" do niczego).
#[allow(dead_code)]
pub fn standalone_text_input(placeholder: Option<&str>, required: bool) -> Result<String> {
    match text_input(placeholder, None, required, None) {
        Err(RalphError::Back) => Err(RalphError::Interrupted),
        other => other,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper ───────────────────────────────────────────────────────────

    /// Oblicza flat char position z (row, col) w MultilineTextInputState.
    /// Używane do weryfikacji pozycji kursora w testach.
    fn flat_pos(state: &MultilineTextInputState) -> usize {
        let (row, col) = state.cursor();
        state
            .buffer()
            .split('\n')
            .take(row)
            .map(|l| l.chars().count() + 1)
            .sum::<usize>()
            + col
    }

    // ── handle_key_event: podstawowe akcje ───────────────────────────────

    #[test]
    fn test_esc_returns_back() {
        let mut state = MultilineTextInputState::with_content("test");
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Back));
        assert_eq!(state.buffer(), "test");
        assert_eq!(state.cursor(), (0, 4));
    }

    #[test]
    fn test_handle_key_event_enter_submits() {
        let mut state = MultilineTextInputState::with_content("test");
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Submit));
        assert_eq!(state.buffer(), "test");
    }

    #[test]
    fn test_handle_key_event_enter_ignored_when_empty_and_required() {
        let mut state = MultilineTextInputState::new();
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(state.buffer(), "");
    }

    #[test]
    fn test_handle_key_event_enter_submits_when_not_required() {
        let mut state = MultilineTextInputState::new();
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Submit));
    }

    #[test]
    fn test_handle_key_event_ctrl_d_submits_when_buffer_not_empty() {
        let mut state = MultilineTextInputState::with_content("test");
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Submit));
    }

    #[test]
    fn test_handle_key_event_ctrl_d_ignored_when_buffer_empty_and_required() {
        let mut state = MultilineTextInputState::new();
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
    }

    #[test]
    fn test_handle_key_event_ctrl_d_allowed_when_not_required() {
        let mut state = MultilineTextInputState::new();
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Submit));
    }

    #[test]
    fn test_handle_key_event_shift_enter_adds_newline() {
        let mut state = MultilineTextInputState::with_content("line1");
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(state.buffer(), "line1\n");
        // Kursor na początku nowej linii (row=1, col=0), flat_pos=6
        assert_eq!(state.cursor(), (1, 0));
        assert_eq!(flat_pos(&state), 6);
    }

    #[test]
    fn test_handle_key_event_backspace_removes_last_char() {
        let mut state = MultilineTextInputState::with_content("hello");
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(state.buffer(), "hell");
        assert_eq!(state.cursor(), (0, 4));
    }

    #[test]
    fn test_handle_key_event_backspace_on_empty_buffer() {
        let mut state = MultilineTextInputState::new();
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer(), "");
        assert_eq!(state.cursor(), (0, 0));
    }

    #[test]
    fn test_handle_key_event_ctrl_c_returns_error() {
        let mut state = MultilineTextInputState::with_content("test");
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RalphError::Interrupted));
    }

    #[test]
    fn test_handle_key_event_char_appends_to_buffer() {
        let mut state = MultilineTextInputState::with_content("hel");
        let key = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer(), "hell");
        assert_eq!(state.cursor(), (0, 4));
    }

    #[test]
    fn test_handle_key_event_whitespace_chars() {
        let mut state = MultilineTextInputState::with_content("a");
        let space_key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        let tab_key = KeyEvent::new(KeyCode::Char('\t'), KeyModifiers::NONE);

        let _ = handle_key_event(space_key, &mut state, false);
        assert_eq!(state.buffer(), "a ");

        let _ = handle_key_event(tab_key, &mut state, false);
        assert_eq!(state.buffer(), "a \t");
    }

    #[test]
    fn test_handle_key_event_ctrl_d_rejects_whitespace_only_when_required() {
        let mut state = MultilineTextInputState::with_content("   \t  ");
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
    }

    #[test]
    fn test_multiline_input_multiple_newlines() {
        let mut state = MultilineTextInputState::with_content("line1");
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);

        let _ = handle_key_event(shift_enter, &mut state, false);
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

        assert_eq!(state.buffer(), "line1\nline2\nline3");
    }

    #[test]
    fn test_ctrl_c_preserves_buffer() {
        let original = "important data";
        let mut state = MultilineTextInputState::with_content(original);
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        let _ = handle_key_event(key, &mut state, true);
        assert_eq!(state.buffer(), original);
    }

    #[test]
    fn test_ctrl_c_during_multiline_input() {
        let mut state = MultilineTextInputState::with_content("line1\nline2\nline3");
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, false);
        assert!(matches!(result, Err(RalphError::Interrupted)));
        assert_eq!(state.buffer(), "line1\nline2\nline3");
    }

    // ── Testy alternatywnych skrótów newline ─────────────────────────────

    #[test]
    fn test_alt_enter_inserts_newline() {
        let mut state = MultilineTextInputState::with_content("line1");
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(state.buffer(), "line1\n");
        // flat_pos = 6: "line1" (5) + '\n' (1) + kursor na (1,0)
        assert_eq!(state.cursor(), (1, 0));
        assert_eq!(flat_pos(&state), 6);
    }

    #[test]
    fn test_ctrl_j_inserts_newline() {
        let mut state = MultilineTextInputState::with_content("line1");
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut state, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(state.buffer(), "line1\n");
        assert_eq!(state.cursor(), (1, 0));
        assert_eq!(flat_pos(&state), 6);
    }

    #[test]
    fn test_enter_with_shift_alt_combinations() {
        // Shift+Enter, Alt+Enter → newline (nie submit)
        let test_cases = [
            (KeyModifiers::SHIFT, "Shift+Enter"),
            (KeyModifiers::ALT, "Alt+Enter"),
        ];

        for (modifiers, desc) in &test_cases {
            let mut state = MultilineTextInputState::with_content("test");
            let key = KeyEvent::new(KeyCode::Enter, *modifiers);

            let result = handle_key_event(key, &mut state, false);
            assert!(result.is_ok(), "{} powinien być obsłużony", desc);
            assert!(
                matches!(result.unwrap(), KeyAction::Continue),
                "{} nie powinien submitować",
                desc
            );
            assert_eq!(
                state.buffer(),
                "test\n",
                "{} powinien wstawić newline",
                desc
            );
            // Kursor na (1, 0), flat_pos = 5
            assert_eq!(state.cursor(), (1, 0), "{}: kursor po newline", desc);
            assert_eq!(flat_pos(&state), 5, "{}: flat_pos po newline", desc);
        }
    }

    #[test]
    fn test_ctrl_j_in_middle_of_text() {
        let mut state = MultilineTextInputState::with_content("hello");
        state.set_cursor(0, 2); // między 'e' i 'l'

        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        let result = handle_key_event(key, &mut state, false);

        assert!(result.is_ok());
        assert_eq!(state.buffer(), "he\nllo");
        // Kursor na początku nowej linii (1, 0), flat_pos = 3
        assert_eq!(state.cursor(), (1, 0));
        assert_eq!(flat_pos(&state), 3);
    }

    #[test]
    fn test_alt_enter_with_empty_buffer() {
        let mut state = MultilineTextInputState::new();
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer(), "\n");
        // Kursor na (1, 0), flat_pos = 1
        assert_eq!(state.cursor(), (1, 0));
        assert_eq!(flat_pos(&state), 1);
    }

    #[test]
    fn test_multiple_newline_keys_sequence() {
        let mut state = MultilineTextInputState::with_content("a");

        // Shift+Enter
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
            &mut state,
            false,
        );
        assert_eq!(state.buffer(), "a\n");

        // Alt+Enter
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
            &mut state,
            false,
        );
        assert_eq!(state.buffer(), "a\n\n");

        // Ctrl+J
        let _ = handle_key_event(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
            &mut state,
            false,
        );
        assert_eq!(state.buffer(), "a\n\n\n");
        // Kursor na (3, 0), flat_pos = 4
        assert_eq!(state.cursor(), (3, 0));
        assert_eq!(flat_pos(&state), 4);
    }

    #[test]
    fn test_unhandled_ctrl_key_ignored() {
        let mut state = MultilineTextInputState::with_content("test");
        let ctrl_z = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL);

        let result = handle_key_event(ctrl_z, &mut state, false);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(state.buffer(), "test", "Ctrl+Z nie powinien wstawiać 'z'");
    }

    #[test]
    fn test_shift_enter_inserts_newline_at_cursor_position() {
        let mut state = MultilineTextInputState::with_content("hello");
        state.set_cursor(0, 2); // między 'e' i 'l'

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        let result = handle_key_event(key, &mut state, false);

        assert!(result.is_ok());
        assert_eq!(state.buffer(), "he\nllo");
        assert_eq!(state.cursor(), (1, 0));
        assert_eq!(flat_pos(&state), 3);
    }

    #[test]
    fn test_multiline_with_shift_enter() {
        let mut state = MultilineTextInputState::new();
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);

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

        assert_eq!(state.buffer(), "a\nb\nc");
    }

    // ── Testy Unicode ────────────────────────────────────────────────────

    #[test]
    fn test_handle_key_polish_char_appends() {
        let mut state = MultilineTextInputState::new();
        let key = KeyEvent::new(KeyCode::Char('ą'), KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer(), "ą");
        assert_eq!(state.buffer().len(), 2, "Znak 'ą' zajmuje 2 bajty UTF-8");
        assert_eq!(state.cursor(), (0, 1), "Kursor po 'ą' na char index 1");
    }

    #[test]
    fn test_handle_key_polish_chars_sequence() {
        let mut state = MultilineTextInputState::new();

        for c in ['ą', 'ę', 'ś', 'ć'] {
            let _ = handle_key_event(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &mut state,
                false,
            );
        }

        assert_eq!(state.buffer(), "ąęść");
        assert_eq!(
            state.buffer().chars().count(),
            4,
            "Powinno być 4 znaki Unicode"
        );
        assert_eq!(state.buffer().len(), 8, "Powinno być 8 bajtów UTF-8");
        assert_eq!(state.cursor(), (0, 4), "Kursor po 'ąęść' na char index 4");
    }

    #[test]
    fn test_backspace_after_polish_char() {
        let mut state = MultilineTextInputState::with_content("ą");
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer(), "", "Backspace powinien usunąć 'ą'");
        assert_eq!(state.buffer().len(), 0);
        assert_eq!(state.cursor(), (0, 0));
    }

    // ── Testy nawigacji Up/Down ──────────────────────────────────────────

    #[test]
    fn test_handle_key_up_moves_cursor_to_previous_line() {
        let mut state = MultilineTextInputState::with_content("line1\nline2\nline3");
        // Kursor na końcu "line3": (2, 5)
        assert_eq!(state.cursor(), (2, 5));

        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        // "line2" ma 5 znaków — ta sama kolumna
        assert_eq!(state.cursor(), (1, 5), "Kursor na końcu 'line2'");
    }

    #[test]
    fn test_handle_key_down_moves_cursor_to_next_line() {
        let mut state = MultilineTextInputState::with_content("line1\nline2\nline3");
        state.set_cursor(0, 5); // koniec "line1"

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.cursor(), (1, 5), "Kursor na końcu 'line2'");
    }

    #[test]
    fn test_handle_key_down_clamps_to_shorter_line() {
        let mut state = MultilineTextInputState::with_content("long_line\nhi");
        state.set_cursor(0, 9); // koniec "long_line"

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.cursor(), (1, 2), "Clamp do końca krótszej linii 'hi'");
    }

    #[test]
    fn test_up_on_first_line_does_nothing() {
        let mut state = MultilineTextInputState::with_content("test");
        state.set_cursor(0, 2);

        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.cursor(), (0, 2), "Up na pierwszej linii — brak zmian");
    }

    #[test]
    fn test_down_on_last_line_does_nothing() {
        let mut state = MultilineTextInputState::with_content("line1\nline2");
        // cursor na "line2" koniec = (1, 5)
        assert_eq!(state.cursor(), (1, 5));

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(
            state.cursor(),
            (1, 5),
            "Down na ostatniej linii — brak zmian"
        );
    }

    // ── Testy word operations ────────────────────────────────────────────

    #[test]
    fn test_ctrl_w_deletes_word_backward() {
        let mut state = MultilineTextInputState::with_content("hello world");
        // Kursor na końcu "hello world" (0, 11)
        let key = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer(), "hello ");
        assert_eq!(state.cursor(), (0, 6));
    }

    #[test]
    fn test_ctrl_u_deletes_to_line_start() {
        let mut state = MultilineTextInputState::with_content("hello world");
        state.set_cursor(0, 6); // po "hello "

        let key = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer(), "world");
        assert_eq!(state.cursor(), (0, 0));
    }

    #[test]
    fn test_ctrl_k_deletes_to_line_end() {
        let mut state = MultilineTextInputState::with_content("hello world");
        state.set_cursor(0, 5); // po "hello"

        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        let result = handle_key_event(key, &mut state, false);
        assert!(result.is_ok());
        assert_eq!(state.buffer(), "hello");
        assert_eq!(state.cursor(), (0, 5));
    }

    // ── Testy cursor_display_col ─────────────────────────────────────────

    #[test]
    fn test_cursor_display_col_first_row_adds_prefix() {
        // Kursor na pozycji 5 w linii "hello", pierwsza widoczna linia
        let col = cursor_display_col("hello", 0, 5, true);
        assert_eq!(col, 7, "2 (prefix) + 5 = 7");
    }

    #[test]
    fn test_cursor_display_col_continuation_with_indent() {
        // Kursor na pozycji 3 w linii "world", continuation linia (nie pierwsza)
        // Continuation lines mają "  " (2 spacje) jako indent — zawsze +2.
        let col = cursor_display_col("world", 0, 3, false);
        assert_eq!(col, 5, "Continuation indent '  ' (2 kol.) + 3 znaki = 5");
    }

    #[test]
    fn test_cursor_display_col_at_beginning_first_row() {
        let col = cursor_display_col("hello world", 0, 0, true);
        assert_eq!(col, 2, "Kursor na początku + prefix = 2");
    }

    #[test]
    fn test_cursor_display_col_with_polish_chars() {
        // Polskie znaki mają display width = 1, ale zajmują 2 bajty UTF-8
        let col = cursor_display_col("ąęśćź", 0, 3, true);
        assert_eq!(col, 5, "2 (prefix) + 3 polskie znaki (każdy 1 kolumna)");
    }

    #[test]
    fn test_cursor_display_col_with_char_offset() {
        // Fragment od char_offset = 5 (np. po wrap)
        // Kursor na col=7 w logicznej linii, fragment zaczyna się od 5
        // col_in_frag = 7-5 = 2, display: "wo" (2 kolumny) + 2 (indent "  ") = 4
        let col = cursor_display_col("hello world", 5, 7, false);
        assert_eq!(col, 4, "2 znaki 'wo' od offset=5 do cursor=7, +2 indent = 4");
    }
}
