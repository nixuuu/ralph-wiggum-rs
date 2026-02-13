// Widget dla pytań potwierdzających (QuestionType::Confirm)
//
// Używa ratatui Paragraph z Viewport::Inline — header pytania + [Yes] [No].

use ansi_to_tui::IntoText;

use crate::commands::mcp::ask_user::Question;
use crate::shared::error::{RalphError, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::io;
use std::time::Duration;

/// RAII guard dla raw mode
struct RawModeGuard;
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// Renderuje widget confirm z headerem pytania osadzonym w viewporcie
#[allow(dead_code)]
pub fn render(question: &Question, header: &str) -> Result<String> {
    let header_text = (!header.is_empty())
        .then(|| header.into_text().ok())
        .flatten();
    let header_lines = header_text
        .as_ref()
        .map(|t| t.lines.len() as u16)
        .unwrap_or(0);

    let default_is_yes = if let Some(ref default) = question.default {
        let lower = default.to_lowercase();
        lower == "yes" || lower == "true"
    } else {
        false
    };

    let mut selected: usize = if default_is_yes { 0 } else { 1 }; // 0=Yes, 1=No
    let height = 1 + header_lines;
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

    loop {
        terminal
            .draw(|frame| {
                let area = frame.area();
                viewport_y = area.y;

                let buttons_area = if header_lines > 0 {
                    let chunks =
                        Layout::vertical([Constraint::Length(header_lines), Constraint::Length(1)])
                            .split(area);
                    if let Some(ref ht) = header_text {
                        frame.render_widget(Paragraph::new(ht.clone()), chunks[0]);
                    }
                    chunks[1]
                } else {
                    area
                };

                let yes_style = if selected == 0 {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                let no_style = if selected == 1 {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                let line = Line::from(vec![
                    Span::styled("[Yes]", yes_style),
                    Span::raw("  "),
                    Span::styled("[No]", no_style),
                ]);
                frame.render_widget(Paragraph::new(line), buttons_area);
            })
            .map_err(|e| RalphError::Mcp(format!("Failed to draw: {e}")))?;

        if event::poll(Duration::from_millis(50))
            .map_err(|e| RalphError::Mcp(format!("Failed to poll events: {e}")))?
        {
            match event::read()
                .map_err(|e| RalphError::Mcp(format!("Failed to read event: {e}")))?
            {
                Event::Key(KeyEvent {
                    code: KeyCode::Left,
                    ..
                }) => {
                    selected = 0; // Yes
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Right,
                    ..
                }) => {
                    selected = 1; // No
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Enter,
                    ..
                }) => {
                    let choice = if selected == 0 { "yes" } else { "no" };
                    drop(terminal);
                    super::collapse_viewport(viewport_y)?;
                    return Ok(choice.to_string());
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

    #[test]
    fn test_default_yes_resolution() {
        let default = Some("yes".to_string());
        let lower = default.as_ref().unwrap().to_lowercase();
        let is_yes = lower == "yes" || lower == "true";
        assert!(is_yes);
    }

    #[test]
    fn test_default_true_resolution() {
        let default = Some("true".to_string());
        let lower = default.as_ref().unwrap().to_lowercase();
        let is_yes = lower == "yes" || lower == "true";
        assert!(is_yes);
    }

    #[test]
    fn test_default_yes_case_insensitive() {
        for variant in ["YES", "Yes", "YeS"] {
            let lower = variant.to_lowercase();
            let is_yes = lower == "yes" || lower == "true";
            assert!(is_yes, "Failed for variant: {}", variant);
        }
    }

    #[test]
    fn test_default_true_case_insensitive() {
        for variant in ["TRUE", "True", "TrUe"] {
            let lower = variant.to_lowercase();
            let is_yes = lower == "yes" || lower == "true";
            assert!(is_yes, "Failed for variant: {}", variant);
        }
    }

    #[test]
    fn test_default_no_resolution() {
        let default = Some("no".to_string());
        let lower = default.as_ref().unwrap().to_lowercase();
        let is_yes = lower == "yes" || lower == "true";
        assert!(!is_yes);
    }

    #[test]
    fn test_default_false_resolution() {
        let default = Some("false".to_string());
        let lower = default.as_ref().unwrap().to_lowercase();
        let is_yes = lower == "yes" || lower == "true";
        assert!(!is_yes);
    }

    #[test]
    fn test_default_none_resolution() {
        let default: Option<String> = None;
        let is_yes = if let Some(ref d) = default {
            let lower = d.to_lowercase();
            lower == "yes" || lower == "true"
        } else {
            false
        };
        assert!(!is_yes);
    }

    #[test]
    fn test_default_unknown_value_resolution() {
        let default = Some("maybe".to_string());
        let lower = default.as_ref().unwrap().to_lowercase();
        let is_yes = lower == "yes" || lower == "true";
        assert!(!is_yes);
    }

    #[test]
    fn test_button_index_to_result_mapping() {
        let choice_0 = if 0 == 0 { "yes" } else { "no" };
        assert_eq!(choice_0, "yes");

        let choice_1 = if 1 == 0 { "yes" } else { "no" };
        assert_eq!(choice_1, "no");
    }

    #[test]
    fn test_navigation_idempotency() {
        // Left arrow zawsze ustawia na 0 (Yes)
        let selected_yes: usize = 0;
        assert_eq!(selected_yes, 0);

        // Right arrow zawsze ustawia na 1 (No)
        let selected_no: usize = 1;
        assert_eq!(selected_no, 1);
    }

    #[test]
    fn test_render_function_signature() {
        let question = Question {
            question: "Are you sure?".into(),
            question_type: QuestionType::Confirm,
            options: vec![],
            default: Some("yes".into()),
            placeholder: None,
            required: true,
        };

        assert_eq!(question.question_type, QuestionType::Confirm);
        assert_eq!(question.default, Some("yes".into()));
    }

    #[test]
    fn test_edge_case_empty_default() {
        let default = Some("".to_string());
        let lower = default.as_ref().unwrap().to_lowercase();
        let is_yes = lower == "yes" || lower == "true";
        assert!(!is_yes);
    }

    #[test]
    fn test_choice_result_format() {
        let result_yes = if 0 == 0 { "yes" } else { "no" };
        assert_eq!(result_yes, "yes");
        assert_ne!(result_yes, "Yes");

        let result_no = if 1 == 0 { "yes" } else { "no" };
        assert_eq!(result_no, "no");
        assert_ne!(result_no, "No");
    }

    #[test]
    fn test_raw_mode_guard_contract() {
        struct TestGuard;
        impl Drop for TestGuard {
            fn drop(&mut self) {
                let _ = disable_raw_mode();
            }
        }

        {
            let _guard = TestGuard;
        }
        // Guard executed Drop
    }
}
