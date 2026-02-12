//! Post-loop cleanup logic for orchestration session.

use std::time::Instant;

use crate::commands::task::orchestrate::orchestrator::Orchestrator;
use crate::commands::task::orchestrate::run_loop::RunLoopContext;
use crate::shared::error::Result;

use super::assignment::WorkerSlot;

// ── Cleanup ─────────────────────────────────────────────────────────

impl Orchestrator {
    /// Perform cleanup after the main orchestration loop finishes.
    ///
    /// This function:
    /// 1. Cleans up orphaned worktrees from pending merges
    /// 2. Saves orchestrator state
    /// 3. Releases lockfile
    pub(in crate::commands::task::orchestrate) async fn post_loop_cleanup(
        &self,
        ctx: &mut RunLoopContext<'_>,
        _started_at: Instant,
    ) -> Result<()> {
        // Clean up orphaned worktrees from pending merges
        for pending in ctx.merge_ctx.pending_merges.iter() {
            if let Some(WorkerSlot::Busy { worktree, .. }) =
                ctx.worker_slots.get(&pending.worker_id)
            {
                ctx.worktree_manager
                    .remove_worktree(&worktree.path)
                    .await
                    .ok();
                ctx.worktree_manager
                    .remove_branch(&worktree.branch)
                    .await
                    .ok();
            }
        }

        if let Err(e) = ctx.state.save(ctx.state_path)
            && self.config.verbose
        {
            eprintln!("Warning: Failed to save orchestrator state: {e}");
        }

        if let Some(lf) = ctx.lockfile.take()
            && let Err(e) = lf.release()
            && self.config.verbose
        {
            eprintln!("Warning: Failed to release lockfile: {e}");
        }

        Ok(())
    }
}
