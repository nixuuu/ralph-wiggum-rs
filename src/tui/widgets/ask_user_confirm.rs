/// Widget dla inline confirmation (Yes/No).
///
/// Renderuje pytanie (markdown) + przyciski [Yes] [No].
/// Obsługa klawiszy (y→Yes, n→No, ←→ Tab→toggle, Enter→submit)
/// jest w warstwie integracyjnej, nie w tym widgecie.
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::shared::markdown::render_markdown;
use crate::tui::theme::DEFAULT_THEME;
use crate::tui::widgets::text_input_overlay::InputAction;

/// Stan confirm widget
#[derive(Debug, Clone)]
pub struct ConfirmState {
    /// Domyślny wybór: true = Yes, false = No
    pub default: bool,
    /// Aktualnie wybrany: true = Yes, false = No
    pub selected: bool,
    /// Przycisk pod kursorem myszy (hover, tylko wizualny feedback).
    /// Some(true) = [Yes] hovered, Some(false) = [No] hovered, None = brak hover.
    pub hovered: Option<bool>,
}

impl ConfirmState {
    /// Tworzy nowy stan z domyślnym wyborem
    pub fn new(default: bool) -> Self {
        Self {
            default,
            selected: default,
            hovered: None,
        }
    }

    /// Ustaw hover na przycisk (true=[Yes], false=[No], None=brak hover).
    /// Wywoływane przez kallerów na podstawie pozycji kursora myszy.
    pub fn set_hovered(&mut self, button: Option<bool>) {
        self.hovered = button;
    }

    /// Aktualizuje hover na podstawie pozycji myszy względem obszaru przycisków.
    ///
    /// `buttons_area` — wiersz z przyciskami (ten sam co przekazywany do handle_mouse).
    pub fn update_hover(&mut self, mouse: MouseEvent, buttons_area: Rect) {
        let pos = Position::new(mouse.column, mouse.row);
        let (yes_rect, no_rect) = ConfirmWidget::button_rects(buttons_area);

        if yes_rect.contains(pos) {
            self.hovered = Some(true);
        } else if no_rect.contains(pos) {
            self.hovered = Some(false);
        } else {
            self.hovered = None;
        }
    }

    /// Przesunięcie na Yes
    pub fn select_yes(&mut self) {
        self.selected = true;
    }

    /// Przesunięcie na No
    pub fn select_no(&mut self) {
        self.selected = false;
    }

    /// Toggle między Yes/No
    pub fn toggle(&mut self) {
        self.selected = !self.selected;
    }

    /// Przywrócenie domyślnego wyboru
    pub fn reset(&mut self) {
        self.selected = self.default;
    }

    /// Zwraca "yes" lub "no"
    pub fn value(&self) -> &'static str {
        if self.selected { "yes" } else { "no" }
    }

    /// Obsługuje kliknięcie myszą na przyciski [Yes] / [No].
    ///
    /// `area` — wiersz z przyciskami (pierwszy wiersz content area, bez pytania).
    ///
    /// Aktualizuje stan `selected` i zwraca:
    /// - `Some(InputAction::Send("yes"))` — klik na [Yes]
    /// - `Some(InputAction::Send("no"))` — klik na [No]
    /// - `None` — klik poza przyciskami (ignorowany)
    pub fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) -> Option<InputAction> {
        // Obsługujemy tylko lewy przycisk myszy (klik w dół)
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return None;
        }

        let pos = Position::new(mouse.column, mouse.row);
        let (yes_rect, no_rect) = ConfirmWidget::button_rects(area);

        if yes_rect.contains(pos) {
            self.selected = true;
            Some(InputAction::Send("yes".to_string()))
        } else if no_rect.contains(pos) {
            self.selected = false;
            Some(InputAction::Send("no".to_string()))
        } else {
            None
        }
    }
}

/// Widget confirm — renderuje pytanie (markdown) + przyciski [Yes] [No].
/// Kolory przycisków: wybrany = Cyan bg + Black fg (bold), niewybrany = DarkGray fg.
/// Obsługa klawiszy (y/n, ←→, Tab, Enter) jest poza widgetem — w warstwie integracyjnej.
#[allow(dead_code)] // TUI component — will be used when full TUI is integrated
pub struct ConfirmWidget<'a> {
    /// Tekst pytania (markdown)
    question: &'a str,
    /// Stan confirma
    state: &'a ConfirmState,
}

#[allow(dead_code)] // TUI component methods — will be used when widget is integrated
impl<'a> ConfirmWidget<'a> {
    /// Tworzy nowy widget
    pub fn new(question: &'a str, state: &'a ConfirmState) -> Self {
        Self { question, state }
    }

