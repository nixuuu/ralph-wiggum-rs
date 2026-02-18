//! Task command runner - shared wrapper around run_once for task subcommands.
//!
//! Centralizes common setup for task add/edit/plan etc:
//! - Blocks dangerous tools (Write, Edit, Bash, etc.)
//! - Blocks AskUserQuestion to enforce MCP ask_user flow
//! - Auto-starts MCP server with tasks_path

use std::path::PathBuf;

use crate::commands::run::{DANGEROUS_TOOLS, RunOnceOptions, run_once};
use crate::shared::error::Result;

/// Options for running a task command via Claude CLI.
pub struct TaskRunOptions {
    /// Prompt to send to Claude
    pub prompt: String,
    /// Short command name for TUI header (e.g. "task plan")
    pub command_name: String,
    /// Optional model override
    pub model: Option<String>,
    /// Use nerd font icons
    pub use_nerd_font: bool,
    /// Path to tasks.yml for MCP server
    pub tasks_path: PathBuf,
}

/// Run a task command with streaming TUI output.
///
/// Uses run_once() internally with task-safe settings:
/// - Blocks dangerous tools (Write, Edit, Bash, NotebookEdit, TodoWrite)
/// - Blocks AskUserQuestion to enforce MCP ask_user flow
/// - Auto-starts MCP server with tasks_path for task mutations
pub async fn run_task_command(options: TaskRunOptions) -> Result<()> {
    run_once(RunOnceOptions {
        prompt: options.prompt,
        model: options.model,
        output_dir: None,
        use_nerd_font: options.use_nerd_font,
        command_name: Some(options.command_name),
        allowed_tools: None,
        disallowed_tools: Some(format!("{},AskUserQuestion", DANGEROUS_TOOLS)),
        mcp_config: None,
        question_rx: None,
        tasks_path: Some(options.tasks_path),
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_run_options_construction() {
        let options = TaskRunOptions {
            prompt: "Test prompt".to_string(),
            command_name: "task test".to_string(),
            model: Some("claude-sonnet-4-5".to_string()),
            use_nerd_font: true,
            tasks_path: PathBuf::from("/tmp/tasks.yml"),
        };

        assert_eq!(options.prompt, "Test prompt");
        assert_eq!(options.command_name, "task test");
        assert_eq!(options.model, Some("claude-sonnet-4-5".to_string()));
        assert!(options.use_nerd_font);
        assert_eq!(options.tasks_path, PathBuf::from("/tmp/tasks.yml"));
    }
}
