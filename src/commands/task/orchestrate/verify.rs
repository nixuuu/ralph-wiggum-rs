use std::path::Path;
use tokio::sync::mpsc;

use crate::commands::task::orchestrate::events::{WorkerEvent, WorkerEventKind};
use crate::commands::task::orchestrate::orchestrator_events::{
    verify_end_separator, verify_start_separator,
};
use crate::shared::file_config::VerifyCommand;

/// Result of running verification commands.
#[derive(Debug)]
pub struct VerifyResult {
    pub success: bool,
    pub results: Vec<CommandResult>,
}

/// Result of a single verification command.
#[derive(Debug)]
pub struct CommandResult {
    pub command: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub exit_code: i32,
    pub success: bool,
    pub output_tail: Vec<String>,
}

/// Run verification commands sequentially.
///
/// Commands are executed via `sh -c` in the given working directory.
/// On first failure (exit code != 0), stops and returns failure with details.
/// Sends OutputLines events to the dashboard for progress visibility.
/// Captures max 50 last lines of stdout+stderr per command.
pub async fn run_verify_commands(
    commands: &[VerifyCommand],
    cwd: &Path,
    event_tx: &mpsc::Sender<WorkerEvent>,
    worker_id: u32,
) -> VerifyResult {
    let mut results = Vec::new();

    // Emit verify start separator
    let _ = event_tx
        .send(WorkerEvent::new(WorkerEventKind::OutputLines {
            worker_id,
            lines: vec![verify_start_separator()],
        }))
        .await;

    for cmd in commands {
        let command_str = cmd.command();
        let name = cmd.name().map(|s| s.to_string());
        let description = cmd.description().map(|s| s.to_string());
        let label = cmd.label();

        // Send command start to dashboard
        let _ = event_tx
            .send(WorkerEvent::new(WorkerEventKind::OutputLines {
                worker_id,
                lines: vec![format!("$ {label}")],
            }))
            .await;

        // Execute command
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command_str)
            .current_dir(cwd)
            .output()
            .await;

        let result = match output {
            Ok(out) => {
                let exit_code = out.status.code().unwrap_or(-1);
                let success = out.status.success();

                // Combine stdout and stderr
                let mut combined = Vec::new();
                combined.extend_from_slice(&out.stdout);
                combined.extend_from_slice(&out.stderr);

                let output_tail = tail_lines(&combined, 50);

                CommandResult {
                    command: command_str.to_string(),
                    name: name.clone(),
                    description: description.clone(),
                    exit_code,
                    success,
                    output_tail,
                }
            }
            Err(e) => {
                // Command failed to execute
                let error_msg = format!("Failed to execute command: {e}");
                CommandResult {
                    command: command_str.to_string(),
                    name: name.clone(),
                    description: description.clone(),
                    exit_code: -1,
                    success: false,
                    output_tail: vec![error_msg],
                }
            }
        };

        // Send result to dashboard
        let status_msg = if result.success {
            format!("✓ {label}")
        } else {
            format!("✗ {label} (exit code: {})", result.exit_code)
        };
        let _ = event_tx
            .send(WorkerEvent::new(WorkerEventKind::OutputLines {
                worker_id,
                lines: vec![status_msg],
            }))
            .await;

        let success = result.success;
        results.push(result);

        if !success {
            // First failure — stop sequence
            // Emit verify end separator (failure)
            let _ = event_tx
                .send(WorkerEvent::new(WorkerEventKind::OutputLines {
                    worker_id,
                    lines: vec![verify_end_separator(false)],
                }))
                .await;

            return VerifyResult {
                success: false,
                results,
            };
        }
    }

    // Emit verify end separator (success)
    let _ = event_tx
        .send(WorkerEvent::new(WorkerEventKind::OutputLines {
            worker_id,
            lines: vec![verify_end_separator(true)],
        }))
        .await;

    VerifyResult {
        success: true,
        results,
    }
}

