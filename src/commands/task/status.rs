use crossterm::style::Stylize;

use crate::shared::error::Result;
use crate::shared::file_config::FileConfig;
use crate::shared::progress::TaskStatus;
use crate::shared::tasks::TasksFile;

pub fn execute(file_config: &FileConfig) -> Result<()> {
    let tasks_path = &file_config.task.tasks_file;

    // Auto-initialize if file doesn't exist (instead of returning error)
    let tasks_file = TasksFile::load_or_init(tasks_path)?;

    // If file was just initialized (empty), show friendly message
    if tasks_file.tasks.is_empty() {
        println!();
        println!("{}", "━".repeat(60).dark_grey());
        println!("{} No tasks yet.", "ℹ".cyan().bold());
        println!(
            "  Run {} or {} to get started.",
            "task add".cyan(),
            "task plan".cyan()
        );
        println!("{}", "━".repeat(60).dark_grey());
        println!();
        return Ok(());
    }

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

    // Display verification profiles if configured
    if !file_config.task.orchestrate.profiles.is_empty() {
        let profile_count = file_config.task.orchestrate.profiles.len();
        println!();
        println!(
            "  {} Profile weryfikacji: {} skonfigurowanych",
            "ⓘ".cyan(),
            profile_count.to_string().bold()
        );
        for profile in &file_config.task.orchestrate.profiles {
            let paths_str = if profile.paths.is_empty() {
                "brak ścieżek".dark_grey().to_string()
            } else {
                profile.paths.join(", ")
            };
            println!(
                "    {} {} ({})",
                "-".dark_grey(),
                profile.name.as_str().bold(),
                paths_str
            );
        }
        println!("{}", "━".repeat(60).dark_grey());
    }

    println!();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::file_config::{OrchestrateConfig, TaskConfig, VerifyProfile};
    use tempfile::TempDir;

    /// Creates a test config with given profiles and a valid temp tasks file.
    fn create_test_config(temp_dir: &TempDir, profiles: Vec<VerifyProfile>) -> FileConfig {
        let tasks_file = temp_dir.path().join("tasks.yml");
        std::fs::write(&tasks_file, "tasks: []").unwrap();

        FileConfig {
            task: TaskConfig {
                orchestrate: OrchestrateConfig {
                    profiles,
                    ..Default::default()
                },
                tasks_file,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_status_without_profiles() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir, vec![]);

        let result = execute(&config);
        assert!(result.is_ok(), "Should succeed without profiles");
    }

    #[test]
    fn test_status_with_single_profile() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(
            &temp_dir,
            vec![VerifyProfile {
                name: "frontend".to_string(),
                description: Some("Frontend tests".to_string()),
                paths: vec!["src/ui/**/*.rs".to_string(), "assets/**/*".to_string()],
                working_dir: None,
                verify_commands: vec![],
                setup_commands: vec![],
            }],
        );

        let result = execute(&config);
        assert!(result.is_ok(), "Should succeed with single profile");
    }

    #[test]
    fn test_status_with_multiple_profiles() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(
            &temp_dir,
            vec![
                VerifyProfile {
                    name: "backend".to_string(),
                    description: None,
                    paths: vec!["src/api/**/*.rs".to_string()],
                    working_dir: None,
                    verify_commands: vec![],
                    setup_commands: vec![],
                },
                VerifyProfile {
                    name: "database".to_string(),
                    description: Some("Database migrations".to_string()),
                    paths: vec!["migrations/**/*.sql".to_string()],
                    working_dir: Some("db".to_string()),
                    verify_commands: vec![],
                    setup_commands: vec![],
                },
            ],
        );

        let result = execute(&config);
        assert!(result.is_ok(), "Should succeed with multiple profiles");
    }

    #[test]
    fn test_status_with_profile_without_paths() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(
            &temp_dir,
            vec![VerifyProfile {
                name: "global-checks".to_string(),
                description: Some("Global checks".to_string()),
                paths: vec![],
                working_dir: None,
                verify_commands: vec![],
                setup_commands: vec![],
            }],
        );

        let result = execute(&config);
        assert!(result.is_ok(), "Should succeed with profile without paths");
    }
}
