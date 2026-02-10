#![allow(dead_code)]
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::commands::run::runner::{ClaudeEvent, ClaudeRunner, ContentBlock};
use crate::commands::task::orchestrate::events::{WorkerEvent, WorkerEventKind, WorkerPhase};
use crate::shared::error::Result;

/// Adapted ClaudeRunner for orchestration workers.
///
/// Wraps ClaudeRunner to forward events through an mpsc channel
/// instead of rendering to terminal. Each phase is a one-shot
/// Claude invocation in the worker's worktree directory.
pub struct WorkerRunner {
    worker_id: u32,
    task_id: String,
    event_tx: mpsc::Sender<WorkerEvent>,
    shutdown: Arc<AtomicBool>,
}

impl WorkerRunner {
    pub fn new(
        worker_id: u32,
        task_id: String,
        event_tx: mpsc::Sender<WorkerEvent>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            worker_id,
            task_id,
            event_tx,
            shutdown,
        }
    }

    /// Run a single phase of the worker lifecycle.
    ///
    /// Invokes Claude CLI as a one-shot in the given working directory,
    /// forwarding cost/token events through the mpsc channel.
    /// Returns the assistant's text output.
    pub async fn run_phase(
        &self,
        phase: WorkerPhase,
        prompt: &str,
        model: Option<&str>,
        cwd: &Path,
    ) -> Result<String> {
        // Notify phase start
        self.send_event(WorkerEventKind::PhaseStarted {
            worker_id: self.worker_id,
            task_id: self.task_id.clone(),
            phase: phase.clone(),
        })
        .await;

        let runner = ClaudeRunner::oneshot(
            prompt.to_string(),
            model.map(|s| s.to_string()),
            Some(cwd.to_path_buf()),
        );

        // Track cost/tokens across events for this phase
        let worker_id = self.worker_id;
        let tx = self.event_tx.clone();

        let result = runner
            .run(
                self.shutdown.clone(),
                |event| {
                    // Forward cost updates from Claude Result events
                    if let ClaudeEvent::Result {
                        cost_usd,
                        usage,
                        ..
                    } = event
                    {
                        let (input_tokens, output_tokens) = usage
                            .as_ref()
                            .map(|u| (u.input_tokens, u.output_tokens))
                            .unwrap_or((0, 0));

                        let cost_event = WorkerEvent::new(WorkerEventKind::CostUpdate {
                            worker_id,
                            cost_usd: cost_usd.unwrap_or(0.0),
                            input_tokens,
                            output_tokens,
                        });
                        // Best-effort send — don't block on channel full
                        let _ = tx.try_send(cost_event);
                    }
                },
                || {},
            )
            .await;

        let success = result.is_ok();
        let output = match result {
            Ok(Some(text)) => text,
            Ok(None) => String::new(),
            Err(e) => {
                self.send_event(WorkerEventKind::PhaseCompleted {
                    worker_id: self.worker_id,
                    task_id: self.task_id.clone(),
                    phase: phase.clone(),
                    success: false,
                })
                .await;
                return Err(e);
            }
        };

        self.send_event(WorkerEventKind::PhaseCompleted {
            worker_id: self.worker_id,
            task_id: self.task_id.clone(),
            phase,
            success,
        })
        .await;

        Ok(output)
    }

    /// Run the implement phase with task-specific prompt.
    pub async fn run_implement(
        &self,
        task_desc: &str,
        system_prompt: &str,
        model: Option<&str>,
        cwd: &Path,
    ) -> Result<String> {
        let prompt = format!(
            "{system_prompt}\n\n---\n\n\
             # Your Task\n\
             {task_desc}\n\n\
             You are working in an isolated git worktree. Focus only on this task.\n\
             Do not modify files outside the scope of this task.\n\
             Commit your changes when done."
        );
        self.run_phase(WorkerPhase::Implement, &prompt, model, cwd)
            .await
    }

    /// Run the review+fix phase with implementation output from phase 1.
    pub async fn run_review(
        &self,
        implementation_output: &str,
        task_desc: &str,
        model: Option<&str>,
        cwd: &Path,
    ) -> Result<String> {
        let prompt = format!(
            "# Self-Review Task\n\n\
             Review your own implementation and fix any issues.\n\n\
             ## Original Task\n{task_desc}\n\n\
             ## Implementation Output\n{implementation_output}\n\n\
             Review the code changes you made. Look for:\n\
             - Bugs or logic errors\n\
             - Missing edge cases\n\
             - Code style issues\n\
             - Incomplete implementations\n\n\
             Fix any issues found and commit your changes."
        );
        self.run_phase(WorkerPhase::ReviewFix, &prompt, model, cwd)
            .await
    }

    /// Run the verify phase — checks that the implementation passes verification.
    pub async fn run_verify(
        &self,
        verification_commands: &str,
        model: Option<&str>,
        cwd: &Path,
    ) -> Result<bool> {
        let prompt = format!(
            "# Verification Task\n\n\
             Run the following verification commands and report results.\n\
             If all commands pass, respond with 'VERIFICATION PASSED'.\n\
             If any command fails, respond with 'VERIFICATION FAILED' and explain why.\n\n\
             ## Commands\n{verification_commands}"
        );
        let output = self
            .run_phase(WorkerPhase::Verify, &prompt, model, cwd)
            .await?;

        // Simple check: look for pass/fail indicators in output
        let passed = output.contains("VERIFICATION PASSED")
            || (!output.contains("VERIFICATION FAILED") && !output.contains("FAILED"));

        Ok(passed)
    }

    /// Extract text content from a ClaudeEvent's assistant message.
    pub fn extract_text(event: &ClaudeEvent) -> Option<String> {
        if let ClaudeEvent::Assistant { message } = event {
            let texts: Vec<&str> = message
                .content
                .iter()
                .filter_map(|block| {
                    if let ContentBlock::Text { text } = block {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        } else {
            None
        }
    }

    async fn send_event(&self, kind: WorkerEventKind) {
        let event = WorkerEvent::new(kind);
        let _ = self.event_tx.send(event).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_runner_creation() {
        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let runner = WorkerRunner::new(1, "T01".to_string(), tx, shutdown);
        assert_eq!(runner.worker_id, 1);
        assert_eq!(runner.task_id, "T01");
    }

    #[tokio::test]
    async fn test_send_event() {
        let (tx, mut rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let runner = WorkerRunner::new(1, "T01".to_string(), tx, shutdown);

        runner
            .send_event(WorkerEventKind::TaskStarted {
                worker_id: 1,
                task_id: "T01".to_string(),
            })
            .await;

        let event = rx.recv().await.unwrap();
        assert!(matches!(event.kind, WorkerEventKind::TaskStarted { .. }));
    }

    #[test]
    fn test_extract_text_from_assistant_event() {
        use crate::commands::run::runner::{AssistantMessage, ContentBlock};

        let event = ClaudeEvent::Assistant {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Hello world".to_string(),
                }],
                usage: None,
            },
        };
        let text = WorkerRunner::extract_text(&event);
        assert_eq!(text.as_deref(), Some("Hello world"));
    }

    #[test]
    fn test_extract_text_from_non_assistant_event() {
        let event = ClaudeEvent::Other;
        assert!(WorkerRunner::extract_text(&event).is_none());
    }
}
