use crossterm::style::Stylize;

use crate::commands::run::RunArgs;
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::progress;

pub async fn execute(file_config: &FileConfig) -> Result<()> {
    let progress_path = &file_config.task.progress_file;
    let system_prompt_path = &file_config.task.system_prompt_file;

    if !progress_path.exists() {
        return Err(RalphError::MissingFile(format!(
            "{} not found. Run `ralph-wiggum task prd` first.",
            progress_path.display()
        )));
    }

    if !system_prompt_path.exists() {
        return Err(RalphError::MissingFile(format!(
            "{} not found. Run `ralph-wiggum task prd` first.",
            system_prompt_path.display()
        )));
    }

    // Read system prompt
    let prompt = std::fs::read_to_string(system_prompt_path)?;

    // Parse progress
    let summary = progress::load_progress(progress_path)?;
    let remaining = summary.remaining() as u32;
    let min_iterations = remaining.max(1);
    let max_iterations = remaining + 5;

    let state_file = std::path::PathBuf::from(".claude/ralph-loop.local.md");

    // Print info
    if let Some(current) = progress::current_task(&summary) {
        println!(
            "\n  {} [{}] {} {}",
            "▶".cyan(),
            current.component.as_str().yellow(),
            current.id.as_str().cyan().bold(),
            current.name
        );
    }
    println!(
        "  {} {} remaining tasks, min_iterations={}, max_iterations={}",
        "ℹ".dark_grey(),
        remaining,
        min_iterations,
        max_iterations
    );
    println!();

    // Build RunArgs programmatically
    let args = RunArgs {
        prompt: Some(prompt),
        min_iterations,
        max_iterations,
        promise: "done".to_string(),
        resume: false,
        state_file,
        config: std::path::PathBuf::from(".ralph.toml"),
        continue_session: false,
        no_nf: false,
        progress_file: Some(progress_path.clone()),
    };

    crate::commands::run::execute(args).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_execute_validates_progress_file() {
        let config = FileConfig {
            task: crate::shared::file_config::TaskConfig {
                progress_file: PathBuf::from("/nonexistent/PROGRESS.md"),
                system_prompt_file: PathBuf::from("/nonexistent/CURRENT_TASK.md"),
                ..Default::default()
            },
            ..Default::default()
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(execute(&config));

        assert!(result.is_err());
        if let Err(RalphError::MissingFile(msg)) = result {
            assert!(msg.contains("PROGRESS.md"));
            assert!(msg.contains("not found"));
        } else {
            panic!("Expected MissingFile error");
        }
    }

    #[test]
    fn test_execute_validates_system_prompt_file() {
        use std::time::{SystemTime, UNIX_EPOCH};

        // Use unique filename to avoid race conditions in parallel tests
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir();
        let progress_path = temp_dir.join(format!("test_progress_{}.md", timestamp));

        std::fs::write(&progress_path, "# Progress\n\n- [ ] Task 1").unwrap();

        let config = FileConfig {
            task: crate::shared::file_config::TaskConfig {
                progress_file: progress_path.clone(),
                system_prompt_file: PathBuf::from("/nonexistent/CURRENT_TASK.md"),
                ..Default::default()
            },
            ..Default::default()
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(execute(&config));

        // Cleanup - ensure file is removed even if test fails
        if progress_path.exists() {
            std::fs::remove_file(&progress_path).expect("Failed to cleanup test file");
        }

        assert!(result.is_err());
        if let Err(RalphError::MissingFile(msg)) = result {
            assert!(msg.contains("CURRENT_TASK.md"));
            assert!(msg.contains("not found"));
        } else {
            panic!("Expected MissingFile error");
        }
    }
}

