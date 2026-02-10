#![allow(dead_code)]
use std::time::Instant;

use crate::commands::task::orchestrate::events::WorkerPhase;
use crate::commands::task::orchestrate::scheduler::SchedulerStatus;

/// Per-worker status for the status bar display.
#[derive(Debug, Clone)]
pub struct WorkerStatus {
    pub worker_id: u32,
    pub state: WorkerState,
    pub task_id: Option<String>,
    pub component: Option<String>,
    pub phase: Option<WorkerPhase>,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Worker state for display purposes.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerState {
    Idle,
    Implementing,
    Reviewing,
    Verifying,
    Merging,
}

impl std::fmt::Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerState::Idle => write!(f, "idle"),
            WorkerState::Implementing => write!(f, "implementing"),
            WorkerState::Reviewing => write!(f, "reviewing"),
            WorkerState::Verifying => write!(f, "verifying"),
            WorkerState::Merging => write!(f, "merging"),
        }
    }
}

/// Snapshot of the orchestrator's status for rendering.
#[derive(Debug, Clone)]
pub struct OrchestratorStatus {
    pub scheduler: SchedulerStatus,
    pub workers: Vec<WorkerStatus>,
    pub total_cost: f64,
    pub elapsed: std::time::Duration,
}

/// Status bar renderer for the orchestrator.
///
/// Renders an N+2 line status bar:
/// - Line 1: Overall progress (bar + stats)
/// - Lines 2..N+1: Per-worker status
/// - Line N+2: Queue info
pub struct OrchestratorStatusBar {
    started_at: Instant,
    worker_count: u32,
}

impl OrchestratorStatusBar {
    pub fn new(worker_count: u32) -> Self {
        Self {
            started_at: Instant::now(),
            worker_count,
        }
    }

    /// Total number of lines this status bar occupies.
    pub fn line_count(&self) -> u16 {
        self.worker_count as u16 + 2
    }

    /// Render the overall progress line.
    pub fn render_progress_line(&self, status: &OrchestratorStatus) -> String {
        let total = status.scheduler.total;
        let done = status.scheduler.done;
        let pct = if total > 0 {
            (done * 100) / total
        } else {
            0
        };

        let bar = render_progress_bar(done, total, 20);
        let elapsed = format_duration(status.elapsed);
        let cost_per_task = if done > 0 {
            status.total_cost / done as f64
        } else {
            0.0
        };

        format!(
            "{bar} {done}/{total} ({pct}%) | ${:.4} | ${:.4}/task | {elapsed}",
            status.total_cost, cost_per_task
        )
    }

    /// Render a single worker status line.
    pub fn render_worker_line(&self, worker: &WorkerStatus) -> String {
        let icon = match &worker.state {
            WorkerState::Idle => "○",
            WorkerState::Implementing => "●",
            WorkerState::Reviewing => "◎",
            WorkerState::Verifying => "◉",
            WorkerState::Merging => "⊕",
        };

        let task = worker
            .task_id
            .as_deref()
            .unwrap_or("---");
        let component = worker
            .component
            .as_deref()
            .unwrap_or("");
        let phase = worker
            .phase
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_else(|| worker.state.to_string());

        format!(
            "  W{}: {icon} {task} [{component}] {phase} | ${:.4}",
            worker.worker_id, worker.cost_usd
        )
    }

    /// Render the queue info line.
    pub fn render_queue_line(&self, status: &OrchestratorStatus) -> String {
        format!(
            "  Queue: {} ready | {} in-progress | {} blocked | {} pending",
            status.scheduler.ready,
            status.scheduler.in_progress,
            status.scheduler.blocked,
            status.scheduler.pending
        )
    }

    /// Render all status bar lines as a single string.
    pub fn render(&self, status: &OrchestratorStatus) -> String {
        let mut lines = Vec::new();
        lines.push(self.render_progress_line(status));
        for worker in &status.workers {
            lines.push(self.render_worker_line(worker));
        }
        lines.push(self.render_queue_line(status));
        lines.join("\n")
    }
}

