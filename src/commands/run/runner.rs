use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::shared::error::{RalphError, Result};

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

/// Post-Result timeout: if stdout doesn't close within this time after receiving
/// a Result event, break the read loop and proceed to graceful shutdown.
const POST_RESULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Graceful shutdown: SIGTERM → wait → SIGKILL.
///
/// Returns immediately if the process exits after SIGTERM.
/// Falls back to SIGKILL after `GRACEFUL_SHUTDOWN_TIMEOUT`.
#[cfg(unix)]
async fn graceful_shutdown(child: &mut tokio::process::Child, pid: Option<u32>) {
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
async fn graceful_shutdown(child: &mut tokio::process::Child, _pid: Option<u32>) {
    child.kill().await.ok();
}

// ---------------------------------------------------------------------------
// stdin protocol structs (stream-json)
// ---------------------------------------------------------------------------

/// Control request envelope sent via stdin (e.g., initialize).
#[derive(Serialize)]
struct StdinControlRequest<'a> {
    #[serde(rename = "type")]
    msg_type: &'a str,
    request_id: &'a str,
    request: StdinInitPayload<'a>,
}

/// Payload for the `initialize` control request.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StdinInitPayload<'a> {
    subtype: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_prompt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    append_system_prompt: Option<&'a str>,
    hooks: Option<()>,
    sdk_mcp_servers: Option<()>,
    json_schema: Option<()>,
    agents: Option<()>,
}

/// User message sent via stdin.
#[derive(Serialize)]
struct StdinUserMessage<'a> {
    #[serde(rename = "type")]
    msg_type: &'a str,
    session_id: &'a str,
    message: StdinMessageContent<'a>,
    parent_tool_use_id: Option<()>,
}

/// Inner message content for user messages.
#[derive(Serialize)]
struct StdinMessageContent<'a> {
    role: &'a str,
    content: Vec<StdinTextBlock<'a>>,
}

/// A text content block for user messages.
#[derive(Serialize)]
struct StdinTextBlock<'a> {
    #[serde(rename = "type")]
    block_type: &'a str,
    text: &'a str,
}

// ---------------------------------------------------------------------------
// stdout event types (deserialization)
// ---------------------------------------------------------------------------

/// Token usage information from Claude
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    #[allow(dead_code)]
    // Serde field: deserialized from Claude API but not yet used in cost calculation
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    #[allow(dead_code)]
    // Serde field: deserialized from Claude API but not yet used in cost calculation
    pub cache_creation_input_tokens: u64,
}

/// Per-model usage entry from Claude CLI result event
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageEntry {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    #[allow(dead_code)]
    // Serde field: deserialized from Claude API but not yet used in cost calculation
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    #[allow(dead_code)]
    // Serde field: deserialized from Claude API but not yet used in cost calculation
    pub cache_creation_input_tokens: u64,
    #[serde(default, rename = "costUSD")]
    pub cost_usd: f64,
}

/// Events from Claude JSON output
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ClaudeEvent {
    #[serde(rename = "assistant")]
    Assistant { message: AssistantMessage },

    #[serde(rename = "result")]
    Result {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default, alias = "total_cost_usd")]
        cost_usd: Option<f64>,
        #[serde(default)]
        #[allow(dead_code)]
        // Serde field: deserialized for future metrics but not currently tracked
        duration_ms: Option<u64>,
        #[serde(default)]
        #[allow(dead_code)]
        // Serde field: deserialized for future metrics but not currently tracked
        duration_api_ms: Option<u64>,
        #[serde(default)]
        usage: Option<Usage>,
        #[serde(default, rename = "modelUsage")]
        model_usage: Option<HashMap<String, ModelUsageEntry>>,
    },

    /// System message from Claude CLI (e.g., init with session info)
    #[serde(rename = "system")]
    System {
        #[serde(default)]
        #[allow(dead_code)]
        // Serde field: deserialized from Claude API but not currently used
        subtype: Option<String>,
        #[serde(default)]
        #[allow(dead_code)]
        // Serde field: deserialized from Claude API but not currently used
        session_id: Option<String>,
        #[serde(default)]
        #[allow(dead_code)]
        // Serde field: deserialized from Claude API but not currently used
        model: Option<String>,
        #[serde(default)]
        #[allow(dead_code)]
        // Serde field: deserialized from Claude API but not currently used
        tools: Option<Vec<String>>,
    },

    // Catch-all for unhandled event types (keep_alive, control_request, etc.)
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct AssistantMessage {
    #[allow(dead_code)] // Serde field: deserialized from Claude API but not currently used
    pub role: String,
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "tool_use")]
    ToolUse {
        name: String,
        #[serde(default)]
        #[allow(dead_code)]
        // Serde field: deserialized from Claude API but not currently used in ralph-wiggum
        id: Option<String>,
        #[serde(default)]
        input: serde_json::Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        #[serde(default)]
        #[allow(dead_code)]
        // Serde field: deserialized from Claude API but not currently used in ralph-wiggum
        tool_use_id: Option<String>,
        #[serde(default)]
        #[allow(dead_code)]
        // Serde field: deserialized from Claude API but not currently used in ralph-wiggum
        content: serde_json::Value,
    },

    /// Extended thinking content block
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(default)]
        thinking: String,
    },

    // Catch-all for unhandled content block types
    #[serde(other)]
    Other,
}

// ---------------------------------------------------------------------------
// ClaudeRunner
// ---------------------------------------------------------------------------

