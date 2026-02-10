use crate::commands::task::orchestrate::state::Lockfile;
use crate::commands::task::orchestrate::worktree::WorktreeManager;
use crate::shared::error::Result;
use crate::shared::file_config::FileConfig;

/// Execute the `task clean` command — interactive cleanup of orchestration resources.
///
/// Scans for orphaned worktrees, branches, state files, and logs,
/// then removes them after confirmation.
pub async fn execute(file_config: &FileConfig) -> Result<()> {
    let project_root = std::env::current_dir()?;
    let ralph_dir = project_root.join(".ralph");

    // Safety check: don't clean if there's an active session
    let lock_path = ralph_dir.join("orchestrate.lock");
    if lock_path.exists() && !Lockfile::is_stale(&lock_path) {
        eprintln!("Active orchestration session detected (lockfile is fresh).");
        eprintln!("Wait for the session to complete or remove the lock manually.");
        return Ok(());
    }

    let worktree_mgr = WorktreeManager::new(
        project_root.clone(),
        file_config.task.orchestrate.worktree_prefix.clone(),
    );

    // 1. Scan for orphaned worktrees
    let orphans = worktree_mgr.list_orphaned().await?;

    // 2. Scan for state files
    let state_path = ralph_dir.join("orchestrate.yaml");
    let log_dir = ralph_dir.join("logs");

    let has_state = state_path.exists();
    let has_logs = log_dir.exists();
    let has_lock = lock_path.exists();

    if orphans.is_empty() && !has_state && !has_logs && !has_lock {
        eprintln!("Nothing to clean up.");
        return Ok(());
    }

    // Display findings
    eprintln!("Found orchestration resources to clean:\n");

    if !orphans.is_empty() {
        eprintln!("  Worktrees ({}):", orphans.len());
        for orphan in &orphans {
            eprintln!("    {} (branch: {})", orphan.path.display(), orphan.branch);
        }
    }

    if has_state {
        eprintln!("  State file: {}", state_path.display());
    }
    if has_logs {
        let log_count = std::fs::read_dir(&log_dir)
            .map(|d| d.count())
            .unwrap_or(0);
        eprintln!("  Log files: {} in {}", log_count, log_dir.display());
    }
    if has_lock {
        eprintln!("  Stale lockfile: {}", lock_path.display());
    }

    eprintln!("\nCleaning up...\n");

    // Clean worktrees
    for orphan in &orphans {
        match worktree_mgr.remove_worktree(&orphan.path).await {
            Ok(()) => eprintln!("  Removed worktree: {}", orphan.path.display()),
            Err(e) => eprintln!("  Failed to remove worktree {}: {e}", orphan.path.display()),
        }
        // Also remove the branch
        worktree_mgr.remove_branch(&orphan.branch).await.ok();
    }

    // Prune git worktree tracking
    worktree_mgr.prune().await.ok();

    // Clean state files
    if has_state {
        std::fs::remove_file(&state_path).ok();
        eprintln!("  Removed state file");
    }

    // Clean logs
    if has_logs {
        std::fs::remove_dir_all(&log_dir).ok();
        eprintln!("  Removed log directory");
    }

    // Clean stale lock
    if has_lock {
        std::fs::remove_file(&lock_path).ok();
        eprintln!("  Removed stale lockfile");
    }

    eprintln!("\nCleanup complete.");
    Ok(())
}
