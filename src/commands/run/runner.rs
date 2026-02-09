use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::shared::error::{RalphError, Result};

/// Token usage information from claude
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    #[allow(dead_code)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    #[allow(dead_code)]
    pub cache_creation_input_tokens: u64,
}

/// Per-model usage entry from Claude CLI result event
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ModelUsageEntry {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default, rename = "costUSD")]
    pub cost_usd: f64,
}

/// Events from claude JSON output
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
pub enum ClaudeEvent {
    #[serde(rename = "assistant")]
    Assistant { message: AssistantMessage },

    #[serde(rename = "user")]
    User {
        #[serde(default)]
        message: serde_json::Value,
    },

    #[serde(rename = "system")]
    System {
        #[serde(default)]
        message: serde_json::Value,
    },

    #[serde(rename = "result")]
    Result {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default, alias = "total_cost_usd")]
        cost_usd: Option<f64>,
        #[serde(default)]
        duration_ms: Option<u64>,
        #[serde(default)]
        duration_api_ms: Option<u64>,
        #[serde(default)]
        usage: Option<Usage>,
        #[serde(default, rename = "modelUsage")]
        model_usage: Option<HashMap<String, ModelUsageEntry>>,
    },

    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct AssistantMessage {
    pub role: String,
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "tool_use")]
    ToolUse {
        name: String,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        input: serde_json::Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        #[serde(default)]
        tool_use_id: Option<String>,
        #[serde(default)]
        content: serde_json::Value,
    },

    #[serde(other)]
    Other,
}

pub struct ClaudeRunner {
    user_prompt: String,
    system_prompt: Option<String>,
    continue_conversation: bool,
    model: Option<String>,
    output_dir: Option<PathBuf>,
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
        }
    }

    /// Run claude and stream output
    /// Calls on_event for each JSON event
    /// Returns last assistant text message
    /// If shutdown flag is set, stops reading and returns early
    ///
    /// Note: If claude exits with non-zero status but we already received
    /// a complete response, we return the response instead of an error.
    /// This handles cases where claude CLI exits with code 1 after
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
        // -p for non-interactive print mode
        // --output-format stream-json for realtime JSON streaming
        // --verbose is required for stream-json
        // --dangerously-skip-permissions to auto-accept all actions
        cmd.arg("-p");
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--verbose");
        cmd.arg("--dangerously-skip-permissions");

        if let Some(ref model) = self.model {
            cmd.arg("--model").arg(model);
        }

        if self.continue_conversation {
            cmd.arg("--continue");
        }

        // Add system prompt via --append-system-prompt
        if let Some(system_prompt) = &self.system_prompt {
            cmd.arg("--append-system-prompt").arg(system_prompt);
        }

        // Add user prompt as positional argument
        cmd.arg(&self.user_prompt);

        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::inherit());
        // Kill child process when dropped (e.g., on Ctrl+C)
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
                    // Shutdown requested - kill child and return
                    child.kill().await.ok();
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
                                    eprintln!("Line: {}", line);
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

        let status = child.wait().await?;
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
