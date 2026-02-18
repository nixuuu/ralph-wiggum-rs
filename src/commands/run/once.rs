use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::commands::mcp::ask_user::Answer;
use crate::commands::mcp::server::McpServer;
use crate::commands::mcp::state::QuestionEnvelope;
use crate::commands::task::command_app::TaskCommandApp;
use crate::shared::error::Result;
use crate::shared::mcp::build_mcp_config;
use crate::tui::app::App;

use super::output::OutputFormatter;
use super::runner::ClaudeRunner;

/// Built-in tools to block when Claude should only read + use MCP.
/// Used with disallowed_tools (denylist approach) to avoid blocking MCP tools.
pub(crate) const DANGEROUS_TOOLS: &str = "Write,Edit,Bash,NotebookEdit,TodoWrite";

pub(crate) struct RunOnceOptions {
    pub prompt: String,
    pub model: Option<String>,
    pub output_dir: Option<PathBuf>,
    pub use_nerd_font: bool,
    /// Short command name for TUI header (e.g. "task plan"). Falls back to truncated prompt.
    pub command_name: Option<String>,
    /// When set, passed as --allowedTools to restrict Claude's available tools.
    pub allowed_tools: Option<String>,
    /// When set, passed as --disallowedTools to explicitly block specific tools.
    pub disallowed_tools: Option<String>,
    /// When set, passed as --mcp-config to provide MCP servers to Claude.
    pub mcp_config: Option<serde_json::Value>,
    /// Optional receiver for ask_user questions from MCP server.
    pub question_rx: Option<mpsc::Receiver<QuestionEnvelope>>,
    /// Path to tasks.yml for auto-starting MCP server.
    pub tasks_path: Option<PathBuf>,
}

