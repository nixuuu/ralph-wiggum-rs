//! Testy integracyjne dla ask_user inline widgets.
//!
//! Testuje pełny workflow interakcji z użytkownikiem:
//! - TextInput: wpisywanie tekstu, submit/cancel
//! - Choice: nawigacja, wybór opcji
//! - MultiChoice: toggle opcji, submit
//! - Confirm: przełączanie Yes/No, submit/cancel
//!
//! Używa `TestApp` z test_helpers.rs do symulacji event loop.

#[cfg(test)]
mod integration_tests {
    use crossterm::event::{KeyCode, KeyEventKind};
    use ratatui::Frame;
    use ratatui::layout::Rect;

    use crate::tui::app::AppState;
    use crate::tui::events::{AppEvent, EventResult};
    use crate::tui::test_helpers::{TestApp, buffer_contains_text, make_key};
    use crate::tui::widgets::{
        AskUserAction, AskUserQuestion, AskUserWidget, QuestionKind, QuestionOption,
    };

    // ── Test AppState wrapper ────────────────────────────────────────────

    /// Prosty AppState opakowujący AskUserWidget do testów integracyjnych.
    struct AskUserTestState {
        widget: AskUserWidget,
        /// Zachowany wynik Submit/Cancel
        result: Option<SubmitResult>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum SubmitResult {
        Submitted(String),
        Cancelled,
    }

    impl AskUserTestState {
        fn new(question: AskUserQuestion) -> Self {
            Self {
                widget: AskUserWidget::new(question),
                result: None,
            }
        }
    }

    impl AppState for AskUserTestState {
        fn draw(&mut self, frame: &mut Frame, area: Rect) {
            let widget_height = self.widget.height_for_width(area.width);
            let widget_area = Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: widget_height.min(area.height),
            };
            frame.render_widget(self.widget.clone(), widget_area);
        }

        fn handle_event(
            &mut self,
            event: AppEvent,
            _resolver: &crate::tui::KeybindingResolver,
        ) -> EventResult {
            match event {
                AppEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    // Przekazujemy cały KeyEvent (z modyfikatorami) — obsługa Shift+Enter
                    match self.widget.handle_key(key) {
                        AskUserAction::Continue => EventResult::Consumed,
                        AskUserAction::Submit(answer) => {
                            self.result = Some(SubmitResult::Submitted(answer));
                            EventResult::Quit
                        }
                        AskUserAction::Cancel => {
                            self.result = Some(SubmitResult::Cancelled);
                            EventResult::Quit
                        }
                    }
                }
                _ => EventResult::Ignored,
            }
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    fn text_question() -> AskUserQuestion {
        AskUserQuestion {
            question: "What is your name?".into(),
            kind: QuestionKind::Text,
            options: vec![],
            default: None,
            placeholder: Some("Enter name".into()),
        }
    }

    fn choice_question() -> AskUserQuestion {
        AskUserQuestion {
            question: "Choose auth method".into(),
            kind: QuestionKind::Choice,
            options: vec![
                QuestionOption {
                    label: "JWT".into(),
                    description: Some("Token-based".into()),
                },
                QuestionOption {
                    label: "Session".into(),
                    description: Some("Cookie-based".into()),
                },
                QuestionOption {
                    label: "OAuth".into(),
                    description: Some("Third-party".into()),
                },
            ],
            default: Some("JWT".into()),
            placeholder: None,
        }
    }

    fn multi_question() -> AskUserQuestion {
        AskUserQuestion {
            question: "Select features".into(),
            kind: QuestionKind::MultiChoice,
            options: vec![
                QuestionOption {
                    label: "Auth".into(),
                    description: Some("Authentication".into()),
                },
                QuestionOption {
                    label: "API".into(),
                    description: Some("REST API".into()),
                },
                QuestionOption {
                    label: "Logging".into(),
                    description: Some("Audit logs".into()),
                },
            ],
            default: None,
            placeholder: None,
        }
    }

