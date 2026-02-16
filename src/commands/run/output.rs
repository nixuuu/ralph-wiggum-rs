use crossterm::style::Stylize;
use std::collections::HashMap;
use std::time::Instant;

use super::event_formatting::{BlockType, TokenState, format_event};
use super::formatting_helpers::{format_duration_short, format_tokens};
use super::runner::ClaudeEvent;
use super::ui::StatusData;
use crate::shared::icons;

pub struct OutputFormatter {
    iteration: u32,
    min_iterations: u32,
    max_iterations: u32,
    start_time: Instant,
    iteration_start_time: Instant,
    total_cost_usd: f64,
    /// Finalized tokens from completed iterations (from modelUsage in result events)
    finalized_input_tokens: u64,
    finalized_output_tokens: u64,
    /// Pending tokens from current iteration's assistant messages (live display)
    pending_input_tokens: u64,
    pending_output_tokens: u64,
    /// Per-model cost breakdown
    model_costs: HashMap<String, f64>,
    last_block_type: BlockType,
    /// Tracks tool_use_id → tool_name for displaying ask_user results
    tool_use_names: HashMap<String, String>,
    use_nerd_font: bool,
    task_progress: Option<TaskProgress>,
    /// Number of done tasks at session start (baseline for speed calculation)
    initial_done_count: usize,
    /// History of iteration durations in seconds
    iteration_durations: Vec<f64>,
}

impl OutputFormatter {
    pub fn new(use_nerd_font: bool) -> Self {
        let now = Instant::now();
        Self {
            iteration: 0,
            min_iterations: 0,
            max_iterations: 0,
            start_time: now,
            iteration_start_time: now,
            total_cost_usd: 0.0,
            finalized_input_tokens: 0,
            finalized_output_tokens: 0,
            pending_input_tokens: 0,
            pending_output_tokens: 0,
            model_costs: HashMap::new(),
            last_block_type: BlockType::None,
            tool_use_names: HashMap::new(),
            use_nerd_font,
            task_progress: None,
            initial_done_count: 0,
            iteration_durations: Vec::new(),
        }
    }

    pub fn set_iteration(&mut self, iteration: u32) {
        self.iteration = iteration;
    }

    pub fn set_min_iterations(&mut self, min: u32) {
        self.min_iterations = min;
    }

    pub fn set_max_iterations(&mut self, max: u32) {
        self.max_iterations = max;
    }

    /// Start a new iteration - reset iteration timer, block type, and pending tokens
    pub fn start_iteration(&mut self) {
        self.iteration_start_time = Instant::now();
        self.last_block_type = BlockType::None;
        self.pending_input_tokens = 0;
        self.pending_output_tokens = 0;
    }

    /// Total input tokens for display (finalized + pending from current iteration)
    fn display_input_tokens(&self) -> u64 {
        self.finalized_input_tokens + self.pending_input_tokens
    }

    /// Total output tokens for display (finalized + pending from current iteration)
    fn display_output_tokens(&self) -> u64 {
        self.finalized_output_tokens + self.pending_output_tokens
    }

    pub fn set_task_progress(&mut self, progress: Option<TaskProgress>) {
        self.task_progress = progress;
    }

    /// Set the baseline done count at session start
    pub fn set_initial_done_count(&mut self, count: usize) {
        self.initial_done_count = count;
    }

    /// Record iteration completion for speed tracking.
    /// Must be called after set_task_progress.
    pub fn record_iteration_end(&mut self) {
        let duration = self.iteration_start_time.elapsed().as_secs_f64();
        self.iteration_durations.push(duration);
    }

    /// Tasks completed during this session
    fn tasks_completed_this_session(&self) -> usize {
        let current_done = self.task_progress.as_ref().map_or(0, |tp| tp.done);
        current_done.saturating_sub(self.initial_done_count)
    }

    /// Compute speed text (tasks/hour) or None if no tasks completed yet
    fn compute_speed_text(&self) -> Option<String> {
        let completed = self.tasks_completed_this_session();
        if completed == 0 {
            return None;
        }
        let elapsed_hours = self.start_time.elapsed().as_secs_f64() / 3600.0;
        if elapsed_hours < 0.001 {
            return None;
        }
        let rate = completed as f64 / elapsed_hours;
        Some(format!("{:.1}/h", rate))
    }

