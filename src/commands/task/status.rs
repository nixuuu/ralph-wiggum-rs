use crossterm::style::Stylize;

use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::progress::TaskStatus;
use crate::shared::tasks::TasksFile;

pub fn execute(file_config: &FileConfig) -> Result<()> {
    let tasks_path = &file_config.task.tasks_file;
    if !tasks_path.exists() {
        return Err(RalphError::MissingFile(format!(
            "{} not found. Run `ralph-wiggum task prd` first.",
            tasks_path.display()
        )));
    }

    let tasks_file = TasksFile::load(tasks_path)?;
    let summary = tasks_file.to_summary();
    let total = summary.total();

    println!();
    println!("{}", "━".repeat(60).dark_grey());
    println!("  Task Progress");
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
    if let Some(current) = tasks_file.current_task() {
        let status_marker = match current.status {
            TaskStatus::InProgress => "~".cyan().bold().to_string(),
            TaskStatus::Todo => " ".to_string(),
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
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty),);

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
