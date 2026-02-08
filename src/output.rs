use crossterm::style::Stylize;
use std::time::Instant;

use crate::claude::{ClaudeEvent, ContentBlock, Usage};
use crate::icons;
use crate::markdown;
use crate::ui::StatusData;

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
    max_iterations: u32,
    start_time: Instant,
    iteration_start_time: Instant,
    total_cost_usd: f64,
    total_input_tokens: u64,
    total_output_tokens: u64,
    last_block_type: BlockType,
    use_nerd_font: bool,
}

impl OutputFormatter {
    pub fn new(use_nerd_font: bool) -> Self {
        let now = Instant::now();
        Self {
            iteration: 0,
            max_iterations: 0,
            start_time: now,
            iteration_start_time: now,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            last_block_type: BlockType::None,
            use_nerd_font,
        }
    }

    pub fn set_iteration(&mut self, iteration: u32) {
        self.iteration = iteration;
    }

    pub fn set_max_iterations(&mut self, max: u32) {
        self.max_iterations = max;
    }

    /// Start a new iteration - reset iteration timer and block type
    pub fn start_iteration(&mut self) {
        self.iteration_start_time = Instant::now();
        self.last_block_type = BlockType::None;
    }

    pub fn add_cost(&mut self, cost: f64) {
        self.total_cost_usd += cost;
    }

    pub fn add_usage(&mut self, usage: &Usage) {
        self.total_input_tokens += usage.input_tokens;
        self.total_output_tokens += usage.output_tokens;
    }

    /// Get current status data for the status bar
    pub fn get_status(&self) -> StatusData {
        StatusData {
            iteration: self.iteration,
            max_iterations: self.max_iterations,
            elapsed_secs: self.start_time.elapsed().as_secs_f64(),
            iteration_elapsed_secs: self.iteration_start_time.elapsed().as_secs_f64(),
            input_tokens: self.total_input_tokens,
            output_tokens: self.total_output_tokens,
            cost_usd: self.total_cost_usd,
            update_info: None,
        }
    }

    #[allow(dead_code)]
    pub fn total_input_tokens(&self) -> u64 {
        self.total_input_tokens
    }

    #[allow(dead_code)]
    pub fn total_output_tokens(&self) -> u64 {
        self.total_output_tokens
    }

    /// Format iteration header and return lines
    pub fn format_iteration_header(&self) -> Vec<String> {
        let elapsed = self.start_time.elapsed();
        vec![
            String::new(),
            format!("{}", "━".repeat(60).dark_grey()),
            format!(
                "{} {} {} {} {:.1}s",
                "▶".cyan(),
                "Iteration".bold(),
                self.iteration.to_string().cyan().bold(),
                "│ Elapsed:".dark_grey(),
                elapsed.as_secs_f64()
            ),
            format!("{}", "━".repeat(60).dark_grey()),
        ]
    }

    /// Format a claude event and return lines to print
    pub fn format_event(&mut self, event: &ClaudeEvent) -> Vec<String> {
        let mut lines = Vec::new();
        match event {
            ClaudeEvent::Assistant { message } => {
                // Extract usage from assistant message
                if let Some(u) = &message.usage {
                    self.add_usage(u);
                }

                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } => {
                            // Add empty line if switching from tool to text
                            if self.last_block_type == BlockType::Tool {
                                lines.push(String::new());
                            }
                            self.last_block_type = BlockType::Text;

                            // Render markdown formatted text
                            let rendered = markdown::render_markdown(text);
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
                cost_usd, usage, ..
            } => {
                if let Some(cost) = cost_usd {
                    self.add_cost(*cost);
                }
                if let Some(u) = usage {
                    self.add_usage(u);
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

        if self.total_input_tokens > 0 || self.total_output_tokens > 0 {
            lines.push(format!(
                "  {}    {} {} {} {}",
                "Tokens:".dark_grey(),
                format_tokens(self.total_input_tokens).green(),
                "in /".dark_grey(),
                format_tokens(self.total_output_tokens).magenta(),
                "out".dark_grey()
            ));
        }

        if self.total_cost_usd > 0.0 {
            lines.push(format!(
                "  {}      {}",
                "Cost:".dark_grey(),
                format!("${:.4}", self.total_cost_usd).yellow()
            ));
        }

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

        if self.total_input_tokens > 0 || self.total_output_tokens > 0 {
            lines.push(format!(
                "  {}    {} {} {} {}",
                "Tokens:".dark_grey(),
                format_tokens(self.total_input_tokens).green(),
                "in /".dark_grey(),
                format_tokens(self.total_output_tokens).magenta(),
                "out".dark_grey()
            ));
        }

        if self.total_cost_usd > 0.0 {
            lines.push(format!(
                "  {}      {}",
                "Cost:".dark_grey(),
                format!("${:.4}", self.total_cost_usd).yellow()
            ));
        }

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

impl Default for OutputFormatter {
    fn default() -> Self {
        Self::new(true)
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

    let truncated_path = truncate_string(path, MAX_DETAIL_WIDTH);
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
                return truncate_string(path, MAX_DETAIL_WIDTH);
            }
        }
        "Write" => {
            if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                return truncate_string(path, MAX_DETAIL_WIDTH);
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
                (Some(d), _) => return truncate_string(d, MAX_DETAIL_WIDTH),
                (None, Some(c)) => return truncate_string(c, MAX_DETAIL_WIDTH),
                _ => {}
            }
        }
        "Glob" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            return truncate_string(&format!("{} in {}", pattern, path), MAX_DETAIL_WIDTH);
        }
        "Grep" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            return truncate_string(
                &format!("\"{}\" in {}", truncate_string(pattern, 30), path),
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
        assert!(result.len() > 0);

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
}
