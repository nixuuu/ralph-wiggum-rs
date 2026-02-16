use std::path::{Path, PathBuf};
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
    /// Summary of profile-level verification results (empty for non-profiled verify).
    #[allow(dead_code)]
    pub profile_results: Vec<ProfileVerifyResult>,
}

/// Summary of a profile's verification status.
#[derive(Debug, Clone)]
pub struct ProfileVerifyResult {
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub success: bool,
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
    /// Name of the profile this command was executed for (None for global verify).
    pub profile_name: Option<String>,
    /// Working directory where the command was executed (relative to worktree root).
    pub working_dir: Option<String>,
}

/// A single verification step in a profiled verify plan.
#[derive(Debug, Clone)]
pub enum VerifyStep {
    /// Global verification (runs at worktree root).
    GlobalVerify { commands: Vec<VerifyCommand> },
    /// Profile-specific verification (runs at profile's working_dir).
    ProfileVerify {
        profile_name: String,
        commands: Vec<VerifyCommand>,
        working_dir: Option<String>,
    },
}

/// A plan for running verification with multiple profiles.
#[derive(Debug)]
pub struct ProfiledVerifyPlan {
    pub steps: Vec<VerifyStep>,
}

/// Normalize a path by resolving `.` and `..` components lexically (no filesystem access).
fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                // Pop last normal component if possible; keep prefix/root
                if matches!(parts.last(), Some(Component::Normal(_))) {
                    parts.pop();
                } else {
                    parts.push(component);
                }
            }
            Component::CurDir => {} // skip `.`
            _ => parts.push(component),
        }
    }
    parts.iter().collect()
}

/// Resolve profile working directory, validating it stays within the worktree root.
///
/// Returns `None` if the resolved path escapes the root (e.g. via `../..`)
/// or if `working_dir` is an absolute path.
fn resolve_profile_cwd(root: &Path, working_dir: Option<&str>) -> Option<PathBuf> {
    match working_dir {
        Some(wd) => {
            // Reject absolute paths — working_dir must be relative to worktree root
            if Path::new(wd).is_absolute() {
                return None;
            }
            let resolved = normalize_path(&root.join(wd));
            let normalized_root = normalize_path(root);
            if resolved.starts_with(&normalized_root) {
                Some(resolved)
            } else {
                None
            }
        }
        None => Some(root.to_path_buf()),
    }
}

/// Run verification commands sequentially.
///
/// Commands are executed via `sh -c` in the given working directory.
/// On first failure (exit code != 0), stops and returns failure with details.
/// Sends OutputLines events to the dashboard for progress visibility.
/// Captures max 50 last lines of stdout+stderr per command.
#[allow(dead_code)]
pub async fn run_verify_commands(
    commands: &[VerifyCommand],
    cwd: &Path,
    event_tx: &mpsc::Sender<WorkerEvent>,
    worker_id: u32,
) -> VerifyResult {
    // Emit verify start separator
    let _ = event_tx
        .send(WorkerEvent::new(WorkerEventKind::OutputLines {
            worker_id,
            lines: vec![verify_start_separator()],
        }))
        .await;

    let result = run_commands_inner(commands, cwd, event_tx, worker_id, None, None).await;

    // Emit verify end separator
    let _ = event_tx
        .send(WorkerEvent::new(WorkerEventKind::OutputLines {
            worker_id,
            lines: vec![verify_end_separator(result.success)],
        }))
        .await;

    result
}

