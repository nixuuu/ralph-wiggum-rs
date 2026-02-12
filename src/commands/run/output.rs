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