/// Render a Unicode progress bar.
fn render_progress_bar(done: usize, total: usize, width: usize) -> String {
    if total == 0 {
        return format!("[{}]", " ".repeat(width));
    }
    let filled = (done * width) / total;
    let empty = width - filled;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

/// Format a duration as human-readable string.
fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_progress_bar() {
        assert_eq!(render_progress_bar(0, 10, 10), "[░░░░░░░░░░]");
        assert_eq!(render_progress_bar(5, 10, 10), "[█████░░░░░]");
        assert_eq!(render_progress_bar(10, 10, 10), "[██████████]");
        assert_eq!(render_progress_bar(0, 0, 10), "[          ]");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(std::time::Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(std::time::Duration::from_secs(90)), "1m30s");
        assert_eq!(
            format_duration(std::time::Duration::from_secs(3661)),
            "1h1m"
        );
    }

    #[test]
    fn test_render_progress_line() {
        let bar = OrchestratorStatusBar::new(2);
        let status = OrchestratorStatus {
            scheduler: SchedulerStatus {
                total: 10,
                done: 5,
                in_progress: 2,
                blocked: 1,
                ready: 2,
                pending: 0,
            },
            workers: vec![],
            total_cost: 0.25,
            elapsed: std::time::Duration::from_secs(120),
        };

        let line = bar.render_progress_line(&status);
        assert!(line.contains("5/10"));
        assert!(line.contains("50%"));
        assert!(line.contains("$0.25"));
        assert!(line.contains("2m0s"));
    }

    #[test]
    fn test_render_worker_line_idle() {
        let bar = OrchestratorStatusBar::new(2);
        let worker = WorkerStatus {
            worker_id: 1,
            state: WorkerState::Idle,
            task_id: None,
            component: None,
            phase: None,
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
        };

        let line = bar.render_worker_line(&worker);
        assert!(line.contains("W1"));
        assert!(line.contains("○"));
        assert!(line.contains("---"));
        assert!(line.contains("idle"));
    }

    #[test]
    fn test_render_worker_line_busy() {
        let bar = OrchestratorStatusBar::new(2);
        let worker = WorkerStatus {
            worker_id: 2,
            state: WorkerState::Implementing,
            task_id: Some("T03".to_string()),
            component: Some("api".to_string()),
            phase: Some(WorkerPhase::Implement),
            cost_usd: 0.042,
            input_tokens: 1200,
            output_tokens: 500,
        };

        let line = bar.render_worker_line(&worker);
        assert!(line.contains("W2"));
        assert!(line.contains("●"));
        assert!(line.contains("T03"));
        assert!(line.contains("api"));
        assert!(line.contains("implement"));
    }

    #[test]
    fn test_render_queue_line() {
        let bar = OrchestratorStatusBar::new(2);
        let status = OrchestratorStatus {
            scheduler: SchedulerStatus {
                total: 10,
                done: 3,
                in_progress: 2,
                blocked: 1,
                ready: 3,
                pending: 1,
            },
            workers: vec![],
            total_cost: 0.0,
            elapsed: std::time::Duration::from_secs(0),
        };

        let line = bar.render_queue_line(&status);
        assert!(line.contains("3 ready"));
        assert!(line.contains("2 in-progress"));
        assert!(line.contains("1 blocked"));
        assert!(line.contains("1 pending"));
    }

    #[test]
    fn test_line_count() {
        let bar = OrchestratorStatusBar::new(3);
        assert_eq!(bar.line_count(), 5); // 1 progress + 3 workers + 1 queue
    }

    #[test]
    fn test_worker_state_display() {
        assert_eq!(WorkerState::Idle.to_string(), "idle");
        assert_eq!(WorkerState::Implementing.to_string(), "implementing");
        assert_eq!(WorkerState::Reviewing.to_string(), "reviewing");
        assert_eq!(WorkerState::Verifying.to_string(), "verifying");
        assert_eq!(WorkerState::Merging.to_string(), "merging");
    }

    #[test]
    fn test_full_render() {
        let bar = OrchestratorStatusBar::new(2);
        let status = OrchestratorStatus {
            scheduler: SchedulerStatus {
                total: 10,
                done: 3,
                in_progress: 2,
                blocked: 0,
                ready: 5,
                pending: 0,
            },
            workers: vec![
                WorkerStatus {
                    worker_id: 1,
                    state: WorkerState::Implementing,
                    task_id: Some("T04".to_string()),
                    component: Some("ui".to_string()),
                    phase: Some(WorkerPhase::Implement),
                    cost_usd: 0.01,
                    input_tokens: 100,
                    output_tokens: 50,
                },
                WorkerStatus {
                    worker_id: 2,
                    state: WorkerState::Idle,
                    task_id: None,
                    component: None,
                    phase: None,
                    cost_usd: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                },
            ],
            total_cost: 0.01,
            elapsed: std::time::Duration::from_secs(45),
        };

        let output = bar.render(&status);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 4); // progress + 2 workers + queue
        assert!(lines[0].contains("3/10"));
        assert!(lines[1].contains("T04"));
        assert!(lines[2].contains("---"));
        assert!(lines[3].contains("Queue"));
    }
}
