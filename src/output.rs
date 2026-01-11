use regex::Regex;
use std::time::Instant;

use crate::claude::{ClaudeEvent, ContentBlock};

pub struct OutputFormatter {
    iteration: u32,
    start_time: Instant,
    total_cost_usd: f64,
}

impl OutputFormatter {
    pub fn new() -> Self {
        Self {
            iteration: 0,
            start_time: Instant::now(),
            total_cost_usd: 0.0,
        }
    }

    pub fn set_iteration(&mut self, iteration: u32) {
        self.iteration = iteration;
    }

    pub fn add_cost(&mut self, cost: f64) {
        self.total_cost_usd += cost;
    }

    /// Print iteration header
    pub fn print_iteration_header(&self) {
        let elapsed = self.start_time.elapsed();
        println!();
        println!("{}", "=".repeat(60));
        println!(
            "Iteration {} | Elapsed: {:.1}s",
            self.iteration,
            elapsed.as_secs_f64()
        );
        println!("{}", "=".repeat(60));
        println!();
    }

    /// Format and print a claude event
    pub fn print_event(&mut self, event: &ClaudeEvent) {
        match event {
            ClaudeEvent::Assistant { message } => {
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } => {
                            println!("{}", text);
                        }
                        ContentBlock::ToolUse { name, input, .. } => {
                            let details = format_tool_details(name, input);
                            if details.is_empty() {
                                println!("[Tool: {}]", name);
                            } else {
                                println!("[Tool: {}] {}", name, details);
                            }
                        }
                        ContentBlock::ToolResult { .. } => {
                            // Don't print tool results - too verbose
                        }
                        ContentBlock::Other => {}
                    }
                }
            }
            ClaudeEvent::Result { cost_usd, .. } => {
                if let Some(cost) = cost_usd {
                    self.add_cost(*cost);
                }
            }
            _ => {}
        }
    }

    /// Print final statistics
    pub fn print_stats(&self, iterations: u32, found_promise: bool, promise: &str) {
        let elapsed = self.start_time.elapsed();
        println!();
        println!("{}", "=".repeat(60));
        if found_promise {
            println!("COMPLETED - Promise found: <promise>{}</promise>", promise);
        } else {
            println!("STOPPED - Promise not found");
        }
        println!("{}", "=".repeat(60));
        println!("Total iterations: {}", iterations);
        println!("Total time: {:.2}s", elapsed.as_secs_f64());
        if self.total_cost_usd > 0.0 {
            println!("Total cost: ${:.4}", self.total_cost_usd);
        }
        println!("{}", "=".repeat(60));
    }

    /// Print interruption message
    pub fn print_interrupted(&self, iterations: u32) {
        let elapsed = self.start_time.elapsed();
        println!();
        println!("{}", "=".repeat(60));
        println!("INTERRUPTED - State saved");
        println!("{}", "=".repeat(60));
        println!("Iterations completed: {}", iterations);
        println!("Time elapsed: {:.2}s", elapsed.as_secs_f64());
        if self.total_cost_usd > 0.0 {
            println!("Cost so far: ${:.4}", self.total_cost_usd);
        }
        println!("Resume with: ralph-wiggum --resume");
        println!("{}", "=".repeat(60));
    }
}

impl Default for OutputFormatter {
    fn default() -> Self {
        Self::new()
    }
}

/// Format tool details for display
fn format_tool_details(name: &str, input: &serde_json::Value) -> String {
    match name {
        "Read" => {
            if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                return path.to_string();
            }
        }
        "Write" => {
            if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                return path.to_string();
            }
        }
        "Edit" => {
            if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                let old = input
                    .get("old_string")
                    .and_then(|v| v.as_str())
                    .map(|s| truncate_string(s, 30))
                    .unwrap_or_default();
                let new = input
                    .get("new_string")
                    .and_then(|v| v.as_str())
                    .map(|s| truncate_string(s, 30))
                    .unwrap_or_default();
                return format!("{} | \"{}\" -> \"{}\"", path, old, new);
            }
        }
        "Bash" => {
            let desc = input.get("description").and_then(|v| v.as_str());
            let cmd = input.get("command").and_then(|v| v.as_str());
            match (desc, cmd) {
                (Some(d), _) => return d.to_string(),
                (None, Some(c)) => return truncate_string(c, 60),
                _ => {}
            }
        }
        "Glob" => {
            let pattern = input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            return format!("{} in {}", pattern, path);
        }
        "Grep" => {
            let pattern = input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            return format!("\"{}\" in {}", truncate_string(pattern, 30), path);
        }
        "Task" => {
            if let Some(desc) = input.get("description").and_then(|v| v.as_str()) {
                let agent = input
                    .get("subagent_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("agent");
                return format!("[{}] {}", agent, desc);
            }
        }
        "WebFetch" => {
            let desc = input.get("prompt").and_then(|v| v.as_str());
            let url = input.get("url").and_then(|v| v.as_str());
            match (desc, url) {
                (Some(d), Some(u)) => return format!("{} ({})", d, truncate_string(u, 40)),
                (None, Some(u)) => return truncate_string(u, 60),
                (Some(d), None) => return d.to_string(),
                _ => {}
            }
        }
        "WebSearch" => {
            if let Some(query) = input.get("query").and_then(|v| v.as_str()) {
                return format!("\"{}\"", query);
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
                return desc.to_string();
            }
        }
    }
    String::new()
}

/// Truncate string and add ellipsis if too long
fn truncate_string(s: &str, max_len: usize) -> String {
    let s = s.replace('\n', "\\n").replace('\r', "");
    if s.len() <= max_len {
        s
    } else {
        format!("{}...", &s[..max_len])
    }
}

/// Check if text contains completion promise in <promise>...</promise> tags
pub fn find_promise(text: &str, promise: &str) -> bool {
    // Escape special regex characters in promise
    let escaped_promise = regex::escape(promise);
    // Allow whitespace around promise text
    let pattern = format!(r"<promise>\s*{}\s*</promise>", escaped_promise);

    match Regex::new(&pattern) {
        Ok(re) => re.is_match(text),
        Err(_) => {
            // Fallback to simple string matching
            text.contains(&format!("<promise>{}</promise>", promise))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_promise_exact() {
        assert!(find_promise("<promise>done</promise>", "done"));
        assert!(find_promise("Some text <promise>done</promise> more text", "done"));
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
        assert!(find_promise("<promise>task (done)</promise>", "task (done)"));
    }
}
