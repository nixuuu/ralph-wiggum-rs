//! Shutdown and orchestrator status types.

use std::time::Duration;

use crate::commands::task::orchestrate::scheduler::SchedulerStatus;

/// Shutdown phase for display purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShutdownState {
    #[default]
    Running,
    /// First q/Ctrl+C — waiting for in-progress tasks to finish
    Draining,
    /// Second q/Ctrl+C — force-aborting all workers
    Aborting,
}

/// Snapshot of the orchestrator's status for rendering.
#[derive(Debug, Clone)]
pub struct OrchestratorStatus {
    pub scheduler: SchedulerStatus,
    pub total_cost: f64,
    pub elapsed: Duration,
    pub shutdown_state: ShutdownState,
    /// Time remaining before force-kill escalation (only during Draining)
    pub shutdown_remaining: Option<Duration>,
    /// Whether the user is in quit confirmation state (first 'q' pressed)
    pub quit_pending: bool,
    /// Whether all tasks have completed (orchestrator in idle state)
    pub completed: bool,
    /// Worker restart pending confirmation: (worker_id, task_id)
    pub restart_pending: Option<(u32, String)>,
    /// Number of active (busy) workers
    pub active_workers: u32,
    /// Number of idle workers
    pub idle_workers: u32,
}
