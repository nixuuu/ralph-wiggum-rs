#![allow(dead_code)]
use std::path::{Path, PathBuf};

use crate::commands::task::orchestrate::git_helpers::git_command;
use crate::shared::error::{RalphError, Result};

/// Information about a created git worktree.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub task_id: String,
}

/// Information about an orphaned (no longer active) worktree.
#[derive(Debug, Clone)]
pub struct OrphanedWorktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Manages git worktree creation, removal, and cleanup for orchestration workers.
///
/// Worktrees are created as sibling directories to the project root,
/// with branches following the pattern `ralph/task/{task_id}`.
pub struct WorktreeManager {
    project_root: PathBuf,
    prefix: String,
}

impl WorktreeManager {
    /// Create a new WorktreeManager.
    ///
    /// `project_root` — the main project directory
    /// `prefix` — directory name prefix for worktrees (default: "{project_name}-ralph-")
    pub fn new(project_root: PathBuf, prefix: Option<String>) -> Self {
        let prefix = prefix.unwrap_or_else(|| {
            let project_name = project_root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project");
            format!("{project_name}-ralph-")
        });

        Self {
            project_root,
            prefix,
        }
    }

    /// Sanitize a task ID for use in filesystem paths and branch names.
    /// Replaces '.' with '-' (e.g., "1.2.3" → "1-2-3").
    pub fn sanitize_task_id(task_id: &str) -> String {
        task_id.replace('.', "-")
    }

    /// Generate the worktree directory path for a given task ID.
    /// Worktrees are created as siblings to the project root.
    pub fn worktree_path(&self, task_id: &str) -> PathBuf {
        let parent = self.project_root.parent().unwrap_or(Path::new("/tmp"));
        let sanitized = Self::sanitize_task_id(task_id);
        parent.join(format!("{}task-{sanitized}", self.prefix))
    }

    /// Generate the git branch name for a task.
    pub fn branch_name(task_id: &str) -> String {
        format!("ralph/task/{task_id}")
    }

