use crossterm::style::Stylize;

use super::args::PrdArgs;
use super::input::resolve_input;
use crate::shared::claude::{ClaudeOnceOptions, run_claude_once};
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::progress;
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

    // Create boilerplate files if missing
    create_boilerplate_if_missing(&output_dir, file_config)?;

    // Check if CLAUDE.md exists
    let claude_md_path = output_dir.join("CLAUDE.md");
    let existing_claude_md = if claude_md_path.exists() {
        "\nNote: CLAUDE.md already exists in the project. Do NOT create or modify it.".to_string()
    } else {
        String::new()
    };

    // Build prompt
    let prompt = templates::PRD_PROMPT
        .replace("{prd_content}", &input)
        .replace("{output_dir}", &output_dir.display().to_string())
        .replace("{existing_claude_md}", &existing_claude_md);

    // Determine model
    let model = args.model.or_else(|| file_config.task.default_model.clone());

    println!(
        "\n{} Generating project files from PRD...\n",
        "▶".cyan()
    );

    // Run Claude
    run_claude_once(ClaudeOnceOptions {
        prompt,
        model,
        output_dir: Some(output_dir.clone()),
    })
    .await?;

    println!();

    // Verify required outputs exist
    let progress_path = output_dir.join(&file_config.task.progress_file);
    let system_prompt_path = output_dir.join(&file_config.task.system_prompt_file);

    if !progress_path.exists() {
        return Err(RalphError::TaskSetup(format!(
            "{} was not created by Claude. Please check the PRD and try again.",
            file_config.task.progress_file.display()
        )));
    }

    if !system_prompt_path.exists() {
        return Err(RalphError::TaskSetup(format!(
            "{} was not created by Claude. Please check the PRD and try again.",
            file_config.task.system_prompt_file.display()
        )));
    }

    // Print summary
    let summary = progress::load_progress(&progress_path)?;
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

    if let Some(current) = progress::current_task(&summary) {
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

fn create_boilerplate_if_missing(
    output_dir: &std::path::Path,
    file_config: &FileConfig,
) -> Result<()> {
    let files = [
        (&file_config.task.files.changenotes, templates::CHANGENOTES_TEMPLATE),
        (&file_config.task.files.issues, templates::ISSUES_TEMPLATE),
        (&file_config.task.files.questions, templates::QUESTIONS_TEMPLATE),
    ];

    for (filename, content) in &files {
        let path = output_dir.join(filename);
        if !path.exists() {
            std::fs::write(&path, content).map_err(|e| {
                RalphError::TaskSetup(format!("Failed to create {}: {}", path.display(), e))
            })?;
        }
    }

    Ok(())
}