/// Run profiled verification plan with global and profile-specific steps.
///
/// Executes verification steps sequentially:
/// 1. Global verify (if present) runs at worktree root
/// 2. Profile verify steps run at profile-specific working directories
/// 3. On first failure (exit code != 0), stops and returns failure with details
/// 4. Emits profile separator events for visibility
///
/// Returns aggregated VerifyResult with all command results.
pub async fn run_profiled_verify(
    plan: &ProfiledVerifyPlan,
    cwd: &Path,
    event_tx: &mpsc::Sender<WorkerEvent>,
    worker_id: u32,
) -> VerifyResult {
    let mut all_results = Vec::new();
    let mut profile_results = Vec::new();

    // Emit overall verify start separator
    let _ = event_tx
        .send(WorkerEvent::new(WorkerEventKind::OutputLines {
            worker_id,
            lines: vec![verify_start_separator()],
        }))
        .await;

    for step in &plan.steps {
        match step {
            VerifyStep::GlobalVerify { commands } => {
                let result =
                    run_commands_inner(commands, cwd, event_tx, worker_id, None, None).await;

                all_results.extend(result.results);

                if !result.success {
                    emit_end_separator(event_tx, worker_id, false).await;
                    return VerifyResult {
                        success: false,
                        results: all_results,
                        profile_results,
                    };
                }
            }
            VerifyStep::ProfileVerify {
                profile_name,
                commands,
                working_dir,
            } => {
                // Emit profile separator event
                let _ = event_tx
                    .send(WorkerEvent::new(WorkerEventKind::OutputLines {
                        worker_id,
                        lines: vec![format!("🔍 Verify [{}]", profile_name)],
                    }))
                    .await;

                // Resolve and validate working directory
                let profile_cwd = match resolve_profile_cwd(cwd, working_dir.as_deref()) {
                    Some(p) => p,
                    None => {
                        // Path escapes worktree root — treat as failure
                        let error_msg = format!(
                            "working_dir '{}' escapes worktree root",
                            working_dir.as_deref().unwrap_or("")
                        );
                        all_results.push(CommandResult {
                            command: format!("cd {}", working_dir.as_deref().unwrap_or("")),
                            name: Some(format!("Profile: {}", profile_name)),
                            description: Some(error_msg.clone()),
                            exit_code: -1,
                            success: false,
                            output_tail: vec![error_msg],
                            profile_name: Some(profile_name.clone()),
                            working_dir: working_dir.clone(),
                        });
                        profile_results.push(ProfileVerifyResult {
                            name: profile_name.clone(),
                            success: false,
                        });
                        emit_end_separator(event_tx, worker_id, false).await;
                        return VerifyResult {
                            success: false,
                            results: all_results,
                            profile_results,
                        };
                    }
                };

                let result = run_commands_inner(
                    commands,
                    &profile_cwd,
                    event_tx,
                    worker_id,
                    Some(profile_name),
                    working_dir.as_deref(),
                )
                .await;

                all_results.extend(result.results);

                // Track profile result
                profile_results.push(ProfileVerifyResult {
                    name: profile_name.clone(),
                    success: result.success,
                });

                if !result.success {
                    emit_end_separator(event_tx, worker_id, false).await;
                    return VerifyResult {
                        success: false,
                        results: all_results,
                        profile_results,
                    };
                }
            }
        }
    }

    // All steps passed
    emit_end_separator(event_tx, worker_id, true).await;

    VerifyResult {
        success: true,
        results: all_results,
        profile_results,
    }
}

/// Emit verify end separator event.
async fn emit_end_separator(event_tx: &mpsc::Sender<WorkerEvent>, worker_id: u32, success: bool) {
    let _ = event_tx
        .send(WorkerEvent::new(WorkerEventKind::OutputLines {
            worker_id,
            lines: vec![verify_end_separator(success)],
        }))
        .await;
}

