// Typy stanu aplikacji i kanałów komunikacji dla HTTP MCP server

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::ask_user;
use super::session::SessionRegistry;

// ── Question Envelope ──────────────────────────────────────────────

/// Koperta z pytaniami i kanałem zwrotnym dla odpowiedzi
///
/// Używana do komunikacji między handlerem MCP (axum) a pętlą TUI.
/// Handler wysyła QuestionEnvelope przez mpsc::Sender, TUI odbiera,
/// wyświetla pytania użytkownikowi, i odsyła odpowiedzi przez oneshot::Sender.
///
/// Używa typów z modułu `ask_user` — format MCP tool params,
/// który bezpośrednio mapuje się na widgety TUI (text_input, choice_select, etc.).
#[derive(Debug)]
#[allow(dead_code)] // Pola czytane przez TUI consumer (przyszły task)
pub struct QuestionEnvelope {
    /// Lista pytań do zadania użytkownikowi (format MCP ask_user tool)
    pub questions: Vec<ask_user::Question>,
    /// Kanał zwrotny dla odpowiedzi (one-shot: jedna odpowiedź)
    pub response_tx: oneshot::Sender<Vec<ask_user::Answer>>,
}

// ── App State ──────────────────────────────────────────────────────

/// Globalny stan aplikacji współdzielony przez handlery axum
///
/// AppState przechowuje:
/// - SessionRegistry: mapę sesji MCP (UUID -> SessionState)
/// - default_tasks_path: domyślną ścieżkę do tasks.yml dla nowych sesji
/// - question_tx: kanał do wysyłania pytań do TUI
/// - cancellation_token: token do graceful shutdown servera
///
/// Stan jest współdzielony przez wszystkie handlery axum za pomocą
/// `axum::extract::State<AppState>`.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Używane w przyszłych taskach (HTTP MCP handlers)
pub struct AppState {
    /// Rejestr sesji MCP (thread-safe — wewnętrzny Arc<Mutex<>> w SessionRegistry)
    pub session_registry: Arc<SessionRegistry>,
    /// Domyślna ścieżka do pliku tasks.yml — używana przy tworzeniu nowych sesji
    pub default_tasks_path: std::path::PathBuf,
    /// Kanał do wysyłania pytań do TUI (mpsc: wiele wątków może wysyłać)
    pub question_tx: mpsc::Sender<QuestionEnvelope>,
    /// Token anulowania dla graceful shutdown
    pub cancellation_token: CancellationToken,
}

impl AppState {
    /// Tworzy nowy AppState z podanymi komponentami
    ///
    /// # Arguments
    ///
    /// * `session_registry` - Rejestr sesji MCP (Arc — SessionRegistry jest wewnętrznie thread-safe)
    /// * `default_tasks_path` - Domyślna ścieżka do tasks.yml dla nowych sesji
    /// * `question_tx` - Kanał do wysyłania pytań do TUI
    /// * `cancellation_token` - Token anulowania dla shutdown
    #[must_use]
    #[allow(dead_code)] // Używane w przyszłych taskach (HTTP MCP handlers)
    pub fn new(
        session_registry: Arc<SessionRegistry>,
        default_tasks_path: std::path::PathBuf,
        question_tx: mpsc::Sender<QuestionEnvelope>,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            session_registry,
            default_tasks_path,
            question_tx,
            cancellation_token,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_question_envelope_creation() {
        let questions = vec![ask_user::Question {
            question: "Test question?".into(),
            question_type: ask_user::QuestionType::Choice,
            options: vec![
                ask_user::QuestionOption {
                    label: "Yes".into(),
                    description: Some("Affirmative".into()),
                },
                ask_user::QuestionOption {
                    label: "No".into(),
                    description: Some("Negative".into()),
                },
            ],
            default: None,
            placeholder: None,
            required: true,
        }];

        let (tx, _rx) = oneshot::channel();
        let envelope = QuestionEnvelope {
            questions: questions.clone(),
            response_tx: tx,
        };

        assert_eq!(envelope.questions.len(), 1);
        assert_eq!(envelope.questions[0].question, "Test question?");
    }

    #[test]
    fn test_app_state_creation() {
        let registry = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let path = std::path::PathBuf::from(".ralph/tasks.yml");

        let state = AppState::new(registry.clone(), path.clone(), tx, token.clone());

        assert!(Arc::ptr_eq(&state.session_registry, &registry));
        assert_eq!(state.default_tasks_path, path);
        assert!(!token.is_cancelled());
    }

    #[test]
    fn test_app_state_clone() {
        let registry = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let path = std::path::PathBuf::from(".ralph/tasks.yml");

        let state1 = AppState::new(registry.clone(), path, tx.clone(), token.clone());
        let state2 = state1.clone();

        // Clone powinien wskazywać na te same Arc
        assert!(Arc::ptr_eq(
            &state1.session_registry,
            &state2.session_registry
        ));
        assert_eq!(state1.default_tasks_path, state2.default_tasks_path);
        assert_eq!(
            state1.cancellation_token.is_cancelled(),
            state2.cancellation_token.is_cancelled()
        );
    }

    #[test]
    fn test_cancellation_token_cancellation() {
        let registry = Arc::new(SessionRegistry::new());
        let (tx, _rx) = mpsc::channel(32);
        let token = CancellationToken::new();
        let path = std::path::PathBuf::from(".ralph/tasks.yml");

        let state = AppState::new(registry, path, tx, token.clone());

        assert!(!state.cancellation_token.is_cancelled());

        token.cancel();

        assert!(state.cancellation_token.is_cancelled());
    }

    #[tokio::test]
    async fn test_question_envelope_channel_communication() {
        let questions = vec![ask_user::Question {
            question: "Choose one".into(),
            question_type: ask_user::QuestionType::Choice,
            options: vec![
                ask_user::QuestionOption {
                    label: "A".into(),
                    description: Some("Option A".into()),
                },
                ask_user::QuestionOption {
                    label: "B".into(),
                    description: Some("Option B".into()),
                },
            ],
            default: None,
            placeholder: None,
            required: true,
        }];

        let (tx, rx) = oneshot::channel();
        let envelope = QuestionEnvelope {
            questions: questions.clone(),
            response_tx: tx,
        };

        // Symulacja odpowiedzi
        let answers = vec![ask_user::Answer {
            question: "Choose one".into(),
            answer: "A".into(),
        }];

        envelope.response_tx.send(answers.clone()).unwrap();

        let received = rx.await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].question, "Choose one");
        assert_eq!(received[0].answer, "A");
    }

    #[tokio::test]
    async fn test_mpsc_channel_for_envelopes() {
        let (tx, mut rx) = mpsc::channel::<QuestionEnvelope>(10);

        let question = ask_user::Question {
            question: "Test?".into(),
            question_type: ask_user::QuestionType::Confirm,
            options: vec![],
            default: None,
            placeholder: None,
            required: true,
        };

        let (resp_tx, resp_rx) = oneshot::channel();
        let envelope = QuestionEnvelope {
            questions: vec![question.clone()],
            response_tx: resp_tx,
        };

        tx.send(envelope).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.questions.len(), 1);
        assert_eq!(received.questions[0].question, "Test?");

        // Symulacja odpowiedzi
        received
            .response_tx
            .send(vec![ask_user::Answer {
                question: "Test?".into(),
                answer: "yes".into(),
            }])
            .unwrap();

        let answer = resp_rx.await.unwrap();
        assert_eq!(answer[0].answer, "yes");
    }
}
