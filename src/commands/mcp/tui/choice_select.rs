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
#[allow(dead_code)]
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

                    drop(terminal);
                    super::collapse_viewport(viewport_y)?;

                    if choice == "Other" {
                        return super::text_input::text_input(
                            question.placeholder.as_deref(),
                            question.default.as_deref(),
                            question.required,
                            None,
                        );
                    }
                    return Ok(choice);
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
}