    /// Compute ETA text or None if no tasks completed or nothing remaining
    fn compute_eta_text(&self) -> Option<String> {
        let completed = self.tasks_completed_this_session();
        if completed == 0 {
            return None;
        }
        let remaining = self
            .task_progress
            .as_ref()
            .map_or(0, |tp| tp.todo + tp.in_progress);
        if remaining == 0 {
            return None;
        }
        let elapsed_secs = self.start_time.elapsed().as_secs_f64();
        let secs_per_task = elapsed_secs / completed as f64;
        let eta_secs = (remaining as f64 * secs_per_task) as u64;
        Some(format_duration_short(eta_secs))
    }

    /// Average iteration duration in seconds, or None if no iterations recorded
    fn avg_iteration_secs(&self) -> Option<f64> {
        if self.iteration_durations.is_empty() {
            return None;
        }
        Some(self.iteration_durations.iter().sum::<f64>() / self.iteration_durations.len() as f64)
    }

    /// Get current status data for the status bar
    pub fn get_status(&self) -> StatusData {
        StatusData {
            iteration: self.iteration,
            min_iterations: self.min_iterations,
            max_iterations: self.max_iterations,
            iteration_elapsed_secs: self.iteration_start_time.elapsed().as_secs_f64(),
            input_tokens: self.display_input_tokens(),
            output_tokens: self.display_output_tokens(),
            cost_usd: self.total_cost_usd,
            update_info: None,
            update_state: Default::default(),
            task_progress: self.task_progress.clone(),
            speed_text: self.compute_speed_text(),
            eta_text: self.compute_eta_text(),
        }
    }

    /// Format token summary lines for stats display
    fn format_token_lines(&self) -> Vec<String> {
        let input = self.display_input_tokens();
        let output = self.display_output_tokens();
        if input > 0 || output > 0 {
            vec![format!(
                "  {}    {} {} {} {}",
                "Tokens:".dark_grey(),
                format_tokens(input).green(),
                "in /".dark_grey(),
                format_tokens(output).magenta(),
                "out".dark_grey()
            )]
        } else {
            vec![]
        }
    }

    /// Format speed/throughput lines for stats display
    fn format_speed_lines(&self) -> Vec<String> {
        let completed = self.tasks_completed_this_session();
        if completed == 0 {
            return vec![];
        }
        let elapsed_h = self.start_time.elapsed().as_secs_f64() / 3600.0;
        let rate = if elapsed_h > 0.001 {
            format!("{:.1}/h", completed as f64 / elapsed_h)
        } else {
            "—".to_string()
        };
        let mut lines = vec![format!(
            "  {}     {} {} {}",
            "Speed:".dark_grey(),
            completed.to_string().green(),
            "tasks |".dark_grey(),
            rate
        )];
        if let Some(avg) = self.avg_iteration_secs() {
            lines.push(format!("  {}   {:.0}s", "Avg iter:".dark_grey(), avg));
        }
        lines
    }

