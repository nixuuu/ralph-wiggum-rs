use crossterm::style::Stylize;

use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::progress;

pub fn execute(file_config: &FileConfig) -> Result<()> {
    let progress_path = &file_config.task.progress_file;
    if !progress_path.exists() {
        return Err(RalphError::MissingFile(format!(
            "{} not found. Run `ralph-wiggum task prd` first.",
            progress_path.display()
        )));
    }

    let summary = progress::load_progress(progress_path)?;
    let total = summary.total();

    println!();
    println!("{}", "━".repeat(60).dark_grey());
    println!("  📋 Task Progress");
    println!("{}", "━".repeat(60).dark_grey());

    // Breakdown
    println!(
        "  {}  {}  {}  {}  {}  {}  {}  {}",
        "Done:".dark_grey(),
        summary.done.to_string().green().bold(),
        "In Progress:".dark_grey(),
        summary.in_progress.to_string().cyan().bold(),
        "Blocked:".dark_grey(),
        summary.blocked.to_string().red().bold(),
        "Todo:".dark_grey(),
        summary.todo.to_string().white().bold(),
    );

    // Current task
    if let Some(current) = progress::current_task(&summary) {
        let status_marker = match current.status {
            progress::TaskStatus::InProgress => "~".cyan().bold().to_string(),
            progress::TaskStatus::Todo => " ".to_string(),
            _ => "?".to_string(),
        };
        println!();
        println!(
            "  {} [{}] {} [{}] {}",
            "▶".cyan(),
            status_marker,
            current.id.as_str().cyan().bold(),
            current.component.as_str().yellow(),
            current.name.as_str().bold()
        );
    }

    // Progress bar (ASCII gauge)
    if total > 0 {
        let ratio = summary.done as f64 / total as f64;
        let bar_width = 40;
        let filled = (ratio * bar_width as f64).round() as usize;
        let empty = bar_width - filled;
        let bar = format!(
            "{}{}",
            "█".repeat(filled),
            "░".repeat(empty),
        );

        println!();
        println!(
            "  [{}] {}/{} ({}%)",
            bar.green(),
            summary.done.to_string().green().bold(),
            total,
            (ratio * 100.0).round() as u32,
        );
    }

    println!("{}", "━".repeat(60).dark_grey());
    println!();

    Ok(())
}
