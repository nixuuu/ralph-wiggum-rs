// Typy danych i handler HTTP dla narzędzia ask_user (MCP tool)
//
// Ten moduł definiuje:
// - Typy używane do parsowania parametrów narzędzia ask_user z MCP protocol
// - Handler SSE (Server-Sent Events) z heartbeat dla interakcji z TUI
//
// Flow: HTTP request → parse_questions → QuestionEnvelope → mpsc → TUI → oneshot → SSE response

use std::convert::Infallible;
use std::time::Duration;

use axum::response::sse::{Event, Sse};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;

use super::protocol::{JsonRpcResponse, McpContent, McpToolCallResult};
use super::state::{AppState, QuestionEnvelope};
use crate::shared::error::{RalphError, Result};

// ── QuestionType Enum ──────────────────────────────────────────────

/// Typ pytania w narzędziu ask_user
///
/// Określa rodzaj interakcji z użytkownikiem:
/// - `Text`: Pytanie otwarte z polem tekstowym
/// - `Choice`: Wybór jednej opcji z listy
/// - `MultiChoice`: Wybór wielu opcji z listy
/// - `Confirm`: Pytanie tak/nie
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum QuestionType {
    /// Pytanie otwarte z polem tekstowym
    Text,
    /// Wybór jednej opcji z listy
    Choice,
    /// Wybór wielu opcji z listy
    #[serde(rename = "multichoice")]
    MultiChoice,
    /// Pytanie tak/nie (confirm)
    Confirm,
}

// ── QuestionOption Struct ──────────────────────────────────────────

/// Opcja wyboru w pytaniu typu Choice/MultiChoice
///
/// Każda opcja ma wymaganą etykietę (label) i opcjonalny opis
/// wyjaśniający kontekst wyboru.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuestionOption {
    /// Tekst wyświetlany jako opcja wyboru
    pub label: String,
    /// Opcjonalny opis opcji (np. trade-offy, konsekwencje)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ── Question Struct ────────────────────────────────────────────────

/// Pytanie do użytkownika (parametry narzędzia ask_user)
///
/// Struktura reprezentująca pojedyncze pytanie w narzędziu MCP ask_user.
/// Zawiera treść pytania, typ, opcje (dla Choice/MultiChoice), wartość
/// domyślną, placeholder oraz flagę wymaganej odpowiedzi.
///
/// # Examples
///
/// ```no_run
/// use ralph_wiggum::commands::mcp::ask_user::{Question, QuestionType, QuestionOption};
///
/// // Pytanie tekstowe
/// let text_question = Question {
///     question: "What is your name?".into(),
///     question_type: QuestionType::Text,
///     options: vec![],
///     default: None,
///     placeholder: Some("John Doe".into()),
///     required: true,
/// };
///
/// // Pytanie wyboru
/// let choice_question = Question {
///     question: "Choose authentication method".into(),
///     question_type: QuestionType::Choice,
///     options: vec![
///         QuestionOption { label: "JWT".into(), description: Some("Token-based".into()) },
///         QuestionOption { label: "Session".into(), description: Some("Cookie-based".into()) },
///     ],
///     default: Some("JWT".into()),
///     placeholder: None,
///     required: true,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Question {
    /// Treść pytania wyświetlana użytkownikowi
    pub question: String,

    /// Typ pytania (text/choice/multichoice/confirm)
    #[serde(rename = "type")]
    pub question_type: QuestionType,

    /// Lista opcji (wymagana dla Choice/MultiChoice, pusta dla Text/Confirm)
    #[serde(default)]
    pub options: Vec<QuestionOption>,

    /// Wartość domyślna (opcjonalna)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    /// Tekst placeholder w polu tekstowym (opcjonalny)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,

    /// Czy odpowiedź jest wymagana
    #[serde(default = "default_required")]
    pub required: bool,
}

/// Default dla pola `required` - domyślnie true
fn default_required() -> bool {
    true
}

// ── Answer Struct ──────────────────────────────────────────────────

/// Odpowiedź użytkownika na pytanie
///
/// Zawiera treść pytania (identyfikator) oraz odpowiedź użytkownika
/// jako string. W przypadku MultiChoice odpowiedzi są połączone
/// przecinkami lub jako JSON array (zależy od implementacji parsera).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Answer {
    /// Treść pytania (używana jako identyfikator)
    pub question: String,
    /// Odpowiedź użytkownika (string lub JSON array dla multichoice)
    pub answer: String,
}

// ── Helper Functions ───────────────────────────────────────────────

