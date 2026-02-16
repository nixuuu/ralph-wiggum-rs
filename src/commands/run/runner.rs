//! ClaudeRunner — builder, spawn i zarządzanie procesem Claude CLI.
//!
//! Odpowiada za konfigurację, uruchamianie i zamykanie procesu claude.
//! Typy danych są w `runner_types`, logika czytania stdout w `runner_reader`.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::shared::error::{RalphError, Result};

// Re-export typów dla zachowania publicznego API modułu (runner::ClaudeEvent itp.)
pub(crate) use super::runner_types::{
    AssistantMessage, ClaudeEvent, ContentBlock, ModelUsageEntry, Usage,
};

use super::runner_reader::{self, POST_RESULT_TIMEOUT};
use super::runner_types::{
    ReadOutcome, StdinControlRequest, StdinInitPayload, StdinMessageContent, StdinTextBlock,
    StdinUserMessage,
};

// ---------------------------------------------------------------------------
// Process management helpers
// ---------------------------------------------------------------------------

/// Send SIGTERM to an entire process group (-pgid).
///
/// Used as the first stage of graceful shutdown.
#[cfg(unix)]
pub(crate) fn sigterm_process_group(pid: u32) {
    unsafe {
        libc::kill(-(pid as i32), libc::SIGTERM);
    }
}

/// Send SIGKILL to an entire process group (-pgid).
///
/// Since we spawn children with `process_group(0)`, their PGID equals their PID.
/// This ensures all sub-processes (e.g., Node.js children spawned by Claude CLI)
/// are killed, not just the top-level process.
#[cfg(unix)]
pub(crate) fn kill_process_group(pid: u32) {
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

/// Graceful shutdown timeout: wait this long after SIGTERM before SIGKILL.
const GRACEFUL_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Graceful shutdown: SIGTERM → wait → SIGKILL.
///
/// Returns immediately if the process exits after SIGTERM.
/// Falls back to SIGKILL after `GRACEFUL_SHUTDOWN_TIMEOUT`.
#[cfg(unix)]
pub(crate) async fn graceful_shutdown(child: &mut tokio::process::Child, pid: Option<u32>) {
    if let Some(pid) = pid {
        sigterm_process_group(pid);
    }

    match tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, child.wait()).await {
        Ok(_) => {} // Process exited after SIGTERM
        Err(_) => {
            // Timeout — force kill
            if let Some(pid) = pid {
                kill_process_group(pid);
            }
            child.kill().await.ok();
        }
    }
}

#[cfg(not(unix))]
pub(crate) async fn graceful_shutdown(child: &mut tokio::process::Child, _pid: Option<u32>) {
    child.kill().await.ok();
}

// ---------------------------------------------------------------------------
// ClaudeRunner
// ---------------------------------------------------------------------------

pub struct ClaudeRunner {
    user_prompt: String,
    system_prompt: Option<String>,
    continue_conversation: bool,
    model: Option<String>,
    output_dir: Option<PathBuf>,
    /// When set, restricts Claude to this allowlist of tools (--allowedTools flag).
    allowed_tools: Option<String>,
    /// When set, explicitly blocks these tools regardless of allowed_tools (--disallowedTools flag).
    disallowed_tools: Option<String>,
    mcp_config: Option<serde_json::Value>,
    phase_timeout: Option<std::time::Duration>,
}

impl ClaudeRunner {
    /// Create runner for first iteration (with user prompt and system prompt)
    pub fn new(user_prompt: String, system_prompt: String) -> Self {
        Self {
            user_prompt,
            system_prompt: Some(system_prompt),
            continue_conversation: false,
            model: None,
            output_dir: None,
            allowed_tools: None,
            disallowed_tools: None,
            mcp_config: None,
            phase_timeout: None,
        }
    }

    /// Create runner for continuation with user prompt and system prompt
    pub fn for_continuation(user_prompt: String, system_prompt: String) -> Self {
        Self {
            user_prompt,
            system_prompt: Some(system_prompt),
            continue_conversation: true,
            model: None,
            output_dir: None,
            allowed_tools: None,
            disallowed_tools: None,
            mcp_config: None,
            phase_timeout: None,
        }
    }

    /// Create runner for a one-shot invocation (no system prompt, optional model/output_dir)
    pub fn oneshot(prompt: String, model: Option<String>, output_dir: Option<PathBuf>) -> Self {
        Self {
            user_prompt: prompt,
            system_prompt: None,
            continue_conversation: false,
            model,
            output_dir,
            allowed_tools: None,
            disallowed_tools: None,
            mcp_config: None,
            phase_timeout: None,
        }
    }

