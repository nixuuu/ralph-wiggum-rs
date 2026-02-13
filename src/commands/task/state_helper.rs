//! Helpers for updating state file after task mutations.

use crate::commands::run::state::StateManager;
use crate::shared::error::{RalphError, Result};
use crate::shared::progress::ProgressSummary;
use std::path::Path;

/// Maximum additional iterations to add beyond remaining tasks.
const MAX_ITERATIONS_BUFFER: u32 = 5;

/// Update the state file (if it exists) with new min_iterations from updated task count.
///
/// This adjusts the iteration limits in `.claude/ralph-loop.local.md` to reflect
/// the new task count after add/edit operations.
///
/// # Arguments
/// * `state_path` - Path to the state file (typically `.claude/ralph-loop.local.md`)
/// * `summary` - Updated progress summary with current task counts
pub fn update_state_file(state_path: &Path, summary: &ProgressSummary) -> Result<()> {
    if !state_path.exists() {
        return Ok(());
    }

    let (mut state, prompt) = StateManager::load_from_file(state_path)?;
    let remaining = summary.remaining() as u32;
    let new_min = state.iteration.saturating_add(remaining);
    state.min_iterations = new_min.max(state.iteration);
    state.max_iterations = new_min + MAX_ITERATIONS_BUFFER;

    // Save state back
    let yaml = serde_yaml::to_string(&state)?;
    let content = format!("---\n{}---\n\n{}", yaml, prompt);
    std::fs::write(state_path, content)
        .map_err(|e| RalphError::StateFile(format!("Failed to write state file: {}", e)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_iterations_buffer_constant() {
        assert_eq!(MAX_ITERATIONS_BUFFER, 5);
    }
}
