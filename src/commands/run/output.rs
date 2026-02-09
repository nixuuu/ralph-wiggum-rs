use crossterm::style::Stylize;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

use super::runner::{ClaudeEvent, ContentBlock, ModelUsageEntry, Usage};
use super::ui::StatusData;
use crate::shared::icons;
use crate::shared::markdown;

/// Cached current working directory for path shortening
static CWD: LazyLock<String> = LazyLock::new(|| {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
});

/// Cached home directory for path shortening
static HOME_DIR: LazyLock<String> = LazyLock::new(|| std::env::var("HOME").unwrap_or_default());

/// Shorten absolute paths for display: CWD → relative, HOME → ~
fn shorten_path(s: &str) -> String {
    let cwd = &*CWD;
    if !cwd.is_empty() {
        let with_slash = format!("{}/", cwd);
        if s.contains(with_slash.as_str()) {
            return s.replace(with_slash.as_str(), "");
        }
        if s.contains(cwd.as_str()) {
            return s.replace(cwd.as_str(), ".");
        }
    }
    let home = &*HOME_DIR;
    if !home.is_empty() && s.contains(home.as_str()) {
        return s.replace(home.as_str(), "~");
    }
    s.to_string()
}

/// Maximum width for tool detail strings (paths, descriptions, etc.)
const MAX_DETAIL_WIDTH: usize = 100;

/// Type of the last content block for grouping output
#[derive(PartialEq, Clone, Copy)]
enum BlockType {
    None,
    Text,
    Tool,
}

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

    /// Add incremental tokens from assistant message (pending, for live display)
    fn add_pending_usage(&mut self, usage: &Usage) {
        self.pending_input_tokens += usage.input_tokens;
        self.pending_output_tokens += usage.output_tokens;
    }

    /// Finalize an iteration's usage from modelUsage (replaces pending tokens)
    fn finalize_model_usage(&mut self, model_usage: &HashMap<String, ModelUsageEntry>) {
        for (model_name, entry) in model_usage {
            self.finalized_input_tokens += entry.input_tokens;
            self.finalized_output_tokens += entry.output_tokens;
            self.total_cost_usd += entry.cost_usd;
            *self.model_costs.entry(model_name.clone()).or_insert(0.0) += entry.cost_usd;
        }
        self.pending_input_tokens = 0;
        self.pending_output_tokens = 0;
    }

    /// Fallback finalization when modelUsage is absent (backwards compat)
    fn finalize_legacy(&mut self, cost: Option<f64>) {
        if let Some(c) = cost {
            self.total_cost_usd += c;
        }
        // Promote pending tokens to finalized (don't add result.usage — it would double-count)
        self.finalized_input_tokens += self.pending_input_tokens;
        self.finalized_output_tokens += self.pending_output_tokens;
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
        Some(
            self.iteration_durations.iter().sum::<f64>()
                / self.iteration_durations.len() as f64,
        )
    }

    /// Get current status data for the status bar
    pub fn get_status(&self) -> StatusData {
        StatusData {
            iteration: self.iteration,
            min_iterations: self.min_iterations,
            max_iterations: self.max_iterations,
            elapsed_secs: self.start_time.elapsed().as_secs_f64(),
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

    #[allow(dead_code)]
    pub fn total_input_tokens(&self) -> u64 {
        self.display_input_tokens()
    }

    #[allow(dead_code)]
    pub fn total_output_tokens(&self) -> u64 {
        self.display_output_tokens()
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
            lines.push(format!(
                "  {}   {:.0}s",
                "Avg iter:".dark_grey(),
                avg
            ));
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
        let mut lines = Vec::new();
        match event {
            ClaudeEvent::Assistant { message } => {
                // Add to pending tokens for live display during iteration
                if let Some(u) = &message.usage {
                    self.add_pending_usage(u);
                }

                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } => {
                            // Add empty line if switching from tool to text
                            if self.last_block_type == BlockType::Tool {
                                lines.push(String::new());
                            }
                            self.last_block_type = BlockType::Text;

                            // Shorten paths, style thinking blocks, render markdown
                            let text = shorten_path(text);
                            let text = process_thinking_blocks(&text);
                            let rendered = markdown::render_markdown(&text);
                            for line in rendered.lines() {
                                lines.push(line.to_string());
                            }
                        }
                        ContentBlock::ToolUse { name, input, .. } => {
                            // Add empty line if switching from text to tool
                            if self.last_block_type == BlockType::Text {
                                lines.push(String::new());
                            }
                            self.last_block_type = BlockType::Tool;

                            let details = format_tool_details(name, input);
                            let tool_icon = icons::tool_icon(name, self.use_nerd_font);
                            let colored_name = colorize_tool_name(name);
                            if details.is_empty() {
                                lines.push(format!("  {} {}", tool_icon, colored_name));
                            } else {
                                lines.push(format!(
                                    "  {} {} {}",
                                    tool_icon,
                                    colored_name,
                                    details.dark_grey()
                                ));
                            }
                        }
                        ContentBlock::ToolResult { .. } => {
                            // Don't print tool results - too verbose
                        }
                        ContentBlock::Other => {}
                    }
                }
            }
            ClaudeEvent::Result {
                cost_usd,
                model_usage,
                ..
            } => {
                if let Some(mu) = model_usage {
                    self.finalize_model_usage(mu);
                } else {
                    self.finalize_legacy(*cost_usd);
                }
            }
            _ => {}
        }
        lines
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

