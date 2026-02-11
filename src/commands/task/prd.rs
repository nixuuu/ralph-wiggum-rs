use crossterm::style::Stylize;

use super::args::PrdArgs;
use super::input::resolve_input;
use crate::commands::run::{RunOnceOptions, run_once};
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::tasks::TasksFile;
use crate::templates;

pub async fn execute(args: PrdArgs, file_config: &FileConfig) -> Result<()> {
    // Resolve input
    let input = resolve_input(args.file.as_ref(), args.prompt.as_deref())?;

    // Determine output dir
    let output_dir = args
        .output_dir
        .clone()
        .or_else(|| file_config.task.output_dir.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Ensure .ralph/ exists
    let ralph_dir = output_dir.join(".ralph");
    std::fs::create_dir_all(&ralph_dir)?;

    // Build prompt (YAML template)
    let prompt = templates::PRD_PROMPT_YAML
        .replace("{prd_content}", &input)
        .replace("{output_dir}", &output_dir.display().to_string());

    // Determine model
    let model = args
        .model
        .or_else(|| file_config.task.default_model.clone());

    // Run Claude with streaming output (all tools — needs Write for file generation)
    run_once(RunOnceOptions {
        prompt,
        model,
        output_dir: Some(output_dir.clone()),
        use_nerd_font: file_config.ui.nerd_font,
        allowed_tools: None,
        mcp_config: None,
    })
    .await?;

    // Verify required outputs exist
    let tasks_path = output_dir.join(&file_config.task.tasks_file);

    if !tasks_path.exists() {
        return Err(RalphError::TaskSetup(format!(
            "{} was not created by Claude. Please check the PRD and try again.",
            file_config.task.tasks_file.display()
        )));
    }

    // Print summary via TasksFile adapter
    let tasks_file = TasksFile::load(&tasks_path)?;
    let summary = tasks_file.to_summary();
    println!("{}", "━".repeat(60).dark_grey());
    println!(
        "{} Project files generated successfully!",
        "✓".green().bold()
    );
    println!("{}", "━".repeat(60).dark_grey());
    println!(
        "  {} {} tasks ({} todo, {} in progress)",
        "Tasks:".dark_grey(),
        summary.total(),
        summary.todo,
        summary.in_progress
    );

    if let Some(current) = tasks_file.current_task() {
        println!(
            "  {} {} [{}] {}",
            "Next:".dark_grey(),
            current.id.as_str().cyan(),
            current.component.as_str().yellow(),
            current.name
        );
    }

    println!();
    println!(
        "  {} {}",
        "Run:".dark_grey(),
        "ralph-wiggum task continue".cyan()
    );
    println!("{}", "━".repeat(60).dark_grey());

    Ok(())
}
