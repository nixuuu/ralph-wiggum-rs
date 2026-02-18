//! Testy kompatybilności wstecznej dla konfiguracji bez profili.
//!
//! Weryfikuje, że istniejące konfiguracje bez sekcji `profiles` działają
//! identycznie jak przed wprowadzeniem funkcjonalności profili weryfikacji.
//!
//! Kluczowe scenariusze:
//! 1. FileConfig bez profiles → OrchestrateConfig.profiles.is_empty()
//! 2. TaskNode bez profiles → deserializacja OK, puste Vec<String>
//! 3. run_profiled_verify z pustymi profilami → tylko GlobalVerify

use crate::shared::file_config::{FileConfig, OrchestrateConfig, VerifyCommand};
use crate::shared::tasks::TaskNode;

// ── Test 1: FileConfig bez profiles ────────────────────────────────────

#[test]
fn test_file_config_without_profiles_section() {
    // Istniejąca konfiguracja bez [task.orchestrate.profiles]
    let toml_content = r#"
[task.orchestrate]
workers = 4
verify_commands = ["cargo test", "cargo clippy"]
"#;

    let config: FileConfig = toml::from_str(toml_content).unwrap();

    // profiles powinno być puste (backward compat)
    assert!(
        config.task.orchestrate.profiles.is_empty(),
        "Config without profiles section should have empty profiles vec"
    );

    // verify_commands nadal działają
    assert_eq!(config.task.orchestrate.verify_commands.len(), 2);
    assert_eq!(
        config.task.orchestrate.verify_commands[0].command(),
        "cargo test"
    );
}

#[test]
fn test_file_config_with_verify_commands_no_profiles() {
    // Stara konfiguracja: globalne verify_commands, brak profili
    let toml_content = r#"
[task.orchestrate]
verify_commands = [
    { command = "cargo test", name = "Tests" },
    "cargo fmt --check"
]
"#;

    let config: FileConfig = toml::from_str(toml_content).unwrap();

    // profiles jest puste
    assert!(config.task.orchestrate.profiles.is_empty());

    // verify_commands zostają zachowane
    assert_eq!(config.task.orchestrate.verify_commands.len(), 2);
    assert_eq!(
        config.task.orchestrate.verify_commands[0].name(),
        Some("Tests")
    );
}

#[test]
fn test_orchestrate_config_default_profiles_empty() {
    let config = OrchestrateConfig::default();
    assert!(
        config.profiles.is_empty(),
        "Default OrchestrateConfig should have empty profiles"
    );
}

#[test]
fn test_file_config_empty_toml_has_empty_profiles() {
    // Pusta konfiguracja
    let config: FileConfig = toml::from_str("").unwrap();
    assert!(config.task.orchestrate.profiles.is_empty());
}

#[test]
fn test_file_config_partial_orchestrate_no_profiles() {
    // Częściowa konfiguracja orchestrate bez profili
    let toml_content = r#"
[task.orchestrate]
workers = 2
max_retries = 3
"#;

    let config: FileConfig = toml::from_str(toml_content).unwrap();
    assert!(config.task.orchestrate.profiles.is_empty());
    assert_eq!(config.task.orchestrate.workers, 2);
    assert_eq!(config.task.orchestrate.max_retries, 3);
}

// ── Test 2: TaskNode bez profiles ──────────────────────────────────────

#[test]
fn test_task_node_without_profiles_field() {
    // YAML task bez pola `profiles`
    let yaml_content = r#"
id: "1"
name: "Task 1"
component: "backend"
status: todo
"#;

    let node: TaskNode = serde_yaml::from_str(yaml_content).unwrap();

    // profiles powinno być puste (backward compat)
    assert!(
        node.profiles.is_empty(),
        "TaskNode without profiles field should have empty profiles vec"
    );
    assert_eq!(node.id, "1");
    assert_eq!(node.name, "Task 1");
}

#[test]
fn test_task_node_with_empty_profiles() {
    // YAML task z pustym profiles: []
    let yaml_content = r#"
id: "2"
name: "Task 2"
acceptance_criteria: []
profiles: []
"#;

    let node: TaskNode = serde_yaml::from_str(yaml_content).unwrap();

    assert!(node.profiles.is_empty());
}

#[test]
fn test_task_node_profiles_field_omitted_in_serialization() {
    // TaskNode z pustymi profilami nie powinien serializować pola profiles
    let node = TaskNode {
        id: "3".to_string(),
        name: "Task 3".to_string(),
        component: Some("tests".to_string()),
        status: Some(crate::shared::progress::TaskStatus::InProgress),
        deps: vec![],
        model: None,
        description: None,
        related_files: vec![],
        implementation_steps: vec![],
        acceptance_criteria: Vec::new(),
        profiles: vec![], // Puste
        subtasks: vec![],
    };

    let yaml = serde_yaml::to_string(&node).unwrap();

    // Pole `profiles` nie powinno być w YAML (skip_serializing_if)
    assert!(
        !yaml.contains("profiles:"),
        "Empty profiles should be skipped in serialization"
    );
}

#[test]
fn test_task_node_deserialize_legacy_yaml() {
    // Istniejący plik tasks.yml bez profili
    let yaml_content = r#"
id: "1"
name: "Implement feature X"
component: "api"
status: todo
deps: []
related_files:
  - src/api/handler.rs
implementation_steps:
  - "Create endpoint"
  - "Add tests"
"#;

    let node: TaskNode = serde_yaml::from_str(yaml_content).unwrap();

    // Deserializacja powinna się udać
    assert_eq!(node.id, "1");
    assert_eq!(node.name, "Implement feature X");
    assert!(node.profiles.is_empty());
    assert_eq!(node.related_files.len(), 1);
    assert_eq!(node.implementation_steps.len(), 2);
}