/// Format seconds into a short human-readable duration string.
/// Examples: "~45s", "~3m", "~12m", "~1h05m", "~2h30m"
fn format_duration_short(total_secs: u64) -> String {
    if total_secs < 60 {
        format!("~{}s", total_secs)
    } else if total_secs < 3600 {
        let mins = total_secs / 60;
        format!("~{}m", mins)
    } else {
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        if mins == 0 {
            format!("~{}h", hours)
        } else {
            format!("~{}h{:02}m", hours, mins)
        }
    }
}

/// Format tokens for display (e.g., 1234 -> "1.2k")
fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

/// Colorize tool name based on tool type
fn colorize_tool_name(name: &str) -> String {
    use crossterm::style::Stylize;
    match name {
        "Read" | "Glob" | "Grep" => name.cyan().to_string(),
        "Write" | "Edit" => name.yellow().to_string(),
        "Bash" => name.magenta().to_string(),
        "Task" => name.blue().to_string(),
        "WebFetch" | "WebSearch" => name.green().to_string(),
        "TodoWrite" => name.white().to_string(),
        _ => name.white().to_string(),
    }
}

/// Format tool details for display
/// Format Edit tool as colored diff
fn format_edit_diff(path: &str, old: &str, new: &str) -> String {
    use crossterm::style::Stylize;

    let truncated_path = truncate_string(&shorten_path(path), MAX_DETAIL_WIDTH);
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // If diff is small (≤5 lines total), show inline diff
    if old_lines.len() + new_lines.len() <= 5 {
        let mut parts = vec![truncated_path];
        for line in &old_lines {
            parts.push(format!("\n    {} {}", "-".red(), truncate_string(line, 60)));
        }
        for line in &new_lines {
            parts.push(format!(
                "\n    {} {}",
                "+".green(),
                truncate_string(line, 60)
            ));
        }
        parts.join("")
    } else {
        // For larger diffs, show summary
        format!(
            "{} | {} {}",
            truncated_path,
            format!("-{}", old_lines.len()).red(),
            format!("+{}", new_lines.len()).green()
        )
    }
}

