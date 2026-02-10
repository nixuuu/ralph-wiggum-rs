#![allow(dead_code)]
use std::path::Path;

use tokio::process::Command;

use crate::commands::task::orchestrate::worktree::WorktreeInfo;
use crate::shared::error::{RalphError, Result};
use crate::shared::progress::ProgressTask;

/// Result of a squash merge attempt.
#[derive(Debug, Clone)]
pub enum MergeResult {
    Success { commit_hash: String },
    Conflict { files: Vec<String> },
    Failed { error: String },
}

/// Perform a squash merge of a worker's branch into the current branch.
///
/// Runs `git merge --squash {branch}` in the project root, then commits
/// with the format `task({task_id}): {task_name}`.
pub async fn squash_merge(
    project_root: &Path,
    worktree: &WorktreeInfo,
    task: &ProgressTask,
) -> Result<MergeResult> {
    let merge_output = Command::new("git")
        .args(["merge", "--squash", &worktree.branch])
        .current_dir(project_root)
        .output()
        .await
        .map_err(|e| RalphError::MergeConflict(format!("Failed to spawn git merge: {e}")))?;

    if !merge_output.status.success() {
        let stderr = String::from_utf8_lossy(&merge_output.stderr);
        let stdout = String::from_utf8_lossy(&merge_output.stdout);

        // Check for conflict indicators
        let conflict_files = extract_conflict_files(&stdout, &stderr);
        if !conflict_files.is_empty() {
            return Ok(MergeResult::Conflict {
                files: conflict_files,
            });
        }

        return Ok(MergeResult::Failed {
            error: stderr.to_string(),
        });
    }

    // Commit the squash merge
    let commit_msg = format_commit_message(&task.id, &task.name);
    let commit_output = Command::new("git")
        .args(["commit", "-m", &commit_msg])
        .current_dir(project_root)
        .output()
        .await
        .map_err(|e| RalphError::MergeConflict(format!("Failed to commit merge: {e}")))?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        return Ok(MergeResult::Failed {
            error: format!("Commit failed: {stderr}"),
        });
    }

    // Get the commit hash
    let hash_output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(project_root)
        .output()
        .await
        .map_err(|e| RalphError::MergeConflict(format!("Failed to get commit hash: {e}")))?;

    let commit_hash = String::from_utf8_lossy(&hash_output.stdout)
        .trim()
        .to_string();

    Ok(MergeResult::Success { commit_hash })
}

/// Abort an in-progress merge.
pub async fn abort_merge(project_root: &Path) -> Result<()> {
    Command::new("git")
        .args(["merge", "--abort"])
        .current_dir(project_root)
        .output()
        .await
        .map_err(|e| RalphError::MergeConflict(format!("Failed to abort merge: {e}")))?;
    Ok(())
}

/// Format the commit message for a squash merge.
pub fn format_commit_message(task_id: &str, task_name: &str) -> String {
    format!("task({task_id}): {task_name}")
}

/// Extract conflicting file paths from git merge output.
fn extract_conflict_files(stdout: &str, stderr: &str) -> Vec<String> {
    let mut files = Vec::new();
    let combined = format!("{stdout}\n{stderr}");

    for line in combined.lines() {
        // git merge --squash shows "CONFLICT (content): Merge conflict in <file>"
        if let Some(rest) = line.strip_prefix("CONFLICT")
            && let Some(file_part) = rest.rsplit("Merge conflict in ").next()
        {
            let file = file_part.trim();
            if !file.is_empty() {
                files.push(file.to_string());
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
}