    fn confirm_question() -> AskUserQuestion {
        AskUserQuestion {
            question: "Are you sure?".into(),
            kind: QuestionKind::Confirm,
            options: vec![],
            default: Some("yes".into()),
            placeholder: None,
        }
    }

    // ── Text Input Tests ─────────────────────────────────────────────────

    #[test]
    fn test_text_input_type_and_submit() {
        let state = AskUserTestState::new(text_question());
        let mut app = TestApp::new(state, 80, 10);

        // Wpisz "hello"
        app.inject_keys(vec![
            make_key(KeyCode::Char('h')),
            make_key(KeyCode::Char('e')),
            make_key(KeyCode::Char('l')),
            make_key(KeyCode::Char('l')),
            make_key(KeyCode::Char('o')),
        ]);
        app.drain_events();

        // Submit
        app.inject_key(make_key(KeyCode::Enter));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(
            app.state().result,
            Some(SubmitResult::Submitted("hello".into()))
        );
    }

    #[test]
    fn test_text_input_empty_submit() {
        let state = AskUserTestState::new(text_question());
        let mut app = TestApp::new(state, 80, 10);

        // Submit bez wpisywania
        app.inject_key(make_key(KeyCode::Enter));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(app.state().result, Some(SubmitResult::Submitted("".into())));
    }

    #[test]
    fn test_text_input_esc_cancel() {
        let state = AskUserTestState::new(text_question());
        let mut app = TestApp::new(state, 80, 10);

        // Wpisz tekst i anuluj
        app.inject_keys(vec![
            make_key(KeyCode::Char('t')),
            make_key(KeyCode::Char('e')),
            make_key(KeyCode::Char('s')),
            make_key(KeyCode::Char('t')),
            make_key(KeyCode::Esc),
        ]);
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(app.state().result, Some(SubmitResult::Cancelled));
    }

    // ── Choice Tests ─────────────────────────────────────────────────────

    #[test]
    fn test_choice_select_first_option() {
        let state = AskUserTestState::new(choice_question());
        let mut app = TestApp::new(state, 80, 10);

        // Submit (domyślnie wybrana pierwsza opcja JWT)
        app.inject_key(make_key(KeyCode::Enter));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(
            app.state().result,
            Some(SubmitResult::Submitted("JWT".into()))
        );
    }

    #[test]
    fn test_choice_navigate_down_and_submit() {
        let state = AskUserTestState::new(choice_question());
        let mut app = TestApp::new(state, 80, 10);

        // Nawiguj w dół na drugą opcję (Session)
        app.inject_key(make_key(KeyCode::Down));
        app.step();

        // Submit
        app.inject_key(make_key(KeyCode::Enter));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(
            app.state().result,
            Some(SubmitResult::Submitted("Session".into()))
        );
    }

    #[test]
    fn test_choice_navigate_down_twice_and_submit() {
        let state = AskUserTestState::new(choice_question());
        let mut app = TestApp::new(state, 80, 10);

        // Nawiguj w dół 2 razy na trzecią opcję (OAuth)
        app.inject_keys(vec![make_key(KeyCode::Down), make_key(KeyCode::Down)]);
        app.drain_events();

        // Submit
        app.inject_key(make_key(KeyCode::Enter));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(
            app.state().result,
            Some(SubmitResult::Submitted("OAuth".into()))
        );
    }

    #[test]
    fn test_choice_esc_cancel() {
        let state = AskUserTestState::new(choice_question());
        let mut app = TestApp::new(state, 80, 10);

        // Anuluj
        app.inject_key(make_key(KeyCode::Esc));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(app.state().result, Some(SubmitResult::Cancelled));
    }

    // ── MultiSelect Tests ────────────────────────────────────────────────

    #[test]
    fn test_multi_toggle_first_and_submit() {
        let state = AskUserTestState::new(multi_question());
        let mut app = TestApp::new(state, 80, 10);

        // Toggle pierwszej opcji (Auth)
        app.inject_key(make_key(KeyCode::Char(' ')));
        app.step();

        // Submit
        app.inject_key(make_key(KeyCode::Enter));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(
            app.state().result,
            Some(SubmitResult::Submitted("Auth".into()))
        );
    }

