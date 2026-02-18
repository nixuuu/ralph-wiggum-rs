//! Widget dla wyboru pojedynczej opcji (QuestionType::Choice)
//!
//! Renderuje pytanie + listę opcji z radio buttonami (●) selected / ( ) unselected.
//! Każda opcja może mieć opcjonalny description w DarkGray.
//! Kontrola: ↑↓ → move selection, Enter → Submit(label)
//!
//! Widget jest stateful — używa `ChoiceState` do śledzenia wybranej opcji.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::StatefulWidget,
};

use crate::tui::theme::DEFAULT_THEME;

// ── QuestionOption ──────────────────────────────────────────────────

/// Opcja wyboru w pytaniu — model danych warstwy TUI.
///
/// Celowo niezależny od MCP `ask_user::QuestionOption` (który ma serde atrybuty).
/// Warstwa UI nie zależy od warstwy protokołu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOption {
    /// Tekst wyświetlany jako opcja wyboru
    pub label: String,
    /// Opcjonalny opis opcji (np. trade-offy, konsekwencje)
    pub description: Option<String>,
}

// ── ChoiceState ─────────────────────────────────────────────────────

/// Stan widgetu wyboru — indeks wybranej opcji
#[derive(Debug, Clone, Default)]
pub struct ChoiceState {
    /// Indeks wybranej opcji w liście `options`
    pub selected: usize,
}

impl ChoiceState {
    /// Tworzy nowy stan z domyślnym wyborem (index 0)
    pub fn new() -> Self {
        Self::default()
    }

    /// Tworzy stan z wybraną opcją o podanym indeksie
    pub fn with_selected(index: usize) -> Self {
        Self { selected: index }
    }

    /// Przesuwa wybór w górę (z wrappingiem)
    pub fn move_up(&mut self, options_count: usize) {
        if options_count == 0 {
            return;
        }
        self.selected = if self.selected == 0 {
            options_count - 1
        } else {
            self.selected - 1
        };
    }

    /// Przesuwa wybór w dół (z wrappingiem)
    pub fn move_down(&mut self, options_count: usize) {
        if options_count == 0 {
            return;
        }
        self.selected = (self.selected + 1) % options_count;
    }

    /// Zwraca label wybranej opcji (jeśli istnieje)
    pub fn get_selected_label<'a>(&self, options: &'a [QuestionOption]) -> Option<&'a str> {
        options.get(self.selected).map(|opt| opt.label.as_str())
    }

    /// Waliduje i naprawia selected jeśli jest poza zakresem
    pub fn clamp(&mut self, options_count: usize) {
        if options_count == 0 {
            self.selected = 0;
        } else if self.selected >= options_count {
            self.selected = options_count - 1;
        }
    }
}

// ── ChoiceWidget ────────────────────────────────────────────────────

/// Widget do wyboru pojedynczej opcji z listy
///
/// Wyświetla:
/// - Pytanie (question text) — opcjonalnie w headerze
/// - Lista opcji z radio buttonami:
///   - (●) selected option — Cyan + Bold
///   - ( ) unselected option — DarkGray
/// - Description pod każdą opcją (jeśli istnieje) — DarkGray
///
/// # Example
///
/// ```ignore
/// use ratatui::widgets::StatefulWidget;
/// use ralph_wiggum::tui::widgets::ask_user_choice::{ChoiceWidget, ChoiceState};
/// use ralph_wiggum::commands::mcp::ask_user::QuestionOption;
///
/// let options = vec![
///     QuestionOption {
///         label: "JWT".into(),
///         description: Some("Token-based authentication".into()),
///     },
///     QuestionOption {
///         label: "Session".into(),
///         description: Some("Cookie-based authentication".into()),
///     },
/// ];
///
/// let widget = ChoiceWidget::new(&options, Some("Choose authentication method"));
/// let mut state = ChoiceState::new();
///
/// // Renderuj w ratatui frame:
/// frame.render_stateful_widget(widget, area, &mut state);
/// ```
#[derive(Debug)]
pub struct ChoiceWidget<'a> {
    /// Lista opcji do wyboru
    options: &'a [QuestionOption],
    /// Opcjonalny tekst pytania (wyświetlany nad listą)
    question: Option<&'a str>,
}

impl<'a> ChoiceWidget<'a> {
    /// Tworzy nowy widget z opcjami i opcjonalnym pytaniem
    pub fn new(options: &'a [QuestionOption], question: Option<&'a str>) -> Self {
        Self { options, question }
    }

