use crossterm::style::Stylize;

use super::args::EditArgs;
use super::input::resolve_input;
use super::task_runner::{TaskRunOptions, run_task_command};
use crate::shared::error::Result;
use crate::shared::file_config::{FileConfig, format_profiles_info};
use crate::shared::tasks::TasksFile;
use crate::templates;

pub async fn execute(args: EditArgs, file_config: &FileConfig) -> Result<()> {
    let tasks_path = &file_config.task.tasks_file;

    // Auto-initialize if file doesn't exist (instead of returning error)
    let before = TasksFile::load_or_init(tasks_path)?;
    let before_summary = before.to_summary();

    // Resolve input
    let input = resolve_input(
        args.file.as_ref(),
        args.prompt.as_deref(),
        Some("Opisz zmiany w istniejących zadaniach..."),
    )?;

    // Generate profiles_info from FileConfig
    let profiles_info = if file_config.task.orchestrate.profiles.is_empty() {
        "No verification profiles configured.".to_string()
    } else {
        format_profiles_info(&file_config.task.orchestrate.profiles)
    };

    // Build prompt (YAML template)
    let prompt = templates::EDIT_PROMPT_YAML
        .replace("{instructions}", &input)
        .replace("{profiles_info}", &profiles_info);

    // Determine model
    let model = args
        .model
        .or_else(|| file_config.task.default_model.clone());

    // Run Claude with fullscreen TUI output + inline ask_user widgets.
    // Uses run_task_command() which:
    // - Blocks dangerous tools (Write, Edit, Bash, etc.)
    // - Blocks AskUserQuestion to enforce MCP ask_user flow
    // - Auto-starts MCP server with tasks_path
    // - Renders ask_user questions as inline TUI widgets
    run_task_command(TaskRunOptions {
        prompt,
        command_name: "task edit".to_string(),
        model,
        use_nerd_font: file_config.ui.nerd_font,
        tasks_path: tasks_path.clone(),
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