/// Extract the last N lines from output bytes.
fn tail_lines(bytes: &[u8], max_lines: usize) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();

    if lines.len() <= max_lines {
        lines
    } else {
        lines[lines.len() - max_lines..].to_vec()
    }
}

/// Format a failure report for a command result.
///
/// For Simple commands (no name/description):
/// ```
/// Komenda weryfikacyjna nie przeszła: <command>
/// Logs:
/// <50 ostatnich linii>
/// ```
///
/// For Detailed commands (with name and/or description):
/// ```
/// Komenda weryfikacyjna nie przeszła
/// *komenda*: <command>
/// *nazwa*: <name>
/// *opis*: <description>
/// *logi*:
/// <50 ostatnich linii>
/// ```
pub fn format_failure_report(result: &CommandResult) -> String {
    let mut report = String::new();

    if result.name.is_none() && result.description.is_none() {
        // Simple format
        report.push_str(&format!(
            "Komenda weryfikacyjna nie przeszła: {}\n",
            result.command
        ));
        report.push_str("Logs:\n");
    } else {
        // Detailed format
        report.push_str("Komenda weryfikacyjna nie przeszła\n");
        report.push_str(&format!("*komenda*: {}\n", result.command));
        if let Some(name) = &result.name {
            report.push_str(&format!("*nazwa*: {name}\n"));
        }
        if let Some(description) = &result.description {
            report.push_str(&format!("*opis*: {description}\n"));
        }
        report.push_str("*logi*:\n");
    }

    // Append output lines
    for line in &result.output_tail {
        report.push_str(line);
        report.push('\n');
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::file_config::VerifyCommand;

    #[test]
    fn test_tail_lines_exact() {
        let bytes = b"line1\nline2\nline3";
        let lines = tail_lines(bytes, 3);
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn test_tail_lines_more_than_available() {
        let bytes = b"line1\nline2";
        let lines = tail_lines(bytes, 10);
        assert_eq!(lines, vec!["line1", "line2"]);
    }

    #[test]
    fn test_tail_lines_fewer() {
        let bytes = b"line1\nline2\nline3\nline4\nline5";
        let lines = tail_lines(bytes, 2);
        assert_eq!(lines, vec!["line4", "line5"]);
    }

    #[test]
    fn test_tail_lines_empty() {
        let bytes = b"";
        let lines = tail_lines(bytes, 10);
        assert!(lines.is_empty() || (lines.len() == 1 && lines[0].is_empty()));
    }

    #[test]
    fn test_format_failure_report_simple() {
        let result = CommandResult {
            command: "cargo test".to_string(),
            name: None,
            description: None,
            exit_code: 1,
            success: false,
            output_tail: vec!["error: test failed".to_string()],
        };

        let report = format_failure_report(&result);
        assert!(report.contains("Komenda weryfikacyjna nie przeszła: cargo test"));
        assert!(report.contains("Logs:"));
        assert!(report.contains("error: test failed"));
        assert!(!report.contains("*komenda*"));
        assert!(!report.contains("*nazwa*"));
    }

    #[test]
    fn test_format_failure_report_detailed_with_name() {
        let result = CommandResult {
            command: "cargo clippy".to_string(),
            name: Some("Lint".to_string()),
            description: None,
            exit_code: 1,
            success: false,
            output_tail: vec!["warning: unused variable".to_string()],
        };

        let report = format_failure_report(&result);
        assert!(report.contains("Komenda weryfikacyjna nie przeszła\n"));
        assert!(report.contains("*komenda*: cargo clippy"));
        assert!(report.contains("*nazwa*: Lint"));
        assert!(report.contains("*logi*:"));
        assert!(report.contains("warning: unused variable"));
        assert!(!report.contains("*opis*"));
    }

    #[test]
    fn test_format_failure_report_detailed_with_description() {
        let result = CommandResult {
            command: "cargo test".to_string(),
            name: None,
            description: Some("Run all tests".to_string()),
            exit_code: 1,
            success: false,
            output_tail: vec!["test result: FAILED".to_string()],
        };

        let report = format_failure_report(&result);
        assert!(report.contains("Komenda weryfikacyjna nie przeszła\n"));
        assert!(report.contains("*komenda*: cargo test"));
        assert!(report.contains("*opis*: Run all tests"));
        assert!(report.contains("*logi*:"));
        assert!(!report.contains("*nazwa*"));
    }

    #[test]
    fn test_format_failure_report_detailed_full() {
        let result = CommandResult {
            command: "npm test".to_string(),
            name: Some("Unit tests".to_string()),
            description: Some("Run all unit tests".to_string()),
            exit_code: 1,
            success: false,
            output_tail: vec![
                "FAIL src/test.js".to_string(),
                "Expected 42 but got 0".to_string(),
            ],
        };

        let report = format_failure_report(&result);
        assert!(report.contains("Komenda weryfikacyjna nie przeszła\n"));
        assert!(report.contains("*komenda*: npm test"));
        assert!(report.contains("*nazwa*: Unit tests"));
        assert!(report.contains("*opis*: Run all unit tests"));
        assert!(report.contains("*logi*:"));
        assert!(report.contains("FAIL src/test.js"));
        assert!(report.contains("Expected 42 but got 0"));
    }

    #[test]
    fn test_format_failure_report_empty_output() {
        let result = CommandResult {
            command: "exit 1".to_string(),
            name: None,
            description: None,
            exit_code: 1,
            success: false,
            output_tail: vec![],
        };

        let report = format_failure_report(&result);
        assert!(report.contains("Komenda weryfikacyjna nie przeszła: exit 1"));
        assert!(report.contains("Logs:"));
        // No output lines, but report should still be valid
    }

    #[tokio::test]
    async fn test_run_verify_commands_success() {
        let commands = vec![
            VerifyCommand::Simple("echo hello".to_string()),
            VerifyCommand::Simple("true".to_string()),
        ];

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_verify_commands(&commands, &cwd, &tx, 1).await;

        assert!(result.success);
        assert_eq!(result.results.len(), 2);
        assert!(result.results[0].success);
        assert!(result.results[1].success);
    }

    #[tokio::test]
    async fn test_run_verify_commands_failure_stops_sequence() {
        let commands = vec![
            VerifyCommand::Simple("true".to_string()),
            VerifyCommand::Simple("false".to_string()),
            VerifyCommand::Simple("echo should_not_run".to_string()),
        ];

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_verify_commands(&commands, &cwd, &tx, 1).await;

        assert!(!result.success);
        // Should stop after second command (false)
        assert_eq!(result.results.len(), 2);
        assert!(result.results[0].success);
        assert!(!result.results[1].success);
    }

    #[tokio::test]
    async fn test_run_verify_commands_captures_output() {
        let commands = vec![VerifyCommand::Simple(
            "echo line1 && echo line2 && echo line3".to_string(),
        )];

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_verify_commands(&commands, &cwd, &tx, 1).await;

        assert!(result.success);
        assert_eq!(result.results.len(), 1);

        let output = &result.results[0].output_tail;
        assert!(output.iter().any(|l| l.contains("line1")));
        assert!(output.iter().any(|l| l.contains("line2")));
        assert!(output.iter().any(|l| l.contains("line3")));
    }

    #[tokio::test]
    async fn test_run_verify_commands_detailed() {
        let commands = vec![VerifyCommand::Detailed {
            command: "echo test".to_string(),
            name: Some("Test echo".to_string()),
            description: Some("Simple echo test".to_string()),
        }];

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_verify_commands(&commands, &cwd, &tx, 1).await;

        assert!(result.success);
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].name.as_deref(), Some("Test echo"));
        assert_eq!(
            result.results[0].description.as_deref(),
            Some("Simple echo test")
        );
    }

    #[tokio::test]
    async fn test_run_verify_commands_empty_list() {
        let commands: Vec<VerifyCommand> = vec![];

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_verify_commands(&commands, &cwd, &tx, 1).await;

        assert!(result.success);
        assert!(result.results.is_empty());
    }

    #[tokio::test]
    async fn test_run_verify_commands_with_no_output() {
        let commands = vec![VerifyCommand::Simple("true".to_string())];

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_verify_commands(&commands, &cwd, &tx, 1).await;

        assert!(result.success);
        assert_eq!(result.results.len(), 1);
        // Empty or minimal output is fine
        assert!(result.results[0].output_tail.len() <= 1);
    }

    #[tokio::test]
    async fn test_run_verify_commands_with_large_output() {
        // Generate command that produces more than 50 lines
        let commands = vec![VerifyCommand::Simple(
            "for i in $(seq 1 100); do echo line$i; done".to_string(),
        )];

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_verify_commands(&commands, &cwd, &tx, 1).await;

        assert!(result.success);
        assert_eq!(result.results.len(), 1);
        // Should be truncated to max 50 lines
        assert!(result.results[0].output_tail.len() <= 50);
        // Should contain lines from the end (line51-line100)
        let output = result.results[0].output_tail.join("\n");
        assert!(output.contains("line100"));
        assert!(output.contains("line51") || output.contains("line52"));
    }

    #[tokio::test]
    async fn test_run_verify_commands_sends_events() {
        use crate::commands::task::orchestrate::events::WorkerEventKind;

        let commands = vec![
            VerifyCommand::Simple("true".to_string()),
            VerifyCommand::Simple("false".to_string()),
        ];

        let (tx, mut rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        // Run in background so we can receive events
        let handle =
            tokio::spawn(async move { run_verify_commands(&commands, &cwd, &tx, 1).await });

        // Collect events
        let mut events = Vec::new();
        while let Ok(event) =
            tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
        {
            if let Some(e) = event {
                events.push(e);
            } else {
                break;
            }
        }

        let result = handle.await.unwrap();
        assert!(!result.success); // Should fail on second command

        // Should have 6 events: verify_start + cmd1_start + cmd1_result + cmd2_start + cmd2_result + verify_end_failure
        assert_eq!(events.len(), 6);

        // Verify start separator
        if let WorkerEventKind::OutputLines { lines, .. } = &events[0].kind {
            assert_eq!(lines.len(), 1);
            assert!(lines[0].contains("🔍"));
            assert!(lines[0].contains("Verify"));
        } else {
            panic!("Expected OutputLines event for verify start");
        }

        // First command start
        if let WorkerEventKind::OutputLines { lines, .. } = &events[1].kind {
            assert_eq!(lines.len(), 1);
            assert!(lines[0].starts_with("$ "));
        } else {
            panic!("Expected OutputLines event");
        }

        // First command success
        if let WorkerEventKind::OutputLines { lines, .. } = &events[2].kind {
            assert_eq!(lines.len(), 1);
            assert!(lines[0].starts_with("✓ "));
        } else {
            panic!("Expected OutputLines event");
        }

        // Second command start
        if let WorkerEventKind::OutputLines { lines, .. } = &events[3].kind {
            assert_eq!(lines.len(), 1);
            assert!(lines[0].starts_with("$ "));
        } else {
            panic!("Expected OutputLines event");
        }

        // Second command failure
        if let WorkerEventKind::OutputLines { lines, .. } = &events[4].kind {
            assert_eq!(lines.len(), 1);
            assert!(lines[0].starts_with("✗ "));
            assert!(lines[0].contains("exit code"));
        } else {
            panic!("Expected OutputLines event");
        }

        // Verify end separator (failure)
        if let WorkerEventKind::OutputLines { lines, .. } = &events[5].kind {
            assert_eq!(lines.len(), 1);
            assert!(lines[0].contains("✗"));
            assert!(lines[0].contains("Verify"));
            assert!(lines[0].contains("[FAILED]"));
        } else {
            panic!("Expected OutputLines event for verify end failure");
        }
    }
}
