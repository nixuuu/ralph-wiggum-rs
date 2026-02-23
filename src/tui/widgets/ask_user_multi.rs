/// Widget dla inline multi-select w ask_user container.
///
/// Renderuje pytanie (markdown) + listę opcji z checkboxami [✓]/[ ].
/// Obsługuje nawigację ↑↓, toggle Space, submit Enter, oraz klik myszą.
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::shared::markdown::render_markdown;
use crate::tui::Theme;
use crate::tui::widgets::text_input_overlay::InputAction;

/// Jedna opcja w multi-select
#[derive(Debug, Clone)]
pub struct MultiSelectOption {
    /// Label wyświetlany obok checkboxa
    pub label: String,
    /// Opcjonalny opis (wyświetlany po label w szarym kolorze)
    pub description: Option<String>,
}

/// Stan multi-select widget
#[derive(Debug, Clone)]
pub struct MultiSelectState {
    /// Lista opcji
    pub options: Vec<MultiSelectOption>,
    /// Pozycja kursora (indeks wybranej opcji)
    pub cursor: usize,
    /// Flagi zaznaczenia dla każdej opcji (true = checked)
    pub checked: Vec<bool>,
    /// Indeks opcji pod kursorem myszy (hover, tylko wizualny feedback).
    /// None = brak hover. Aktualizowany przez kallerów na podstawie MouseMoved.
    pub hovered: Option<usize>,
}

impl MultiSelectState {
    /// Tworzy nowy stan z podaną listą opcji
    pub fn new(options: Vec<MultiSelectOption>) -> Self {
        let count = options.len();
        Self {
            options,
            cursor: 0,
            checked: vec![false; count],
            hovered: None,
        }
    }

    /// Ustaw hover na opcję o podanym indeksie (None = brak hover).
    /// Wywoływane przez kallerów na podstawie hit-testu pozycji myszy.
    pub fn set_hovered(&mut self, index: Option<usize>) {
        self.hovered = index;
    }

    /// Przesuwa kursor w górę (z zawijaniem)
    pub fn move_up(&mut self) {
        if self.options.is_empty() {
            return;
        }
        self.cursor = if self.cursor == 0 {
            self.options.len() - 1
        } else {
            self.cursor - 1
        };
    }

    /// Przesuwa kursor w dół (z zawijaniem)
    pub fn move_down(&mut self) {
        if self.options.is_empty() {
            return;
        }
        self.cursor = (self.cursor + 1) % self.options.len();
    }

    /// Toggleuje zaznaczenie opcji pod kursorem
    pub fn toggle_current(&mut self) {
        if self.cursor < self.checked.len() {
            self.checked[self.cursor] = !self.checked[self.cursor];
        }
    }

    /// Zwraca labele zaznaczonych opcji jako comma-separated string
    pub fn get_selected_labels(&self) -> String {
        self.options
            .iter()
            .enumerate()
            .filter(|(i, _)| *i < self.checked.len() && self.checked[*i])
            .map(|(_, opt)| opt.label.clone())
            .collect::<Vec<String>>()
            .join(", ")
    }

    /// Sprawdza czy jakakolwiek opcja jest zaznaczona
    pub fn has_selection(&self) -> bool {
        self.checked.iter().any(|&checked| checked)
    }

    /// Obsługuje kliknięcie myszą na opcję w liście.
    ///
    /// `area` — obszar renderowania opcji (content area bezpośrednio po pytaniu,
    /// taki sam jak przekazywany do `render_multi_active`). Każda opcja zajmuje
    /// jeden wiersz: opcja `i` jest na `area.y + i`.
    ///
    /// Zwraca:
    /// - `Some(InputAction::Continue)` — klik trafił w opcję; kursor przesunięty,
    ///   zaznaczenie toggled
    /// - `None` — klik poza opcjami (ignorowany)
    pub fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) -> Option<InputAction> {
        // Obsługujemy tylko lewy przycisk myszy (klik w dół)
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return None;
        }

        let pos = Position::new(mouse.column, mouse.row);

        // Klik musi być w poziomych granicach obszaru
        if pos.x < area.x || pos.x >= area.x.saturating_add(area.width) {
            return None;
        }

        // Klik musi być w pionowych granicach obszaru
        if pos.y < area.y || pos.y >= area.y.saturating_add(area.height) {
            return None;
        }

        // Indeks opcji = przesunięcie od początku obszaru
        let row_idx = (pos.y - area.y) as usize;

        // Ignoruj klik na hint line (poza zakresem opcji) lub poza listą
        if row_idx >= self.options.len() {
            return None;
        }

        // Ustaw kursor na klikniętą opcję i toggluj zaznaczenie
        self.cursor = row_idx;
        self.toggle_current();

        Some(InputAction::Continue)
    }
}

