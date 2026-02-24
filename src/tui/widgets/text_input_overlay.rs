//! Multi-line text input overlay widget dla wysyłania wiadomości do workerów.
//!
//! Wyświetla wyśrodkowany modal overlay z polem tekstowym do komponowania wiadomości.
//! Obsługuje:
//! - Wieloliniowy input via [`MultilineTextInputState`] (bez duplikacji logiki cursor/scroll)
//! - Enter = nowa linia (orchestrate-specific)
//! - Ctrl+Enter = wyślij wiadomość (orchestrate-specific)
//! - Esc = anuluj
//! - Cursor positioning, backspace, Home/End, Up/Down (delegowane do MultilineTextInputState)
//! - Pionowe scrollowanie gdy treść przekracza widoczny obszar (auto-follow)

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Wrap};

use crate::tui::Theme;
use crate::tui::formatting::unicode_column_to_char_index;
use crate::tui::widgets::multiline_text_input::MultilineTextInputState;

/// Akcja zwracana przez handle_key — sygnalizuje co powinno się stać dalej.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    /// Kontynuuj edycję (klawisz obsłużony, brak specjalnej akcji).
    Continue,
    /// Wyślij wiadomość (Ctrl+Enter wciśnięty).
    Send(String),
    /// Anuluj input (Esc wciśnięty).
    Cancel,
}

/// Konwertuje char index na byte offset w stringu.
///
/// Jeśli `char_idx` poza zakresem → zwraca `s.len()`.
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Multi-line text input overlay widget.
///
/// Wyświetla wyśrodkowany modal z tytułem "Message to Worker N",
/// polem tekstowym z scrollingiem i hintem o Ctrl+Enter/Esc.
///
/// Logika cursor i scroll jest delegowana do [`MultilineTextInputState`]
/// — unika duplikacji względem innych widgetów tekstowych.
pub struct TextInputOverlay {
    /// Współdzielony stan multiline input — obsługuje całą logikę edycji i kursora.
    input: MultilineTextInputState,
    /// Target worker ID dla wiadomości.
    target_worker_id: u32,
    /// Theme dla kolorów.
    theme: Theme,
}

impl TextInputOverlay {
    /// Tworzy nowy overlay dla wysyłania wiadomości do podanego workera.
    pub fn new(worker_id: u32) -> Self {
        Self::with_theme(worker_id, Theme::default())
    }

    /// Tworzy nowy overlay z niestandardowym theme.
    pub fn with_theme(worker_id: u32, theme: Theme) -> Self {
        Self {
            input: MultilineTextInputState::new(),
            target_worker_id: worker_id,
            theme,
        }
    }

    /// Obsługuje zdarzenie klawiszowe i zwraca akcję do wykonania.
    ///
    /// # Zachowanie orchestrate-specific (nadpisuje domyślne MultilineTextInputState):
    /// - **Ctrl+Enter**: wyślij wiadomość (zwraca [`InputAction::Send`])
    /// - **Esc**: anuluj (zwraca [`InputAction::Cancel`])
    /// - **Enter**: wstaw nową linię (nie wysyła!)
    ///
    /// # Zachowanie delegowane do MultilineTextInputState:
    /// - Backspace, Delete: usuwanie znaków
    /// - Left/Right/Up/Down: nawigacja kursora
    /// - Home/End, Ctrl+A/Ctrl+E: początek/koniec linii
    /// - Char input: wstawianie znaków
    pub fn handle_key(&mut self, key: KeyEvent) -> InputAction {
        // Ctrl+Enter: wyślij wiadomość (orchestrate-specific)
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
            // Nie wysyłaj pustych wiadomości
            if self.input.buffer().trim().is_empty() {
                return InputAction::Cancel;
            }
            return InputAction::Send(self.input.buffer().to_string());
        }

