use crossterm::style::Stylize;

use super::args::GenerateDepsArgs;
use crate::commands::run::{RunOnceOptions, run_once};
use crate::shared::dag::TaskDag;
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::tasks::TasksFile;
use crate::templates;

pub async fn execute(args: GenerateDepsArgs, file_config: &FileConfig) -> Result<()> {
    let tasks_path = &file_config.task.tasks_file;
    if !tasks_path.exists() {
        return Err(RalphError::MissingFile(format!(
            "{} not found. Run `ralph-wiggum task prd` first.",
            tasks_path.display()
        )));
    }

    // Get before state
    let before = TasksFile::load(tasks_path)?;

    // Build prompt (YAML template — Claude edits tasks.yml directly)
    let prompt = templates::DEPS_GENERATION_PROMPT_YAML.to_string();

    // Determine model
    let model = args
        .model
        .or_else(|| file_config.task.orchestrate.default_model.clone())
        .or_else(|| file_config.task.default_model.clone());

    // Run Claude with streaming output — Claude reads and edits .ralph/tasks.yml directly
    run_once(RunOnceOptions {
        prompt,
        model,
        output_dir: None,
        use_nerd_font: file_config.ui.nerd_font,
    })
    .await?;

    // Re-parse and show results
    let after = TasksFile::load(tasks_path)?;
    let deps_map = after.deps_map();

    println!("{}", "━".repeat(60).dark_grey());

    let deps_count = deps_map.values().filter(|v| !v.is_empty()).count();
    let roots = deps_map.values().filter(|v| v.is_empty()).count();

    if deps_count > 0 {
        println!(
            "{} dependencies generated for {} tasks",
            "✓".green().bold(),
            deps_map.len()
        );
        println!(
            "  {} {} with deps, {} roots (no deps)",
            "Graph:".dark_grey(),
            deps_count,
            roots
        );

        // Validate DAG
        let dag = TaskDag::from_tasks_file(&after);
        if let Some(cycle) = dag.detect_cycles() {
            println!(
                "  {} cycle detected: {}",
                "⚠".yellow().bold(),
                cycle.join(" → ")
            );
        } else {
            println!("  {} no cycles detected", "✓".green());
        }
    } else {
        println!("{} no deps found after generation", "⚠".yellow().bold());
    }

    let had_deps = before.deps_map().values().any(|v| !v.is_empty());
    if had_deps {
        println!("  {} existing deps were replaced", "Note:".dark_grey());
    }

    let summary = after.to_summary();
    println!(
        "  {} {} total ({} todo, {} done, {} blocked)",
        "Tasks:".dark_grey(),
        summary.total(),
        summary.todo,
        summary.done,
        summary.blocked
    );
    println!("{}", "━".repeat(60).dark_grey());

    Ok(())
}
