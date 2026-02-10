#![allow(dead_code)]
use std::path::{Path, PathBuf};

use tokio::process::Command;

use crate::shared::error::{RalphError, Result};

/// Information about a created git worktree.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub worker_id: u32,
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
/// with branches following the pattern `ralph/w{N}/{task_id}`.
pub struct WorktreeManager {
    project_root: PathBuf,
    prefix: String,
}

impl WorktreeManager {
    /// Create a new WorktreeManager.
    ///
    /// `project_root` — the main project directory
    /// `prefix` — directory name prefix for worktrees (default: "{project_name}-ralph-w")
    pub fn new(project_root: PathBuf, prefix: Option<String>) -> Self {
        let prefix = prefix.unwrap_or_else(|| {
            let project_name = project_root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project");
            format!("{project_name}-ralph-w")
        });

        Self {
            project_root,
            prefix,
        }
    }

    /// Generate the worktree directory path for a given worker ID.
    /// Worktrees are created as siblings to the project root.
    pub fn worktree_path(&self, worker_id: u32) -> PathBuf {
        let parent = self.project_root.parent().unwrap_or(Path::new("/tmp"));
        parent.join(format!("{}{worker_id}", self.prefix))
    }

    /// Generate the git branch name for a worker + task combination.
    pub fn branch_name(worker_id: u32, task_id: &str) -> String {
        format!("ralph/w{worker_id}/{task_id}")
    }

    /// Create a git worktree for a worker to work on a specific task.
    ///
    /// Handles resumption after interruption:
    /// - If worktree + branch exist and match → reuse (continue previous work)
    /// - If branch exists but worktree is gone → prune + attach existing branch
    /// - If worktree exists with wrong branch → remove + create fresh
    /// - Nothing exists → create new branch + worktree
    pub async fn create_worktree(&self, worker_id: u32, task_id: &str) -> Result<WorktreeInfo> {
        let path = self.worktree_path(worker_id);
        let branch = Self::branch_name(worker_id, task_id);

        // Prune FIRST — clear stale git tracking before any inspection
        self.prune().await.ok();

        let branch_exists = self.branch_exists(&branch).await;

        if path.exists() {
            if branch_exists && self.worktree_has_branch(&path, &branch).await {
                // Worktree exists with correct branch — reuse it (resume)
                return Ok(WorktreeInfo {
                    path,
                    branch,
                    worker_id,
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
            let output = Command::new("git")
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
            let output = Command::new("git")
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
            worker_id,
            task_id: task_id.to_string(),
        })
    }

    /// Check if a git branch exists.
    async fn branch_exists(&self, branch: &str) -> bool {
        Command::new("git")
            .args(["rev-parse", "--verify", &format!("refs/heads/{branch}")])
            .current_dir(&self.project_root)
            .output()
            .await
            .is_ok_and(|o| o.status.success())
    }

    /// Check if a worktree directory is on the expected branch.
    async fn worktree_has_branch(&self, worktree_path: &Path, expected_branch: &str) -> bool {
        Command::new("git")
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
        let output = Command::new("git")
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
        let output = Command::new("git")
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

    /// List orphaned ralph worktrees — worktrees matching our prefix
    /// that exist in `git worktree list` output.
    pub async fn list_orphaned(&self) -> Result<Vec<OrphanedWorktree>> {
        let output = Command::new("git")
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
                    && branch.starts_with("ralph/w")
                {
                    orphans.push(OrphanedWorktree { path, branch });
                }
            }
        }
        // Handle last entry (no trailing empty line)
        if let (Some(path), Some(branch)) = (current_path, current_branch)
            && branch.starts_with("ralph/w")
        {
            orphans.push(OrphanedWorktree { path, branch });
        }

        Ok(orphans)
    }

    /// Force-remove a worktree path from both git tracking and the filesystem.
    /// Returns error with actionable hint if path cannot be removed.
    async fn force_cleanup_path(&self, path: &Path) -> Result<()> {
        // Try git worktree remove (best-effort — git may not track this path)
        let _ = Command::new("git")
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
        let output = Command::new("git")
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
    fn test_worktree_path_generation() {
        let mgr = WorktreeManager::new(PathBuf::from("/home/user/myproject"), None);
        let path = mgr.worktree_path(1);
        assert_eq!(path, PathBuf::from("/home/user/myproject-ralph-w1"));

        let path = mgr.worktree_path(3);
        assert_eq!(path, PathBuf::from("/home/user/myproject-ralph-w3"));
    }

    #[test]
    fn test_worktree_path_custom_prefix() {
        let mgr = WorktreeManager::new(
            PathBuf::from("/home/user/myproject"),
            Some("custom-prefix-".to_string()),
        );
        let path = mgr.worktree_path(2);
        assert_eq!(path, PathBuf::from("/home/user/custom-prefix-2"));
    }

    #[test]
    fn test_branch_name_generation() {
        assert_eq!(WorktreeManager::branch_name(1, "T01"), "ralph/w1/T01");
        assert_eq!(WorktreeManager::branch_name(3, "1.2.3"), "ralph/w3/1.2.3");
    }

    #[test]
    fn test_worktree_info_construction() {
        let info = WorktreeInfo {
            path: PathBuf::from("/tmp/proj-ralph-w1"),
            branch: "ralph/w1/T01".to_string(),
            worker_id: 1,
            task_id: "T01".to_string(),
        };
        assert_eq!(info.worker_id, 1);
        assert_eq!(info.task_id, "T01");
    }

    #[test]
    fn test_worktree_path_with_root_at_fs_root() {
        // Edge case: project at filesystem root
        let mgr = WorktreeManager::new(PathBuf::from("/project"), None);
        let path = mgr.worktree_path(1);
        assert_eq!(path, PathBuf::from("/project-ralph-w1"));
    }

    #[test]
    fn test_default_prefix_from_project_name() {
        let mgr = WorktreeManager::new(PathBuf::from("/home/user/ralph-wiggum-rs"), None);
        let path = mgr.worktree_path(2);
        assert_eq!(path, PathBuf::from("/home/user/ralph-wiggum-rs-ralph-w2"));
    }

    #[tokio::test]
    async fn test_force_cleanup_nonexistent_path() {
        let mgr = WorktreeManager::new(PathBuf::from("/tmp/ralph-test-wt"), None);
        let result = mgr
            .force_cleanup_path(Path::new("/tmp/ralph-test-nonexistent-xxxxx"))
            .await;
        assert!(result.is_ok());
    }
}