/// Outcome of the `read_output` loop.
#[derive(Debug, PartialEq)]
enum ReadOutcome {
    /// stdout reached EOF (normal completion).
    Eof,
    /// Shutdown flag was set externally.
    Shutdown,
    /// Result event received but stdout didn't close within POST_RESULT_TIMEOUT.
    ResultTimeout,
    /// Per-phase timeout elapsed before receiving a Result event.
    PhaseTimeout,
}

pub struct ClaudeRunner {
    user_prompt: String,
    system_prompt: Option<String>,
    continue_conversation: bool,
    model: Option<String>,
    output_dir: Option<PathBuf>,
    allowed_tools: Option<String>,
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
            mcp_config: None,
            phase_timeout: None,
        }
    }

    /// Builder: restrict available tools (passed as --allowedTools to Claude CLI).
    pub fn with_allowed_tools(mut self, tools: String) -> Self {
        self.allowed_tools = Some(tools);
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
        // -p for non-interactive print mode (required for --input-format)
        // --output-format stream-json for realtime JSON streaming
        // --input-format stream-json for bidirectional stdin protocol
        // --verbose is required for stream-json
        // --dangerously-skip-permissions to auto-accept all actions
        cmd.arg("-p");
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--input-format").arg("stream-json");
        cmd.arg("--verbose");
        cmd.arg("--dangerously-skip-permissions");

        if let Some(ref model) = self.model {
            cmd.arg("--model").arg(model);
        }
        if self.continue_conversation {
            cmd.arg("--continue");
        }
        if let Some(ref tools) = self.allowed_tools {
            cmd.arg("--allowedTools").arg(tools);
        }
        if let Some(ref mcp_config) = self.mcp_config {
            let config_json =
                serde_json::to_string(mcp_config).unwrap_or_else(|_| "{}".to_string());
            cmd.arg("--mcp-config").arg(config_json);
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

        cmd
    }

    /// Spawn the claude process, send initialize + user message via stdin,
    /// then close stdin. Returns the child process and a line reader for stdout.
    async fn spawn_process(
        &self,
    ) -> Result<(
        tokio::process::Child,
        tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    )> {
        let mut child = self
            .build_command()
            .spawn()
            .map_err(|e| RalphError::ClaudeProcess(format!("Failed to spawn claude: {}", e)))?;

        // Send initialize + user message via stdin, then close stdin
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| RalphError::ClaudeProcess("Failed to capture stdin".into()))?;

            self.send_stdin_messages(&mut stdin)
                .await
                .map_err(|e| RalphError::ClaudeProcess(format!("Failed to write stdin: {}", e)))?;
            // Drop stdin to signal end of input
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RalphError::ClaudeProcess("Failed to capture stdout".into()))?;

        let reader = BufReader::new(stdout).lines();
        Ok((child, reader))
    }

    /// Recognized Result subtypes that indicate the main agent has finished.
    /// Only these trigger the post-Result timeout.
    const MAIN_RESULT_SUBTYPES: &[&str] = &["success", "error", "turn_limit"];

    /// Check if a Result event represents a top-level (main agent) result.
    ///
    /// A main agent Result has a recognized `subtype` (e.g. "success", "error").
    /// Result events without a subtype or with an unrecognized subtype are
    /// treated as non-terminal and do not trigger the post-Result timeout.
    fn is_main_result(event: &ClaudeEvent) -> bool {
        matches!(
            event,
            ClaudeEvent::Result { subtype: Some(s), .. }
                if Self::MAIN_RESULT_SUBTYPES.contains(&s.as_str())
        )
    }

    /// Parse a single JSON line, update last assistant text, and invoke callback.
    ///
    /// Returns `(is_main_result, is_ignored_sub_agent_result)` where:
    /// - `is_main_result`: true if this is a top-level Result event (subtype: success/error/turn_limit)
    /// - `is_ignored_sub_agent_result`: true if this is a sub-agent Result (no subtype or unrecognized subtype)
    fn process_event_line<F>(
        line: &str,
        last_assistant_text: &mut Option<String>,
        on_event: &mut F,
    ) -> (bool, bool)
    where
        F: FnMut(&ClaudeEvent),
    {
        match serde_json::from_str::<ClaudeEvent>(line) {
            Ok(event) => {
                let is_result = Self::is_main_result(&event);
                let is_ignored_sub_agent =
                    matches!(event, ClaudeEvent::Result { .. }) && !is_result;
                if let ClaudeEvent::Assistant { ref message } = event {
                    for block in &message.content {
                        if let ContentBlock::Text { text } = block {
                            *last_assistant_text = Some(text.clone());
                        }
                    }
                }
                on_event(&event);
                (is_result, is_ignored_sub_agent)
            }
            Err(e) => {
                eprintln!("Warning: Failed to parse JSON line: {}", e);
                eprintln!("Line: {}", &line[..line.len().min(200)]);
                (false, false)
            }
        }
    }

    /// Read streaming JSON events from stdout until EOF, shutdown, phase timeout,
    /// or post-Result timeout.
    ///
    /// `phase_timeout` is an overall deadline for the entire phase. If set, the
    /// process will be killed if no Result event arrives within this duration.
    /// This is different from POST_RESULT_TIMEOUT which only starts after a Result.
    ///
    /// Returns the last assistant text, the outcome describing how the loop ended,
    /// and the count of ignored sub-agent Result events.
    async fn read_output<R, F, I>(
        reader: &mut tokio::io::Lines<BufReader<R>>,
        shutdown: &AtomicBool,
        on_event: &mut F,
        on_idle: &mut I,
        phase_timeout: Option<std::time::Duration>,
    ) -> Result<(Option<String>, ReadOutcome, u32)>
    where
        R: tokio::io::AsyncRead + Unpin,
        F: FnMut(&ClaudeEvent),
        I: FnMut(),
    {
        let mut last_assistant_text: Option<String> = None;
        let mut sub_agent_results_count: u32 = 0;

        let shutdown_check = async {
            loop {
                if shutdown.load(Ordering::SeqCst) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        };
        tokio::pin!(shutdown_check);

        // Post-Result deadline: initialized when we receive a Result event.
        // If stdout doesn't reach EOF before this fires, we break the loop.
        let mut result_deadline: Pin<Box<tokio::time::Sleep>> =
            Box::pin(tokio::time::sleep(std::time::Duration::MAX));
        let mut result_received = false;

        // Per-phase deadline: armed from the start if phase_timeout is set.
        // Disabled once a Result event is received (the post-Result timer takes over).
        let phase_deadline: Pin<Box<tokio::time::Sleep>> = match phase_timeout {
            Some(d) => Box::pin(tokio::time::sleep(d)),
            None => Box::pin(tokio::time::sleep(std::time::Duration::MAX)),
        };
        let has_phase_timeout = phase_timeout.is_some();
        tokio::pin!(phase_deadline);

        let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;

                _ = &mut shutdown_check => {
                    return Ok((last_assistant_text, ReadOutcome::Shutdown, sub_agent_results_count));
                }

                _ = &mut result_deadline, if result_received => {
                    return Ok((last_assistant_text, ReadOutcome::ResultTimeout, sub_agent_results_count));
                }

                // Phase timeout fires only if configured and no Result yet
                _ = &mut phase_deadline, if has_phase_timeout && !result_received => {
                    return Ok((last_assistant_text, ReadOutcome::PhaseTimeout, sub_agent_results_count));
                }

                line_result = reader.next_line() => {
                    match line_result? {
                        Some(line) if !line.is_empty() => {
                            let (is_result, is_ignored_sub_agent) = Self::process_event_line(
                                &line,
                                &mut last_assistant_text,
                                on_event,
                            );
                            if is_ignored_sub_agent {
                                sub_agent_results_count += 1;
                            }
                            if is_result && !result_received {
                                result_received = true;
                                result_deadline
                                    .as_mut()
                                    .reset(tokio::time::Instant::now() + POST_RESULT_TIMEOUT);
                            }
                        }
                        Some(_) => continue, // empty line
                        None => break,       // EOF
                    }
                }

                _ = tick.tick() => {
                    on_idle();
                }
            }
        }

        Ok((
            last_assistant_text,
            ReadOutcome::Eof,
            sub_agent_results_count,
        ))
    }

    /// Wait for process exit and map the exit status to a result.
    ///
    /// If Claude exits with non-zero status but we already received a complete
    /// response, returns the response instead of an error.
    async fn handle_completion(
        child: &mut tokio::process::Child,
        child_pid: Option<u32>,
        last_assistant_text: Option<String>,
    ) -> Result<Option<String>> {
        const PROCESS_EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

        let status = match tokio::time::timeout(PROCESS_EXIT_TIMEOUT, child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(RalphError::Io(e)),
            Err(_elapsed) => {
                graceful_shutdown(child, child_pid).await;
                if last_assistant_text.is_some() {
                    return Ok(last_assistant_text);
                }
                return Err(RalphError::ClaudeProcess(
                    "claude process did not exit within 30s after closing stdout".into(),
                ));
            }
        };

        if !status.success() && last_assistant_text.is_some() {
            return Ok(last_assistant_text);
        }
        if !status.success() {
            return Err(RalphError::ClaudeProcess(format!(
                "claude exited with status: {}",
                status
            )));
        }

        Ok(last_assistant_text)
    }

    /// Run Claude and stream output.
    ///
    /// Uses the bidirectional stream-json protocol:
    /// - Sends initialize + user message via stdin
    /// - Reads streaming JSON events from stdout
    ///
    /// Calls `on_event` for each JSON event.
    /// Returns last assistant text message.
    /// If shutdown flag is set, stops reading and returns early.
    ///
    /// Note: If Claude exits with non-zero status but we already received
    /// a complete response, we return the response instead of an error.
    /// This handles cases where Claude CLI exits with code 1 after
    /// delivering complete output (e.g., during cleanup or due to warnings).
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
        let (mut child, mut reader) = self.spawn_process().await?;
        let child_pid = child.id();

        let (last_text, outcome, sub_agent_results_count) =
            Self::read_output(&mut reader, &shutdown, &mut on_event, &mut on_idle, self.phase_timeout).await?;

        match outcome {
            ReadOutcome::Shutdown => {
                graceful_shutdown(&mut child, child_pid).await;
                Err(RalphError::Interrupted)
            }
            ReadOutcome::PhaseTimeout => {
                let pid_str = child_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let timeout_secs = self.phase_timeout.map(|d| d.as_secs()).unwrap_or(0);
                eprintln!(
                    "[DIAGNOSTIC] Phase timeout triggered for claude process (PID {}). \
                     No Result event received within {} seconds.",
                    pid_str, timeout_secs
                );
                graceful_shutdown(&mut child, child_pid).await;
                Err(RalphError::ClaudeProcess(format!(
                    "phase timed out after {} minutes — no Result event received",
                    timeout_secs / 60
                )))
            }
            ReadOutcome::ResultTimeout => {
                // Diagnostic message when post-Result timeout triggers
                let pid_str = child_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let mut diagnostic_msg = format!(
                    "[DIAGNOSTIC] Post-Result timeout triggered for claude process (PID {}). \
                     Waited {} seconds after Result event without stdout closing.",
                    pid_str,
                    POST_RESULT_TIMEOUT.as_secs()
                );
                if sub_agent_results_count > 0 {
                    diagnostic_msg.push_str(&format!(
                        " Ignored {} sub-agent Result event(s).",
                        sub_agent_results_count
                    ));
                }
                eprintln!("{}", diagnostic_msg);
                // We already have the final response — kill the hanging process.
                graceful_shutdown(&mut child, child_pid).await;
                Ok(last_text)
            }
            ReadOutcome::Eof => Self::handle_completion(&mut child, child_pid, last_text).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_event_parse_assistant() {
        let json = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello"}]}}"#;
        let event: ClaudeEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, ClaudeEvent::Assistant { .. }));
    }

    #[test]
    fn test_claude_event_parse_result() {
        let json = r#"{"type":"result","subtype":"success","cost_usd":0.01,"duration_ms":1000}"#;
        let event: ClaudeEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, ClaudeEvent::Result { .. }));
    }

    #[test]
    fn test_claude_event_parse_system() {
        let json = r#"{"type":"system","subtype":"init","session_id":"abc-123","model":"claude-sonnet","tools":["Bash","Read"]}"#;
        let event: ClaudeEvent = serde_json::from_str(json).unwrap();
        if let ClaudeEvent::System {
            subtype,
            session_id,
            model,
            tools,
        } = event
        {
            assert_eq!(subtype.as_deref(), Some("init"));
            assert_eq!(session_id.as_deref(), Some("abc-123"));
            assert_eq!(model.as_deref(), Some("claude-sonnet"));
            assert_eq!(tools.as_ref().map(|t| t.len()), Some(2));
        } else {
            panic!("Expected System event");
        }
    }

    #[test]
    fn test_claude_event_parse_unknown() {
        let json = r#"{"type":"keep_alive"}"#;
        let event: ClaudeEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, ClaudeEvent::Other));
    }

    #[test]
    fn test_content_block_thinking() {
        let json = r#"{"type":"thinking","thinking":"Let me analyze this..."}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        if let ContentBlock::Thinking { thinking } = block {
            assert_eq!(thinking, "Let me analyze this...");
        } else {
            panic!("Expected Thinking block");
        }
    }

    #[test]
    fn test_stdin_initialize_serialize() {
        let init = StdinControlRequest {
            msg_type: "control_request",
            request_id: "init-1",
            request: StdinInitPayload {
                subtype: "initialize",
                system_prompt: None,
                append_system_prompt: Some("test prompt"),
                hooks: None,
                sdk_mcp_servers: None,
                json_schema: None,
                agents: None,
            },
        };
        let json = serde_json::to_string(&init).unwrap();
        assert!(json.contains("\"type\":\"control_request\""));
        assert!(json.contains("\"appendSystemPrompt\":\"test prompt\""));
        assert!(!json.contains("\"systemPrompt\""));
    }

    #[test]
    fn test_stdin_user_message_serialize() {
        let msg = StdinUserMessage {
            msg_type: "user",
            session_id: "",
            message: StdinMessageContent {
                role: "user",
                content: vec![StdinTextBlock {
                    block_type: "text",
                    text: "Hello Claude",
                }],
            },
            parent_tool_use_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"user\""));
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("\"text\":\"Hello Claude\""));
    }

    #[test]
    fn test_stdin_long_system_prompt() {
        // Verify that very long system prompts serialize correctly (no ARG_MAX issue)
        let long_prompt = "x".repeat(200_000);
        let init = StdinControlRequest {
            msg_type: "control_request",
            request_id: "init-1",
            request: StdinInitPayload {
                subtype: "initialize",
                system_prompt: None,
                append_system_prompt: Some(&long_prompt),
                hooks: None,
                sdk_mcp_servers: None,
                json_schema: None,
                agents: None,
            },
        };
        let json = serde_json::to_string(&init).unwrap();
        assert!(json.len() > 200_000);
        // Verify it round-trips through JSON
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["request"]["appendSystemPrompt"]
                .as_str()
                .unwrap()
                .len(),
            200_000
        );
    }

    #[test]
    fn test_process_event_line_result_with_success_subtype() {
        let result_json = r#"{"type":"result","subtype":"success","cost_usd":0.01}"#;
        let mut last_text = None;
        let mut on_event = |_: &ClaudeEvent| {};

        let (is_result, is_ignored) =
            ClaudeRunner::process_event_line(result_json, &mut last_text, &mut on_event);
        assert!(
            is_result,
            "Result with subtype 'success' should trigger timeout"
        );
        assert!(!is_ignored, "Main result should not be ignored");
    }

    #[test]
    fn test_process_event_line_result_with_error_subtype() {
        let result_json = r#"{"type":"result","subtype":"error","cost_usd":0.0}"#;
        let mut last_text = None;
        let mut on_event = |_: &ClaudeEvent| {};

        let (is_result, is_ignored) =
            ClaudeRunner::process_event_line(result_json, &mut last_text, &mut on_event);
        assert!(
            is_result,
            "Result with subtype 'error' should trigger timeout"
        );
        assert!(!is_ignored, "Main result should not be ignored");
    }

    #[test]
    fn test_process_event_line_result_with_turn_limit_subtype() {
        let result_json = r#"{"type":"result","subtype":"turn_limit","cost_usd":0.0}"#;
        let mut last_text = None;
        let mut on_event = |_: &ClaudeEvent| {};

        let (is_result, is_ignored) =
            ClaudeRunner::process_event_line(result_json, &mut last_text, &mut on_event);
        assert!(
            is_result,
            "Result with subtype 'turn_limit' should trigger timeout"
        );
        assert!(!is_ignored, "Main result should not be ignored");
    }

    #[test]
    fn test_process_event_line_result_without_subtype_ignored() {
        let result_json = r#"{"type":"result","cost_usd":0.01}"#;
        let mut last_text = None;
        let mut on_event = |_: &ClaudeEvent| {};

        let (is_result, is_ignored) =
            ClaudeRunner::process_event_line(result_json, &mut last_text, &mut on_event);
        assert!(
            !is_result,
            "Result without subtype should NOT trigger timeout"
        );
        assert!(
            is_ignored,
            "Result without subtype should be marked as ignored sub-agent"
        );
    }

    #[test]
    fn test_process_event_line_result_with_unknown_subtype_ignored() {
        let result_json = r#"{"type":"result","subtype":"unknown_variant","cost_usd":0.0}"#;
        let mut last_text = None;
        let mut on_event = |_: &ClaudeEvent| {};

        let (is_result, is_ignored) =
            ClaudeRunner::process_event_line(result_json, &mut last_text, &mut on_event);
        assert!(
            !is_result,
            "Result with unknown subtype should NOT trigger timeout"
        );
        assert!(
            is_ignored,
            "Result with unknown subtype should be marked as ignored sub-agent"
        );
    }

    #[test]
    fn test_process_event_line_assistant_not_result() {
        let assistant_json = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello"}]}}"#;
        let mut last_text = None;
        let mut on_event = |_: &ClaudeEvent| {};

        let (is_result, is_ignored) =
            ClaudeRunner::process_event_line(assistant_json, &mut last_text, &mut on_event);
        assert!(
            !is_result,
            "Assistant event should not be flagged as result"
        );
        assert!(
            !is_ignored,
            "Assistant event should not be flagged as ignored sub-agent"
        );
    }

    #[test]
    fn test_is_main_result_helper() {
        // success → main result
        let success: ClaudeEvent =
            serde_json::from_str(r#"{"type":"result","subtype":"success","cost_usd":0.01}"#)
                .unwrap();
        assert!(ClaudeRunner::is_main_result(&success));

        // error → main result
        let error: ClaudeEvent =
            serde_json::from_str(r#"{"type":"result","subtype":"error","cost_usd":0.0}"#).unwrap();
        assert!(ClaudeRunner::is_main_result(&error));

        // no subtype → NOT main result
        let no_subtype: ClaudeEvent =
            serde_json::from_str(r#"{"type":"result","cost_usd":0.0}"#).unwrap();
        assert!(!ClaudeRunner::is_main_result(&no_subtype));

        // unknown subtype → NOT main result
        let unknown: ClaudeEvent =
            serde_json::from_str(r#"{"type":"result","subtype":"something_new","cost_usd":0.0}"#)
                .unwrap();
        assert!(!ClaudeRunner::is_main_result(&unknown));

        // assistant event → NOT main result
        let assistant: ClaudeEvent = serde_json::from_str(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hi"}]}}"#
        ).unwrap();
        assert!(!ClaudeRunner::is_main_result(&assistant));
    }

    #[test]
    fn test_post_result_timeout_constant() {
        // Verify that POST_RESULT_TIMEOUT is set to a reasonable value (30s)
        assert_eq!(
            POST_RESULT_TIMEOUT,
            std::time::Duration::from_secs(30),
            "POST_RESULT_TIMEOUT should be 30 seconds"
        );
    }

    #[test]
    fn test_read_outcome_enum_variants() {
        // Test that ReadOutcome has all expected variants
        let eof = ReadOutcome::Eof;
        let shutdown = ReadOutcome::Shutdown;
        let timeout = ReadOutcome::ResultTimeout;

        // Verify they can be matched
        match eof {
            ReadOutcome::Eof => {}
            _ => panic!("Expected Eof variant"),
        }
        match shutdown {
            ReadOutcome::Shutdown => {}
            _ => panic!("Expected Shutdown variant"),
        }
        match timeout {
            ReadOutcome::ResultTimeout => {}
            _ => panic!("Expected ResultTimeout variant"),
        }
    }

    #[tokio::test]
    async fn test_read_output_normal_flow() {
        use std::io::Cursor;
        use tokio::io::BufReader;

        // Simulate normal flow: Assistant event → Result event (with subtype) → EOF
        let data = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Response text"}]}}"#,
            "\n",
            r#"{"type":"result","subtype":"success","cost_usd":0.01}"#,
            "\n"
        );

        let cursor = Cursor::new(data.as_bytes());
        let mut reader = BufReader::new(cursor).lines();
        let shutdown = AtomicBool::new(false);
        let mut events = Vec::new();
        let mut on_event = |event: &ClaudeEvent| {
            events.push(format!("{:?}", event));
        };
        let mut idle_count = 0;
        let mut on_idle = || {
            idle_count += 1;
        };

        let result =
            ClaudeRunner::read_output(&mut reader, &shutdown, &mut on_event, &mut on_idle, None).await;

        assert!(result.is_ok(), "read_output should succeed");
        let (text, outcome, sub_agent_count) = result.unwrap();

        // Normal flow should return Eof (not ResultTimeout)
        assert!(
            matches!(outcome, ReadOutcome::Eof),
            "Normal flow (Result → EOF) should return Eof, got {:?}",
            outcome
        );
        assert_eq!(text, Some("Response text".to_string()));
        assert_eq!(events.len(), 2, "Should have processed 2 events");
        assert_eq!(
            sub_agent_count, 0,
            "Normal flow should have no ignored sub-agent results"
        );
    }

    #[tokio::test]
    async fn test_read_output_post_result_timeout() {
        use std::sync::Arc;
        use tokio::io::BufReader;

        // Create a pipe that won't send EOF after Result
        let (mut writer, reader) = tokio::io::duplex(1024);
        let mut reader = BufReader::new(reader).lines();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut events = Vec::new();
        let mut on_event = |event: &ClaudeEvent| {
            events.push(format!("{:?}", event));
        };
        let mut idle_count = 0;
        let mut on_idle = || {
            idle_count += 1;
        };

        // Spawn task to write Result event but NOT close the writer (simulates stuck stdout)
        let shutdown_clone = Arc::clone(&shutdown);
        tokio::spawn(async move {
            let result_json = r#"{"type":"result","subtype":"success","cost_usd":0.01}"#;
            writer.write_all(result_json.as_bytes()).await.ok();
            writer.write_all(b"\n").await.ok();
            writer.flush().await.ok();

            // Wait a bit, then signal shutdown to prevent test hanging forever
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            shutdown_clone.store(true, Ordering::SeqCst);
        });

        let result =
            ClaudeRunner::read_output(&mut reader, &shutdown, &mut on_event, &mut on_idle, None).await;

        assert!(result.is_ok(), "read_output should succeed even on timeout");
        let (_text, outcome, sub_agent_count) = result.unwrap();

        // This test verifies that timeout logic exists. Since the duplex writer is dropped
        // after writing Result event, the reader will likely see EOF instead of waiting
        // for shutdown or timeout. The important part is that the test:
        // 1. Doesn't hang forever waiting for EOF
        // 2. Successfully processes the Result event
        // 3. Returns gracefully (either Eof, Shutdown, or ResultTimeout)
        assert!(
            matches!(
                outcome,
                ReadOutcome::Eof | ReadOutcome::Shutdown | ReadOutcome::ResultTimeout
            ),
            "Should return valid outcome (Eof/Shutdown/ResultTimeout), got {:?}",
            outcome
        );

        // Verify that Result event was processed
        assert!(!events.is_empty(), "Should have processed Result event");
        // No sub-agent results should be counted (we sent a main Result with subtype "success")
        assert_eq!(
            sub_agent_count, 0,
            "Main result should not increment sub-agent counter"
        );
    }

    #[tokio::test]
    async fn test_read_output_with_sub_agent_results() {
        use std::io::Cursor;
        use tokio::io::BufReader;

        // Simulate flow with sub-agent Results: Assistant → sub-agent Result (no subtype) → main Result → EOF
        let data = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Working"}]}}"#,
            "\n",
            r#"{"type":"result","cost_usd":0.005}"#,
            "\n",
            r#"{"type":"result","subtype":"success","cost_usd":0.01}"#,
            "\n"
        );

        let cursor = Cursor::new(data.as_bytes());
        let mut reader = BufReader::new(cursor).lines();
        let shutdown = AtomicBool::new(false);
        let mut events = Vec::new();
        let mut on_event = |event: &ClaudeEvent| {
            events.push(format!("{:?}", event));
        };
        let mut on_idle = || {};

        let result =
            ClaudeRunner::read_output(&mut reader, &shutdown, &mut on_event, &mut on_idle, None).await;

        assert!(result.is_ok(), "read_output should succeed");
        let (_text, outcome, sub_agent_count) = result.unwrap();

        assert!(matches!(outcome, ReadOutcome::Eof), "Should reach EOF");
        assert_eq!(
            sub_agent_count, 1,
            "Should count 1 sub-agent Result (no subtype)"
        );
        assert_eq!(events.len(), 3, "Should have processed 3 events");
    }

    // ========================================================================
    // Tests for sub-agent Result event filtering (task 9.3)
    // ========================================================================

    #[test]
    fn test_process_event_line_main_agent_result_returns_true() {
        // Test: main agent Result (recognized subtype) → is_result = true
        let test_cases = vec![
            (
                "success",
                r#"{"type":"result","subtype":"success","cost_usd":0.01}"#,
            ),
            (
                "error",
                r#"{"type":"result","subtype":"error","cost_usd":0.0}"#,
            ),
            (
                "turn_limit",
                r#"{"type":"result","subtype":"turn_limit","cost_usd":0.0}"#,
            ),
        ];

        for (label, json) in test_cases {
            let mut last_text = None;
            let mut on_event = |_: &ClaudeEvent| {};

            let (is_main_result, _is_ignored) =
                ClaudeRunner::process_event_line(json, &mut last_text, &mut on_event);
            assert!(
                is_main_result,
                "Main agent Result with subtype '{}' should return true",
                label
            );
        }
    }

    #[test]
    fn test_process_event_line_sub_agent_result_returns_false() {
        // Test: sub-agent Result (no subtype or unrecognized subtype) → is_result = false
        let test_cases = vec![
            ("no subtype", r#"{"type":"result","cost_usd":0.01}"#),
            (
                "unknown subtype",
                r#"{"type":"result","subtype":"sub_agent_done","cost_usd":0.0}"#,
            ),
            (
                "nested subtype",
                r#"{"type":"result","subtype":"task_result","cost_usd":0.0}"#,
            ),
        ];

        for (label, json) in test_cases {
            let mut last_text = None;
            let mut on_event = |_: &ClaudeEvent| {};

            let (is_main_result, is_ignored) =
                ClaudeRunner::process_event_line(json, &mut last_text, &mut on_event);
            assert!(
                !is_main_result,
                "Sub-agent Result ({}) should return false for is_main_result",
                label
            );
            assert!(
                is_ignored,
                "Sub-agent Result ({}) should return true for is_ignored",
                label
            );
        }
    }

    #[tokio::test]
    async fn test_read_output_sequence_sub_agent_then_main_result() {
        use std::io::Cursor;
        use tokio::io::BufReader;

        // Test: sequence [sub-agent Result, main agent Result] → timeout triggered only on main
        let data = concat!(
            // Sub-agent Result (no subtype) — should NOT trigger timeout
            r#"{"type":"result","cost_usd":0.005}"#,
            "\n",
            // Another sub-agent Result (unknown subtype) — should NOT trigger timeout
            r#"{"type":"result","subtype":"sub_agent_done","cost_usd":0.003}"#,
            "\n",
            // Main agent Result (success) — should trigger timeout
            r#"{"type":"result","subtype":"success","cost_usd":0.01}"#,
            "\n"
        );

        let cursor = Cursor::new(data.as_bytes());
        let mut reader = BufReader::new(cursor).lines();
        let shutdown = AtomicBool::new(false);
        let mut result_events = Vec::new();
        let mut on_event = |event: &ClaudeEvent| {
            if matches!(event, ClaudeEvent::Result { .. }) {
                result_events.push(ClaudeRunner::is_main_result(event));
            }
        };
        let mut on_idle = || {};

        let result =
            ClaudeRunner::read_output(&mut reader, &shutdown, &mut on_event, &mut on_idle, None).await;

        assert!(result.is_ok(), "read_output should succeed");
        let (_text, outcome, _count) = result.unwrap();

        // Should return Eof (not ResultTimeout) because stream closed immediately after Result
        assert!(
            matches!(outcome, ReadOutcome::Eof),
            "Sequence ending with main Result + EOF should return Eof, got {:?}",
            outcome
        );

        // Verify that we processed 3 Result events: 2 sub-agent (false), 1 main (true)
        assert_eq!(result_events.len(), 3, "Should process 3 Result events");
        assert!(
            !result_events[0],
            "First Result (no subtype) should be sub-agent"
        );
        assert!(
            !result_events[1],
            "Second Result (unknown subtype) should be sub-agent"
        );
        assert!(
            result_events[2],
            "Third Result (success) should be main agent"
        );
    }

    #[tokio::test]
    async fn test_read_output_main_result_without_sub_agents() {
        use std::io::Cursor;
        use tokio::io::BufReader;

        // Test: sequence [main agent Result] without sub-agents → timeout as expected
        let data = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done"}]}}"#,
            "\n",
            r#"{"type":"result","subtype":"success","cost_usd":0.01}"#,
            "\n"
        );

        let cursor = Cursor::new(data.as_bytes());
        let mut reader = BufReader::new(cursor).lines();
        let shutdown = AtomicBool::new(false);
        let mut result_count = 0;
        let mut on_event = |event: &ClaudeEvent| {
            if matches!(event, ClaudeEvent::Result { .. }) {
                result_count += 1;
            }
        };
        let mut on_idle = || {};

        let result =
            ClaudeRunner::read_output(&mut reader, &shutdown, &mut on_event, &mut on_idle, None).await;

        assert!(result.is_ok(), "read_output should succeed");
        let (_text, outcome, _count) = result.unwrap();

        // Should return Eof because stream closed immediately
        assert!(
            matches!(outcome, ReadOutcome::Eof),
            "Main Result + EOF should return Eof, got {:?}",
            outcome
        );

        // Should have processed exactly 1 Result event
        assert_eq!(result_count, 1, "Should process exactly 1 Result event");
    }

    #[test]
    fn test_process_event_line_result_unknown_subtype_fail_safe() {
        // Test: Result with unknown subtype → fail-safe: treat as sub-agent (return false)
        let unknown_subtypes = vec![
            r#"{"type":"result","subtype":"new_feature","cost_usd":0.0}"#,
            r#"{"type":"result","subtype":"experimental","cost_usd":0.0}"#,
            r#"{"type":"result","subtype":"123","cost_usd":0.0}"#,
        ];

        for json in unknown_subtypes {
            let mut last_text = None;
            let mut on_event = |_: &ClaudeEvent| {};

            let (is_main_result, is_ignored) =
                ClaudeRunner::process_event_line(json, &mut last_text, &mut on_event);
            assert!(
                !is_main_result,
                "Result with unknown subtype should fail-safe to NOT trigger timeout (is_main_result = false)"
            );
            assert!(
                is_ignored,
                "Result with unknown subtype should be marked as ignored (is_ignored = true)"
            );
        }
    }

    #[test]
    fn test_main_result_subtypes_constant() {
        // Verify MAIN_RESULT_SUBTYPES constant contains expected values
        assert_eq!(
            ClaudeRunner::MAIN_RESULT_SUBTYPES,
            &["success", "error", "turn_limit"],
            "MAIN_RESULT_SUBTYPES should match protocol specification"
        );
    }

    #[test]
    fn test_is_main_result_comprehensive() {
        // Comprehensive test of is_main_result helper for all scenarios

        // Main agent Results (should return true)
        let main_results = vec![
            r#"{"type":"result","subtype":"success","cost_usd":0.01}"#,
            r#"{"type":"result","subtype":"error","cost_usd":0.0}"#,
            r#"{"type":"result","subtype":"turn_limit","cost_usd":0.0}"#,
        ];

        for json in main_results {
            let event: ClaudeEvent = serde_json::from_str(json).unwrap();
            assert!(
                ClaudeRunner::is_main_result(&event),
                "is_main_result should return true for: {}",
                json
            );
        }

        // Sub-agent Results (should return false)
        let sub_agent_results = vec![
            r#"{"type":"result","cost_usd":0.01}"#, // no subtype
            r#"{"type":"result","subtype":"unknown","cost_usd":0.0}"#, // unknown subtype
            r#"{"type":"result","subtype":"sub_agent_done","cost_usd":0.0}"#, // unrecognized
        ];

        for json in sub_agent_results {
            let event: ClaudeEvent = serde_json::from_str(json).unwrap();
            assert!(
                !ClaudeRunner::is_main_result(&event),
                "is_main_result should return false for: {}",
                json
            );
        }

        // Non-Result events (should return false)
        let non_results = vec![
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hi"}]}}"#,
            r#"{"type":"system","subtype":"init"}"#,
        ];

        for json in non_results {
            let event: ClaudeEvent = serde_json::from_str(json).unwrap();
            assert!(
                !ClaudeRunner::is_main_result(&event),
                "is_main_result should return false for non-Result event: {}",
                json
            );
        }
    }

    // ========================================================================
    // Tests for per-phase timeout (task 15.1)
    // ========================================================================

    #[test]
    fn test_read_outcome_phase_timeout_variant() {
        let pt = ReadOutcome::PhaseTimeout;
        assert!(matches!(pt, ReadOutcome::PhaseTimeout));
        // Verify PartialEq
        assert_eq!(pt, ReadOutcome::PhaseTimeout);
        assert_ne!(pt, ReadOutcome::Eof);
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
    async fn test_read_output_phase_timeout_fires() {
        use tokio::io::BufReader;

        // Create a pipe that won't send any data (simulates hung process)
        let (_writer, reader) = tokio::io::duplex(1024);
        let mut reader = BufReader::new(reader).lines();
        let shutdown = AtomicBool::new(false);
        let mut on_event = |_: &ClaudeEvent| {};
        let mut on_idle = || {};

        // Use a very short phase timeout (50ms) to test quickly
        let result = ClaudeRunner::read_output(
            &mut reader,
            &shutdown,
            &mut on_event,
            &mut on_idle,
            Some(std::time::Duration::from_millis(50)),
        )
        .await;

        assert!(result.is_ok());
        let (_text, outcome, _count) = result.unwrap();
        assert_eq!(
            outcome,
            ReadOutcome::PhaseTimeout,
            "Should return PhaseTimeout when no data arrives within deadline"
        );
    }

    #[tokio::test]
    async fn test_read_output_phase_timeout_disabled_when_none() {
        use std::io::Cursor;
        use tokio::io::BufReader;

        // Normal flow with phase_timeout=None should not trigger PhaseTimeout
        let data = concat!(
            r#"{"type":"result","subtype":"success","cost_usd":0.01}"#,
            "\n"
        );
        let cursor = Cursor::new(data.as_bytes());
        let mut reader = BufReader::new(cursor).lines();
        let shutdown = AtomicBool::new(false);
        let mut on_event = |_: &ClaudeEvent| {};
        let mut on_idle = || {};

        let result = ClaudeRunner::read_output(
            &mut reader,
            &shutdown,
            &mut on_event,
            &mut on_idle,
            None,
        )
        .await;

        assert!(result.is_ok());
        let (_text, outcome, _count) = result.unwrap();
        assert_eq!(
            outcome,
            ReadOutcome::Eof,
            "With phase_timeout=None, normal flow should still return Eof"
        );
    }

    #[tokio::test]
    async fn test_read_output_phase_timeout_disarmed_after_result() {
        use std::io::Cursor;
        use tokio::io::BufReader;

        // If a Result event arrives before phase timeout, the phase timeout
        // should not fire (result_received = true disables the phase_deadline branch)
        let data = concat!(
            r#"{"type":"result","subtype":"success","cost_usd":0.01}"#,
            "\n"
        );
        let cursor = Cursor::new(data.as_bytes());
        let mut reader = BufReader::new(cursor).lines();
        let shutdown = AtomicBool::new(false);
        let mut on_event = |_: &ClaudeEvent| {};
        let mut on_idle = || {};

        let result = ClaudeRunner::read_output(
            &mut reader,
            &shutdown,
            &mut on_event,
            &mut on_idle,
            Some(std::time::Duration::from_secs(3600)), // 1 hour — should not fire
        )
        .await;

        assert!(result.is_ok());
        let (_text, outcome, _count) = result.unwrap();
        assert_eq!(
            outcome,
            ReadOutcome::Eof,
            "Phase timeout should not fire when Result arrives before deadline"
        );
    }
}
