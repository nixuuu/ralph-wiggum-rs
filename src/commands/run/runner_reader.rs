//! Logika czytania i parsowania stdout procesu Claude CLI.
//!
//! Zawiera `read_output()` — główną pętlę odczytu stream-json eventów,
//! `process_event_line()` — parsowanie pojedynczej linii JSON,
//! `send_user_message()` — wysyłanie wiadomości via stdin,
//! oraz `handle_completion()` — obsługę zakończenia procesu.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncWriteExt, BufReader};

use crate::shared::error::{RalphError, Result};

use super::runner_types::{
    ClaudeEvent, ContentBlock, ReadOutcome, StdinContentBlock, StdinMessageContent, StdinTextBlock,
    StdinUserMessage,
};

/// Post-Result timeout: if stdout doesn't close within this time after receiving
/// a Result event, break the read loop and proceed to graceful shutdown.
///
/// Reduced from 30s to 5s after task 32.1 (stdin drop fix). With stdin properly
/// closed after non-interactive messages, Claude CLI should close stdout quickly
/// (< 1s) after Result event. 5s provides adequate margin for edge cases.
pub(crate) const POST_RESULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Timeout for waiting for process exit after stdout closes.
const PROCESS_EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Recognized Result subtypes that indicate the main agent has finished.
/// Only these trigger the post-Result timeout.
const MAIN_RESULT_SUBTYPES: &[&str] = &["success", "error", "turn_limit"];

/// Check if a Result event represents a top-level (main agent) result.
///
/// A main agent Result has a recognized `subtype` (e.g. "success", "error").
/// Result events without a subtype or with an unrecognized subtype are
/// treated as non-terminal and do not trigger the post-Result timeout.
pub(crate) fn is_main_result(event: &ClaudeEvent) -> bool {
    matches!(
        event,
        ClaudeEvent::Result { subtype: Some(s), .. }
            if MAIN_RESULT_SUBTYPES.contains(&s.as_str())
    )
}

/// Parse a single JSON line, update last assistant text, and invoke callback.
///
/// Returns `(is_main_result, is_ignored_sub_agent_result)` where:
/// - `is_main_result`: true if this is a top-level Result event (subtype: success/error/turn_limit)
/// - `is_ignored_sub_agent_result`: true if this is a sub-agent Result (no subtype or unrecognized)
pub(crate) fn process_event_line<F>(
    line: &str,
    last_assistant_text: &mut Option<String>,
    on_event: &mut F,
) -> (bool, bool)
where
    F: FnMut(&ClaudeEvent),
{
    match serde_json::from_str::<ClaudeEvent>(line) {
        Ok(event) => {
            // Log event type for debugging
            let event_type = match &event {
                ClaudeEvent::Assistant { .. } => "assistant",
                ClaudeEvent::Result { .. } => "result",
                ClaudeEvent::User { .. } => "user",
                ClaudeEvent::System { .. } => "system",
                ClaudeEvent::Other => "other",
            };
            crate::diag_debug!("Event received: type={}", event_type);

            let is_result = is_main_result(&event);
            let is_ignored_sub_agent = matches!(event, ClaudeEvent::Result { .. }) && !is_result;

            // Log Result events with subtype
            if let ClaudeEvent::Result { subtype, .. } = &event {
                crate::diag_debug!("Result event received, subtype={:?}", subtype);
            }

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
            crate::diag_warn!("Failed to parse JSON line: {}", e);
            crate::diag_debug!("Unparsed line: {}", &line[..line.len().min(200)]);
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
/// `stdin` and `message_rx` are for interactive mode: when both are provided
/// (Some), the loop can receive additional user messages via the channel and
/// send them to Claude via stdin using `send_user_message()`.
///
/// Returns the last assistant text, the outcome describing how the loop ended,
/// and the count of ignored sub-agent Result events.
pub(crate) async fn read_output<R, F, I>(
    reader: &mut tokio::io::Lines<BufReader<R>>,
    shutdown: &AtomicBool,
    on_event: &mut F,
    on_idle: &mut I,
    phase_timeout: Option<std::time::Duration>,
    stdin: &mut Option<tokio::process::ChildStdin>,
    mut message_rx: Option<tokio::sync::mpsc::Receiver<String>>,
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

            // Receive additional user messages from channel (interactive mode)
            Some(msg) = async {
                match &mut message_rx {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                // Send message to stdin if available
                if let Some(stdin_handle) = stdin.as_mut()
                    && let Err(e) = send_user_message(stdin_handle, &msg).await
                {
                    crate::diag_warn!("Failed to send user message to stdin: {}", e);
                }
            }

            line_result = reader.next_line() => {
                match line_result? {
                    Some(line) if !line.is_empty() => {
                        let (is_result, is_ignored_sub_agent) = process_event_line(
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
pub(crate) async fn handle_completion(
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    last_assistant_text: Option<String>,
) -> Result<Option<String>> {
    let status = match tokio::time::timeout(PROCESS_EXIT_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => {
            crate::diag_debug!(
                "Claude process exited with status: {:?} (PID={:?})",
                status,
                child_pid
            );
            status
        }
        Ok(Err(e)) => return Err(RalphError::Io(e)),
        Err(_elapsed) => {
            crate::diag_warn!(
                "Claude process did not exit within {}s after closing stdout (PID={:?})",
                PROCESS_EXIT_TIMEOUT.as_secs(),
                child_pid
            );
            super::runner::graceful_shutdown(child, child_pid).await;
            if last_assistant_text.is_some() {
                return Ok(last_assistant_text);
            }
            return Err(RalphError::ClaudeProcess(format!(
                "claude process did not exit within {}s after closing stdout",
                PROCESS_EXIT_TIMEOUT.as_secs()
            )));
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

/// Send a user message via stdin to an already-running Claude process.
///
/// Formats the message as a StdinUserMessage JSON event and writes it to stdin.
/// This allows sending additional prompts or follow-up messages after the initial
/// message has been sent.
#[allow(dead_code)] // Public API — będzie użyte w orchestratorze
pub async fn send_user_message<W>(stdin: &mut W, text: &str) -> std::io::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let user_msg = StdinUserMessage {
        msg_type: "user",
        session_id: "",
        message: StdinMessageContent {
            role: "user",
            content: vec![StdinContentBlock::Text(StdinTextBlock {
                block_type: "text",
                text,
            })],
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

#[cfg(test)]
#[path = "runner_reader_tests.rs"]
mod tests;