    #[test]
    fn test_multi_toggle_two_options_and_submit() {
        let state = AskUserTestState::new(multi_question());
        let mut app = TestApp::new(state, 80, 10);

        // Toggle pierwszej opcji (Auth)
        app.inject_key(make_key(KeyCode::Char(' ')));
        app.step();

        // Nawiguj w dół
        app.inject_key(make_key(KeyCode::Down));
        app.step();

        // Toggle drugiej opcji (API)
        app.inject_key(make_key(KeyCode::Char(' ')));
        app.step();

        // Submit
        app.inject_key(make_key(KeyCode::Enter));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(
            app.state().result,
            Some(SubmitResult::Submitted("Auth, API".into()))
        );
    }

    #[test]
    fn test_multi_empty_submit() {
        let state = AskUserTestState::new(multi_question());
        let mut app = TestApp::new(state, 80, 10);

        // Submit bez wyboru (pusta lista)
        app.inject_key(make_key(KeyCode::Enter));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(app.state().result, Some(SubmitResult::Submitted("".into())));
    }

    #[test]
    fn test_multi_esc_cancel() {
        let state = AskUserTestState::new(multi_question());
        let mut app = TestApp::new(state, 80, 10);

        // Toggle opcji i anuluj
        app.inject_keys(vec![
            make_key(KeyCode::Char(' ')),
            make_key(KeyCode::Down),
            make_key(KeyCode::Char(' ')),
            make_key(KeyCode::Esc),
        ]);
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(app.state().result, Some(SubmitResult::Cancelled));
    }

    // ── Confirm Tests ────────────────────────────────────────────────────

    #[test]
    fn test_confirm_default_yes_submit() {
        let state = AskUserTestState::new(confirm_question());
        let mut app = TestApp::new(state, 80, 10);

        // Submit (domyślnie yes)
        app.inject_key(make_key(KeyCode::Enter));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(
            app.state().result,
            Some(SubmitResult::Submitted("yes".into()))
        );
    }

    #[test]
    fn test_confirm_toggle_to_no_and_submit() {
        let state = AskUserTestState::new(confirm_question());
        let mut app = TestApp::new(state, 80, 10);

        // Przełącz na No
        app.inject_key(make_key(KeyCode::Char('n')));
        app.step();

        // Submit
        app.inject_key(make_key(KeyCode::Enter));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(
            app.state().result,
            Some(SubmitResult::Submitted("no".into()))
        );
    }

    #[test]
    fn test_confirm_toggle_with_tab() {
        let state = AskUserTestState::new(confirm_question());
        let mut app = TestApp::new(state, 80, 10);

        // Przełącz Tab (yes → no)
        app.inject_key(make_key(KeyCode::Tab));
        app.step();

        // Submit
        app.inject_key(make_key(KeyCode::Enter));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(
            app.state().result,
            Some(SubmitResult::Submitted("no".into()))
        );
    }

    #[test]
    fn test_confirm_esc_cancel() {
        let state = AskUserTestState::new(confirm_question());
        let mut app = TestApp::new(state, 80, 10);

        // Anuluj
        app.inject_key(make_key(KeyCode::Esc));
        app.drain_events();

        // Sprawdź wynik
        assert_eq!(app.state().result, Some(SubmitResult::Cancelled));
    }

    // ── Render Tests ─────────────────────────────────────────────────────

    #[test]
    fn test_render_text_widget() {
        let state = AskUserTestState::new(text_question());
        let mut app = TestApp::new(state, 80, 10);

        let buffer = app.render();
        assert!(
            buffer_contains_text(&buffer, "What is your name?"),
            "Pytanie powinno być widoczne w buforze"
        );
    }

