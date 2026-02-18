use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::HashMap;
use std::time::Instant;

use super::event_formatting::{BlockType, TokenState, format_event};
use super::formatting_helpers::{format_duration_short, format_tokens};
use super::runner::ClaudeEvent;
use super::ui::StatusData;
use crate::shared::icons;
use crate::tui::formatter::RatuiFormatter;
use crate::tui::theme::DEFAULT_THEME;

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

    /// Get total elapsed time in seconds (for summary printing)
    pub fn get_elapsed_secs(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    /// Get total input tokens (for summary printing)
    pub fn get_total_input_tokens(&self) -> u64 {
        self.display_input_tokens()
    }

    /// Get total output tokens (for summary printing)
    pub fn get_total_output_tokens(&self) -> u64 {
        self.display_output_tokens()
    }

    /// Get total cost in USD (for summary printing)
    pub fn get_total_cost(&self) -> f64 {
        self.total_cost_usd
    }

    /// Get speed text (tasks/hour) if available
    pub fn get_speed_text(&self) -> Option<String> {
        self.compute_speed_text()
    }

    /// Get ETA text if available
    pub fn get_eta_text(&self) -> Option<String> {
        self.compute_eta_text()
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
    fn format_token_lines(&self) -> Vec<Line<'static>> {
        let input = self.display_input_tokens();
        let output = self.display_output_tokens();
        if input > 0 || output > 0 {
            vec![Line::from(vec![
                Span::styled("  Tokens:", Style::default().fg(DEFAULT_THEME.muted)),
                Span::raw("    "),
                Span::styled(format_tokens(input), Style::default().fg(Color::Green)),
                Span::styled(" in / ", Style::default().fg(DEFAULT_THEME.muted)),
                Span::styled(format_tokens(output), Style::default().fg(Color::Magenta)),
                Span::styled(" out", Style::default().fg(DEFAULT_THEME.muted)),
            ])]
        } else {
            vec![]
        }
    }

    /// Format speed/throughput lines for stats display
    fn format_speed_lines(&self) -> Vec<Line<'static>> {
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
        let mut lines = vec![Line::from(vec![
            Span::styled("  Speed:", Style::default().fg(DEFAULT_THEME.muted)),
            Span::raw("     "),
            Span::styled(completed.to_string(), Style::default().fg(Color::Green)),
            Span::styled(" tasks | ", Style::default().fg(DEFAULT_THEME.muted)),
            Span::raw(rate),
        ])];
        if let Some(avg) = self.avg_iteration_secs() {
            lines.push(Line::from(vec![
                Span::styled("  Avg iter:", Style::default().fg(DEFAULT_THEME.muted)),
                Span::raw(format!("   {:.0}s", avg)),
            ]));
        }
        lines
    }

    /// Format cost lines with per-model breakdown for stats display
    fn format_cost_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if self.total_cost_usd > 0.0 {
            lines.push(Line::from(vec![
                Span::styled("  Cost:", Style::default().fg(DEFAULT_THEME.muted)),
                Span::raw("      "),
                Span::styled(
                    format!("${:.4}", self.total_cost_usd),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
            if !self.model_costs.is_empty() {
                let mut sorted: Vec<_> = self.model_costs.iter().collect();
                sorted.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
                for (model, cost) in &sorted {
                    lines.push(Line::from(vec![
                        Span::raw("            "),
                        Span::styled(
                            format!("${:.4}", cost),
                            Style::default().fg(DEFAULT_THEME.muted),
                        ),
                        Span::raw(" "),
                        Span::styled(model.to_string(), Style::default().fg(DEFAULT_THEME.muted)),
                    ]));
                }
            }
        }
        lines
    }

    /// Format iteration header and return styled lines
    pub fn format_iteration_header(&self) -> Vec<Line<'static>> {
        let elapsed = self.start_time.elapsed();
        let separator = Line::from(Span::styled(
            "━".repeat(60),
            Style::default().fg(DEFAULT_THEME.muted),
        ));

        let mut header_spans = vec![
            Span::styled("▶", Style::default().fg(Color::Cyan)),
            Span::raw(" "),
            Span::styled("Iteration", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(
                self.iteration.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled("│ Elapsed:", Style::default().fg(DEFAULT_THEME.muted)),
            Span::raw(format!(" {:.1}s", elapsed.as_secs_f64())),
        ];

        if let Some(avg) = self.avg_iteration_secs() {
            header_spans.push(Span::styled(" │", Style::default().fg(DEFAULT_THEME.muted)));
            header_spans.push(Span::raw(format!(" {:.0}s/iter", avg)));
        }

        vec![
            Line::default(),
            separator.clone(),
            Line::from(header_spans),
            separator,
        ]
    }

    /// Format a claude event and return styled ratatui Lines.
    pub fn format_event(&mut self, event: &ClaudeEvent) -> Vec<Line<'static>> {
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

    /// Format final statistics and return styled lines
    pub fn format_stats(
        &self,
        iterations: u32,
        found_promise: bool,
        promise: &str,
    ) -> Vec<Line<'static>> {
        let elapsed = self.start_time.elapsed();
        let separator = Line::from(Span::styled(
            "━".repeat(60),
            Style::default().fg(DEFAULT_THEME.muted),
        ));

        let mut lines = vec![Line::default(), separator.clone()];

        if found_promise {
            lines.push(Line::from(vec![
                Span::styled(
                    icons::status_check(self.use_nerd_font).to_string(),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    "COMPLETED",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("- Promise found: <promise>{}</promise>", promise),
                    Style::default().fg(DEFAULT_THEME.muted),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    icons::status_fail(self.use_nerd_font).to_string(),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    "STOPPED - Promise not found",
                    Style::default().fg(Color::Red),
                ),
            ]));
        }

        lines.push(separator.clone());
        lines.push(Line::from(vec![
            Span::styled("  Iterations:", Style::default().fg(DEFAULT_THEME.muted)),
            Span::raw(" "),
            Span::styled(
                iterations.to_string(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Time:", Style::default().fg(DEFAULT_THEME.muted)),
            Span::raw(format!("      {:.2}s", elapsed.as_secs_f64())),
        ]));

        lines.extend(self.format_speed_lines());
        lines.extend(self.format_token_lines());
        lines.extend(self.format_cost_lines());

        lines.push(separator);
        lines
    }

    /// Format interruption message and return styled lines
    pub fn format_interrupted(&self, iterations: u32) -> Vec<Line<'static>> {
        let elapsed = self.start_time.elapsed();
        let separator = Line::from(Span::styled(
            "━".repeat(60),
            Style::default().fg(DEFAULT_THEME.muted),
        ));

        let mut lines = vec![
            Line::default(),
            separator.clone(),
            Line::from(vec![
                Span::styled(
                    icons::status_pause(self.use_nerd_font).to_string(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    "INTERRUPTED",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled("- State saved", Style::default().fg(DEFAULT_THEME.muted)),
            ]),
            separator.clone(),
            Line::from(vec![
                Span::styled("  Iterations:", Style::default().fg(DEFAULT_THEME.muted)),
                Span::raw(" "),
                Span::styled(
                    iterations.to_string(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("  Time:", Style::default().fg(DEFAULT_THEME.muted)),
                Span::raw(format!("      {:.2}s", elapsed.as_secs_f64())),
            ]),
        ];

        lines.extend(self.format_speed_lines());
        lines.extend(self.format_token_lines());
        lines.extend(self.format_cost_lines());

        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("  Resume:", Style::default().fg(DEFAULT_THEME.muted)),
            Span::raw(" "),
            Span::styled("ralph-wiggum --resume", Style::default().fg(Color::Cyan)),
        ]));
        lines.push(separator);
        lines
    }
}

/// Task progress data for enhanced status bar.
///
/// Legacy: used by StatusTerminal (inline mode). Kept for tests and potential reuse.
#[allow(dead_code)]
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

#[allow(dead_code)]
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

/// RatuiFormatter implementation — deleguje do istniejących metod.
///
/// format_event() → pełna obsługa eventów z token tracking
/// format_iteration_header() → separator + numer iteracji + elapsed
/// format_stats() → bazowe statystyki (tokeny, koszt, speed) — bez parametrów completion
impl RatuiFormatter<ClaudeEvent> for OutputFormatter {
    fn format_event(&mut self, event: &ClaudeEvent) -> Vec<Line<'static>> {
        // Deleguje do istniejącej metody inherent
        OutputFormatter::format_event(self, event)
    }

    fn format_iteration_header(&self) -> Vec<Line<'static>> {
        OutputFormatter::format_iteration_header(self)
    }

    /// Bazowe statystyki sesji (tokeny, koszt, speed).
    /// Dla pełnych statystyk z wynikiem completion użyj `format_stats(iterations, found_promise, promise)`.
    fn format_stats(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.extend(self.format_speed_lines());
        lines.extend(self.format_token_lines());
        lines.extend(self.format_cost_lines());
        lines
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

    /// Helper: konwertuje Vec<Line> na plain text (span content bez stylów)
    fn lines_to_text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
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

    #[test]
    fn test_output_formatter_zero_values() {
        let formatter = OutputFormatter::new(true);

        assert_eq!(formatter.display_input_tokens(), 0);
        assert_eq!(formatter.display_output_tokens(), 0);
        assert_eq!(formatter.total_cost_usd, 0.0);

        // Format token lines with zero values → empty vec
        assert_eq!(formatter.format_token_lines().len(), 0);
        assert_eq!(formatter.format_cost_lines().len(), 0);
        assert_eq!(formatter.format_speed_lines().len(), 0);

        // Check iteration header formatting (should not contain NaN or Inf)
        let header_text = lines_to_text(&formatter.format_iteration_header());
        assert!(!header_text.contains("NaN"));
        assert!(!header_text.contains("Inf"));
        assert!(!header_text.contains("inf"));

        // Check stats formatting with zero values
        let stats_text = lines_to_text(&formatter.format_stats(0, false, ""));
        assert!(!stats_text.contains("NaN"));
        assert!(!stats_text.contains("Inf"));
        assert!(!stats_text.contains("inf"));

        // Check interrupted formatting with zero values
        let interrupted_text = lines_to_text(&formatter.format_interrupted(0));
        assert!(!interrupted_text.contains("NaN"));
        assert!(!interrupted_text.contains("Inf"));
        assert!(!interrupted_text.contains("inf"));
    }

    #[test]
    fn test_output_formatter_zero_division_protection() {
        let formatter = OutputFormatter::new(true);

        assert_eq!(formatter.avg_iteration_secs(), None);
        assert_eq!(formatter.compute_speed_text(), None);
        assert_eq!(formatter.compute_eta_text(), None);
        assert!(formatter.model_costs.is_empty());
    }

    #[test]
    fn test_output_formatter_zero_task_progress() {
        let mut formatter = OutputFormatter::new(false);

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

        let stats_text = lines_to_text(&formatter.format_stats(0, false, ""));
        assert!(!stats_text.contains("NaN"));
        assert!(!stats_text.contains("Inf"));
        assert!(stats_text.contains("Iterations") || stats_text.contains("Iteration"));
    }

    #[test]
    fn test_format_tokens_zero_sanity() {
        let zero_tokens = format_tokens(0);
        assert_eq!(zero_tokens, "0");
    }

    #[test]
    fn test_format_duration_short_zero_sanity() {
        let zero_duration = format_duration_short(0);
        assert_eq!(zero_duration, "~0s");
    }

    /// Test: RatuiFormatter trait implementation works correctly
    #[test]
    fn test_ratatui_formatter_trait_impl() {
        let mut formatter = OutputFormatter::new(false);
        formatter.finalized_input_tokens = 1000;
        formatter.finalized_output_tokens = 500;
        formatter.total_cost_usd = 0.05;

        // format_iteration_header via trait
        let header: Vec<Line<'static>> =
            RatuiFormatter::<ClaudeEvent>::format_iteration_header(&formatter);
        assert_eq!(header.len(), 4); // empty line, separator, header, separator

        // format_stats via trait (bazowe statystyki)
        let stats: Vec<Line<'static>> = RatuiFormatter::<ClaudeEvent>::format_stats(&formatter);
        // Powinny być token lines + cost lines (speed lines = 0 bo brak completed tasks)
        assert!(!stats.is_empty());
        let stats_text = lines_to_text(&stats);
        assert!(stats_text.contains("Tokens:"));
        assert!(stats_text.contains("Cost:"));
    }

    // -- Snapshot tests --

    #[test]
    fn snapshot_format_stats_zero_values() {
        let formatter = OutputFormatter::new(false);
        let stats = formatter.format_stats(5, true, "done");
        let output = normalize_elapsed_time(&lines_to_text(&stats));
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_format_tokens_999() {
        let mut formatter = OutputFormatter::new(false);
        formatter.finalized_input_tokens = 999;
        formatter.finalized_output_tokens = 999;
        let output = lines_to_text(&formatter.format_token_lines());
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_format_tokens_1000() {
        let mut formatter = OutputFormatter::new(false);
        formatter.finalized_input_tokens = 1000;
        formatter.finalized_output_tokens = 1000;
        let output = lines_to_text(&formatter.format_token_lines());
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_format_tokens_999999() {
        let mut formatter = OutputFormatter::new(false);
        formatter.finalized_input_tokens = 999_999;
        formatter.finalized_output_tokens = 999_999;
        let output = lines_to_text(&formatter.format_token_lines());
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_format_tokens_1000000() {
        let mut formatter = OutputFormatter::new(false);
        formatter.finalized_input_tokens = 1_000_000;
        formatter.finalized_output_tokens = 1_000_000;
        let output = lines_to_text(&formatter.format_token_lines());
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_format_cost_small() {
        let mut formatter = OutputFormatter::new(false);
        formatter.total_cost_usd = 0.0001;
        formatter
            .model_costs
            .insert("claude-sonnet-4-5".to_string(), 0.0001);
        let output = lines_to_text(&formatter.format_cost_lines());
        insta::assert_snapshot!(output);
    }

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
        let output = lines_to_text(&formatter.format_cost_lines());
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_format_cost_large() {
        let mut formatter = OutputFormatter::new(false);
        formatter.total_cost_usd = 99.99;
        formatter
            .model_costs
            .insert("claude-opus-4-6".to_string(), 99.99);
        let output = lines_to_text(&formatter.format_cost_lines());
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_format_cost_rounding_boundary() {
        let mut formatter = OutputFormatter::new(false);
        formatter.total_cost_usd = 0.00005;
        formatter
            .model_costs
            .insert("claude-haiku-4-5".to_string(), 0.00005);
        let output = lines_to_text(&formatter.format_cost_lines());
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_format_cost_three_digit() {
        let mut formatter = OutputFormatter::new(false);
        formatter.total_cost_usd = 100.00;
        formatter
            .model_costs
            .insert("claude-opus-4-6".to_string(), 100.00);
        let output = lines_to_text(&formatter.format_cost_lines());
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_format_cost_very_large() {
        let mut formatter = OutputFormatter::new(false);
        formatter.total_cost_usd = 999.9999;
        formatter
            .model_costs
            .insert("claude-opus-4-6".to_string(), 999.9999);
        let output = lines_to_text(&formatter.format_cost_lines());
        insta::assert_snapshot!(output);
    }

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
        let output = normalize_elapsed_time(&lines_to_text(&stats));
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_iteration_header_duration_59s() {
        let mut formatter = OutputFormatter::new(false);
        formatter.set_iteration(3);
        formatter.iteration_durations = vec![58.0, 59.0, 60.0];
        let header = formatter.format_iteration_header();
        let output = normalize_elapsed_time(&lines_to_text(&header));
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_iteration_header_duration_61s() {
        let mut formatter = OutputFormatter::new(false);
        formatter.set_iteration(5);
        formatter.iteration_durations = vec![60.0, 61.0, 62.0, 60.5, 61.5];
        let header = formatter.format_iteration_header();
        let output = normalize_elapsed_time(&lines_to_text(&header));
        insta::assert_snapshot!(output);
    }

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
        formatter.iteration_durations = vec![59.5, 60.2, 59.8];

        let output = lines_to_text(&formatter.format_speed_lines());
        insta::assert_snapshot!(output);
    }
}
