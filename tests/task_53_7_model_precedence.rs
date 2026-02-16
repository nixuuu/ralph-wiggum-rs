/// Test 53.7: Model precedence - .ralph.toml vs tasks.yml
///
/// Sprawdza która wartość default_model ma priorytet gdy:
/// - `.ralph.toml [task] default_model = "opus"`
/// - `tasks.yml default_model: sonnet`
///
/// Oczekiwane zachowanie:
/// 1. W orkiestratorze: tasks.yml > .ralph.toml [task.orchestrate]
/// 2. W `task add/edit`: .ralph.toml [task] (tasks.yml NIE jest używany)
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

/// Helper: Tworzy .ralph.toml z [task] model=opus
fn create_ralph_toml_with_task_model(dir: &Path, model: &str) {
    let toml_content = format!(
        r#"
[task]
default_model = "{model}"
"#
    );
    let toml_path = dir.join(".ralph.toml");
    let mut file = fs::File::create(&toml_path).unwrap();
    file.write_all(toml_content.as_bytes()).unwrap();
}

/// Helper: Tworzy .ralph.toml z [task.orchestrate] default_model
fn create_ralph_toml_with_orchestrate_model(dir: &Path, model: &str) {
    let toml_content = format!(
        r#"
[task.orchestrate]
default_model = "{model}"
"#
    );
    let toml_path = dir.join(".ralph.toml");
    let mut file = fs::File::create(&toml_path).unwrap();
    file.write_all(toml_content.as_bytes()).unwrap();
}

/// Helper: Tworzy tasks.yml z default_model
fn create_tasks_yml_with_model(dir: &Path, model: &str) {
    let yaml_content = format!(
        r#"default_model: {model}

tasks:
  - id: "1"
    name: "Test task"
    component: test
    status: todo
"#
    );
    let tasks_path = dir.join(".ralph").join("tasks.yml");
    fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();
    let mut file = fs::File::create(&tasks_path).unwrap();
    file.write_all(yaml_content.as_bytes()).unwrap();
}

#[test]
fn test_orchestrate_model_precedence_tasks_yml_wins() {
    // W orkiestratorze tasks.yml wygrywa nad .ralph.toml [task.orchestrate]
    let dir = TempDir::new().unwrap();

    create_ralph_toml_with_orchestrate_model(dir.path(), "claude-opus-4-6");
    create_tasks_yml_with_model(dir.path(), "claude-sonnet-4-5-20250929");

    let file_config = ralph_wiggum::shared::file_config::FileConfig::load_from_path(
        &dir.path().join(".ralph.toml"),
    )
    .unwrap();
    let tasks_file =
        ralph_wiggum::shared::tasks::TasksFile::load(&dir.path().join(".ralph").join("tasks.yml"))
            .unwrap();

    assert_eq!(
        file_config.task.orchestrate.default_model.as_deref(),
        Some("claude-opus-4-6"),
        ".ralph.toml [task.orchestrate] should have opus"
    );
    assert_eq!(
        tasks_file.default_model.as_deref(),
        Some("claude-sonnet-4-5-20250929"),
        "tasks.yml should have sonnet"
    );

    // Symulacja logiki orchestratora (orchestrator.rs:298-304):
    //   models.get(task_id).or(tasks_file.default_model).or(config.model)
    let models = tasks_file.models_map();
    let orchestrate_model = file_config.task.orchestrate.default_model.as_ref();

    let resolved_model = models
        .get("1")
        .map(String::as_str)
        .or(tasks_file.default_model.as_deref())
        .or(orchestrate_model.map(String::as_str));

    assert_eq!(
        resolved_model,
        Some("claude-sonnet-4-5-20250929"),
        "In orchestrator, tasks.yml default_model should take precedence over .ralph.toml [task.orchestrate]"
    );
}

