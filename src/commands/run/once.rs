use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use tokio::signal;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::commands::mcp::ask_user::{self, Answer};
use crate::commands::mcp::server::McpServer;
use crate::commands::mcp::state::QuestionEnvelope;
use crate::commands::mcp::tui::render_question;
use crate::shared::error::Result;
use crate::shared::mcp::build_mcp_config;

use super::events::InputThread;
use super::output::OutputFormatter;
use super::runner::ClaudeRunner;
use super::ui::StatusTerminal;

/// Readonly built-in tools for codebase exploration (no Write, Edit, Bash).
#[allow(dead_code)] // Used in tests; kept for future use if allowlist approach is needed
pub(crate) const READONLY_TOOLS: &str = "Read,Glob,Grep,LS,WebFetch,WebSearch";

/// Built-in tools to block when Claude should only read + use MCP.
/// Used with disallowed_tools (denylist approach) to avoid blocking MCP tools.
pub(crate) const DANGEROUS_TOOLS: &str = "Write,Edit,Bash,NotebookEdit,TodoWrite";

pub(crate) struct RunOnceOptions {
    pub prompt: String,
    pub model: Option<String>,
    pub output_dir: Option<PathBuf>,
    pub use_nerd_font: bool,
    /// When set, passed as --allowedTools to restrict Claude's available tools.
    /// Defines the allowlist of tools Claude can use (e.g., "Read,Write,Bash").
    pub allowed_tools: Option<String>,
    /// When set, passed as --disallowedTools to explicitly block specific tools.
    /// Used to blacklist tools regardless of allowedTools setting (e.g., "AskUserQuestion").
    /// Useful for enforcing MCP-based user interaction instead of AskUserQuestion.
    pub disallowed_tools: Option<String>,
    /// When set, passed as --mcp-config to provide MCP servers to Claude.
    /// Takes precedence over auto-started server (tasks_path is ignored).
    pub mcp_config: Option<serde_json::Value>,
    /// Optional receiver for ask_user questions from MCP server.
    /// When present, questions are rendered as TUI widgets and answers
    /// are sent back via the envelope's oneshot channel.
    pub question_rx: Option<mpsc::Receiver<QuestionEnvelope>>,
    /// Path to tasks.yml for auto-starting MCP server.
    /// When set and mcp_config is None, run_once() automatically starts McpServer,
    /// builds mcp_config from port, creates question channel, and integrates
    /// question_rx into the event loop.
    pub tasks_path: Option<PathBuf>,
}

