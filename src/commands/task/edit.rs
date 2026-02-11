use crossterm::style::Stylize;

use super::args::EditArgs;
use super::input::resolve_input;
use crate::commands::run::{RunOnceOptions, run_once};
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::tasks::TasksFile;
use crate::templates;

pub async fn execute(args: EditArgs, file_config: &FileConfig) -> Result<()> {
    let tasks_path = &file_config.task.tasks_file;
    if !tasks_path.exists() {
        return Err(RalphError::MissingFile(format!(
            "{} not found. Run `ralph-wiggum task prd` first.",
            tasks_path.display()
        )));
    }

    // Get before state
    let before = TasksFile::load(tasks_path)?;
    let before_summary = before.to_summary();

    // Resolve input
    let input = resolve_input(args.file.as_ref(), args.prompt.as_deref())?;

    // Build prompt (YAML template)
    let prompt = templates::EDIT_PROMPT_YAML.replace("{instructions}", &input);

    // Determine model
    let model = args
        .model
        .or_else(|| file_config.task.default_model.clone());

    // Run Claude with streaming output
    run_once(RunOnceOptions {
        prompt,
        model,
        output_dir: None,
        use_nerd_font: file_config.ui.nerd_font,
    })
    .await?;

    // Re-parse and show diff
    let after = TasksFile::load(tasks_path)?;
    let after_summary = after.to_summary();

    println!("{}", "━".repeat(60).dark_grey());
    println!("{} tasks edited successfully", "✓".green().bold());

    let total_diff = after_summary.total() as i32 - before_summary.total() as i32;
    let done_diff = after_summary.done as i32 - before_summary.done as i32;

    if total_diff != 0 {
        let sign = if total_diff > 0 { "+" } else { "" };
        println!(
            "  {} {}{} task(s) (was {}, now {})",
            "Count:".dark_grey(),
            sign,
            total_diff,
            before_summary.total(),
            after_summary.total()
        );
    }

    if done_diff != 0 {
        let sign = if done_diff > 0 { "+" } else { "" };
        println!(
            "  {} {}{} done (was {}, now {})",
            "Done:".dark_grey(),
            sign,
            done_diff,
            before_summary.done,
            after_summary.done
        );
    }

    println!(
        "  {} {} total ({} todo, {} done, {} blocked)",
        "Tasks:".dark_grey(),
        after_summary.total(),
        after_summary.todo,
        after_summary.done,
        after_summary.blocked
    );

    // Update state file if it exists
    update_state_file(file_config, &after_summary)?;

    if let Some(current) = after.current_task() {
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
fn update_state_file(
    file_config: &FileConfig,
    summary: &crate::shared::progress::ProgressSummary,
) -> Result<()> {
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