/// Parsuje listę pytań z parametrów MCP tool call
///
/// Funkcja parsuje pole "questions" z parametrów narzędzia ask_user.
/// Oczekuje tablicy obiektów JSON reprezentujących pytania.
///
/// # Arguments
///
/// * `params` - Parametry narzędzia (JSON Value)
///
/// # Returns
///
/// * `Ok(Vec<Question>)` - Lista sparsowanych pytań
/// * `Err(RalphError)` - Błąd parsowania lub brak pola "questions"
///
/// # Errors
///
/// Zwraca błąd jeśli:
/// - Brak pola "questions" w params
/// - Pole "questions" nie jest tablicą
/// - Deserializacja pytań nie powiodła się
/// - Lista pytań jest pusta
/// - Pytanie zawiera pusty string jako treść
/// - Choice/MultiChoice nie ma opcji
/// - Opcje zawierają puste labele
/// - Opcje mają duplikaty labeli
///
/// # Examples
///
/// ```no_run
/// use serde_json::json;
/// use ralph_wiggum::commands::mcp::ask_user::parse_questions;
///
/// let params = json!({
///     "questions": [
///         {
///             "question": "What is your name?",
///             "type": "text",
///             "required": true
///         }
///     ]
/// });
///
/// let questions = parse_questions(&params).expect("Valid questions");
/// assert_eq!(questions.len(), 1);
/// ```
pub fn parse_questions(params: &Value) -> Result<Vec<Question>> {
    // Compat: simple format {question: string, options?: string[]}
    // → convert to [{question, type: "text"|"choice", options: [{label}]}]
    if let Some(question_str) = params.get("question").and_then(|v| v.as_str()) {
        if question_str.trim().is_empty() {
            return Err(RalphError::Mcp(
                "Question 0: 'question' field cannot be empty".into(),
            ));
        }

        let options: Vec<QuestionOption> = params
            .get("options")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| QuestionOption {
                        label: s.to_string(),
                        description: None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let multi_select = params
            .get("multiSelect")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let question_type = if options.is_empty() {
            QuestionType::Text
        } else if multi_select {
            QuestionType::MultiChoice
        } else {
            QuestionType::Choice
        };

        return Ok(vec![Question {
            question: question_str.to_string(),
            question_type,
            options,
            default: None,
            placeholder: None,
            required: true,
        }]);
    }

    // Full format: {questions: [{...}]}
    let questions_value = params.get("questions").ok_or_else(|| {
        RalphError::Mcp("Missing 'question' or 'questions' field in params".into())
    })?;

    // Zdeserializuj tablicę pytań
    let questions: Vec<Question> = serde_json::from_value(questions_value.clone())
        .map_err(|e| RalphError::Mcp(format!("Failed to parse questions: {e}")))?;

    // Walidacja: co najmniej jedno pytanie
    if questions.is_empty() {
        return Err(RalphError::Mcp("At least one question is required".into()));
    }

    // Walidacja dla każdego pytania
    for (idx, q) in questions.iter().enumerate() {
        // Walidacja: question nie może być pusty
        if q.question.trim().is_empty() {
            return Err(RalphError::Mcp(format!(
                "Question {idx}: 'question' field cannot be empty"
            )));
        }

        // Walidacja: Choice/MultiChoice muszą mieć opcje
        match q.question_type {
            QuestionType::Choice | QuestionType::MultiChoice => {
                if q.options.is_empty() {
                    return Err(RalphError::Mcp(format!(
                        "Question {idx}: Choice/MultiChoice requires non-empty options"
                    )));
                }

                // Walidacja: labels nie mogą być puste
                for (opt_idx, opt) in q.options.iter().enumerate() {
                    if opt.label.trim().is_empty() {
                        return Err(RalphError::Mcp(format!(
                            "Question {idx}, option {opt_idx}: label cannot be empty"
                        )));
                    }
                }

                // Walidacja: labels muszą być unikalne
                let mut seen_labels = std::collections::HashSet::new();
                for opt in &q.options {
                    let label = opt.label.trim();
                    if !seen_labels.insert(label) {
                        return Err(RalphError::Mcp(format!(
                            "Question {idx}: duplicate option label '{label}'"
                        )));
                    }
                }
            }
            QuestionType::Text | QuestionType::Confirm => {
                // Text i Confirm nie wymagają opcji
            }
        }
    }

    Ok(questions)
}

/// Formatuje odpowiedzi użytkownika do postaci markdown
///
/// Funkcja tworzy formatowany string markdown z listy par pytanie-odpowiedź.
/// Każde pytanie jest wyświetlane jako bold header, a odpowiedź jako plain text.
///
/// # Arguments
///
/// * `answers` - Slice odpowiedzi użytkownika (Question, Answer pairs)
///
/// # Returns
///
/// String w formacie markdown z pytaniami i odpowiedziami
///
/// # Examples
///
/// ```no_run
/// use ralph_wiggum::commands::mcp::ask_user::format_answers;
///
/// let answers = vec![
///     ("What is your name?", "Alice"),
///     ("Choose auth method", "JWT"),
/// ];
///
/// let markdown = format_answers(&answers);
/// // Wynik:
/// // **What is your name?**
/// // Alice
/// //
/// // **Choose auth method**
/// // JWT
/// ```
pub fn format_answers(answers: &[(&str, &str)]) -> String {
    if answers.is_empty() {
        return String::new();
    }

    answers
        .iter()
        .enumerate()
        .map(|(idx, (question, answer))| {
            let pair = format!("**{}**\n{}", question, answer);

            // Add blank line separator between pairs (not after last)
            if idx < answers.len() - 1 {
                // When answer is empty, pair ends with \n already.
                // Add only \n to maintain exactly 2 newlines between pairs.
                // When answer is not empty, pair ends with text.
                // Add \n\n to create blank line separator (2 newlines total).
                if answer.is_empty() {
                    format!("{}\n", pair)
                } else {
                    format!("{}\n\n", pair)
                }
            } else {
                pair
            }
        })
        .collect()
}

// ── SSE Heartbeat Config ─────────────────────────────────────────

/// Interwał heartbeat SSE (15 sekund)
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

// ── SSE Handler ─────────────────────────────────────────────────

/// Typ streamu SSE — używany do unifikacji return type
/// (różne `impl Stream` to różne opaque types, Box<dyn> je ujednolica)
type SseStream = std::pin::Pin<
    Box<dyn futures_core::Stream<Item = std::result::Result<Event, Infallible>> + Send>,
>;

/// Handler ask_user: bridges MCP request → TUI → SSE response
///
/// Flow:
/// 1. Parsuje pytania z params via `parse_questions()`
/// 2. Tworzy `oneshot::channel()` do odbioru odpowiedzi z TUI
/// 3. Buduje `QuestionEnvelope` i wysyła przez `state.question_tx`
/// 4. Zwraca SSE stream z heartbeat co 15s
/// 5. Po odpowiedzi formatuje markdown i wysyła JSON-RPC result
///
/// # SSE Stream Format
///
/// - Heartbeat: `: ping\n\n` (SSE comment — nie jest eventem)
/// - Response: `event: message\ndata: {json-rpc-response}\n\n`
///
/// # Error Handling
///
/// - Błąd parsowania pytań → SSE event z JSON-RPC error
/// - Kanał question_tx zamknięty → SSE event z internal error
/// - Oneshot response dropped → SSE event z internal error
pub async fn handle_ask_user(params: Value, state: AppState, rpc_id: Value) -> Sse<SseStream> {
    // Parsowanie pytań z parametrów MCP
    let questions = match parse_questions(&params) {
        Ok(q) => q,
        Err(e) => {
            return error_sse_response(
                rpc_id,
                super::protocol::INVALID_PARAMS,
                format!("Invalid ask_user params: {e}"),
            );
        }
    };

    // Kanał zwrotny: TUI wyśle odpowiedzi po interakcji z użytkownikiem
    let (response_tx, response_rx) = oneshot::channel::<Vec<Answer>>();

    // Budujemy kopertę i wysyłamy do TUI przez mpsc
    let envelope = QuestionEnvelope {
        questions: questions.clone(),
        response_tx,
    };

    if state.question_tx.send(envelope).await.is_err() {
        return error_sse_response(
            rpc_id,
            super::protocol::INTERNAL_ERROR,
            "TUI channel closed — cannot deliver questions".into(),
        );
    }

    // SSE stream z heartbeat i oczekiwaniem na odpowiedź
    build_sse_stream(rpc_id, questions, response_rx)
}

/// Buduje SSE stream z heartbeat co 15s i oczekiwaniem na odpowiedź z TUI
fn build_sse_stream(
    rpc_id: Value,
    questions: Vec<Question>,
    response_rx: oneshot::Receiver<Vec<Answer>>,
) -> Sse<SseStream> {
    let stream = async_stream::stream! {
        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        // Pierwsze ticknięcie jest natychmiastowe — pomijamy je
        heartbeat.tick().await;

        // Pin oneshot receiver dla użycia w tokio::select!
        tokio::pin!(response_rx);

        loop {
            tokio::select! {
                // Heartbeat: SSE comment `: ping` — utrzymuje połączenie
                _ = heartbeat.tick() => {
                    yield Ok(Event::default().comment("ping"));
                }

                // Odpowiedź z TUI: formatuj markdown i wyślij JSON-RPC result
                result = &mut response_rx => {
                    let event = match result {
                        Ok(answers) => {
                            let markdown = build_answer_markdown(&questions, &answers);
                            let tool_result = McpToolCallResult {
                                content: vec![McpContent::Text { text: markdown }],
                                is_error: None,
                            };
                            let rpc_response = JsonRpcResponse::success(
                                Some(rpc_id),
                                serde_json::to_value(tool_result)
                                    .expect("McpToolCallResult serialization should never fail"),
                            );
                            make_message_event(&rpc_response)
                        }
                        Err(_) => {
                            // Oneshot sender dropped — TUI zamknięte bez odpowiedzi
                            let rpc_response = JsonRpcResponse::error(
                                Some(rpc_id),
                                super::protocol::INTERNAL_ERROR,
                                "TUI closed without responding".into(),
                            );
                            make_message_event(&rpc_response)
                        }
                    };

                    yield Ok(event);
                    break; // Zakończ stream po wysłaniu odpowiedzi
                }
            }
        }
    };

    Sse::new(Box::pin(stream) as SseStream)
}

/// Tworzy SSE Event typu "message" z JSON-RPC response w polu data
fn make_message_event(response: &JsonRpcResponse) -> Event {
    let json =
        serde_json::to_string(response).expect("JsonRpcResponse serialization should never fail");
    Event::default().event("message").data(json)
}

/// Tworzy SSE response z pojedynczym error eventem (dla wczesnych błędów)
fn error_sse_response(rpc_id: Value, code: i32, message: String) -> Sse<SseStream> {
    let stream = async_stream::stream! {
        let rpc_response = JsonRpcResponse::error(Some(rpc_id), code, message);
        yield Ok(make_message_event(&rpc_response));
    };
    Sse::new(Box::pin(stream) as SseStream)
}

/// Buduje markdown z par pytanie-odpowiedź (mapując Answer na Question.question)
///
/// Odpowiedzi mapowane są po indeksie — answer[i] odpowiada question[i].
/// Nadmiarowe odpowiedzi (bez pytań) są ignorowane.
fn build_answer_markdown(questions: &[Question], answers: &[Answer]) -> String {
    let pairs: Vec<(&str, &str)> = questions
        .iter()
        .zip(answers.iter())
        .map(|(q, a)| (q.question.as_str(), a.answer.as_str()))
        .collect();
    format_answers(&pairs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_question_type_serialize_lowercase() {
        let types = vec![
            (QuestionType::Text, "\"text\""),
            (QuestionType::Choice, "\"choice\""),
            (QuestionType::MultiChoice, "\"multichoice\""),
            (QuestionType::Confirm, "\"confirm\""),
        ];

        for (qtype, expected) in types {
            let json = serde_json::to_string(&qtype).unwrap();
            assert_eq!(json, expected);
        }
    }

    #[test]
    fn test_question_type_deserialize_lowercase() {
        let json = json!("text");
        let qtype: QuestionType = serde_json::from_value(json).unwrap();
        assert_eq!(qtype, QuestionType::Text);

        let json = json!("multichoice");
        let qtype: QuestionType = serde_json::from_value(json).unwrap();
        assert_eq!(qtype, QuestionType::MultiChoice);
    }

    #[test]
    fn test_question_option_with_description() {
        let opt = QuestionOption {
            label: "JWT".into(),
            description: Some("Token-based authentication".into()),
        };

        let json = serde_json::to_value(&opt).unwrap();
        assert_eq!(json["label"], "JWT");
        assert_eq!(json["description"], "Token-based authentication");
    }

    #[test]
    fn test_question_option_without_description() {
        let opt = QuestionOption {
            label: "Option".into(),
            description: None,
        };

        let json = serde_json::to_value(&opt).unwrap();
        assert_eq!(json["label"], "Option");
        // description nie powinno być w JSON gdy None
        assert!(json.get("description").is_none());
    }

    #[test]
    fn test_question_text_type() {
        let question = Question {
            question: "What is your name?".into(),
            question_type: QuestionType::Text,
            options: vec![],
            default: None,
            placeholder: Some("John Doe".into()),
            required: true,
        };

        let json = serde_json::to_value(&question).unwrap();
        assert_eq!(json["question"], "What is your name?");
        assert_eq!(json["type"], "text");
        assert_eq!(json["placeholder"], "John Doe");
        assert_eq!(json["required"], true);
    }

    #[test]
    fn test_question_choice_type() {
        let question = Question {
            question: "Choose authentication method".into(),
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

        let json = serde_json::to_value(&question).unwrap();
        assert_eq!(json["type"], "choice");
        assert_eq!(json["options"].as_array().unwrap().len(), 2);
        assert_eq!(json["default"], "JWT");
    }

    #[test]
    fn test_question_multichoice_type() {
        let question = Question {
            question: "Select features".into(),
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
                QuestionOption {
                    label: "Database".into(),
                    description: None,
                },
            ],
            default: None,
            placeholder: None,
            required: false,
        };

        let json = serde_json::to_value(&question).unwrap();
        assert_eq!(json["type"], "multichoice");
        assert_eq!(json["required"], false);
        assert_eq!(json["options"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_question_confirm_type() {
        let question = Question {
            question: "Are you sure?".into(),
            question_type: QuestionType::Confirm,
            options: vec![],
            default: Some("yes".into()),
            placeholder: None,
            required: true,
        };

        let json = serde_json::to_value(&question).unwrap();
        assert_eq!(json["type"], "confirm");
        assert_eq!(json["default"], "yes");
    }

    #[test]
    fn test_question_default_required() {
        let json = json!({
            "question": "Test?",
            "type": "text"
        });

        let question: Question = serde_json::from_value(json).unwrap();
        assert!(question.required); // Powinno być true z defaultu
    }

    #[test]
    fn test_question_roundtrip() {
        let original = Question {
            question: "Test question?".into(),
            question_type: QuestionType::Choice,
            options: vec![QuestionOption {
                label: "Option 1".into(),
                description: Some("Description 1".into()),
            }],
            default: Some("Option 1".into()),
            placeholder: None,
            required: true,
        };

        let json = serde_json::to_value(&original).unwrap();
        let parsed: Question = serde_json::from_value(json).unwrap();

        assert_eq!(parsed, original);
    }

    #[test]
    fn test_answer_serialize() {
        let answer = Answer {
            question: "What is your name?".into(),
            answer: "Alice".into(),
        };

        let json = serde_json::to_value(&answer).unwrap();
        assert_eq!(json["question"], "What is your name?");
        assert_eq!(json["answer"], "Alice");
    }

    #[test]
    fn test_answer_multichoice() {
        let answer = Answer {
            question: "Select features".into(),
            answer: "[\"Auth\", \"API\", \"Database\"]".into(),
        };

        let json = serde_json::to_value(&answer).unwrap();
        assert_eq!(json["answer"], "[\"Auth\", \"API\", \"Database\"]");
    }

    #[test]
    fn test_answer_roundtrip() {
        let original = Answer {
            question: "Test?".into(),
            answer: "Yes".into(),
        };

        let json = serde_json::to_value(&original).unwrap();
        let parsed: Answer = serde_json::from_value(json).unwrap();

        assert_eq!(parsed, original);
    }

    #[test]
    fn test_parse_questions_success() {
        let params = json!({
            "questions": [
                {
                    "question": "What is your name?",
                    "type": "text",
                    "placeholder": "John Doe",
                    "required": true
                },
                {
                    "question": "Choose authentication",
                    "type": "choice",
                    "options": [
                        {"label": "JWT", "description": "Token-based"},
                        {"label": "Session", "description": "Cookie-based"}
                    ],
                    "default": "JWT"
                }
            ]
        });

        let questions = parse_questions(&params).unwrap();
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[0].question, "What is your name?");
        assert_eq!(questions[0].question_type, QuestionType::Text);
        assert_eq!(questions[1].question_type, QuestionType::Choice);
        assert_eq!(questions[1].options.len(), 2);
    }

    #[test]
    fn test_parse_questions_missing_field() {
        let params = json!({
            "other_field": "value"
        });

        let result = parse_questions(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Missing 'question'"));
    }

    #[test]
    fn test_parse_questions_empty_array() {
        let params = json!({
            "questions": []
        });

        let result = parse_questions(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("At least one question is required")
        );
    }

    #[test]
    fn test_parse_questions_choice_without_options() {
        let params = json!({
            "questions": [
                {
                    "question": "Choose one",
                    "type": "choice",
                    "options": []
                }
            ]
        });

        let result = parse_questions(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("Choice/MultiChoice requires non-empty options")
        );
    }

    #[test]
    fn test_parse_questions_multichoice_without_options() {
        let params = json!({
            "questions": [
                {
                    "question": "Select features",
                    "type": "multichoice",
                    "options": []
                }
            ]
        });

        let result = parse_questions(&params);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_questions_text_without_options_valid() {
        let params = json!({
            "questions": [
                {
                    "question": "Enter text",
                    "type": "text"
                }
            ]
        });

        let result = parse_questions(&params);
        assert!(result.is_ok());
        let questions = result.unwrap();
        assert_eq!(questions.len(), 1);
        assert!(questions[0].options.is_empty());
    }

    #[test]
    fn test_parse_questions_confirm_without_options_valid() {
        let params = json!({
            "questions": [
                {
                    "question": "Are you sure?",
                    "type": "confirm"
                }
            ]
        });

        let result = parse_questions(&params);
        assert!(result.is_ok());
        let questions = result.unwrap();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].question_type, QuestionType::Confirm);
    }

    #[test]
    fn test_parse_questions_invalid_json() {
        let params = json!({
            "questions": "not an array"
        });

        let result = parse_questions(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Failed to parse questions"));
    }

    #[test]
    fn test_question_option_equality() {
        let opt1 = QuestionOption {
            label: "Test".into(),
            description: Some("Desc".into()),
        };
        let opt2 = opt1.clone();
        let opt3 = QuestionOption {
            label: "Other".into(),
            description: Some("Desc".into()),
        };

        assert_eq!(opt1, opt2);
        assert_ne!(opt1, opt3);
    }

    #[test]
    fn test_question_equality() {
        let q1 = Question {
            question: "Test?".into(),
            question_type: QuestionType::Text,
            options: vec![],
            default: None,
            placeholder: None,
            required: true,
        };
        let q2 = q1.clone();
        let q3 = Question {
            question: "Other?".into(),
            question_type: QuestionType::Text,
            options: vec![],
            default: None,
            placeholder: None,
            required: true,
        };

        assert_eq!(q1, q2);
        assert_ne!(q1, q3);
    }

    #[test]
    fn test_answer_equality() {
        let a1 = Answer {
            question: "Q1".into(),
            answer: "A1".into(),
        };
        let a2 = a1.clone();
        let a3 = Answer {
            question: "Q2".into(),
            answer: "A1".into(),
        };

        assert_eq!(a1, a2);
        assert_ne!(a1, a3);
    }

    // ── New Validation Tests ──────────────────────────────────────────

    #[test]
    fn test_parse_questions_empty_question_text() {
        let params = json!({
            "questions": [
                {
                    "question": "",
                    "type": "text"
                }
            ]
        });

        let result = parse_questions(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("'question' field cannot be empty"));
    }

    #[test]
    fn test_parse_questions_whitespace_only_question() {
        let params = json!({
            "questions": [
                {
                    "question": "   ",
                    "type": "text"
                }
            ]
        });

        let result = parse_questions(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("'question' field cannot be empty"));
    }

    #[test]
    fn test_parse_questions_empty_option_label() {
        let params = json!({
            "questions": [
                {
                    "question": "Choose one",
                    "type": "choice",
                    "options": [
                        {"label": "Valid"},
                        {"label": ""}
                    ]
                }
            ]
        });

        let result = parse_questions(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("label cannot be empty"));
    }

    #[test]
    fn test_parse_questions_whitespace_only_option_label() {
        let params = json!({
            "questions": [
                {
                    "question": "Choose one",
                    "type": "choice",
                    "options": [
                        {"label": "Valid"},
                        {"label": "   "}
                    ]
                }
            ]
        });

        let result = parse_questions(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("label cannot be empty"));
    }

    #[test]
    fn test_parse_questions_duplicate_option_labels() {
        let params = json!({
            "questions": [
                {
                    "question": "Choose one",
                    "type": "choice",
                    "options": [
                        {"label": "Option A"},
                        {"label": "Option B"},
                        {"label": "Option A"}
                    ]
                }
            ]
        });

        let result = parse_questions(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("duplicate option label"));
        assert!(err.to_string().contains("Option A"));
    }

    #[test]
    fn test_parse_questions_duplicate_labels_case_sensitive() {
        // Duplikaty są case-sensitive (trim tylko whitespace)
        let params = json!({
            "questions": [
                {
                    "question": "Choose one",
                    "type": "choice",
                    "options": [
                        {"label": "Option A"},
                        {"label": "option a"}
                    ]
                }
            ]
        });

        let result = parse_questions(&params);
        // To powinno być OK - różne case
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_questions_multichoice_validation() {
        let params = json!({
            "questions": [
                {
                    "question": "Select features",
                    "type": "multichoice",
                    "options": [
                        {"label": "Auth"},
                        {"label": "Auth"}
                    ]
                }
            ]
        });

        let result = parse_questions(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("duplicate option label"));
    }

    // ── format_answers Tests ──────────────────────────────────────────

    #[test]
    fn test_format_answers_single_qa() {
        let answers = vec![("What is your name?", "Alice")];
        let markdown = super::format_answers(&answers);

        assert_eq!(markdown, "**What is your name?**\nAlice");
    }

    #[test]
    fn test_format_answers_multiple_qa() {
        let answers = vec![
            ("What is your name?", "Alice"),
            ("Choose authentication method", "JWT"),
            ("Select features", "Auth, API, Database"),
        ];

        let markdown = super::format_answers(&answers);

        let expected = "**What is your name?**\nAlice\n\n\
                        **Choose authentication method**\nJWT\n\n\
                        **Select features**\nAuth, API, Database";

        assert_eq!(markdown, expected);
    }

    #[test]
    fn test_format_answers_empty_slice() {
        let answers: Vec<(&str, &str)> = vec![];
        let markdown = super::format_answers(&answers);

        assert_eq!(markdown, "");
    }

    #[test]
    fn test_format_answers_with_special_chars() {
        let answers = vec![
            ("Question with **bold**?", "Answer with *italic*"),
            ("Question with `code`", "Answer with [link](url)"),
        ];

        let markdown = super::format_answers(&answers);

        let expected = "**Question with **bold**?**\nAnswer with *italic*\n\n\
             **Question with `code`**\nAnswer with [link](url)";

        assert_eq!(markdown, expected);
    }

    #[test]
    fn test_format_answers_multiline_answer() {
        let answers = vec![("Describe the issue", "First line\nSecond line\nThird line")];

        let markdown = super::format_answers(&answers);

        assert_eq!(
            markdown,
            "**Describe the issue**\nFirst line\nSecond line\nThird line"
        );
    }

    #[test]
    fn test_format_answers_empty_strings() {
        let answers = vec![("", ""), ("Question", ""), ("", "Answer")];

        let markdown = super::format_answers(&answers);

        let expected = "****\n\n\
                        **Question**\n\n\
                        ****\nAnswer";

        assert_eq!(markdown, expected);
    }

    #[test]
    fn test_format_answers_confirm_type() {
        let answers = vec![("Are you sure?", "yes"), ("Continue?", "no")];

        let markdown = super::format_answers(&answers);

        let expected = "**Are you sure?**\nyes\n\n\
                        **Continue?**\nno";

        assert_eq!(markdown, expected);
    }

    #[test]
    fn test_format_answers_multichoice_json_array() {
        let answers = vec![(
            "Select features",
            "[\"Authentication\", \"API\", \"Database\"]",
        )];

        let markdown = super::format_answers(&answers);

        assert_eq!(
            markdown,
            "**Select features**\n[\"Authentication\", \"API\", \"Database\"]"
        );
    }

    #[test]
    fn test_format_answers_no_trailing_newline() {
        let answers = vec![("Question 1", "Answer 1"), ("Question 2", "Answer 2")];

        let markdown = super::format_answers(&answers);

        // Sprawdź że nie ma trailing newline
        assert!(!markdown.ends_with('\n'));
        assert_eq!(
            markdown,
            "**Question 1**\nAnswer 1\n\n**Question 2**\nAnswer 2"
        );
    }

    #[test]
    fn test_format_answers_unicode() {
        let answers = vec![
            ("Jak się nazywasz?", "Paweł"),
            ("选择语言", "中文"),
            ("🤔 Question?", "✅ Answer"),
        ];

        let markdown = super::format_answers(&answers);

        let expected = "**Jak się nazywasz?**\nPaweł\n\n\
                        **选择语言**\n中文\n\n\
                        **🤔 Question?**\n✅ Answer";

        assert_eq!(markdown, expected);
    }

    // ── Additional Coverage Tests ─────────────────────────────────────

    #[test]
    fn test_parse_questions_all_types_mixed() {
        // Test wszystkich 4 typów w jednym wywołaniu
        let params = json!({
            "questions": [
                {
                    "question": "Enter your name",
                    "type": "text",
                    "placeholder": "John Doe"
                },
                {
                    "question": "Choose authentication",
                    "type": "choice",
                    "options": [
                        {"label": "JWT"},
                        {"label": "Session"}
                    ]
                },
                {
                    "question": "Select features",
                    "type": "multichoice",
                    "options": [
                        {"label": "Auth"},
                        {"label": "API"}
                    ]
                },
                {
                    "question": "Are you sure?",
                    "type": "confirm",
                    "default": "yes"
                }
            ]
        });

        let questions = parse_questions(&params).unwrap();
        assert_eq!(questions.len(), 4);
        assert_eq!(questions[0].question_type, QuestionType::Text);
        assert_eq!(questions[1].question_type, QuestionType::Choice);
        assert_eq!(questions[2].question_type, QuestionType::MultiChoice);
        assert_eq!(questions[3].question_type, QuestionType::Confirm);
    }

    #[test]
    fn test_parse_questions_required_default_false() {
        // Test explicite required: false
        let params = json!({
            "questions": [
                {
                    "question": "Optional question",
                    "type": "text",
                    "required": false
                }
            ]
        });

        let questions = parse_questions(&params).unwrap();
        assert_eq!(questions.len(), 1);
        assert!(!questions[0].required);
    }

    #[test]
    fn test_parse_questions_with_all_optional_fields() {
        // Test pytania z wszystkimi opcjonalnymi polami
        let params = json!({
            "questions": [
                {
                    "question": "Complete question",
                    "type": "choice",
                    "options": [
                        {
                            "label": "Option A",
                            "description": "Description of A"
                        }
                    ],
                    "default": "Option A",
                    "placeholder": "Choose wisely",
                    "required": true
                }
            ]
        });

        let questions = parse_questions(&params).unwrap();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].default, Some("Option A".into()));
        assert_eq!(questions[0].placeholder, Some("Choose wisely".into()));
        assert!(questions[0].required);
        assert_eq!(
            questions[0].options[0].description,
            Some("Description of A".into())
        );
    }

    #[test]
    fn test_format_answers_preserves_markdown_in_questions() {
        // Sprawdź że markdown w pytaniach nie jest escapowany
        let answers = vec![
            ("What is **bold** text?", "Answer"),
            ("What is `code`?", "Code block"),
        ];

        let markdown = super::format_answers(&answers);

        // Markdown powinien być zachowany (nie escapowany)
        assert!(markdown.contains("**What is **bold** text?**"));
        assert!(markdown.contains("**What is `code`?**"));
    }

    #[test]
    fn test_format_answers_single_empty_answer() {
        // Test pojedynczej pary z pustą odpowiedzią
        let answers = vec![("Question", "")];
        let markdown = super::format_answers(&answers);

        // Pojedyncza para nie powinna mieć trailing newline
        assert_eq!(markdown, "**Question**\n");
        assert!(!markdown.ends_with("\n\n"));
    }

    // ── Handler Tests ─────────────────────────────────────────────────

    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::super::session::SessionRegistry;

    /// Helper: tworzy testowy AppState z kanałem question_tx
    fn make_test_state() -> (AppState, mpsc::Receiver<QuestionEnvelope>) {
        let (tx, rx) = mpsc::channel(32);
        let registry = Arc::new(SessionRegistry::new());
        let token = CancellationToken::new();
        let state = AppState::new(
            registry,
            std::path::PathBuf::from("/tmp/tasks.yml"),
            tx,
            token,
        );
        (state, rx)
    }

    /// Helper: parsuje SSE stream events do Vec<String> (raw text)
    async fn collect_sse_events(sse: Sse<SseStream>) -> String {
        use axum::response::IntoResponse;
        let response = sse.into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    // ── build_answer_markdown Tests ──────────────────────────────────

    #[test]
    fn test_build_answer_markdown_basic() {
        let questions = vec![
            Question {
                question: "What is your name?".into(),
                question_type: QuestionType::Text,
                options: vec![],
                default: None,
                placeholder: None,
                required: true,
            },
            Question {
                question: "Choose auth method".into(),
                question_type: QuestionType::Choice,
                options: vec![
                    QuestionOption {
                        label: "JWT".into(),
                        description: None,
                    },
                    QuestionOption {
                        label: "Session".into(),
                        description: None,
                    },
                ],
                default: None,
                placeholder: None,
                required: true,
            },
        ];

        let answers = vec![
            Answer {
                question: "What is your name?".into(),
                answer: "Alice".into(),
            },
            Answer {
                question: "Choose auth method".into(),
                answer: "JWT".into(),
            },
        ];

        let markdown = build_answer_markdown(&questions, &answers);
        assert!(markdown.contains("**What is your name?**"));
        assert!(markdown.contains("Alice"));
        assert!(markdown.contains("**Choose auth method**"));
        assert!(markdown.contains("JWT"));
    }

    #[test]
    fn test_build_answer_markdown_empty() {
        let questions: Vec<Question> = vec![];
        let answers: Vec<Answer> = vec![];
        let markdown = build_answer_markdown(&questions, &answers);
        assert_eq!(markdown, "");
    }

    #[test]
    fn test_build_answer_markdown_mismatched_lengths() {
        // Więcej pytań niż odpowiedzi — nadmiarowe pytania ignorowane
        let questions = vec![
            Question {
                question: "Q1".into(),
                question_type: QuestionType::Text,
                options: vec![],
                default: None,
                placeholder: None,
                required: true,
            },
            Question {
                question: "Q2".into(),
                question_type: QuestionType::Text,
                options: vec![],
                default: None,
                placeholder: None,
                required: true,
            },
        ];

        let answers = vec![Answer {
            question: "Q1".into(),
            answer: "A1".into(),
        }];

        let markdown = build_answer_markdown(&questions, &answers);
        assert!(markdown.contains("Q1"));
        assert!(markdown.contains("A1"));
        // Q2 nie powinno być w markdown (brak odpowiedzi)
        assert!(!markdown.contains("Q2"));
    }

    // ── make_message_event Tests ─────────────────────────────────────

    #[test]
    fn test_make_message_event_success() {
        let response = JsonRpcResponse::success(Some(json!(1)), json!({"text": "hello"}));
        let _event = make_message_event(&response);
        // Event tworzenie nie panikuje — format jest poprawny
    }

    #[test]
    fn test_make_message_event_error() {
        let response = JsonRpcResponse::error(Some(json!(1)), -32600, "bad request".into());
        let _event = make_message_event(&response);
    }

    // ── handle_ask_user Integration Tests ─────────────────────────────

    #[tokio::test]
    async fn test_handle_ask_user_invalid_params() {
        let (state, _rx) = make_test_state();

        // Brak pola "questions" w params
        let params = json!({"invalid": "params"});
        let result = handle_ask_user(params, state, json!(1)).await;

        let body = collect_sse_events(result).await;
        // Powinien zwrócić SSE event z JSON-RPC error
        assert!(body.contains("event: message"));
        assert!(body.contains("-32602")); // INVALID_PARAMS
        assert!(body.contains("Invalid ask_user params"));
    }

    #[tokio::test]
    async fn test_handle_ask_user_channel_closed() {
        let (state, rx) = make_test_state();
        // Zamknij receiver — sender.send() zwróci błąd
        drop(rx);

        let params = json!({
            "questions": [{
                "question": "Test?",
                "type": "text"
            }]
        });

        let result = handle_ask_user(params, state, json!(2)).await;
        let body = collect_sse_events(result).await;

        // Powinien zwrócić SSE event z internal error
        assert!(body.contains("event: message"));
        assert!(body.contains("-32603")); // INTERNAL_ERROR
        assert!(body.contains("TUI channel closed"));
    }

    #[tokio::test]
    async fn test_handle_ask_user_happy_path() {
        let (state, mut rx) = make_test_state();

        let params = json!({
            "questions": [{
                "question": "What is your name?",
                "type": "text"
            }]
        });

        // Spawn task symulujący TUI — odbiera pytanie i odsyła odpowiedź
        let tui_handle = tokio::spawn(async move {
            let envelope = rx.recv().await.expect("Should receive envelope");
            assert_eq!(envelope.questions.len(), 1);
            assert_eq!(envelope.questions[0].question, "What is your name?");

            envelope
                .response_tx
                .send(vec![Answer {
                    question: "What is your name?".into(),
                    answer: "Alice".into(),
                }])
                .expect("Should send answer");
        });

        let result = handle_ask_user(params, state, json!(3)).await;
        let body = collect_sse_events(result).await;

        tui_handle.await.unwrap();

        // Powinien zwrócić SSE event z JSON-RPC success i markdown
        assert!(body.contains("event: message"));
        assert!(body.contains("\"jsonrpc\":\"2.0\""));
        assert!(body.contains("Alice"));
    }

    #[tokio::test]
    async fn test_handle_ask_user_tui_drops_sender() {
        let (state, mut rx) = make_test_state();

        let params = json!({
            "questions": [{
                "question": "Will be dropped",
                "type": "text"
            }]
        });

        // Spawn task symulujący TUI — odbiera ale dropuje sender bez odpowiedzi
        let tui_handle = tokio::spawn(async move {
            let envelope = rx.recv().await.expect("Should receive envelope");
            // Drop response_tx bez wysłania odpowiedzi
            drop(envelope.response_tx);
        });

        let result = handle_ask_user(params, state, json!(4)).await;
        let body = collect_sse_events(result).await;

        tui_handle.await.unwrap();

        // Powinien zwrócić SSE event z internal error (TUI closed)
        assert!(body.contains("event: message"));
        assert!(body.contains("-32603")); // INTERNAL_ERROR
        assert!(body.contains("TUI closed without responding"));
    }

    #[tokio::test]
    async fn test_handle_ask_user_multiple_questions() {
        let (state, mut rx) = make_test_state();

        let params = json!({
            "questions": [
                {
                    "question": "Your name?",
                    "type": "text"
                },
                {
                    "question": "Choose method",
                    "type": "choice",
                    "options": [
                        {"label": "JWT"},
                        {"label": "Session"}
                    ]
                }
            ]
        });

        let tui_handle = tokio::spawn(async move {
            let envelope = rx.recv().await.expect("Should receive envelope");
            assert_eq!(envelope.questions.len(), 2);

            envelope
                .response_tx
                .send(vec![
                    Answer {
                        question: "Your name?".into(),
                        answer: "Bob".into(),
                    },
                    Answer {
                        question: "Choose method".into(),
                        answer: "JWT".into(),
                    },
                ])
                .expect("Should send answers");
        });

        let result = handle_ask_user(params, state, json!(5)).await;
        let body = collect_sse_events(result).await;

        tui_handle.await.unwrap();

        assert!(body.contains("Bob"));
        assert!(body.contains("JWT"));
        assert!(body.contains("**Your name?**"));
        assert!(body.contains("**Choose method**"));
    }

    // ── error_sse_response Tests ─────────────────────────────────────

    #[tokio::test]
    async fn test_error_sse_response_format() {
        let sse = error_sse_response(json!(99), -32600, "bad request".into());
        let body = collect_sse_events(sse).await;

        // Sprawdź format SSE event (axum SSE format: `event: message\n`)
        assert!(body.contains("event: message"));
        assert!(body.contains("-32600"));
        assert!(body.contains("bad request"));
        assert!(body.contains("\"id\":99"));
    }

    // ── Simple format compat tests ─────────────────────────────────

    #[test]
    fn test_parse_questions_simple_text() {
        let params = json!({"question": "What is your name?"});
        let questions = parse_questions(&params).unwrap();

        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].question, "What is your name?");
        assert_eq!(questions[0].question_type, QuestionType::Text);
        assert!(questions[0].options.is_empty());
        assert!(questions[0].required);
    }

    #[test]
    fn test_parse_questions_simple_with_options() {
        let params = json!({
            "question": "Choose auth method",
            "options": ["JWT", "Session", "OAuth"]
        });
        let questions = parse_questions(&params).unwrap();

        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].question_type, QuestionType::Choice);
        assert_eq!(questions[0].options.len(), 3);
        assert_eq!(questions[0].options[0].label, "JWT");
        assert_eq!(questions[0].options[1].label, "Session");
        assert!(questions[0].options[0].description.is_none());
    }

    #[test]
    fn test_parse_questions_simple_empty_question_fails() {
        let params = json!({"question": ""});
        let result = parse_questions(&params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_parse_questions_simple_whitespace_question_fails() {
        let params = json!({"question": "   "});
        let result = parse_questions(&params);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_questions_simple_empty_options_is_text() {
        let params = json!({"question": "Tell me more", "options": []});
        let questions = parse_questions(&params).unwrap();

        assert_eq!(questions[0].question_type, QuestionType::Text);
        assert!(questions[0].options.is_empty());
    }

    #[test]
    fn test_parse_questions_simple_multi_select_with_options() {
        let params = json!({
            "question": "Select features",
            "options": ["Auth", "Logging", "Caching"],
            "multiSelect": true
        });
        let questions = parse_questions(&params).unwrap();

        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].question_type, QuestionType::MultiChoice);
        assert_eq!(questions[0].options.len(), 3);
        assert_eq!(questions[0].options[0].label, "Auth");
    }

    #[test]
    fn test_parse_questions_simple_multi_select_false_is_choice() {
        let params = json!({
            "question": "Pick one",
            "options": ["A", "B"],
            "multiSelect": false
        });
        let questions = parse_questions(&params).unwrap();

        assert_eq!(questions[0].question_type, QuestionType::Choice);
    }

    #[test]
    fn test_parse_questions_simple_multi_select_without_options_is_text() {
        let params = json!({
            "question": "Tell me more",
            "multiSelect": true
        });
        let questions = parse_questions(&params).unwrap();

        assert_eq!(questions[0].question_type, QuestionType::Text);
    }

    #[test]
    fn test_parse_questions_simple_multi_select_missing_defaults_to_choice() {
        let params = json!({
            "question": "Pick one",
            "options": ["X", "Y"]
        });
        let questions = parse_questions(&params).unwrap();

        assert_eq!(questions[0].question_type, QuestionType::Choice);
    }

    #[test]
    fn test_parse_questions_full_format_still_works() {
        let params = json!({
            "questions": [{
                "question": "Pick one",
                "type": "choice",
                "options": [
                    {"label": "A", "description": "First"},
                    {"label": "B"}
                ]
            }]
        });
        let questions = parse_questions(&params).unwrap();

        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].question_type, QuestionType::Choice);
        assert_eq!(questions[0].options[0].description, Some("First".into()));
    }
}
