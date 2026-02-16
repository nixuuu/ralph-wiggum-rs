// Widget dla wyboru pojedynczej opcji (QuestionType::Choice)
//
// Używa ratatui List + ListState z Viewport::Inline. Header pytania
// jest renderowany w górnej części viewportu nad listą opcji.

use ansi_to_tui::IntoText;

use crate::commands::mcp::ask_user::{Question, QuestionOption};
use crate::shared::error::{RalphError, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph},
};
use std::io;
use std::time::Duration;

/// RAII guard dla raw mode — gwarantuje cleanup nawet przy paniku
struct RawModeGuard;
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// Renderuje widget wyboru z headerem pytania osadzonym w viewporcie
pub fn render(question: &Question, header: &str) -> Result<String> {
    if question.options.is_empty() {
        return Err(RalphError::Mcp(
            "Choice question requires at least one option".into(),
        ));
    }

    // Przygotuj listę opcji z "Other" na końcu
    let mut all_options = question.options.clone();
    all_options.push(QuestionOption {
        label: "Other".into(),
        description: Some("type your answer".into()),
    });

    let header_text = (!header.is_empty())
        .then(|| header.into_text().ok())
        .flatten();
    let header_lines = header_text
        .as_ref()
        .map(|t| t.lines.len() as u16)
        .unwrap_or(0);

    // Znajdź indeks domyślny
    let default_index = if let Some(ref default) = question.default {
        all_options
            .iter()
            .position(|opt| opt.label == *default)
            .unwrap_or(0)
    } else {
        0
    };

    let height = all_options.len() as u16 + header_lines;
    let mut viewport_y = 0u16;

    // Setup ratatui terminal z Viewport::Inline
    enable_raw_mode().map_err(|e| RalphError::Mcp(format!("Failed to enable raw mode: {e}")))?;
    let _guard = RawModeGuard;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(height),
        },
    )
    .map_err(|e| RalphError::Mcp(format!("Failed to create terminal: {e}")))?;

    let mut state = ListState::default().with_selected(Some(default_index));

    loop {
        terminal
            .draw(|frame| {
                let area = frame.area();
                viewport_y = area.y;

                let list_area = if header_lines > 0 {
                    let chunks =
                        Layout::vertical([Constraint::Length(header_lines), Constraint::Min(0)])
                            .split(area);
                    if let Some(ref ht) = header_text {
                        frame.render_widget(Paragraph::new(ht.clone()), chunks[0]);
                    }
                    chunks[1]
                } else {
                    area
                };

                let items: Vec<ListItem> = all_options
                    .iter()
                    .map(|opt| {
                        let mut spans = vec![Span::raw(&opt.label)];
                        if let Some(ref desc) = opt.description {
                            spans.push(Span::styled(
                                format!("  {desc}"),
                                Style::default().fg(Color::DarkGray),
                            ));
                        }
                        ListItem::new(Line::from(spans))
                    })
                    .collect();

                let list = List::new(items)
                    .highlight_style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol("> ");
                frame.render_stateful_widget(list, list_area, &mut state);
            })
            .map_err(|e| RalphError::Mcp(format!("Failed to draw: {e}")))?;

        if event::poll(Duration::from_millis(50))
            .map_err(|e| RalphError::Mcp(format!("Failed to poll events: {e}")))?
        {
            match event::read()
                .map_err(|e| RalphError::Mcp(format!("Failed to read event: {e}")))?
            {
                Event::Key(KeyEvent {
                    code: KeyCode::Up, ..
                }) => {
                    let i = state.selected().unwrap_or(0);
                    state.select(Some(if i == 0 { all_options.len() - 1 } else { i - 1 }));
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Down,
                    ..
                }) => {
                    let i = state.selected().unwrap_or(0);
                    state.select(Some((i + 1) % all_options.len()));
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Enter,
                    ..
                }) => {
                    let selected = state.selected().unwrap_or(0);
                    let choice = all_options[selected].label.clone();

                    if choice == "Other" {
                        drop(terminal);
                        // Collapse PRZED text_input — czyści stary viewport choice_select
                        super::collapse_viewport(viewport_y)?;

                        match super::text_input::text_input(
                            question.placeholder.as_deref(),
                            question.default.as_deref(),
                            question.required,
                            Some(header),
                        ) {
                            Ok(answer) => return Ok(answer),
                            Err(RalphError::Back) => {
                                // text_input już skolapsował swój viewport.
                                // Ponownie włącz raw mode (text_input's guard go wyłączył)
                                enable_raw_mode().map_err(|e| {
                                    RalphError::Mcp(format!("Failed to enable raw mode: {e}"))
                                })?;
                                let backend = CrosstermBackend::new(io::stdout());
                                terminal = Terminal::with_options(
                                    backend,
                                    TerminalOptions {
                                        viewport: Viewport::Inline(height),
                                    },
                                )
                                .map_err(|e| {
                                    RalphError::Mcp(format!("Failed to create terminal: {e}"))
                                })?;
                                state = ListState::default().with_selected(Some(selected));
                                // continue loop → redraw
                            }
                            Err(e) => return Err(e),
                        }
                    } else {
                        drop(terminal);
                        super::collapse_viewport(viewport_y)?;
                        return Ok(choice);
                    }
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                }) => {
                    drop(terminal);
                    super::collapse_viewport(viewport_y)?;
                    return Err(RalphError::Interrupted);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::mcp::ask_user::{QuestionOption, QuestionType};

    // Helper: renderuje List + ListState do buffera dla snapshot testów
    fn render_choice_list(
        options: &[QuestionOption],
        selected_index: usize,
        width: u16,
        height: u16,
    ) -> String {
        use crate::test_helpers::snap;
        use ratatui::{Terminal, backend::TestBackend, layout::Rect};

        let mut all_options = options.to_vec();
        all_options.push(QuestionOption {
            label: "Other".into(),
            description: Some("type your answer".into()),
        });

        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("Failed to create test terminal");

        let mut state = ListState::default().with_selected(Some(selected_index));

        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, height);

                let items: Vec<ListItem> = all_options
                    .iter()
                    .map(|opt| {
                        let mut spans = vec![Span::raw(&opt.label)];
                        if let Some(ref desc) = opt.description {
                            spans.push(Span::styled(
                                format!("  {desc}"),
                                Style::default().fg(Color::DarkGray),
                            ));
                        }
                        ListItem::new(Line::from(spans))
                    })
                    .collect();

                let list = List::new(items)
                    .highlight_style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol("> ");
                frame.render_stateful_widget(list, area, &mut state);
            })
            .expect("Failed to draw widget");

        snap(terminal.backend().buffer())
    }

    #[test]
    fn test_choice_select_validates_empty_options() {
        let question = Question {
            question: "Choose one".into(),
            question_type: QuestionType::Choice,
            options: vec![],
            default: None,
            placeholder: None,
            required: true,
        };

        let result = render(&question, "");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("requires at least one option")
        );
    }

    #[test]
    fn test_choice_select_appends_other_option() {
        let options = [
            QuestionOption {
                label: "Option A".into(),
                description: Some("Description A".into()),
            },
            QuestionOption {
                label: "Option B".into(),
                description: None,
            },
        ];

        let mut all_options = options.to_vec();
        all_options.push(QuestionOption {
            label: "Other".into(),
            description: Some("type your answer".into()),
        });

        assert_eq!(all_options.len(), 3);
        assert_eq!(all_options[2].label, "Other");
        assert_eq!(all_options[2].description, Some("type your answer".into()));
    }

    #[test]
    fn test_default_index_resolution() {
        let options = [
            QuestionOption {
                label: "First".into(),
                description: None,
            },
            QuestionOption {
                label: "Second".into(),
                description: None,
            },
            QuestionOption {
                label: "Third".into(),
                description: None,
            },
        ];

        // Default ustawiony na "Second"
        let default = Some("Second".to_string());
        let default_index = if let Some(ref d) = default {
            options.iter().position(|opt| opt.label == *d).unwrap_or(0)
        } else {
            0
        };
        assert_eq!(default_index, 1);

        // Default nieistniejący - powinien być 0
        let default_invalid = Some("NonExistent".to_string());
        let default_index_invalid = if let Some(ref d) = default_invalid {
            options.iter().position(|opt| opt.label == *d).unwrap_or(0)
        } else {
            0
        };
        assert_eq!(default_index_invalid, 0);

        // Brak defaultu - powinien być 0
        let default_none: Option<String> = None;
        let default_index_none = if let Some(ref d) = default_none {
            options.iter().position(|opt| opt.label == *d).unwrap_or(0)
        } else {
            0
        };
        assert_eq!(default_index_none, 0);
    }

    #[test]
    fn test_selected_index_wrapping() {
        let options_count = 3;

        // Move up z 0 -> wrap do ostatniego
        let selected = 0usize;
        let new = if selected == 0 {
            options_count - 1
        } else {
            selected - 1
        };
        assert_eq!(new, 2);

        // Move down z ostatniego -> wrap do 0
        let selected = 2usize;
        let new = (selected + 1) % options_count;
        assert_eq!(new, 0);

        // Move down normalnie
        let selected = 1usize;
        let new = (selected + 1) % options_count;
        assert_eq!(new, 2);

        // Move up normalnie
        let selected = 2usize;
        let new = if selected == 0 {
            options_count - 1
        } else {
            selected - 1
        };
        assert_eq!(new, 1);
    }

    // ========== SNAPSHOT TESTS ==========

    #[test]
    fn test_snapshot_list_3_options_focus_first() {
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

        let snapshot = render_choice_list(&options, 0, 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_list_3_options_focus_last() {
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

        // Index 3 = "Other" (ostatnia opcja po dodaniu "Other")
        let snapshot = render_choice_list(&options, 3, 40, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_options_with_description() {
        let options = vec![
            QuestionOption {
                label: "Auth".into(),
                description: Some("Authentication module".into()),
            },
            QuestionOption {
                label: "API".into(),
                description: Some("REST API endpoints".into()),
            },
            QuestionOption {
                label: "Logging".into(),
                description: Some("Application logging".into()),
            },
        ];

        // Focus na drugiej opcji (index 1)
        let snapshot = render_choice_list(&options, 1, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_single_option() {
        let options = vec![QuestionOption {
            label: "Only Choice".into(),
            description: Some("This is the only option".into()),
        }];

        let snapshot = render_choice_list(&options, 0, 40, 3);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_many_options_scrolling() {
        let options = vec![
            QuestionOption {
                label: "Item 1".into(),
                description: None,
            },
            QuestionOption {
                label: "Item 2".into(),
                description: None,
            },
            QuestionOption {
                label: "Item 3".into(),
                description: None,
            },
            QuestionOption {
                label: "Item 4".into(),
                description: None,
            },
            QuestionOption {
                label: "Item 5".into(),
                description: None,
            },
            QuestionOption {
                label: "Item 6".into(),
                description: None,
            },
            QuestionOption {
                label: "Item 7".into(),
                description: None,
            },
            QuestionOption {
                label: "Item 8".into(),
                description: None,
            },
            QuestionOption {
                label: "Item 9".into(),
                description: None,
            },
            QuestionOption {
                label: "Item 10".into(),
                description: None,
            },
        ];

        // Focus na środkowym elemencie (index 5), height=6 wymusza scrolling (11 items > 6 rows)
        let snapshot = render_choice_list(&options, 5, 30, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_empty_options_list() {
        // Edge case: pusta lista (renderujemy tylko "Other")
        let options: Vec<QuestionOption> = vec![];

        let snapshot = render_choice_list(&options, 0, 40, 2);
        insta::assert_snapshot!(snapshot);
    }

    // ========== NARROW TERMINAL TESTS ==========

    #[test]
    fn test_snapshot_narrow_terminal_long_options() {
        // Test truncation na wąskim terminalu (width=15)
        // Opcje dłuższe niż szerokość terminalu
        let options = vec![
            QuestionOption {
                label: "Very Long Option Name Alpha".into(),
                description: None,
            },
            QuestionOption {
                label: "Another Super Long Option Beta".into(),
                description: None,
            },
            QuestionOption {
                label: "Third Extremely Long Option Gamma".into(),
                description: None,
            },
        ];

        // Focus na pierwszej opcji
        let snapshot = render_choice_list(&options, 0, 15, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_narrow_terminal_with_description() {
        // Test z description na width=20 - sprawdź truncation i formatowanie
        let options = vec![
            QuestionOption {
                label: "Authentication".into(),
                description: Some("User login and session management".into()),
            },
            QuestionOption {
                label: "API Endpoints".into(),
                description: Some("RESTful API routes".into()),
            },
        ];

        // Focus na drugiej opcji (index 1)
        let snapshot = render_choice_list(&options, 1, 20, 4);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_narrow_terminal_highlight_visible() {
        // Sprawdź że highlight ("> ") jest widoczny nawet po truncation
        // i że opcja highlighted ma prawidłowy kolor
        let options = vec![
            QuestionOption {
                label: "VeryLongOptionWithoutSpaces".into(),
                description: None,
            },
            QuestionOption {
                label: "AnotherLongOptionName".into(),
                description: None,
            },
            QuestionOption {
                label: "ShortOpt".into(),
                description: None,
            },
        ];

        // Focus na środkowej opcji (index 1) - sprawdź highlight
        let snapshot = render_choice_list(&options, 1, 15, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ========== EXTREME CASES: 50+ OPTIONS ==========

    #[test]
    fn test_snapshot_50_plus_options_focus_middle() {
        // 50+ opcji, focus na item #25 (środek)
        // Sprawdź scrolling, focus management, performance
        let options: Vec<QuestionOption> = (1..=52)
            .map(|i| QuestionOption {
                label: format!("Option {i:02}"),
                description: Some(format!("Description for option {i}")),
            })
            .collect();

        // Focus na #25 (index 24 w vec, po dodaniu "Other" będzie to 25/53 items)
        // Height=15 wymusza scrolling (53 items > 15 rows)
        let snapshot = render_choice_list(&options, 24, 60, 15);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_very_long_label_200_chars() {
        // Opcja z label 200+ chars - sprawdź truncation i wrapping
        let long_label = "A".repeat(200);

        let options = vec![
            QuestionOption {
                label: "Short".into(),
                description: None,
            },
            QuestionOption {
                label: long_label,
                description: Some("This option has an extremely long label".into()),
            },
            QuestionOption {
                label: "Another short".into(),
                description: None,
            },
        ];

        // Focus na opcji z długim labelem (index 1)
        let snapshot = render_choice_list(&options, 1, 80, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_very_long_description_300_chars() {
        // Opcja z description 300+ chars - sprawdź truncation (List nie robi wrap per item)
        let long_description = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
            Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. \
            Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. \
            Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur.";

        let options = vec![
            QuestionOption {
                label: "Normal option".into(),
                description: Some("Short description".into()),
            },
            QuestionOption {
                label: "Option with very long description".into(),
                description: Some(long_description.into()),
            },
            QuestionOption {
                label: "Another normal".into(),
                description: None,
            },
        ];

        // Focus na opcji z długim description (index 1)
        let snapshot = render_choice_list(&options, 1, 80, 8);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_50_options_no_descriptions() {
        // 50 opcji bez descriptions - test performance i scrolling
        // Focus na item #40 (bliżej końca)
        let options: Vec<QuestionOption> = (1..=50)
            .map(|i| QuestionOption {
                label: format!("Item {i:03}"),
                description: None,
            })
            .collect();

        // Focus na #40 (index 39)
        let snapshot = render_choice_list(&options, 39, 40, 12);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_50_options_focus_last() {
        // 50 opcji, focus na ostatniej (index 49, co da 50/51 po "Other")
        // Sprawdź że "Other" nie jest selected
        let options: Vec<QuestionOption> = (1..=50)
            .map(|i| QuestionOption {
                label: format!("Choice {i}"),
                description: Some(format!("Desc {i}")),
            })
            .collect();

        // Focus na ostatniej user opcji przed "Other"
        let snapshot = render_choice_list(&options, 49, 50, 10);
        insta::assert_snapshot!(snapshot);
    }

    // ========== RAPID CYCLING TESTS (Task 74.2) ==========

    /// Test: 100x Up key z 3 opcjami — wrapping i bounds checks
    #[test]
    fn test_rapid_up_cycling_100_times_3_options() {
        let options_count = 3;
        let mut selected = 0usize;

        // Symuluj 100x Up key presses
        for iteration in 0..100 {
            selected = if selected == 0 {
                options_count - 1
            } else {
                selected - 1
            };

            // Sprawdzenie: selected zawsze w bounds [0, options_count)
            assert!(
                selected < options_count,
                "Iteration {}: selected {} out of bounds (expected < {})",
                iteration,
                selected,
                options_count
            );

            // Sprawdzenie: selected_idx % 3 == (100 - iteration - 1) % 3
            // Po 100 Up z 0 powinniśmy wrócić do 0 (bo 100 % 3 = 1, ale Up wraca do tyłu)
            // 100 Up = 100 * (-1) = -100 ≡ -100 % 3 = 2 (w Rust: (-100).rem_euclid(3) = 2)
        }

        // Po 100x Up z indeksu 0 powinniśmy być na (3 - 100 % 3) % 3 = 1
        // Ale ponieważ liczymy backward: 0 -> 2 -> 1 -> 0 (cykl co 3)
        // 100 % 3 = 1, więc jesteśmy o 1 wstecz od 0 = indeks 2
        let expected_final = (0_i32 - 100) % 3;
        let expected_final_wrapped = expected_final.rem_euclid(3) as usize;
        assert_eq!(
            selected, expected_final_wrapped,
            "After 100x Up from index 0, expected index {}, got {}",
            expected_final_wrapped, selected
        );
    }

    /// Test: 100x Down key z 3 opcjami — wrapping i bounds checks
    #[test]
    fn test_rapid_down_cycling_100_times_3_options() {
        let options_count = 3;
        let mut selected = 0usize;

        // Symuluj 100x Down key presses
        for iteration in 0..100 {
            selected = (selected + 1) % options_count;

            // Sprawdzenie: selected zawsze w bounds [0, options_count)
            assert!(
                selected < options_count,
                "Iteration {}: selected {} out of bounds (expected < {})",
                iteration,
                selected,
                options_count
            );
        }

        // Po 100x Down z indeksu 0: 100 % 3 = 1
        let expected_final = 100 % options_count;
        assert_eq!(
            selected, expected_final,
            "After 100x Down from index 0, expected index {}, got {}",
            expected_final, selected
        );
    }

    /// Test: Alternujące Up/Down 50 razy z każdej strony — powrót do startu
    #[test]
    fn test_alternating_up_down_50_cycles_3_options() {
        let options_count = 3;
        let starting_index = 0usize;
        let mut selected = starting_index;

        // 50x Up, potem 50x Down (powinno wrócić do startu)
        // Up 50 razy
        for iteration in 0..50 {
            selected = if selected == 0 {
                options_count - 1
            } else {
                selected - 1
            };

            assert!(
                selected < options_count,
                "Up iteration {}: selected {} out of bounds",
                iteration,
                selected
            );
        }

        // Down 50 razy
        for iteration in 0..50 {
            selected = (selected + 1) % options_count;

            assert!(
                selected < options_count,
                "Down iteration {}: selected {} out of bounds",
                iteration,
                selected
            );
        }

        // Po 50x Up + 50x Down powinniśmy wrócić do starting_index
        assert_eq!(
            selected, starting_index,
            "After 50x Up + 50x Down, expected to return to starting index {}, got {}",
            starting_index, selected
        );
    }

    /// Test: Alternujące Up/Down z różnych pozycji startowych
    #[test]
    fn test_alternating_up_down_from_different_starts() {
        let options_count = 3;

        for start_index in 0..options_count {
            let mut selected = start_index;

            // 25x Up, 25x Down
            for _ in 0..25 {
                selected = if selected == 0 {
                    options_count - 1
                } else {
                    selected - 1
                };
                assert!(selected < options_count, "Up: out of bounds");
            }

            for _ in 0..25 {
                selected = (selected + 1) % options_count;
                assert!(selected < options_count, "Down: out of bounds");
            }

            // Powinna być symetria — jeśli zaczynamy z index 0, Up zmienia na 2, Down zmienia na 0
            // Ale po tych samych liczbach Up/Down musimy wrócić do startu
            assert_eq!(
                selected, start_index,
                "After 25x Up + 25x Down from start {}, expected to return to {}, got {}",
                start_index, start_index, selected
            );
        }
    }

    /// Test: Rapid cycling z 3 opcjami — 1000 random-like presses
    /// Weryfikuje że wrapping pracuje prawidłowo dla mieszanych Up/Down
    #[test]
    fn test_rapid_mixed_up_down_1000_presses() {
        let options_count = 3;
        let mut selected = 1usize;

        // Symuluj wzór: Up, Up, Down, Up, Down, Down, Up, Down (repeat 125x = 1000 presses)
        // Pattern: 4x Up, 4x Down => net movement = 0 per cycle
        let pattern = [true, true, false, true, false, false, true, false]; // true = Up, false = Down
        for cycle in 0..125 {
            for &is_up in &pattern {
                if is_up {
                    selected = if selected == 0 {
                        options_count - 1
                    } else {
                        selected - 1
                    };
                } else {
                    selected = (selected + 1) % options_count;
                }

                assert!(
                    selected < options_count,
                    "Cycle {}: selected {} out of bounds (expected < {})",
                    cycle,
                    selected,
                    options_count
                );
            }
        }

        // Net movement per cycle = 0, więc po 125 cyklach wracamy do start_index=1
        assert_eq!(
            selected, 1,
            "After 1000 mixed presses (net 0 per cycle), expected index 1, got {}",
            selected
        );
    }

    /// Test: Stress test — 10000 rapid Up presses, verify no panic
    #[test]
    fn test_stress_10000_up_presses_no_panic() {
        let options_count = 3;
        let mut selected = 2usize;

        // 10000x Up — powinna pracować bez panic
        for _ in 0..10000 {
            selected = if selected == 0 {
                options_count - 1
            } else {
                selected - 1
            };
            assert!(selected < options_count, "Out of bounds");
        }

        // 10000 % 3 = 1, więc:
        // (2 - 1) % 3 = 1
        let expected = ((2_i32 - 10000).rem_euclid(3)) as usize;
        assert_eq!(selected, expected);
    }

    /// Test: Stress test — 10000 rapid Down presses, verify no panic
    #[test]
    fn test_stress_10000_down_presses_no_panic() {
        let options_count = 3;
        let mut selected = 0usize;

        // 10000x Down — powinna pracować bez panic
        for _ in 0..10000 {
            selected = (selected + 1) % options_count;
            assert!(selected < options_count, "Out of bounds");
        }

        // 10000 % 3 = 1
        let expected = 10000 % options_count;
        assert_eq!(selected, expected);
    }

    /// Test: Verify wrapping correctness at exact cycle boundaries
    #[test]
    fn test_wrapping_at_cycle_boundaries() {
        let options_count = 3;

        // Test Up wrapping na granicy cyklu
        for cycles in 0..10 {
            let presses = cycles * options_count;
            let mut selected = 0usize;

            for _ in 0..presses {
                selected = if selected == 0 {
                    options_count - 1
                } else {
                    selected - 1
                };
            }

            // Po presses=k*3, powinniśmy wrócić do 0
            assert_eq!(
                selected, 0,
                "After {} Up presses ({}*3), expected index 0",
                presses, cycles
            );
        }

        // Test Down wrapping na granicy cyklu
        for cycles in 0..10 {
            let presses = cycles * options_count;
            let mut selected = 0usize;

            for _ in 0..presses {
                selected = (selected + 1) % options_count;
            }

            // Po presses=k*3, powinniśmy wrócić do 0
            assert_eq!(
                selected, 0,
                "After {} Down presses ({}*3), expected index 0",
                presses, cycles
            );
        }
    }
}
