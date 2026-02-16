//! Typy danych protokołu Claude CLI (stream-json).
//!
//! Zawiera struktury do serializacji (stdin) i deserializacji (stdout)
//! komunikacji z procesem Claude CLI.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// stdin protocol structs (stream-json)
// ---------------------------------------------------------------------------

/// Control request envelope sent via stdin (e.g., initialize).
#[derive(Serialize)]
pub(crate) struct StdinControlRequest<'a> {
    #[serde(rename = "type")]
    pub msg_type: &'a str,
    pub request_id: &'a str,
    pub request: StdinInitPayload<'a>,
}

/// Payload for the `initialize` control request.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StdinInitPayload<'a> {
    pub subtype: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<&'a str>,
    pub hooks: Option<()>,
    pub sdk_mcp_servers: Option<()>,
    pub json_schema: Option<()>,
    pub agents: Option<()>,
}

/// User message sent via stdin.
#[derive(Serialize)]
pub(crate) struct StdinUserMessage<'a> {
    #[serde(rename = "type")]
    pub msg_type: &'a str,
    pub session_id: &'a str,
    pub message: StdinMessageContent<'a>,
    pub parent_tool_use_id: Option<()>,
}

/// Inner message content for user messages.
#[derive(Serialize)]
pub(crate) struct StdinMessageContent<'a> {
    pub role: &'a str,
    pub content: Vec<StdinTextBlock<'a>>,
}

/// A text content block for user messages.
#[derive(Serialize)]
pub(crate) struct StdinTextBlock<'a> {
    #[serde(rename = "type")]
    pub block_type: &'a str,
    pub text: &'a str,
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

    /// User message (replayed via --replay-user-messages).
    /// Contains tool_result blocks with ask_user answers.
    #[serde(rename = "user")]
    User { message: AssistantMessage },

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

/// Custom deserializer for content field that accepts both String and Vec<ContentBlock>.
///
/// Claude CLI sometimes sends `content` as a plain string (shortened API Messages format),
/// particularly for replayed user messages with `--replay-user-messages` flag.
/// This affects both `assistant` and `user` events (both reuse AssistantMessage).
///
/// Handles two formats:
/// - String: converts to Vec<ContentBlock> with single Text block
/// - Array: deserializes as Vec<ContentBlock> normally
fn deserialize_content<'de, D>(deserializer: D) -> std::result::Result<Vec<ContentBlock>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct ContentVisitor;

    impl<'de> Visitor<'de> for ContentVisitor {
        type Value = Vec<ContentBlock>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or array of content blocks")
        }

        fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            // Convert plain string to single Text content block
            Ok(vec![ContentBlock::Text {
                text: value.to_string(),
            }])
        }

        fn visit_seq<A>(self, seq: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            // Deserialize as Vec<ContentBlock> normally
            Vec::deserialize(de::value::SeqAccessDeserializer::new(seq))
        }
    }

    deserializer.deserialize_any(ContentVisitor)
}

#[derive(Debug, Deserialize)]
pub struct AssistantMessage {
    #[allow(dead_code)] // Serde field: deserialized from Claude API but not currently used
    pub role: String,
    #[serde(deserialize_with = "deserialize_content")]
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

/// Outcome of the `read_output` loop.
#[derive(Debug, PartialEq)]
pub enum ReadOutcome {
    /// stdout reached EOF (normal completion).
    Eof,
    /// Shutdown flag was set externally.
    Shutdown,
    /// Result event received but stdout didn't close within POST_RESULT_TIMEOUT.
    ResultTimeout,
    /// Per-phase timeout elapsed before receiving a Result event.
    PhaseTimeout,
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
    fn test_claude_event_parse_user() {
        let json = r#"{"type":"user","session_id":"abc-123","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01ABC","content":"Answer"}]}}"#;
        let event: ClaudeEvent = serde_json::from_str(json).unwrap();
        if let ClaudeEvent::User { message } = event {
            assert_eq!(message.role, "user");
            assert_eq!(message.content.len(), 1);
            assert!(matches!(
                &message.content[0],
                ContentBlock::ToolResult { .. }
            ));
        } else {
            panic!("Expected User event");
        }
    }

