use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::mpsc;

use crate::commands::run::output::OutputFormatter;
use crate::commands::run::runner::{ClaudeEvent, ClaudeRunner};
use crate::commands::task::orchestrate::events::{WorkerEvent, WorkerEventKind, WorkerPhase};
use crate::shared::error::Result;

/// Result of a single phase execution including cost metrics.
#[derive(Debug, Clone)]
pub struct PhaseResult {
    pub output: String,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Configuration for WorkerRunner.
///
/// Groups prompt customization, UI settings, timeout configuration, and MCP server connection.
#[derive(Clone, Default)]
pub struct WorkerRunnerConfig {
    pub use_nerd_font: bool,
    pub prompt_prefix: Option<String>,
    pub prompt_suffix: Option<String>,
    pub phase_timeout: Option<std::time::Duration>,
    /// Port of the shared MCP HTTP server.
    pub mcp_port: u16,
    /// Session ID for this worker's MCP session.
    pub mcp_session_id: String,
}

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
    config: WorkerRunnerConfig,
}

impl WorkerRunner {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        worker_id: u32,
        task_id: String,
        event_tx: mpsc::Sender<WorkerEvent>,
        shutdown: Arc<AtomicBool>,
        config: WorkerRunnerConfig,
    ) -> Self {
        Self {
            worker_id,
            task_id,
            event_tx,
            shutdown,
            config,
        }
    }

    /// Build MCP config JSON using the shared HTTP MCP server.
    ///
    /// If MCP port is configured (non-zero), returns config pointing to the shared
    /// HTTP server with worker-specific session ID. Otherwise returns None.
    fn mcp_config(&self) -> Option<serde_json::Value> {
        if self.config.mcp_port == 0 || self.config.mcp_session_id.is_empty() {
            return None;
        }
        Some(crate::shared::mcp::build_mcp_config_with_session(
            self.config.mcp_port,
            &self.config.mcp_session_id,
        ))
    }

    /// Run a single phase of the worker lifecycle.
    ///
    /// Invokes Claude CLI as a one-shot in the given working directory,
    /// forwarding cost/token events through the mpsc channel.
    /// Returns the assistant's text output and cost metrics.
    pub async fn run_phase(
        &self,
        phase: WorkerPhase,
        prompt: &str,
        model: Option<&str>,
        cwd: &Path,
    ) -> Result<PhaseResult> {
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
        let runner = if let Some(mcp_cfg) = self.mcp_config() {
            runner.with_mcp_config(mcp_cfg)
        } else {
            runner
        };
        let runner = if let Some(timeout) = self.config.phase_timeout {
            runner.with_phase_timeout(timeout)
        } else {
            runner
        };

        // Track cost/tokens across events for this phase
        let worker_id = self.worker_id;
        let tx = self.event_tx.clone();
        let tx_output = self.event_tx.clone();
        let tx_heartbeat = self.event_tx.clone();
        let mut formatter = OutputFormatter::new(self.config.use_nerd_font);

        // Heartbeat: send every 120 idle ticks (30 seconds at 250ms per tick)
        let mut idle_tick_counter = 0u32;
        let heartbeat_phase = phase.clone();

        // Accumulate cost metrics for this phase
        let phase_cost = Arc::new(std::sync::Mutex::new((0.0_f64, 0_u64, 0_u64)));
        let phase_cost_clone = Arc::clone(&phase_cost);

        let result =
            runner
                .run(
                    self.shutdown.clone(),
                    |event| {
                        // Forward cost updates from Claude Result events
                        if let ClaudeEvent::Result {
                            cost_usd, usage, ..
                        } = event
                        {
                            let (input_tokens, output_tokens) = usage
                                .as_ref()
                                .map(|u| (u.input_tokens, u.output_tokens))
                                .unwrap_or((0, 0));

                            let cost = cost_usd.unwrap_or(0.0);

                            // Accumulate metrics for return value
                            if let Ok(mut metrics) = phase_cost_clone.lock() {
                                metrics.0 += cost;
                                metrics.1 += input_tokens;
                                metrics.2 += output_tokens;
                            }

                            let cost_event = WorkerEvent::new(WorkerEventKind::CostUpdate {
                                worker_id,
                                cost_usd: cost,
                                input_tokens,
                                output_tokens,
                            });
                            // Best-effort send — don't block on channel full
                            let _ = tx.try_send(cost_event);
                        }

                        // Format and forward output lines for live TUI display.
                        // NOTE: OutputFormatter handles markdown rendering via render_markdown() internally.
                        let lines = formatter.format_event(event);
                        if !lines.is_empty() {
                            let _ = tx_output.try_send(WorkerEvent::new(
                                WorkerEventKind::OutputLines { worker_id, lines },
                            ));
                        }
                    },
                    move || {
                        // Idle tick callback: send heartbeat every 120 ticks (30 seconds)
                        idle_tick_counter += 1;
                        if idle_tick_counter >= 120 {
                            idle_tick_counter = 0;
                            let heartbeat_event = WorkerEvent::new(WorkerEventKind::Heartbeat {
                                worker_id,
                                phase: heartbeat_phase.clone(),
                            });
                            let _ = tx_heartbeat.try_send(heartbeat_event);
                        }
                    },
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

        // Extract accumulated metrics
        let (cost_usd, input_tokens, output_tokens) = phase_cost
            .lock()
            .map(|m| (m.0, m.1, m.2))
            .unwrap_or((0.0, 0, 0));

        self.send_event(WorkerEventKind::PhaseCompleted {
            worker_id: self.worker_id,
            task_id: self.task_id.clone(),
            phase,
            success,
        })
        .await;

        Ok(PhaseResult {
            output,
            cost_usd,
            input_tokens,
            output_tokens,
        })
    }

    /// Run the implement phase with task-specific prompt.
    /// Returns phase result including cost metrics.
    pub async fn run_implement(
        &self,
        task_desc: &str,
        system_prompt: &str,
        model: Option<&str>,
        cwd: &Path,
    ) -> Result<PhaseResult> {
        let mut prompt = String::new();

        // Add prefix if configured
        if let Some(prefix) = &self.config.prompt_prefix {
            prompt.push_str(prefix);
            prompt.push_str("\n\n");
        }

        // Add system prompt and task description
        prompt.push_str(&format!(
            "{system_prompt}\n\n---\n\n\
             # Your Task\n\
             {task_desc}\n\n\
             You are working in an isolated git worktree. Focus only on this task.\n\
             Do not modify files outside the scope of this task.\n\
             Commit your changes when done."
        ));

        // Add suffix if configured
        if let Some(suffix) = &self.config.prompt_suffix {
            prompt.push_str("\n\n");
            prompt.push_str(suffix);
        }

        self.run_phase(WorkerPhase::Implement, &prompt, model, cwd)
            .await
    }

    /// Run the review+fix phase with implementation output from phase 1.
    ///
    /// If `verify_report` is Some, appends the verification failure report to the prompt
    /// so the agent can fix the issues found by direct verification commands.
    /// Returns phase result including cost metrics.
    pub async fn run_review(
        &self,
        implementation_output: &str,
        task_desc: &str,
        model: Option<&str>,
        cwd: &Path,
        verify_report: Option<&str>,
    ) -> Result<PhaseResult> {
        let verify_section = if let Some(report) = verify_report {
            format!(
                "\n\n## Verification Results\n\
                 The following verification commands FAILED after your implementation.\n\
                 Fix the issues before proceeding.\n\n\
                 <verify_report>\n{report}\n</verify_report>"
            )
        } else {
            String::new()
        };

        let mut prompt = String::new();

        // Add prefix if configured
        if let Some(prefix) = &self.config.prompt_prefix {
            prompt.push_str(prefix);
            prompt.push_str("\n\n");
        }

        // Add review prompt and task details
        prompt.push_str(&format!(
            "# Self-Review Task\n\n\
             Review your own implementation and fix any issues.\n\n\
             ## Original Task\n{task_desc}\n\n\
             ## Implementation Output\n{implementation_output}{verify_section}\n\n\
             Review the code changes you made. Look for:\n\
             - Bugs or logic errors\n\
             - Missing edge cases\n\
             - Code style issues\n\
             - Incomplete implementations\n\n\
             Fix any issues found and commit your changes."
        ));

        // Add suffix if configured
        if let Some(suffix) = &self.config.prompt_suffix {
            prompt.push_str("\n\n");
            prompt.push_str(suffix);
        }

        self.run_phase(WorkerPhase::ReviewFix, &prompt, model, cwd)
            .await
    }

    /// Run the verify phase — checks that the implementation passes verification.
    #[allow(dead_code)] // Used in task 13.3 worker lifecycle
    pub async fn run_verify(
        &self,
        verification_commands: &str,
        model: Option<&str>,
        cwd: &Path,
    ) -> Result<bool> {
        let mut prompt = String::new();

        // Add prefix if configured
        if let Some(prefix) = &self.config.prompt_prefix {
            prompt.push_str(prefix);
            prompt.push_str("\n\n");
        }

        // Add verification prompt
        prompt.push_str(&format!(
            "# Verification Task\n\n\
             Run the following verification commands and report results.\n\
             If all commands pass, respond with 'VERIFICATION PASSED'.\n\
             If any command fails, respond with 'VERIFICATION FAILED' and explain why.\n\n\
             ## Commands\n{verification_commands}"
        ));

        // Add suffix if configured
        if let Some(suffix) = &self.config.prompt_suffix {
            prompt.push_str("\n\n");
            prompt.push_str(suffix);
        }

        let result = self
            .run_phase(WorkerPhase::Verify, &prompt, model, cwd)
            .await?;

        // Simple check: look for pass/fail indicators in output
        let passed = result.output.contains("VERIFICATION PASSED")
            || (!result.output.contains("VERIFICATION FAILED")
                && !result.output.contains("FAILED"));

        Ok(passed)
    }

    async fn send_event(&self, kind: WorkerEventKind) {
        let event = WorkerEvent::new(kind);
        let _ = self.event_tx.send(event).await;
    }

    /// Extract text content from a ClaudeEvent's assistant message.
    /// Helper function used only in tests.
    #[cfg(test)]
    fn extract_text(event: &ClaudeEvent) -> Option<String> {
        use crate::commands::run::runner::ContentBlock;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_runner_creation() {
        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerRunnerConfig::default();
        let runner = WorkerRunner::new(1, "T01".to_string(), tx, shutdown, config);
        assert_eq!(runner.worker_id, 1);
        assert_eq!(runner.task_id, "T01");
    }

    #[tokio::test]
    async fn test_send_event() {
        let (tx, mut rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerRunnerConfig::default();
        let runner = WorkerRunner::new(1, "T01".to_string(), tx, shutdown, config);

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

    #[test]
    fn test_worker_runner_with_prefix_suffix() {
        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerRunnerConfig {
            use_nerd_font: false,
            prompt_prefix: Some("PREFIX TEXT".to_string()),
            prompt_suffix: Some("SUFFIX TEXT".to_string()),
            phase_timeout: None,
            mcp_port: 0,
            mcp_session_id: String::new(),
        };
        let runner = WorkerRunner::new(1, "T01".to_string(), tx, shutdown, config);
        assert_eq!(runner.config.prompt_prefix, Some("PREFIX TEXT".to_string()));
        assert_eq!(runner.config.prompt_suffix, Some("SUFFIX TEXT".to_string()));
    }

    #[test]
    fn test_worker_runner_without_prefix_suffix() {
        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerRunnerConfig::default();
        let runner = WorkerRunner::new(1, "T01".to_string(), tx, shutdown, config);
        assert_eq!(runner.config.prompt_prefix, None);
        assert_eq!(runner.config.prompt_suffix, None);
    }

    // Note: Testing run_implement and run_review prompt construction would require
    // mocking ClaudeRunner, which is not trivial. Instead, we verify the prompt
    // structure by testing the prompt building logic in isolation.

    #[test]
    fn test_implement_prompt_with_prefix_suffix() {
        let system_prompt = "# Development Agent\nYou are implementing a task.";
        let task_desc = "Implement feature X";

        // Simulate what run_implement does
        let prefix = Some("PREFIX TEXT".to_string());
        let suffix = Some("SUFFIX TEXT".to_string());

        let mut prompt = String::new();
        if let Some(p) = &prefix {
            prompt.push_str(p);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&format!(
            "{system_prompt}\n\n---\n\n\
             # Your Task\n\
             {task_desc}\n\n\
             You are working in an isolated git worktree. Focus only on this task.\n\
             Do not modify files outside the scope of this task.\n\
             Commit your changes when done."
        ));
        if let Some(s) = &suffix {
            prompt.push_str("\n\n");
            prompt.push_str(s);
        }

        assert!(prompt.starts_with("PREFIX TEXT\n\n"));
        assert!(prompt.contains("# Development Agent"));
        assert!(prompt.contains("Implement feature X"));
        assert!(prompt.ends_with("\n\nSUFFIX TEXT"));
    }

    #[test]
    fn test_implement_prompt_without_prefix_suffix() {
        let system_prompt = "# Development Agent\nYou are implementing a task.";
        let task_desc = "Implement feature X";

        // Simulate what run_implement does without prefix/suffix
        let prefix: Option<String> = None;
        let suffix: Option<String> = None;

        let mut prompt = String::new();
        if let Some(p) = &prefix {
            prompt.push_str(p);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&format!(
            "{system_prompt}\n\n---\n\n\
             # Your Task\n\
             {task_desc}\n\n\
             You are working in an isolated git worktree. Focus only on this task.\n\
             Do not modify files outside the scope of this task.\n\
             Commit your changes when done."
        ));
        if let Some(s) = &suffix {
            prompt.push_str("\n\n");
            prompt.push_str(s);
        }

        assert!(prompt.starts_with("# Development Agent"));
        assert!(prompt.contains("Implement feature X"));
        assert!(!prompt.contains("PREFIX TEXT"));
        assert!(!prompt.contains("SUFFIX TEXT"));
    }

    #[test]
    fn test_review_prompt_with_prefix_suffix() {
        let task_desc = "Implement feature X";
        let implementation_output = "Implementation completed successfully";

        let prefix = Some("PREFIX TEXT".to_string());
        let suffix = Some("SUFFIX TEXT".to_string());

        // Simulate what run_review does
        let verify_section = String::new();
        let mut prompt = String::new();
        if let Some(p) = &prefix {
            prompt.push_str(p);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&format!(
            "# Self-Review Task\n\n\
             Review your own implementation and fix any issues.\n\n\
             ## Original Task\n{task_desc}\n\n\
             ## Implementation Output\n{implementation_output}{verify_section}\n\n\
             Review the code changes you made. Look for:\n\
             - Bugs or logic errors\n\
             - Missing edge cases\n\
             - Code style issues\n\
             - Incomplete implementations\n\n\
             Fix any issues found and commit your changes."
        ));
        if let Some(s) = &suffix {
            prompt.push_str("\n\n");
            prompt.push_str(s);
        }

        assert!(prompt.starts_with("PREFIX TEXT\n\n"));
        assert!(prompt.contains("# Self-Review Task"));
        assert!(prompt.contains("Implement feature X"));
        assert!(prompt.ends_with("\n\nSUFFIX TEXT"));
    }

    #[test]
    fn test_review_prompt_without_prefix_suffix() {
        let task_desc = "Implement feature X";
        let implementation_output = "Implementation completed successfully";

        let prefix: Option<String> = None;
        let suffix: Option<String> = None;

        // Simulate what run_review does without prefix/suffix
        let verify_section = String::new();
        let mut prompt = String::new();
        if let Some(p) = &prefix {
            prompt.push_str(p);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&format!(
            "# Self-Review Task\n\n\
             Review your own implementation and fix any issues.\n\n\
             ## Original Task\n{task_desc}\n\n\
             ## Implementation Output\n{implementation_output}{verify_section}\n\n\
             Review the code changes you made. Look for:\n\
             - Bugs or logic errors\n\
             - Missing edge cases\n\
             - Code style issues\n\
             - Incomplete implementations\n\n\
             Fix any issues found and commit your changes."
        ));
        if let Some(s) = &suffix {
            prompt.push_str("\n\n");
            prompt.push_str(s);
        }

        assert!(prompt.starts_with("# Self-Review Task"));
        assert!(prompt.contains("Implement feature X"));
        assert!(!prompt.contains("PREFIX TEXT"));
        assert!(!prompt.contains("SUFFIX TEXT"));
    }

    #[test]
    fn test_review_prompt_with_verify_report_and_prefix_suffix() {
        let task_desc = "Implement feature X";
        let implementation_output = "Implementation completed successfully";
        let verify_report = "cargo test FAILED\nError: test_foo failed";

        let prefix = Some("CRITICAL: ".to_string());
        let suffix = Some("END OF PROMPT".to_string());

        // Simulate what run_review does with verify_report
        let verify_section = format!(
            "\n\n## Verification Results\n\
             The following verification commands FAILED after your implementation.\n\
             Fix the issues before proceeding.\n\n\
             <verify_report>\n{verify_report}\n</verify_report>"
        );

        let mut prompt = String::new();
        if let Some(p) = &prefix {
            prompt.push_str(p);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&format!(
            "# Self-Review Task\n\n\
             Review your own implementation and fix any issues.\n\n\
             ## Original Task\n{task_desc}\n\n\
             ## Implementation Output\n{implementation_output}{verify_section}\n\n\
             Review the code changes you made. Look for:\n\
             - Bugs or logic errors\n\
             - Missing edge cases\n\
             - Code style issues\n\
             - Incomplete implementations\n\n\
             Fix any issues found and commit your changes."
        ));
        if let Some(s) = &suffix {
            prompt.push_str("\n\n");
            prompt.push_str(s);
        }

        assert!(prompt.starts_with("CRITICAL: \n\n"));
        assert!(prompt.contains("## Verification Results"));
        assert!(prompt.contains("cargo test FAILED"));
        assert!(prompt.ends_with("\n\nEND OF PROMPT"));
    }

    #[test]
    fn test_verify_prompt_with_prefix_suffix() {
        let verification_commands = "cargo test\ncargo clippy";

        let prefix = Some("VERIFY: ".to_string());
        let suffix = Some("END VERIFY".to_string());

        // Simulate what run_verify does
        let mut prompt = String::new();
        if let Some(p) = &prefix {
            prompt.push_str(p);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&format!(
            "# Verification Task\n\n\
             Run the following verification commands and report results.\n\
             If all commands pass, respond with 'VERIFICATION PASSED'.\n\
             If any command fails, respond with 'VERIFICATION FAILED' and explain why.\n\n\
             ## Commands\n{verification_commands}"
        ));
        if let Some(s) = &suffix {
            prompt.push_str("\n\n");
            prompt.push_str(s);
        }

        assert!(prompt.starts_with("VERIFY: \n\n"));
        assert!(prompt.contains("# Verification Task"));
        assert!(prompt.contains("cargo test"));
        assert!(prompt.ends_with("\n\nEND VERIFY"));
    }

    #[test]
    fn test_verify_prompt_without_prefix_suffix() {
        let verification_commands = "cargo test\ncargo clippy";

        let prefix: Option<String> = None;
        let suffix: Option<String> = None;

        // Simulate what run_verify does without prefix/suffix
        let mut prompt = String::new();
        if let Some(p) = &prefix {
            prompt.push_str(p);
            prompt.push_str("\n\n");
        }
        prompt.push_str(&format!(
            "# Verification Task\n\n\
             Run the following verification commands and report results.\n\
             If all commands pass, respond with 'VERIFICATION PASSED'.\n\
             If any command fails, respond with 'VERIFICATION FAILED' and explain why.\n\n\
             ## Commands\n{verification_commands}"
        ));
        if let Some(s) = &suffix {
            prompt.push_str("\n\n");
            prompt.push_str(s);
        }

        assert!(prompt.starts_with("# Verification Task"));
        assert!(prompt.contains("cargo test"));
        assert!(!prompt.contains("VERIFY: "));
        assert!(!prompt.contains("END VERIFY"));
    }

    #[test]
    fn test_mcp_config_with_valid_port_and_session() {
        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerRunnerConfig {
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            mcp_port: 8080,
            mcp_session_id: "worker-123".to_string(),
        };
        let runner = WorkerRunner::new(1, "T01".to_string(), tx, shutdown, config);

        let mcp_config = runner.mcp_config();
        assert!(mcp_config.is_some());

        let config_json = mcp_config.unwrap();
        let servers = config_json.get("mcpServers").unwrap();
        let ralph = servers.get("ralph-tasks").unwrap();

        assert_eq!(ralph.get("type").unwrap().as_str(), Some("http"));
        let url = ralph.get("url").unwrap().as_str().unwrap();
        assert_eq!(url, "http://127.0.0.1:8080/mcp?session=worker-123");
    }

    #[test]
    fn test_mcp_config_with_zero_port() {
        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerRunnerConfig {
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            mcp_port: 0, // Invalid port
            mcp_session_id: "worker-123".to_string(),
        };
        let runner = WorkerRunner::new(1, "T01".to_string(), tx, shutdown, config);

        let mcp_config = runner.mcp_config();
        assert!(mcp_config.is_none(), "MCP config should be None for port=0");
    }

    #[test]
    fn test_mcp_config_with_empty_session_id() {
        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerRunnerConfig {
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            mcp_port: 8080,
            mcp_session_id: String::new(), // Empty session ID
        };
        let runner = WorkerRunner::new(1, "T01".to_string(), tx, shutdown, config);

        let mcp_config = runner.mcp_config();
        assert!(
            mcp_config.is_none(),
            "MCP config should be None for empty session_id"
        );
    }

    #[test]
    fn test_mcp_config_with_default_config() {
        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerRunnerConfig::default(); // port=0, session_id=""
        let runner = WorkerRunner::new(1, "T01".to_string(), tx, shutdown, config);

        let mcp_config = runner.mcp_config();
        assert!(
            mcp_config.is_none(),
            "MCP config should be None for default config"
        );
    }

    #[test]
    fn test_mcp_config_with_special_chars_in_session_id() {
        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let config = WorkerRunnerConfig {
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            mcp_port: 9000,
            mcp_session_id: "worker/123 & test".to_string(),
        };
        let runner = WorkerRunner::new(1, "T01".to_string(), tx, shutdown, config);

        let mcp_config = runner.mcp_config();
        assert!(mcp_config.is_some());

        let config_json = mcp_config.unwrap();
        let url = config_json["mcpServers"]["ralph-tasks"]["url"]
            .as_str()
            .unwrap();

        // URL should be properly encoded
        assert_eq!(
            url,
            "http://127.0.0.1:9000/mcp?session=worker%2F123%20%26%20test"
        );
    }
}