    #[test]
    fn test_render_choice_widget() {
        let state = AskUserTestState::new(choice_question());
        let mut app = TestApp::new(state, 80, 10);

        let buffer = app.render();
        assert!(
            buffer_contains_text(&buffer, "Choose auth method"),
            "Pytanie powinno być widoczne w buforze"
        );
    }

    #[test]
    fn test_render_after_text_input() {
        let state = AskUserTestState::new(text_question());
        let mut app = TestApp::new(state, 80, 10);

        // Wpisz tekst
        app.inject_keys(vec![
            make_key(KeyCode::Char('h')),
            make_key(KeyCode::Char('i')),
        ]);
        app.drain_events();

        let buffer = app.render();
        assert!(
            buffer_contains_text(&buffer, "hi"),
            "Wpisany tekst powinien być widoczny w buforze"
        );
    }

    // ── Edge Case Tests ─────────────────────────────────────────────────

    #[test]
    fn test_text_input_backspace_and_submit() {
        let state = AskUserTestState::new(text_question());
        let mut app = TestApp::new(state, 80, 10);

        // Wpisz "helo" → Backspace → "hel" → wpisz "lo" → "hello"
        app.inject_keys(vec![
            make_key(KeyCode::Char('h')),
            make_key(KeyCode::Char('e')),
            make_key(KeyCode::Char('l')),
            make_key(KeyCode::Char('o')),
            make_key(KeyCode::Backspace),
            make_key(KeyCode::Char('l')),
            make_key(KeyCode::Char('o')),
            make_key(KeyCode::Enter),
        ]);
        app.drain_events();

        assert_eq!(
            app.state().result,
            Some(SubmitResult::Submitted("hello".into()))
        );
    }

    #[test]
    fn test_choice_wrap_around_up() {
        // choice_question() ma 3 opcje: JWT(0), Session(1), OAuth(2) + "Other"(3)
        // ↑ z JWT(0) → wrap na "Other"(3)
        // Enter na "Other" aktywuje text input (Continue, nie Submit)
        // Esc wraca do listy, ↑ daje OAuth(2)
        let state = AskUserTestState::new(choice_question());
        let mut app = TestApp::new(state, 80, 15);

        // ↑ z JWT → "Other", Enter → aktivuje text input, Esc → wróć do listy
        // ↑ → OAuth, Enter → Submit("OAuth")
        app.inject_keys(vec![
            make_key(KeyCode::Up),    // → "Other" (index 3)
            make_key(KeyCode::Enter), // aktywuje text input
            make_key(KeyCode::Esc),   // wróć do listy
            make_key(KeyCode::Up),    // → OAuth (index 2)
            make_key(KeyCode::Enter), // Submit "OAuth"
        ]);
        app.drain_events();

        assert_eq!(
            app.state().result,
            Some(SubmitResult::Submitted("OAuth".into()))
        );
    }

    #[test]
    fn test_multi_toggle_untoggle_submit() {
        let state = AskUserTestState::new(multi_question());
        let mut app = TestApp::new(state, 80, 10);

        // Toggle Auth ON → Toggle Auth OFF → Submit (pusta lista)
        app.inject_keys(vec![
            make_key(KeyCode::Char(' ')),
            make_key(KeyCode::Char(' ')),
            make_key(KeyCode::Enter),
        ]);
        app.drain_events();

        assert_eq!(app.state().result, Some(SubmitResult::Submitted("".into())));
    }

    #[test]
    fn test_confirm_y_shortcut_after_toggle() {
        let state = AskUserTestState::new(confirm_question());
        let mut app = TestApp::new(state, 80, 10);

        // Przełącz na No, potem z powrotem na Yes przez 'y'
        app.inject_keys(vec![
            make_key(KeyCode::Char('n')),
            make_key(KeyCode::Char('y')),
            make_key(KeyCode::Enter),
        ]);
        app.drain_events();

        assert_eq!(
            app.state().result,
            Some(SubmitResult::Submitted("yes".into()))
        );
    }
}