/// Widget multi-select — renderuje pytanie (markdown) + listę opcji z checkboxami
#[allow(dead_code)] // TUI component — will be used when full TUI is integrated
pub struct MultiSelectWidget<'a> {
    /// Tekst pytania (markdown)
    question: &'a str,
    /// Stan multi-select
    state: &'a MultiSelectState,
    /// Theme dla kolorów
    theme: &'a Theme,
    /// Czy widget jest aktywny (focused)
    focused: bool,
}

#[allow(dead_code)] // TUI component methods — will be used when widget is integrated
impl<'a> MultiSelectWidget<'a> {
    /// Tworzy nowy widget
    pub fn new(
        question: &'a str,
        state: &'a MultiSelectState,
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

    /// Renderuje pytanie — plain text dla prostych stringów, markdown dla sformatowanych
    fn render_question(&self, max_lines: usize) -> Vec<Line<'a>> {
        let has_markdown = self.question.contains('*')
            || self.question.contains('#')
            || self.question.contains('[')
            || self.question.contains('`');

        if !has_markdown {
            return self
                .question
                .lines()
                .take(max_lines)
                .map(|line| Line::from(line.to_string()))
                .collect();
        }

        let rendered = render_markdown(self.question);
        rendered
            .lines()
            .take(max_lines)
            .map(|line| Line::from(line.to_string()))
            .collect()
    }

    /// Renderuje pojedynczą opcję z checkboxem.
    ///
    /// `is_hovered` — czy opcja jest pod kursorem myszy (subtelne tło).
    fn render_option_line(
        &self,
        index: usize,
        option: &MultiSelectOption,
        is_hovered: bool,
    ) -> Line<'a> {
        let mut spans = Vec::new();

        // Prefix: kursor ("> " lub "  ")
        let cursor_prefix = if self.focused && self.state.cursor == index {
            "> "
        } else {
            "  "
        };

        let cursor_style = if self.focused && self.state.cursor == index {
            self.theme.header_style().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        spans.push(Span::styled(cursor_prefix, cursor_style));

        // Checkbox: [✓] lub [ ]
        let checkbox = if index < self.state.checked.len() && self.state.checked[index] {
            "[✓] "
        } else {
            "[ ] "
        };
        spans.push(Span::styled(checkbox, cursor_style));

        // Label
        spans.push(Span::styled(option.label.clone(), cursor_style));

        // Opcjonalny opis
        if let Some(ref desc) = option.description {
            spans.push(Span::styled(
                format!("  {}", desc),
                self.theme.muted_style(),
            ));
        }

        // Hover: subtelne tło — zachowuje fg (kursor i kolor zaznaczenia)
        if is_hovered {
            let hover_bg = self.theme.hover_row_bg;
            for span in &mut spans {
                span.style = span.style.bg(hover_bg);
            }
        }

        Line::from(spans)
    }
}

