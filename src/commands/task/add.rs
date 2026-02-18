use crossterm::style::Stylize;

use super::args::AddArgs;
use super::input::resolve_input;
use super::task_runner::{TaskRunOptions, run_task_command};
use crate::shared::error::Result;
use crate::shared::file_config::{FileConfig, VerifyProfile, format_profiles_info};
use crate::shared::tasks::TasksFile;
use crate::templates;

/// Execute task add command.
///
/// Delegates to run_once() with MCP server for task mutations.
/// TODO(5.4): Integrate fullscreen TUI via TaskCommandApp with inline ask_user widgets.
pub async fn execute(args: AddArgs, file_config: &FileConfig) -> Result<()> {
    let tasks_path = &file_config.task.tasks_file;

    // Auto-initialize if file doesn't exist
    let before = if tasks_path.exists() {
        TasksFile::load(tasks_path)?
    } else {
        println!(
            "{}",
            format!("Initializing {}...", tasks_path.display()).dark_grey()
        );
        TasksFile::load_or_init(tasks_path)?
    };
    let before_count = before.flatten_leaves().len();

    // Resolve input
    let input = resolve_input(
        args.file.as_ref(),
        args.prompt.as_deref(),
        Some("Opisz zadania do dodania..."),
    )?;

    // Build profiles section only when profiles are configured
    let profiles_section = build_profiles_section(&file_config.task.orchestrate.profiles);

    // Build prompt (YAML template)
    let prompt = templates::ADD_PROMPT_YAML
        .replace("{requirements}", &input)
        .replace("{profiles_section}", &profiles_section);

    // Determine model
    let model = args
        .model
        .or_else(|| file_config.task.default_model.clone());

    // Run Claude with readonly built-in tools + MCP server for task mutations.
    // Blocks dangerous tools and AskUserQuestion via run_task_command().
    run_task_command(TaskRunOptions {
        prompt,
        command_name: "task add".to_string(),
        model,
        use_nerd_font: file_config.ui.nerd_font,
        tasks_path: tasks_path.clone(),
    })
    .await?;

    // Re-parse and show diff
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Test: Logika auto-inicjalizacji — gdy plik nie istnieje, użyj load_or_init()
    #[test]
    fn test_auto_init_logic_file_does_not_exist() {
        let dir = std::env::temp_dir().join("ralph_test_add_auto_init_new");
        let _ = std::fs::remove_dir_all(&dir);

        let tasks_path = dir.join("tasks.yml");

        // Symulujemy logikę z execute(): plik nie istnieje, więc używamy load_or_init
        assert!(!tasks_path.exists());
        let before = TasksFile::load_or_init(&tasks_path).unwrap();

        // Verify empty TasksFile z default model
        assert_eq!(before.tasks.len(), 0);
        assert_eq!(before.default_model.as_deref(), Some("sonnet"));

        // Verify plik został stworzony na dysku
        assert!(tasks_path.exists());
        let loaded = TasksFile::load(&tasks_path).unwrap();
        assert_eq!(loaded.tasks.len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test: Logika auto-inicjalizacji — gdy plik istnieje, użyj load()
    #[test]
    fn test_auto_init_logic_file_exists() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("ralph_test_add_auto_init_existing");
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

        // Symulujemy logikę z execute(): plik istnieje, więc używamy load()
        assert!(tasks_path.exists());
        let before = TasksFile::load(&tasks_path).unwrap();

        // Verify loaded correct content
        assert_eq!(before.tasks.len(), 1);
        assert_eq!(before.default_model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(before.flatten_leaves().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test: Logika before_count — pusty plik daje 0
    #[test]
    fn test_before_count_empty() {
        let dir = std::env::temp_dir().join("ralph_test_add_before_count_empty");
        let _ = std::fs::remove_dir_all(&dir);

        let tasks_path = dir.join("tasks.yml");
        let before = TasksFile::load_or_init(&tasks_path).unwrap();
        let before_count = before.flatten_leaves().len();

        assert_eq!(before_count, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test: Logika before_count — istniejący plik z taskami
    #[test]
    fn test_before_count_existing_tasks() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("ralph_test_add_before_count_existing");
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

    // --- build_profiles_section tests ---

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

        // Should contain the section header and instructions
        assert!(result.contains("## DOSTĘPNE PROFILE WERYFIKACJI"));
        assert!(result.contains("**backend**"));
        assert!(result.contains("Przypisywanie profili do tasków"));
    }

    #[test]
    fn test_build_profiles_section_not_in_prompt_when_empty() {
        // Verify that template placeholder is cleanly replaced when no profiles
        let prompt = templates::ADD_PROMPT_YAML
            .replace("{requirements}", "test")
            .replace("{profiles_section}", &build_profiles_section(&[]));

        // No profiles header should appear
        assert!(!prompt.contains("DOSTĘPNE PROFILE WERYFIKACJI"));
        assert!(!prompt.contains("Przypisywanie profili"));
    }
}