/// Run Claude CLI once with fullscreen TUI (App + TaskCommandApp).
///
/// Sets up OutputFormatter for event formatting, TaskCommandApp for fullscreen
/// rendering, and runs Claude in a tokio task while the TUI event loop runs
/// in a blocking thread.
///
/// When `tasks_path` is set and `mcp_config` is None, automatically starts
/// an McpServer and builds mcp_config from its port.
///
/// When `question_rx` is provided (or auto-created), a background task listens
/// for incoming QuestionEnvelopes and pushes them to TaskCommandApp's interactive
/// AskUserWidget.
pub(crate) async fn run_once(options: RunOnceOptions) -> Result<()> {
    // Destructure to separate question_rx (not Clone)
    let RunOnceOptions {
        prompt,
        model,
        output_dir,
        use_nerd_font,
        command_name,
        allowed_tools,
        disallowed_tools,
        mcp_config,
        question_rx,
        tasks_path,
    } = options;

    // ── Resolve MCP config ──
    let (resolved_mcp_config, resolved_question_rx, _server_guard) =
        resolve_mcp_config(mcp_config, question_rx, tasks_path.as_deref()).await?;

    // ── Shared state ──
    let shutdown = Arc::new(AtomicBool::new(false));
    // Use explicit command_name for header, or fall back to first 60 chars of prompt
    let header_name = command_name.unwrap_or_else(|| {
        let first_line = prompt.lines().next().unwrap_or(&prompt);
        if first_line.len() > 60 {
            format!("{}…", &first_line[..60])
        } else {
            first_line.to_string()
        }
    });
    let state = Arc::new(Mutex::new(TaskCommandApp::new(header_name)));
    let formatter = Arc::new(Mutex::new(OutputFormatter::new(use_nerd_font)));
    formatter.lock().unwrap().start_iteration();

    // ── Configure runner ──
    let mut runner = ClaudeRunner::oneshot(prompt, model, output_dir);
    if let Some(tools) = &allowed_tools {
        runner = runner.with_allowed_tools(tools.clone());
    }
    if let Some(tools) = &disallowed_tools {
        runner = runner.with_disallowed_tools(tools.clone());
    }
    if let Some(mcp_config) = resolved_mcp_config {
        runner = runner.with_mcp_config(mcp_config);
    }

    // ── Spawn question handler ──
    let question_handle =
        spawn_question_handler(resolved_question_rx, state.clone(), shutdown.clone());

    // ── Spawn Claude runner (async) ──
    let runner_state = state.clone();
    let runner_formatter = formatter.clone();
    let runner_shutdown = shutdown.clone();
    let runner_handle = tokio::spawn(async move {
        let event_state = runner_state.clone();
        let event_formatter = runner_formatter.clone();

        runner
            .run(
                runner_shutdown,
                move |event| {
                    let lines = {
                        let mut fmt = event_formatter.lock().unwrap();
                        fmt.format_event(event)
                    };
                    if !lines.is_empty() {
                        let mut app = event_state.lock().unwrap();
                        app.push_event(lines);
                    }
                },
                || {
                    // idle callback — TUI redraws on its own Tick, nothing to do
                },
            )
            .await
    });

    // ── Run TUI (blocking) ──
    let tui_state = state.clone();
    let tui_shutdown = shutdown.clone();
    let tui_handle = tokio::task::spawn_blocking(move || {
        let mut app = App::new(Duration::from_millis(100))?.with_shutdown(tui_shutdown);
        app.run_shared(tui_state)
    });

    // ── Wait for either TUI or runner to finish ──
    // Pin both handles so we can borrow them in select! and still await later.
    tokio::pin!(runner_handle);
    tokio::pin!(tui_handle);

    let result: Result<()> = tokio::select! {
        runner_result = &mut runner_handle => {
            // Runner finished — notify TUI but keep it open for browsing
            let runner_res = match runner_result {
                Ok(inner) => inner.map(|_| ()),
                Err(e) => Err(crate::shared::error::RalphError::ClaudeProcess(format!("Runner task panicked: {e}"))),
            };
            {
                let mut app = state.lock().unwrap();
                app.set_runner_completed();
            }
            // Wait for user to close TUI (q / Ctrl+C)
            let _ = tui_handle.await;
            shutdown.store(true, Ordering::SeqCst);
            runner_res
        }
        tui_result = &mut tui_handle => {
            // TUI exited (user pressed q or Ctrl+C) — signal runner to stop
            shutdown.store(true, Ordering::SeqCst);
            match tui_result {
                Ok(inner) => inner.map_err(Into::into),
                Err(e) => Err(crate::shared::error::RalphError::ClaudeProcess(format!("TUI task panicked: {e}"))),
            }
        }
    };

    // ── Cleanup ──
    // Drop server guard to close MCP server
    drop(_server_guard);

    // Wait for question handler to finish (with timeout)
    if let Some(handle) = question_handle {
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }

    result
}

/// Data returned by auto-starting an MCP server inside run_once().
struct AutoStartedServer {
    mcp_config: serde_json::Value,
    question_rx: mpsc::Receiver<QuestionEnvelope>,
    guard: ServerGuard,
}

/// RAII guard that cancels the MCP server on drop via CancellationToken.
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

/// Resolve MCP configuration from options or auto-start server.
async fn resolve_mcp_config(
    explicit_config: Option<serde_json::Value>,
    external_question_rx: Option<mpsc::Receiver<QuestionEnvelope>>,
    tasks_path: Option<&std::path::Path>,
) -> Result<(
    Option<serde_json::Value>,
    Option<mpsc::Receiver<QuestionEnvelope>>,
    Option<ServerGuard>,
)> {
    let (auto_mcp_config, auto_question_rx, server_guard) = match (tasks_path, &explicit_config) {
        (Some(tasks_path), None) => {
            let started = start_mcp_server(tasks_path.to_path_buf()).await?;
            (
                Some(started.mcp_config),
                Some(started.question_rx),
                Some(started.guard),
            )
        }
        _ => (None, None, None),
    };

    let mcp_config = explicit_config.or(auto_mcp_config);
    let question_rx = external_question_rx.or(auto_question_rx);

    Ok((mcp_config, question_rx, server_guard))
}

