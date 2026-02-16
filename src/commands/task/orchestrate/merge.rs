#![allow(dead_code)]
use std::path::Path;
use std::time::Duration;

use crate::commands::task::orchestrate::git_helpers::git_command;
use crate::commands::task::orchestrate::worktree::WorktreeInfo;
use crate::shared::error::{RalphError, Result};
use crate::shared::progress::ProgressTask;

/// Timeout for individual git operations (merge, commit, rev-parse).
pub(super) const GIT_OPERATION_TIMEOUT: Duration = Duration::from_secs(120);

/// Result of a squash merge attempt.
#[derive(Debug, Clone)]
pub enum MergeResult {
    Success { commit_hash: String },
    Conflict { files: Vec<String> },
    Failed { error: String },
}

/// Output of a single merge step — carries command string and raw output for display.
#[derive(Debug, Clone)]
pub struct StepOutput {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

/// Step 1: `git merge --squash {branch}`
///
/// Returns (StepOutput, conflicting_files). If conflicting_files is non-empty,
/// the merge has conflicts and the caller must decide how to proceed.
pub async fn step_merge_squash(
    project_root: &Path,
    branch: &str,
) -> Result<(StepOutput, Vec<String>)> {
    let command = format!("git merge --squash {branch}");
    let git_cmd = git_command()
        .args(["merge", "--squash", branch])
        .current_dir(project_root)
        .output();

    let output = tokio::time::timeout(GIT_OPERATION_TIMEOUT, git_cmd)
        .await
        .map_err(|_| {
            RalphError::MergeConflict("git merge --squash timed out after 120s".to_string())
        })?
        .map_err(|e| RalphError::MergeConflict(format!("Failed to spawn git merge: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let success = output.status.success();

    let conflict_files = if !success {
        extract_conflict_files(&stdout, &stderr)
    } else {
        Vec::new()
    };

    Ok((
        StepOutput {
            command,
            stdout,
            stderr,
            success,
        },
        conflict_files,
    ))
}

/// Step 2: `git commit -m "task({task_id}): {task_name}"`
pub async fn step_commit(
    project_root: &Path,
    task_id: &str,
    task_name: &str,
) -> Result<StepOutput> {
    let commit_msg = format_commit_message(task_id, task_name);
    let command = format!("git commit --no-gpg-sign -m \"{commit_msg}\"");
    let git_cmd = git_command()
        .args(["commit", "--no-gpg-sign", "-m", &commit_msg])
        .current_dir(project_root)
        .output();

    let output = tokio::time::timeout(GIT_OPERATION_TIMEOUT, git_cmd)
        .await
        .map_err(|_| RalphError::MergeConflict("git commit timed out after 120s".to_string()))?
        .map_err(|e| RalphError::MergeConflict(format!("Failed to commit merge: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(StepOutput {
        command,
        stdout,
        stderr,
        success: output.status.success(),
    })
}

/// Step 3: `git rev-parse --short HEAD`
pub async fn step_rev_parse(project_root: &Path) -> Result<(StepOutput, String)> {
    let command = "git rev-parse --short HEAD".to_string();
    let git_cmd = git_command()
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(project_root)
        .output();

    let output = tokio::time::timeout(GIT_OPERATION_TIMEOUT, git_cmd)
        .await
        .map_err(|_| RalphError::MergeConflict("git rev-parse timed out after 120s".to_string()))?
        .map_err(|e| RalphError::MergeConflict(format!("Failed to get commit hash: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let hash = stdout.trim().to_string();

    Ok((
        StepOutput {
            command,
            stdout,
            stderr,
            success: output.status.success(),
        },
        hash,
    ))
}

/// Perform a squash merge of a worker's branch into the current branch.
///
/// Thin wrapper over step functions for backward compatibility and tests.
pub async fn squash_merge(
    project_root: &Path,
    worktree: &WorktreeInfo,
    task: &ProgressTask,
) -> Result<MergeResult> {
    let (step1, conflict_files) = step_merge_squash(project_root, &worktree.branch).await?;

    if !step1.success {
        if !conflict_files.is_empty() {
            return Ok(MergeResult::Conflict {
                files: conflict_files,
            });
        }
        return Ok(MergeResult::Failed {
            error: step1.stderr,
        });
    }

    let step2 = step_commit(project_root, &task.id, &task.name).await?;
    if !step2.success {
        return Ok(MergeResult::Failed {
            error: format!("Commit failed: {}", step2.stderr),
        });
    }

    let (_step3, commit_hash) = step_rev_parse(project_root).await?;

    Ok(MergeResult::Success { commit_hash })
}

/// Abort an in-progress merge.
pub async fn abort_merge(project_root: &Path) -> Result<()> {
    let git_cmd = git_command()
        .args(["merge", "--abort"])
        .current_dir(project_root)
        .output();

    tokio::time::timeout(GIT_OPERATION_TIMEOUT, git_cmd)
        .await
        .map_err(|_| {
            RalphError::MergeConflict("git merge --abort timed out after 120s".to_string())
        })?
        .map_err(|e| RalphError::MergeConflict(format!("Failed to abort merge: {e}")))?;
    Ok(())
}

/// Format the commit message for a squash merge.
pub fn format_commit_message(task_id: &str, task_name: &str) -> String {
    format!("task({task_id}): {task_name}")
}

/// Format a StepOutput as display lines for the dashboard.
pub fn step_output_lines(step: &StepOutput) -> Vec<String> {
    let mut lines = vec![format!("$ {}", step.command)];
    for line in step.stdout.lines() {
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    for line in step.stderr.lines() {
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

/// Extract conflicting file paths from git merge output.
///
/// Supports multiple conflict formats:
/// - `CONFLICT (content): Merge conflict in <file>`
/// - `CONFLICT (add/add): Merge conflict in <file>`
/// - `CONFLICT (modify/delete): <file> deleted in HEAD ...`
/// - `CONFLICT (rename/delete): <file> deleted in HEAD ...`
/// - `CONFLICT (rename/rename): <file> renamed to <a> in HEAD and to <b> in ...`
///
/// Deduplicates results when the same file appears in both stdout and stderr.
fn extract_conflict_files(stdout: &str, stderr: &str) -> Vec<String> {
    let mut files = Vec::new();
    let combined = format!("{stdout}\n{stderr}");

    for line in combined.lines() {
        if let Some(rest) = line.strip_prefix("CONFLICT") {
            let after_type = if let Some(colon_pos) = rest.find(':') {
                rest[colon_pos + 1..].trim()
            } else {
                continue;
            };

            // "Merge conflict in <file>"
            if let Some(file) = after_type.strip_prefix("Merge conflict in ") {
                let file = file.trim();
                if !file.is_empty() && !files.contains(&file.to_string()) {
                    files.push(file.to_string());
                }
                continue;
            }

            // "<file> deleted in HEAD ..." lub "<file> renamed to ..."
            if (after_type.contains(" deleted in") || after_type.contains(" renamed to "))
                && let Some(file) = after_type
                    .split(" deleted in")
                    .next()
                    .and_then(|s| s.split(" renamed to ").next())
            {
                let file = file.trim();
                if !file.is_empty() && !files.contains(&file.to_string()) {
                    files.push(file.to_string());
                }
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_commit_message() {
        assert_eq!(
            format_commit_message("T01", "Setup JWT authentication"),
            "task(T01): Setup JWT authentication"
        );
        assert_eq!(
            format_commit_message("1.2.3", "Add frontmatter parser"),
            "task(1.2.3): Add frontmatter parser"
        );
    }

    #[test]
    fn test_extract_conflict_files() {
        let stdout = "\
Auto-merging src/main.rs
CONFLICT (content): Merge conflict in src/main.rs
CONFLICT (content): Merge conflict in Cargo.toml
Auto-merging README.md";
        let files = extract_conflict_files(stdout, "");
        assert_eq!(files, vec!["src/main.rs", "Cargo.toml"]);
    }

    #[test]
    fn test_extract_conflict_files_empty() {
        let files = extract_conflict_files("", "");
        assert!(files.is_empty());
    }

    #[test]
    fn test_extract_conflict_files_no_conflicts() {
        let stdout = "Auto-merging src/main.rs\nMerge made by the 'ort' strategy.";
        let files = extract_conflict_files(stdout, "");
        assert!(files.is_empty());
    }

    #[test]
    fn test_merge_result_variants() {
        let success = MergeResult::Success {
            commit_hash: "abc1234".to_string(),
        };
        assert!(matches!(success, MergeResult::Success { .. }));

        let conflict = MergeResult::Conflict {
            files: vec!["src/main.rs".to_string()],
        };
        assert!(matches!(conflict, MergeResult::Conflict { .. }));

        let failed = MergeResult::Failed {
            error: "some error".to_string(),
        };
        assert!(matches!(failed, MergeResult::Failed { .. }));
    }

    #[test]
    fn test_step_output_lines_format() {
        let step = StepOutput {
            command: "git merge --squash ralph/task/T03".to_string(),
            stdout: "Auto-merging src/main.rs\n".to_string(),
            stderr: "".to_string(),
            success: true,
        };
        let lines = step_output_lines(&step);
        assert_eq!(lines[0], "$ git merge --squash ralph/task/T03");
        assert_eq!(lines[1], "Auto-merging src/main.rs");
    }

    #[test]
    fn test_step_output_lines_with_stderr() {
        let step = StepOutput {
            command: "git commit -m \"task(T01): Fix bug\"".to_string(),
            stdout: "".to_string(),
            stderr: "warning: something\n".to_string(),
            success: true,
        };
        let lines = step_output_lines(&step);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], "warning: something");
    }

    #[test]
    fn test_extract_conflict_files_format_content() {
        let stdout = "CONFLICT (content): Merge conflict in src/main.rs";
        let files = extract_conflict_files(stdout, "");
        assert_eq!(files, vec!["src/main.rs"]);
    }

    #[test]
    fn test_extract_conflict_files_format_modify_delete() {
        let stdout = "CONFLICT (modify/delete): src/old.rs deleted in HEAD and modified in branch.";
        let files = extract_conflict_files(stdout, "");
        assert_eq!(files, vec!["src/old.rs"]);
    }

    #[test]
    fn test_extract_conflict_files_format_add_add() {
        let stdout = "CONFLICT (add/add): Merge conflict in src/new.rs";
        let files = extract_conflict_files(stdout, "");
        assert_eq!(files, vec!["src/new.rs"]);
    }

    #[test]
    fn test_extract_conflict_files_mixed_formats() {
        let stdout = "\
Auto-merging src/main.rs
CONFLICT (content): Merge conflict in src/main.rs
Auto-merging src/old.rs
CONFLICT (modify/delete): src/old.rs deleted in HEAD and modified in branch.
CONFLICT (add/add): Merge conflict in src/new.rs
Auto-merging Cargo.toml
CONFLICT (content): Merge conflict in Cargo.toml";
        let files = extract_conflict_files(stdout, "");
        assert_eq!(
            files,
            vec!["src/main.rs", "src/old.rs", "src/new.rs", "Cargo.toml"]
        );
    }

    #[test]
    fn test_extract_conflict_files_rename_delete() {
        let stdout = "CONFLICT (rename/delete): src/renamed.rs deleted in HEAD and renamed to src/new_name.rs in branch.";
        let files = extract_conflict_files(stdout, "");
        assert_eq!(files, vec!["src/renamed.rs"]);
    }

    #[test]
    fn test_extract_conflict_files_from_stderr() {
        let stderr = "\
CONFLICT (content): Merge conflict in lib/utils.rs
CONFLICT (modify/delete): lib/legacy.rs deleted in HEAD and modified in branch.";
        let files = extract_conflict_files("", stderr);
        assert_eq!(files, vec!["lib/utils.rs", "lib/legacy.rs"]);
    }

    #[test]
    fn test_extract_conflict_files_combined_stdout_stderr() {
        let stdout = "CONFLICT (content): Merge conflict in src/app.rs";
        let stderr = "CONFLICT (add/add): Merge conflict in src/config.rs";
        let files = extract_conflict_files(stdout, stderr);
        assert_eq!(files, vec!["src/app.rs", "src/config.rs"]);
    }

    #[test]
    fn test_extract_conflict_files_dedup_across_streams() {
        let stdout = "CONFLICT (content): Merge conflict in src/main.rs";
        let stderr = "CONFLICT (content): Merge conflict in src/main.rs";
        let files = extract_conflict_files(stdout, stderr);
        assert_eq!(files, vec!["src/main.rs"]);
    }

    #[test]
    fn test_extract_conflict_files_rename_rename() {
        let stdout = "CONFLICT (rename/rename): src/old.rs renamed to src/a.rs in HEAD and to src/b.rs in branch.";
        let files = extract_conflict_files(stdout, "");
        assert_eq!(files, vec!["src/old.rs"]);
    }

    #[test]
    fn test_extract_conflict_files_empty_after_colon() {
        let stdout = "CONFLICT (content):";
        let files = extract_conflict_files(stdout, "");
        assert!(files.is_empty());
    }

    #[test]
    fn test_step_output_lines_empty_fields() {
        let step = StepOutput {
            command: "".to_string(),
            stdout: "".to_string(),
            stderr: "".to_string(),
            success: true,
        };
        let lines = step_output_lines(&step);
        // Puste pola → tylko linia z promptem "$ "
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "$ ");
    }
}