    /// Oblicza recty przycisków [Yes] i [No] względem podanego obszaru.
    ///
    /// `area` — wiersz z przyciskami (pierwszy wiersz content area, po pytaniu).
    /// Layout: `[Yes]` (5 znaków) + `  ` (2 znaki) + `[No]` (4 znaki).
    ///
    /// Returns `(yes_rect, no_rect)`.
    pub fn button_rects(area: Rect) -> (Rect, Rect) {
        let yes_rect = Rect {
            x: area.x,
            y: area.y,
            width: 5, // "[Yes]"
            height: 1,
        };
        let no_rect = Rect {
            x: area.x.saturating_add(7), // "[Yes]" (5) + "  " (2)
            y: area.y,
            width: 4, // "[No]"
            height: 1,
        };
        (yes_rect, no_rect)
    }
}

impl<'a> Widget for ConfirmWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Renderuj pytanie jako markdown (String)
        let rendered_question = if !self.question.is_empty() {
            render_markdown(self.question)
        } else {
            String::new()
        };

        // Konwertuj na linie
        let question_lines: Vec<&str> = rendered_question.lines().collect();
        let header_height = std::cmp::min(question_lines.len() as u16, area.height);

        // Renderuj pytanie
        if header_height > 0 {
            for (i, line_text) in question_lines.iter().enumerate() {
                if i >= header_height as usize {
                    break;
                }
                let y = area.y + i as u16;
                let line = Line::from(line_text.to_string());
                line.render(
                    Rect {
                        x: area.x,
                        y,
                        width: area.width,
                        height: 1,
                    },
                    buf,
                );
            }
        }

        // Przyciski na linii po pytaniu
        let buttons_y = area.y + header_height;
        if buttons_y >= area.y + area.height {
            return; // Brak miejsca na przyciski
        }

        // Priorytet: selected (Cyan bg) > hovered (border_hover fg) > normalny (DarkGray)
        let yes_style = if self.state.selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if self.state.hovered == Some(true) {
            // Hover na [Yes] (nie wybrany): subtelne podświetlenie fg
            Style::default().fg(DEFAULT_THEME.border_hover)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let no_style = if !self.state.selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if self.state.hovered == Some(false) {
            // Hover na [No] (nie wybrany): subtelne podświetlenie fg
            Style::default().fg(DEFAULT_THEME.border_hover)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        // Renderuj [Yes]  [No]
        let line = Line::from(vec![
            Span::styled("[Yes]", yes_style),
            Span::raw("  "),
            Span::styled("[No]", no_style),
        ]);

        let buttons_area = Rect {
            x: area.x,
            y: buttons_y,
            width: area.width,
            height: 1,
        };

        line.render(buttons_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{render_widget_to_buffer, snap};

    #[test]
    fn test_confirm_state_new_default_yes() {
        let state = ConfirmState::new(true);
        assert!(state.selected);
        assert!(state.default);
        assert_eq!(state.value(), "yes");
    }

    #[test]
    fn test_confirm_state_new_default_no() {
        let state = ConfirmState::new(false);
        assert!(!state.selected);
        assert!(!state.default);
        assert_eq!(state.value(), "no");
    }

    #[test]
    fn test_confirm_state_select_yes() {
        let mut state = ConfirmState::new(false);
        state.select_yes();
        assert!(state.selected);
        assert_eq!(state.value(), "yes");
    }

    #[test]
    fn test_confirm_state_select_no() {
        let mut state = ConfirmState::new(true);
        state.select_no();
        assert!(!state.selected);
        assert_eq!(state.value(), "no");
    }

    #[test]
    fn test_confirm_state_toggle() {
        let mut state = ConfirmState::new(true);
        state.toggle();
        assert!(!state.selected);

        state.toggle();
        assert!(state.selected);
    }

    #[test]
    fn test_confirm_state_reset() {
        let mut state = ConfirmState::new(true);
        state.selected = false; // zmień
        state.reset();
        assert!(state.selected); // powróć do domyślnego
    }

    // ── Snapshot testy ──

    #[test]
    fn test_snapshot_yes_selected() {
        let state = ConfirmState::new(true);
        let widget = ConfirmWidget::new("", &state);

        let buffer = render_widget_to_buffer(widget, 80, 1);
        insta::assert_snapshot!(snap(&buffer), @"[Yes]  [No]");

        // Sprawdzenie stylów: [Yes] powinien być wyróżniony
        let yes_cell = buffer.cell((0, 0)).expect("Valid cell");
        assert_eq!(yes_cell.fg, Color::Black);
        assert_eq!(yes_cell.bg, Color::Cyan);

        // [No] powinien być wygaszony
        let no_cell = buffer.cell((7, 0)).expect("Valid cell");
        assert_eq!(no_cell.fg, Color::DarkGray);
    }

    #[test]
    fn test_snapshot_no_selected() {
        let state = ConfirmState::new(false);
        let widget = ConfirmWidget::new("", &state);

        let buffer = render_widget_to_buffer(widget, 80, 1);
        insta::assert_snapshot!(snap(&buffer), @"[Yes]  [No]");

        // [Yes] powinien być wygaszony
        let yes_cell = buffer.cell((0, 0)).expect("Valid cell");
        assert_eq!(yes_cell.fg, Color::DarkGray);

        // [No] powinien być wyróżniony
        let no_cell = buffer.cell((7, 0)).expect("Valid cell");
        assert_eq!(no_cell.fg, Color::Black);
        assert_eq!(no_cell.bg, Color::Cyan);
    }

    #[test]
    fn test_snapshot_with_short_question() {
        let mut state = ConfirmState::new(true);
        state.selected = false;
        let widget = ConfirmWidget::new("Are you sure?", &state);

        let buffer = render_widget_to_buffer(widget, 80, 2);
        let output = snap(&buffer);

        // Pytanie powinno być na pierwszej linii, przyciski na drugiej
        assert!(output.contains("Are you sure?"));
        assert!(output.contains("[Yes]"));
        assert!(output.contains("[No]"));
    }

    #[test]
    fn test_snapshot_with_long_question() {
        let state = ConfirmState::new(true);
        let long_question =
            "This is a very long confirmation message that might wrap on narrow terminals";
        let widget = ConfirmWidget::new(long_question, &state);

        let buffer = render_widget_to_buffer(widget, 40, 2);
        let output = snap(&buffer);

        // Przyciski powinny być widoczne
        assert!(output.contains("[Yes]"));
        assert!(output.contains("[No]"));
    }

    #[test]
    fn test_snapshot_multiline_question() {
        let state = ConfirmState::new(true);
        let widget = ConfirmWidget::new("Line one\nLine two\nLine three", &state);

        // 3 linie pytania + 1 linia przycisków = potrzeba 4 wierszy
        let buffer = render_widget_to_buffer(widget, 40, 5);
        let output = snap(&buffer);
        assert!(output.contains("[Yes]"));
        assert!(output.contains("[No]"));
    }

    #[test]
    fn test_snapshot_narrow_terminal() {
        let state = ConfirmState::new(true);
        let widget = ConfirmWidget::new("", &state);

        let buffer = render_widget_to_buffer(widget, 20, 1);
        insta::assert_snapshot!(snap(&buffer), @"[Yes]  [No]");
    }

    #[test]
    fn test_snapshot_wide_terminal() {
        let state = ConfirmState::new(false);
        let widget = ConfirmWidget::new("", &state);

        let buffer = render_widget_to_buffer(widget, 120, 1);
        insta::assert_snapshot!(snap(&buffer), @"[Yes]  [No]");
    }

    #[test]
    fn test_snapshot_zero_height() {
        let state = ConfirmState::new(true);
        let widget = ConfirmWidget::new("Test", &state);

        // zero height — widget powinien się nie renderować, buffer ma zero linii
        let buffer = render_widget_to_buffer(widget, 80, 0);
        let output = snap(&buffer);
        // Pusty buffer o wysokości 0 daje pusty string (nie ma nawet newline)
        assert_eq!(output, "");
    }

    #[test]
    fn test_snapshot_zero_width() {
        let state = ConfirmState::new(true);
        let widget = ConfirmWidget::new("Test", &state);

        // zero width — widget powinien się nie renderować
        let buffer = render_widget_to_buffer(widget, 0, 1);
        let output = snap(&buffer);
        assert_eq!(output, "\n"); // pusta linia
    }

    #[test]
    fn test_snapshot_not_enough_height_for_buttons() {
        let state = ConfirmState::new(true);
        // Pytanie z 2 liniami, ale area height = 2 → brak miejsca na przyciski
        let widget = ConfirmWidget::new("Line 1\nLine 2", &state);

        let buffer = render_widget_to_buffer(widget, 80, 2);
        let output = snap(&buffer);
        // Przyciski nie powinny się zmieścić
        assert!(!output.contains("[Yes]"));
    }

    // ── Testy button_rects ──

    #[test]
    fn test_button_rects_positions() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let (yes_rect, no_rect) = ConfirmWidget::button_rects(area);

        // Yes: x=0, width=5 ("[Yes]")
        assert_eq!(yes_rect.x, 0);
        assert_eq!(yes_rect.y, 0);
        assert_eq!(yes_rect.width, 5);
        assert_eq!(yes_rect.height, 1);

        // No: x=7 (5+"  "), width=4 ("[No]")
        assert_eq!(no_rect.x, 7);
        assert_eq!(no_rect.y, 0);
        assert_eq!(no_rect.width, 4);
        assert_eq!(no_rect.height, 1);
    }

    #[test]
    fn test_button_rects_with_offset() {
        // Area z offsetem (np. wewnątrz AskUserWidget)
        let area = Rect {
            x: 10,
            y: 5,
            width: 60,
            height: 1,
        };
        let (yes_rect, no_rect) = ConfirmWidget::button_rects(area);

        assert_eq!(yes_rect.x, 10);
        assert_eq!(yes_rect.y, 5);

        assert_eq!(no_rect.x, 17); // 10 + 7
        assert_eq!(no_rect.y, 5);
    }

    // ── Testy handle_mouse ──

    fn make_left_click(col: u16, row: u16) -> MouseEvent {
        use crossterm::event::KeyModifiers;
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn make_right_click(col: u16, row: u16) -> MouseEvent {
        use crossterm::event::KeyModifiers;
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn test_handle_mouse_click_yes() {
        let mut state = ConfirmState::new(false); // domyślnie No
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };

        // Klik na "[Yes]" (x=0..4)
        let result = state.handle_mouse(make_left_click(2, 0), area);
        assert_eq!(result, Some(InputAction::Send("yes".to_string())));
        assert!(
            state.selected,
            "state.selected powinno być true po kliknięciu Yes"
        );
    }

    #[test]
    fn test_handle_mouse_click_no() {
        let mut state = ConfirmState::new(true); // domyślnie Yes
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };

        // Klik na "[No]" (x=7..10)
        let result = state.handle_mouse(make_left_click(8, 0), area);
        assert_eq!(result, Some(InputAction::Send("no".to_string())));
        assert!(
            !state.selected,
            "state.selected powinno być false po kliknięciu No"
        );
    }

    #[test]
    fn test_handle_mouse_click_outside() {
        let mut state = ConfirmState::new(true);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };

        // Klik poza przyciskami (x=20)
        let result = state.handle_mouse(make_left_click(20, 0), area);
        assert_eq!(
            result, None,
            "Klik poza przyciskami powinien być ignorowany"
        );
    }

    #[test]
    fn test_handle_mouse_click_between_buttons() {
        let mut state = ConfirmState::new(true);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };

        // Klik w odstępie między przyciskami (x=5 lub x=6, "  ")
        let result5 = state.handle_mouse(make_left_click(5, 0), area);
        let result6 = state.handle_mouse(make_left_click(6, 0), area);
        assert_eq!(result5, None);
        assert_eq!(result6, None);
    }

    #[test]
    fn test_handle_mouse_wrong_row() {
        let mut state = ConfirmState::new(true);
        let area = Rect {
            x: 0,
            y: 5,
            width: 80,
            height: 1,
        };

        // Klik w złym wierszu (row=0, ale area.y=5)
        let result = state.handle_mouse(make_left_click(2, 0), area);
        assert_eq!(result, None, "Klik w złym wierszu powinien być ignorowany");
    }

    #[test]
    fn test_handle_mouse_right_click_ignored() {
        let mut state = ConfirmState::new(false);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };

        // Prawy klik nie powinien być obsługiwany
        let result = state.handle_mouse(make_right_click(2, 0), area);
        assert_eq!(result, None, "Prawy klik powinien być ignorowany");
        assert!(
            !state.selected,
            "Stan nie powinien się zmienić przy prawym kliku"
        );
    }

    #[test]
    fn test_handle_mouse_click_yes_boundary() {
        let mut state = ConfirmState::new(false);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };

        // Klik na ostatnim znaku "[Yes]" (x=4)
        let result = state.handle_mouse(make_left_click(4, 0), area);
        assert_eq!(result, Some(InputAction::Send("yes".to_string())));

        // Klik poza "[Yes]" (x=5, to już odstęp)
        let result_out = state.handle_mouse(make_left_click(5, 0), area);
        assert_eq!(result_out, None);
    }

    #[test]
    fn test_handle_mouse_click_no_boundary() {
        let mut state = ConfirmState::new(true);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };

        // Klik na pierwszym znaku "[No]" (x=7)
        let result_start = state.handle_mouse(make_left_click(7, 0), area);
        assert_eq!(result_start, Some(InputAction::Send("no".to_string())));

        // Reset
        state.selected = true;

        // Klik na ostatnim znaku "[No]" (x=10)
        let result_end = state.handle_mouse(make_left_click(10, 0), area);
        assert_eq!(result_end, Some(InputAction::Send("no".to_string())));
    }

    #[test]
    fn test_handle_mouse_with_area_offset() {
        let mut state = ConfirmState::new(false);
        // Area z offsetem — przyciski zaczynają się od x=10
        let area = Rect {
            x: 10,
            y: 3,
            width: 60,
            height: 1,
        };

        // Klik na Yes (x=10+2=12, row=3)
        let result = state.handle_mouse(make_left_click(12, 3), area);
        assert_eq!(result, Some(InputAction::Send("yes".to_string())));

        // Klik na No (x=10+7=17, row=3)
        let result_no = state.handle_mouse(make_left_click(17, 3), area);
        assert_eq!(result_no, Some(InputAction::Send("no".to_string())));

        // Klik przed area (x=5, row=3) → poza przyciskami
        let result_out = state.handle_mouse(make_left_click(5, 3), area);
        assert_eq!(result_out, None);
    }

    // ── Hover Tests ──────────────────────────────────────────────────

    fn make_moved(col: u16, row: u16) -> MouseEvent {
        use crossterm::event::KeyModifiers;
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn test_confirm_state_new_has_no_hover() {
        let state = ConfirmState::new(true);
        assert_eq!(state.hovered, None);
    }

    #[test]
    fn test_set_hovered_updates_field() {
        let mut state = ConfirmState::new(true);
        state.set_hovered(Some(true));
        assert_eq!(state.hovered, Some(true));

        state.set_hovered(Some(false));
        assert_eq!(state.hovered, Some(false));

        state.set_hovered(None);
        assert_eq!(state.hovered, None);
    }

    #[test]
    fn test_update_hover_yes_button() {
        let mut state = ConfirmState::new(false);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };

        // Ruch na [Yes] (x=2, w środku yes_rect x=0..4)
        state.update_hover(make_moved(2, 0), area);
        assert_eq!(state.hovered, Some(true));
    }

    #[test]
    fn test_update_hover_no_button() {
        let mut state = ConfirmState::new(true);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };

        // Ruch na [No] (x=8, w środku no_rect x=7..10)
        state.update_hover(make_moved(8, 0), area);
        assert_eq!(state.hovered, Some(false));
    }

    #[test]
    fn test_update_hover_outside_clears_hover() {
        let mut state = ConfirmState::new(true);
        state.hovered = Some(true);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };

        // Ruch poza przyciskami (x=20)
        state.update_hover(make_moved(20, 0), area);
        assert_eq!(state.hovered, None);
    }

    #[test]
    fn test_hovered_no_button_gets_hover_fg_when_not_selected() {
        // selected=true (Yes wybrany), hover=[No] → [No] powinien mieć border_hover fg
        let mut state = ConfirmState::new(true);
        state.hovered = Some(false); // hover na [No]

        let buffer = render_widget_to_buffer(ConfirmWidget::new("", &state), 80, 1);

        // [No] zaczyna się od x=7
        let no_cell = buffer.cell((7, 0)).expect("cell");
        assert_eq!(
            no_cell.fg, DEFAULT_THEME.border_hover,
            "[No] hovered (nie wybrany) → border_hover fg"
        );
    }

    #[test]
    fn test_hovered_yes_button_gets_hover_fg_when_not_selected() {
        // selected=false (No wybrany), hover=[Yes] → [Yes] powinien mieć border_hover fg
        let mut state = ConfirmState::new(false);
        state.hovered = Some(true); // hover na [Yes]

        let buffer = render_widget_to_buffer(ConfirmWidget::new("", &state), 80, 1);

        // [Yes] zaczyna się od x=0
        let yes_cell = buffer.cell((0, 0)).expect("cell");
        assert_eq!(
            yes_cell.fg, DEFAULT_THEME.border_hover,
            "[Yes] hovered (nie wybrany) → border_hover fg"
        );
    }

    #[test]
    fn test_selected_has_priority_over_hover_confirm() {
        // selected=true (Yes wybrany), hover=[Yes] → [Yes] ma Cyan bg (priorytet)
        let mut state = ConfirmState::new(true);
        state.hovered = Some(true); // hover na [Yes] — ten sam co selected

        let buffer = render_widget_to_buffer(ConfirmWidget::new("", &state), 80, 1);

        // [Yes] ma Cyan bg ponieważ jest wybrany
        let yes_cell = buffer.cell((0, 0)).expect("cell");
        assert_eq!(
            yes_cell.bg,
            Color::Cyan,
            "[Yes] wybrany → Cyan bg (priorytet nad hover)"
        );
    }
}