impl<'a> Widget for MultiSelectWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Oblicz wysokość pytania (max = wysokość - separator - min 1 opcja)
        let max_question_lines = area.height.saturating_sub(2) as usize;
        let question_lines = self.render_question(max_question_lines);
        let question_height = question_lines.len() as u16;

        // Renderuj pytanie
        for (i, line) in question_lines.iter().enumerate() {
            let y = area.y + i as u16;
            if y < area.y + area.height {
                buf.set_line(area.x, y, line, area.width);
            }
        }

        // Pusta linia separator
        let separator_y = area.y + question_height;
        if separator_y < area.y + area.height {
            buf.set_line(area.x, separator_y, &Line::from(""), area.width);
        }

        // Renderuj opcje (od separator_y + 1)
        let options_start_y = separator_y + 1;
        let available_height = area.height.saturating_sub(question_height + 1);

        for (i, option) in self.state.options.iter().enumerate() {
            let option_y = options_start_y + i as u16;
            if option_y >= area.y + area.height || i >= available_height as usize {
                break;
            }

            // Hover: wizualny feedback — pokazywany nawet gdy cursor == hover
            // (kursor ">" jest tekstem, hover to tło — nie kolidują)
            let is_hovered = self.state.hovered == Some(i);
            let option_line = self.render_option_line(i, option, is_hovered);
            buf.set_line(area.x, option_y, &option_line, area.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{render_widget_to_buffer, snap};
    use crate::tui::Theme;

    fn create_options(labels: &[&str]) -> Vec<MultiSelectOption> {
        labels
            .iter()
            .map(|&label| MultiSelectOption {
                label: label.to_string(),
                description: None,
            })
            .collect()
    }

    fn create_options_with_desc(items: &[(&str, Option<&str>)]) -> Vec<MultiSelectOption> {
        items
            .iter()
            .map(|(label, desc)| MultiSelectOption {
                label: label.to_string(),
                description: desc.map(|d| d.to_string()),
            })
            .collect()
    }

    #[test]
    fn test_multi_select_state_new() {
        let options = create_options(&["Option A", "Option B", "Option C"]);
        let state = MultiSelectState::new(options);

        assert_eq!(state.options.len(), 3);
        assert_eq!(state.cursor, 0);
        assert_eq!(state.checked, vec![false, false, false]);
    }

    #[test]
    fn test_move_up_down() {
        let options = create_options(&["A", "B", "C"]);
        let mut state = MultiSelectState::new(options);

        assert_eq!(state.cursor, 0);

        // Down: 0 -> 1
        state.move_down();
        assert_eq!(state.cursor, 1);

        // Down: 1 -> 2
        state.move_down();
        assert_eq!(state.cursor, 2);

        // Down: 2 -> 0 (wrap)
        state.move_down();
        assert_eq!(state.cursor, 0);

        // Up: 0 -> 2 (wrap)
        state.move_up();
        assert_eq!(state.cursor, 2);

        // Up: 2 -> 1
        state.move_up();
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn test_toggle_current() {
        let options = create_options(&["A", "B", "C"]);
        let mut state = MultiSelectState::new(options);

        // Toggle first (0)
        state.toggle_current();
        assert_eq!(state.checked, vec![true, false, false]);

        // Toggle again (uncheck)
        state.toggle_current();
        assert_eq!(state.checked, vec![false, false, false]);

        // Move to second and toggle
        state.move_down();
        state.toggle_current();
        assert_eq!(state.checked, vec![false, true, false]);
    }

    #[test]
    fn test_get_selected_labels() {
        let options = create_options(&["Auth", "API", "Logging"]);
        let mut state = MultiSelectState::new(options);

        // Nic nie zaznaczone
        assert_eq!(state.get_selected_labels(), "");

        // Zaznacz Auth
        state.toggle_current();
        assert_eq!(state.get_selected_labels(), "Auth");

        // Zaznacz Logging (index 2)
        state.cursor = 2;
        state.toggle_current();
        assert_eq!(state.get_selected_labels(), "Auth, Logging");

        // Zaznacz API (index 1)
        state.cursor = 1;
        state.toggle_current();
        assert_eq!(state.get_selected_labels(), "Auth, API, Logging");

        // Odznacz Auth
        state.cursor = 0;
        state.toggle_current();
        assert_eq!(state.get_selected_labels(), "API, Logging");
    }

    #[test]
    fn test_has_selection() {
        let options = create_options(&["A", "B"]);
        let mut state = MultiSelectState::new(options);

        assert!(!state.has_selection());

        state.toggle_current();
        assert!(state.has_selection());

        state.toggle_current();
        assert!(!state.has_selection());
    }

    #[test]
    fn test_move_on_empty_options() {
        let state = MultiSelectState::new(vec![]);
        let mut state = state;

        // Nie powinno panikować
        state.move_up();
        assert_eq!(state.cursor, 0);

        state.move_down();
        assert_eq!(state.cursor, 0);
    }

    // === Snapshot testy ===

    #[test]
    fn test_snapshot_no_selection() {
        let theme = Theme::default();
        let options = create_options(&["Auth", "API", "Logging"]);
        let state = MultiSelectState::new(options);
        let widget = MultiSelectWidget::new("Select modules:", &state, &theme, true);

        let buffer = render_widget_to_buffer(widget, 40, 6);
        insta::assert_snapshot!(snap(&buffer), @r"
        Select modules:

        > [ ] Auth
          [ ] API
          [ ] Logging
        ");
    }

    #[test]
    fn test_snapshot_first_and_third_checked() {
        let theme = Theme::default();
        let options = create_options(&["Auth", "API", "Logging"]);
        let mut state = MultiSelectState::new(options);

        // Zaznacz Auth (index 0)
        state.checked[0] = true;
        // Zaznacz Logging (index 2)
        state.checked[2] = true;

        let widget = MultiSelectWidget::new("Select modules:", &state, &theme, true);

        let buffer = render_widget_to_buffer(widget, 40, 6);
        insta::assert_snapshot!(snap(&buffer), @r"
        Select modules:

        > [✓] Auth
          [ ] API
          [✓] Logging
        ");
    }

    #[test]
    fn test_snapshot_cursor_on_second_item() {
        let theme = Theme::default();
        let options = create_options(&["Auth", "API", "Logging"]);
        let mut state = MultiSelectState::new(options);
        state.cursor = 1; // Kursor na "API"

        let widget = MultiSelectWidget::new("Select modules:", &state, &theme, true);

        let buffer = render_widget_to_buffer(widget, 40, 6);
        insta::assert_snapshot!(snap(&buffer), @r"
        Select modules:

          [ ] Auth
        > [ ] API
          [ ] Logging
        ");
    }

    #[test]
    fn test_snapshot_all_checked() {
        let theme = Theme::default();
        let options = create_options(&["Option A", "Option B", "Option C"]);
        let mut state = MultiSelectState::new(options);
        state.checked = vec![true, true, true];

        let widget = MultiSelectWidget::new("Select all:", &state, &theme, true);

        let buffer = render_widget_to_buffer(widget, 40, 6);
        insta::assert_snapshot!(snap(&buffer), @r"
        Select all:

        > [✓] Option A
          [✓] Option B
          [✓] Option C
        ");
    }

    #[test]
    fn test_snapshot_with_descriptions() {
        let theme = Theme::default();
        let options = create_options_with_desc(&[
            ("Auth", Some("Authentication module")),
            ("API", Some("RESTful API endpoints")),
            ("Logging", Some("Structured logging")),
        ]);
        let mut state = MultiSelectState::new(options);
        state.checked[1] = true; // API checked
        state.cursor = 1;

        let widget = MultiSelectWidget::new("Select modules:", &state, &theme, true);

        let buffer = render_widget_to_buffer(widget, 60, 6);
        insta::assert_snapshot!(snap(&buffer), @r"
        Select modules:

          [ ] Auth  Authentication module
        > [✓] API  RESTful API endpoints
          [ ] Logging  Structured logging
        ");
    }

    #[test]
    fn test_snapshot_unfocused() {
        let theme = Theme::default();
        let options = create_options(&["Auth", "API", "Logging"]);
        let mut state = MultiSelectState::new(options);
        state.checked[0] = true;

        let widget = MultiSelectWidget::new("Select modules:", &state, &theme, false);

        let buffer = render_widget_to_buffer(widget, 40, 6);
        insta::assert_snapshot!(snap(&buffer), @r"
        Select modules:

          [✓] Auth
          [ ] API
          [ ] Logging
        ");
    }

    #[test]
    fn test_snapshot_cursor_on_last_item() {
        let theme = Theme::default();
        let options = create_options(&["Auth", "API", "Logging"]);
        let mut state = MultiSelectState::new(options);
        state.cursor = 2;

        let widget = MultiSelectWidget::new("Select:", &state, &theme, true);

        let buffer = render_widget_to_buffer(widget, 40, 6);
        insta::assert_snapshot!(snap(&buffer), @r"
        Select:

          [ ] Auth
          [ ] API
        > [ ] Logging
        ");
    }

    #[test]
    fn test_snapshot_empty_options() {
        let theme = Theme::default();
        let state = MultiSelectState::new(vec![]);

        let widget = MultiSelectWidget::new("No options available:", &state, &theme, true);

        let buffer = render_widget_to_buffer(widget, 40, 4);
        insta::assert_snapshot!(snap(&buffer), @r"
        No options available:

        ");
    }

    #[test]
    fn test_snapshot_single_option() {
        let theme = Theme::default();
        let options = create_options(&["Only one"]);
        let mut state = MultiSelectState::new(options);
        state.checked[0] = true;

        let widget = MultiSelectWidget::new("Confirm?", &state, &theme, true);

        let buffer = render_widget_to_buffer(widget, 40, 4);
        insta::assert_snapshot!(snap(&buffer), @r"
        Confirm?

        > [✓] Only one
        ");
    }

    #[test]
    fn test_snapshot_long_list_truncated() {
        let theme = Theme::default();
        let options = create_options(&[
            "Item 1", "Item 2", "Item 3", "Item 4", "Item 5", "Item 6", "Item 7", "Item 8",
        ]);
        let mut state = MultiSelectState::new(options);
        state.cursor = 2;
        state.checked[2] = true;

        // Height = 8: 1 pytanie + 1 separator + 6 opcji (truncated)
        let widget = MultiSelectWidget::new("Q:", &state, &theme, true);

        let buffer = render_widget_to_buffer(widget, 30, 8);
        insta::assert_snapshot!(snap(&buffer), @r"
        Q:

          [ ] Item 1
          [ ] Item 2
        > [✓] Item 3
          [ ] Item 4
          [ ] Item 5
          [ ] Item 6
        ");
    }

    // === Testy handle_mouse ===

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

    fn make_options_area(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn test_handle_mouse_click_first_option_toggles() {
        let options = create_options(&["Auth", "API", "Logging"]);
        let mut state = MultiSelectState::new(options);
        let area = make_options_area(0, 5, 40, 5);

        // Klik na pierwszą opcję (row=5 = area.y + 0)
        let result = state.handle_mouse(make_left_click(5, 5), area);
        assert_eq!(result, Some(InputAction::Continue));
        assert_eq!(state.cursor, 0);
        assert!(state.checked[0], "Opcja 0 powinna być zaznaczona");
        assert!(!state.checked[1]);
        assert!(!state.checked[2]);
    }

    #[test]
    fn test_handle_mouse_click_second_option_toggles_and_moves_cursor() {
        let options = create_options(&["Auth", "API", "Logging"]);
        let mut state = MultiSelectState::new(options);
        let area = make_options_area(0, 5, 40, 5);

        // Klik na drugą opcję (row=6 = area.y + 1)
        let result = state.handle_mouse(make_left_click(0, 6), area);
        assert_eq!(result, Some(InputAction::Continue));
        assert_eq!(state.cursor, 1, "Kursor powinien przesunąć się na indeks 1");
        assert!(!state.checked[0]);
        assert!(state.checked[1], "Opcja 1 powinna być zaznaczona");
        assert!(!state.checked[2]);
    }

    #[test]
    fn test_handle_mouse_click_same_option_twice_toggles_off() {
        let options = create_options(&["Auth", "API"]);
        let mut state = MultiSelectState::new(options);
        let area = make_options_area(0, 0, 40, 4);

        // Zaznacz (row=0)
        state.handle_mouse(make_left_click(0, 0), area);
        assert!(state.checked[0]);

        // Odznacz (row=0 ponownie)
        let result = state.handle_mouse(make_left_click(0, 0), area);
        assert_eq!(result, Some(InputAction::Continue));
        assert!(
            !state.checked[0],
            "Opcja 0 powinna być odznaczona po drugim kliku"
        );
    }

    #[test]
    fn test_handle_mouse_click_outside_area_ignored() {
        let options = create_options(&["Auth", "API"]);
        let mut state = MultiSelectState::new(options);
        let area = make_options_area(0, 5, 40, 4);

        // Klik powyżej obszaru (row=4 < area.y=5)
        let result = state.handle_mouse(make_left_click(5, 4), area);
        assert_eq!(result, None, "Klik powyżej obszaru powinien być ignorowany");

        // Klik poniżej obszaru (row=9 >= area.y + area.height = 9)
        let result = state.handle_mouse(make_left_click(5, 9), area);
        assert_eq!(result, None, "Klik poniżej obszaru powinien być ignorowany");
    }

    #[test]
    fn test_handle_mouse_click_on_hint_line_ignored() {
        // Opcji jest 3, hint line jest na area.y + 3
        let options = create_options(&["Auth", "API", "Logging"]);
        let mut state = MultiSelectState::new(options);
        let area = make_options_area(0, 0, 40, 5);

        // Klik na hint line (row=3 = area.y + 3 = options.len())
        let result = state.handle_mouse(make_left_click(5, 3), area);
        assert_eq!(result, None, "Klik na hint line powinien być ignorowany");
        assert_eq!(
            state.checked,
            vec![false, false, false],
            "Żadna opcja nie powinna się zmienić"
        );
    }

    #[test]
    fn test_handle_mouse_right_click_ignored() {
        let options = create_options(&["Auth", "API"]);
        let mut state = MultiSelectState::new(options);
        let area = make_options_area(0, 0, 40, 4);

        let result = state.handle_mouse(make_right_click(5, 0), area);
        assert_eq!(result, None, "Prawy klik powinien być ignorowany");
        assert!(!state.checked[0], "Stan nie powinien się zmienić");
    }

    #[test]
    fn test_handle_mouse_click_outside_x_bounds_ignored() {
        let options = create_options(&["Auth"]);
        let mut state = MultiSelectState::new(options);
        // area.x=10, width=20 → x-range: [10, 30)
        let area = make_options_area(10, 0, 20, 3);

        // Klik po lewej stronie obszaru (col=9 < area.x=10)
        let result = state.handle_mouse(make_left_click(9, 0), area);
        assert_eq!(
            result, None,
            "Klik poza lewą krawędzią powinien być ignorowany"
        );

        // Klik po prawej stronie obszaru (col=30 >= area.x + area.width=30)
        let result = state.handle_mouse(make_left_click(30, 0), area);
        assert_eq!(
            result, None,
            "Klik poza prawą krawędzią powinien być ignorowany"
        );
    }

    #[test]
    fn test_handle_mouse_click_with_area_offset() {
        // Area z offsetem — jak w prawdziwym TUI wewnątrz widgetu
        let options = create_options(&["Auth", "API", "Logging"]);
        let mut state = MultiSelectState::new(options);
        let area = make_options_area(10, 5, 60, 5);

        // Klik na trzecią opcję (row=7 = area.y + 2, col=15 ∈ [10, 70))
        let result = state.handle_mouse(make_left_click(15, 7), area);
        assert_eq!(result, Some(InputAction::Continue));
        assert_eq!(state.cursor, 2);
        assert!(state.checked[2], "Opcja 2 powinna być zaznaczona");
    }

    #[test]
    fn test_handle_mouse_on_empty_options_ignored() {
        let mut state = MultiSelectState::new(vec![]);
        let area = make_options_area(0, 0, 40, 4);

        let result = state.handle_mouse(make_left_click(0, 0), area);
        assert_eq!(
            result, None,
            "Klik przy pustej liście powinien być ignorowany"
        );
    }

    // ── Hover Tests ─────────────────────────────────────────────────

    #[test]
    fn test_set_hovered_updates_field() {
        let options = create_options(&["A", "B"]);
        let mut state = MultiSelectState::new(options);
        assert_eq!(state.hovered, None);

        state.set_hovered(Some(1));
        assert_eq!(state.hovered, Some(1));

        state.set_hovered(None);
        assert_eq!(state.hovered, None);
    }

    #[test]
    fn test_hover_new_state_has_none() {
        let options = create_options(&["A", "B", "C"]);
        let state = MultiSelectState::new(options);
        assert_eq!(state.hovered, None, "Domyślny hover powinien być None");
    }

    #[test]
    fn test_hovered_option_has_hover_bg() {
        let theme = Theme::default();
        let options = create_options(&["Auth", "API", "Logging"]);
        let mut state = MultiSelectState::new(options);
        state.hovered = Some(1); // hover na opcję "API"

        let buffer = render_widget_to_buffer(
            MultiSelectWidget::new("Select:", &state, &theme, true),
            40,
            6,
        );

        // Opcje zaczynają się od y=2 (pytanie y=0, separator y=1)
        // Opcja "API" jest na y=3 (y=2 + offset 1)
        let hovered_cell = buffer.cell((0, 3)).expect("cell");
        assert_eq!(
            hovered_cell.bg, theme.hover_row_bg,
            "Hovered option powinna mieć hover_row_bg"
        );

        // Opcja "Auth" (y=2) nie powinna mieć hover_row_bg
        let normal_cell = buffer.cell((0, 2)).expect("cell");
        assert_ne!(
            normal_cell.bg, theme.hover_row_bg,
            "Normalny wiersz nie powinien mieć hover_row_bg"
        );
    }

    #[test]
    fn test_hover_does_not_change_cursor_text() {
        let theme = Theme::default();
        let options = create_options(&["Auth", "API"]);
        let mut state = MultiSelectState::new(options);
        state.cursor = 0; // kursor na "Auth"
        state.hovered = Some(0); // hover na "Auth" (kursor i hover na tej samej opcji)

        let buffer =
            render_widget_to_buffer(MultiSelectWidget::new("Q:", &state, &theme, true), 40, 4);

        // Opcja "Auth" powinna mieć hover bg (y=2)
        let cell = buffer.cell((0, 2)).expect("cell");
        assert_eq!(
            cell.bg, theme.hover_row_bg,
            "Kursor+hover: hover_row_bg stosowane nawet gdy cursor==hover"
        );
    }

    #[test]
    fn test_handle_mouse_multiple_clicks_sequence() {
        let options = create_options(&["Auth", "API", "Logging"]);
        let mut state = MultiSelectState::new(options);
        let area = make_options_area(0, 0, 40, 5);

        // Zaznacz Auth (row=0)
        state.handle_mouse(make_left_click(5, 0), area);
        // Zaznacz Logging (row=2)
        state.handle_mouse(make_left_click(5, 2), area);
        // Odznacz Auth (row=0)
        state.handle_mouse(make_left_click(5, 0), area);

        assert!(!state.checked[0], "Auth powinien być odznaczony");
        assert!(!state.checked[1], "API nie był klikany");
        assert!(state.checked[2], "Logging powinien być zaznaczony");
        assert_eq!(state.cursor, 0, "Kursor na ostatnio klikniętej pozycji");
    }

    #[test]
    fn test_checkbox_rendering_format() {
        let options = create_options(&["A", "B"]);
        let mut state = MultiSelectState::new(options);

        // Unchecked
        assert!(!state.checked[0]);
        let checkbox_unchecked = if state.checked[0] { "[✓] " } else { "[ ] " };
        assert_eq!(checkbox_unchecked, "[ ] ");

        // Checked
        state.toggle_current();
        assert!(state.checked[0]);
        let checkbox_checked = if state.checked[0] { "[✓] " } else { "[ ] " };
        assert_eq!(checkbox_checked, "[✓] ");
    }

    #[test]
    fn test_toggle_all_on_then_off() {
        let options = create_options(&["Auth", "API", "Logging", "Metrics", "Caching"]);
        let mut state = MultiSelectState::new(options);

        // Faza 1: zaznacz wszystkie (Space→Down 5x)
        for _ in 0..5 {
            state.toggle_current();
            state.move_down();
        }

        assert_eq!(state.checked, vec![true, true, true, true, true]);
        assert_eq!(
            state.get_selected_labels(),
            "Auth, API, Logging, Metrics, Caching"
        );

        // Faza 2: odznacz wszystkie
        state.cursor = 0;
        for _ in 0..5 {
            state.toggle_current();
            state.move_down();
        }

        assert_eq!(state.checked, vec![false, false, false, false, false]);
        assert_eq!(state.get_selected_labels(), "");
    }

    #[test]
    fn test_snapshot_toggle_all_on_off() {
        let theme = Theme::default();
        let options = create_options(&["Option A", "Option B", "Option C", "Option D"]);
        let mut state = MultiSelectState::new(options);

        // Faza 1: wszystkie zaznaczone
        state.checked = vec![true, true, true, true];
        state.cursor = 3;

        let widget = MultiSelectWidget::new("All on:", &state, &theme, true);
        let buffer = render_widget_to_buffer(widget, 30, 7);
        insta::assert_snapshot!(snap(&buffer), @r"
        All on:

          [✓] Option A
          [✓] Option B
          [✓] Option C
        > [✓] Option D
        ");

        // Faza 2: wszystkie odznaczone
        state.checked = vec![false, false, false, false];
        state.cursor = 0;

        let widget = MultiSelectWidget::new("All off:", &state, &theme, true);
        let buffer = render_widget_to_buffer(widget, 30, 7);
        insta::assert_snapshot!(snap(&buffer), @r"
        All off:

        > [ ] Option A
          [ ] Option B
          [ ] Option C
          [ ] Option D
        ");
    }
}
