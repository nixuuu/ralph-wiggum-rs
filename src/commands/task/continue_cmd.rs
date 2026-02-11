use crossterm::style::Stylize;

use crate::commands::run::RunArgs;
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::tasks::{format_task_prompt, TasksFile};
use crate::templates;

pub async fn execute(file_config: &FileConfig) -> Result<()> {
    let tasks_path = &file_config.task.tasks_file;

    if !tasks_path.exists() {
        return Err(RalphError::MissingFile(format!(
            "{} not found. Run `ralph-wiggum task prd` first.",
            tasks_path.display()
        )));
    }

    // Parse tasks.yml and find current task
    let tasks_file = TasksFile::load(tasks_path)?;

    let current = tasks_file.current_task().ok_or_else(|| {
        RalphError::TaskSetup("No pending tasks found in tasks.yml.".to_string())
    })?;

    // Build system prompt from embedded template
    let task_prompt = format_task_prompt(&current);
    let prompt = templates::CONTINUE_SYSTEM_PROMPT
        .replace("{current_task_prompt}", &task_prompt);

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
        progress_file: Some(tasks_path.clone()),
    };

    crate::commands::run::execute(args).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_execute_validates_tasks_file() {
        let config = FileConfig {
            task: crate::shared::file_config::TaskConfig {
                tasks_file: PathBuf::from("/nonexistent/tasks.yml"),
                ..Default::default()
            },
            ..Default::default()
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(execute(&config));

        assert!(result.is_err());
        if let Err(RalphError::MissingFile(msg)) = result {
            assert!(msg.contains("tasks.yml"));
            assert!(msg.contains("not found"));
        } else {
            panic!("Expected MissingFile error");
        }
    }
}
