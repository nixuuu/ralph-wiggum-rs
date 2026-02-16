// Widget dla wyboru wielu opcji (QuestionType::MultiChoice)
//
// Używa ratatui List + ListState z checkboxami [x]/[ ] i Viewport::Inline.
// Header pytania jest renderowany w górnej części viewportu nad listą opcji.

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
use std::collections::HashSet;
use std::io;
use std::time::Duration;

/// RAII guard dla raw mode
struct RawModeGuard;
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// Renderuje widget multi-select z headerem pytania osadzonym w viewporcie
pub fn render(question: &Question, header: &str) -> Result<String> {
    if question.options.is_empty() {
        return Err(RalphError::Mcp(
            "MultiChoice question requires at least one option".into(),
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

    let other_index = all_options.len() - 1;
    let height = all_options.len() as u16 + header_lines;
    let mut viewport_y = 0u16;

    // Setup ratatui terminal
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

    let mut list_state = ListState::default().with_selected(Some(0));
    let mut toggled = HashSet::<usize>::new();
    let mut custom_answer: Option<String> = None;

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
                    .enumerate()
                    .map(|(i, opt)| {
                        let checkbox = if toggled.contains(&i) { "[x] " } else { "[ ] " };
                        let mut spans = vec![Span::raw(checkbox), Span::raw(&opt.label)];
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
                frame.render_stateful_widget(list, list_area, &mut list_state);
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
                    let i = list_state.selected().unwrap_or(0);
                    list_state.select(Some(if i == 0 { all_options.len() - 1 } else { i - 1 }));
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Down,
                    ..
                }) => {
                    let i = list_state.selected().unwrap_or(0);
                    list_state.select(Some((i + 1) % all_options.len()));
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Char(' '),
                    modifiers: KeyModifiers::NONE,
                    ..
                }) => {
                    let cursor = list_state.selected().unwrap_or(0);

                    if toggled.contains(&cursor) {
                        toggled.remove(&cursor);
                        if cursor == other_index {
                            custom_answer = None;
                        }
                    } else {
                        toggled.insert(cursor);

                        // Jeśli to "Other", otwórz text_input
                        if cursor == other_index {
                            drop(terminal);
                            // Collapse PRZED text_input — czyści stary viewport multi_select
                            super::collapse_viewport(viewport_y)?;

                            match super::text_input::text_input(
                                question.placeholder.as_deref(),
                                question.default.as_deref(),
                                question.required,
                                Some(header),
                            ) {
                                Ok(answer) => {
                                    custom_answer = Some(answer);
                                }
                                Err(RalphError::Interrupted) => {
                                    // Anulowano text_input — odznacz "Other"
                                    toggled.remove(&other_index);
                                }
                                Err(RalphError::Back) => {
                                    // Wróć z text_input — odznacz "Other"
                                    toggled.remove(&other_index);
                                }
                                Err(e) => return Err(e),
                            }

                            // text_input skolapsował swój viewport — odtwórz terminal
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
                            list_state = ListState::default().with_selected(Some(cursor));
                        }
                    }
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Enter,
                    ..
                }) => {
                    // Zbierz labels zaznaczonych opcji (bez "Other")
                    let mut selected_labels: Vec<String> = all_options
                        .iter()
                        .enumerate()
                        .filter(|(idx, _)| toggled.contains(idx) && *idx != other_index)
                        .map(|(_, opt)| opt.label.clone())
                        .collect();

                    // Dodaj custom answer jeśli "Other" zaznaczone
                    if toggled.contains(&other_index)
                        && let Some(answer) = custom_answer
                    {
                        selected_labels.push(answer);
                    }

                    drop(terminal);
                    super::collapse_viewport(viewport_y)?;
                    return Ok(selected_labels.join(", "));
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
    use crate::commands::mcp::ask_user::QuestionType;
    use crate::test_helpers::snap;
    use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect};

    /// Helper do renderowania List widget z ListState do bufora testowego
    fn render_list_to_buffer(
        items: Vec<ListItem>,
        width: u16,
        height: u16,
        selected: Option<usize>,
    ) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("Failed to create test terminal");
        let mut list_state = ListState::default().with_selected(selected);

        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, height);
                let list = List::new(items.clone())
                    .highlight_style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol("> ");
                frame.render_stateful_widget(list, area, &mut list_state);
            })
            .expect("Failed to draw widget");
        terminal.backend().buffer().clone()
    }

    #[test]
    fn test_snapshot_no_selection() {
        let options = vec![
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("Auth")])),
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("API")])),
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("Logging")])),
        ];

        let buffer = render_list_to_buffer(options, 30, 3, Some(0));
        insta::assert_snapshot!(snap(&buffer), @"
        > [ ] Auth
          [ ] API
          [ ] Logging
        ");
    }

    #[test]
    fn test_snapshot_first_and_third_checked() {
        let toggled = HashSet::from([0, 2]);
        let options = vec![
            ListItem::new(Line::from(vec![
                Span::raw(if toggled.contains(&0) { "[x] " } else { "[ ] " }),
                Span::raw("Auth"),
            ])),
            ListItem::new(Line::from(vec![
                Span::raw(if toggled.contains(&1) { "[x] " } else { "[ ] " }),
                Span::raw("API"),
            ])),
            ListItem::new(Line::from(vec![
                Span::raw(if toggled.contains(&2) { "[x] " } else { "[ ] " }),
                Span::raw("Logging"),
            ])),
        ];

        let buffer = render_list_to_buffer(options, 30, 3, Some(0));
        insta::assert_snapshot!(snap(&buffer), @"
        > [x] Auth
          [ ] API
          [x] Logging
        ");
    }

    #[test]
    fn test_snapshot_with_other_option() {
        let options = vec![
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("Auth")])),
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("API")])),
            ListItem::new(Line::from(vec![
                Span::raw("[ ] "),
                Span::raw("Other"),
                Span::styled("  type your answer", Style::default().fg(Color::DarkGray)),
            ])),
        ];

        let buffer = render_list_to_buffer(options, 40, 3, Some(0));
        insta::assert_snapshot!(snap(&buffer), @"
        > [ ] Auth
          [ ] API
          [ ] Other  type your answer
        ");
    }

    #[test]
    fn test_snapshot_single_item_checked() {
        let options = vec![
            ListItem::new(Line::from(vec![Span::raw("[x] "), Span::raw("Auth")])),
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("API")])),
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("Logging")])),
        ];

        let buffer = render_list_to_buffer(options, 30, 3, Some(1));
        insta::assert_snapshot!(snap(&buffer), @"
          [x] Auth
        > [ ] API
          [ ] Logging
        ");
    }

    #[test]
    fn test_snapshot_empty_list_edge_case() {
        let options: Vec<ListItem> = vec![];
        let buffer = render_list_to_buffer(options, 30, 3, None);
        insta::assert_snapshot!(snap(&buffer), @"");
    }

    #[test]
    fn test_snapshot_focus_on_second_item() {
        let options = vec![
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("Auth")])),
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("API")])),
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("Logging")])),
        ];

        let buffer = render_list_to_buffer(options, 30, 3, Some(1));
        insta::assert_snapshot!(snap(&buffer), @"
          [ ] Auth
        > [ ] API
          [ ] Logging
        ");
    }

    #[test]
    fn test_snapshot_focus_on_last_item() {
        let options = vec![
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("Auth")])),
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("API")])),
            ListItem::new(Line::from(vec![Span::raw("[ ] "), Span::raw("Logging")])),
        ];

        let buffer = render_list_to_buffer(options, 30, 3, Some(2));
        insta::assert_snapshot!(snap(&buffer), @"
          [ ] Auth
          [ ] API
        > [ ] Logging
        ");
    }

    #[test]
    fn test_multi_select_validates_empty_options() {
        let question = Question {
            question: "Select multiple".into(),
            question_type: QuestionType::MultiChoice,
            options: vec![],
            default: None,
            placeholder: None,
            required: false,
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
    fn test_multi_select_appends_other_option() {
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
    }

    #[test]
    fn test_checkbox_rendering_format() {
        let toggled = HashSet::from([0, 2]);

        let checkbox_checked = if toggled.contains(&0) { "[x] " } else { "[ ] " };
        assert_eq!(checkbox_checked, "[x] ");

        let checkbox_unchecked = if toggled.contains(&1) { "[x] " } else { "[ ] " };
        assert_eq!(checkbox_unchecked, "[ ] ");

        let checkbox_checked_2 = if toggled.contains(&2) { "[x] " } else { "[ ] " };
        assert_eq!(checkbox_checked_2, "[x] ");
    }

    #[test]
    fn test_toggle_set_operations() {
        let mut toggled = HashSet::<usize>::new();

        toggled.insert(0);
        assert!(toggled.contains(&0));
        assert_eq!(toggled.len(), 1);

        toggled.insert(2);
        assert!(toggled.contains(&2));
        assert_eq!(toggled.len(), 2);

        toggled.remove(&0);
        assert!(!toggled.contains(&0));
        assert_eq!(toggled.len(), 1);
    }

    #[test]
    fn test_comma_separated_output() {
        let selected_labels = ["Auth".to_string(), "API".to_string(), "Logging".to_string()];
        assert_eq!(selected_labels.join(", "), "Auth, API, Logging");

        let single = ["Auth".to_string()];
        assert_eq!(single.join(", "), "Auth");

        let empty: [String; 0] = [];
        assert_eq!(empty.join(", "), "");
    }

    #[test]
    fn test_filtering_selected_labels() {
        let options = [
            QuestionOption {
                label: "Auth".into(),
                description: None,
            },
            QuestionOption {
                label: "API".into(),
                description: None,
            },
            QuestionOption {
                label: "Logging".into(),
                description: None,
            },
            QuestionOption {
                label: "Other".into(),
                description: Some("type your answer".into()),
            },
        ];

        let toggled = HashSet::from([0, 2, 3]);
        let other_index = 3;

        let selected_labels: Vec<String> = options
            .iter()
            .enumerate()
            .filter(|(idx, _)| toggled.contains(idx) && *idx != other_index)
            .map(|(_, opt)| opt.label.clone())
            .collect();

        assert_eq!(selected_labels, vec!["Auth", "Logging"]);
    }

    #[test]
    fn test_cursor_navigation_wrapping() {
        let options_count = 3;

        let cursor = 0usize;
        let new = if cursor == 0 {
            options_count - 1
        } else {
            cursor - 1
        };
        assert_eq!(new, 2);

        let cursor = 2usize;
        let new = (cursor + 1) % options_count;
        assert_eq!(new, 0);
    }

    #[test]
    fn test_other_option_index() {
        let options = [
            QuestionOption {
                label: "A".into(),
                description: None,
            },
            QuestionOption {
                label: "B".into(),
                description: None,
            },
        ];

        let mut all_options = options.to_vec();
        all_options.push(QuestionOption {
            label: "Other".into(),
            description: Some("type your answer".into()),
        });

        let other_index = all_options.len() - 1;
        assert_eq!(other_index, 2);
        assert_eq!(all_options[other_index].label, "Other");
    }

    #[test]
    fn test_hashset_empty_result() {
        let options = [
            QuestionOption {
                label: "Auth".into(),
                description: None,
            },
            QuestionOption {
                label: "API".into(),
                description: None,
            },
        ];

        let toggled = HashSet::<usize>::new();
        let other_index = options.len();

        let selected_labels: Vec<String> = options
            .iter()
            .enumerate()
            .filter(|(idx, _)| toggled.contains(idx) && *idx != other_index)
            .map(|(_, opt)| opt.label.clone())
            .collect();

        assert!(selected_labels.is_empty());
    }

    // Task 72.2: Nowe snapshot testy dla multi_select

    /// Helper: buduje ListItem'y z checkboxami na podstawie labeli i zbioru zaznaczonych indeksów
    fn build_checkbox_items(labels: &[&str], toggled: &HashSet<usize>) -> Vec<ListItem<'static>> {
        labels
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let checkbox = if toggled.contains(&i) { "[x] " } else { "[ ] " };
                ListItem::new(Line::from(vec![
                    Span::raw(checkbox),
                    Span::raw(label.to_string()),
                ]))
            })
            .collect()
    }

    #[test]
    fn test_snapshot_all_five_checked() {
        let labels = ["Option A", "Option B", "Option C", "Option D", "Option E"];
        let toggled = HashSet::from([0, 1, 2, 3, 4]);
        let options = build_checkbox_items(&labels, &toggled);

        let buffer = render_list_to_buffer(options, 30, 5, Some(0));
        insta::assert_snapshot!(snap(&buffer), @"
        > [x] Option A
          [x] Option B
          [x] Option C
          [x] Option D
          [x] Option E
        ");
    }

    #[test]
    fn test_snapshot_toggle_option_2_off() {
        // Option B (index 1) odznaczona — reszta zaznaczona, kursor na B
        let labels = ["Option A", "Option B", "Option C", "Option D", "Option E"];
        let toggled = HashSet::from([0, 2, 3, 4]);
        let options = build_checkbox_items(&labels, &toggled);

        let buffer = render_list_to_buffer(options, 30, 5, Some(1));
        insta::assert_snapshot!(snap(&buffer), @"
          [x] Option A
        > [ ] Option B
          [x] Option C
          [x] Option D
          [x] Option E
        ");
    }

    #[test]
    fn test_snapshot_scrolling_20_options_height_10() {
        let toggled = HashSet::from([0, 5, 10, 15]);
        let labels: Vec<String> = (1..=20).map(|i| format!("Item {:02}", i)).collect();
        let options: Vec<ListItem> = labels
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let checkbox = if toggled.contains(&i) { "[x] " } else { "[ ] " };
                ListItem::new(Line::from(vec![
                    Span::raw(checkbox),
                    Span::raw(label.clone()),
                ]))
            })
            .collect();

        let buffer = render_list_to_buffer(options, 30, 10, Some(5));
        insta::assert_snapshot!(snap(&buffer), @"
          [x] Item 01
          [ ] Item 02
          [ ] Item 03
          [ ] Item 04
          [ ] Item 05
        > [x] Item 06
          [ ] Item 07
          [ ] Item 08
          [ ] Item 09
          [ ] Item 10
        ");
    }

    #[test]
    fn test_snapshot_options_with_descriptions() {
        let options = vec![
            ListItem::new(Line::from(vec![
                Span::raw("[ ] "),
                Span::raw("Auth"),
                Span::styled(
                    "  Authentication module",
                    Style::default().fg(Color::DarkGray),
                ),
            ])),
            ListItem::new(Line::from(vec![
                Span::raw("[x] "),
                Span::raw("API"),
                Span::styled(
                    "  RESTful API endpoints",
                    Style::default().fg(Color::DarkGray),
                ),
            ])),
            ListItem::new(Line::from(vec![
                Span::raw("[ ] "),
                Span::raw("Logging"),
                Span::styled(
                    "  Structured logging system",
                    Style::default().fg(Color::DarkGray),
                ),
            ])),
            ListItem::new(Line::from(vec![
                Span::raw("[x] "),
                Span::raw("Caching"),
                Span::styled("  Redis cache layer", Style::default().fg(Color::DarkGray)),
            ])),
        ];

        let buffer = render_list_to_buffer(options, 50, 4, Some(1));
        insta::assert_snapshot!(snap(&buffer), @"
          [ ] Auth  Authentication module
        > [x] API  RESTful API endpoints
          [ ] Logging  Structured logging system
          [x] Caching  Redis cache layer
        ");
    }

    // Task 74.3: Symulacja togglowania wszystkich opcji on, potem off
    // Używa toggle logic identycznej z produkcyjną (linia 147-148) oraz
    // filtrowania labels (linia 200-205) zamiast sztucznych wektorów.
    #[test]
    fn test_toggle_all_options_on_then_off() {
        let options = [
            QuestionOption {
                label: "Auth".into(),
                description: None,
            },
            QuestionOption {
                label: "API".into(),
                description: None,
            },
            QuestionOption {
                label: "Logging".into(),
                description: None,
            },
            QuestionOption {
                label: "Metrics".into(),
                description: None,
            },
            QuestionOption {
                label: "Caching".into(),
                description: None,
            },
        ];
        let other_index = options.len(); // "Other" byłby na pozycji 5
        let mut toggled = HashSet::<usize>::new();
        let mut cursor = 0usize;

        // Faza 1: Space→Down→Space→Down→... 5x — zaznacz wszystkie
        for _ in 0..options.len() {
            // Space: toggle at cursor
            if toggled.contains(&cursor) {
                toggled.remove(&cursor);
            } else {
                toggled.insert(cursor);
            }
            // Down: move cursor
            cursor = (cursor + 1) % (options.len() + 1); // +1 bo "Other"
        }

        assert_eq!(toggled.len(), 5, "Faza 1: wszystkie 5 opcji zaznaczonych");
        for i in 0..options.len() {
            assert!(toggled.contains(&i), "Faza 1: opcja {i} zaznaczona");
        }

        // Weryfikacja output po fazie 1 — produkcyjna logika filtrowania
        let labels_phase1: Vec<String> = options
            .iter()
            .enumerate()
            .filter(|(idx, _)| toggled.contains(idx) && *idx != other_index)
            .map(|(_, opt)| opt.label.clone())
            .collect();
        assert_eq!(
            labels_phase1.join(", "),
            "Auth, API, Logging, Metrics, Caching"
        );

        // Faza 2: Reset kursora i odznacz wszystkie Space→Down→... 5x
        cursor = 0;
        for _ in 0..options.len() {
            if toggled.contains(&cursor) {
                toggled.remove(&cursor);
            } else {
                toggled.insert(cursor);
            }
            cursor = (cursor + 1) % (options.len() + 1);
        }

        assert!(
            toggled.is_empty(),
            "Faza 2: checked set pusty po odznaczeniu"
        );

        // Weryfikacja output format — pusty string (produkcyjna logika)
        let labels_phase2: Vec<String> = options
            .iter()
            .enumerate()
            .filter(|(idx, _)| toggled.contains(idx) && *idx != other_index)
            .map(|(_, opt)| opt.label.clone())
            .collect();
        assert_eq!(labels_phase2.join(", "), "");
    }

    // Snapshot test: toggle all on → all off, weryfikacja renderowania w 3 fazach
    #[test]
    fn test_snapshot_toggle_all_on_off() {
        let labels = ["Option A", "Option B", "Option C", "Option D", "Option E"];
        let mut toggled = HashSet::<usize>::new();

        // Faza 1: zaznacz wszystkie
        for i in 0..labels.len() {
            toggled.insert(i);
        }
        let items_all_on = build_checkbox_items(&labels, &toggled);
        let buf_all_on = render_list_to_buffer(items_all_on, 30, 5, Some(4));
        insta::assert_snapshot!(snap(&buf_all_on), @"
          [x] Option A
          [x] Option B
          [x] Option C
          [x] Option D
        > [x] Option E
        ");

        // Faza 2: odznacz wszystkie
        for i in 0..labels.len() {
            toggled.remove(&i);
        }
        assert!(toggled.is_empty());

        let items_all_off = build_checkbox_items(&labels, &toggled);
        let buf_all_off = render_list_to_buffer(items_all_off, 30, 5, Some(0));
        insta::assert_snapshot!(snap(&buf_all_off), @"
        > [ ] Option A
          [ ] Option B
          [ ] Option C
          [ ] Option D
          [ ] Option E
        ");
    }
}
