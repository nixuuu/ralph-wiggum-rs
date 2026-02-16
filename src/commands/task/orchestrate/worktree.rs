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
    ///
    /// Security: strips path separators (`/`, `\`) and parent-directory sequences (`..`)
    /// to prevent path traversal attacks. Only alphanumeric, `-`, and `_` survive.
    pub fn sanitize_task_id(task_id: &str) -> String {
        task_id
            .chars()
            .map(|c| match c {
                c if c.is_ascii_alphanumeric() || c == '-' || c == '_' => c,
                _ => '-',
            })
            .collect()
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
    fn test_worktree_path_when_project_root_is_fs_root() {
        // Edge case: project_root = "/" — parent() returns None, fallback to /tmp.
        // file_name() also returns None, so project_name defaults to "project".
        let mgr = WorktreeManager::new(PathBuf::from("/"), None);
        let path = mgr.worktree_path("T01");

        assert_eq!(path, PathBuf::from("/tmp/project-ralph-task-T01"));

        // No double separators in the generated path
        assert!(!path.to_string_lossy().contains("//"));
    }

    #[test]
    fn test_sanitize_task_id_unicode() {
        // sanitize_task_id przepuszcza TYLKO ASCII alphanumeric, '-' i '_'.
        // Znaki unicode (non-ASCII) zostają zamienione na '-'.
        assert_eq!(WorktreeManager::sanitize_task_id("zażółć"), "za----");
        assert_eq!(
            WorktreeManager::sanitize_task_id("task (copy)"),
            "task--copy-"
        );
        assert_eq!(WorktreeManager::sanitize_task_id("task:name"), "task-name");
        assert_eq!(WorktreeManager::sanitize_task_id("task~1"), "task-1");
        assert_eq!(
            WorktreeManager::sanitize_task_id("task[1].name~2"),
            "task-1--name-2"
        );
    }

    #[test]
    fn test_sanitize_task_id_git_branch_compatibility() {
        // sanitize_task_id jest używane TYLKO dla ścieżek filesystem (worktree_path).
        // branch_name() używa oryginalnego task_id bez sanityzacji.
        // Znaki zabronione w git branch: space, ~, ^, :, ?, *, [, ], \

        let forbidden_in_git = [
            (" ", "space"),
            ("~", "tilde"),
            ("^", "caret"),
            (":", "colon"),
            ("?", "question"),
            ("*", "asterisk"),
            ("[", "bracket-open"),
            ("]", "bracket-close"),
            ("\\", "backslash"),
        ];

        for (ch, label) in forbidden_in_git {
            let task_id = format!("task{ch}name");
            let branch = WorktreeManager::branch_name(&task_id);

            // Dokumentacja obecnego zachowania: branch_name nie filtruje specjalnych znaków
            assert!(
                branch.contains(ch),
                "branch_name nie filtruje znaku {label}: {branch}"
            );
        }
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

    // --- Path traversal security tests (task 54.1) ---

    /// Unix-style path traversal: "../../../etc" must not contain slashes.
    #[test]
    fn test_sanitize_task_id_path_traversal_unix() {
        let sanitized = WorktreeManager::sanitize_task_id("../../../etc");
        assert!(
            !sanitized.contains('/'),
            "sanitized ID must not contain '/'"
        );
        assert!(
            !sanitized.contains('\\'),
            "sanitized ID must not contain '\\'"
        );
        assert!(
            !sanitized.contains(".."),
            "sanitized ID must not contain '..'"
        );
        // Each non-alphanumeric char becomes '-': "../../../etc" has 9 such chars
        assert_eq!(sanitized, "---------etc");
    }

    /// Mixed traversal: "task/../../secret" — slashes stripped.
    #[test]
    fn test_sanitize_task_id_path_traversal_mixed() {
        let sanitized = WorktreeManager::sanitize_task_id("task/../../secret");
        assert!(!sanitized.contains('/'));
        assert!(!sanitized.contains(".."));
        assert_eq!(sanitized, "task-------secret");
    }

    /// Windows-style traversal: "task\..\..\secret" — backslashes stripped.
    #[test]
    fn test_sanitize_task_id_path_traversal_windows() {
        let sanitized = WorktreeManager::sanitize_task_id(r"task\..\..\secret");
        assert!(!sanitized.contains('\\'));
        assert!(!sanitized.contains('/'));
        assert!(!sanitized.contains(".."));
        assert_eq!(sanitized, "task-------secret");
    }

    /// Worktree path with traversal ID stays inside parent directory.
    #[test]
    fn test_worktree_path_no_traversal() {
        let mgr = WorktreeManager::new(PathBuf::from("/home/user/myproject"), None);
        let path = mgr.worktree_path("../../../etc/passwd");
        let path_str = path.to_string_lossy();

        // Path must stay under /home/user/ (sibling to project)
        assert!(
            path_str.starts_with("/home/user/"),
            "worktree path escaped parent: {path_str}"
        );
        assert!(
            !path_str.contains(".."),
            "worktree path contains '..': {path_str}"
        );
        assert!(
            !path_str.contains("/etc/"),
            "worktree path reached /etc/: {path_str}"
        );
    }

    /// Branch name with traversal ID — slashes in ID are sanitized before use.
    /// Note: branch_name() uses raw task_id by design (git refs allow slashes).
    /// Security boundary is at worktree_path() and sanitize_task_id().
    #[test]
    fn test_branch_name_with_traversal_id() {
        // branch_name uses raw ID — callers must sanitize if needed
        let branch = WorktreeManager::branch_name("../../../etc");
        assert_eq!(branch, "ralph/task/../../../etc");

        // But worktree_path always sanitizes:
        let mgr = WorktreeManager::new(PathBuf::from("/home/user/proj"), None);
        let path = mgr.worktree_path("../../../etc");
        assert!(!path.to_string_lossy().contains(".."));
    }

    /// Edge cases: null bytes, spaces, special characters.
    #[test]
    fn test_sanitize_task_id_special_chars() {
        // Only alphanumeric, dash, underscore survive
        assert_eq!(WorktreeManager::sanitize_task_id("a b"), "a-b");
        assert_eq!(WorktreeManager::sanitize_task_id("a\0b"), "a-b");
        assert_eq!(WorktreeManager::sanitize_task_id("a:b"), "a-b");
        assert_eq!(WorktreeManager::sanitize_task_id("a_b"), "a_b");
        assert_eq!(WorktreeManager::sanitize_task_id(""), "");
    }

    // --- Worker failure cleanup tests (task 54.4) ---

    /// Helper: tworzy tymczasowe git repo z jednym commitem (wymagany dla worktree).
    fn init_test_git_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        use std::fs;
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let root = temp_dir.path().to_path_buf();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output()
            .expect("git init failed");
        std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(&root)
            .output()
            .ok();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&root)
            .output()
            .ok();

        fs::write(root.join("README.md"), "test").expect("write failed");
        std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&root)
            .output()
            .expect("git add failed");
        std::process::Command::new("git")
            .args(["commit", "-m", "Initial commit"])
            .current_dir(&root)
            .output()
            .expect("git commit failed");

        (temp_dir, root)
    }

    /// Helper: sprawdza czy branch istnieje w repozytorium.
    fn branch_exists(project_root: &Path, branch: &str) -> bool {
        std::process::Command::new("git")
            .args(["rev-parse", "--verify", branch])
            .current_dir(project_root)
            .output()
            .expect("git rev-parse failed")
            .status
            .success()
    }

    /// Test cleanup po worker failure — worktree i branch usunięte, brak orphans.
    ///
    /// Scenariusz:
    /// 1. Worker tworzy worktree dla zadania
    /// 2. Worker kończy się błędem
    /// 3. Orchestrator wywołuje cleanup (remove_worktree + remove_branch)
    /// 4. Weryfikacja: worktree nie istnieje, branch usunięty, brak orphaned worktrees
    #[tokio::test]
    async fn test_worktree_cleanup_after_worker_failure() {
        let (_temp_dir, project_root) = init_test_git_repo();
        let mgr = WorktreeManager::new(project_root.clone(), None);
        let task_id = "54.4";

        // Worker tworzy worktree
        let wt = mgr
            .create_worktree(task_id)
            .await
            .expect("Failed to create worktree");

        assert!(wt.path.exists(), "Worktree should exist after creation");
        assert_eq!(wt.task_id, task_id);
        assert_eq!(wt.branch, format!("ralph/task/{task_id}"));
        assert!(
            branch_exists(&project_root, &wt.branch),
            "Branch should exist after worktree creation"
        );

        // Orchestrator robi cleanup po failure
        mgr.remove_worktree(&wt.path)
            .await
            .expect("Failed to remove worktree");
        mgr.remove_branch(&wt.branch)
            .await
            .expect("Failed to remove branch");

        // Weryfikacja
        assert!(
            !wt.path.exists(),
            "Worktree path should not exist after cleanup"
        );
        assert!(
            !branch_exists(&project_root, &wt.branch),
            "Branch should not exist after cleanup"
        );
        let orphans = mgr.list_orphaned().await.expect("list_orphaned failed");
        assert!(
            orphans.is_empty(),
            "Should have no orphaned worktrees after cleanup, found: {orphans:?}"
        );
    }

    /// Test cleanup dirty worktree — worktree z niecommitowanymi zmianami.
    ///
    /// Edge case: worker zaczął modyfikować pliki w worktree ale nie zcommitował.
    /// Cleanup powinien nadal działać poprawnie.
    #[tokio::test]
    async fn test_worktree_cleanup_with_dirty_working_tree() {
        let (_temp_dir, project_root) = init_test_git_repo();
        let mgr = WorktreeManager::new(project_root.clone(), None);
        let task_id = "dirty-wt";

        let wt = mgr
            .create_worktree(task_id)
            .await
            .expect("Failed to create worktree");
        assert!(wt.path.exists());

        // Symuluj pracę workera — niecommitowane zmiany w worktree
        std::fs::write(wt.path.join("dirty_file.txt"), "uncommitted work")
            .expect("write to worktree failed");
        std::fs::write(wt.path.join("README.md"), "modified content")
            .expect("modify file in worktree failed");

        // Cleanup powinien działać mimo dirty worktree
        let cleanup_result = mgr.remove_worktree(&wt.path).await;
        assert!(
            cleanup_result.is_ok(),
            "Cleanup should succeed on dirty worktree: {cleanup_result:?}"
        );

        let branch_result = mgr.remove_branch(&wt.branch).await;
        assert!(
            branch_result.is_ok(),
            "Branch cleanup should succeed: {branch_result:?}"
        );

        assert!(!wt.path.exists(), "Dirty worktree should be removed");
        assert!(
            !branch_exists(&project_root, &wt.branch),
            "Branch should be removed"
        );
        let orphans = mgr.list_orphaned().await.expect("list_orphaned failed");
        assert!(orphans.is_empty(), "No orphaned worktrees should remain");
    }

    // --- Task 55.2: ID with hyphens and underscores ---

    /// Test: sanitize_task_id z myślnikami (np. "1.1-beta", "2.0-rc1")
    /// Myślniki są dozwolone, kropki zamieniamy na myślniki
    #[test]
    fn test_sanitize_task_id_with_hyphens() {
        // Myślniki są już dozwolone — przetrwają sanityzację
        assert_eq!(WorktreeManager::sanitize_task_id("1-beta"), "1-beta");
        assert_eq!(WorktreeManager::sanitize_task_id("2-0-rc1"), "2-0-rc1");

        // Kropki są zamieniane na myślniki
        assert_eq!(WorktreeManager::sanitize_task_id("1.1-beta"), "1-1-beta");
        assert_eq!(WorktreeManager::sanitize_task_id("2.0-rc1"), "2-0-rc1");
    }

    /// Test: sanitize_task_id z podkreśleniami (np. "T01_draft", "T01_v2")
    /// Podkreślenia są dozwolone
    #[test]
    fn test_sanitize_task_id_with_underscores() {
        assert_eq!(WorktreeManager::sanitize_task_id("T01_draft"), "T01_draft");
        assert_eq!(WorktreeManager::sanitize_task_id("T01_v2"), "T01_v2");
        assert_eq!(
            WorktreeManager::sanitize_task_id("TASK_001_FINAL"),
            "TASK_001_FINAL"
        );
    }

    /// Test: worktree_path z ID zawierającymi myślniki
    /// Sprawdza że ścieżka jest poprawnie generowana
    #[test]
    fn test_worktree_path_with_hyphens() {
        let mgr = WorktreeManager::new(PathBuf::from("/home/user/myproject"), None);

        let path = mgr.worktree_path("1.1-beta");
        assert_eq!(
            path,
            PathBuf::from("/home/user/myproject-ralph-task-1-1-beta")
        );

        let path = mgr.worktree_path("2.0-rc1");
        assert_eq!(
            path,
            PathBuf::from("/home/user/myproject-ralph-task-2-0-rc1")
        );
    }

    /// Test: worktree_path z ID zawierającymi podkreślenia
    #[test]
    fn test_worktree_path_with_underscores() {
        let mgr = WorktreeManager::new(PathBuf::from("/home/user/myproject"), None);

        let path = mgr.worktree_path("T01_draft");
        assert_eq!(
            path,
            PathBuf::from("/home/user/myproject-ralph-task-T01_draft")
        );

        let path = mgr.worktree_path("T01_v2");
        assert_eq!(
            path,
            PathBuf::from("/home/user/myproject-ralph-task-T01_v2")
        );

        let path = mgr.worktree_path("TASK_001_FINAL");
        assert_eq!(
            path,
            PathBuf::from("/home/user/myproject-ralph-task-TASK_001_FINAL")
        );
    }

    /// Test: branch_name z ID zawierającymi myślniki i podkreślenia
    /// Branch name używa oryginalnego ID (bez sanityzacji)
    #[test]
    fn test_branch_name_with_special_chars() {
        // Branch name NIE sanityzuje — używa raw ID
        assert_eq!(
            WorktreeManager::branch_name("1.1-beta"),
            "ralph/task/1.1-beta"
        );
        assert_eq!(
            WorktreeManager::branch_name("T01_draft"),
            "ralph/task/T01_draft"
        );
        assert_eq!(
            WorktreeManager::branch_name("2.0-rc1"),
            "ralph/task/2.0-rc1"
        );
        assert_eq!(
            WorktreeManager::branch_name("TASK_001_FINAL"),
            "ralph/task/TASK_001_FINAL"
        );
    }

    /// Test: is_ralph_branch z ID zawierającymi special chars
    /// Sprawdza że pattern matching działa z myślnikami i podkreśleniami
    #[test]
    fn test_is_ralph_branch_with_special_chars() {
        // New pattern: ralph/task/{task_id} z myślnikami i underscores
        assert!(WorktreeManager::is_ralph_branch("ralph/task/1.1-beta"));
        assert!(WorktreeManager::is_ralph_branch("ralph/task/T01_draft"));
        assert!(WorktreeManager::is_ralph_branch("ralph/task/2.0-rc1"));
        assert!(WorktreeManager::is_ralph_branch(
            "ralph/task/TASK_001_FINAL"
        ));
        assert!(WorktreeManager::is_ralph_branch("ralph/task/v1.2.3-beta_1"));

        // Legacy pattern: ralph/w{N}/{task_id}
        assert!(WorktreeManager::is_ralph_branch("ralph/w0/1.1-beta"));
        assert!(WorktreeManager::is_ralph_branch("ralph/w1/T01_draft"));
        assert!(WorktreeManager::is_ralph_branch("ralph/w99/2.0-rc1"));
    }

    /// Test: Mieszane ID (myślniki + podkreślenia + kropki) w worktree path
    /// Sprawdza poprawność sanityzacji dla złożonych ID
    #[test]
    fn test_worktree_path_mixed_special_chars() {
        let mgr = WorktreeManager::new(PathBuf::from("/home/user/project"), None);

        // "v1.2.3-beta_1" → kropki → myślniki, reszta bez zmian
        let path = mgr.worktree_path("v1.2.3-beta_1");
        assert_eq!(
            path,
            PathBuf::from("/home/user/project-ralph-task-v1-2-3-beta_1")
        );

        // "alpha-1_beta" → bez kropek, wszystko przetrwa
        let path = mgr.worktree_path("alpha-1_beta");
        assert_eq!(
            path,
            PathBuf::from("/home/user/project-ralph-task-alpha-1_beta")
        );

        // "T2_1-RC" → bez kropek
        let path = mgr.worktree_path("T2_1-RC");
        assert_eq!(path, PathBuf::from("/home/user/project-ralph-task-T2_1-RC"));
    }

    /// Test: Ścieżka worktree nie duplikuje separatorów dla ID z myślnikami
    /// Sprawdza że nie występują podwójne myślniki w ścieżce
    #[test]
    fn test_worktree_path_no_double_separators() {
        let mgr = WorktreeManager::new(PathBuf::from("/home/user/my-project"), None);

        let path = mgr.worktree_path("1.1-beta");
        let path_str = path.to_string_lossy();

        // Separator pomiędzy prefix a "task-" jest pojedynczy
        assert!(path_str.contains("my-project-ralph-task-"));
        assert!(!path_str.contains("---"), "Path nie może zawierać ---");
    }
}