    /// Format cost lines with per-model breakdown for stats display
    fn format_cost_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.total_cost_usd > 0.0 {
            lines.push(format!(
                "  {}      {}",
                "Cost:".dark_grey(),
                format!("${:.4}", self.total_cost_usd).yellow()
            ));
            if !self.model_costs.is_empty() {
                let mut sorted: Vec<_> = self.model_costs.iter().collect();
                sorted.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
                for (model, cost) in &sorted {
                    lines.push(format!(
                        "            {} {}",
                        format!("${:.4}", cost).dark_grey(),
                        model.as_str().dark_grey()
                    ));
                }
            }
        }
        lines
    }

    /// Format iteration header and return lines
    pub fn format_iteration_header(&self) -> Vec<String> {
        let elapsed = self.start_time.elapsed();
        let mut header = format!(
            "{} {} {} {} {:.1}s",
            "▶".cyan(),
            "Iteration".bold(),
            self.iteration.to_string().cyan().bold(),
            "│ Elapsed:".dark_grey(),
            elapsed.as_secs_f64()
        );

        if let Some(avg) = self.avg_iteration_secs() {
            header.push_str(&format!(" {} {:.0}s/iter", "│".dark_grey(), avg));
        }

        vec![
            String::new(),
            format!("{}", "━".repeat(60).dark_grey()),
            header,
            format!("{}", "━".repeat(60).dark_grey()),
        ]
    }

    /// Format a claude event and return lines to print
    pub fn format_event(&mut self, event: &ClaudeEvent) -> Vec<String> {
        let mut tokens = TokenState {
            finalized_input_tokens: &mut self.finalized_input_tokens,
            finalized_output_tokens: &mut self.finalized_output_tokens,
            pending_input_tokens: &mut self.pending_input_tokens,
            pending_output_tokens: &mut self.pending_output_tokens,
            total_cost_usd: &mut self.total_cost_usd,
            model_costs: &mut self.model_costs,
        };
        format_event(
            event,
            &mut self.last_block_type,
            &mut self.tool_use_names,
            self.use_nerd_font,
            &mut tokens,
        )
    }

    /// Format final statistics and return lines
    pub fn format_stats(&self, iterations: u32, found_promise: bool, promise: &str) -> Vec<String> {
        let elapsed = self.start_time.elapsed();
        let mut lines = vec![String::new(), format!("{}", "━".repeat(60).dark_grey())];

        if found_promise {
            lines.push(format!(
                "{} {} {}",
                icons::status_check(self.use_nerd_font).green().bold(),
                "COMPLETED".green().bold(),
                format!("- Promise found: <promise>{}</promise>", promise).dark_grey()
            ));
        } else {
            lines.push(format!(
                "{} {}",
                icons::status_fail(self.use_nerd_font).red().bold(),
                "STOPPED - Promise not found".red()
            ));
        }

        lines.push(format!("{}", "━".repeat(60).dark_grey()));
        lines.push(format!(
            "  {} {}",
            "Iterations:".dark_grey(),
            iterations.to_string().white().bold()
        ));
        lines.push(format!(
            "  {}      {:.2}s",
            "Time:".dark_grey(),
            elapsed.as_secs_f64()
        ));

        lines.extend(self.format_speed_lines());
        lines.extend(self.format_token_lines());
        lines.extend(self.format_cost_lines());

        lines.push(format!("{}", "━".repeat(60).dark_grey()));
        lines
    }

    /// Format interruption message and return lines
    pub fn format_interrupted(&self, iterations: u32) -> Vec<String> {
        let elapsed = self.start_time.elapsed();
        let mut lines = vec![
            String::new(),
            format!("{}", "━".repeat(60).dark_grey()),
            format!(
                "{} {} {}",
                icons::status_pause(self.use_nerd_font).yellow().bold(),
                "INTERRUPTED".yellow().bold(),
                "- State saved".dark_grey()
            ),
            format!("{}", "━".repeat(60).dark_grey()),
            format!(
                "  {} {}",
                "Iterations:".dark_grey(),
                iterations.to_string().white().bold()
            ),
            format!(
                "  {}      {:.2}s",
                "Time:".dark_grey(),
                elapsed.as_secs_f64()
            ),
        ];

        lines.extend(self.format_speed_lines());
        lines.extend(self.format_token_lines());
        lines.extend(self.format_cost_lines());

        lines.push(String::new());
        lines.push(format!(
            "  {} {}",
            "Resume:".dark_grey(),
            "ralph-wiggum --resume".cyan()
        ));
        lines.push(format!("{}", "━".repeat(60).dark_grey()));
        lines
    }
}

/// Task progress data for enhanced status bar
#[derive(Debug, Clone, Default)]
pub struct TaskProgress {
    pub total: usize,
    pub done: usize,
    pub in_progress: usize,
    pub blocked: usize,
    pub todo: usize,
    pub current_task_id: Option<String>,
    pub current_task_name: Option<String>,
    pub current_task_component: Option<String>,
}

impl TaskProgress {
    /// Build a ratatui Line for the status bar (line 2 of 3)
    pub fn to_status_line(&self) -> ratatui::text::Line<'static> {
        use ratatui::style::{Color, Style};
        use ratatui::text::Span;

        let mut spans = Vec::new();

        if let (Some(id), Some(component)) = (&self.current_task_id, &self.current_task_component) {
            spans.push(Span::styled("▶ ", Style::default().fg(Color::Cyan)));
            spans.push(Span::styled(
                id.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ));
            spans.push(Span::raw(" ["));
            spans.push(Span::styled(
                component.clone(),
                Style::default().fg(Color::Yellow),
            ));
            spans.push(Span::raw("] "));
        }

        if let Some(name) = &self.current_task_name {
            spans.push(Span::raw(name.clone()));
        }

        spans.push(Span::raw(" │ "));
        spans.push(Span::styled(
            format!("✓{}", self.done),
            Style::default().fg(Color::Green),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("~{}", self.in_progress),
            Style::default().fg(Color::Cyan),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("!{}", self.blocked),
            Style::default().fg(Color::Red),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("○{}", self.todo),
            Style::default().fg(Color::DarkGray),
        ));

