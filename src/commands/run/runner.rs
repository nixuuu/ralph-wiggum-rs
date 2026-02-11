use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
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
        #[allow(dead_code)]
        // Serde field: deserialized from Claude API but not currently used
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

pub struct ClaudeRunner {
    user_prompt: String,
    system_prompt: Option<String>,
    continue_conversation: bool,
    model: Option<String>,
    output_dir: Option<PathBuf>,
    allowed_tools: Option<String>,
    mcp_config: Option<serde_json::Value>,
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
            let config_json = serde_json::to_string(mcp_config)
                .unwrap_or_else(|_| "{}".to_string());
            cmd.arg("--mcp-config").arg(config_json);
        }

        // System prompt and user prompt are sent via stdin (no CLI args)

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

        let mut child = cmd
            .spawn()
            .map_err(|e| RalphError::ClaudeProcess(format!("Failed to spawn claude: {}", e)))?;

        // Save PID for process-group signals on shutdown
        let child_pid = child.id();

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

        let mut reader = BufReader::new(stdout).lines();
        let mut last_assistant_text: Option<String> = None;

        // Create a future that completes when shutdown is requested
        let shutdown_check = async {
            loop {
                if shutdown.load(Ordering::SeqCst) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        };
        tokio::pin!(shutdown_check);

        let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;

                _ = &mut shutdown_check => {
                    // Graceful shutdown: SIGTERM → wait → SIGKILL
                    graceful_shutdown(&mut child, child_pid).await;
                    return Err(RalphError::Interrupted);
                }

                line_result = reader.next_line() => {
                    match line_result? {
                        Some(line) => {
                            if line.is_empty() {
                                continue;
                            }

                            match serde_json::from_str::<ClaudeEvent>(&line) {
                                Ok(event) => {
                                    // Extract text from assistant messages
                                    if let ClaudeEvent::Assistant { ref message } = event {
                                        for block in &message.content {
                                            if let ContentBlock::Text { text } = block {
                                                last_assistant_text = Some(text.clone());
                                            }
                                        }
                                    }

                                    on_event(&event);
                                }
                                Err(e) => {
                                    eprintln!("Warning: Failed to parse JSON line: {}", e);
                                    eprintln!("Line: {}", &line[..line.len().min(200)]);
                                }
                            }
                        }
                        None => break, // EOF - child process finished
                    }
                }

                _ = tick.tick() => {
                    on_idle();
                }
            }
        }

        // Wait for process exit with timeout — Claude CLI sometimes hangs
        // after emitting all output (stdout EOF) but before actually exiting.
        const PROCESS_EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

        let status = match tokio::time::timeout(PROCESS_EXIT_TIMEOUT, child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(RalphError::Io(e)),
            Err(_elapsed) => {
                // Timeout — graceful shutdown instead of immediate SIGKILL
                graceful_shutdown(&mut child, child_pid).await;
                // If we already have a complete response, return it despite timeout
                if last_assistant_text.is_some() {
                    return Ok(last_assistant_text);
                }
                return Err(RalphError::ClaudeProcess(
                    "claude process did not exit within 30s after closing stdout".into(),
                ));
            }
        };

        if !status.success() {
            // If we already received assistant's response, return it despite non-zero exit code.
            // Claude CLI may exit with code 1 after delivering complete response
            // (e.g., during cleanup or due to internal warnings).
            if last_assistant_text.is_some() {
                return Ok(last_assistant_text);
            }
            return Err(RalphError::ClaudeProcess(format!(
                "claude exited with status: {}",
                status
            )));
        }

        Ok(last_assistant_text)
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
}
