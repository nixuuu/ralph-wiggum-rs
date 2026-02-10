use crossterm::style::Stylize;

use super::args::GenerateDepsArgs;
use crate::commands::run::{RunOnceOptions, run_once};
use crate::shared::dag::TaskDag;
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::progress;
use crate::templates;

pub async fn execute(args: GenerateDepsArgs, file_config: &FileConfig) -> Result<()> {
    let progress_path = &file_config.task.progress_file;
    if !progress_path.exists() {
        return Err(RalphError::MissingFile(format!(
            "{} not found. Run `ralph-wiggum task prd` first.",
            progress_path.display()
        )));
    }

    // Get before state
    let before = progress::load_progress(progress_path)?;

    // Build prompt
    let prompt = templates::DEPS_GENERATION_PROMPT.to_string();

    // Determine model
    let model = args
        .model
        .or_else(|| file_config.task.orchestrate.default_model.clone())
        .or_else(|| file_config.task.default_model.clone());

    // Run Claude with streaming output — Claude reads and edits PROGRESS.md directly
    run_once(RunOnceOptions {
        prompt,
        model,
        output_dir: None,
        use_nerd_font: file_config.ui.nerd_font,
    })
    .await?;

    // Re-parse and show results
    let after = progress::load_progress(progress_path)?;

    println!("{}", "━".repeat(60).dark_grey());

    match &after.frontmatter {
        Some(fm) if !fm.deps.is_empty() => {
            let deps_count = fm.deps.values().filter(|v| !v.is_empty()).count();
            let roots = fm.deps.values().filter(|v| v.is_empty()).count();

            println!(
                "{} dependencies generated for {} tasks",
                "✓".green().bold(),
                fm.deps.len()
            );
            println!(
                "  {} {} with deps, {} roots (no deps)",
                "Graph:".dark_grey(),
                deps_count,
                roots
            );

            // Validate DAG
            let dag = TaskDag::from_frontmatter(fm);
            if let Some(cycle) = dag.detect_cycles() {
                println!(
                    "  {} cycle detected: {}",
                    "⚠".yellow().bold(),
                    cycle.join(" → ")
                );
            } else {
                println!("  {} no cycles detected", "✓".green());
            }
        }
        _ => {
            println!(
                "{} no deps frontmatter found after generation",
                "⚠".yellow().bold()
            );
        }
    }

    let had_deps = before
        .frontmatter
        .as_ref()
        .is_some_and(|fm| !fm.deps.is_empty());
    if had_deps {
        println!("  {} existing deps were replaced", "Note:".dark_grey());
    }

    println!(
        "  {} {} total ({} todo, {} done, {} blocked)",
        "Tasks:".dark_grey(),
        after.total(),
        after.todo,
        after.done,
        after.blocked
    );
    println!("{}", "━".repeat(60).dark_grey());

    Ok(())
}