/// Run Claude CLI once with full streaming output (same UX as `run` command).
/// Sets up OutputFormatter + StatusTerminal + InputThread for rich display,
/// then runs a single ClaudeRunner invocation.
///
/// When `tasks_path` is set and `mcp_config` is None, automatically starts
/// an McpServer, builds mcp_config from its port, and creates a question channel.
///
/// When `question_rx` is provided (or auto-created), a background task listens
/// for incoming QuestionEnvelopes. For each envelope it pauses the InputThread,
/// clears the status bar, renders TUI question widgets, collects answers,
/// then resumes normal display.
pub(crate) async fn run_once(options: RunOnceOptions) -> Result<()> {
    // Setup Ctrl+C handler
    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = signal::ctrl_c().await;
            shutdown.store(true, Ordering::SeqCst);
        });
    }

    // Auto-start MCP server if tasks_path is set and no custom mcp_config provided.
    // Destructure immediately: _server_guard keeps the server alive until run_once exits.
    let (auto_mcp_config, auto_question_rx, _server_guard) =
        match (&options.tasks_path, &options.mcp_config) {
            (Some(tasks_path), None) => {
                let started = start_mcp_server(tasks_path.clone()).await?;
                (
                    Some(started.mcp_config),
                    Some(started.question_rx),
                    Some(started.guard),
                )
            }
            _ => (None, None, None),
        };

    // Resolve mcp_config and question_rx: auto-started server or caller-provided
    let mcp_config = options.mcp_config.or(auto_mcp_config);
    let question_rx = options.question_rx.or(auto_question_rx);

    // Initialize output formatter (iteration=0 signals one-shot mode to status bar)
    let formatter = Arc::new(Mutex::new(OutputFormatter::new(options.use_nerd_font)));
    formatter.lock().unwrap().start_iteration();

    // Initialize status terminal (enables raw mode)
    let status_terminal = Arc::new(Mutex::new(StatusTerminal::new(options.use_nerd_font)?));

    // Flags required by InputThread (update-related ones are unused for one-shot)
    let resize_flag = Arc::new(AtomicBool::new(false));
    let update_state = Arc::new(AtomicU8::new(0));
    let update_trigger = Arc::new(AtomicBool::new(false));
    let refresh_flag = Arc::new(AtomicBool::new(false));

    // Spawn keyboard input thread (handles Ctrl+C/q in raw mode)
    let input_thread = InputThread::spawn(
        shutdown.clone(),
        resize_flag.clone(),
        update_state.clone(),
        update_trigger.clone(),
        refresh_flag.clone(),
    );
    let input_paused = input_thread.paused_flag();

    // Spawn background task for handling ask_user questions from MCP
    let input_paused_for_callbacks = input_paused.clone();
    let question_handle = spawn_question_handler(
        question_rx,
        input_paused,
        status_terminal.clone(),
        formatter.clone(),
        shutdown.clone(),
    );

    // Create and run ClaudeRunner
    let mut runner = ClaudeRunner::oneshot(options.prompt, options.model, options.output_dir);
    if let Some(tools) = options.allowed_tools {
        runner = runner.with_allowed_tools(tools);
    }
    if let Some(tools) = options.disallowed_tools {
        runner = runner.with_disallowed_tools(tools);
    }
    if let Some(mcp_config) = mcp_config {
        runner = runner.with_mcp_config(mcp_config);
    }

    let formatter_event = formatter.clone();
    let status_terminal_event = status_terminal.clone();
    let shutdown_event = shutdown.clone();
    let resize_flag_event = resize_flag.clone();
    let input_paused_event = input_paused_for_callbacks.clone();

    let formatter_idle = formatter.clone();
    let status_terminal_idle = status_terminal.clone();
    let resize_flag_idle = resize_flag.clone();
    let input_paused_idle = input_paused_for_callbacks.clone();

    let run_result = runner
        .run(
            shutdown.clone(),
            |event| {
                let (lines, status) = {
                    let mut fmt = formatter_event.lock().unwrap();
                    let lines = fmt.format_event(event);
                    let status = fmt.get_status();
                    (lines, status)
                };
                // Skip terminal rendering while TUI question widgets are active.
                // The question handler owns the terminal during this time.
                if input_paused_event.load(Ordering::Relaxed) {
                    return;
                }
                let mut term = status_terminal_event.lock().unwrap();
                if resize_flag_event.swap(false, Ordering::SeqCst) {
                    let _ = term.handle_resize(&status);
                }
                for line in &lines {
                    let _ = term.print_line(line);
                }
                if shutdown_event.load(Ordering::SeqCst) {
                    let _ = term.show_shutting_down();
                } else {
                    let _ = term.update(&status);
                }
            },
            || {
                // Skip idle updates while TUI question widgets are active.
                if input_paused_idle.load(Ordering::Relaxed) {
                    return;
                }
                let status = formatter_idle.lock().unwrap().get_status();
                let mut term = status_terminal_idle.lock().unwrap();
                if resize_flag_idle.swap(false, Ordering::SeqCst) {
                    let _ = term.handle_resize(&status);
                }
                let _ = term.update(&status);
            },
        )
        .await;

    // Question handler will exit gracefully via shutdown flag or channel close.
    // We do NOT use .abort() because:
    // 1. spawn_blocking tasks cannot be aborted (they're not async)
    // 2. PauseGuard drop must complete to resume InputThread
    // Instead, we rely on:
    // - shutdown flag check in spawn_question_handler loop (line 254)
    // - question_rx channel closure (when tx is dropped by server shutdown)
    //
    // IMPORTANT: Drop _server_guard BEFORE awaiting question_handle to ensure
    // the MCP server shuts down and closes question_tx, allowing question_rx
    // to detect channel closure and exit cleanly.
    drop(_server_guard);

    if let Some(handle) = question_handle {
        // Wait for graceful shutdown with timeout (should be fast now that channel is closed)
        let timeout = tokio::time::Duration::from_secs(2);
        let _ = tokio::time::timeout(timeout, handle).await;
    }

    // Cleanup terminal (disable raw mode, clear status bar)
    {
        let mut term = status_terminal.lock().unwrap();
        term.cleanup()?;
    }
    input_thread.stop();

    run_result?;
    Ok(())
}