/// Spawns a background task that listens for QuestionEnvelopes from MCP server.
///
/// For each envelope, creates a oneshot channel, pushes questions to
/// TaskCommandApp via `start_ask_user()`, and waits for answers from the
/// TUI event loop. Answers are then sent back via the envelope's response channel.
fn spawn_question_handler(
    question_rx: Option<mpsc::Receiver<QuestionEnvelope>>,
    state: Arc<Mutex<TaskCommandApp>>,
    shutdown: Arc<AtomicBool>,
) -> Option<tokio::task::JoinHandle<()>> {
    let mut question_rx = question_rx?;

    Some(tokio::spawn(async move {
        while let Some(envelope) = question_rx.recv().await {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }

            let (answer_tx, answer_rx) = tokio::sync::oneshot::channel::<Vec<Answer>>();

            // Push questions to TUI
            {
                let mut app = state.lock().unwrap();
                app.start_ask_user(envelope.questions, answer_tx);
            }

            // Wait for answers from TUI event loop (user interaction)
            match answer_rx.await {
                Ok(answers) => {
                    let _ = envelope.response_tx.send(answers);
                }
                Err(_) => {
                    // Widget was cancelled or app shut down — send empty answers
                    let _ = envelope.response_tx.send(Vec::new());
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::mcp::ask_user::{Question, QuestionType};
    use tokio::sync::oneshot;

    #[test]
    fn test_run_once_options_defaults() {
        let _opts = RunOnceOptions {
            prompt: "test".into(),
            model: None,
            output_dir: None,
            use_nerd_font: false,
            command_name: None,
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
            command_name: None,
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
            command_name: None,
            allowed_tools: None,
            disallowed_tools: None,
            mcp_config: None,
            question_rx: None,
            tasks_path: Some(PathBuf::from(".ralph/tasks.yml")),
        };
    }

    #[test]
    fn test_run_once_options_custom_mcp_overrides_tasks_path() {
        let custom_config = serde_json::json!({"mcpServers": {}});
        let _opts = RunOnceOptions {
            prompt: "test".into(),
            model: None,
            output_dir: None,
            use_nerd_font: false,
            command_name: None,
            allowed_tools: None,
            disallowed_tools: None,
            mcp_config: Some(custom_config),
            question_rx: None,
            tasks_path: Some(PathBuf::from(".ralph/tasks.yml")),
        };
    }

    #[test]
    fn test_run_once_options_with_disallowed_tools() {
        let _opts = RunOnceOptions {
            prompt: "test".into(),
            model: None,
            output_dir: None,
            use_nerd_font: false,
            command_name: None,
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
        let tasks_path = PathBuf::from("/tmp/test_tasks.yml");
        let server = start_mcp_server(tasks_path).await.unwrap();

        let url = server.mcp_config["mcpServers"]["ralph-tasks"]["url"]
            .as_str()
            .unwrap()
            .to_string();

        let _mcp_config = server.mcp_config;
        let _question_rx = server.question_rx;
        let guard = server.guard;

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

        let url = server.mcp_config["mcpServers"]["ralph-tasks"]["url"]
            .as_str()
            .unwrap();
        let port_str = url
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches("/mcp");
        let port: u16 = port_str.parse().unwrap();
        assert!(port > 0);

        let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await;
        assert!(result.is_ok(), "Should connect to auto-started server");
    }

    #[tokio::test]
    async fn test_question_handler_shutdown_drops_envelope() {
        let (tx, rx) = mpsc::channel::<QuestionEnvelope>(1);
        let shutdown = Arc::new(AtomicBool::new(true)); // Already shut down
        let state = Arc::new(Mutex::new(TaskCommandApp::new("test")));

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

        tx.send(envelope).await.unwrap();
        drop(tx);

        let handle = spawn_question_handler(Some(rx), state, shutdown);
        if let Some(h) = handle {
            h.await.unwrap();
        }

        // Handler broke out of loop without answering — sender dropped
        assert!(response_rx.await.is_err());
    }

    #[tokio::test]
    async fn test_question_handler_delivers_answers() {
        let (tx, rx) = mpsc::channel::<QuestionEnvelope>(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(TaskCommandApp::new("test")));
        let state_clone = state.clone();

        let handle = spawn_question_handler(Some(rx), state, shutdown.clone());

        let (response_tx, response_rx) = oneshot::channel::<Vec<Answer>>();
        let envelope = QuestionEnvelope {
            questions: vec![Question {
                question: "Name?".into(),
                question_type: QuestionType::Text,
                options: vec![],
                default: None,
                placeholder: None,
                required: true,
            }],
            response_tx,
        };

        tx.send(envelope).await.unwrap();

        // Give handler time to process and push to state
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Simulate user answering through TaskCommandApp
        {
            let mut app = state_clone.lock().unwrap();
            assert!(app.has_active_question());
            app.advance_question("Alice".to_string());
        }

        // Wait for answer from handler
        let answers = response_rx.await.unwrap();
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].answer, "Alice");

        shutdown.store(true, Ordering::SeqCst);
        drop(tx);
        if let Some(h) = handle {
            let _ = tokio::time::timeout(Duration::from_secs(1), h).await;
        }
    }

    #[tokio::test]
    async fn test_server_guard_releases_port_on_drop() {
        let tasks_path = PathBuf::from("/tmp/test_tasks.yml");
        let server = start_mcp_server(tasks_path.clone()).await.unwrap();

        let url = server.mcp_config["mcpServers"]["ralph-tasks"]["url"]
            .as_str()
            .unwrap()
            .to_string();
        let port_str = url
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches("/mcp");
        let port: u16 = port_str.parse().unwrap();

        let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await;
        assert!(result.is_ok(), "Server should be reachable before drop");

        drop(server.guard);
        tokio::time::sleep(Duration::from_millis(100)).await;

        let server2 = start_mcp_server(tasks_path).await;
        assert!(
            server2.is_ok(),
            "Should be able to start new server after dropping guard"
        );
    }

    #[tokio::test]
    async fn test_multiple_server_guards_independent_cancellation() {
        let (tx1, _rx1) = mpsc::channel(1);
        let (tx2, _rx2) = mpsc::channel(1);
        let token1 = CancellationToken::new();
        let token2 = CancellationToken::new();

        let server1 = McpServer::new(PathBuf::from("/tmp/t1.yml"), tx1, token1.clone());
        let server2 = McpServer::new(PathBuf::from("/tmp/t2.yml"), tx2, token2.clone());

        let (_port1, _handle1) = server1.start().await.unwrap();
        let (_port2, _handle2) = server2.start().await.unwrap();

        token1.cancel();
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(token1.is_cancelled());
        assert!(!token2.is_cancelled());

        token2.cancel();
    }

    #[tokio::test]
    async fn test_resolve_mcp_config_explicit_takes_precedence() {
        let explicit = serde_json::json!({"mcpServers": {"custom": {}}});
        let (config, _rx, _guard) = resolve_mcp_config(
            Some(explicit.clone()),
            None,
            Some(std::path::Path::new("/tmp/t.yml")),
        )
        .await
        .unwrap();

        assert_eq!(config.unwrap(), explicit);
        assert!(
            _guard.is_none(),
            "Should not auto-start server when explicit config provided"
        );
    }

    #[tokio::test]
    async fn test_resolve_mcp_config_auto_starts_server() {
        let (config, rx, guard) =
            resolve_mcp_config(None, None, Some(std::path::Path::new("/tmp/t.yml")))
                .await
                .unwrap();

        assert!(config.is_some());
        assert!(rx.is_some());
        assert!(guard.is_some());
    }

    #[tokio::test]
    async fn test_resolve_mcp_config_none_when_no_tasks() {
        let (config, rx, guard) = resolve_mcp_config(None, None, None).await.unwrap();

        assert!(config.is_none());
        assert!(rx.is_none());
        assert!(guard.is_none());
    }

    #[test]
    fn test_spawn_question_handler_none_returns_none() {
        let state = Arc::new(Mutex::new(TaskCommandApp::new("test")));
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle = spawn_question_handler(None, state, shutdown);
        assert!(handle.is_none());
    }
}