// ── Test 3: run_profiled_verify z pustymi profilami ───────────────────

/// Test że run_profiled_verify z tylko GlobalVerify działa identycznie
/// jak stare run_verify_commands (backward compat).
///
/// Wywołuje run_profiled_verify bezpośrednio (bez tokio::spawn) — kanał z buforem
/// wystarczy do zebrania eventów bez deadlocka na single-thread runtime.
#[tokio::test]
async fn test_run_profiled_verify_backward_compat_global_only() {
    use crate::commands::task::orchestrate::verify::{ProfiledVerifyPlan, VerifyStep};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let temp = TempDir::new().unwrap();
    let worktree_path = temp.path();

    // Symulacja starej konfiguracji: tylko globalne verify_commands
    let plan = ProfiledVerifyPlan {
        steps: vec![VerifyStep::GlobalVerify {
            commands: vec![VerifyCommand::Simple("echo test".to_string())],
        }],
    };

    // Bufor 32 wystarczy na wszystkie eventy emitowane przez run_profiled_verify
    let (tx, mut rx) = mpsc::channel(32);

    let result = crate::commands::task::orchestrate::verify::run_profiled_verify(
        &plan,
        worktree_path,
        &tx,
        1,
    )
    .await;

    // Verify powinien się udać (echo test zawsze zwraca 0)
    assert!(
        result.success,
        "Backward compat: global verify should succeed"
    );
    assert_eq!(result.results.len(), 1);
    assert_eq!(result.results[0].command, "echo test");
    assert!(result.results[0].success);

    // profile_results powinno być puste (brak profili)
    assert!(
        result.profile_results.is_empty(),
        "Backward compat: no profiles should have empty profile_results"
    );

    // Eventy powinny być emitowane
    drop(tx); // zamknij sender żeby rx.recv() zwróciło None
    let mut event_count = 0;
    while rx.recv().await.is_some() {
        event_count += 1;
    }
    assert!(event_count > 0, "Should emit at least one event");
}

/// Test że plan z pustym krokami zwraca success=true (backward compat safety).
#[tokio::test]
async fn test_run_profiled_verify_empty_plan() {
    use crate::commands::task::orchestrate::verify::ProfiledVerifyPlan;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let temp = TempDir::new().unwrap();
    let worktree_path = temp.path();

    // Plan bez żadnych kroków (teoretycznie niemożliwy, ale test safety)
    let plan = ProfiledVerifyPlan { steps: vec![] };

    let (tx, _rx) = mpsc::channel(16);

    let result = crate::commands::task::orchestrate::verify::run_profiled_verify(
        &plan,
        worktree_path,
        &tx,
        1,
    )
    .await;

    // Powinno zwrócić success=true (brak kroków = brak błędów)
    assert!(result.success);
    assert!(result.results.is_empty());
    assert!(result.profile_results.is_empty());
}

// ── Test 4: Integracyjne testy backward compat ────────────────────────

/// Test że migracja z pustej konfiguracji do profili nie łamie backward compat.
#[test]
fn test_migration_from_no_profiles_to_profiles() {
    // Przed: brak profili
    let old_toml = r#"
[task.orchestrate]
verify_commands = ["cargo test"]
"#;
    let old_config: FileConfig = toml::from_str(old_toml).unwrap();
    assert!(old_config.task.orchestrate.profiles.is_empty());

    // Po: dodanie profili, ale verify_commands nadal działa
    let new_toml = r#"
[task.orchestrate]
verify_commands = ["cargo test"]

[[task.orchestrate.profiles]]
name = "backend"
paths = ["src/api/**"]
verify_commands = ["cargo test --package api"]
"#;
    let new_config: FileConfig = toml::from_str(new_toml).unwrap();
    assert_eq!(new_config.task.orchestrate.profiles.len(), 1);

    // Globalne verify_commands nadal istnieją
    assert_eq!(new_config.task.orchestrate.verify_commands.len(), 1);
    assert_eq!(
        new_config.task.orchestrate.verify_commands[0].command(),
        "cargo test"
    );
}

// ── Test 6: Dodatkowe edge cases ───────────────────────────────────────

#[test]
fn test_file_config_only_profiles_no_verify_commands() {
    // Odwrotny przypadek: profile istnieją, ale globalne verify_commands puste
    let toml_content = r#"
[[task.orchestrate.profiles]]
name = "test-profile"
verify_commands = ["echo hello"]
"#;

    let config: FileConfig = toml::from_str(toml_content).unwrap();
    assert_eq!(config.task.orchestrate.profiles.len(), 1);
    assert!(config.task.orchestrate.verify_commands.is_empty());
}

#[test]
fn test_task_node_legacy_fields_preserved() {
    // Test że dodanie `profiles` nie wpływa na inne pola
    let yaml_content = r#"
id: "5"
name: "Legacy task"
component: "core"
status: in_progress
deps: ["1", "2"]
model: "claude-opus-4-6"
description: "Old description"
related_files:
  - src/main.rs
implementation_steps:
  - "Step 1"
"#;

    let node: TaskNode = serde_yaml::from_str(yaml_content).unwrap();

    assert_eq!(node.id, "5");
    assert_eq!(node.name, "Legacy task");
    assert_eq!(node.component.as_deref(), Some("core"));
    assert_eq!(node.deps, vec!["1", "2"]);
    assert_eq!(node.model.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(node.description.as_deref(), Some("Old description"));
    assert_eq!(node.related_files, vec!["src/main.rs"]);
    assert_eq!(node.implementation_steps, vec!["Step 1"]);
    assert!(node.profiles.is_empty()); // Puste, bo nie podano
}
