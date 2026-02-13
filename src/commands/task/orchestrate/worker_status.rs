//! Worker status types for the orchestrator subsystem.

use crate::commands::task::orchestrate::events::WorkerPhase;

/// Per-worker status for display purposes.
#[derive(Debug, Clone)]
pub struct WorkerStatus {
    pub state: WorkerState,
    pub task_id: Option<String>,
    pub component: Option<String>,
    pub phase: Option<WorkerPhase>,
    /// Model alias (opus/sonnet/haiku) for the current task.
    /// Will be displayed in dashboard (task 11.1.2).
    #[allow(dead_code)]
    pub model: Option<String>,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl WorkerStatus {
    /// Create an idle worker status.
    pub fn idle(_worker_id: u32) -> Self {
        Self {
            state: WorkerState::Idle,
            task_id: None,
            component: None,
            phase: None,
            model: None,
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    /// Check if this worker is in Idle state.
    pub fn is_idle(&self) -> bool {
        self.state == WorkerState::Idle
    }
}

/// Worker state for display purposes.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerState {
    Idle,
    SettingUp,
    Implementing,
    Reviewing,
    Verifying,
    Merging,
    ResolvingConflicts,
}

impl std::fmt::Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerState::Idle => write!(f, "idle"),
            WorkerState::SettingUp => write!(f, "setting up"),
            WorkerState::Implementing => write!(f, "implementing"),
            WorkerState::Reviewing => write!(f, "reviewing"),
            WorkerState::Verifying => write!(f, "verifying"),
            WorkerState::Merging => write!(f, "merging"),
            WorkerState::ResolvingConflicts => write!(f, "resolving conflicts"),
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── WorkerStatus model field tests ───────────────────────────────

    #[test]
    fn test_worker_status_idle_model_is_none() {
        let status = WorkerStatus::idle(0);
        assert_eq!(status.model, None);
    }

    #[test]
    fn test_worker_status_with_model_set() {
        let mut status = WorkerStatus::idle(1);
        status.model = Some("sonnet".to_string());
        assert_eq!(status.model, Some("sonnet".to_string()));
    }

    #[test]
    fn test_worker_status_model_preserved_on_clone() {
        let mut status = WorkerStatus::idle(2);
        status.model = Some("sonnet".to_string());

        let cloned = status.clone();
        assert_eq!(cloned.model, Some("sonnet".to_string()));
        assert_eq!(status.model, cloned.model);
    }

    #[test]
    fn test_worker_status_model_update_on_resolving_conflicts() {
        let mut status = WorkerStatus::idle(3);
        status.model = Some("sonnet".to_string());
        status.state = WorkerState::ResolvingConflicts;

        assert_eq!(status.state, WorkerState::ResolvingConflicts);
        assert_eq!(status.model, Some("sonnet".to_string()));
    }

    #[test]
    fn test_worker_status_multiple_models() {
        let mut status1 = WorkerStatus::idle(4);
        status1.model = Some("opus".to_string());

        let mut status2 = WorkerStatus::idle(5);
        status2.model = Some("haiku".to_string());

        assert_eq!(status1.model, Some("opus".to_string()));
        assert_eq!(status2.model, Some("haiku".to_string()));
    }
}