    /// Renderuje listę opcji z radio buttonami
    fn render_options(&self, area: Rect, buf: &mut Buffer, state: &ChoiceState) {
        let mut y = area.y;

        // Renderuj opcjonalne pytanie
        if let Some(question_text) = self.question {
            if y >= area.y + area.height {
                return; // Brak miejsca
            }
            let question_line = Line::from(vec![Span::styled(
                question_text,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )]);
            buf.set_line(area.x, y, &question_line, area.width);
            y += 1;

            // Pusta linia między pytaniem a opcjami
            if y >= area.y + area.height {
                return;
            }
            y += 1;
        }

        // Renderuj każdą opcję
        for (idx, option) in self.options.iter().enumerate() {
            if y >= area.y + area.height {
                break; // Brak miejsca na kolejne opcje
            }

            let is_selected = idx == state.selected;

            // Radio button + label
            let radio = if is_selected { "●" } else { "○" };
            let style = if is_selected {
                Style::default()
                    .fg(DEFAULT_THEME.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                DEFAULT_THEME.muted_style()
            };

            let line = Line::from(vec![
                Span::styled(radio, style),
                Span::raw(" "),
                Span::styled(&option.label, style),
            ]);

            buf.set_line(area.x, y, &line, area.width);
            y += 1;

            // Description (jeśli istnieje)
            if let Some(ref desc) = option.description {
                if y >= area.y + area.height {
                    break;
                }
                let desc_line = Line::from(vec![
                    Span::raw("  "), // Indent
                    Span::styled(desc, DEFAULT_THEME.muted_style()),
                ]);
                buf.set_line(area.x, y, &desc_line, area.width);
                y += 1;
            }
        }
    }
}

impl<'a> StatefulWidget for ChoiceWidget<'a> {
    type State = ChoiceState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Walidacja stanu przed renderowaniem
        state.clamp(self.options.len());

        // Clear area (opcjonalne — ratatui zazwyczaj czyści)
        // buf.set_style(area, DEFAULT_THEME.text);

        // Renderuj opcje
        self.render_options(area, buf, state);
    }
}

// ── Event Enum ──────────────────────────────────────────────────────

/// Wynik obsługi zdarzenia w widget'cie
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceEvent {
    /// Użytkownik wybrał opcję (zwraca label)
    Submit(String),
    /// Zmiana wyboru (brak akcji)
    None,
}

