use crossterm::style::Stylize;

use super::args::PlanArgs;
use super::input::resolve_input;
use crate::commands::run::{DANGEROUS_TOOLS, RunOnceOptions, run_once};
use crate::shared::error::Result;
use crate::shared::file_config::FileConfig;
use crate::shared::tasks::TasksFile;
use crate::templates;

/// Default model for plan command — uses opus for architectural reasoning
const DEFAULT_PLAN_MODEL: &str = "opus";

pub async fn execute(args: PlanArgs, file_config: &FileConfig) -> Result<()> {
    // Resolve input (file > prompt > stdin)
    let input = resolve_input(args.file.as_ref(), args.prompt.as_deref())?;

    let tasks_path = file_config.task.tasks_file.clone();

    // Detect tasks.yml or create .ralph/ directory
    let context = build_task_context(&tasks_path);
    ensure_ralph_dir(&tasks_path)?;

    // Build prompt from template
    let prompt = templates::PLAN_PROMPT
        .replace("{requirements}", &input)
        .replace("{context}", &context);

    // Model: args > config > default (opus for plan)
    let model = Some(
        args.model
            .or_else(|| file_config.task.default_model.clone())
            .unwrap_or_else(|| DEFAULT_PLAN_MODEL.to_string()),
    );

    // Run Claude with auto-started MCP server.
    // The MCP server and question channel are automatically managed by run_once.
    // Use disallowed_tools denylist to block dangerous built-in tools (Write, Edit, Bash, etc.)
    // while allowing MCP tools (ask_user, tasks_create, etc.) to pass through.
    // The plan_prompt.md instructs Claude to use exploration tools (Read, Glob, Grep, WebFetch,
    // WebSearch) and MCP tools (ask_user, tasks_*) while avoiding direct file modifications.
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

    // Display task status on completion
    display_completion_status(&tasks_path);

    Ok(())
}

/// Build context string from existing tasks.yml, or "No existing tasks" if absent.
fn build_task_context(tasks_path: &std::path::Path) -> String {
    if !tasks_path.exists() {
        return "No existing tasks".to_string();
    }

    match TasksFile::load(tasks_path) {
        Ok(tasks_file) => {
            let summary = tasks_file.to_summary();
            let total = summary.total();
            let pct = if total > 0 {
                (summary.done as f64 / total as f64 * 100.0) as u32
            } else {
                0
            };

            format!(
                "Existing tasks: {} total, {} done, {} todo, {} in_progress, {} blocked ({}% complete)",
                total, summary.done, summary.todo, summary.in_progress, summary.blocked, pct
            )
        }
        Err(_) => "No existing tasks (failed to parse tasks.yml)".to_string(),
    }
}

/// Ensure `.ralph/` directory exists (creates if absent).
fn ensure_ralph_dir(tasks_path: &std::path::Path) -> Result<()> {
    if let Some(parent) = tasks_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Display task status after successful plan generation.
fn display_completion_status(tasks_path: &std::path::Path) {
    if !tasks_path.exists() {
        println!(
            "\n{} Plan session completed. No tasks file was created.",
            "ℹ".cyan()
        );
        return;
    }

    let tasks_file = match TasksFile::load(tasks_path) {
        Ok(tf) => tf,
        Err(_) => {
            println!(
                "\n{} Plan session completed but tasks file could not be parsed.",
                "⚠".yellow()
            );
            return;
        }
    };

    let summary = tasks_file.to_summary();

    println!("{}", "━".repeat(60).dark_grey());
    println!("{} Plan generated successfully!", "✓".green().bold());
    println!("{}", "━".repeat(60).dark_grey());
    println!(
        "  {} {} tasks ({} todo, {} in progress, {} done, {} blocked)",
        "Tasks:".dark_grey(),
        summary.total(),
        summary.todo,
        summary.in_progress,
        summary.done,
        summary.blocked
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_default_plan_model() {
        assert_eq!(DEFAULT_PLAN_MODEL, "opus");
    }

    #[test]
    fn test_build_task_context_no_file() {
        let path = PathBuf::from("/nonexistent/path/tasks.yml");
        let context = build_task_context(&path);
        assert_eq!(context, "No existing tasks");
    }

    #[test]
    fn test_build_task_context_with_tasks() {
        let dir = std::env::temp_dir().join("ralph_plan_test_context");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tasks.yml");

        let yaml = r#"
default_model: sonnet
tasks:
  - id: "1"
    name: "Task A"
    status: done
    component: api
  - id: "2"
    name: "Task B"
    status: todo
    component: api
  - id: "3"
    name: "Task C"
    status: in_progress
    component: ui
"#;
        std::fs::write(&path, yaml).unwrap();

        let context = build_task_context(&path);
        assert!(context.contains("3 total"));
        assert!(context.contains("1 done"));
        assert!(context.contains("1 todo"));
        assert!(context.contains("1 in_progress"));
        assert!(context.contains("33% complete"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_task_context_empty_tasks() {
        let dir = std::env::temp_dir().join("ralph_plan_test_empty");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tasks.yml");

        let yaml = "tasks: []\n";
        std::fs::write(&path, yaml).unwrap();

        let context = build_task_context(&path);
        assert!(context.contains("0 total"));
        assert!(context.contains("0% complete"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_task_context_invalid_yaml() {
        let dir = std::env::temp_dir().join("ralph_plan_test_invalid");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tasks.yml");

        std::fs::write(&path, "{{invalid yaml").unwrap();

        let context = build_task_context(&path);
        assert!(context.contains("failed to parse"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_task_context_all_done() {
        let dir = std::env::temp_dir().join("ralph_plan_test_all_done");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tasks.yml");

        let yaml = r#"
tasks:
  - id: "1"
    name: "Done A"
    status: done
    component: api
  - id: "2"
    name: "Done B"
    status: done
    component: api
"#;
        std::fs::write(&path, yaml).unwrap();

        let context = build_task_context(&path);
        assert!(context.contains("100% complete"));
        assert!(context.contains("2 done"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ensure_ralph_dir_creates_parent() {
        let dir = std::env::temp_dir().join("ralph_plan_test_mkdir");
        let ralph_dir = dir.join(".ralph");
        let tasks_path = ralph_dir.join("tasks.yml");

        // Clean up if exists from previous run
        let _ = std::fs::remove_dir_all(&dir);

        assert!(!ralph_dir.exists());
        ensure_ralph_dir(&tasks_path).unwrap();
        assert!(ralph_dir.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ensure_ralph_dir_idempotent() {
        let dir = std::env::temp_dir().join("ralph_plan_test_mkdir_idem");
        let ralph_dir = dir.join(".ralph");
        let tasks_path = ralph_dir.join("tasks.yml");

        let _ = std::fs::remove_dir_all(&dir);

        ensure_ralph_dir(&tasks_path).unwrap();
        ensure_ralph_dir(&tasks_path).unwrap(); // second call should not fail
        assert!(ralph_dir.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_prompt_template_substitution() {
        let prompt = templates::PLAN_PROMPT
            .replace("{requirements}", "Build a REST API")
            .replace("{context}", "No existing tasks");

        assert!(prompt.contains("Build a REST API"));
        assert!(prompt.contains("No existing tasks"));
        assert!(!prompt.contains("{requirements}"));
        assert!(!prompt.contains("{context}"));
    }
}