/// Core logic for executing commands sequentially with fail-fast.
///
/// Does NOT emit start/end separators — callers control those.
/// Sends per-command progress events to the dashboard.
///
/// # Parameters
/// * `profile_name` - Optional profile name for tracking (None for global verify)
/// * `working_dir` - Optional working directory relative path for reporting
async fn run_commands_inner(
    commands: &[VerifyCommand],
    cwd: &Path,
    event_tx: &mpsc::Sender<WorkerEvent>,
    worker_id: u32,
    profile_name: Option<&str>,
    working_dir: Option<&str>,
) -> VerifyResult {
    let mut results = Vec::new();

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
                    profile_name: profile_name.map(|s| s.to_string()),
                    working_dir: working_dir.map(|s| s.to_string()),
                }
            }
            Err(e) => {
                let error_msg = format!("Failed to execute command: {e}");
                CommandResult {
                    command: command_str.to_string(),
                    name: name.clone(),
                    description: description.clone(),
                    exit_code: -1,
                    success: false,
                    output_tail: vec![error_msg],
                    profile_name: profile_name.map(|s| s.to_string()),
                    working_dir: working_dir.map(|s| s.to_string()),
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
            return VerifyResult {
                success: false,
                results,
                profile_results: Vec::new(), // Filled by caller (run_profiled_verify)
            };
        }
    }

    VerifyResult {
        success: true,
        results,
        profile_results: Vec::new(), // Filled by caller (run_profiled_verify)
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
/// For Simple commands (no name/description, no profile):
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
///
/// For Profiled commands (with profile_name):
/// ```
/// Komenda weryfikacyjna nie przeszła
/// *profil*: <profile_name>
/// *working_dir*: <working_dir>
/// *komenda*: <command>
/// *nazwa*: <name>
/// *opis*: <description>
/// *logi*:
/// <50 ostatnich linii>
/// ```
pub fn format_failure_report(result: &CommandResult) -> String {
    let mut report = String::new();

    // Check if this is a profiled command
    let has_profile_info = result.profile_name.is_some();
    let has_detail_info = result.name.is_some() || result.description.is_some();

    if !has_profile_info && !has_detail_info {
        // Simple format (no profile, no name/description)
        report.push_str(&format!(
            "Komenda weryfikacyjna nie przeszła: {}\n",
            result.command
        ));
        report.push_str("Logs:\n");
    } else {
        // Detailed format (with profile and/or name/description)
        report.push_str("Komenda weryfikacyjna nie przeszła\n");

        // Profile info first (if present)
        if let Some(profile_name) = &result.profile_name {
            report.push_str(&format!("*profil*: {profile_name}\n"));
        }
        if let Some(working_dir) = &result.working_dir {
            report.push_str(&format!("*working_dir*: {working_dir}\n"));
        }

        // Command info
        report.push_str(&format!("*komenda*: {}\n", result.command));

        // Optional name/description
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

/// Format a comprehensive profiled verification report.
///
/// Generates a report with:
/// 1. Summary of all profile verification results with checkmarks (✓/✗)
/// 2. Detailed failure reports for each failed command
///
/// Example output:
/// ```
/// Dopasowane profile: [frontend ✓, backend ✗]
///
/// Komenda weryfikacyjna nie przeszła
/// *profil*: backend
/// *working_dir*: Services/
/// *komenda*: dotnet test
/// *logi*:
/// Test failed...
/// ```
#[allow(dead_code)]
pub fn format_profiled_report(verify_result: &VerifyResult) -> String {
    let mut report = String::new();

    // 1. Profile summary section
    if !verify_result.profile_results.is_empty() {
        report.push_str("Dopasowane profile: [");
        let profile_summaries: Vec<String> = verify_result
            .profile_results
            .iter()
            .map(|p| {
                let status = if p.success { "✓" } else { "✗" };
                format!("{} {}", p.name, status)
            })
            .collect();
        report.push_str(&profile_summaries.join(", "));
        report.push_str("]\n\n");
    }

    // 2. Detailed failure reports
    let failed_commands: Vec<&CommandResult> = verify_result
        .results
        .iter()
        .filter(|r| !r.success)
        .collect();

    for (i, result) in failed_commands.iter().enumerate() {
        if i > 0 {
            report.push_str("\n---\n\n");
        }
        report.push_str(&format_failure_report(result));
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
            profile_name: None,
            working_dir: None,
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
            profile_name: None,
            working_dir: None,
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
            profile_name: None,
            working_dir: None,
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
            profile_name: None,
            working_dir: None,
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
            profile_name: None,
            working_dir: None,
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

    // ── Task 41.3: Tests for ProfiledVerifyPlan ────

    #[tokio::test]
    async fn test_profiled_verify_global_only() {
        // Plan with only global verify step
        let plan = ProfiledVerifyPlan {
            steps: vec![VerifyStep::GlobalVerify {
                commands: vec![
                    VerifyCommand::Simple("echo global1".to_string()),
                    VerifyCommand::Simple("echo global2".to_string()),
                ],
            }],
        };

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_profiled_verify(&plan, &cwd, &tx, 1).await;

        assert!(result.success);
        assert_eq!(result.results.len(), 2);
        assert!(result.results[0].success);
        assert!(result.results[1].success);
    }

    #[tokio::test]
    async fn test_profiled_verify_profile_only() {
        // Plan with single profile verify step
        let plan = ProfiledVerifyPlan {
            steps: vec![VerifyStep::ProfileVerify {
                profile_name: "frontend".to_string(),
                commands: vec![VerifyCommand::Simple("echo profile".to_string())],
                working_dir: None,
            }],
        };

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_profiled_verify(&plan, &cwd, &tx, 1).await;

        assert!(result.success);
        assert_eq!(result.results.len(), 1);
        assert!(result.results[0].success);
    }

    #[tokio::test]
    async fn test_profiled_verify_mixed_global_and_profiles() {
        // Plan with global verify and multiple profiles
        let plan = ProfiledVerifyPlan {
            steps: vec![
                VerifyStep::GlobalVerify {
                    commands: vec![VerifyCommand::Simple("echo global".to_string())],
                },
                VerifyStep::ProfileVerify {
                    profile_name: "backend".to_string(),
                    commands: vec![VerifyCommand::Simple("echo backend".to_string())],
                    working_dir: None,
                },
                VerifyStep::ProfileVerify {
                    profile_name: "frontend".to_string(),
                    commands: vec![VerifyCommand::Simple("echo frontend".to_string())],
                    working_dir: None,
                },
            ],
        };

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_profiled_verify(&plan, &cwd, &tx, 1).await;

        assert!(result.success);
        assert_eq!(result.results.len(), 3);
        assert!(result.results.iter().all(|r| r.success));
    }

    #[tokio::test]
    async fn test_profiled_verify_fail_fast_on_global() {
        // Global verify fails - should stop before profile
        let plan = ProfiledVerifyPlan {
            steps: vec![
                VerifyStep::GlobalVerify {
                    commands: vec![
                        VerifyCommand::Simple("echo global".to_string()),
                        VerifyCommand::Simple("false".to_string()),
                    ],
                },
                VerifyStep::ProfileVerify {
                    profile_name: "backend".to_string(),
                    commands: vec![VerifyCommand::Simple("echo should_not_run".to_string())],
                    working_dir: None,
                },
            ],
        };

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_profiled_verify(&plan, &cwd, &tx, 1).await;

        assert!(!result.success);
        // Should stop after global failure - only 2 results (1 success + 1 fail)
        assert_eq!(result.results.len(), 2);
        assert!(result.results[0].success);
        assert!(!result.results[1].success);
    }

    #[tokio::test]
    async fn test_profiled_verify_fail_fast_across_profiles() {
        // First profile fails - should stop before second profile
        let plan = ProfiledVerifyPlan {
            steps: vec![
                VerifyStep::ProfileVerify {
                    profile_name: "backend".to_string(),
                    commands: vec![VerifyCommand::Simple("false".to_string())],
                    working_dir: None,
                },
                VerifyStep::ProfileVerify {
                    profile_name: "frontend".to_string(),
                    commands: vec![VerifyCommand::Simple("echo should_not_run".to_string())],
                    working_dir: None,
                },
            ],
        };

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_profiled_verify(&plan, &cwd, &tx, 1).await;

        assert!(!result.success);
        // Should stop after first profile failure
        assert_eq!(result.results.len(), 1);
        assert!(!result.results[0].success);
    }

    #[tokio::test]
    async fn test_profiled_verify_with_working_dir() {
        // Profile with custom working directory
        // Create a temp directory structure
        let temp_dir = std::env::temp_dir().join("ralph-test-verify");
        let sub_dir = temp_dir.join("subdir");
        std::fs::create_dir_all(&sub_dir).ok();

        let plan = ProfiledVerifyPlan {
            steps: vec![VerifyStep::ProfileVerify {
                profile_name: "subdir-test".to_string(),
                commands: vec![VerifyCommand::Simple("pwd".to_string())],
                working_dir: Some("subdir".to_string()),
            }],
        };

        let (tx, _rx) = mpsc::channel(10);

        let result = run_profiled_verify(&plan, &temp_dir, &tx, 1).await;

        assert!(result.success);
        assert_eq!(result.results.len(), 1);

        // Verify command ran in subdir (output should contain "subdir")
        let output = result.results[0].output_tail.join("");
        assert!(output.contains("subdir"));

        // Cleanup
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[tokio::test]
    async fn test_profiled_verify_emits_profile_separator() {
        use crate::commands::task::orchestrate::events::WorkerEventKind;

        let plan = ProfiledVerifyPlan {
            steps: vec![VerifyStep::ProfileVerify {
                profile_name: "my-profile".to_string(),
                commands: vec![VerifyCommand::Simple("true".to_string())],
                working_dir: None,
            }],
        };

        let (tx, mut rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        // Run in background
        let handle = tokio::spawn(async move { run_profiled_verify(&plan, &cwd, &tx, 1).await });

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
        assert!(result.success);

        // Should have events: verify_start + profile_separator + cmd_start + cmd_result + verify_end
        assert!(events.len() >= 4);

        // Find profile separator event
        let separator_found = events.iter().any(|e| {
            if let WorkerEventKind::OutputLines { lines, .. } = &e.kind {
                lines.iter().any(|l| l.contains("🔍 Verify [my-profile]"))
            } else {
                false
            }
        });
        assert!(separator_found, "Profile separator event not found");
    }

    #[tokio::test]
    async fn test_profiled_verify_empty_plan() {
        // Empty plan should succeed immediately
        let plan = ProfiledVerifyPlan { steps: vec![] };

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_profiled_verify(&plan, &cwd, &tx, 1).await;

        assert!(result.success);
        assert!(result.results.is_empty());
    }

    // ── resolve_profile_cwd tests ────

    #[test]
    fn test_resolve_profile_cwd_none_returns_root() {
        let root = Path::new("/tmp/worktree");
        let result = resolve_profile_cwd(root, None);
        assert_eq!(result, Some(PathBuf::from("/tmp/worktree")));
    }

    #[test]
    fn test_resolve_profile_cwd_subdir() {
        let root = Path::new("/tmp/worktree");
        let result = resolve_profile_cwd(root, Some("frontend"));
        // Lexical check: /tmp/worktree/frontend starts_with /tmp/worktree
        assert_eq!(result, Some(PathBuf::from("/tmp/worktree/frontend")));
    }

    #[test]
    fn test_resolve_profile_cwd_rejects_traversal() {
        let root = Path::new("/tmp/worktree");
        // ../../etc would escape root lexically
        let result = resolve_profile_cwd(root, Some("../../etc"));
        assert!(
            result.is_none(),
            "Should reject path that escapes worktree root"
        );
    }

    #[test]
    fn test_resolve_profile_cwd_rejects_absolute_path() {
        let root = Path::new("/tmp/worktree");
        let result = resolve_profile_cwd(root, Some("/etc/passwd"));
        assert!(result.is_none(), "Should reject absolute working_dir path");
    }

    #[tokio::test]
    async fn test_profiled_verify_rejects_path_traversal() {
        // Profile with path traversal should fail
        let plan = ProfiledVerifyPlan {
            steps: vec![VerifyStep::ProfileVerify {
                profile_name: "evil".to_string(),
                commands: vec![VerifyCommand::Simple("echo pwned".to_string())],
                working_dir: Some("../../etc".to_string()),
            }],
        };

        let (tx, _rx) = mpsc::channel(10);
        let cwd = std::env::temp_dir();

        let result = run_profiled_verify(&plan, &cwd, &tx, 1).await;

        assert!(!result.success);
        assert_eq!(result.results.len(), 1);
        assert!(
            result.results[0]
                .output_tail
                .join("")
                .contains("escapes worktree root")
        );
    }

    // ── Task 43.1: Tests for profiled reporting ────

    #[test]
    fn test_format_failure_report_with_profile() {
        let result = CommandResult {
            command: "dotnet test".to_string(),
            name: Some("Backend Tests".to_string()),
            description: Some("Run backend unit tests".to_string()),
            exit_code: 1,
            success: false,
            output_tail: vec!["Test failed: NullReferenceException".to_string()],
            profile_name: Some("backend".to_string()),
            working_dir: Some("Services/".to_string()),
        };

        let report = format_failure_report(&result);
        assert!(report.contains("Komenda weryfikacyjna nie przeszła\n"));
        assert!(report.contains("*profil*: backend"));
        assert!(report.contains("*working_dir*: Services/"));
        assert!(report.contains("*komenda*: dotnet test"));
        assert!(report.contains("*nazwa*: Backend Tests"));
        assert!(report.contains("*opis*: Run backend unit tests"));
        assert!(report.contains("*logi*:"));
        assert!(report.contains("Test failed: NullReferenceException"));
    }

    #[test]
    fn test_format_failure_report_with_profile_no_working_dir() {
        let result = CommandResult {
            command: "npm test".to_string(),
            name: None,
            description: None,
            exit_code: 1,
            success: false,
            output_tail: vec!["Test suite failed".to_string()],
            profile_name: Some("frontend".to_string()),
            working_dir: None,
        };

        let report = format_failure_report(&result);
        assert!(report.contains("*profil*: frontend"));
        assert!(!report.contains("*working_dir*:"));
        assert!(report.contains("*komenda*: npm test"));
        assert!(report.contains("Test suite failed"));
    }

    #[test]
    fn test_format_profiled_report_single_profile_success() {
        let verify_result = VerifyResult {
            success: true,
            results: vec![],
            profile_results: vec![ProfileVerifyResult {
                name: "frontend".to_string(),
                success: true,
            }],
        };

        let report = format_profiled_report(&verify_result);
        assert!(report.contains("Dopasowane profile: [frontend ✓]"));
    }

    #[test]
    fn test_format_profiled_report_multiple_profiles_mixed() {
        let verify_result = VerifyResult {
            success: false,
            results: vec![CommandResult {
                command: "cargo test".to_string(),
                name: Some("Backend Tests".to_string()),
                description: None,
                exit_code: 1,
                success: false,
                output_tail: vec!["test failed".to_string()],
                profile_name: Some("backend".to_string()),
                working_dir: Some("Services/".to_string()),
            }],
            profile_results: vec![
                ProfileVerifyResult {
                    name: "frontend".to_string(),
                    success: true,
                },
                ProfileVerifyResult {
                    name: "backend".to_string(),
                    success: false,
                },
            ],
        };

        let report = format_profiled_report(&verify_result);
        assert!(report.contains("Dopasowane profile: [frontend ✓, backend ✗]"));
        assert!(report.contains("*profil*: backend"));
        assert!(report.contains("*working_dir*: Services/"));
        assert!(report.contains("*komenda*: cargo test"));
        assert!(report.contains("test failed"));
    }

    #[test]
    fn test_format_profiled_report_multiple_failures() {
        let verify_result = VerifyResult {
            success: false,
            results: vec![
                CommandResult {
                    command: "cargo test".to_string(),
                    name: None,
                    description: None,
                    exit_code: 1,
                    success: false,
                    output_tail: vec!["backend test failed".to_string()],
                    profile_name: Some("backend".to_string()),
                    working_dir: Some("Services/".to_string()),
                },
                CommandResult {
                    command: "npm test".to_string(),
                    name: None,
                    description: None,
                    exit_code: 1,
                    success: false,
                    output_tail: vec!["frontend test failed".to_string()],
                    profile_name: Some("frontend".to_string()),
                    working_dir: Some("Web/".to_string()),
                },
            ],
            profile_results: vec![
                ProfileVerifyResult {
                    name: "backend".to_string(),
                    success: false,
                },
                ProfileVerifyResult {
                    name: "frontend".to_string(),
                    success: false,
                },
            ],
        };

        let report = format_profiled_report(&verify_result);
        assert!(report.contains("Dopasowane profile: [backend ✗, frontend ✗]"));
        assert!(report.contains("*profil*: backend"));
        assert!(report.contains("backend test failed"));
        assert!(report.contains("---")); // Separator between failures
        assert!(report.contains("*profil*: frontend"));
        assert!(report.contains("frontend test failed"));
    }

    #[test]
    fn test_format_profiled_report_no_profiles() {
        let verify_result = VerifyResult {
            success: false,
            results: vec![CommandResult {
                command: "cargo test".to_string(),
                name: None,
                description: None,
                exit_code: 1,
                success: false,
                output_tail: vec!["test failed".to_string()],
                profile_name: None,
                working_dir: None,
            }],
            profile_results: vec![],
        };

        let report = format_profiled_report(&verify_result);
        // No profile summary when profile_results is empty
        assert!(!report.contains("Dopasowane profile:"));
        assert!(report.contains("Komenda weryfikacyjna nie przeszła: cargo test"));
        assert!(report.contains("test failed"));
    }

    #[test]
    fn test_format_profiled_report_only_successful_commands() {
        let verify_result = VerifyResult {
            success: true,
            results: vec![CommandResult {
                command: "cargo test".to_string(),
                name: None,
                description: None,
                exit_code: 0,
                success: true,
                output_tail: vec!["all tests passed".to_string()],
                profile_name: Some("backend".to_string()),
                working_dir: Some("Services/".to_string()),
            }],
            profile_results: vec![ProfileVerifyResult {
                name: "backend".to_string(),
                success: true,
            }],
        };

        let report = format_profiled_report(&verify_result);
        assert!(report.contains("Dopasowane profile: [backend ✓]"));
        // No failure details when all commands succeed
        assert!(!report.contains("Komenda weryfikacyjna nie przeszła"));
    }

    // ── Task 43.2: Rozszerzone testy format_profiled_report ────

    /// Test 1: Raport bez profili — backward compat (stary format)
    #[test]
    fn test_format_profiled_report_no_profiles_backward_compat() {
        let verify_result = VerifyResult {
            success: false,
            results: vec![CommandResult {
                command: "cargo test".to_string(),
                name: None,
                description: None,
                exit_code: 1,
                success: false,
                output_tail: vec!["test failed".to_string()],
                profile_name: None,
                working_dir: None,
            }],
            profile_results: vec![], // No profile results
        };

        let report = format_profiled_report(&verify_result);

        // Nie ma sekcji profili
        assert!(!report.contains("Dopasowane profile:"));
        // Format prosty dla komend bez profilu
        assert!(report.contains("Komenda weryfikacyjna nie przeszła: cargo test"));
        assert!(report.contains("test failed"));
        // Nie ma pól profilu
        assert!(!report.contains("*profil*:"));
        assert!(!report.contains("*working_dir*:"));
    }

    /// Test 2: Raport z jednym profilem - success (tylko nagłówek, brak failure details)
    #[test]
    fn test_format_profiled_report_single_profile_success_only() {
        let verify_result = VerifyResult {
            success: true,
            results: vec![CommandResult {
                command: "npm test".to_string(),
                name: Some("Frontend Tests".to_string()),
                description: None,
                exit_code: 0,
                success: true,
                output_tail: vec!["all tests passed".to_string()],
                profile_name: Some("frontend".to_string()),
                working_dir: Some("Web/".to_string()),
            }],
            profile_results: vec![ProfileVerifyResult {
                name: "frontend".to_string(),
                success: true,
            }],
        };

        let report = format_profiled_report(&verify_result);

        // Sekcja profili z checkmarkiem ✓
        assert!(report.contains("Dopasowane profile: [frontend ✓]"));
        // Brak failure details (wszystko przeszło)
        assert!(!report.contains("Komenda weryfikacyjna nie przeszła"));
        assert!(!report.contains("*profil*:"));
    }

    /// Test 3: Raport z jednym profilem - failure (z pełnymi szczegółami)
    #[test]
    fn test_format_profiled_report_single_profile_failure() {
        let verify_result = VerifyResult {
            success: false,
            results: vec![CommandResult {
                command: "dotnet test".to_string(),
                name: Some("Backend Tests".to_string()),
                description: Some("Run backend unit tests".to_string()),
                exit_code: 1,
                success: false,
                output_tail: vec![
                    "Test failed: NullReferenceException".to_string(),
                    "at MyApp.Services.Test()".to_string(),
                ],
                profile_name: Some("backend".to_string()),
                working_dir: Some("Services/".to_string()),
            }],
            profile_results: vec![ProfileVerifyResult {
                name: "backend".to_string(),
                success: false,
            }],
        };

        let report = format_profiled_report(&verify_result);

        // Sekcja profili z checkmarkiem ✗
        assert!(report.contains("Dopasowane profile: [backend ✗]"));
        // Szczegóły błędu
        assert!(report.contains("Komenda weryfikacyjna nie przeszła"));
        assert!(report.contains("*profil*: backend"));
        assert!(report.contains("*working_dir*: Services/"));
        assert!(report.contains("*komenda*: dotnet test"));
        assert!(report.contains("*nazwa*: Backend Tests"));
        assert!(report.contains("*opis*: Run backend unit tests"));
        assert!(report.contains("*logi*:"));
        assert!(report.contains("Test failed: NullReferenceException"));
        assert!(report.contains("at MyApp.Services.Test()"));
    }

    /// Test 4: Raport z wieloma profilami - partial failure (mix ✓/✗)
    #[test]
    fn test_format_profiled_report_multiple_profiles_partial_failure() {
        let verify_result = VerifyResult {
            success: false,
            results: vec![
                // Frontend passed
                CommandResult {
                    command: "npm test".to_string(),
                    name: Some("Frontend Tests".to_string()),
                    description: None,
                    exit_code: 0,
                    success: true,
                    output_tail: vec!["all tests passed".to_string()],
                    profile_name: Some("frontend".to_string()),
                    working_dir: Some("Web/".to_string()),
                },
                // Backend failed
                CommandResult {
                    command: "cargo test".to_string(),
                    name: Some("Backend Tests".to_string()),
                    description: None,
                    exit_code: 1,
                    success: false,
                    output_tail: vec!["test backend::auth failed".to_string()],
                    profile_name: Some("backend".to_string()),
                    working_dir: Some("Services/".to_string()),
                },
                // Mobile failed
                CommandResult {
                    command: "flutter test".to_string(),
                    name: Some("Mobile Tests".to_string()),
                    description: None,
                    exit_code: 1,
                    success: false,
                    output_tail: vec!["Widget test failed".to_string()],
                    profile_name: Some("mobile".to_string()),
                    working_dir: Some("Mobile/".to_string()),
                },
            ],
            profile_results: vec![
                ProfileVerifyResult {
                    name: "frontend".to_string(),
                    success: true,
                },
                ProfileVerifyResult {
                    name: "backend".to_string(),
                    success: false,
                },
                ProfileVerifyResult {
                    name: "mobile".to_string(),
                    success: false,
                },
            ],
        };

        let report = format_profiled_report(&verify_result);

        // Sekcja profili z mieszanymi statusami
        assert!(report.contains("Dopasowane profile: [frontend ✓, backend ✗, mobile ✗]"));

        // Szczegóły tylko dla failed commands (backend i mobile)
        assert!(report.contains("*profil*: backend"));
        assert!(report.contains("*working_dir*: Services/"));
        assert!(report.contains("test backend::auth failed"));

        assert!(report.contains("---")); // Separator between failures

        assert!(report.contains("*profil*: mobile"));
        assert!(report.contains("*working_dir*: Mobile/"));
        assert!(report.contains("Widget test failed"));

        // Frontend nie ma failure report (success)
        assert!(!report.contains("*profil*: frontend"));
        assert!(!report.contains("*working_dir*: Web/"));
    }

    /// Test 5: Raport zawiera working_dir profilu
    #[test]
    fn test_format_profiled_report_contains_working_dir() {
        let verify_result = VerifyResult {
            success: false,
            results: vec![CommandResult {
                command: "cargo build".to_string(),
                name: Some("Build".to_string()),
                description: None,
                exit_code: 1,
                success: false,
                output_tail: vec!["compilation error".to_string()],
                profile_name: Some("api".to_string()),
                working_dir: Some("Services/Api/".to_string()),
            }],
            profile_results: vec![ProfileVerifyResult {
                name: "api".to_string(),
                success: false,
            }],
        };

        let report = format_profiled_report(&verify_result);

        // Raport zawiera working_dir
        assert!(report.contains("*working_dir*: Services/Api/"));
        // I profil
        assert!(report.contains("*profil*: api"));
    }

    /// Test 6: Raport zawiera profile_name
    #[test]
    fn test_format_profiled_report_contains_profile_name() {
        let verify_result = VerifyResult {
            success: false,
            results: vec![CommandResult {
                command: "go test ./...".to_string(),
                name: Some("Go Tests".to_string()),
                description: None,
                exit_code: 1,
                success: false,
                output_tail: vec!["FAIL: TestAuth".to_string()],
                profile_name: Some("golang-services".to_string()),
                working_dir: Some("Services/Go/".to_string()),
            }],
            profile_results: vec![ProfileVerifyResult {
                name: "golang-services".to_string(),
                success: false,
            }],
        };

        let report = format_profiled_report(&verify_result);

        // Raport zawiera profile_name w nagłówku i w szczegółach
        assert!(report.contains("Dopasowane profile: [golang-services ✗]"));
        assert!(report.contains("*profil*: golang-services"));
        // I working_dir
        assert!(report.contains("*working_dir*: Services/Go/"));
    }

    /// Test 7: Raport z profilem bez working_dir (działa z rootem worktree)
    #[test]
    fn test_format_profiled_report_profile_without_working_dir() {
        let verify_result = VerifyResult {
            success: false,
            results: vec![CommandResult {
                command: "make lint".to_string(),
                name: Some("Linter".to_string()),
                description: None,
                exit_code: 1,
                success: false,
                output_tail: vec!["linting errors found".to_string()],
                profile_name: Some("root-profile".to_string()),
                working_dir: None, // Brak working_dir (root)
            }],
            profile_results: vec![ProfileVerifyResult {
                name: "root-profile".to_string(),
                success: false,
            }],
        };

        let report = format_profiled_report(&verify_result);

        // Raport zawiera profile_name
        assert!(report.contains("*profil*: root-profile"));
        // Ale nie zawiera working_dir (None)
        assert!(!report.contains("*working_dir*:"));
    }
}
