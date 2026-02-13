use crossterm::style::Stylize;

use super::args::EditArgs;
use super::input::resolve_input;
use crate::commands::run::{DANGEROUS_TOOLS, RunOnceOptions, run_once};
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

    // Run Claude with readonly built-in tools + MCP server for task mutations.
    // Block AskUserQuestion to enforce MCP ask_user flow.
    run_once(RunOnceOptions {
        prompt,
        model,
        output_dir: None,
        use_nerd_font: file_config.ui.nerd_font,
        allowed_tools: None,
        disallowed_tools: Some(format!("{},AskUserQuestion", DANGEROUS_TOOLS)),
        mcp_config: None,
        question_rx: None,
        tasks_path: Some(tasks_path.clone()),
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
    let state_path = std::path::PathBuf::from(".claude/ralph-loop.local.md");
    super::state_helper::update_state_file(&state_path, &after_summary)?;

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