        match key.code {
            KeyCode::Esc => InputAction::Cancel,
            KeyCode::Enter => {
                // Enter wstawia nową linię (orchestrate-specific — nie submit!)
                self.input.insert_newline();
                InputAction::Continue
            }
            _ => {
                // Deleguj pozostałe klawisze do MultilineTextInputState.
                // handle_key_event obsługuje: Backspace, Delete, strzałki, Home, End,
                // Ctrl+A/E, Char(c). Shift+Enter też wstawia newline.
                // Nie obsługuje Enter (już obsłużony powyżej).
                self.input.handle_key_event(key);
                InputAction::Continue
            }
        }
    }

    /// Zwraca aktualną zawartość tekstową.
    #[allow(dead_code)] // Używane w testach; dostępne dla przyszłej integracji
    pub fn content(&self) -> &str {
        self.input.buffer()
    }

    /// Zwraca flat cursor position (char index) — kompatybilność z zewnętrznymi callerami.
    pub fn cursor_pos(&self) -> usize {
        let (row, col) = self.input.cursor();
        let buf = self.input.buffer();
        let mut offset = 0;
        for (i, line) in buf.split('\n').enumerate() {
            if i == row {
                return offset + col;
            }
            offset += line.chars().count() + 1; // +1 za '\n'
        }
        offset
    }

    /// Zwraca target worker ID tego overlaya.
    pub fn target_worker_id(&self) -> u32 {
        self.target_worker_id
    }

    /// Compute the Rect where this overlay will be rendered within the given terminal area.
    ///
    /// Uses the same centering and sizing logic as `render()`. Useful for hit-testing
    /// in mouse handlers — allows checking whether a click falls inside or outside the overlay.
    pub fn compute_rect(area: Rect) -> Rect {
        let overlay_width = (area.width * 6 / 10).clamp(40, 80);
        let overlay_height = (area.height / 2).clamp(10, 20);
        centered_rect(overlay_width, overlay_height, area)
    }

    /// Obsługuje zdarzenie myszy w obszarze overlaya.
    ///
    /// `area` — Rect overlaya (jak zwrócony przez `compute_rect()`, czyli po wycentrowaniu).
    ///
    /// Geometria (`Block::default().padding(Padding::uniform(1))`):
    /// - Tytuł: wiersz `area.y` (padding_top = 1)
    /// - Treść: wiersze `area.y+1 .. area.y+height-2` (viewport_height = height-2)
    /// - Hint: wiersz `area.y+height-1` (padding_bottom = 1)
    /// - Kolumna treści: `area.x+1` (padding_left = 1)
    ///
    /// Dla `MouseDown Left` w obszarze treści: ustawia kursor na pozycji
    /// odpowiadającej klikniętemu wierszowi i kolumnie (unicode-aware). Uwzględnia
    /// `scroll_offset`. Click poza treścią (tytuł, hint, poza zasięgiem) jest ignorowany.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }

        // Obszar treści: padding_top=1, dolny wiersz (hint) = y+height-1
        let content_y_start = area.y.saturating_add(1);
        let content_y_end_excl = area.y.saturating_add(area.height).saturating_sub(1);

        // Klik na tytule lub poniżej hinta → ignoruj
        if mouse.row < content_y_start || mouse.row >= content_y_end_excl {
            return;
        }

        let line_in_view = (mouse.row - content_y_start) as usize;
        let actual_line_idx = line_in_view + self.input.scroll_offset();

        let text_lines: Vec<&str> = self.input.buffer().split('\n').collect();

        // Click poza zawartością → kursor na koniec ostatniej linii
        if actual_line_idx >= text_lines.len() {
            let last_row = text_lines.len().saturating_sub(1);
            let last_col = text_lines.last().map(|l| l.chars().count()).unwrap_or(0);
            self.input.set_cursor(last_row, last_col);
            return;
        }

        let line = text_lines[actual_line_idx];
        // Kolumna w treści: odejmij padding_left=1
        let col_in_content = (mouse.column as usize).saturating_sub(area.x as usize + 1);
        let char_idx_in_line = unicode_column_to_char_index(line, col_in_content);

        self.input.set_cursor(actual_line_idx, char_idx_in_line);
    }

    /// Renderuje overlay widget na podanej ramce w określonym obszarze.
    ///
    /// Overlay jest wyśrodkowany w obszarze (60% szerokości, 50% wysokości,
    /// minimum 40x10, maksimum 80x20).
    ///
    /// Aktualizuje viewport_height w [`MultilineTextInputState`] dla poprawnego
    /// auto-follow scrollowania kursora.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Delegate to compute_rect to avoid duplicating the sizing/centering logic.
        let overlay_area = Self::compute_rect(area);
        let overlay_width = overlay_area.width;
        let overlay_height = overlay_area.height;

        // Aktualizuj viewport height dla auto-follow scroll
        // (wywoływane przed build_overlay_widget tak, by clamp_scroll użył aktualnych wymiarów)
        let inner_height = overlay_height.saturating_sub(2) as usize;
        self.input.set_viewport_height(inner_height);

        // Tło (semi-transparent efekt przez pusty blok)
        let backdrop = Block::default().style(Style::default().bg(self.theme.border_normal));
        frame.render_widget(backdrop, area);

        // Zbuduj i renderuj overlay widget
        let overlay_widget = self.build_overlay_widget(overlay_width, overlay_height);
        frame.render_widget(overlay_widget, overlay_area);
    }

    /// Buduje overlay widget (blok z tytułem, treścią i hintem).
    fn build_overlay_widget(&self, width: u16, height: u16) -> Paragraph<'_> {
        // Tytuł: "Message to Worker N"
        let title = format!(" Message to Worker {} ", self.target_worker_id);

        // Hint: "Ctrl+Enter to send, Esc to cancel"
        let hint = " Ctrl+Enter=send | Esc=cancel ";

        // Blok z tytułem — bez obramowania, z tłem
        let block = Block::default()
            .padding(Padding::uniform(1))
            .style(Style::default().bg(self.theme.panel_bg_focused))
            .title(Span::styled(title, self.theme.header_style()))
            .title_bottom(Span::styled(hint, self.theme.muted_style()));

        // Renderuj treść z kursorem
        let content_lines = self.render_content_with_cursor(width.saturating_sub(2));

        // Zastosuj scrolling używając scroll_offset z MultilineTextInputState
        let visible_lines: Vec<Line> = content_lines
            .into_iter()
            .skip(self.input.scroll_offset())
            .take((height.saturating_sub(2)) as usize) // Zostaw miejsce na obramowanie/padding
            .collect();

        Paragraph::new(visible_lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Left)
    }

    /// Renderuje treść z wskaźnikiem kursora.
    ///
    /// Zwraca listę `Line` reprezentujących treść z kursorem jako odwrócony blok
    /// na znaku pod kursorem, lub '|' gdy kursor jest na końcu treści.
    ///
    /// Używa `(cursor_row, cursor_col)` z [`MultilineTextInputState`] — brak duplikacji
    /// logiki śledzenia pozycji kursora w stosunku do implementacji widgetów tekstowych.
    fn render_content_with_cursor(&self, _max_width: u16) -> Vec<Line<'_>> {
        let (cursor_row, cursor_col) = self.input.cursor();
        let content = self.input.buffer();

        // Pusty bufor — pokaż placeholder z kursorem
        if content.is_empty() {
            return vec![Line::from(vec![Span::styled(
                "|",
                self.theme.primary_style().add_modifier(Modifier::REVERSED),
            )])];
        }

        // Podziel treść na linie logiczne i renderuj z kursorem.
        // cursor_row / cursor_col pochodzą bezpośrednio z MultilineTextInputState —
        // nie potrzeba obliczania running char_offset jak w starej implementacji.
        let text_lines: Vec<&str> = content.split('\n').collect();
        let mut lines = Vec::new();

        for (line_idx, line_text) in text_lines.iter().enumerate() {
            if line_idx == cursor_row {
                // Kursor na tej linii — renderuj z wskaźnikiem
                let cursor_byte = char_to_byte(line_text, cursor_col);
                let before = &line_text[..cursor_byte];
                let after = &line_text[cursor_byte..];

                let mut spans = Vec::new();
                if !before.is_empty() {
                    spans.push(Span::raw(before.to_string()));
                }

                // Wskaźnik kursora (odwrócony blok na znaku pod kursorem, lub '|' na końcu)
                if after.is_empty() {
                    spans.push(Span::styled(
                        "|",
                        self.theme.primary_style().add_modifier(Modifier::REVERSED),
                    ));
                } else {
                    // Weź dokładnie pierwszy znak (może być wielobajtowy UTF-8)
                    let first_char = after.chars().next().unwrap();
                    let first_char_len = first_char.len_utf8();
                    spans.push(Span::styled(
                        after[..first_char_len].to_string(),
                        self.theme.primary_style().add_modifier(Modifier::REVERSED),
                    ));

                    if after.len() > first_char_len {
                        spans.push(Span::raw(after[first_char_len..].to_string()));
                    }
                }

                lines.push(Line::from(spans));
            } else {
                // Renderuj linię bez kursora
                lines.push(Line::from(line_text.to_string()));
            }
        }

        // Zawijanie linii jest obsługiwane przez Paragraph::wrap(Wrap { trim: false })
        lines
    }
}

