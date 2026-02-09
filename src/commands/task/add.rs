use crossterm::style::Stylize;

use super::args::AddArgs;
use super::input::resolve_input;
use crate::commands::run::{RunOnceOptions, run_once};
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::progress;
use crate::templates;

pub async fn execute(args: AddArgs, file_config: &FileConfig) -> Result<()> {
    let progress_path = &file_config.task.progress_file;
    if !progress_path.exists() {
        return Err(RalphError::MissingFile(format!(
            "{} not found. Run `ralph-wiggum task prd` first.",
            progress_path.display()
        )));
    }

    // Get before count
    let before = progress::load_progress(progress_path)?;

    // Resolve input
    let input = resolve_input(args.file.as_ref(), args.prompt.as_deref())?;

    // Build prompt
    let prompt = templates::ADD_PROMPT.replace("{requirements}", &input);

    // Determine model
    let model = args.model.or_else(|| file_config.task.default_model.clone());

    // Run Claude with streaming output
    run_once(RunOnceOptions {
        prompt,
        model,
        output_dir: None,
        use_nerd_font: file_config.ui.nerd_font,
    })
    .await?;

    // Re-parse and show diff
    let after = progress::load_progress(progress_path)?;
    let new_tasks = after.total().saturating_sub(before.total());

    println!("{}", "━".repeat(60).dark_grey());
    println!("{} {} new task(s) added", "✓".green().bold(), new_tasks);
    println!(
        "  {} {} total ({} todo, {} done, {} blocked)",
        "Tasks:".dark_grey(),
        after.total(),
        after.todo,
        after.done,
        after.blocked
    );

    // Update state file if it exists
    update_state_file(file_config, &after)?;

    if let Some(current) = progress::current_task(&after) {
        println!(
            "  {} {} [{}] {}",
            "Current:".dark_grey(),
            current.id.as_str().cyan(),
            current.component.as_str().yellow(),
            current.name
        );
    }
    println!("{}", "━".repeat(60).dark_grey());

    Ok(())
}

/// Update the state file (if it exists) with new min_iterations from updated task count.
fn update_state_file(file_config: &FileConfig, summary: &progress::ProgressSummary) -> Result<()> {
    use crate::commands::run::state::StateManager;
    use std::path::PathBuf;

    let state_path = PathBuf::from(".claude/ralph-loop.local.md");
    if !state_path.exists() {
        return Ok(());
    }

    let (mut state, prompt) = StateManager::load_from_file(&state_path)?;
    let remaining = summary.remaining() as u32;
    let new_min = state.iteration.saturating_add(remaining);
    state.min_iterations = new_min.max(state.iteration);
    state.max_iterations = new_min + 5;

    // Save state back
    let yaml = serde_yaml::to_string(&state)?;
    let content = format!("---\n{}---\n\n{}", yaml, prompt);
    std::fs::write(&state_path, content).map_err(|e| {
        crate::shared::error::RalphError::StateFile(format!("Failed to write state file: {}", e))
    })?;

    let _ = file_config; // used for consistency
    Ok(())
}