        ratatui::text::Line::from(spans)
    }
}

impl Default for OutputFormatter {
    fn default() -> Self {
        Self::new(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_formatter_zero_values() {
        // Create OutputFormatter without any updates (0 tokens, 0 cost)
        let formatter = OutputFormatter::new(true);

        // Verify zero token counts
        assert_eq!(formatter.display_input_tokens(), 0);
        assert_eq!(formatter.display_output_tokens(), 0);

        // Verify zero cost
        assert_eq!(formatter.total_cost_usd, 0.0);

        // Format token lines with zero values
        let token_lines = formatter.format_token_lines();
        // With zero tokens, should return empty vec (no lines)
        assert_eq!(token_lines.len(), 0);

        // Format cost lines with zero cost
        let cost_lines = formatter.format_cost_lines();
        // With zero cost, should return empty vec (no lines)
        assert_eq!(cost_lines.len(), 0);

        // Format speed lines with zero completed tasks
        let speed_lines = formatter.format_speed_lines();
        // With zero tasks completed, should return empty vec (no lines)
        assert_eq!(speed_lines.len(), 0);

        // Check iteration header formatting (should not contain NaN or Inf)
        let header = formatter.format_iteration_header();
        let header_text = header.join("\n");
        assert!(!header_text.contains("NaN"));
        assert!(!header_text.contains("Inf"));
        assert!(!header_text.contains("inf"));

        // Check stats formatting with zero values (should not contain NaN or Inf)
        let stats = formatter.format_stats(0, false, "");
        let stats_text = stats.join("\n");
        assert!(!stats_text.contains("NaN"));
        assert!(!stats_text.contains("Inf"));
        assert!(!stats_text.contains("inf"));

        // Check interrupted formatting with zero values (should not contain NaN or Inf)
        let interrupted = formatter.format_interrupted(0);
        let interrupted_text = interrupted.join("\n");
        assert!(!interrupted_text.contains("NaN"));
        assert!(!interrupted_text.contains("Inf"));
        assert!(!interrupted_text.contains("inf"));
    }

    #[test]
    fn test_output_formatter_zero_division_protection() {
        let formatter = OutputFormatter::new(true);

        // avg_iteration_secs should return None for empty duration list
        assert_eq!(formatter.avg_iteration_secs(), None);

        // compute_speed_text should return None for zero completed tasks
        assert_eq!(formatter.compute_speed_text(), None);

        // compute_eta_text should return None for zero completed tasks
        assert_eq!(formatter.compute_eta_text(), None);

        // Verify model_costs map is empty
        assert!(formatter.model_costs.is_empty());
    }

    #[test]
    fn test_output_formatter_zero_task_progress() {
        let mut formatter = OutputFormatter::new(false); // use ASCII mode

        // Set task progress with all zeros
        let progress = TaskProgress {
            total: 0,
            done: 0,
            in_progress: 0,
            blocked: 0,
            todo: 0,
            current_task_id: None,
            current_task_name: None,
            current_task_component: None,
        };

        formatter.set_task_progress(Some(progress));
        formatter.set_initial_done_count(0);

        // Format stats with zero task progress
        let stats = formatter.format_stats(0, false, "");
        let stats_text = stats.join("\n");

        // Verify no division by zero errors (no NaN or Inf)
        assert!(!stats_text.contains("NaN"));
        assert!(!stats_text.contains("Inf"));

        // Verify iterations count is displayed correctly
        assert!(stats_text.contains("Iterations") || stats_text.contains("Iteration"));
    }

    #[test]
    fn test_format_tokens_zero_sanity() {
        // Verify format_tokens handles zero correctly
        let zero_tokens = format_tokens(0);
        assert_eq!(zero_tokens, "0");
        assert!(!zero_tokens.contains("NaN"));
        assert!(!zero_tokens.contains("Inf"));
    }

    #[test]
    fn test_format_duration_short_zero_sanity() {
        // Verify format_duration_short handles zero correctly
        let zero_duration = format_duration_short(0);
        assert_eq!(zero_duration, "~0s");
        assert!(!zero_duration.contains("NaN"));
        assert!(!zero_duration.contains("Inf"));
    }

    /// Snapshot test: Format stats with zero tokens and zero cost
    #[test]
    fn snapshot_format_stats_zero_values() {
        let formatter = OutputFormatter::new(false); // ASCII mode for consistent snapshots
        let stats = formatter.format_stats(5, true, "done");

        // Join lines, strip ANSI codes, and normalize elapsed time for deterministic snapshots
        let output = strip_ansi_codes(&stats.join("\n"));
        let output = normalize_elapsed_time(&output);
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Format token lines with 999 tokens (boundary before 'k')
    #[test]
    fn snapshot_format_tokens_999() {
        let mut formatter = OutputFormatter::new(false);
        formatter.finalized_input_tokens = 999;
        formatter.finalized_output_tokens = 999;

        let lines = formatter.format_token_lines();
        let output = strip_ansi_codes(&lines.join("\n"));
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Format token lines with 1000 tokens (exactly 1.0k)
    #[test]
    fn snapshot_format_tokens_1000() {
        let mut formatter = OutputFormatter::new(false);
        formatter.finalized_input_tokens = 1000;
        formatter.finalized_output_tokens = 1000;

        let lines = formatter.format_token_lines();
        let output = strip_ansi_codes(&lines.join("\n"));
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Format token lines with 999999 tokens (boundary before 'M')
    #[test]
    fn snapshot_format_tokens_999999() {
        let mut formatter = OutputFormatter::new(false);
        formatter.finalized_input_tokens = 999_999;
        formatter.finalized_output_tokens = 999_999;

        let lines = formatter.format_token_lines();
        let output = strip_ansi_codes(&lines.join("\n"));
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Format token lines with 1000000 tokens (exactly 1.0M)
    #[test]
    fn snapshot_format_tokens_1000000() {
        let mut formatter = OutputFormatter::new(false);
        formatter.finalized_input_tokens = 1_000_000;
        formatter.finalized_output_tokens = 1_000_000;

        let lines = formatter.format_token_lines();
        let output = strip_ansi_codes(&lines.join("\n"));
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Format cost lines with various amounts
    #[test]
    fn snapshot_format_cost_small() {
        let mut formatter = OutputFormatter::new(false);
        formatter.total_cost_usd = 0.0001;
        formatter
            .model_costs
            .insert("claude-sonnet-4-5".to_string(), 0.0001);

        let lines = formatter.format_cost_lines();
        let output = strip_ansi_codes(&lines.join("\n"));
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Format cost lines with medium amount
    #[test]
    fn snapshot_format_cost_medium() {
        let mut formatter = OutputFormatter::new(false);
        formatter.total_cost_usd = 1.23;
        formatter
            .model_costs
            .insert("claude-sonnet-4-5".to_string(), 0.85);
        formatter
            .model_costs
            .insert("claude-haiku-4-5".to_string(), 0.38);

        let lines = formatter.format_cost_lines();
        let output = strip_ansi_codes(&lines.join("\n"));
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Format cost lines with large amount
    #[test]
    fn snapshot_format_cost_large() {
        let mut formatter = OutputFormatter::new(false);
        formatter.total_cost_usd = 99.99;
        formatter
            .model_costs
            .insert("claude-opus-4-6".to_string(), 99.99);

        let lines = formatter.format_cost_lines();
        let output = strip_ansi_codes(&lines.join("\n"));
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Format cost lines with very small amount (rounding boundary).
    /// Tests edge case: $0.00005 should round to $0.0001 with .4f precision.
    #[test]
    fn snapshot_format_cost_rounding_boundary() {
        let mut formatter = OutputFormatter::new(false);
        formatter.total_cost_usd = 0.00005;
        formatter
            .model_costs
            .insert("claude-haiku-4-5".to_string(), 0.00005);

        let lines = formatter.format_cost_lines();
        let output = strip_ansi_codes(&lines.join("\n"));
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Format cost lines with 3-digit dollar amount.
    /// Tests boundary: Cost display with $100+ (3-digit whole number).
    #[test]
    fn snapshot_format_cost_three_digit() {
        let mut formatter = OutputFormatter::new(false);
        formatter.total_cost_usd = 100.00;
        formatter
            .model_costs
            .insert("claude-opus-4-6".to_string(), 100.00);

        let lines = formatter.format_cost_lines();
        let output = strip_ansi_codes(&lines.join("\n"));
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Format cost lines with very large amount.
    /// Tests extreme boundary: $999.9999 with .4f precision.
    #[test]
    fn snapshot_format_cost_very_large() {
        let mut formatter = OutputFormatter::new(false);
        formatter.total_cost_usd = 999.9999;
        formatter
            .model_costs
            .insert("claude-opus-4-6".to_string(), 999.9999);

        let lines = formatter.format_cost_lines();
        let output = strip_ansi_codes(&lines.join("\n"));
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Format complete stats with mixed token counts
    #[test]
    fn snapshot_format_stats_mixed_tokens() {
        let mut formatter = OutputFormatter::new(false);
        formatter.finalized_input_tokens = 12_345;
        formatter.finalized_output_tokens = 6_789;
        formatter.total_cost_usd = 0.5432;
        formatter
            .model_costs
            .insert("claude-sonnet-4-5".to_string(), 0.5432);

        let stats = formatter.format_stats(3, true, "done");
        let output = strip_ansi_codes(&stats.join("\n"));
        let output = normalize_elapsed_time(&output);
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Duration boundary - iteration header with avg close to 60s (below).
    /// Tests display of iteration average just below the minute boundary (59s).
    /// Average should display as "59s/iter", not switch to minute format yet.
    #[test]
    fn snapshot_iteration_header_duration_59s() {
        let mut formatter = OutputFormatter::new(false);
        formatter.set_iteration(3);
        // Simulate 3 iterations with avg ~59s (58+59+60)/3 = 59
        formatter.iteration_durations = vec![58.0, 59.0, 60.0];

        let header = formatter.format_iteration_header();
        let output = strip_ansi_codes(&header.join("\n"));
        let output = normalize_elapsed_time(&output);
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Duration boundary - iteration header with avg close to 60s (above).
    /// Tests display of iteration average just above the minute boundary (61s).
    /// Average should display as "61s/iter" (integer rounded).
    #[test]
    fn snapshot_iteration_header_duration_61s() {
        let mut formatter = OutputFormatter::new(false);
        formatter.set_iteration(5);
        // Simulate 5 iterations with avg ~61s (60+61+62+60.5+61.5)/5 = 61
        formatter.iteration_durations = vec![60.0, 61.0, 62.0, 60.5, 61.5];

        let header = formatter.format_iteration_header();
        let output = strip_ansi_codes(&header.join("\n"));
        let output = normalize_elapsed_time(&output);
        insta::assert_snapshot!(output);
    }

    /// Snapshot test: Speed lines with completed tasks and duration boundary.
    /// Tests that avg iteration duration is properly formatted when crossing 60s boundary.
    /// Expected display: "Avg iter: 60s" (rounded to nearest second).
    #[test]
    fn snapshot_speed_lines_duration_boundary() {
        let mut formatter = OutputFormatter::new(false);
        formatter.set_initial_done_count(0);

        let progress = TaskProgress {
            total: 5,
            done: 3,
            in_progress: 1,
            blocked: 0,
            todo: 1,
            current_task_id: Some("1.2".to_string()),
            current_task_name: Some("Test task".to_string()),
            current_task_component: Some("tests".to_string()),
        };
        formatter.set_task_progress(Some(progress));

        // Simulate iterations with avg near 60s boundary (59.5+60.2+59.8)/3 = 59.83
        formatter.iteration_durations = vec![59.5, 60.2, 59.8];

        let lines = formatter.format_speed_lines();
        let output = strip_ansi_codes(&lines.join("\n"));
        insta::assert_snapshot!(output);
    }

    /// Normalize elapsed time values in stats output for deterministic snapshots.
    /// Replaces "Time: Xs" and "Elapsed: Xs" with fixed values.
    fn normalize_elapsed_time(s: &str) -> String {
        s.lines()
            .map(|line| {
                if let Some(pos) = line.find("Time:") {
                    let prefix = &line[..pos];
                    return format!("{}Time:      0.00s", prefix);
                }
                if let Some(pos) = line.find("Elapsed:") {
                    let before = &line[..pos];
                    let after = &line[pos + "Elapsed:".len()..];
                    let rest = after.trim_start();
                    // Skip the time value (e.g., "0.0s")
                    let time_end = rest
                        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != 's')
                        .unwrap_or(rest.len());
                    let suffix = &rest[time_end..];
                    return format!("{}Elapsed: 0.0s{}", before, suffix);
                }
                line.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Helper function to strip ANSI color codes for snapshot testing
    fn strip_ansi_codes(s: &str) -> String {
        // Simple regex-free ANSI stripper for tests
        let mut result = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // Skip ESC sequence
                if chars.next() == Some('[') {
                    // Skip until 'm' (end of color code)
                    for ch in chars.by_ref() {
                        if ch == 'm' {
                            break;
                        }
                    }
                }
            } else {
                result.push(c);
            }
        }
        result
    }
}