fn format_tool_details(name: &str, input: &serde_json::Value) -> String {
    match name {
        "Read" => {
            if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                return truncate_string(&shorten_path(path), MAX_DETAIL_WIDTH);
            }
        }
        "Write" => {
            if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                return truncate_string(&shorten_path(path), MAX_DETAIL_WIDTH);
            }
        }
        "Edit" => {
            if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                let old = input
                    .get("old_string")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let new = input
                    .get("new_string")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                return format_edit_diff(path, old, new);
            }
        }
        "Bash" => {
            let desc = input.get("description").and_then(|v| v.as_str());
            let cmd = input.get("command").and_then(|v| v.as_str());
            match (desc, cmd) {
                (Some(d), Some(c)) => {
                    return truncate_string(
                        &shorten_path(&format!("{}: {}", d, c)),
                        MAX_DETAIL_WIDTH,
                    );
                }
                (Some(d), None) => {
                    return truncate_string(&shorten_path(d), MAX_DETAIL_WIDTH);
                }
                (None, Some(c)) => {
                    return truncate_string(&shorten_path(c), MAX_DETAIL_WIDTH);
                }
                _ => {}
            }
        }
        "Glob" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            return truncate_string(
                &format!("{} in {}", pattern, shorten_path(path)),
                MAX_DETAIL_WIDTH,
            );
        }
        "Grep" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            return truncate_string(
                &format!(
                    "\"{}\" in {}",
                    truncate_string(pattern, 30),
                    shorten_path(path)
                ),
                MAX_DETAIL_WIDTH,
            );
        }
        "Task" => {
            if let Some(desc) = input.get("description").and_then(|v| v.as_str()) {
                let agent = input
                    .get("subagent_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("agent");
                return truncate_string(&format!("[{}] {}", agent, desc), MAX_DETAIL_WIDTH);
            }
        }
        "WebFetch" => {
            let desc = input.get("prompt").and_then(|v| v.as_str());
            let url = input.get("url").and_then(|v| v.as_str());
            match (desc, url) {
                (Some(d), Some(u)) => {
                    return truncate_string(
                        &format!("{} ({})", d, truncate_string(u, 40)),
                        MAX_DETAIL_WIDTH,
                    );
                }
                (None, Some(u)) => return truncate_string(u, MAX_DETAIL_WIDTH),
                (Some(d), None) => return truncate_string(d, MAX_DETAIL_WIDTH),
                _ => {}
            }
        }
        "WebSearch" => {
            if let Some(query) = input.get("query").and_then(|v| v.as_str()) {
                return truncate_string(&format!("\"{}\"", query), MAX_DETAIL_WIDTH);
            }
        }
        "TodoWrite" => {
            if let Some(todos) = input.get("todos").and_then(|v| v.as_array()) {
                let in_progress: Vec<_> = todos
                    .iter()
                    .filter(|t| t.get("status").and_then(|s| s.as_str()) == Some("in_progress"))
                    .filter_map(|t| t.get("content").and_then(|c| c.as_str()))
                    .collect();
                if !in_progress.is_empty() {
                    return format!("{} task(s) in progress", in_progress.len());
                }
                return format!("{} task(s)", todos.len());
            }
        }
        _ => {
            // Fallback: check for common description field
            if let Some(desc) = input.get("description").and_then(|v| v.as_str()) {
                return truncate_string(desc, MAX_DETAIL_WIDTH);
            }
        }
    }
    String::new()
}

/// Truncate string and add ellipsis if too long
/// Uses character count instead of byte count to properly handle Unicode (including emoji)
fn truncate_string(s: &str, max_len: usize) -> String {
    let s = s.replace('\n', "\\n").replace('\r', "");
    let char_count = s.chars().count();
    if char_count <= max_len {
        s
    } else {
        // Collect first max_len characters to avoid byte boundary issues
        let truncated: String = s.chars().take(max_len).collect();
        format!("{}...", truncated)
    }
}

/// Convert `<thinking>...</thinking>` blocks to markdown blockquotes with italic.
///
/// Text outside thinking tags is passed through unchanged.
/// Example: `<thinking>\nfoo\nbar\n</thinking>` → `> *foo*\n> *bar*`
fn process_thinking_blocks(text: &str) -> String {
    let open_tag = "<thinking>";
    let close_tag = "</thinking>";

    let mut result = String::with_capacity(text.len());
    let mut search_from = 0;

    while let Some(start) = text[search_from..].find(open_tag) {
        let abs_start = search_from + start;
        // Append text before the tag
        result.push_str(&text[search_from..abs_start]);

        let content_start = abs_start + open_tag.len();
        if let Some(end) = text[content_start..].find(close_tag) {
            let inner = text[content_start..content_start + end].trim_matches('\n');
            // Convert each line to blockquote with italic
            for line in inner.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    result.push_str("> \n");
                } else {
                    result.push_str(&format!("> *{}*\n", trimmed));
                }
            }
            search_from = content_start + end + close_tag.len();
        } else {
            // No closing tag — output remainder as-is
            result.push_str(&text[abs_start..]);
            return result;
        }
    }

    // Append remaining text after last tag
    result.push_str(&text[search_from..]);
    result
}