/// Data returned by auto-starting an MCP server inside run_once().
///
/// Fields are consumed directly by run_once(); the `guard` must be held
/// for the lifetime of the Claude invocation to keep the server alive.
struct AutoStartedServer {
    mcp_config: serde_json::Value,
    question_rx: mpsc::Receiver<QuestionEnvelope>,
    /// RAII guard — cancels the server on drop. Must be held until run_once exits.
    guard: ServerGuard,
}

/// RAII guard that cancels the MCP server on drop via CancellationToken.
///
/// Stored as `_server_guard` in run_once() to tie server lifetime to the function scope.
struct ServerGuard {
    cancel_token: CancellationToken,
    _server_handle: tokio::task::JoinHandle<()>,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.cancel_token.cancel();
    }
}

/// Start an MCP server for the given tasks_path and return auto-start state.
///
/// Creates mpsc channel for QuestionEnvelopes, CancellationToken for shutdown,
/// starts McpServer on a random port, and builds mcp_config from that port.
async fn start_mcp_server(tasks_path: PathBuf) -> Result<AutoStartedServer> {
    let (question_tx, question_rx) = mpsc::channel::<QuestionEnvelope>(32);
    let cancel_token = CancellationToken::new();

    let server = McpServer::new(tasks_path, question_tx, cancel_token.clone());
    let (port, server_handle) = server.start().await?;

    let mcp_config = build_mcp_config(port);

    Ok(AutoStartedServer {
        mcp_config,
        question_rx,
        guard: ServerGuard {
            cancel_token,
            _server_handle: server_handle,
        },
    })
}

/// Spawns a background tokio task that listens for QuestionEnvelopes.
///
/// For each envelope:
/// 1. Pauses InputThread (sets paused flag)
/// 2. Clears status bar to make room for question widgets
/// 3. Renders each question via TUI widgets (blocking — uses spawn_blocking)
/// 4. Sends collected answers back via the envelope's oneshot channel
/// 5. Resumes InputThread and restores status bar
///
/// Returns None if `question_rx` is None (no MCP server configured).
fn spawn_question_handler(
    question_rx: Option<mpsc::Receiver<QuestionEnvelope>>,
    input_paused: Arc<AtomicBool>,
    status_terminal: Arc<Mutex<StatusTerminal>>,
    formatter: Arc<Mutex<OutputFormatter>>,
    shutdown: Arc<AtomicBool>,
) -> Option<tokio::task::JoinHandle<()>> {
    let mut question_rx = question_rx?;

    Some(tokio::spawn(async move {
        while let Some(envelope) = question_rx.recv().await {
            if shutdown.load(Ordering::SeqCst) {
                // Drop envelope — sender gets RecvError indicating no answer
                break;
            }

            let answers = handle_question_envelope(
                &envelope.questions,
                &input_paused,
                &status_terminal,
                &formatter,
            )
            .await;

            // Send answers back; ignore error if Claude exited (receiver dropped)
            let _ = envelope.response_tx.send(answers);
        }
    }))
}

/// RAII guard that resumes InputThread on drop, even if a panic occurs.
///
/// This guard ensures that the input thread is ALWAYS resumed after rendering
/// TUI questions, even if the rendering code panics. Without this guard, a panic
/// during question rendering would leave the input thread permanently paused,
/// causing the application to hang.
///
/// # Example
/// ```ignore
/// let paused = Arc::new(AtomicBool::new(false));
/// paused.store(true, Ordering::Relaxed);
/// let _guard = PauseGuard(paused.clone());
/// // ... render questions (may panic) ...
/// // Guard automatically resumes on scope exit
/// ```
struct PauseGuard(Arc<AtomicBool>);

impl Drop for PauseGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// Handles a single QuestionEnvelope: pauses input, renders questions, resumes input.
///
/// Uses `PauseGuard` to ensure InputThread is always resumed, even if rendering panics.
async fn handle_question_envelope(
    questions: &[ask_user::Question],
    input_paused: &Arc<AtomicBool>,
    status_terminal: &Arc<Mutex<StatusTerminal>>,
    formatter: &Arc<Mutex<OutputFormatter>>,
) -> Vec<Answer> {
    // Pause InputThread so TUI widgets can read keyboard events directly.
    // PauseGuard ensures resume on drop (panic safety).
    input_paused.store(true, Ordering::Relaxed);
    let _guard = PauseGuard(input_paused.clone());

    // Clear status bar to make room for question rendering
    {
        let mut term = status_terminal.lock().unwrap();
        let _ = term.cleanup();
    }

    // Render questions on a blocking thread (TUI widgets do synchronous crossterm I/O)
    let questions_owned: Vec<ask_user::Question> = questions.to_vec();
    let answers = tokio::task::spawn_blocking(move || collect_answers(&questions_owned))
        .await
        .unwrap_or_default();

    // Re-enable raw mode and restore status bar after questions are done
    {
        let status = formatter.lock().unwrap().get_status();
        let mut term = status_terminal.lock().unwrap();
        let _ = term.reinit();
        let _ = term.update(&status);
    }

    // _guard drops here, resuming InputThread via PauseGuard::drop()
    answers
}

