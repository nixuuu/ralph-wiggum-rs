// Widget dla pytań tekstowych (QuestionType::Text)
//
// Używa ratatui Paragraph z Viewport::Inline. Tekst pytania (header)
// jest renderowany w górnej części viewportu, a pole input na dole.

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

/// Renderuje widget tekstowy z headerem pytania osadzonym w viewporcie
#[allow(dead_code)]
pub fn render(question: &Question, header: &str) -> Result<String> {
    let placeholder = question.placeholder.as_deref();
    let default = question.default.as_deref();
    text_input(placeholder, default, question.required, Some(header))
}

/// Minimalny custom readline z ratatui Viewport::Inline
///
/// Gdy `header` jest Some, tekst pytania jest renderowany w górnej części
/// viewportu. Gdy None (np. wywołanie z multi_select "Other"), viewport
/// zawiera tylko pole input.
pub fn text_input(
    placeholder: Option<&str>,
    default: Option<&str>,
    required: bool,
    header: Option<&str>,
) -> Result<String> {
    let header_text = header
        .filter(|h| !h.is_empty())
        .and_then(|h| h.into_text().ok());
    let header_lines = header_text
        .as_ref()
        .map(|t| t.lines.len() as u16)
        .unwrap_or(0);
    let height = 1 + header_lines;

    let mut buffer = default.unwrap_or("").to_string();
    let mut viewport_y = 0u16;

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

                let input_area = if header_lines > 0 {
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

                let line = if buffer.is_empty() {
                    Line::from(vec![
                        Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)),
                        Span::styled(
                            placeholder.unwrap_or(""),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(&buffer),
                    ])
                };
                frame.render_widget(Paragraph::new(line), input_area);

                // Pozycja kursora: "> " (2 chars) + buffer length
                let cursor_x = 2 + buffer.len() as u16;
                frame.set_cursor_position((cursor_x, input_area.y));
            })
            .map_err(|e| RalphError::Mcp(format!("Failed to draw: {e}")))?;

        if event::poll(Duration::from_millis(50))
            .map_err(|e| RalphError::Mcp(format!("Failed to poll events: {e}")))?
            && let Event::Key(key) =
                event::read().map_err(|e| RalphError::Mcp(format!("Failed to read event: {e}")))?
        {
            match handle_key_event(key, &mut buffer, required) {
                Err(e) => {
                    drop(terminal);
                    super::collapse_viewport(viewport_y)?;
                    return Err(e);
                }
                Ok(KeyAction::Continue) => {}
                Ok(KeyAction::Submit) => {
                    drop(terminal);
                    super::collapse_viewport(viewport_y)?;
                    return Ok(buffer);
                }
            }
        }
    }
}

/// Wynik obsługi klawisza
#[derive(Debug)]
enum KeyAction {
    Continue,
    Submit,
}