impl ChoiceState {
    /// Obsługuje klawisze ↑↓ Enter i zwraca event
    ///
    /// # Arguments
    ///
    /// * `key` - Kod klawisza (z crossterm::event::KeyCode)
    /// * `options` - Lista opcji (do walidacji i pobrania labela)
    ///
    /// # Returns
    ///
    /// * `ChoiceEvent::Submit(label)` - gdy Enter, zwraca label wybranej opcji
    /// * `ChoiceEvent::None` - gdy ↑↓ (zmiana wyboru)
    pub fn handle_key(
        &mut self,
        key: crossterm::event::KeyCode,
        options: &[QuestionOption],
    ) -> ChoiceEvent {
        use crossterm::event::KeyCode;

        match key {
            KeyCode::Up => {
                self.move_up(options.len());
                ChoiceEvent::None
            }
            KeyCode::Down => {
                self.move_down(options.len());
                ChoiceEvent::None
            }
            KeyCode::Enter => {
                if let Some(label) = self.get_selected_label(options) {
                    ChoiceEvent::Submit(label.to_string())
                } else {
                    ChoiceEvent::None
                }
            }
            _ => ChoiceEvent::None,
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::snap;
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    // ── ChoiceState Tests ───────────────────────────────────────────

    #[test]
    fn test_choice_state_new() {
        let state = ChoiceState::new();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_choice_state_with_selected() {
        let state = ChoiceState::with_selected(2);
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn test_move_up_wrapping() {
        let mut state = ChoiceState::new();
        state.move_up(3);
        assert_eq!(state.selected, 2); // Wrap do ostatniego
    }

    #[test]
    fn test_move_up_normal() {
        let mut state = ChoiceState::with_selected(2);
        state.move_up(3);
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn test_move_down_wrapping() {
        let mut state = ChoiceState::with_selected(2);
        state.move_down(3);
        assert_eq!(state.selected, 0); // Wrap do pierwszego
    }

    #[test]
    fn test_move_down_normal() {
        let mut state = ChoiceState::new();
        state.move_down(3);
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn test_move_up_empty_options() {
        let mut state = ChoiceState::new();
        state.move_up(0);
        assert_eq!(state.selected, 0); // Nie zmienia się
    }

    #[test]
    fn test_move_down_empty_options() {
        let mut state = ChoiceState::new();
        state.move_down(0);
        assert_eq!(state.selected, 0); // Nie zmienia się
    }

    #[test]
    fn test_get_selected_label() {
        let options = vec![
            QuestionOption {
                label: "Option A".into(),
                description: None,
            },
            QuestionOption {
                label: "Option B".into(),
                description: None,
            },
        ];

        let state = ChoiceState::with_selected(1);
        assert_eq!(state.get_selected_label(&options), Some("Option B"));
    }

    #[test]
    fn test_get_selected_label_out_of_bounds() {
        let options = vec![QuestionOption {
            label: "Only option".into(),
            description: None,
        }];

        let state = ChoiceState::with_selected(5);
        assert_eq!(state.get_selected_label(&options), None);
    }

    #[test]
    fn test_clamp_out_of_bounds() {
        let mut state = ChoiceState::with_selected(10);
        state.clamp(3);
        assert_eq!(state.selected, 2); // Max index dla 3 opcji
    }

    #[test]
    fn test_clamp_empty_options() {
        let mut state = ChoiceState::with_selected(5);
        state.clamp(0);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_clamp_valid_index() {
        let mut state = ChoiceState::with_selected(1);
        state.clamp(3);
        assert_eq!(state.selected, 1); // Nie zmienia się
    }

    // ── Event Handling Tests ────────────────────────────────────────

    #[test]
    fn test_handle_key_up() {
        use crossterm::event::KeyCode;

        let options = vec![
            QuestionOption {
                label: "A".into(),
                description: None,
            },
            QuestionOption {
                label: "B".into(),
                description: None,
            },
            QuestionOption {
                label: "C".into(),
                description: None,
            },
        ];

        let mut state = ChoiceState::with_selected(1);
        let event = state.handle_key(KeyCode::Up, &options);

        assert_eq!(event, ChoiceEvent::None);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_handle_key_down() {
        use crossterm::event::KeyCode;

        let options = vec![
            QuestionOption {
                label: "A".into(),
                description: None,
            },
            QuestionOption {
                label: "B".into(),
                description: None,
            },
        ];

        let mut state = ChoiceState::new();
        let event = state.handle_key(KeyCode::Down, &options);

        assert_eq!(event, ChoiceEvent::None);
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn test_handle_key_enter() {
        use crossterm::event::KeyCode;

        let options = vec![QuestionOption {
            label: "Selected".into(),
            description: None,
        }];

        let mut state = ChoiceState::new();
        let event = state.handle_key(KeyCode::Enter, &options);

        assert_eq!(event, ChoiceEvent::Submit("Selected".to_string()));
    }

    #[test]
    fn test_handle_key_enter_empty_options() {
        use crossterm::event::KeyCode;

        let options: Vec<QuestionOption> = vec![];
        let mut state = ChoiceState::new();
        let event = state.handle_key(KeyCode::Enter, &options);

        assert_eq!(event, ChoiceEvent::None);
    }

    #[test]
    fn test_handle_key_other() {
        use crossterm::event::KeyCode;

        let options = vec![QuestionOption {
            label: "A".into(),
            description: None,
        }];

        let mut state = ChoiceState::new();
        let event = state.handle_key(KeyCode::Char('x'), &options);

        assert_eq!(event, ChoiceEvent::None);
        assert_eq!(state.selected, 0); // Nie zmienia się
    }

    // ── Snapshot Tests ──────────────────────────────────────────────

    /// Helper: renderuje widget do buffera
    fn render_widget(
        options: &[QuestionOption],
        question: Option<&str>,
        selected_index: usize,
        width: u16,
        height: u16,
    ) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("Failed to create test terminal");

        let mut state = ChoiceState::with_selected(selected_index);

        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, height);
                let widget = ChoiceWidget::new(options, question);
                frame.render_stateful_widget(widget, area, &mut state);
            })
            .expect("Failed to draw widget");

        snap(terminal.backend().buffer())
    }

    #[test]
    fn test_snapshot_3_options_first_selected() {
        let options = vec![
            QuestionOption {
                label: "Option Alpha".into(),
                description: None,
            },
            QuestionOption {
                label: "Option Beta".into(),
                description: None,
            },
            QuestionOption {
                label: "Option Gamma".into(),
                description: None,
            },
        ];

        let snapshot = render_widget(&options, None, 0, 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_3_options_last_selected() {
        let options = vec![
            QuestionOption {
                label: "Option Alpha".into(),
                description: None,
            },
            QuestionOption {
                label: "Option Beta".into(),
                description: None,
            },
            QuestionOption {
                label: "Option Gamma".into(),
                description: None,
            },
        ];

        let snapshot = render_widget(&options, None, 2, 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_options_with_description() {
        let options = vec![
            QuestionOption {
                label: "JWT".into(),
                description: Some("Token-based authentication".into()),
            },
            QuestionOption {
                label: "Session".into(),
                description: Some("Cookie-based authentication".into()),
            },
            QuestionOption {
                label: "OAuth".into(),
                description: Some("Third-party authentication".into()),
            },
        ];

        let snapshot = render_widget(&options, None, 1, 50, 10);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_with_question_header() {
        let options = vec![
            QuestionOption {
                label: "Option A".into(),
                description: Some("Description A".into()),
            },
            QuestionOption {
                label: "Option B".into(),
                description: None,
            },
        ];

        let snapshot = render_widget(&options, Some("Choose authentication method"), 0, 50, 8);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_single_option() {
        let options = vec![QuestionOption {
            label: "Only choice".into(),
            description: Some("This is the only option".into()),
        }];

        let snapshot = render_widget(&options, Some("Select one"), 0, 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_empty_options() {
        let options: Vec<QuestionOption> = vec![];
        let snapshot = render_widget(&options, Some("No options available"), 0, 40, 3);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_long_labels_truncation() {
        let options = vec![
            QuestionOption {
                label: "Very Long Option Name That Exceeds Terminal Width".into(),
                description: None,
            },
            QuestionOption {
                label: "Another Super Long Option Label".into(),
                description: Some(
                    "Also a very long description text that will be truncated".into(),
                ),
            },
        ];

        let snapshot = render_widget(&options, None, 0, 30, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_narrow_terminal() {
        let options = vec![
            QuestionOption {
                label: "Auth".into(),
                description: Some("Authentication module".into()),
            },
            QuestionOption {
                label: "API".into(),
                description: Some("REST API endpoints".into()),
            },
        ];

        let snapshot = render_widget(&options, Some("Pick"), 1, 20, 8);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_many_options_scrolling() {
        // 10 opcji na height=8 → sprawdź czy renderu się poprawnie (bez scroll logic)
        let options: Vec<QuestionOption> = (1..=10)
            .map(|i| QuestionOption {
                label: format!("Item {i}"),
                description: None,
            })
            .collect();

        let snapshot = render_widget(&options, None, 4, 30, 8);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_mixed_description_presence() {
        let options = vec![
            QuestionOption {
                label: "With desc".into(),
                description: Some("Has description".into()),
            },
            QuestionOption {
                label: "No desc".into(),
                description: None,
            },
            QuestionOption {
                label: "Also with desc".into(),
                description: Some("Another description".into()),
            },
        ];

        let snapshot = render_widget(&options, None, 2, 40, 10);
        insta::assert_snapshot!(snapshot);
    }

    // ── Rapid Cycling Tests ────────────────────────────────────────

    #[test]
    fn test_rapid_up_100_times() {
        let mut state = ChoiceState::new();
        for _ in 0..100 {
            state.move_up(3);
            assert!(state.selected < 3);
        }
        // 100 % 3 = 1, więc 0 - 100 = -100 ≡ 2 (mod 3)
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn test_rapid_down_100_times() {
        let mut state = ChoiceState::new();
        for _ in 0..100 {
            state.move_down(3);
            assert!(state.selected < 3);
        }
        // 100 % 3 = 1
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn test_alternating_up_down_50_cycles() {
        let mut state = ChoiceState::new();
        for _ in 0..50 {
            state.move_up(3);
        }
        for _ in 0..50 {
            state.move_down(3);
        }
        // 50 Up, 50 Down → powinno wrócić do 0
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_stress_10000_up() {
        let mut state = ChoiceState::with_selected(2);
        for _ in 0..10000 {
            state.move_up(3);
            assert!(state.selected < 3);
        }
        // (2 - 10000) % 3 ≡ 1 (mod 3)
        let expected = ((2_i32 - 10000).rem_euclid(3)) as usize;
        assert_eq!(state.selected, expected);
    }

    #[test]
    fn test_stress_10000_down() {
        let mut state = ChoiceState::new();
        for _ in 0..10000 {
            state.move_down(3);
            assert!(state.selected < 3);
        }
        // 10000 % 3 = 1
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn test_wrapping_at_boundaries() {
        // Up z 0 → 2
        let mut state = ChoiceState::new();
        state.move_up(3);
        assert_eq!(state.selected, 2);

        // Down z 2 → 0
        state.move_down(3);
        assert_eq!(state.selected, 0);
    }
}
