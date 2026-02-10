#![allow(dead_code)]
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// ANSI color codes for worker output differentiation.
const WORKER_COLORS: [&str; 8] = [
    "\x1b[36m", // cyan
    "\x1b[33m", // yellow
    "\x1b[35m", // magenta
    "\x1b[32m", // green
    "\x1b[34m", // blue
    "\x1b[91m", // bright red
    "\x1b[96m", // bright cyan
    "\x1b[93m", // bright yellow
];
const ORCH_COLOR: &str = "\x1b[1;37m"; // bold white
const RESET: &str = "\x1b[0m";

/// Multiplexed output renderer for orchestration workers.
///
/// Each worker gets a deterministic color, and output lines are prefixed
/// with `[W{N}|{task_id}]`. Orchestrator messages use `[ORCH]`.
pub struct MultiplexedOutput {
    /// Per-worker cost tracking: worker_id → (cost_usd, input_tokens, output_tokens)
    worker_costs: HashMap<u32, (f64, u64, u64)>,
    /// Per-worker current task assignment
    worker_tasks: HashMap<u32, String>,
    /// Combined JSONL log file (optional)
    combined_log: Option<std::fs::File>,
}

impl MultiplexedOutput {
    /// Create a new MultiplexedOutput with optional combined log file.
    pub fn new(combined_log_path: Option<&Path>) -> Self {
        let combined_log = combined_log_path.and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        });

        Self {
            worker_costs: HashMap::new(),
            worker_tasks: HashMap::new(),
            combined_log,
        }
    }

    /// Assign a task to a worker (for prefix display).
    pub fn assign_worker(&mut self, worker_id: u32, task_id: &str) {
        self.worker_tasks
            .insert(worker_id, task_id.to_string());
    }

    /// Clear a worker's task assignment (when idle).
    pub fn clear_worker(&mut self, worker_id: u32) {
        self.worker_tasks.remove(&worker_id);
    }

    /// Get the ANSI color for a worker (deterministic, hash-based).
    pub fn worker_color(worker_id: u32) -> &'static str {
        WORKER_COLORS[(worker_id as usize) % WORKER_COLORS.len()]
    }

    /// Format a worker output line with colored prefix.
    pub fn format_worker_line(&self, worker_id: u32, text: &str) -> String {
        let color = Self::worker_color(worker_id);
        let task_id = self
            .worker_tasks
            .get(&worker_id)
            .map(|s| s.as_str())
            .unwrap_or("---");
        format!("{color}[W{worker_id}|{task_id}]{RESET} {text}")
    }

    /// Format an orchestrator message line.
    pub fn format_orchestrator_line(text: &str) -> String {
        format!("{ORCH_COLOR}[ORCH]{RESET} {text}")
    }

    /// Update cost tracking for a worker.
    pub fn update_cost(
        &mut self,
        worker_id: u32,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        let entry = self.worker_costs.entry(worker_id).or_insert((0.0, 0, 0));
        entry.0 += cost_usd;
        entry.1 += input_tokens;
        entry.2 += output_tokens;
    }

    /// Get cost for a specific worker.
    pub fn worker_cost(&self, worker_id: u32) -> (f64, u64, u64) {
        self.worker_costs
            .get(&worker_id)
            .copied()
            .unwrap_or((0.0, 0, 0))
    }

    /// Get total cost across all workers.
    pub fn total_cost(&self) -> f64 {
        self.worker_costs.values().map(|(c, _, _)| c).sum()
    }

    /// Get total tokens across all workers.
    pub fn total_tokens(&self) -> (u64, u64) {
        let input: u64 = self.worker_costs.values().map(|(_, i, _)| i).sum();
        let output: u64 = self.worker_costs.values().map(|(_, _, o)| o).sum();
        (input, output)
    }

    /// Write a line to the combined log (if configured).
    pub fn log_line(&mut self, line: &str) {
        if let Some(log) = &mut self.combined_log {
            writeln!(log, "{line}").ok();
        }
    }

    /// Get the log file path (for reference).
    pub fn log_path(&self) -> Option<PathBuf> {
        // We don't store the path, but callers can track it
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_color_deterministic() {
        // Same worker_id always gets same color
        assert_eq!(
            MultiplexedOutput::worker_color(1),
            MultiplexedOutput::worker_color(1)
        );
        // Different workers may get different colors
        let c1 = MultiplexedOutput::worker_color(1);
        let c2 = MultiplexedOutput::worker_color(2);
        // They cycle through 8 colors, so 1 and 9 would be same
        assert_eq!(c1, MultiplexedOutput::worker_color(9));
        assert_ne!(c1, c2);
    }

    #[test]
    fn test_format_worker_line() {
        let mut output = MultiplexedOutput::new(None);
        output.assign_worker(1, "T01");

        let line = output.format_worker_line(1, "Hello world");
        assert!(line.contains("[W1|T01]"));
        assert!(line.contains("Hello world"));
        assert!(line.contains(RESET));
    }

    #[test]
    fn test_format_worker_line_no_task() {
        let output = MultiplexedOutput::new(None);
        let line = output.format_worker_line(1, "idle message");
        assert!(line.contains("[W1|---]"));
    }

    #[test]
    fn test_format_orchestrator_line() {
        let line = MultiplexedOutput::format_orchestrator_line("Starting session");
        assert!(line.contains("[ORCH]"));
        assert!(line.contains("Starting session"));
    }

    #[test]
    fn test_cost_tracking() {
        let mut output = MultiplexedOutput::new(None);

        output.update_cost(1, 0.01, 100, 50);
        output.update_cost(1, 0.02, 200, 100);
        output.update_cost(2, 0.05, 500, 250);

        let (cost, input, out) = output.worker_cost(1);
        assert!((cost - 0.03).abs() < f64::EPSILON);
        assert_eq!(input, 300);
        assert_eq!(out, 150);

        assert!((output.total_cost() - 0.08).abs() < f64::EPSILON);

        let (total_in, total_out) = output.total_tokens();
        assert_eq!(total_in, 800);
        assert_eq!(total_out, 400);
    }

    #[test]
    fn test_worker_cost_unknown() {
        let output = MultiplexedOutput::new(None);
        let (cost, input, out) = output.worker_cost(99);
        assert_eq!(cost, 0.0);
        assert_eq!(input, 0);
        assert_eq!(out, 0);
    }

    #[test]
    fn test_assign_clear_worker() {
        let mut output = MultiplexedOutput::new(None);

        output.assign_worker(1, "T03");
        let line = output.format_worker_line(1, "text");
        assert!(line.contains("T03"));

        output.clear_worker(1);
        let line = output.format_worker_line(1, "text");
        assert!(line.contains("---"));
    }
}
