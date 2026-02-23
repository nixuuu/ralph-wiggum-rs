//! Shared runner for task add/edit/plan commands.
//!
//! All three commands follow the same flow:
//! 1. Init tasks.yml (mode-specific)
//! 2. Resolve input (file > prompt > stdin)
//! 3. Build prompt from template (mode-specific)
//! 4. Resolve model
//! 5. Run Claude via run_task_command()
//! 6. Display results (mode-specific)

use std::path::{Path, PathBuf};

use crossterm::style::Stylize;

use super::input::resolve_input;
use super::task_runner::{TaskRunOptions, run_task_command};
use crate::shared::error::Result;
use crate::shared::file_config::{FileConfig, VerifyProfile, format_profiles_info};
use crate::shared::tasks::TasksFile;
use crate::templates;

/// Default model for plan command — opus for architectural reasoning.
const DEFAULT_PLAN_MODEL: &str = "opus";

/// Distinguishes the behaviour of each task command variant.
pub enum TaskCommandMode {
    Add,
    Edit,
    Plan,
}

impl TaskCommandMode {
    fn context_hint(&self) -> &'static str {
        match self {
            Self::Add => "Opisz zadania do dodania...",
            Self::Edit => "Opisz zmiany w istniejących zadaniach...",
            Self::Plan => "Opisz wymagania projektu do zaplanowania...",
        }
    }

    fn command_name(&self) -> &'static str {
        match self {
            Self::Add => "task add",
            Self::Edit => "task edit",
            Self::Plan => "task plan",
        }
    }

    /// Returns a model fallback when neither args nor config provide one.
    fn default_model(&self) -> Option<String> {
        match self {
            Self::Plan => Some(DEFAULT_PLAN_MODEL.to_string()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Execute a task command (add / edit / plan) using a shared flow.
pub async fn execute_task_command(
    mode: TaskCommandMode,
    file: Option<PathBuf>,
    prompt: Option<String>,
    model: Option<String>,
    file_config: &FileConfig,
) -> Result<()> {
    let tasks_path = &file_config.task.tasks_file;

    // 1. Init tasks.yml (mode-specific behaviour)
    let before = init_tasks_file(&mode, tasks_path)?;

    // 2. Resolve input (file > prompt > interactive stdin)
    let input = resolve_input(file.as_ref(), prompt.as_deref(), Some(mode.context_hint()))?;

    // 3. Build prompt from mode-specific template
    let prompt_str = build_prompt(&mode, &input, file_config, tasks_path);

    // 4. Resolve model: args > config > mode default
    let resolved_model = model
        .or_else(|| file_config.task.default_model.clone())
        .or_else(|| mode.default_model());

    // 5. Run Claude (blocks dangerous tools, auto-starts MCP server)
    run_task_command(TaskRunOptions {
        prompt: prompt_str,
        command_name: mode.command_name().to_string(),
        model: resolved_model,
        use_nerd_font: file_config.ui.nerd_font,
        tasks_path: tasks_path.clone(),
    })
    .await?;

    // 6. Display results
    display_results(&mode, tasks_path, before.as_ref())?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Step 1: init_tasks_file
// ---------------------------------------------------------------------------

/// Initialise the tasks file and return the pre-run snapshot.
///
/// - `Add`: conditional load vs load_or_init (prints init message when creating)
/// - `Edit`: always load_or_init
/// - `Plan`: ensures `.ralph/` dir exists, conditionally inits, returns None
fn init_tasks_file(mode: &TaskCommandMode, tasks_path: &Path) -> Result<Option<TasksFile>> {
    match mode {
        TaskCommandMode::Add => {
            let before = if tasks_path.exists() {
                TasksFile::load(tasks_path)?
            } else {
                println!(
                    "{}",
                    format!("Initializing {}...", tasks_path.display()).dark_grey()
                );
                TasksFile::load_or_init(tasks_path)?
            };
            Ok(Some(before))
        }
        TaskCommandMode::Edit => {
            let before = TasksFile::load_or_init(tasks_path)?;
            Ok(Some(before))
        }
        TaskCommandMode::Plan => {
            ensure_ralph_dir(tasks_path)?;
            if !tasks_path.exists() {
                TasksFile::load_or_init(tasks_path)?;
            }
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// Step 3: build_prompt
// ---------------------------------------------------------------------------

fn build_prompt(
    mode: &TaskCommandMode,
    input: &str,
    file_config: &FileConfig,
    tasks_path: &Path,
) -> String {
    match mode {
        TaskCommandMode::Add => {
            let profiles_section = build_profiles_section(&file_config.task.orchestrate.profiles);
            templates::ADD_PROMPT_YAML
                .replace("{requirements}", input)
                .replace("{profiles_section}", &profiles_section)
        }
        TaskCommandMode::Edit => {
            let profiles_info = format_profiles_info(&file_config.task.orchestrate.profiles);
            templates::EDIT_PROMPT_YAML
                .replace("{instructions}", input)
                .replace("{profiles_info}", &profiles_info)
        }
        TaskCommandMode::Plan => {
            let context = build_task_context(tasks_path);
            let profiles_info = format_profiles_info(&file_config.task.orchestrate.profiles);
            templates::PLAN_PROMPT
                .replace("{requirements}", input)
                .replace("{context}", &context)
                .replace("{profiles_info}", &profiles_info)
        }
    }
}

// ---------------------------------------------------------------------------
// Step 6: display_results
// ---------------------------------------------------------------------------

fn display_results(
    mode: &TaskCommandMode,
    tasks_path: &Path,
    before: Option<&TasksFile>,
) -> Result<()> {
    match mode {
        TaskCommandMode::Add => display_add_results(tasks_path, before),
        TaskCommandMode::Edit => display_edit_results(tasks_path, before),
        TaskCommandMode::Plan => {
            display_plan_results(tasks_path);
            Ok(())
        }
    }
}

fn display_add_results(tasks_path: &Path, before: Option<&TasksFile>) -> Result<()> {
    let before_count = before.map(|b| b.flatten_leaves().len()).unwrap_or(0);

    let after = TasksFile::load(tasks_path)?;
    let after_summary = after.to_summary();
    let new_tasks = after.flatten_leaves().len().saturating_sub(before_count);

    println!("{}", "━".repeat(60).dark_grey());
    println!("{} {} new task(s) added", "✓".green().bold(), new_tasks);
    println!(
        "  {} {} total ({} todo, {} done, {} blocked)",
        "Tasks:".dark_grey(),
        after_summary.total(),
        after_summary.todo,
        after_summary.done,
        after_summary.blocked
    );

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

fn display_edit_results(tasks_path: &Path, before: Option<&TasksFile>) -> Result<()> {
    let after = TasksFile::load(tasks_path)?;
    let after_summary = after.to_summary();

    println!("{}", "━".repeat(60).dark_grey());
    println!("{} tasks edited successfully", "✓".green().bold());

    if let Some(before) = before {
        let before_summary = before.to_summary();
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
    }

    println!(
        "  {} {} total ({} todo, {} done, {} blocked)",
        "Tasks:".dark_grey(),
        after_summary.total(),
        after_summary.todo,
        after_summary.done,
        after_summary.blocked
    );

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

fn display_plan_results(tasks_path: &Path) {
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Builds the profiles section for the add prompt template.
/// Returns empty string when no profiles are configured (omits the entire section).
fn build_profiles_section(profiles: &[VerifyProfile]) -> String {
    if profiles.is_empty() {
        return String::new();
    }

    let profiles_info = format_profiles_info(profiles);
    format!(
        r#"## DOSTĘPNE PROFILE WERYFIKACJI

{profiles_info}
**Przypisywanie profili do tasków:**
- Analizuj opis taska i `related_files` aby określić, które obszary kodu będą modyfikowane
- Przypisz profile, których wzorce `paths` pasują do plików związanych z taskiem
- Możesz przypisać wiele profili do jednego taska (np. `profiles: [backend, api]`)
- Profile są opcjonalne — pomiń pole `profiles` jeśli żaden profil nie pasuje lub gdy tworzymy zadanie organizacyjne/dokumentacyjne
- Profile wpływają na fazę weryfikacji podczas orchestrate — workery będą uruchamiać verify_commands odpowiednich profili

"#
    )
}

/// Build context string from existing tasks.yml, or "No existing tasks" if absent.
fn build_task_context(tasks_path: &Path) -> String {
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
fn ensure_ralph_dir(tasks_path: &Path) -> Result<()> {
    if let Some(parent) = tasks_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests (consolidated from add.rs and plan.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- build_profiles_section ---

    #[test]
    fn test_build_profiles_section_empty() {
        let result = build_profiles_section(&[]);
        assert_eq!(result, "");
    }

    #[test]
    fn test_build_profiles_section_with_profiles() {
        use crate::shared::file_config::VerifyCommand;

        let profiles = vec![VerifyProfile {
            name: "backend".to_string(),
            description: Some("Backend verification".to_string()),
            paths: vec!["src/**/*.rs".to_string()],
            working_dir: None,
            verify_commands: vec![VerifyCommand::Simple("cargo test".to_string())],
            setup_commands: vec![],
        }];

        let result = build_profiles_section(&profiles);
        assert!(result.contains("## DOSTĘPNE PROFILE WERYFIKACJI"));
        assert!(result.contains("**backend**"));
        assert!(result.contains("Przypisywanie profili do tasków"));
    }

    #[test]
    fn test_build_profiles_section_not_in_prompt_when_empty() {
        let prompt = templates::ADD_PROMPT_YAML
            .replace("{requirements}", "test")
            .replace("{profiles_section}", &build_profiles_section(&[]));

        assert!(!prompt.contains("DOSTĘPNE PROFILE WERYFIKACJI"));
        assert!(!prompt.contains("Przypisywanie profili"));
    }

    // --- auto-init logic (add mode) ---

    #[test]
    fn test_auto_init_logic_file_does_not_exist() {
        let dir = std::env::temp_dir().join("ralph_test_shared_auto_init_new");
        let _ = std::fs::remove_dir_all(&dir);

        let tasks_path = dir.join("tasks.yml");

        assert!(!tasks_path.exists());
        let before = TasksFile::load_or_init(&tasks_path).unwrap();

        assert_eq!(before.tasks.len(), 0);
        assert_eq!(before.default_model.as_deref(), Some("sonnet"));
        assert!(tasks_path.exists());
        let loaded = TasksFile::load(&tasks_path).unwrap();
        assert_eq!(loaded.tasks.len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_auto_init_logic_file_exists() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("ralph_test_shared_auto_init_existing");
        let _ = std::fs::create_dir_all(&dir);
        let tasks_path = dir.join("tasks.yml");

        let yaml = r#"
default_model: claude-opus-4-6

tasks:
  - id: "1"
    name: "Existing task"
    component: test
    status: todo
"#;
        let mut file = std::fs::File::create(&tasks_path).unwrap();
        file.write_all(yaml.as_bytes()).unwrap();
        drop(file);

        assert!(tasks_path.exists());
        let before = TasksFile::load(&tasks_path).unwrap();

        assert_eq!(before.tasks.len(), 1);
        assert_eq!(before.default_model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(before.flatten_leaves().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- before_count (add mode) ---

    #[test]
    fn test_before_count_empty() {
        let dir = std::env::temp_dir().join("ralph_test_shared_before_count_empty");
        let _ = std::fs::remove_dir_all(&dir);

        let tasks_path = dir.join("tasks.yml");
        let before = TasksFile::load_or_init(&tasks_path).unwrap();
        let before_count = before.flatten_leaves().len();

        assert_eq!(before_count, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_before_count_existing_tasks() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("ralph_test_shared_before_count_existing");
        let _ = std::fs::create_dir_all(&dir);
        let tasks_path = dir.join("tasks.yml");

        let yaml = r#"
tasks:
  - id: "1"
    name: "Task 1"
    status: todo
  - id: "2"
    name: "Task 2"
    status: done
  - id: "3"
    name: "Epic"
    subtasks:
      - id: "3.1"
        name: "Subtask"
        status: todo
"#;
        let mut file = std::fs::File::create(&tasks_path).unwrap();
        file.write_all(yaml.as_bytes()).unwrap();
        drop(file);

        let before = TasksFile::load(&tasks_path).unwrap();
        let before_count = before.flatten_leaves().len();

        // 3 leaf tasks: "1", "2", "3.1"
        assert_eq!(before_count, 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- build_task_context (plan mode) ---

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
        let dir = std::env::temp_dir().join("ralph_shared_plan_test_context");
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
        let dir = std::env::temp_dir().join("ralph_shared_plan_test_empty");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tasks.yml");

        std::fs::write(&path, "tasks: []\n").unwrap();

        let context = build_task_context(&path);
        assert!(context.contains("0 total"));
        assert!(context.contains("0% complete"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_task_context_invalid_yaml() {
        let dir = std::env::temp_dir().join("ralph_shared_plan_test_invalid");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tasks.yml");

        std::fs::write(&path, "{{invalid yaml").unwrap();

        let context = build_task_context(&path);
        assert!(context.contains("failed to parse"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_task_context_all_done() {
        let dir = std::env::temp_dir().join("ralph_shared_plan_test_all_done");
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

    // --- ensure_ralph_dir ---

    #[test]
    fn test_ensure_ralph_dir_creates_parent() {
        let dir = std::env::temp_dir().join("ralph_shared_plan_test_mkdir");
        let ralph_dir = dir.join(".ralph");
        let tasks_path = ralph_dir.join("tasks.yml");

        let _ = std::fs::remove_dir_all(&dir);

        assert!(!ralph_dir.exists());
        ensure_ralph_dir(&tasks_path).unwrap();
        assert!(ralph_dir.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ensure_ralph_dir_idempotent() {
        let dir = std::env::temp_dir().join("ralph_shared_plan_test_mkdir_idem");
        let ralph_dir = dir.join(".ralph");
        let tasks_path = ralph_dir.join("tasks.yml");

        let _ = std::fs::remove_dir_all(&dir);

        ensure_ralph_dir(&tasks_path).unwrap();
        ensure_ralph_dir(&tasks_path).unwrap();
        assert!(ralph_dir.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- prompt template substitution ---

    #[test]
    fn test_plan_prompt_template_substitution() {
        let prompt = templates::PLAN_PROMPT
            .replace("{requirements}", "Build a REST API")
            .replace("{context}", "No existing tasks")
            .replace("{profiles_info}", "");

        assert!(prompt.contains("Build a REST API"));
        assert!(prompt.contains("No existing tasks"));
        assert!(!prompt.contains("{requirements}"));
        assert!(!prompt.contains("{context}"));
        assert!(!prompt.contains("{profiles_info}"));
    }

    #[test]
    fn test_tasks_yml_initialized_after_ensure_ralph_dir() {
        let dir = std::env::temp_dir().join("ralph_shared_plan_test_init_tasks");
        let ralph_dir = dir.join(".ralph");
        let tasks_path = ralph_dir.join("tasks.yml");

        let _ = std::fs::remove_dir_all(&dir);

        ensure_ralph_dir(&tasks_path).unwrap();
        assert!(ralph_dir.exists());
        assert!(!tasks_path.exists());

        if !tasks_path.exists() {
            TasksFile::load_or_init(&tasks_path).unwrap();
        }

        assert!(tasks_path.exists());

        let tasks_file = TasksFile::load(&tasks_path).unwrap();
        assert_eq!(tasks_file.tasks.len(), 0);
        assert!(tasks_file.default_model.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- profiles_info ---

    #[test]
    fn test_profiles_info_empty_when_no_profiles() {
        let profiles: Vec<VerifyProfile> = vec![];
        let profiles_info = format_profiles_info(&profiles);
        assert_eq!(profiles_info, "");
    }

    #[test]
    fn test_profiles_info_with_profiles() {
        use crate::shared::file_config::{VerifyCommand, VerifyProfile};

        let profiles = vec![VerifyProfile {
            name: "rust".to_string(),
            description: Some("Rust verification".to_string()),
            paths: vec!["src/**/*.rs".to_string()],
            working_dir: None,
            verify_commands: vec![
                VerifyCommand::Simple("cargo check".to_string()),
                VerifyCommand::Simple("cargo test".to_string()),
            ],
            setup_commands: vec![],
        }];

        let profiles_info = format_profiles_info(&profiles);
        assert!(profiles_info.contains("**rust**"));
        assert!(profiles_info.contains("Rust verification"));
        assert!(profiles_info.contains("src/**/*.rs"));
        assert!(profiles_info.contains("cargo check"));
        assert!(profiles_info.contains("cargo test"));
    }

    #[test]
    fn test_plan_prompt_includes_profiles_info() {
        use crate::shared::file_config::VerifyProfile;

        let profiles = vec![VerifyProfile {
            name: "test".to_string(),
            description: None,
            paths: vec!["test/**".to_string()],
            working_dir: None,
            verify_commands: vec![],
            setup_commands: vec![],
        }];

        let profiles_info = format_profiles_info(&profiles);
        let prompt = templates::PLAN_PROMPT
            .replace("{requirements}", "Build API")
            .replace("{context}", "No tasks")
            .replace("{profiles_info}", &profiles_info);

        assert!(prompt.contains("Build API"));
        assert!(prompt.contains("No tasks"));
        assert!(prompt.contains("**test**"));
        assert!(!prompt.contains("{profiles_info}"));
    }
}