    /// Create a git worktree for a specific task.
    ///
    /// Handles resumption after interruption:
    /// - If worktree + branch exist and match → reuse (continue previous work)
    /// - If branch exists but worktree is gone → prune + attach existing branch
    /// - If worktree exists with wrong branch → remove + create fresh
    /// - Nothing exists → create new branch + worktree
    pub async fn create_worktree(&self, task_id: &str) -> Result<WorktreeInfo> {
        let path = self.worktree_path(task_id);
        let branch = Self::branch_name(task_id);

        // Prune FIRST — clear stale git tracking before any inspection
        self.prune().await.ok();

        let branch_exists = self.branch_exists(&branch).await;

        if path.exists() {
            if branch_exists && self.worktree_has_branch(&path, &branch).await {
                // Worktree exists with correct branch — reuse it (resume)
                return Ok(WorktreeInfo {
                    path,
                    branch,
                    task_id: task_id.to_string(),
                });
            }

            // Wrong branch or stale directory — force cleanup (errors propagated!)
            self.force_cleanup_path(&path).await?;

            // Prune again after removing directory
            self.prune().await.ok();
        }

        // Re-check branch after cleanup (prune may have freed it)
        let branch_exists = self.branch_exists(&branch).await;

        if branch_exists {
            // Branch exists from a previous run — attach it to a new worktree
            let output = git_command()
                .args(["worktree", "add"])
                .arg(&path)
                .arg(&branch)
                .current_dir(&self.project_root)
                .output()
                .await
                .map_err(|e| RalphError::WorktreeError(format!("Failed to spawn git: {e}")))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(RalphError::WorktreeError(format!(
                    "git worktree add (branch '{}') failed: {}\nHint: try 'ralph task clean'",
                    branch,
                    stderr.trim(),
                )));
            }
        } else {
            // Fresh start — create new branch from HEAD
            let output = git_command()
                .args(["worktree", "add", "-b", &branch])
                .arg(&path)
                .arg("HEAD")
                .current_dir(&self.project_root)
                .output()
                .await
                .map_err(|e| RalphError::WorktreeError(format!("Failed to spawn git: {e}")))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(RalphError::WorktreeError(format!(
                    "git worktree add -b '{}' failed: {}\nHint: try 'ralph task clean'",
                    branch,
                    stderr.trim(),
                )));
            }
        }

        Ok(WorktreeInfo {
            path,
            branch,
            task_id: task_id.to_string(),
        })
    }

    /// Check if a git branch exists.
    async fn branch_exists(&self, branch: &str) -> bool {
        git_command()
            .args(["rev-parse", "--verify", &format!("refs/heads/{branch}")])
            .current_dir(&self.project_root)
            .output()
            .await
            .is_ok_and(|o| o.status.success())
    }

    /// Check if a worktree directory is on the expected branch.
    async fn worktree_has_branch(&self, worktree_path: &Path, expected_branch: &str) -> bool {
        git_command()
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(worktree_path)
            .output()
            .await
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
            .is_some_and(|b| b == expected_branch)
    }

    /// Remove a git worktree directory.
    pub async fn remove_worktree(&self, path: &Path) -> Result<()> {
        let output = git_command()
            .args(["worktree", "remove", "--force"])
            .arg(path)
            .current_dir(&self.project_root)
            .output()
            .await
            .map_err(|e| RalphError::WorktreeError(format!("Failed to spawn git: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RalphError::WorktreeError(format!(
                "git worktree remove failed: {stderr}"
            )));
        }
        Ok(())
    }

    /// Delete a git branch.
    pub async fn remove_branch(&self, branch: &str) -> Result<()> {
        let output = git_command()
            .args(["branch", "-D", branch])
            .current_dir(&self.project_root)
            .output()
            .await
            .map_err(|e| RalphError::WorktreeError(format!("Failed to spawn git: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RalphError::WorktreeError(format!(
                "git branch -D failed: {stderr}"
            )));
        }
        Ok(())
    }

    /// Check if a branch name matches ralph worktree patterns.
    /// Supports both new pattern `ralph/task/{task_id}` and legacy `ralph/w{N}/{task_id}`.
    fn is_ralph_branch(branch: &str) -> bool {
        if branch.starts_with("ralph/task/") {
            return true;
        }
        // Legacy pattern: ralph/w{digit}/...
        if let Some(rest) = branch.strip_prefix("ralph/w") {
            return rest
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false);
        }
        false
    }

    /// List orphaned ralph worktrees — worktrees matching our prefix
    /// that exist in `git worktree list` output.
    pub async fn list_orphaned(&self) -> Result<Vec<OrphanedWorktree>> {
        let output = git_command()
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&self.project_root)
            .output()
            .await
            .map_err(|e| RalphError::WorktreeError(format!("Failed to spawn git: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut orphans = Vec::new();

        let mut current_path: Option<PathBuf> = None;
        let mut current_branch: Option<String> = None;

        for line in stdout.lines() {
            if let Some(path_str) = line.strip_prefix("worktree ") {
                current_path = Some(PathBuf::from(path_str));
                current_branch = None;
            } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                current_branch = Some(branch.to_string());
            } else if line.is_empty() {
                // End of entry — check if it's a ralph worktree
                if let (Some(path), Some(branch)) = (current_path.take(), current_branch.take())
                    && Self::is_ralph_branch(&branch)
                {
                    orphans.push(OrphanedWorktree { path, branch });
                }
            }
        }
        // Handle last entry (no trailing empty line)
        if let (Some(path), Some(branch)) = (current_path, current_branch)
            && Self::is_ralph_branch(&branch)
        {
            orphans.push(OrphanedWorktree { path, branch });
        }

        Ok(orphans)
    }

    /// Force-remove a worktree path from both git tracking and the filesystem.
    /// Returns error with actionable hint if path cannot be removed.
    async fn force_cleanup_path(&self, path: &Path) -> Result<()> {
        // Try git worktree remove (best-effort — git may not track this path)
        let _ = git_command()
            .args(["worktree", "remove", "--force"])
            .arg(path)
            .current_dir(&self.project_root)
            .output()
            .await;

        // If still exists, force-remove from filesystem
        if path.exists() {
            tokio::fs::remove_dir_all(path).await.map_err(|e| {
                RalphError::WorktreeError(format!(
                    "Cannot remove stale worktree '{}': {e}\n\
                     Hint: rm -rf '{}'",
                    path.display(),
                    path.display(),
                ))
            })?;
        }

        // Verify (race condition guard)
        if path.exists() {
            return Err(RalphError::WorktreeError(format!(
                "Worktree '{}' still exists after cleanup.\n\
                 Another ralph instance may be using it.\n\
                 Hint: check running processes, then: rm -rf '{}'",
                path.display(),
                path.display(),
            )));
        }

        Ok(())
    }

    /// Prune stale worktree entries from git's tracking.
    pub async fn prune(&self) -> Result<()> {
        let output = git_command()
            .args(["worktree", "prune"])
            .current_dir(&self.project_root)
            .output()
            .await
            .map_err(|e| RalphError::WorktreeError(format!("Failed to spawn git: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RalphError::WorktreeError(format!(
                "git worktree prune failed: {stderr}"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_task_id() {
        assert_eq!(WorktreeManager::sanitize_task_id("1.2.3"), "1-2-3");
        assert_eq!(WorktreeManager::sanitize_task_id("T01"), "T01");
        assert_eq!(WorktreeManager::sanitize_task_id("a.b"), "a-b");
    }

    #[test]
    fn test_worktree_path_generation() {
        let mgr = WorktreeManager::new(PathBuf::from("/home/user/myproject"), None);
        let path = mgr.worktree_path("T01");
        assert_eq!(path, PathBuf::from("/home/user/myproject-ralph-task-T01"));

        let path = mgr.worktree_path("1.2.3");
        assert_eq!(path, PathBuf::from("/home/user/myproject-ralph-task-1-2-3"));
    }

    #[test]
    fn test_worktree_path_custom_prefix() {
        let mgr = WorktreeManager::new(
            PathBuf::from("/home/user/myproject"),
            Some("custom-prefix-".to_string()),
        );
        let path = mgr.worktree_path("T02");
        assert_eq!(path, PathBuf::from("/home/user/custom-prefix-task-T02"));
    }

    #[test]
    fn test_branch_name_generation() {
        assert_eq!(WorktreeManager::branch_name("T01"), "ralph/task/T01");
        assert_eq!(WorktreeManager::branch_name("1.2.3"), "ralph/task/1.2.3");
    }

    #[test]
    fn test_worktree_info_construction() {
        let info = WorktreeInfo {
            path: PathBuf::from("/tmp/proj-ralph-task-T01"),
            branch: "ralph/task/T01".to_string(),
            task_id: "T01".to_string(),
        };
        assert_eq!(info.task_id, "T01");
        assert_eq!(info.branch, "ralph/task/T01");
    }

    #[test]
    fn test_worktree_path_with_root_at_fs_root() {
        // Edge case: project at filesystem root
        let mgr = WorktreeManager::new(PathBuf::from("/project"), None);
        let path = mgr.worktree_path("T01");
        assert_eq!(path, PathBuf::from("/project-ralph-task-T01"));
    }

    #[test]
    fn test_default_prefix_from_project_name() {
        let mgr = WorktreeManager::new(PathBuf::from("/home/user/ralph-wiggum-rs"), None);
        let path = mgr.worktree_path("2.1");
        assert_eq!(
            path,
            PathBuf::from("/home/user/ralph-wiggum-rs-ralph-task-2-1")
        );
    }

    #[tokio::test]
    async fn test_force_cleanup_nonexistent_path() {
        let mgr = WorktreeManager::new(PathBuf::from("/tmp/ralph-test-wt"), None);
        let result = mgr
            .force_cleanup_path(Path::new("/tmp/ralph-test-nonexistent-xxxxx"))
            .await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_is_ralph_branch_new_pattern() {
        // New pattern: ralph/task/{task_id}
        assert!(WorktreeManager::is_ralph_branch("ralph/task/T01"));
        assert!(WorktreeManager::is_ralph_branch("ralph/task/1.2.3"));
        assert!(WorktreeManager::is_ralph_branch("ralph/task/6.3"));
    }

    #[test]
    fn test_is_ralph_branch_legacy_pattern() {
        // Legacy pattern: ralph/w{N}/{task_id}
        assert!(WorktreeManager::is_ralph_branch("ralph/w0/T01"));
        assert!(WorktreeManager::is_ralph_branch("ralph/w1/1.2.3"));
        assert!(WorktreeManager::is_ralph_branch("ralph/w99/6.3"));
    }

    #[test]
    fn test_is_ralph_branch_non_ralph() {
        // Non-ralph branches should return false
        assert!(!WorktreeManager::is_ralph_branch("main"));
        assert!(!WorktreeManager::is_ralph_branch("master"));
        assert!(!WorktreeManager::is_ralph_branch("feature/xyz"));
        assert!(!WorktreeManager::is_ralph_branch("ralph"));
        assert!(!WorktreeManager::is_ralph_branch("ralph/"));

        // Edge cases: ralph/w followed by non-digit
        assert!(!WorktreeManager::is_ralph_branch("ralph/wiggum"));
        assert!(!WorktreeManager::is_ralph_branch("ralph/worktree/test"));
        assert!(!WorktreeManager::is_ralph_branch("ralph/w"));
        assert!(!WorktreeManager::is_ralph_branch("ralph/w/"));
    }

    #[test]
    fn test_is_ralph_branch_patterns() {
        // New pattern: ralph/task/{task_id}
        assert!(WorktreeManager::is_ralph_branch("ralph/task/1.2.3"));
        assert!(WorktreeManager::is_ralph_branch("ralph/task/T01"));
        assert!(WorktreeManager::is_ralph_branch("ralph/task/feature-xyz"));

        // Legacy pattern: ralph/w{N}/{task_id}
        assert!(WorktreeManager::is_ralph_branch("ralph/w0/T01"));
        assert!(WorktreeManager::is_ralph_branch("ralph/w1/1.2.3"));
        assert!(WorktreeManager::is_ralph_branch("ralph/w99/xyz"));

        // Negative cases
        assert!(!WorktreeManager::is_ralph_branch("master"));
        assert!(!WorktreeManager::is_ralph_branch("feature/xyz"));
        assert!(!WorktreeManager::is_ralph_branch("ralph/other/T01"));
        assert!(!WorktreeManager::is_ralph_branch("ralph/w/T01")); // missing digit
        assert!(!WorktreeManager::is_ralph_branch("ralph/wX/T01")); // non-digit
    }

    #[test]
    fn test_list_orphaned_parsing_logic() {
        // Test the parsing logic used in list_orphaned() method
        // This verifies the algorithm handles various edge cases correctly

        // Case 1: Multiple entries with trailing empty line
        let output1 = "\
worktree /home/user/myproject
HEAD abc123
branch refs/heads/master

worktree /home/user/myproject-ralph-task-1-2-3
HEAD def456
branch refs/heads/ralph/task/1.2.3

";
        let mut orphans1 = Vec::new();
        let mut current_path: Option<PathBuf> = None;
        let mut current_branch: Option<String> = None;

        for line in output1.lines() {
            if let Some(path_str) = line.strip_prefix("worktree ") {
                current_path = Some(PathBuf::from(path_str));
                current_branch = None;
            } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                current_branch = Some(branch.to_string());
            } else if line.is_empty()
                && let (Some(path), Some(branch)) = (current_path.take(), current_branch.take())
                && WorktreeManager::is_ralph_branch(&branch)
            {
                orphans1.push((path, branch));
            }
        }
        // Handle last entry (no trailing empty line)
        if let (Some(path), Some(branch)) = (current_path, current_branch)
            && WorktreeManager::is_ralph_branch(&branch)
        {
            orphans1.push((path, branch));
        }

        assert_eq!(orphans1.len(), 1);
        assert_eq!(
            orphans1[0].0,
            PathBuf::from("/home/user/myproject-ralph-task-1-2-3")
        );
        assert_eq!(orphans1[0].1, "ralph/task/1.2.3");

        // Case 2: Last entry WITHOUT trailing empty line (edge case from line 267-271)
        let output2 = "\
worktree /home/user/myproject
HEAD abc123
branch refs/heads/master

worktree /home/user/myproject-ralph-w0-T01
HEAD ghi789
branch refs/heads/ralph/w0/T01";

        let mut orphans2 = Vec::new();
        current_path = None;
        current_branch = None;

        for line in output2.lines() {
            if let Some(path_str) = line.strip_prefix("worktree ") {
                current_path = Some(PathBuf::from(path_str));
                current_branch = None;
            } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                current_branch = Some(branch.to_string());
            } else if line.is_empty()
                && let (Some(path), Some(branch)) = (current_path.take(), current_branch.take())
                && WorktreeManager::is_ralph_branch(&branch)
            {
                orphans2.push((path, branch));
            }
        }
        // Handle last entry — CRITICAL for edge case coverage
        if let (Some(path), Some(branch)) = (current_path, current_branch)
            && WorktreeManager::is_ralph_branch(&branch)
        {
            orphans2.push((path, branch));
        }

        assert_eq!(orphans2.len(), 1);
        assert_eq!(
            orphans2[0].0,
            PathBuf::from("/home/user/myproject-ralph-w0-T01")
        );
        assert_eq!(orphans2[0].1, "ralph/w0/T01");

        // Case 3: Mix of ralph and non-ralph branches
        let output3 = "\
worktree /home/user/myproject
HEAD abc123
branch refs/heads/master

worktree /home/user/myproject-ralph-task-1-2-3
HEAD def456
branch refs/heads/ralph/task/1.2.3

worktree /home/user/myproject-feature
HEAD jkl012
branch refs/heads/feature/xyz

worktree /home/user/myproject-ralph-w1-T99
HEAD mno345
branch refs/heads/ralph/w1/T99";

        let mut orphans3 = Vec::new();
        current_path = None;
        current_branch = None;

        for line in output3.lines() {
            if let Some(path_str) = line.strip_prefix("worktree ") {
                current_path = Some(PathBuf::from(path_str));
                current_branch = None;
            } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                current_branch = Some(branch.to_string());
            } else if line.is_empty()
                && let (Some(path), Some(branch)) = (current_path.take(), current_branch.take())
                && WorktreeManager::is_ralph_branch(&branch)
            {
                orphans3.push((path.clone(), branch.clone()));
            }
        }
        if let (Some(path), Some(branch)) = (current_path, current_branch)
            && WorktreeManager::is_ralph_branch(&branch)
        {
            orphans3.push((path, branch));
        }

        // Should detect both ralph patterns, skip feature branch
        assert_eq!(orphans3.len(), 2);
        assert!(orphans3.iter().any(|(_, b)| b == "ralph/task/1.2.3"));
        assert!(orphans3.iter().any(|(_, b)| b == "ralph/w1/T99"));
    }
}