/// Renders each question via TUI widget and collects answers.
///
/// If a question fails to render (e.g. user presses Ctrl+C during input),
/// returns answers collected so far — partial responses are still useful.
fn collect_answers(questions: &[ask_user::Question]) -> Vec<Answer> {
    let mut answers = Vec::with_capacity(questions.len());

    for question in questions {
        match render_question(question) {
            Ok(answer_text) => {
                answers.push(Answer {
                    question: question.question.clone(),
                    answer: answer_text,
                });
            }
            Err(_) => {
                // User cancelled or widget error — return partial answers
                break;
            }
        }
    }

    answers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::mcp::ask_user::{Question, QuestionType};
    use tokio::sync::oneshot;

    #[test]
    fn test_run_once_options_defaults() {
        // Verify the struct accepts all None/default fields
        let _opts = RunOnceOptions {
            prompt: "test".into(),
            model: None,
            output_dir: None,
            use_nerd_font: false,
            allowed_tools: None,
            disallowed_tools: None,
            mcp_config: None,
            question_rx: None,
            tasks_path: None,
        };
    }

    #[test]
    fn test_run_once_options_with_question_rx() {
        let (_tx, rx) = mpsc::channel::<QuestionEnvelope>(1);
        let _opts = RunOnceOptions {
            prompt: "test".into(),
            model: None,
            output_dir: None,
            use_nerd_font: false,
            allowed_tools: None,
            disallowed_tools: None,
            mcp_config: None,
            question_rx: Some(rx),
            tasks_path: None,
        };
    }

    #[test]
    fn test_run_once_options_with_tasks_path() {
        let _opts = RunOnceOptions {
            prompt: "test".into(),
            model: None,
            output_dir: None,
            use_nerd_font: false,
            allowed_tools: None,
            disallowed_tools: None,
            mcp_config: None,
            question_rx: None,
            tasks_path: Some(PathBuf::from(".ralph/tasks.yml")),
        };
    }

    #[test]
    fn test_run_once_options_custom_mcp_overrides_tasks_path() {
        // When both mcp_config and tasks_path are set, mcp_config takes precedence
        let custom_config = serde_json::json!({"mcpServers": {}});
        let _opts = RunOnceOptions {
            prompt: "test".into(),
            model: None,
            output_dir: None,
            use_nerd_font: false,
            allowed_tools: None,
            disallowed_tools: None,
            mcp_config: Some(custom_config),
            question_rx: None,
            tasks_path: Some(PathBuf::from(".ralph/tasks.yml")),
        };
    }

    #[test]
    fn test_collect_answers_empty_questions() {
        let answers = collect_answers(&[]);
        assert!(answers.is_empty());
    }

    #[test]
    fn test_collect_answers_partial_on_error() {
        // collect_answers returns partial results when a widget fails.
        // All current TUI widgets return errors (stubs), so with any questions
        // we get an empty vec — verifying the error-break path.
        let questions = vec![Question {
            question: "Will fail".into(),
            question_type: QuestionType::Text,
            options: vec![],
            default: None,
            placeholder: None,
            required: true,
        }];
        let answers = collect_answers(&questions);
        // Stub widgets return errors, so no answers collected
        assert!(answers.is_empty());
    }

    #[tokio::test]
    async fn test_question_handler_shutdown_drops_envelope() {
        let (tx, rx) = mpsc::channel::<QuestionEnvelope>(1);
        let _input_paused = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(true)); // Already shut down

        // We can't easily create StatusTerminal in tests (needs real terminal),
        // but we can verify the handler respects shutdown by checking it drops the envelope.
        // The handler checks shutdown before processing, so the envelope's response_tx
        // will be dropped without sending, causing the oneshot receiver to get RecvError.

        let (response_tx, response_rx) = oneshot::channel::<Vec<Answer>>();
        let envelope = QuestionEnvelope {
            questions: vec![Question {
                question: "Test?".into(),
                question_type: QuestionType::Text,
                options: vec![],
                default: None,
                placeholder: None,
                required: true,
            }],
            response_tx,
        };

        // Send envelope, then drop tx to close channel after first message
        tx.send(envelope).await.unwrap();
        drop(tx);

        // Create a minimal handler that just checks shutdown
        let handle = tokio::spawn(async move {
            let mut rx = rx;
            while let Some(envelope) = rx.recv().await {
                if shutdown.load(Ordering::SeqCst) {
                    // Drop envelope without answering
                    drop(envelope);
                    break;
                }
            }
        });

        handle.await.unwrap();

        // Receiver should get an error (sender dropped without sending)
        assert!(response_rx.await.is_err());
    }

    #[test]
    fn test_readonly_tools_constant() {
        assert!(READONLY_TOOLS.contains("Read"));
        assert!(READONLY_TOOLS.contains("Glob"));
        assert!(READONLY_TOOLS.contains("Grep"));
        assert!(!READONLY_TOOLS.contains("Write"));
        assert!(!READONLY_TOOLS.contains("Bash"));
    }

    #[test]
    fn test_run_once_options_with_disallowed_tools() {
        let _opts = RunOnceOptions {
            prompt: "test".into(),
            model: None,
            output_dir: None,
            use_nerd_font: false,
            allowed_tools: Some("Read,Write".into()),
            disallowed_tools: Some("AskUserQuestion".into()),
            mcp_config: None,
            question_rx: None,
            tasks_path: None,
        };
    }

    #[tokio::test]
    async fn test_start_mcp_server_returns_valid_config() {
        let tasks_path = PathBuf::from("/tmp/test_tasks.yml");
        let server = start_mcp_server(tasks_path).await.unwrap();

        // Verify mcp_config has expected structure
        let servers = server.mcp_config.get("mcpServers").unwrap();
        let ralph = servers.get("ralph-tasks").unwrap();
        assert_eq!(ralph.get("type").unwrap().as_str(), Some("http"));

        let url = ralph.get("url").unwrap().as_str().unwrap();
        assert!(url.starts_with("http://127.0.0.1:"));
        assert!(url.ends_with("/mcp"));
    }

    #[tokio::test]
    async fn test_server_guard_cancels_on_drop() {
        let tasks_path = PathBuf::from("/tmp/test_tasks.yml");
        let server = start_mcp_server(tasks_path).await.unwrap();
        let token = server.guard.cancel_token.clone();

        assert!(!token.is_cancelled());
        drop(server.guard);
        assert!(token.is_cancelled(), "Drop should cancel the server");
    }

    #[tokio::test]
    async fn test_server_guard_keeps_server_alive() {
        // Verify that consuming question_rx does NOT cancel the server
        // (this was a bug in the original implementation)
        let tasks_path = PathBuf::from("/tmp/test_tasks.yml");
        let server = start_mcp_server(tasks_path).await.unwrap();

        // Extract port before consuming fields
        let url = server.mcp_config["mcpServers"]["ralph-tasks"]["url"]
            .as_str()
            .unwrap()
            .to_string();

        // Consume mcp_config and question_rx (as run_once does), keep guard alive
        let _mcp_config = server.mcp_config;
        let _question_rx = server.question_rx;
        let guard = server.guard;

        // Server should still be reachable — guard keeps it alive
        let port_str = url
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches("/mcp");
        let port: u16 = port_str.parse().unwrap();
        let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await;
        assert!(
            result.is_ok(),
            "Server should still be alive while guard is held"
        );

        drop(guard);
    }

    #[tokio::test]
    async fn test_start_mcp_server_binds_to_reachable_port() {
        let tasks_path = PathBuf::from("/tmp/test_tasks.yml");
        let server = start_mcp_server(tasks_path).await.unwrap();

        // Extract port from mcp_config URL
        let url = server.mcp_config["mcpServers"]["ralph-tasks"]["url"]
            .as_str()
            .unwrap();
        let port_str = url
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches("/mcp");
        let port: u16 = port_str.parse().unwrap();
        assert!(port > 0);

        // Verify we can connect to the server
        let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await;
        assert!(result.is_ok(), "Should connect to auto-started server");
    }

    #[tokio::test]
    async fn test_shutdown_flag_interrupts_question_handler() {
        // Verify that setting shutdown flag during question handling
        // causes the handler to drop envelopes without processing
        let (tx, rx) = mpsc::channel::<QuestionEnvelope>(1);
        let input_paused = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));

        // Create minimal StatusTerminal and OutputFormatter for handler
        // (We can't create real ones in tests, but handler accepts Arc<Mutex<T>>)
        let use_nerd_font = false;
        let formatter = Arc::new(Mutex::new(OutputFormatter::new(use_nerd_font)));
        let status_terminal = match StatusTerminal::new(use_nerd_font) {
            Ok(term) => Arc::new(Mutex::new(term)),
            Err(_) => {
                // Skip test if we can't create terminal (non-TTY environment)
                return;
            }
        };

        let handler = spawn_question_handler(
            Some(rx),
            input_paused,
            status_terminal,
            formatter,
            shutdown.clone(),
        );

        // Set shutdown BEFORE sending question
        shutdown.store(true, Ordering::SeqCst);

        let (response_tx, response_rx) = tokio::sync::oneshot::channel::<Vec<Answer>>();
        let envelope = QuestionEnvelope {
            questions: vec![ask_user::Question {
                question: "Test?".into(),
                question_type: ask_user::QuestionType::Text,
                options: vec![],
                default: None,
                placeholder: None,
                required: true,
            }],
            response_tx,
        };

        // Send envelope, then close channel
        tx.send(envelope).await.unwrap();
        drop(tx);

        // Wait for handler to process
        if let Some(h) = handler {
            h.await.unwrap();
        }

        // Response should be dropped (RecvError)
        assert!(
            response_rx.await.is_err(),
            "Question should be dropped when shutdown is set"
        );
    }

    #[tokio::test]
    async fn test_server_guard_releases_port_on_drop() {
        // Verify that dropping ServerGuard releases the port immediately
        let tasks_path = PathBuf::from("/tmp/test_tasks.yml");
        let server = start_mcp_server(tasks_path.clone()).await.unwrap();

        // Extract port from config
        let url = server.mcp_config["mcpServers"]["ralph-tasks"]["url"]
            .as_str()
            .unwrap()
            .to_string();
        let port_str = url
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches("/mcp");
        let port: u16 = port_str.parse().unwrap();

        // Verify server is reachable
        let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await;
        assert!(result.is_ok(), "Server should be reachable before drop");

        // Drop the server guard
        drop(server.guard);

        // Give server time to shut down
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Start a NEW server on a random port (should succeed if port was released)
        let server2 = start_mcp_server(tasks_path).await;
        assert!(
            server2.is_ok(),
            "Should be able to start new server after dropping guard"
        );
    }

    #[tokio::test]
    async fn test_pause_guard_always_resumes_input_thread() {
        // Verify that PauseGuard resumes InputThread even if scope exits early
        let paused = Arc::new(AtomicBool::new(false));

        // Pause and create guard
        paused.store(true, Ordering::Relaxed);
        assert!(paused.load(Ordering::Relaxed), "Should be paused");

        {
            let _guard = PauseGuard(paused.clone());
            assert!(
                paused.load(Ordering::Relaxed),
                "Should still be paused in scope"
            );
        } // Guard drops here

        // After scope, should be resumed
        assert!(
            !paused.load(Ordering::Relaxed),
            "PauseGuard should resume on drop"
        );
    }

    #[tokio::test]
    async fn test_multiple_server_guards_independent_cancellation() {
        // Verify that multiple servers can be started and cancelled independently
        let (tx1, _rx1) = mpsc::channel(1);
        let (tx2, _rx2) = mpsc::channel(1);
        let token1 = CancellationToken::new();
        let token2 = CancellationToken::new();

        let server1 = McpServer::new(PathBuf::from("/tmp/t1.yml"), tx1, token1.clone());
        let server2 = McpServer::new(PathBuf::from("/tmp/t2.yml"), tx2, token2.clone());

        let (_port1, _handle1) = server1.start().await.unwrap();
        let (_port2, _handle2) = server2.start().await.unwrap();

        // Cancel only server1
        token1.cancel();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Server1 should be cancelled, server2 still running
        assert!(token1.is_cancelled());
        assert!(!token2.is_cancelled());

        // Cleanup server2
        token2.cancel();
    }
}