#[test]
fn test_task_add_model_precedence_ralph_toml_task_wins() {
    // W `task add` używany jest .ralph.toml [task] default_model (tasks.yml NIE jest źródłem)
    let dir = TempDir::new().unwrap();

    create_ralph_toml_with_task_model(dir.path(), "claude-opus-4-6");
    create_tasks_yml_with_model(dir.path(), "claude-sonnet-4-5-20250929");

    let file_config = ralph_wiggum::shared::file_config::FileConfig::load_from_path(
        &dir.path().join(".ralph.toml"),
    )
    .unwrap();

    // Symulacja logiki add.rs:42-44:
    //   args.model.or_else(|| file_config.task.default_model.clone())
    let args_model: Option<String> = None; // User nie podał --model
    let resolved_model = args_model.or_else(|| file_config.task.default_model.clone());

    assert_eq!(
        resolved_model.as_deref(),
        Some("claude-opus-4-6"),
        "In `task add`, .ralph.toml [task] default_model should be used (NOT tasks.yml)"
    );
}

#[test]
fn test_orchestrate_model_fallback_chain() {
    // Pełny łańcuch precedencji w orkiestratorze:
    // task.model > tasks.yml default_model > .ralph.toml [task.orchestrate] default_model
    let dir = TempDir::new().unwrap();

    create_ralph_toml_with_orchestrate_model(dir.path(), "claude-opus-4-6");

    // tasks.yml z default_model + task 2 z własnym model override
    let yaml_content = r#"default_model: claude-sonnet-4-5-20250929

tasks:
  - id: "1"
    name: "Task without model"
    component: test
    status: todo
  - id: "2"
    name: "Task with model override"
    component: test
    status: todo
    model: claude-haiku-4-5-20251001
"#;
    let tasks_path = dir.path().join(".ralph").join("tasks.yml");
    fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();
    fs::write(&tasks_path, yaml_content).unwrap();

    let file_config = ralph_wiggum::shared::file_config::FileConfig::load_from_path(
        &dir.path().join(".ralph.toml"),
    )
    .unwrap();
    let tasks_file = ralph_wiggum::shared::tasks::TasksFile::load(&tasks_path).unwrap();

    let models = tasks_file.models_map();
    let orchestrate_model = file_config.task.orchestrate.default_model.as_ref();

    // Task 1 (bez własnego modelu) → tasks.yml default_model (sonnet)
    let resolved_task1 = models
        .get("1")
        .map(String::as_str)
        .or(tasks_file.default_model.as_deref())
        .or(orchestrate_model.map(String::as_str));

    assert_eq!(
        resolved_task1,
        Some("claude-sonnet-4-5-20250929"),
        "Task 1: should use tasks.yml default_model"
    );

    // Task 2 (z własnym modelem) → task-specific model (haiku)
    let resolved_task2 = models
        .get("2")
        .map(String::as_str)
        .or(tasks_file.default_model.as_deref())
        .or(orchestrate_model.map(String::as_str));

    assert_eq!(
        resolved_task2,
        Some("claude-haiku-4-5-20251001"),
        "Task 2: should use task-specific model override"
    );
}

#[test]
fn test_orchestrate_model_fallback_to_ralph_toml_when_tasks_yml_empty() {
    // Gdy tasks.yml NIE ma default_model, fallback do .ralph.toml [task.orchestrate]
    let dir = TempDir::new().unwrap();

    create_ralph_toml_with_orchestrate_model(dir.path(), "claude-opus-4-6");

    // tasks.yml BEZ default_model
    let yaml_content = r#"
tasks:
  - id: "1"
    name: "Test task"
    component: test
    status: todo
"#;
    let tasks_path = dir.path().join(".ralph").join("tasks.yml");
    fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();
    fs::write(&tasks_path, yaml_content).unwrap();

    let file_config = ralph_wiggum::shared::file_config::FileConfig::load_from_path(
        &dir.path().join(".ralph.toml"),
    )
    .unwrap();
    let tasks_file = ralph_wiggum::shared::tasks::TasksFile::load(&tasks_path).unwrap();

    assert_eq!(
        tasks_file.default_model, None,
        "tasks.yml should have no default_model"
    );

    let models = tasks_file.models_map();
    let orchestrate_model = file_config.task.orchestrate.default_model.as_ref();

    let resolved_model = models
        .get("1")
        .map(String::as_str)
        .or(tasks_file.default_model.as_deref())
        .or(orchestrate_model.map(String::as_str));

    assert_eq!(
        resolved_model,
        Some("claude-opus-4-6"),
        "When tasks.yml has no default_model, should fall back to .ralph.toml [task.orchestrate]"
    );
}