/// Obsługuje pojedyncze zdarzenie klawiatury
fn handle_key_event(key: KeyEvent, buffer: &mut String, required: bool) -> Result<KeyAction> {
    match (key.code, key.modifiers) {
        // Ctrl+C: anuluj sesję
        (KeyCode::Char('c'), mods) if mods.contains(KeyModifiers::CONTROL) => {
            Err(RalphError::Interrupted)
        }

        // Enter: zwróć buffer (jeśli niepusty lub !required)
        (KeyCode::Enter, mods) if !mods.contains(KeyModifiers::SHIFT) => {
            if !required || !buffer.trim().is_empty() {
                Ok(KeyAction::Submit)
            } else {
                Ok(KeyAction::Continue)
            }
        }

        // Shift+Enter: dodaj znak nowej linii
        (KeyCode::Enter, mods) if mods.contains(KeyModifiers::SHIFT) => {
            buffer.push('\n');
            Ok(KeyAction::Continue)
        }

        // Backspace: usuń ostatni znak
        (KeyCode::Backspace, _) => {
            buffer.pop();
            Ok(KeyAction::Continue)
        }

        // Char: dodaj do buffera
        (KeyCode::Char(c), _) => {
            buffer.push(c);
            Ok(KeyAction::Continue)
        }

        _ => Ok(KeyAction::Continue),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::mcp::ask_user::QuestionType;

    #[test]
    fn test_handle_key_event_enter_returns_submit_when_buffer_not_empty() {
        let mut buffer = "test".to_string();
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut buffer, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Submit));
    }

    #[test]
    fn test_handle_key_event_enter_ignored_when_buffer_empty_and_required() {
        let mut buffer = String::new();
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut buffer, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
    }

    #[test]
    fn test_handle_key_event_enter_allowed_when_not_required() {
        let mut buffer = String::new();
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut buffer, false);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Submit));
    }

    #[test]
    fn test_handle_key_event_shift_enter_adds_newline() {
        let mut buffer = "line1".to_string();
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);

        let result = handle_key_event(key, &mut buffer, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(buffer, "line1\n");
    }

    #[test]
    fn test_handle_key_event_backspace_removes_last_char() {
        let mut buffer = "hello".to_string();
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut buffer, false);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
        assert_eq!(buffer, "hell");
    }

    #[test]
    fn test_handle_key_event_backspace_on_empty_buffer() {
        let mut buffer = String::new();
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut buffer, false);
        assert!(result.is_ok());
        assert_eq!(buffer, "");
    }

    #[test]
    fn test_handle_key_event_ctrl_c_returns_error() {
        let mut buffer = "test".to_string();
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut buffer, false);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RalphError::Interrupted));
    }

    #[test]
    fn test_handle_key_event_char_appends_to_buffer() {
        let mut buffer = "hel".to_string();
        let key = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);

        let result = handle_key_event(key, &mut buffer, false);
        assert!(result.is_ok());
        assert_eq!(buffer, "hell");
    }

    #[test]
    fn test_handle_key_event_whitespace_chars() {
        let mut buffer = "a".to_string();
        let space_key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        let tab_key = KeyEvent::new(KeyCode::Char('\t'), KeyModifiers::NONE);

        let _ = handle_key_event(space_key, &mut buffer, false);
        assert_eq!(buffer, "a ");

        let _ = handle_key_event(tab_key, &mut buffer, false);
        assert_eq!(buffer, "a \t");
    }

    #[test]
    fn test_render_with_default_value() {
        let question = Question {
            question: "Test?".into(),
            question_type: QuestionType::Text,
            options: vec![],
            default: Some("default value".into()),
            placeholder: None,
            required: false,
        };

        assert!(question.default.is_some());
    }

    #[test]
    fn test_render_with_placeholder() {
        let question = Question {
            question: "Test?".into(),
            question_type: QuestionType::Text,
            options: vec![],
            default: None,
            placeholder: Some("Enter your name...".into()),
            required: true,
        };

        assert!(question.placeholder.is_some());
    }

    #[test]
    fn test_handle_key_event_enter_rejects_whitespace_only_when_required() {
        let mut buffer = "   \t  ".to_string();
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        let result = handle_key_event(key, &mut buffer, true);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), KeyAction::Continue));
    }

    #[test]
    fn test_multiline_input_multiple_newlines() {
        let mut buffer = String::new();
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);

        buffer.push_str("line1");
        let _ = handle_key_event(shift_enter, &mut buffer, false);
        buffer.push_str("line2");
        let _ = handle_key_event(shift_enter, &mut buffer, false);
        buffer.push_str("line3");

        assert_eq!(buffer, "line1\nline2\nline3");
    }

    #[test]
    fn test_ctrl_c_preserves_buffer() {
        let original = "important data";
        let mut buffer = String::from(original);
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        let _ = handle_key_event(key, &mut buffer, true);

        assert_eq!(buffer, original);
    }

    #[test]
    fn test_ctrl_c_during_multiline_input() {
        let mut buffer = String::from("line1\nline2\nline3");
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        let result = handle_key_event(key, &mut buffer, false);

        assert!(matches!(result, Err(RalphError::Interrupted)));
        assert_eq!(buffer, "line1\nline2\nline3");
    }

    #[test]
    fn test_raw_mode_guard_contract() {
        {
            let _guard = RawModeGuard;
        }
        // Guard executed Drop — disable_raw_mode called
    }
}