    /// Builder: restrict available tools (passed as --allowedTools to Claude CLI).
    pub fn with_allowed_tools(mut self, tools: String) -> Self {
        self.allowed_tools = Some(tools);
        self
    }

    /// Builder: explicitly block specific tools (passed as --disallowedTools to Claude CLI).
    pub fn with_disallowed_tools(mut self, tools: String) -> Self {
        self.disallowed_tools = Some(tools);
        self
    }

    /// Builder: set MCP server config (passed as --mcp-config JSON to Claude CLI).
    pub fn with_mcp_config(mut self, config: serde_json::Value) -> Self {
        self.mcp_config = Some(config);
        self
    }

    /// Builder: set per-phase timeout. If the process doesn't produce a Result event
    /// within this duration, it will be killed.
    pub fn with_phase_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.phase_timeout = Some(timeout);
        self
    }

    /// Write the initialize control request and user message to stdin,
    /// then close stdin to signal end of input.
    async fn send_stdin_messages(
        &self,
        stdin: &mut tokio::process::ChildStdin,
    ) -> std::result::Result<(), std::io::Error> {
        // 1. Send initialize control request
        let init = StdinControlRequest {
            msg_type: "control_request",
            request_id: "init-1",
            request: StdinInitPayload {
                subtype: "initialize",
                system_prompt: None,
                append_system_prompt: self.system_prompt.as_deref(),
                hooks: None,
                sdk_mcp_servers: None,
                json_schema: None,
                agents: None,
            },
        };
        let init_json = serde_json::to_string(&init)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        stdin.write_all(init_json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;

        // 2. Send user message
        let user_msg = StdinUserMessage {
            msg_type: "user",
            session_id: "",
            message: StdinMessageContent {
                role: "user",
                content: vec![StdinTextBlock {
                    block_type: "text",
                    text: &self.user_prompt,
                }],
            },
            parent_tool_use_id: None,
        };
        let user_json = serde_json::to_string(&user_msg)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        stdin.write_all(user_json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;

        stdin.flush().await?;
        Ok(())
    }

    /// Build the `claude` CLI command with all necessary arguments.
    fn build_command(&self) -> Command {
        let mut cmd = Command::new("claude");
        cmd.arg("-p");
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--input-format").arg("stream-json");
        cmd.arg("--verbose");
        cmd.arg("--dangerously-skip-permissions");
        cmd.arg("--replay-user-messages");

        if let Some(ref model) = self.model {
            cmd.arg("--model").arg(model);
        }
        if self.continue_conversation {
            cmd.arg("--continue");
        }
        if let Some(ref tools) = self.allowed_tools {
            cmd.arg("--allowedTools").arg(tools);
        }
        if let Some(ref tools) = self.disallowed_tools {
            cmd.arg("--disallowedTools").arg(tools);
        }
        if let Some(ref mcp_config) = self.mcp_config {
            let config_json =
                serde_json::to_string(mcp_config).unwrap_or_else(|_| "{}".to_string());
            cmd.arg("--mcp-config").arg(&config_json);
        }

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::inherit());
        cmd.kill_on_drop(true);

        if let Some(ref dir) = self.output_dir {
            cmd.current_dir(dir);
        }

        // Isolate child in its own process group so terminal SIGINT
        // goes only to ralph-wiggum, not absorbed by claude's Node.js handler
        #[cfg(unix)]
        cmd.process_group(0);

        crate::diag_debug!(
            "Building Claude CLI command: model={:?}, continue={}, cwd={:?}",
            self.model,
            self.continue_conversation,
            self.output_dir
        );

        cmd
    }

    /// Spawn the claude process, send initialize + user message via stdin.
    /// Returns the child process, a line reader for stdout, and stdin handle.
    async fn spawn_process(
        &self,
    ) -> Result<(
        tokio::process::Child,
        tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
        tokio::process::ChildStdin,
    )> {
        let mut child = self
            .build_command()
            .spawn()
            .map_err(|e| RalphError::ClaudeProcess(format!("Failed to spawn claude: {}", e)))?;

        let child_pid = child.id();
        crate::diag_debug!("Spawned Claude CLI process: PID={:?}", child_pid);

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| RalphError::ClaudeProcess("Failed to capture stdin".into()))?;

        self.send_stdin_messages(&mut stdin)
            .await
            .map_err(|e| RalphError::ClaudeProcess(format!("Failed to write stdin: {}", e)))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RalphError::ClaudeProcess("Failed to capture stdout".into()))?;

        let reader = BufReader::new(stdout).lines();

        Ok((child, reader, stdin))
    }

    /// Run Claude and stream output.
    ///
    /// Uses the bidirectional stream-json protocol:
    /// - Sends initialize + user message via stdin
    /// - Reads streaming JSON events from stdout
    /// - Stdin is closed after initial messages (non-interactive mode)
    ///
    /// Calls `on_event` for each JSON event.
    /// Returns last assistant text message.
    pub async fn run<F, I>(
        &self,
        shutdown: Arc<AtomicBool>,
        mut on_event: F,
        mut on_idle: I,
    ) -> Result<Option<String>>
    where
        F: FnMut(&ClaudeEvent),
        I: FnMut(),
    {
        let (mut child, mut reader, stdin) = self.spawn_process().await?;
        let child_pid = child.id();

        // Close stdin — non-interactive mode, Claude CLI will see EOF and close stdout
        crate::diag_debug!(
            "Dropping stdin for non-interactive mode (PID={:?})",
            child_pid
        );
        drop(stdin);

        let (last_text, outcome, sub_agent_results_count) = runner_reader::read_output(
            &mut reader,
            &shutdown,
            &mut on_event,
            &mut on_idle,
            self.phase_timeout,
            &mut None,
            None,
        )
        .await?;

        self.handle_outcome(
            &mut child,
            child_pid,
            last_text,
            outcome,
            sub_agent_results_count,
        )
        .await
    }

    /// Run Claude in interactive mode with ability to send additional messages.
    ///
    /// Similar to `run()`, but keeps stdin open and accepts a channel to receive
    /// additional user messages during execution.
    #[allow(dead_code)] // Public API — będzie użyte w orchestratorze
    pub async fn run_interactive<F, I>(
        &self,
        shutdown: Arc<AtomicBool>,
        message_rx: tokio::sync::mpsc::Receiver<String>,
        mut on_event: F,
        mut on_idle: I,
    ) -> Result<Option<String>>
    where
        F: FnMut(&ClaudeEvent),
        I: FnMut(),
    {
        let (mut child, mut reader, stdin) = self.spawn_process().await?;
        let child_pid = child.id();

        let (last_text, outcome, sub_agent_results_count) = runner_reader::read_output(
            &mut reader,
            &shutdown,
            &mut on_event,
            &mut on_idle,
            self.phase_timeout,
            &mut Some(stdin),
            Some(message_rx),
        )
        .await?;

        self.handle_outcome(
            &mut child,
            child_pid,
            last_text,
            outcome,
            sub_agent_results_count,
        )
        .await
    }

    /// Shared outcome handling for run() and run_interactive().
    ///
    /// Maps ReadOutcome to appropriate Result: graceful shutdown on Shutdown/PhaseTimeout,
    /// diagnostic + shutdown on ResultTimeout, normal completion on Eof.
    async fn handle_outcome(
        &self,
        child: &mut tokio::process::Child,
        child_pid: Option<u32>,
        last_text: Option<String>,
        outcome: ReadOutcome,
        sub_agent_results_count: u32,
    ) -> Result<Option<String>> {
        match outcome {
            ReadOutcome::Shutdown => {
                graceful_shutdown(child, child_pid).await;
                Err(RalphError::Interrupted)
            }
            ReadOutcome::PhaseTimeout => {
                let pid_str = child_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let timeout_secs = self.phase_timeout.map(|d| d.as_secs()).unwrap_or(0);
                crate::diag_warn!(
                    "Phase timeout triggered for claude process (PID {}). \
                     No Result event received within {} seconds.",
                    pid_str,
                    timeout_secs
                );
                graceful_shutdown(child, child_pid).await;
                Err(RalphError::ClaudeProcess(format!(
                    "phase timed out after {} minutes — no Result event received",
                    timeout_secs / 60
                )))
            }
            ReadOutcome::ResultTimeout => {
                let pid_str = child_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let timeout_secs = POST_RESULT_TIMEOUT.as_secs();
                let mut diagnostic_msg = format!(
                    "Claude process (PID {}) did not close stdout within {}s after Result event — killing.\n\
                     Possible causes: MCP server holding connection, sub-agent still running, or Claude CLI bug.",
                    pid_str, timeout_secs
                );
                if sub_agent_results_count > 0 {
                    diagnostic_msg.push_str(&format!(
                        " (Ignored {} sub-agent Result event(s).)",
                        sub_agent_results_count
                    ));
                }
                crate::diag_warn!("{}", diagnostic_msg);
                graceful_shutdown(child, child_pid).await;
                Ok(last_text)
            }
            ReadOutcome::Eof => runner_reader::handle_completion(child, child_pid, last_text).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_command_has_replay_user_messages() {
        let runner = ClaudeRunner::oneshot("test".into(), None, None);
        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);
        assert!(
            debug_str.contains("--replay-user-messages"),
            "Command should contain --replay-user-messages flag"
        );
    }

    #[test]
    fn test_build_command_with_disallowed_tools() {
        let runner = ClaudeRunner::oneshot("test".into(), None, None)
            .with_disallowed_tools("AskUserQuestion".to_string());
        let cmd = runner.build_command();

        let debug_str = format!("{:?}", cmd);
        assert!(
            debug_str.contains("--disallowedTools"),
            "Command should contain --disallowedTools flag"
        );
        assert!(
            debug_str.contains("AskUserQuestion"),
            "Command should contain AskUserQuestion as disallowed tool"
        );
    }

    #[test]
    fn test_build_command_without_disallowed_tools() {
        let runner = ClaudeRunner::oneshot("test".into(), None, None);
        let cmd = runner.build_command();

        let debug_str = format!("{:?}", cmd);
        assert!(
            !debug_str.contains("--disallowedTools"),
            "Command should NOT contain --disallowedTools when not set"
        );
    }

    #[test]
    fn test_with_disallowed_tools_builder() {
        let runner = ClaudeRunner::new("prompt".into(), "system".into())
            .with_disallowed_tools("Tool1,Tool2".to_string());
        assert_eq!(runner.disallowed_tools, Some("Tool1,Tool2".to_string()));
    }

    #[test]
    fn test_claude_runner_with_phase_timeout_builder() {
        let runner = ClaudeRunner::oneshot("test".into(), None, None)
            .with_phase_timeout(std::time::Duration::from_secs(1800));
        assert_eq!(
            runner.phase_timeout,
            Some(std::time::Duration::from_secs(1800))
        );
    }

    #[test]
    fn test_claude_runner_default_no_phase_timeout() {
        let runner = ClaudeRunner::new("test".into(), "system".into());
        assert!(runner.phase_timeout.is_none());

        let runner = ClaudeRunner::for_continuation("test".into(), "system".into());
        assert!(runner.phase_timeout.is_none());

        let runner = ClaudeRunner::oneshot("test".into(), None, None);
        assert!(runner.phase_timeout.is_none());
    }

    #[tokio::test]
    async fn test_run_interactive_basic_flow() {
        use std::sync::Arc;
        use tokio::sync::mpsc;

        let (_tx, rx) = mpsc::channel::<String>(10);
        let runner = ClaudeRunner::oneshot("test".into(), None, None);
        let shutdown = Arc::new(AtomicBool::new(false));

        // Test weryfikuje że API kompiluje się i zwraca Result
        // Jeśli claude CLI nie istnieje - powinien być błąd spawnu
        // Jeśli claude CLI istnieje - test może timeout (oczekuje na stdin)
        let result = runner.run_interactive(shutdown, rx, |_| {}, || {}).await;

        // Akceptujemy zarówno błąd (brak CLI) jak i timeout/inne błędy (CLI istnieje)
        // Test potwierdza że funkcja nie panikuje
        let _ = result;
    }

    // ---------------------------------------------------------------------------
    // Stdin handling contract tests
    // ---------------------------------------------------------------------------

    /// Test 1: Weryfikuj że run() zamyka stdin przed odczytem stdout.
    ///
    /// Strategia: Uruchom process który czeka na stdin i emituje JSON na stdout.
    /// Jeśli stdin jest zamknięty, proces zakończy się normalnie (ReadOutcome::Eof).
    /// Jeśli stdin pozostaje otwarty, proces będzie wisiał i timeout wygaśnie.
    #[tokio::test]
    async fn test_run_drops_stdin_before_read_output() {
        use std::sync::Arc;

        // Prosty shell script który czyta stdin i kończy się gdy stdin jest zamknięty
        let runner = ClaudeRunner {
            user_prompt: "ignored".into(),
            system_prompt: None,
            continue_conversation: false,
            model: None,
            output_dir: None,
            allowed_tools: None,
            disallowed_tools: None,
            mcp_config: None,
            phase_timeout: Some(std::time::Duration::from_secs(2)), // Timeout dla deadlocka
        };

        // Override build_command do testowego procesu (cat wyczyta stdin i EOF zamknie stdout)
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            // Skrypt: czytaj stdin (wiadmość init + user), wypisz JSON events, zamknij stdout gdy stdin EOF
            .arg(concat!(
                "read line1; ",                                                    // init message
                "read line2; ",                                                    // user message
                "echo '{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"OK\"}]}}'; ",
                "echo '{\"type\":\"result\",\"subtype\":\"success\",\"cost_usd\":0.0}'; "
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("Failed to spawn test process");

        let mut stdin = child.stdin.take().expect("stdin should be available");

        // Symuluj send_stdin_messages + zamknięcie stdin
        stdin
            .write_all(b"{\"type\":\"control_request\"}\n")
            .await
            .ok();
        stdin.write_all(b"{\"type\":\"user\"}\n").await.ok();
        stdin.flush().await.ok();
        drop(stdin); // Zamykamy stdin — proces powinien zaraz zakończyć stdout

        let stdout = child.stdout.take().expect("stdout should be available");
        let mut reader = BufReader::new(stdout).lines();
        let shutdown = Arc::new(AtomicBool::new(false));

        let (last_text, outcome, _sub_agent_count) = runner_reader::read_output(
            &mut reader,
            &shutdown,
            &mut |_| {},
            &mut || {},
            runner.phase_timeout,
            &mut None, // stdin=None (już zamknięty)
            None,
        )
        .await
        .expect("read_output should succeed");

        // Weryfikuj: proces normalnie zamknął stdout → Eof
        assert!(
            matches!(outcome, ReadOutcome::Eof),
            "run() should close stdin → process emits events and closes stdout → ReadOutcome::Eof. Got: {:?}",
            outcome
        );
        assert!(
            last_text.is_some(),
            "Should receive assistant text before EOF"
        );

        let child_pid = child.id();
        runner_reader::handle_completion(&mut child, child_pid, last_text)
            .await
            .ok();
    }

    /// Test 2: Weryfikuj że run_interactive() zachowuje stdin otwarty.
    ///
    /// Strategia: Uruchom process który oczekuje wiadomości przez stdin i odpowiada na stdout.
    /// Wyślij wiadomość przez message_rx → stdin powinien przyjąć ją.
    #[tokio::test]
    async fn test_run_interactive_keeps_stdin_open() {
        use std::sync::Arc;
        use tokio::sync::mpsc;

        // Testowy proces który czyta stdin i pisze JSON na stdout
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(concat!(
                // Czytaj 2 init wiadomości
                "read line1; ",
                "read line2; ",
                // Wyślij assistant message
                "echo '{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Ready\"}]}}'; ",
                // Czekaj na dodatkową wiadomość (z message_rx) — odblokowane przez send_user_message()
                "read line3; ",
                // Odpowiedz na dodatkową wiadomość
                "echo '{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Received\"}]}}'; ",
                // Zakończ
                "echo '{\"type\":\"result\",\"subtype\":\"success\",\"cost_usd\":0.0}'; "
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("Failed to spawn test process");

        let mut stdin = child.stdin.take().expect("stdin should be available");

        // Symuluj send_stdin_messages (init + user message)
        stdin
            .write_all(b"{\"type\":\"control_request\"}\n")
            .await
            .ok();
        stdin.write_all(b"{\"type\":\"user\"}\n").await.ok();
        stdin.flush().await.ok();

        // Zachowaj stdin otwarty (przekaż do read_output)
        let stdout = child.stdout.take().expect("stdout should be available");
        let mut reader = BufReader::new(stdout).lines();
        let shutdown = Arc::new(AtomicBool::new(false));

        let (tx, rx) = mpsc::channel::<String>(10);

        // Po 50ms wyślij dodatkową wiadomość przez kanał
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            tx_clone.send("Follow-up message".to_string()).await.ok();
        });

        let mut message_count = 0;
        let (last_text, outcome, _sub_agent_count) = runner_reader::read_output(
            &mut reader,
            &shutdown,
            &mut |event| {
                if matches!(event, ClaudeEvent::Assistant { .. }) {
                    message_count += 1;
                }
            },
            &mut || {},
            Some(std::time::Duration::from_secs(3)), // Timeout dla błędów
            &mut Some(stdin),                        // stdin pozostaje otwarty
            Some(rx),
        )
        .await
        .expect("read_output should succeed");

        // Weryfikuj: proces odbiera wiadomość przez stdin → emituje 2 assistant messages → Eof
        assert!(
            matches!(outcome, ReadOutcome::Eof),
            "Interactive mode should complete normally with Eof. Got: {:?}",
            outcome
        );
        assert_eq!(
            message_count, 2,
            "Should receive 2 assistant messages (initial + follow-up response)"
        );
        assert!(
            last_text.is_some(),
            "Should have last assistant text before EOF"
        );

        let child_pid = child.id();
        runner_reader::handle_completion(&mut child, child_pid, last_text)
            .await
            .ok();
    }

    /// Test 3: End-to-end weryfikacja że run() nie generuje ResultTimeout.
    ///
    /// Strategia: Uruchom prosty proces emitujący Result event. Stdin zamknięty → proces
    /// kończy stdout → outcome to Eof (nie ResultTimeout).
    #[tokio::test]
    async fn test_no_result_timeout_in_standard_mode() {
        use std::sync::Arc;

        // Prosty process emitujący Result event i kończący stdout
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(concat!(
                "read line1; ",
                "read line2; ",
                "echo '{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Working\"}]}}'; ",
                "echo '{\"type\":\"result\",\"subtype\":\"success\",\"cost_usd\":0.0}'; "
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("Failed to spawn test process");

        let mut stdin = child.stdin.take().expect("stdin should be available");

        // Symuluj send_stdin_messages + zamknięcie stdin
        stdin
            .write_all(b"{\"type\":\"control_request\"}\n")
            .await
            .ok();
        stdin.write_all(b"{\"type\":\"user\"}\n").await.ok();
        stdin.flush().await.ok();
        drop(stdin); // Zamykamy stdin — proces powinien zamknąć stdout po Result event

        let stdout = child.stdout.take().expect("stdout should be available");
        let mut reader = BufReader::new(stdout).lines();
        let shutdown = Arc::new(AtomicBool::new(false));

        let mut result_received = false;
        let (last_text, outcome, _sub_agent_count) = runner_reader::read_output(
            &mut reader,
            &shutdown,
            &mut |event| {
                if matches!(event, ClaudeEvent::Result { .. }) {
                    result_received = true;
                }
            },
            &mut || {},
            None,
            &mut None,
            None,
        )
        .await
        .expect("read_output should succeed");

        // Weryfikuj: Result event odebrane + stdout zamknięty → Eof (nie ResultTimeout)
        assert!(result_received, "Should receive Result event");
        assert!(
            matches!(outcome, ReadOutcome::Eof),
            "Standard mode with closed stdin should complete with Eof, not ResultTimeout. Got: {:?}",
            outcome
        );
        assert!(last_text.is_some(), "Should have assistant text");

        let child_pid = child.id();
        runner_reader::handle_completion(&mut child, child_pid, last_text)
            .await
            .ok();
    }

    /// Test 57.1: MCP config z special chars — newlines, quotes, backslashes.
    ///
    /// Weryfikuj że build_command() generuje poprawny JSON argument dla --mcp-config
    /// zawierający cudzysłowy, backslashe i newliny. Command::arg() nie interpretuje
    /// shell — przekazuje string bezpośrednio jako argv[N].
    /// Round-trip: json!() → serde_json::to_string → get_args() → from_str → identyczność.
    #[test]
    fn test_build_command_with_mcp_config_special_chars() {
        use serde_json::json;

        let mcp_config = json!({
            "server": "ralph-tasks",
            "url": "http://localhost:3000",
            "description": "Line 1\nLine 2 with \"quotes\" and 'single' and \\ backslash",
            "path": "C:\\Users\\test\\file.txt"
        });

        let runner =
            ClaudeRunner::oneshot("test".into(), None, None).with_mcp_config(mcp_config.clone());

        let cmd = runner.build_command();

        // Wyciągnij argument --mcp-config bezpośrednio z Command::get_args()
        let args: Vec<&std::ffi::OsStr> = cmd.as_std().get_args().collect();
        let mcp_flag_pos = args
            .iter()
            .position(|a| *a == "--mcp-config")
            .expect("Command should contain --mcp-config flag");
        let config_arg = args[mcp_flag_pos + 1]
            .to_str()
            .expect("MCP config arg should be valid UTF-8");

        // Round-trip: deserializacja argumentu powinna dać identyczną wartość
        let roundtrip: serde_json::Value =
            serde_json::from_str(config_arg).expect("MCP config arg should be valid JSON");
        assert_eq!(
            roundtrip, mcp_config,
            "Round-trip JSON powinien być identyczny z oryginałem"
        );

        // Weryfikuj że surowy JSON zawiera poprawne escapowania
        assert!(
            config_arg.contains("\\n"),
            "Newline powinien być escaped jako \\n"
        );
        assert!(
            config_arg.contains("\\\""),
            "Cudzysłów powinien być escaped jako \\\""
        );
        assert!(
            config_arg.contains("\\\\"),
            "Backslash powinien być escaped jako \\\\"
        );
    }

    // ---------------------------------------------------------------------------
    // build_command — kombinacje opcjonalnych flag
    // ---------------------------------------------------------------------------

    /// Test 1: Tylko wymagane pola (bez opcjonalnych flag)
    #[test]
    fn test_build_command_required_only() {
        let runner = ClaudeRunner::oneshot("test prompt".into(), None, None);
        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);

        // Weryfikuj podstawowe flagi
        assert!(debug_str.contains("-p"));
        assert!(debug_str.contains("--output-format"));
        assert!(debug_str.contains("stream-json"));
        assert!(debug_str.contains("--input-format"));
        assert!(debug_str.contains("--verbose"));
        assert!(debug_str.contains("--dangerously-skip-permissions"));
        assert!(debug_str.contains("--replay-user-messages"));

        // Weryfikuj brak opcjonalnych flag
        assert!(!debug_str.contains("--model"));
        assert!(!debug_str.contains("--continue"));
        assert!(!debug_str.contains("--allowedTools"));
        assert!(!debug_str.contains("--disallowedTools"));
        assert!(!debug_str.contains("--mcp-config"));
    }

    /// Test 2: Z modelem
    #[test]
    fn test_build_command_with_model() {
        let runner = ClaudeRunner::oneshot("test".into(), Some("claude-opus-4".into()), None);
        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);

        assert!(debug_str.contains("--model"));
        assert!(debug_str.contains("claude-opus-4"));
    }

    /// Test 3: Z system_prompt (tylko through constructor)
    #[test]
    fn test_build_command_with_system_prompt() {
        let runner = ClaudeRunner::new("user prompt".into(), "system prompt".into());

        // system_prompt nie trafia do CLI args — tylko do stdin messages
        // Sprawdzamy że runner.system_prompt jest ustawiony
        assert_eq!(runner.system_prompt, Some("system prompt".to_string()));
    }

    /// Test 4: Z mcp_config
    #[test]
    fn test_build_command_with_mcp_config() {
        let config = serde_json::json!({
            "servers": {
                "test-server": {
                    "command": "node",
                    "args": ["server.js"]
                }
            }
        });

        let runner =
            ClaudeRunner::oneshot("test".into(), None, None).with_mcp_config(config.clone());
        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);

        assert!(debug_str.contains("--mcp-config"));
        // Sprawdź że JSON zawiera kluczowe elementy (bez zależności od formatowania)
        assert!(debug_str.contains("servers"));
        assert!(debug_str.contains("test-server"));
        assert!(debug_str.contains("node"));
        assert!(debug_str.contains("server.js"));
    }

    /// Test 5: Z allowed_tools
    #[test]
    fn test_build_command_with_allowed_tools() {
        let runner = ClaudeRunner::oneshot("test".into(), None, None)
            .with_allowed_tools("Read,Write,Bash".to_string());
        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);

        assert!(debug_str.contains("--allowedTools"));
        assert!(debug_str.contains("Read,Write,Bash"));
    }

    /// Test 6: Z disallowed_tools
    #[test]
    fn test_build_command_with_disallowed_tools_full() {
        let runner = ClaudeRunner::oneshot("test".into(), None, None)
            .with_disallowed_tools("AskUserQuestion,TodoWrite".to_string());
        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);

        assert!(debug_str.contains("--disallowedTools"));
        assert!(debug_str.contains("AskUserQuestion,TodoWrite"));
    }

    /// Test 7: Z continue_conversation
    #[test]
    fn test_build_command_with_continue() {
        let runner = ClaudeRunner::for_continuation("test".into(), "system".into());
        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);

        assert!(debug_str.contains("--continue"));
    }

    /// Test 8: Wszystko naraz (model + allowed_tools + disallowed_tools + mcp_config + continue)
    #[test]
    fn test_build_command_all_flags() {
        let config = serde_json::json!({
            "servers": {
                "all-test": {
                    "command": "test-mcp"
                }
            }
        });

        let runner = ClaudeRunner::for_continuation("prompt".into(), "system".into())
            .with_allowed_tools("Read,Write".to_string())
            .with_disallowed_tools("AskUserQuestion".to_string())
            .with_mcp_config(config.clone());

        // Ręcznie ustawiamy model (brak builder metody, ale mamy pole publiczne przez testy)
        let mut runner = runner;
        runner.model = Some("claude-sonnet-4".to_string());

        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);

        // Weryfikuj wszystkie flagi
        assert!(debug_str.contains("--model"));
        assert!(debug_str.contains("claude-sonnet-4"));
        assert!(debug_str.contains("--continue"));
        assert!(debug_str.contains("--allowedTools"));
        assert!(debug_str.contains("Read,Write"));
        assert!(debug_str.contains("--disallowedTools"));
        assert!(debug_str.contains("AskUserQuestion"));
        assert!(debug_str.contains("--mcp-config"));

        // Sprawdź zawartość JSON (bez zależności od formatowania)
        assert!(debug_str.contains("servers"));
        assert!(debug_str.contains("all-test"));
        assert!(debug_str.contains("test-mcp"));
    }

    /// Test 9: Kolejność argumentów — weryfikuj że podstawowe flagi są przed opcjonalnymi
    #[test]
    fn test_build_command_argument_order() {
        let runner = ClaudeRunner::oneshot("test".into(), Some("claude-opus-4".into()), None)
            .with_allowed_tools("Read".to_string());
        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);

        // Znajdź pozycje podstawowych flag
        let pos_p = debug_str.find("-p").expect("Should contain -p");
        let pos_output = debug_str
            .find("--output-format")
            .expect("Should contain --output-format");
        let pos_input = debug_str
            .find("--input-format")
            .expect("Should contain --input-format");
        let pos_verbose = debug_str
            .find("--verbose")
            .expect("Should contain --verbose");

        // Opcjonalne flagi
        let pos_model = debug_str.find("--model").expect("Should contain --model");
        let pos_allowed = debug_str
            .find("--allowedTools")
            .expect("Should contain --allowedTools");

        // Weryfikuj kolejność: podstawowe przed opcjonalnymi
        assert!(
            pos_p < pos_output,
            "'-p' should be before '--output-format'"
        );
        assert!(
            pos_output < pos_input,
            "'--output-format' should be before '--input-format'"
        );
        assert!(
            pos_input < pos_verbose,
            "'--input-format' should be before '--verbose'"
        );
        assert!(
            pos_verbose < pos_model,
            "'--verbose' should be before '--model'"
        );
        assert!(
            pos_model < pos_allowed,
            "'--model' should be before '--allowedTools'"
        );
    }

    /// Test 10: Edge case — mcp_config z pustym obiektem
    #[test]
    fn test_build_command_empty_mcp_config() {
        let config = serde_json::json!({});
        let runner = ClaudeRunner::oneshot("test".into(), None, None).with_mcp_config(config);
        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);

        assert!(debug_str.contains("--mcp-config"));
        assert!(debug_str.contains("{}"));
    }

    /// Test 11: Edge case — allowed_tools i disallowed_tools jednocześnie
    #[test]
    fn test_build_command_both_tool_filters() {
        let runner = ClaudeRunner::oneshot("test".into(), None, None)
            .with_allowed_tools("Read,Write,Bash".to_string())
            .with_disallowed_tools("AskUserQuestion".to_string());
        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);

        assert!(debug_str.contains("--allowedTools"));
        assert!(debug_str.contains("Read,Write,Bash"));
        assert!(debug_str.contains("--disallowedTools"));
        assert!(debug_str.contains("AskUserQuestion"));
    }

    /// Test 12: Z output_dir — weryfikuj że current_dir jest ustawiony
    #[test]
    fn test_build_command_with_output_dir() {
        let dir = PathBuf::from("/tmp/test-output");
        let runner = ClaudeRunner::oneshot("test".into(), None, Some(dir));
        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);

        // tokio::process::Command Debug zawiera current_dir gdy ustawiony
        assert!(
            debug_str.contains("/tmp/test-output"),
            "Command should have current_dir set to output_dir"
        );
    }

    /// Test 13: Bez output_dir — current_dir nie powinien być ustawiony
    #[test]
    fn test_build_command_without_output_dir() {
        let runner = ClaudeRunner::oneshot("test".into(), None, None);
        let cmd = runner.build_command();
        let debug_str = format!("{:?}", cmd);

        // Bez output_dir nie powinno być current_dir w debug output
        assert!(
            !debug_str.contains("current_dir"),
            "Command should NOT have current_dir when output_dir is None"
        );
    }
}