// ── Helper functions ────────────────────────────────────────────────────

/// Tworzy wyśrodkowany prostokąt o podanej szerokości i wysokości wewnątrz obszaru.
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

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper: flat cursor position ────────────────────────────────────
    //
    // Konwertuje (row, col) z MultilineTextInputState na flat char offset.
    // Używane do asercji kursora w testach (zamiast bezpośredniego dostępu do pola cursor_pos).

    fn cursor_pos(overlay: &TextInputOverlay) -> usize {
        let (row, col) = overlay.input.cursor();
        let buf = overlay.input.buffer();
        let mut offset = 0;
        for (i, line) in buf.split('\n').enumerate() {
            if i == row {
                return offset + col;
            }
            offset += line.chars().count() + 1; // +1 za '\n'
        }
        offset
    }

    // ── Basic tests ─────────────────────────────────────────────────────

    #[test]
    fn test_new_overlay_empty_content() {
        let overlay = TextInputOverlay::new(3);
        assert_eq!(overlay.content(), "");
        assert_eq!(overlay.input.cursor(), (0, 0));
        assert_eq!(overlay.target_worker_id, 3);
    }

    #[test]
    fn test_handle_char_input() {
        let mut overlay = TextInputOverlay::new(1);
        let action = overlay.handle_key(KeyEvent::from(KeyCode::Char('h')));
        assert_eq!(action, InputAction::Continue);
        assert_eq!(overlay.content(), "h");
        assert_eq!(cursor_pos(&overlay), 1);
    }

    #[test]
    fn test_handle_multiple_chars() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('h')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('i')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('!')));
        assert_eq!(overlay.content(), "hi!");
        assert_eq!(cursor_pos(&overlay), 3);
    }

    #[test]
    fn test_handle_enter_inserts_newline() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        let action = overlay.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(action, InputAction::Continue);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        assert_eq!(overlay.content(), "a\nb");
        assert_eq!(cursor_pos(&overlay), 3);
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
        assert_eq!(cursor_pos(&overlay), 2);
    }

    #[test]
    fn test_handle_backspace_at_start_is_noop() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "");
        assert_eq!(cursor_pos(&overlay), 0);
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
        assert_eq!(overlay.content(), "");

        let mut key = KeyEvent::from(KeyCode::Enter);
        key.modifiers = KeyModifiers::CONTROL;
        let action = overlay.handle_key(key);

        assert_eq!(action, InputAction::Cancel);
    }

    #[test]
    fn test_handle_ctrl_enter_whitespace_only_cancels() {
        let mut overlay = TextInputOverlay::new(1);
        // Ustaw treść ze spacjami — with_content() umieszcza kursor na końcu
        overlay.input = MultilineTextInputState::with_content("   \n\t  ");

        let mut key = KeyEvent::from(KeyCode::Enter);
        key.modifiers = KeyModifiers::CONTROL;
        let action = overlay.handle_key(key);

        // Powinna anulować białoznakową treść
        assert_eq!(action, InputAction::Cancel);
    }

    #[test]
    fn test_cursor_movement_left() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        assert_eq!(cursor_pos(&overlay), 2);

        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(cursor_pos(&overlay), 1);
    }

    #[test]
    fn test_cursor_movement_right() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(cursor_pos(&overlay), 1);

        overlay.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(cursor_pos(&overlay), 2);
    }

    #[test]
    fn test_cursor_movement_home() {
        let mut overlay = TextInputOverlay::new(1);
        // "hello\nworld", cursor_pos=8 → row=1, col=2 (na 'r' w "world")
        overlay.input = MultilineTextInputState::with_content("hello\nworld");
        overlay.input.set_cursor(1, 2); // middle of "world"

        overlay.handle_key(KeyEvent::from(KeyCode::Home));
        assert_eq!(overlay.input.cursor(), (1, 0)); // start of "world"
    }

    #[test]
    fn test_cursor_movement_end() {
        let mut overlay = TextInputOverlay::new(1);
        // "hello\nworld", cursor_pos=7 → row=1, col=1 (na 'o' w "world")
        overlay.input = MultilineTextInputState::with_content("hello\nworld");
        overlay.input.set_cursor(1, 1); // start of "world"

        overlay.handle_key(KeyEvent::from(KeyCode::End));
        assert_eq!(overlay.input.cursor(), (1, 5)); // end of "world"
    }

    #[test]
    fn test_cursor_up() {
        let mut overlay = TextInputOverlay::new(1);
        // Wpisz "a\nb" — kursor na row=1, col=1
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Enter));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        assert_eq!(overlay.input.cursor(), (1, 1));

        // Up przenosi kursor na poprzednią linię
        overlay.handle_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(overlay.input.cursor(), (0, 1));
    }

    #[test]
    fn test_cursor_down() {
        let mut overlay = TextInputOverlay::new(1);
        // Wpisz "a\nb" — kursor na row=1, col=1
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Enter));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        // Przesuń kursor na pierwszą linię
        overlay.handle_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(overlay.input.cursor(), (0, 1));

        // Down przenosi kursor na następną linię
        overlay.handle_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(overlay.input.cursor(), (1, 1));
    }

    #[test]
    fn test_cursor_up_at_top_is_noop() {
        let mut overlay = TextInputOverlay::new(1);
        // Kursor na (0,0) — Up powinno być no-op
        assert_eq!(overlay.input.cursor(), (0, 0));

        overlay.handle_key(KeyEvent::from(KeyCode::Up));
        overlay.handle_key(KeyEvent::from(KeyCode::Up));
        overlay.handle_key(KeyEvent::from(KeyCode::Up));

        // Nadal na górze — bez crash
        assert_eq!(overlay.input.cursor(), (0, 0));
    }

    // ── compute_rect tests ──────────────────────────────────────────

    #[test]
    fn test_compute_rect_standard_terminal() {
        // Standardowy terminal 80x24 — weryfikuje rozmiar i centrowanie overlaya
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let rect = TextInputOverlay::compute_rect(area);

        // overlay_width = (80 * 6 / 10).clamp(40, 80) = 48
        // overlay_height = (24 / 2).clamp(10, 20) = 12
        assert_eq!(rect.width, 48);
        assert_eq!(rect.height, 12);
        // Centered: x = (80 - 48) / 2 = 16, y = (24 - 12) / 2 = 6
        assert_eq!(rect.x, 16);
        assert_eq!(rect.y, 6);
    }

    #[test]
    fn test_compute_rect_click_outside_returns_false() {
        // Weryfikacja hit-testowania: click poza overlay_rect → contains() = false
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let rect = TextInputOverlay::compute_rect(area);

        // Lewy górny róg całego terminala jest poza overlay (który zaczyna się od x=16, y=6)
        assert!(!rect.contains(ratatui::layout::Position::new(0, 0)));
        assert!(!rect.contains(ratatui::layout::Position::new(15, 5)));
    }

    #[test]
    fn test_compute_rect_click_inside_returns_true() {
        // Weryfikacja hit-testowania: click wewnątrz overlay_rect → contains() = true
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let rect = TextInputOverlay::compute_rect(area);

        // Środek overlaya (x=16+24=40, y=6+6=12) powinien być wewnątrz
        let center_x = rect.x + rect.width / 2;
        let center_y = rect.y + rect.height / 2;
        assert!(rect.contains(ratatui::layout::Position::new(center_x, center_y)));

        // Lewy górny róg overlay_rect powinien być wewnątrz
        assert!(rect.contains(ratatui::layout::Position::new(rect.x, rect.y)));
    }

    #[test]
    fn test_compute_rect_matches_render_logic() {
        // compute_rect musi być spójne z render() — ta sama logika centrowania
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        };
        let computed = TextInputOverlay::compute_rect(area);

        // overlay_width = (120 * 6 / 10).clamp(40, 80) = 72.clamp(40, 80) = 72
        // overlay_height = (40 / 2).clamp(10, 20) = 20.clamp(10, 20) = 20
        assert_eq!(computed.width, 72);
        assert_eq!(computed.height, 20);
        // Centered: x = (120 - 72) / 2 = 24, y = (40 - 20) / 2 = 10
        assert_eq!(computed.x, 24);
        assert_eq!(computed.y, 10);
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
        // Wyśrodkowany: x = (100 - 40) / 2 = 30, y = (50 - 20) / 2 = 15
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

        // Powinno clampować do rozmiaru obszaru
        assert_eq!(rect.width, 30);
        assert_eq!(rect.height, 10);
    }

    #[test]
    fn test_multi_line_content() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("line1\nline2\nline3");
        overlay.input.set_cursor(0, 0);

        let lines = overlay.render_content_with_cursor(80);
        // Powinno mieć co najmniej 3 linie
        assert!(lines.len() >= 3);
    }

    #[test]
    fn test_cursor_at_end_of_content() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("test");
        // with_content ustawia kursor na końcu — (0, 4)

        let lines = overlay.render_content_with_cursor(80);
        assert_eq!(lines.len(), 1);
        // Kursor na końcu — powinien być renderowany (jako '|')
        let spans = &lines[0].spans;
        assert!(spans.len() > 1); // Tekst + kursor
    }

    #[test]
    fn test_empty_content_shows_cursor() {
        let overlay = TextInputOverlay::new(1);
        let lines = overlay.render_content_with_cursor(80);
        assert_eq!(lines.len(), 1);
        // Powinien mieć wskaźnik kursora
        assert!(!lines[0].spans.is_empty());
    }

    #[test]
    fn test_insert_at_middle() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("ac");
        overlay.input.set_cursor(0, 1); // między 'a' a 'c'

        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        assert_eq!(overlay.content(), "abc");
        assert_eq!(cursor_pos(&overlay), 2);
    }

    #[test]
    fn test_backspace_at_middle() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("abc");
        overlay.input.set_cursor(0, 2); // po 'b'

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "ac");
        assert_eq!(cursor_pos(&overlay), 1);
    }

    #[test]
    fn test_newline_at_middle() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("ac");
        overlay.input.set_cursor(0, 1); // między 'a' a 'c'

        overlay.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(overlay.content(), "a\nc");
        assert_eq!(cursor_pos(&overlay), 2); // po '\n'
    }

    #[test]
    fn test_render_overlay_widget_title() {
        let overlay = TextInputOverlay::new(5);
        let widget = overlay.build_overlay_widget(60, 20);

        // Widget powinien być tworzony bez paniki
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
        // 'ą' to 2 bajty w UTF-8
        let s = "ąę";
        assert_eq!(char_to_byte(s, 0), 0); // start of 'ą'
        assert_eq!(char_to_byte(s, 1), 2); // start of 'ę'
        assert_eq!(char_to_byte(s, 2), 4); // past end
    }

    #[test]
    fn test_unicode_char_input() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        assert_eq!(overlay.content(), "ą");
        assert_eq!(cursor_pos(&overlay), 1); // char index, nie byte offset
    }

    #[test]
    fn test_unicode_multiple_chars() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ś')));
        assert_eq!(overlay.content(), "ąęś");
        assert_eq!(cursor_pos(&overlay), 3);
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
        assert_eq!(cursor_pos(&overlay), 2);
    }

    #[test]
    fn test_unicode_left_right_navigation() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ć')));
        assert_eq!(cursor_pos(&overlay), 3);

        // Left dwukrotnie
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(cursor_pos(&overlay), 2);
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(cursor_pos(&overlay), 1);

        // Wstaw na pozycji 1 (między 'ą' a 'b')
        overlay.handle_key(KeyEvent::from(KeyCode::Char('x')));
        assert_eq!(overlay.content(), "ąxbć");
        assert_eq!(cursor_pos(&overlay), 2);

        // Right z powrotem na koniec
        overlay.handle_key(KeyEvent::from(KeyCode::Right));
        overlay.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(cursor_pos(&overlay), 4);
    }

    #[test]
    fn test_unicode_home_end() {
        let mut overlay = TextInputOverlay::new(1);
        // "ąę\nść": row=0 "ąę", row=1 "ść"
        overlay.input = MultilineTextInputState::with_content("ąę\nść");
        overlay.input.set_cursor(1, 1); // na 'ć' (środek drugiej linii)

        overlay.handle_key(KeyEvent::from(KeyCode::Home));
        assert_eq!(overlay.input.cursor(), (1, 0)); // start of "ść"

        overlay.handle_key(KeyEvent::from(KeyCode::End));
        assert_eq!(overlay.input.cursor(), (1, 2)); // end of "ść"
    }

    #[test]
    fn test_unicode_render_no_panic() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("ąęść");
        overlay.input.set_cursor(0, 2); // na 'ś'

        // Nie powinno panikować przy renderowaniu kursora na wielobajtowym znaku
        let lines = overlay.render_content_with_cursor(80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_unicode_mixed_ascii_and_polish() {
        let mut overlay = TextInputOverlay::new(1);
        // Wpisz: "aąb"
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        assert_eq!(overlay.content(), "aąb");
        assert_eq!(cursor_pos(&overlay), 3);

        // Backspace usuwa 'b'
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "aą");
        assert_eq!(cursor_pos(&overlay), 2);

        // Backspace usuwa 'ą'
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "a");
        assert_eq!(cursor_pos(&overlay), 1);
    }

    #[test]
    fn test_unicode_insert_in_middle() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("ąć");
        overlay.input.set_cursor(0, 1); // między 'ą' a 'ć'

        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        assert_eq!(overlay.content(), "ąęć");
        assert_eq!(cursor_pos(&overlay), 2);
    }

    #[test]
    fn test_unicode_backspace_in_middle() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("ąęć");
        overlay.input.set_cursor(0, 2); // po 'ę', przed 'ć'

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "ąć");
        assert_eq!(cursor_pos(&overlay), 1);
    }

    #[test]
    fn test_multiple_newlines() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Enter));
        overlay.handle_key(KeyEvent::from(KeyCode::Enter));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));

        assert_eq!(overlay.content(), "a\n\nb");
        assert_eq!(cursor_pos(&overlay), 4);
    }

    #[test]
    fn test_backspace_across_newline() {
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("a\nb");
        overlay.input.set_cursor(1, 0); // po '\n', przed 'b'

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));

        assert_eq!(overlay.content(), "ab");
        assert_eq!(cursor_pos(&overlay), 1);
    }

    #[test]
    fn test_render_with_many_lines() {
        let mut overlay = TextInputOverlay::new(1);
        // Utwórz treść z wieloma liniami
        for i in 0..20 {
            overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
            if i < 19 {
                overlay.handle_key(KeyEvent::from(KeyCode::Enter));
            }
        }

        let lines = overlay.render_content_with_cursor(80);

        // Powinno mieć co najmniej 20 linii (jedna na iterację)
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

        // Powinno być wyśrodkowane względem pozycji obszaru
        assert_eq!(rect.x, 10 + (100 - 40) / 2);
        assert_eq!(rect.y, 20 + (50 - 20) / 2);
    }

    // ── Snapshot testy dla TextInputOverlay modal ──

    use crate::test_helpers::{render_widget_to_buffer, snap};

    /// Wrapper Widget dla TextInputOverlay — renderuje build_overlay_widget w pełnym area.
    ///
    /// Testuje treść i layout widgetu (tytuł, border, tekst, hint) bez centrowania.
    /// Ustawia viewport_height dla poprawnego auto-follow scroll.
    struct TextInputOverlayWidget {
        overlay: TextInputOverlay,
    }

    impl ratatui::widgets::Widget for TextInputOverlayWidget {
        fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
            let mut overlay = self.overlay;
            // Ustaw viewport height tak, by clamp_scroll działał poprawnie w testach
            let viewport_h = area.height.saturating_sub(2) as usize;
            overlay.input.set_viewport_height(viewport_h);
            let overlay_widget = overlay.build_overlay_widget(area.width, area.height);
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
        Message to Worker 1

        |






        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_single_line_text() {
        // Overlay z jedną linią tekstu
        let mut overlay = TextInputOverlay::new(2);
        overlay.input = MultilineTextInputState::with_content("Hello Worker!");
        // with_content ustawia kursor na końcu (0, 13)

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 2

        Hello Worker!|






        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_multiline_text() {
        // Overlay z wieloma liniami tekstu
        let mut overlay = TextInputOverlay::new(3);
        overlay.input = MultilineTextInputState::with_content("Line one\nLine two\nLine three");
        // with_content ustawia kursor na końcu ostatniej linii

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 3

        Line one
        Line two
        Line three|




        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_cursor_at_start() {
        // Kursor na początku tekstu
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("test message");
        overlay.input.set_cursor(0, 0); // na początku

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 1

        test message






        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_cursor_in_middle() {
        // Kursor w środku tekstu
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("test message");
        overlay.input.set_cursor(0, 5); // między "test" a "message"

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 1

        test message






        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_cursor_multiline_positions() {
        // Kursor na różnych pozycjach w wieloliniowym tekście
        let mut overlay = TextInputOverlay::new(2);
        overlay.input = MultilineTextInputState::with_content("abc\ndef\nghi");
        // cursor_pos=4 (flat) → row=1, col=0 (początek "def")
        overlay.input.set_cursor(1, 0);

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 2

        abc
        def
        ghi




        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_with_scrolling() {
        // Overlay z długim tekstem i scrolling via cursor position
        let mut overlay = TextInputOverlay::new(5);
        // Tworzę 15 linii tekstu
        let lines: Vec<String> = (1..=15).map(|i| format!("Line {}", i)).collect();
        overlay.input = MultilineTextInputState::with_content(&lines.join("\n"));
        // Ustaw kursor na linii 12 (0-indexed), viewport_height=8 → scroll_offset=5
        // (clamp_scroll: 12 + 1 - 8 = 5)
        overlay.input.set_cursor(12, 0);

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        // Po scrolling offset=5 widzimy linie 6-11 (6 linii w viewport 10-4=6 po paddingu)
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 5

        Line 6
        Line 7
        Line 8
        Line 9
        Line 10
        Line 11

        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_narrow_terminal_40x10() {
        // Wąski area 40x10 — weryfikuje layout przy minimalnym rozmiarze
        let mut overlay = TextInputOverlay::new(7);
        overlay.input = MultilineTextInputState::with_content("Short text");
        // with_content ustawia kursor na końcu (0, 10)

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 40, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 7

        Short text|






        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_medium_terminal_80x15() {
        // Średni area 80x15 — weryfikuje layout przy standardowym rozmiarze
        let mut overlay = TextInputOverlay::new(10);
        overlay.input = MultilineTextInputState::with_content("Medium terminal test");
        // with_content ustawia kursor na końcu (0, 20)

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 80, 15);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 10

        Medium terminal test|











        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_wide_terminal_120x20() {
        // Szerokie area 120x20 — weryfikuje layout przy dużym rozmiarze
        let mut overlay = TextInputOverlay::new(15);
        overlay.input = MultilineTextInputState::with_content("Wide terminal test message");
        // with_content ustawia kursor na końcu (0, 26)

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 120, 20);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 15

        Wide terminal test message|
















        Ctrl+Enter=send | Esc=cancel
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

    // ── Snapshot testy dla Unicode input ──

    #[test]
    fn test_snapshot_unicode_input_aes() {
        // Test 1: wpisanie 'ąęś' — cursor_pos==3, content length==6 bytes
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("ąęś");
        // with_content ustawia kursor na końcu (0, 3)

        // Weryfikacja założeń: cursor to (row=0, col=3), content.len() to bajty
        assert_eq!(overlay.input.cursor(), (0, 3)); // 3 znaki
        assert_eq!(overlay.input.buffer().len(), 6); // 6 bajtów (ą=2B, ę=2B, ś=2B)
        assert_eq!(overlay.input.buffer().chars().count(), 3); // potwierdzenie char count

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 1

        ąęś|






        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_unicode_backspace_removal() {
        // Test 2: backspace po polskim znaku — symulacja pełnego flow
        let mut overlay = TextInputOverlay::new(2);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ś')));
        assert_eq!(cursor_pos(&overlay), 3);

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(cursor_pos(&overlay), 2);
        assert_eq!(overlay.input.buffer().len(), 4); // 4 bajty (ą=2B, ę=2B)
        assert_eq!(overlay.input.buffer().chars().count(), 2);

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 2

        ąę|






        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_unicode_cursor_left_middle() {
        // Test 3a: cursor left przez polskie znaki
        let mut overlay = TextInputOverlay::new(3);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ś')));
        assert_eq!(cursor_pos(&overlay), 3);

        // Left dwukrotnie: 3 → 2 → 1 (kursor na 'ę')
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(cursor_pos(&overlay), 1);

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 3

        ąęś






        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_unicode_cursor_right_middle() {
        // Test 3b: cursor right przez polskie znaki
        let mut overlay = TextInputOverlay::new(4);
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ś')));

        // Left do początku: 3 → 0
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(cursor_pos(&overlay), 0);

        // Right dwukrotnie: 0 → 1 → 2 (kursor na 'ś')
        overlay.handle_key(KeyEvent::from(KeyCode::Right));
        overlay.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(cursor_pos(&overlay), 2);

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 4

        ąęś






        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_unicode_home_key() {
        // Test 4a: Home z unicode content — wieloliniowy tekst
        let mut overlay = TextInputOverlay::new(5);
        overlay.input = MultilineTextInputState::with_content("ąę\nśćź");
        // cursor_pos=5 (flat) → row=1, col=2 (na 'ź')
        overlay.input.set_cursor(1, 2);

        overlay.handle_key(KeyEvent::from(KeyCode::Home));
        assert_eq!(overlay.input.cursor(), (1, 0)); // początek "śćź"

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 5

        ąę
        śćź





        Ctrl+Enter=send | Esc=cancel
        ");
    }

    #[test]
    fn test_snapshot_unicode_end_key() {
        // Test 4b: End z unicode content — wieloliniowy tekst
        let mut overlay = TextInputOverlay::new(6);
        overlay.input = MultilineTextInputState::with_content("ąę\nśćź");
        // cursor_pos=3 (flat) → row=1, col=0 (początek "śćź")
        overlay.input.set_cursor(1, 0);

        overlay.handle_key(KeyEvent::from(KeyCode::End));
        assert_eq!(overlay.input.cursor(), (1, 3)); // koniec "śćź"

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 6

        ąę
        śćź|





        Ctrl+Enter=send | Esc=cancel
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
        assert_eq!(cursor_pos(&overlay), 11); // na końcu

        // Left 4x: kursor na ' ' przed "xyz" (pos=7)
        for _ in 0..4 {
            overlay.handle_key(KeyEvent::from(KeyCode::Left));
        }
        assert_eq!(cursor_pos(&overlay), 7);

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 7

        abc ąęś xyz






        Ctrl+Enter=send | Esc=cancel
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
        assert_eq!(cursor_pos(&overlay), 0);

        let widget = TextInputOverlayWidget { overlay };
        let buffer = render_widget_to_buffer(widget, 60, 10);
        insta::assert_snapshot!(snap(&buffer), @"
        Message to Worker 8

        ąęś






        Ctrl+Enter=send | Esc=cancel
        ");
    }

    // ── Snapshot testy renderowania pełnego overlay modal z centrowaniem ──

    /// Pomocnicza funkcja do renderowania overlay z pełnym centrowaniem.
    fn render_overlay_full(
        overlay: TextInputOverlay,
        width: u16,
        height: u16,
    ) -> ratatui::buffer::Buffer {
        use ratatui::backend::TestBackend;
        use ratatui::prelude::Terminal;

        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("Failed to create terminal");
        let mut overlay = overlay;
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
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_full_render_single_line_80x24() {
        // Test 2: Overlay z jedną linią tekstu i kursorem — centrowanie 80x24
        let mut overlay = TextInputOverlay::new(2);
        overlay.input = MultilineTextInputState::with_content("Hello from worker!");
        // with_content ustawia kursor na końcu (0, 18)

        let buffer = render_overlay_full(overlay, 80, 24);
        insta::assert_snapshot!(snap(&buffer));
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
        overlay.input = MultilineTextInputState::with_content(&lines.join("\n"));
        // with_content ustawia kursor na końcu ostatniej linii

        let buffer = render_overlay_full(overlay, 80, 24);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_full_render_empty_40x15() {
        // Test 5: Pusty overlay modal — centrowanie na małym terminalu 40x15
        let overlay = TextInputOverlay::new(5);
        let buffer = render_overlay_full(overlay, 40, 15);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_full_render_text_40x15() {
        // Test 6: Overlay z tekstem — centrowanie na małym terminalu 40x15
        let mut overlay = TextInputOverlay::new(6);
        overlay.input = MultilineTextInputState::with_content("Short text\nAnother line");

        let buffer = render_overlay_full(overlay, 40, 15);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_full_render_hint_display() {
        // Test 7: Weryfikacja wyświetlania hint text w różnych konfiguracjach
        let mut overlay = TextInputOverlay::new(10);
        overlay.input = MultilineTextInputState::with_content("Testing hints");

        let buffer = render_overlay_full(overlay, 60, 12);

        // Weryfikujemy że hint jest na dole overlay
        let snapshot = snap(&buffer);
        assert!(snapshot.contains("Ctrl+Enter=send | Esc=cancel"));

        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_full_render_border_display() {
        // Test 8: Weryfikacja tytułu i struktury panelu
        let overlay = TextInputOverlay::new(42);
        let buffer = render_overlay_full(overlay, 70, 14);
        let snapshot = snap(&buffer);

        // Weryfikujemy że tytuł zawiera worker ID
        assert!(snapshot.contains("Message to Worker 42"));

        insta::assert_snapshot!(snapshot);
    }

    // ── Testy backspace na granicy unicode char ──

    #[test]
    fn test_backspace_after_multibyte_unicode_char() {
        // Test 1: wpisz 'aąb', cursor_pos=3, backspace → 'aą', cursor_pos=2
        let mut overlay = TextInputOverlay::new(1);

        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));

        assert_eq!(overlay.content(), "aąb");
        assert_eq!(cursor_pos(&overlay), 3); // 3 znaki
        assert_eq!(overlay.input.buffer().len(), 4); // 4 bajty: 'a'=1B, 'ą'=2B, 'b'=1B

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));

        assert_eq!(overlay.content(), "aą");
        assert_eq!(cursor_pos(&overlay), 2);
        assert_eq!(overlay.input.buffer().len(), 3); // 3 bajty: 'a'=1B, 'ą'=2B

        // Drugi backspace usuwa 'ą' (wielobajtowy znak)
        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));

        assert_eq!(overlay.content(), "a");
        assert_eq!(cursor_pos(&overlay), 1);
        assert_eq!(overlay.input.buffer().len(), 1);
    }

    #[test]
    fn test_backspace_after_multibyte_unicode_from_middle() {
        // Test 2: wpisz 'aąb', left, backspace → 'ab', cursor_pos=1
        let mut overlay = TextInputOverlay::new(1);

        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));

        assert_eq!(overlay.content(), "aąb");
        assert_eq!(cursor_pos(&overlay), 3);
        assert_eq!(overlay.input.buffer().len(), 4); // a=1B, ą=2B, b=1B

        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(cursor_pos(&overlay), 2);

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));

        assert_eq!(overlay.content(), "ab");
        assert_eq!(cursor_pos(&overlay), 1);
        assert_eq!(overlay.input.buffer().len(), 2);
    }

    #[test]
    fn test_backspace_emoji_multibyte() {
        // Test 3: wpisz emoji '🎉', backspace → '', cursor_pos=0
        let mut overlay = TextInputOverlay::new(1);

        overlay.handle_key(KeyEvent::from(KeyCode::Char('🎉')));

        assert_eq!(overlay.content(), "🎉");
        assert_eq!(cursor_pos(&overlay), 1); // 1 znak
        assert_eq!(overlay.input.buffer().len(), 4); // 4 bajty

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));

        assert_eq!(overlay.content(), "");
        assert_eq!(cursor_pos(&overlay), 0);
        assert_eq!(overlay.input.buffer().len(), 0);
    }

    #[test]
    fn test_insert_unicode_in_middle_of_ascii() {
        // Test 4: wpisz 'abc', left, wpisz 'ą', sprawdź content='abąc', cursor_pos=3
        let mut overlay = TextInputOverlay::new(1);

        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('c')));

        assert_eq!(overlay.content(), "abc");
        assert_eq!(cursor_pos(&overlay), 3);

        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(cursor_pos(&overlay), 2);

        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));

        assert_eq!(overlay.content(), "abąc");
        assert_eq!(cursor_pos(&overlay), 3);
        assert_eq!(overlay.input.buffer().len(), 5);
    }

    #[test]
    fn test_backspace_mixed_unicode_sequence() {
        // Test kompleksowy: mieszanka ASCII i unicode, backspace w różnych miejscach
        let mut overlay = TextInputOverlay::new(1);

        overlay.handle_key(KeyEvent::from(KeyCode::Char('a')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('ę')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('c')));

        assert_eq!(overlay.content(), "aąbęc");
        assert_eq!(cursor_pos(&overlay), 5);
        assert_eq!(overlay.input.buffer().len(), 7);

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "aąbę");
        assert_eq!(cursor_pos(&overlay), 4);

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "aąb");
        assert_eq!(cursor_pos(&overlay), 3);

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "aą");
        assert_eq!(cursor_pos(&overlay), 2);

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "a");
        assert_eq!(cursor_pos(&overlay), 1);

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "");
        assert_eq!(cursor_pos(&overlay), 0);
    }

    #[test]
    fn test_backspace_unicode_at_string_boundaries() {
        let mut overlay = TextInputOverlay::new(1);

        overlay.handle_key(KeyEvent::from(KeyCode::Char('ą')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('b')));

        assert_eq!(overlay.content(), "ąb");
        assert_eq!(cursor_pos(&overlay), 2);

        overlay.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(cursor_pos(&overlay), 1);

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "b");
        assert_eq!(cursor_pos(&overlay), 0);
        assert_eq!(overlay.input.buffer().len(), 1);
    }

    #[test]
    fn test_multiple_emoji_backspace() {
        let mut overlay = TextInputOverlay::new(1);

        overlay.handle_key(KeyEvent::from(KeyCode::Char('🎉')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('🚀')));
        overlay.handle_key(KeyEvent::from(KeyCode::Char('🌟')));

        assert_eq!(overlay.content(), "🎉🚀🌟");
        assert_eq!(cursor_pos(&overlay), 3);
        assert_eq!(overlay.input.buffer().len(), 12); // 3 × 4 bajty

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "🎉🚀");
        assert_eq!(cursor_pos(&overlay), 2);
        assert_eq!(overlay.input.buffer().len(), 8);

        overlay.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(overlay.content(), "🎉");
        assert_eq!(cursor_pos(&overlay), 1);
        assert_eq!(overlay.input.buffer().len(), 4);
    }

    // ── handle_mouse tests ────────────────────────────────────────────

    fn make_left_click_overlay(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    fn make_right_click_overlay(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: col,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    // Overlay area używana w testach: x=20, y=10, width=40, height=10
    fn test_overlay_area() -> Rect {
        Rect::new(20, 10, 40, 10)
    }

    #[test]
    fn test_handle_mouse_click_first_line_first_char() {
        // overlay: x=20, y=10, h=10 → content starts y=11, x=21
        let area = test_overlay_area();
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("Hello\nWorld");
        // with_content ustawia kursor na końcu — przesuwamy na koniec drugiej linii
        // cursor_pos() = 11 (flat)

        // Klik na (21, 11) → line 0, col 0 ("H")
        overlay.handle_mouse(make_left_click_overlay(21, 11), area);
        assert_eq!(overlay.input.cursor(), (0, 0));
        assert_eq!(overlay.cursor_pos(), 0);
    }

    #[test]
    fn test_handle_mouse_click_first_line_offset() {
        let area = test_overlay_area();
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("Hello\nWorld");
        overlay.input.set_cursor(0, 0);

        // Klik na (24, 11) → line 0, col = 24-21=3 ("l" at char 3)
        overlay.handle_mouse(make_left_click_overlay(24, 11), area);
        assert_eq!(overlay.input.cursor(), (0, 3));
        assert_eq!(overlay.cursor_pos(), 3);
    }

    #[test]
    fn test_handle_mouse_click_second_line() {
        let area = test_overlay_area();
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("Hello\nWorld");
        overlay.input.set_cursor(0, 0);

        // Klik na (22, 12) → line 1 ("World"), col = 22-21=1 → "o" (char 1)
        overlay.handle_mouse(make_left_click_overlay(22, 12), area);
        assert_eq!(overlay.input.cursor(), (1, 1));
        assert_eq!(overlay.cursor_pos(), 7); // 6 + 1
    }

    #[test]
    fn test_handle_mouse_right_click_ignored() {
        let area = test_overlay_area();
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("Hello");
        overlay.input.set_cursor(0, 3);

        overlay.handle_mouse(make_right_click_overlay(22, 11), area);
        assert_eq!(overlay.cursor_pos(), 3); // bez zmian
    }

    #[test]
    fn test_handle_mouse_click_on_title_ignored() {
        let area = test_overlay_area();
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("Hello");
        overlay.input.set_cursor(0, 3);

        // Tytuł jest w wierszu area.y=10 (poza content_y_start=11)
        overlay.handle_mouse(make_left_click_overlay(22, 10), area);
        assert_eq!(overlay.cursor_pos(), 3); // bez zmian
    }

    #[test]
    fn test_handle_mouse_click_on_hint_ignored() {
        let area = test_overlay_area(); // y=10, height=10 → hint row = y+height-1 = 19
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("Hello");
        overlay.input.set_cursor(0, 3);

        // Hint w wierszu y+height-1 = 19 (poza content_y_end_excl=19)
        overlay.handle_mouse(make_left_click_overlay(22, 19), area);
        assert_eq!(overlay.cursor_pos(), 3); // bez zmian
    }

    #[test]
    fn test_handle_mouse_click_beyond_content_sets_end() {
        let area = test_overlay_area();
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("Hi");
        overlay.input.set_cursor(0, 0);

        // Klik na wierszu 15 (poza "Hi" → tylko 1 linia)
        overlay.handle_mouse(make_left_click_overlay(22, 15), area);
        assert_eq!(overlay.cursor_pos(), 2); // koniec
    }

    #[test]
    fn test_handle_mouse_with_scroll_offset() {
        let area = test_overlay_area();
        let mut overlay = TextInputOverlay::new(1);
        // 5 linii, viewport_height=8 (area.height=10 - 2 padding)
        overlay.input = MultilineTextInputState::with_content("line0\nline1\nline2\nline3\nline4");
        overlay.input.set_viewport_height(8);
        // Ustaw kursor na linii 4 → scroll_offset skoczył do 0 (bo 5 linii < viewport 8)
        // Aby wymusić scroll_offset=2, potrzebujemy viewport_height mniejszy od ilości linii
        overlay.input.set_viewport_height(3); // viewport=3, 5 linii
        overlay.input.set_cursor(4, 0); // kursor na "line4" → clamp_scroll: 4+1-3=2
        assert_eq!(overlay.input.scroll_offset(), 2);

        // Klik na pierwszej widocznej linii (row=11=content_y_start)
        // → line_in_view=0, actual_line_idx=0+scroll_offset(2)=2 ("line2")
        overlay.handle_mouse(make_left_click_overlay(21, 11), area);
        assert_eq!(overlay.input.cursor(), (2, 0));
        assert_eq!(overlay.cursor_pos(), 12); // (5+1)*2 = 12
    }

    #[test]
    fn test_handle_mouse_cjk_aware() {
        let area = test_overlay_area();
        let mut overlay = TextInputOverlay::new(1);
        overlay.input = MultilineTextInputState::with_content("你好"); // 2 CJK = 4 kolumny
        overlay.input.set_cursor(0, 0);

        // col=21 → col_in_content = 0 → char 0 (你)
        overlay.handle_mouse(make_left_click_overlay(21, 11), area);
        assert_eq!(overlay.cursor_pos(), 0);

        // col=23 → col_in_content = 2 → char 1 (好) [CJK = 2 cols each]
        overlay.handle_mouse(make_left_click_overlay(23, 11), area);
        assert_eq!(overlay.cursor_pos(), 1);
    }

    #[test]
    fn test_handle_mouse_empty_content_sets_zero() {
        let area = test_overlay_area();
        let mut overlay = TextInputOverlay::new(1);
        // Pusty overlay — domyślny kursor (0, 0)

        overlay.handle_mouse(make_left_click_overlay(22, 11), area);
        assert_eq!(overlay.cursor_pos(), 0);
    }
}
