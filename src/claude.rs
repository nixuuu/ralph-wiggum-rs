use serde::Deserialize;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::error::{RalphError, Result};

/// Token usage information from claude
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
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
    },

    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct AssistantMessage {
    pub role: String,
    pub content: Vec<ContentBlock>,
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
    prompt: Option<String>,
    continue_conversation: bool,
}

impl ClaudeRunner {
    /// Create runner for first iteration (with prompt)
    pub fn with_prompt(prompt: String) -> Self {
        Self {
            prompt: Some(prompt),
            continue_conversation: false,
        }
    }

    /// Create runner for continuation with verification prompt
    pub fn for_continuation(verification_prompt: String) -> Self {
        Self {
            prompt: Some(verification_prompt),
            continue_conversation: true,
        }
    }

    /// Run claude and stream output
    /// Calls on_event for each JSON event
    /// Returns last assistant text message
    pub async fn run<F>(&self, mut on_event: F) -> Result<Option<String>>
    where
        F: FnMut(&ClaudeEvent),
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

        if self.continue_conversation {
            cmd.arg("--continue");
        }

        // Add prompt as positional argument
        if let Some(prompt) = &self.prompt {
            cmd.arg(prompt);
        } else {
            return Err(RalphError::Config(
                "ClaudeRunner: Prompt is required".into(),
            ));
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::inherit());

        let mut child = cmd.spawn().map_err(|e| {
            RalphError::ClaudeProcess(format!("Failed to spawn claude: {}", e))
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            RalphError::ClaudeProcess("Failed to capture stdout".into())
        })?;

        let mut reader = BufReader::new(stdout).lines();
        let mut last_assistant_text: Option<String> = None;

        while let Some(line) = reader.next_line().await? {
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

        let status = child.wait().await?;
        if !status.success() {
            return Err(RalphError::ClaudeProcess(format!(
                "claude exited with status: {}",
                status
            )));
        }

        Ok(last_assistant_text)
    }
}
