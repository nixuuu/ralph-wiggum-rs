/// Widget dla inline confirmation (Yes/No).
///
/// Renderuje pytanie (markdown) + przyciski [Yes] [No].
/// Obsługa klawiszy (y→Yes, n→No, ←→ Tab→toggle, Enter→submit)
/// jest w warstwie integracyjnej, nie w tym widgecie.
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::shared::markdown::render_markdown;

/// Stan confirm widget
#[derive(Debug, Clone)]
pub struct ConfirmState {
    /// Domyślny wybór: true = Yes, false = No
    pub default: bool,
    /// Aktualnie wybrany: true = Yes, false = No
    pub selected: bool,
}

impl ConfirmState {
    /// Tworzy nowy stan z domyślnym wyborem
    pub fn new(default: bool) -> Self {
        Self {
            default,
            selected: default,
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

        let yes_style = if self.state.selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let no_style = if !self.state.selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
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
}
