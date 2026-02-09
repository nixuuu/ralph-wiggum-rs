use crossterm::style::Stylize;

use crate::commands::run::RunArgs;
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::progress;

pub async fn execute(file_config: &FileConfig) -> Result<()> {
    let progress_path = &file_config.task.progress_file;
    let system_prompt_path = &file_config.task.system_prompt_file;

    if !progress_path.exists() {
        return Err(RalphError::MissingFile(format!(
            "{} not found. Run `ralph-wiggum task prd` first.",
            progress_path.display()
        )));
    }

    if !system_prompt_path.exists() {
        return Err(RalphError::MissingFile(format!(
            "{} not found. Run `ralph-wiggum task prd` first.",
            system_prompt_path.display()
        )));
    }

    // Read system prompt
    let prompt = std::fs::read_to_string(system_prompt_path)?;

    // Parse progress
    let summary = progress::load_progress(progress_path)?;
    let remaining = summary.remaining() as u32;
    let min_iterations = remaining.max(1);
    let max_iterations = remaining + 5;

    // Check if state file exists for resume
    let state_file = std::path::PathBuf::from(".claude/ralph-loop.local.md");
    let resume = state_file.exists();

    // Print info
    if let Some(current) = progress::current_task(&summary) {
        println!(
            "\n  {} [{}] {} {}",
            "▶".cyan(),
            current.component.as_str().yellow(),
            current.id.as_str().cyan().bold(),
            current.name
        );
    }
    println!(
        "  {} {} remaining tasks, min_iterations={}, max_iterations={}",
        "ℹ".dark_grey(),
        remaining,
        min_iterations,
        max_iterations
    );
    if resume {
        println!("  {} Resuming from state file", "↻".dark_grey());
    }
    println!();

    // Build RunArgs programmatically
    let args = RunArgs {
        prompt: Some(prompt),
        min_iterations,
        max_iterations,
        promise: "done".to_string(),
        resume,
        state_file,
        config: std::path::PathBuf::from(".ralph.toml"),
        continue_session: true,
        no_nf: false,
        progress_file: Some(progress_path.clone()),
    };

    crate::commands::run::execute(args).await
}