/// Check if text contains completion promise in <promise>...</promise> tags
pub fn find_promise(text: &str, promise: &str) -> bool {
    let open_tag = "<promise>";
    let close_tag = "</promise>";

    let mut search_from = 0;
    while let Some(start) = text[search_from..].find(open_tag) {
        let content_start = search_from + start + open_tag.len();
        if let Some(end) = text[content_start..].find(close_tag) {
            let inner = &text[content_start..content_start + end];
            if inner.trim() == promise {
                return true;
            }
            search_from = content_start + end + close_tag.len();
        } else {
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_promise_exact() {
        assert!(find_promise("<promise>done</promise>", "done"));
        assert!(find_promise(
            "Some text <promise>done</promise> more text",
            "done"
        ));
    }

    #[test]
    fn test_find_promise_with_whitespace() {
        assert!(find_promise("<promise> done </promise>", "done"));
        assert!(find_promise("<promise>\ndone\n</promise>", "done"));
    }

    #[test]
    fn test_find_promise_custom() {
        assert!(find_promise(
            "<promise>task completed</promise>",
            "task completed"
        ));
    }

    #[test]
    fn test_find_promise_not_found() {
        assert!(!find_promise("no promise here", "done"));
        assert!(!find_promise("<promise>wrong</promise>", "done"));
    }

    #[test]
    fn test_find_promise_special_chars() {
        assert!(find_promise("<promise>done!</promise>", "done!"));
        assert!(find_promise(
            "<promise>task (done)</promise>",
            "task (done)"
        ));
    }

    #[test]
    fn test_truncate_string_ascii() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world", 5), "hello...");
        assert_eq!(truncate_string("abc", 3), "abc");
    }

    #[test]
    fn test_truncate_string_with_emoji() {
        // Emoji like 🔍 takes 4 bytes but is 1 character
        let s = "| 🔍 Do dodania |";
        // Should not panic - this was the original bug
        let result = truncate_string(s, 10);
        assert_eq!(result.chars().count(), 13); // 10 chars + "..."

        // Test with multiple emoji
        let emoji_str = "✅ ⚠️ 🔍 test";
        let truncated = truncate_string(emoji_str, 5);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_truncate_string_unicode_boundary() {
        // This is the exact case from the bug report
        let s = "| popover, calendar, radio-group, sheet | ✅ | ⚠️ | 🔍 Do dodania |";
        // Should not panic even with small max_len
        let result = truncate_string(s, 60);
        assert!(!result.is_empty());

        // Test truncation at various points
        for max_len in 1..=s.chars().count() {
            let _ = truncate_string(s, max_len); // Should not panic
        }
    }

    #[test]
    fn test_truncate_string_newlines() {
        assert_eq!(truncate_string("hello\nworld", 20), "hello\\nworld");
        assert_eq!(truncate_string("a\rb\nc", 10), "ab\\nc");
    }

    #[test]
    fn test_process_thinking_no_tags() {
        assert_eq!(process_thinking_blocks("hello world"), "hello world");
    }

    #[test]
    fn test_process_thinking_simple() {
        let input = "<thinking>\nfoo\nbar\n</thinking>";
        let result = process_thinking_blocks(input);
        assert_eq!(result, "> *foo*\n> *bar*\n");
    }

    #[test]
    fn test_process_thinking_with_surrounding_text() {
        let input = "Before\n<thinking>\nthought\n</thinking>\nAfter";
        let result = process_thinking_blocks(input);
        assert_eq!(result, "Before\n> *thought*\n\nAfter");
    }

    #[test]
    fn test_process_thinking_empty_lines() {
        let input = "<thinking>\nfoo\n\nbar\n</thinking>";
        let result = process_thinking_blocks(input);
        assert_eq!(result, "> *foo*\n> \n> *bar*\n");
    }

    #[test]
    fn test_process_thinking_no_closing_tag() {
        let input = "text <thinking>unclosed";
        let result = process_thinking_blocks(input);
        assert_eq!(result, "text <thinking>unclosed");
    }

    #[test]
    fn test_format_duration_short_seconds() {
        assert_eq!(format_duration_short(0), "~0s");
        assert_eq!(format_duration_short(45), "~45s");
        assert_eq!(format_duration_short(59), "~59s");
    }

    #[test]
    fn test_format_duration_short_minutes() {
        assert_eq!(format_duration_short(60), "~1m");
        assert_eq!(format_duration_short(90), "~1m");
        assert_eq!(format_duration_short(720), "~12m");
        assert_eq!(format_duration_short(3599), "~59m");
    }

    #[test]
    fn test_format_duration_short_hours() {
        assert_eq!(format_duration_short(3600), "~1h");
        assert_eq!(format_duration_short(3900), "~1h05m");
        assert_eq!(format_duration_short(9000), "~2h30m");
        assert_eq!(format_duration_short(7200), "~2h");
    }
}
