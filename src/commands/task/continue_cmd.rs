use crossterm::style::Stylize;

use crate::commands::run::RunArgs;
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::tasks::{TasksFile, format_task_prompt};
use crate::templates;

pub async fn execute(file_config: &FileConfig) -> Result<()> {
    let tasks_path = &file_config.task.tasks_file;

    // Auto-initialize if file doesn't exist (instead of returning error)
    let tasks_file = TasksFile::load_or_init(tasks_path)?;

    // If file was just initialized (empty), show friendly message
    if tasks_file.tasks.is_empty() {
        println!("{}", "━".repeat(60).dark_grey());
        println!("{} No tasks yet.", "ℹ".cyan().bold());
        println!(
            "  Run {} or {} to get started.",
            "task add".cyan(),
            "task plan".cyan()
        );
        println!("{}", "━".repeat(60).dark_grey());
        return Ok(());
    }

    let current = tasks_file
        .current_task()
        .ok_or_else(|| RalphError::TaskSetup("No pending tasks found in tasks.yml.".to_string()))?;

    // Build system prompt from embedded template
    let task_prompt = format_task_prompt(&current);
    let prompt = templates::CONTINUE_SYSTEM_PROMPT.replace("{current_task_prompt}", &task_prompt);

    let summary = tasks_file.to_summary();
    let remaining = summary.remaining() as u32;
    let min_iterations = remaining.max(1);
    let max_iterations = remaining + 5;

    let state_file = std::path::PathBuf::from(".claude/ralph-loop.local.md");

    // Print info
    println!(
        "\n  {} [{}] {} {}",
        "▶".cyan(),
        current.component.as_str().yellow(),
        current.id.as_str().cyan().bold(),
        current.name
    );
    println!(
        "  {} {} remaining tasks, min_iterations={}, max_iterations={}",
        "ℹ".dark_grey(),
        remaining,
        min_iterations,
        max_iterations
    );
    println!();

    // Build RunArgs programmatically — pass tasks_path as progress_file for compatibility
    let args = RunArgs {
        prompt: Some(prompt),
        min_iterations,
        max_iterations,
        promise: "done".to_string(),
        resume: false,
        state_file,
        config: std::path::PathBuf::from(".ralph.toml"),
        continue_session: false,
        no_nf: false,
        debug: false,
        progress_file: Some(tasks_path.clone()),
    };

    crate::commands::run::execute(args).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execute_handles_empty_tasks_file() {
        // Test that continue gracefully handles empty tasks file
        let dir = std::env::temp_dir().join("ralph_test_continue_empty");
        let _ = std::fs::remove_dir_all(&dir);

        let tasks_path = dir.join("tasks.yml");

        let config = FileConfig {
            task: crate::shared::file_config::TaskConfig {
                tasks_file: tasks_path,
                ..Default::default()
            },
            ..Default::default()
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(execute(&config));

        // Should succeed with friendly message, not error
        assert!(result.is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
