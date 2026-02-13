// TUI widgets dla narzędzia ask_user
//
// Ten moduł implementuje interaktywne widgety terminala dla różnych
// typów pytań (text, choice, multichoice, confirm). Tekst pytania jest
// renderowany WEWNĄTRZ viewportu widgetu, dzięki czemu terminal.clear()
// czyści zarówno pytanie jak i widget — brak orphaned lines.

use crate::commands::mcp::ask_user::{Question, QuestionType};
use crate::shared::error::{RalphError, Result};
use crate::shared::markdown;
use crossterm::cursor::MoveTo;
use crossterm::terminal::{Clear, ClearType};
use std::io;

mod choice_select;
mod confirm_select;
mod multi_select;
mod text_input;

/// Kolapsuje viewport: przesuwa kursor na absolutną pozycję Y viewportu
/// i czyści wszystko poniżej. Wywoływać po drop(terminal).
///
/// `viewport_y` to wartość `frame.area().y` przechwycona podczas ostatniego draw().
pub(super) fn collapse_viewport(viewport_y: u16) -> Result<()> {
    crossterm::execute!(
        io::stdout(),
        MoveTo(0, viewport_y),
        Clear(ClearType::FromCursorDown)
    )
    .map_err(|e| RalphError::Mcp(format!("Failed to collapse viewport: {e}")))?;
    Ok(())
}

/// Renderuje pytanie i wywołuje odpowiedni widget do zebrania odpowiedzi.
///
/// Tekst pytania jest osadzony w viewporcie widgetu (nie println!), więc
/// po zakończeniu interakcji wszystko jest czyszczone razem — brak pustych
/// linii ani zduplikowanych pytań.
#[allow(dead_code)] // Wywoływane przez ask_user handler
pub fn render_question(question: &Question) -> Result<String> {
    let rendered_header = markdown::render_markdown(&question.question);

    match question.question_type {
        QuestionType::Text => text_input::render(question, &rendered_header),
        QuestionType::Choice => choice_select::render(question, &rendered_header),
        QuestionType::MultiChoice => multi_select::render(question, &rendered_header),
        QuestionType::Confirm => confirm_select::render(question, &rendered_header),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::mcp::ask_user::QuestionOption;

    #[test]
    fn test_render_question_text_type() {
        let question = Question {
            question: "**What** is your name?".into(),
            question_type: QuestionType::Text,
            options: vec![],
            default: None,
            placeholder: Some("John Doe".into()),
            required: true,
        };

        // Ten test obecnie zawiedzie bo text_input::render jest stubem
        // Będzie działał po implementacji widgetów w przyszłych taskach
        let result = render_question(&question);
        assert!(result.is_err()); // Stub zwraca błąd
    }

    #[test]
    fn test_render_question_choice_type() {
        let question = Question {
            question: "Choose **authentication** method".into(),
            question_type: QuestionType::Choice,
            options: vec![
                QuestionOption {
                    label: "JWT".into(),
                    description: Some("Token-based".into()),
                },
                QuestionOption {
                    label: "Session".into(),
                    description: Some("Cookie-based".into()),
                },
            ],
            default: Some("JWT".into()),
            placeholder: None,
            required: true,
        };

        // Stub zwraca błąd
        let result = render_question(&question);
        assert!(result.is_err());
    }

    #[test]
    fn test_render_question_multichoice_type() {
        let question = Question {
            question: "Select features to enable".into(),
            question_type: QuestionType::MultiChoice,
            options: vec![
                QuestionOption {
                    label: "Auth".into(),
                    description: None,
                },
                QuestionOption {
                    label: "API".into(),
                    description: None,
                },
            ],
            default: None,
            placeholder: None,
            required: false,
        };

        // Stub zwraca błąd
        let result = render_question(&question);
        assert!(result.is_err());
    }

    #[test]
    fn test_render_question_confirm_type() {
        let question = Question {
            question: "Are you **sure**?".into(),
            question_type: QuestionType::Confirm,
            options: vec![],
            default: Some("yes".into()),
            placeholder: None,
            required: true,
        };

        // Stub zwraca błąd
        let result = render_question(&question);
        assert!(result.is_err());
    }

    #[test]
    fn test_render_question_with_empty_text() {
        // Edge case: puste pytanie
        let question = Question {
            question: "".into(),
            question_type: QuestionType::Text,
            options: vec![],
            default: None,
            placeholder: None,
            required: false,
        };

        // Stub zwraca błąd, ale funkcja powinna obsłużyć puste pytanie
        let result = render_question(&question);
        assert!(result.is_err());
    }

    #[test]
    fn test_render_question_with_complex_markdown() {
        // Edge case: złożone formatowanie markdown
        let question = Question {
            question: r#"# Header
**Bold** and *italic* text with `inline code`

```rust
fn test() {}
```

- Bullet 1
- Bullet 2"#
                .into(),
            question_type: QuestionType::Text,
            options: vec![],
            default: None,
            placeholder: None,
            required: true,
        };

        // Stub zwraca błąd, ale markdown powinien się wyrenderować
        let result = render_question(&question);
        assert!(result.is_err());
    }
}