    #[test]
    fn test_claude_event_parse_user_with_text_and_tool_result() {
        let json = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"User prompt"},{"type":"tool_result","tool_use_id":"toolu_01","content":"Answer"}]}}"#;
        let event: ClaudeEvent = serde_json::from_str(json).unwrap();
        if let ClaudeEvent::User { message } = event {
            assert_eq!(message.content.len(), 2);
            assert!(matches!(&message.content[0], ContentBlock::Text { .. }));
            assert!(matches!(
                &message.content[1],
                ContentBlock::ToolResult { .. }
            ));
        } else {
            panic!("Expected User event");
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
    fn test_read_outcome_enum_variants() {
        let eof = ReadOutcome::Eof;
        let shutdown = ReadOutcome::Shutdown;
        let timeout = ReadOutcome::ResultTimeout;

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

    #[test]
    fn test_read_outcome_phase_timeout_variant() {
        let pt = ReadOutcome::PhaseTimeout;
        assert!(matches!(pt, ReadOutcome::PhaseTimeout));
        assert_eq!(pt, ReadOutcome::PhaseTimeout);
        assert_ne!(pt, ReadOutcome::Eof);
    }

    #[test]
    fn test_send_user_message_struct_serialization() {
        let msg = StdinUserMessage {
            msg_type: "user",
            session_id: "test-session",
            message: StdinMessageContent {
                role: "user",
                content: vec![StdinTextBlock {
                    block_type: "text",
                    text: "test content",
                }],
            },
            parent_tool_use_id: None,
        };

        let json = serde_json::to_string(&msg).expect("Serialization should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("Should parse back");

        assert_eq!(parsed["type"], "user");
        assert_eq!(parsed["session_id"], "test-session");
        assert_eq!(parsed["message"]["role"], "user");
        assert_eq!(parsed["message"]["content"][0]["type"], "text");
        assert_eq!(parsed["message"]["content"][0]["text"], "test content");
        assert!(
            parsed.get("parent_tool_use_id").is_none() || parsed["parent_tool_use_id"].is_null(),
            "None should serialize as null or be omitted"
        );
    }

    // ---------------------------------------------------------------------------
    // Tests for custom content deserialization (string → Vec<ContentBlock>)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_assistant_message_content_as_string() {
        // Test that plain string content deserializes to Vec<ContentBlock>
        let json = r#"{
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": "Hello, world!"
            }
        }"#;

        let event: ClaudeEvent = serde_json::from_str(json)
            .expect("Should deserialize assistant message with string content");

        if let ClaudeEvent::Assistant { message } = event {
            assert_eq!(message.content.len(), 1, "Should have one content block");
            if let ContentBlock::Text { text } = &message.content[0] {
                assert_eq!(text, "Hello, world!");
            } else {
                panic!("Expected ContentBlock::Text, got {:?}", message.content[0]);
            }
        } else {
            panic!("Expected ClaudeEvent::Assistant");
        }
    }

    #[test]
    fn test_assistant_message_content_as_array() {
        // Test that array content still works correctly
        let json = r#"{
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Hello"},
                    {"type": "text", "text": "World"}
                ]
            }
        }"#;

        let event: ClaudeEvent = serde_json::from_str(json)
            .expect("Should deserialize assistant message with array content");

        if let ClaudeEvent::Assistant { message } = event {
            assert_eq!(message.content.len(), 2, "Should have two content blocks");
            if let ContentBlock::Text { text } = &message.content[0] {
                assert_eq!(text, "Hello");
            } else {
                panic!("Expected first block to be Text");
            }
            if let ContentBlock::Text { text } = &message.content[1] {
                assert_eq!(text, "World");
            } else {
                panic!("Expected second block to be Text");
            }
        } else {
            panic!("Expected ClaudeEvent::Assistant");
        }
    }

    #[test]
    fn test_assistant_message_content_empty_array() {
        // Test that empty array works
        let json = r#"{
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": []
            }
        }"#;

        let event: ClaudeEvent = serde_json::from_str(json)
            .expect("Should deserialize assistant message with empty array");

        if let ClaudeEvent::Assistant { message } = event {
            assert_eq!(message.content.len(), 0, "Should have zero content blocks");
        } else {
            panic!("Expected ClaudeEvent::Assistant");
        }
    }

    #[test]
    fn test_user_message_content_as_string() {
        // Test that user event with string content works (replayed messages)
        let json = r#"{
            "type": "user",
            "message": {
                "role": "user",
                "content": "<task-notification>Task 30.1 completed</task-notification>"
            }
        }"#;

        let event: ClaudeEvent = serde_json::from_str(json)
            .expect("Should deserialize user message with string content");

        if let ClaudeEvent::User { message } = event {
            assert_eq!(message.content.len(), 1, "Should have one content block");
            if let ContentBlock::Text { text } = &message.content[0] {
                assert!(text.contains("<task-notification>"));
                assert!(text.contains("Task 30.1 completed"));
            } else {
                panic!("Expected ContentBlock::Text");
            }
        } else {
            panic!("Expected ClaudeEvent::User");
        }
    }

    #[test]
    fn test_user_message_content_mixed_blocks() {
        // Test that mixed content blocks (text + tool_result) still work
        let json = r#"{
            "type": "user",
            "message": {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Here is the result"},
                    {"type": "tool_result", "tool_use_id": "123", "content": {"status": "ok"}}
                ]
            }
        }"#;

        let event: ClaudeEvent = serde_json::from_str(json)
            .expect("Should deserialize user message with mixed content blocks");

        if let ClaudeEvent::User { message } = event {
            assert_eq!(message.content.len(), 2, "Should have two content blocks");
            assert!(matches!(message.content[0], ContentBlock::Text { .. }));
            assert!(matches!(
                message.content[1],
                ContentBlock::ToolResult { .. }
            ));
        } else {
            panic!("Expected ClaudeEvent::User");
        }
    }

    #[test]
    fn test_user_message_task_notification_full() {
        // Full reproduction of the bug: task-notification with XML content as plain string
        let json = r#"{
            "type": "user",
            "session_id": "session_abc123",
            "message": {
                "role": "user",
                "content": "<task-notification>\n<task id=\"30.2\" component=\"run\" status=\"in_progress\">\n<name>Testy deserializacji</name>\n<description>Dodaj testy jednostkowe weryfikujące nową deserializację</description>\n</task>\n</task-notification>"
            }
        }"#;

        let event: ClaudeEvent = serde_json::from_str(json)
            .expect("Should deserialize task-notification (bug reproduction case)");

        if let ClaudeEvent::User { message } = event {
            assert_eq!(message.role, "user");
            assert_eq!(message.content.len(), 1, "Should have one content block");

            if let ContentBlock::Text { text } = &message.content[0] {
                assert!(
                    text.contains("<task-notification>"),
                    "Should contain task-notification XML"
                );
                assert!(text.contains("id=\"30.2\""), "Should contain task ID");
                assert!(
                    text.contains("Testy deserializacji"),
                    "Should contain task name"
                );
            } else {
                panic!("Expected ContentBlock::Text, got {:?}", message.content[0]);
            }
        } else {
            panic!("Expected ClaudeEvent::User, got {:?}", event);
        }
    }
}
