//! Testy jednostkowe dla runner_reader — parsowanie eventów, is_main_result,
//! send_user_message, read_output.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[test]
fn test_process_event_line_result_with_success_subtype() {
    let result_json = r#"{"type":"result","subtype":"success","cost_usd":0.01}"#;
    let mut last_text = None;
    let mut on_event = |_: &ClaudeEvent| {};

    let (is_result, is_ignored) = process_event_line(result_json, &mut last_text, &mut on_event);
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

    let (is_result, is_ignored) = process_event_line(result_json, &mut last_text, &mut on_event);
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

    let (is_result, is_ignored) = process_event_line(result_json, &mut last_text, &mut on_event);
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

    let (is_result, is_ignored) = process_event_line(result_json, &mut last_text, &mut on_event);
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

    let (is_result, is_ignored) = process_event_line(result_json, &mut last_text, &mut on_event);
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

    let (is_result, is_ignored) = process_event_line(assistant_json, &mut last_text, &mut on_event);
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
    let success: ClaudeEvent =
        serde_json::from_str(r#"{"type":"result","subtype":"success","cost_usd":0.01}"#).unwrap();
    assert!(is_main_result(&success));

    let error: ClaudeEvent =
        serde_json::from_str(r#"{"type":"result","subtype":"error","cost_usd":0.0}"#).unwrap();
    assert!(is_main_result(&error));

    let no_subtype: ClaudeEvent =
        serde_json::from_str(r#"{"type":"result","cost_usd":0.0}"#).unwrap();
    assert!(!is_main_result(&no_subtype));

    let unknown: ClaudeEvent =
        serde_json::from_str(r#"{"type":"result","subtype":"something_new","cost_usd":0.0}"#)
            .unwrap();
    assert!(!is_main_result(&unknown));

    let assistant: ClaudeEvent = serde_json::from_str(
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hi"}]}}"#
    ).unwrap();
    assert!(!is_main_result(&assistant));
}

#[test]
fn test_post_result_timeout_constant() {
    assert_eq!(
        POST_RESULT_TIMEOUT,
        std::time::Duration::from_secs(5),
        "POST_RESULT_TIMEOUT should be 5 seconds (reduced after task 32.1)"
    );
}

#[test]
fn test_main_result_subtypes_constant() {
    assert_eq!(
        MAIN_RESULT_SUBTYPES,
        &["success", "error", "turn_limit"],
        "MAIN_RESULT_SUBTYPES should match protocol specification"
    );
}

#[test]
fn test_process_event_line_main_agent_result_returns_true() {
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

        let (is_main_result, _is_ignored) = process_event_line(json, &mut last_text, &mut on_event);
        assert!(
            is_main_result,
            "Main agent Result with subtype '{}' should return true",
            label
        );
    }
}

#[test]
fn test_process_event_line_sub_agent_result_returns_false() {
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

        let (is_main_result, is_ignored) = process_event_line(json, &mut last_text, &mut on_event);
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

#[test]
fn test_process_event_line_result_unknown_subtype_fail_safe() {
    let unknown_subtypes = vec![
        r#"{"type":"result","subtype":"new_feature","cost_usd":0.0}"#,
        r#"{"type":"result","subtype":"experimental","cost_usd":0.0}"#,
        r#"{"type":"result","subtype":"123","cost_usd":0.0}"#,
    ];

    for json in unknown_subtypes {
        let mut last_text = None;
        let mut on_event = |_: &ClaudeEvent| {};

        let (is_main_result, is_ignored) = process_event_line(json, &mut last_text, &mut on_event);
        assert!(
            !is_main_result,
            "Result with unknown subtype should fail-safe to NOT trigger timeout"
        );
        assert!(
            is_ignored,
            "Result with unknown subtype should be marked as ignored"
        );
    }
}

#[test]
fn test_is_main_result_comprehensive() {
    let main_results = vec![
        r#"{"type":"result","subtype":"success","cost_usd":0.01}"#,
        r#"{"type":"result","subtype":"error","cost_usd":0.0}"#,
        r#"{"type":"result","subtype":"turn_limit","cost_usd":0.0}"#,
    ];

    for json in main_results {
        let event: ClaudeEvent = serde_json::from_str(json).unwrap();
        assert!(
            is_main_result(&event),
            "is_main_result should return true for: {}",
            json
        );
    }

    let sub_agent_results = vec![
        r#"{"type":"result","cost_usd":0.01}"#,
        r#"{"type":"result","subtype":"unknown","cost_usd":0.0}"#,
        r#"{"type":"result","subtype":"sub_agent_done","cost_usd":0.0}"#,
    ];

    for json in sub_agent_results {
        let event: ClaudeEvent = serde_json::from_str(json).unwrap();
        assert!(
            !is_main_result(&event),
            "is_main_result should return false for: {}",
            json
        );
    }

    let non_results = vec![
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hi"}]}}"#,
        r#"{"type":"system","subtype":"init"}"#,
    ];

    for json in non_results {
        let event: ClaudeEvent = serde_json::from_str(json).unwrap();
        assert!(
            !is_main_result(&event),
            "is_main_result should return false for non-Result event: {}",
            json
        );
    }
}

#[tokio::test]
async fn test_send_user_message_formats_correctly() {
    use tokio::io::AsyncReadExt;

    let (mut stdin, mut reader) = tokio::io::duplex(1024);

    send_user_message(&mut stdin, "Hello Claude")
        .await
        .expect("send_user_message should succeed");

    drop(stdin);
    let mut buffer = String::new();
    reader
        .read_to_string(&mut buffer)
        .await
        .expect("Should read written data");

    let lines: Vec<&str> = buffer.trim().split('\n').collect();
    assert_eq!(lines.len(), 1, "Should write exactly one line");

    let parsed: serde_json::Value = serde_json::from_str(lines[0]).expect("Should be valid JSON");
    assert_eq!(parsed["type"], "user", "type should be 'user'");
    assert_eq!(parsed["message"]["role"], "user", "role should be 'user'");
    assert_eq!(
        parsed["message"]["content"][0]["type"], "text",
        "content type should be 'text'"
    );
    assert_eq!(
        parsed["message"]["content"][0]["text"], "Hello Claude",
        "text should match input"
    );
}

#[tokio::test]
async fn test_send_user_message_multiple_calls() {
    use tokio::io::AsyncReadExt;

    let (mut stdin, mut reader) = tokio::io::duplex(2048);

    send_user_message(&mut stdin, "First message")
        .await
        .expect("First send should succeed");
    send_user_message(&mut stdin, "Second message")
        .await
        .expect("Second send should succeed");

    drop(stdin);
    let mut buffer = String::new();
    reader
        .read_to_string(&mut buffer)
        .await
        .expect("Should read written data");

    let lines: Vec<&str> = buffer.trim().split('\n').collect();
    assert_eq!(lines.len(), 2, "Should write two lines");

    let first: serde_json::Value =
        serde_json::from_str(lines[0]).expect("First line should be valid JSON");
    let second: serde_json::Value =
        serde_json::from_str(lines[1]).expect("Second line should be valid JSON");

    assert_eq!(
        first["message"]["content"][0]["text"], "First message",
        "First message should match"
    );
    assert_eq!(
        second["message"]["content"][0]["text"], "Second message",
        "Second message should match"
    );
}

#[tokio::test]
async fn test_send_user_message_empty_string() {
    use tokio::io::AsyncReadExt;

    let (mut stdin, mut reader) = tokio::io::duplex(512);

    send_user_message(&mut stdin, "")
        .await
        .expect("Empty string should be valid");

    drop(stdin);
    let mut buffer = String::new();
    reader.read_to_string(&mut buffer).await.ok();

    let parsed: serde_json::Value =
        serde_json::from_str(buffer.trim()).expect("Should be valid JSON");
    assert_eq!(
        parsed["message"]["content"][0]["text"], "",
        "Empty string should be preserved"
    );
}

#[tokio::test]
async fn test_send_user_message_special_characters() {
    use tokio::io::AsyncReadExt;

    let (mut stdin, mut reader) = tokio::io::duplex(1024);

    let special_text = "Text with \"quotes\", \nnewlines\n, and 🚀 emoji";
    send_user_message(&mut stdin, special_text)
        .await
        .expect("Special characters should be handled");

    drop(stdin);
    let mut buffer = String::new();
    reader.read_to_string(&mut buffer).await.ok();

    let parsed: serde_json::Value =
        serde_json::from_str(buffer.trim()).expect("Should be valid JSON");
    assert_eq!(
        parsed["message"]["content"][0]["text"], special_text,
        "Special characters should be preserved"
    );
}

#[tokio::test]
async fn test_send_user_message_json_format_verification() {
    use tokio::io::AsyncReadExt;

    let (mut stdin, mut reader) = tokio::io::duplex(2048);

    let test_message = "Test prompt with special chars: \"quotes\", \nnewlines\n, emoji 🎯";
    send_user_message(&mut stdin, test_message)
        .await
        .expect("send_user_message should succeed");

    drop(stdin);
    let mut buffer = String::new();
    reader.read_to_string(&mut buffer).await.ok();

    let parsed: serde_json::Value =
        serde_json::from_str(buffer.trim()).expect("Should be valid JSON");

    assert_eq!(parsed["type"], "user", "type field must be 'user'");
    assert_eq!(parsed["session_id"], "", "session_id must be empty string");
    assert!(
        parsed.get("parent_tool_use_id").is_none() || parsed["parent_tool_use_id"].is_null(),
        "parent_tool_use_id must be null or omitted"
    );

    let message = &parsed["message"];
    assert_eq!(message["role"], "user", "message.role must be 'user'");

    let content = message["content"]
        .as_array()
        .expect("content must be array");
    assert_eq!(
        content.len(),
        1,
        "content array must have exactly 1 element"
    );

    let text_block = &content[0];
    assert_eq!(text_block["type"], "text", "block type must be 'text'");
    assert_eq!(
        text_block["text"], test_message,
        "text content must match input"
    );

    assert_eq!(
        parsed.as_object().unwrap().keys().count(),
        4,
        "Should have exactly 4 top-level fields"
    );
}

// ---------------------------------------------------------------------------
// Testy async read_output
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_read_output_normal_flow() {
    use std::io::Cursor;

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

    let result = read_output(
        &mut reader,
        &shutdown,
        &mut on_event,
        &mut on_idle,
        None,
        &mut None,
        None,
    )
    .await;

    assert!(result.is_ok(), "read_output should succeed");
    let (text, outcome, sub_agent_count) = result.unwrap();

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

    let shutdown_clone = Arc::clone(&shutdown);
    tokio::spawn(async move {
        let result_json = r#"{"type":"result","subtype":"success","cost_usd":0.01}"#;
        writer.write_all(result_json.as_bytes()).await.ok();
        writer.write_all(b"\n").await.ok();
        writer.flush().await.ok();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        shutdown_clone.store(true, Ordering::SeqCst);
    });

    let result = read_output(
        &mut reader,
        &shutdown,
        &mut on_event,
        &mut on_idle,
        None,
        &mut None,
        None,
    )
    .await;

    assert!(result.is_ok(), "read_output should succeed even on timeout");
    let (_text, outcome, sub_agent_count) = result.unwrap();

    assert!(
        matches!(
            outcome,
            ReadOutcome::Eof | ReadOutcome::Shutdown | ReadOutcome::ResultTimeout
        ),
        "Should return valid outcome (Eof/Shutdown/ResultTimeout), got {:?}",
        outcome
    );
    assert!(!events.is_empty(), "Should have processed Result event");
    assert_eq!(
        sub_agent_count, 0,
        "Main result should not increment sub-agent counter"
    );
}

#[tokio::test]
async fn test_read_output_with_sub_agent_results() {
    use std::io::Cursor;

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

    let result = read_output(
        &mut reader,
        &shutdown,
        &mut on_event,
        &mut on_idle,
        None,
        &mut None,
        None,
    )
    .await;

    assert!(result.is_ok(), "read_output should succeed");
    let (_text, outcome, sub_agent_count) = result.unwrap();

    assert!(matches!(outcome, ReadOutcome::Eof), "Should reach EOF");
    assert_eq!(
        sub_agent_count, 1,
        "Should count 1 sub-agent Result (no subtype)"
    );
    assert_eq!(events.len(), 3, "Should have processed 3 events");
}

#[tokio::test]
async fn test_read_output_sequence_sub_agent_then_main_result() {
    use std::io::Cursor;

    let data = concat!(
        r#"{"type":"result","cost_usd":0.005}"#,
        "\n",
        r#"{"type":"result","subtype":"sub_agent_done","cost_usd":0.003}"#,
        "\n",
        r#"{"type":"result","subtype":"success","cost_usd":0.01}"#,
        "\n"
    );

    let cursor = Cursor::new(data.as_bytes());
    let mut reader = BufReader::new(cursor).lines();
    let shutdown = AtomicBool::new(false);
    let mut result_events = Vec::new();
    let mut on_event = |event: &ClaudeEvent| {
        if matches!(event, ClaudeEvent::Result { .. }) {
            result_events.push(is_main_result(event));
        }
    };
    let mut on_idle = || {};

    let result = read_output(
        &mut reader,
        &shutdown,
        &mut on_event,
        &mut on_idle,
        None,
        &mut None,
        None,
    )
    .await;

    assert!(result.is_ok(), "read_output should succeed");
    let (_text, outcome, _count) = result.unwrap();

    assert!(
        matches!(outcome, ReadOutcome::Eof),
        "Sequence ending with main Result + EOF should return Eof, got {:?}",
        outcome
    );

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

    let result = read_output(
        &mut reader,
        &shutdown,
        &mut on_event,
        &mut on_idle,
        None,
        &mut None,
        None,
    )
    .await;

    assert!(result.is_ok(), "read_output should succeed");
    let (_text, outcome, _count) = result.unwrap();

    assert!(
        matches!(outcome, ReadOutcome::Eof),
        "Main Result + EOF should return Eof, got {:?}",
        outcome
    );
    assert_eq!(result_count, 1, "Should process exactly 1 Result event");
}

#[tokio::test]
async fn test_read_output_phase_timeout_fires() {
    let (_writer, reader) = tokio::io::duplex(1024);
    let mut reader = BufReader::new(reader).lines();
    let shutdown = AtomicBool::new(false);
    let mut on_event = |_: &ClaudeEvent| {};
    let mut on_idle = || {};

    let result = read_output(
        &mut reader,
        &shutdown,
        &mut on_event,
        &mut on_idle,
        Some(std::time::Duration::from_millis(50)),
        &mut None,
        None,
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

    let data = concat!(
        r#"{"type":"result","subtype":"success","cost_usd":0.01}"#,
        "\n"
    );
    let cursor = Cursor::new(data.as_bytes());
    let mut reader = BufReader::new(cursor).lines();
    let shutdown = AtomicBool::new(false);
    let mut on_event = |_: &ClaudeEvent| {};
    let mut on_idle = || {};

    let result = read_output(
        &mut reader,
        &shutdown,
        &mut on_event,
        &mut on_idle,
        None,
        &mut None,
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

    let data = concat!(
        r#"{"type":"result","subtype":"success","cost_usd":0.01}"#,
        "\n"
    );
    let cursor = Cursor::new(data.as_bytes());
    let mut reader = BufReader::new(cursor).lines();
    let shutdown = AtomicBool::new(false);
    let mut on_event = |_: &ClaudeEvent| {};
    let mut on_idle = || {};

    let result = read_output(
        &mut reader,
        &shutdown,
        &mut on_event,
        &mut on_idle,
        Some(std::time::Duration::from_secs(3600)),
        &mut None,
        None,
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

#[tokio::test]
async fn test_read_output_with_message_channel() {
    use std::io::Cursor;

    let data = concat!(
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Response"}]}}"#,
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

    let (tx, rx) = tokio::sync::mpsc::channel(10);

    let tx_clone = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        tx_clone.send("Additional prompt".to_string()).await.ok();
    });

    let result = read_output(
        &mut reader,
        &shutdown,
        &mut on_event,
        &mut on_idle,
        None,
        &mut None,
        Some(rx),
    )
    .await;

    assert!(result.is_ok(), "read_output should succeed");
    let (_text, outcome, _count) = result.unwrap();
    assert_eq!(outcome, ReadOutcome::Eof);
    assert_eq!(events.len(), 2, "Should process both events");
}

#[tokio::test]
async fn test_read_output_backward_compat_stdin_closed() {
    use std::io::Cursor;

    let data = concat!(
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done"}]}}"#,
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

    let result = read_output(
        &mut reader,
        &shutdown,
        &mut on_event,
        &mut on_idle,
        None,
        &mut None,
        None,
    )
    .await;

    assert!(result.is_ok(), "Non-interactive mode should still work");
    let (_text, outcome, _count) = result.unwrap();
    assert_eq!(
        outcome,
        ReadOutcome::Eof,
        "Non-interactive mode should complete normally"
    );
    assert_eq!(events.len(), 2, "Should process both events");
}

#[tokio::test]
async fn test_read_output_interactive_with_duplex_pipe() {
    let (mut writer, reader) = tokio::io::duplex(4096);
    let mut reader = BufReader::new(reader).lines();
    let shutdown = AtomicBool::new(false);
    let mut events = Vec::new();
    let mut on_event = |event: &ClaudeEvent| {
        events.push(format!("{:?}", event));
    };
    let mut on_idle = || {};

    let (_tx, rx) = tokio::sync::mpsc::channel(10);

    tokio::spawn(async move {
        writer
            .write_all(
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Thinking"}]}}"#
                    .as_bytes(),
            )
            .await
            .ok();
        writer.write_all(b"\n").await.ok();
        writer.flush().await.ok();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        writer
            .write_all(r#"{"type":"result","subtype":"success","cost_usd":0.01}"#.as_bytes())
            .await
            .ok();
        writer.write_all(b"\n").await.ok();
        writer.flush().await.ok();
    });

    let mut stdin_opt: Option<tokio::process::ChildStdin> = None;

    let result = read_output(
        &mut reader,
        &shutdown,
        &mut on_event,
        &mut on_idle,
        None,
        &mut stdin_opt,
        Some(rx),
    )
    .await;

    assert!(result.is_ok(), "read_output should succeed");
    let (_text, outcome, _count) = result.unwrap();
    assert_eq!(
        outcome,
        ReadOutcome::Eof,
        "Interactive mode should complete normally"
    );
    assert_eq!(
        events.len(),
        2,
        "Should process both assistant and result events"
    );
}
