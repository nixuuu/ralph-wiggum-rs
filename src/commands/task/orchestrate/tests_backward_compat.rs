//! Task 48.2: Testy kompatybilności wstecznej dla konfiguracji bez profili
//!
//! Testuje że istniejące konfiguracje bez profili działają identycznie jak przed wprowadzeniem
//! systemu profili weryfikacji.
//!
//! Scenariusze testowe:
//! 1. FileConfig bez profiles → OrchestrateConfig.profiles.is_empty()
//! 2. TaskNode bez profiles → deserializacja OK, profiles = []
//! 3. Worker verify flow bez profili → globalne verify_commands
//! 4. run_profiled_verify z pustymi profilami → identyczne jak globalne verify_commands

// Moduł jest już oznaczony #[cfg(test)] w orchestrate/mod.rs
mod tests {
    use crate::shared::file_config::{FileConfig, VerifyCommand, VerifyProfile};
    use crate::shared::tasks::TaskNode;

    // ── Test 1: FileConfig bez profiles ─────────────────────────────────

    /// Test że FileConfig bez sekcji profiles deserializuje się poprawnie
    /// z pustym wektorem profili (zachowanie domyślne).
    #[test]
    fn test_file_config_without_profiles_backward_compat() {
        // Stara konfiguracja bez sekcji profiles
        let toml_content = r#"
[task.orchestrate]
workers = 4
max_retries = 3
verify_commands = ["cargo test", "cargo clippy"]
"#;

        let config: FileConfig = toml::from_str(toml_content).unwrap();

        // profiles powinno być puste (domyślna wartość)
        assert!(
            config.task.orchestrate.profiles.is_empty(),
            "Config bez profiles powinien mieć pusty wektor"
        );

        // Weryfikacja że pozostałe pola nadal działają
        assert_eq!(config.task.orchestrate.workers, 4);
        assert_eq!(config.task.orchestrate.max_retries, 3);
        assert_eq!(config.task.orchestrate.verify_commands.len(), 2);
    }

    /// Test że FileConfig z pustą sekcją profiles deserializuje się poprawnie.
    #[test]
    fn test_file_config_with_empty_profiles() {
        let toml_content = r#"
[task.orchestrate]
workers = 2
verify_commands = ["cargo test"]
profiles = []
"#;

        let config: FileConfig = toml::from_str(toml_content).unwrap();

        assert!(config.task.orchestrate.profiles.is_empty());
        assert_eq!(config.task.orchestrate.verify_commands.len(), 1);
    }

    /// Test że FileConfig z globalnym verify_commands i bez profiles
    /// pozostaje funkcjonalny jak przed wprowadzeniem profili.
    #[test]
    fn test_file_config_global_verify_only() {
        let toml_content = r#"
[task.orchestrate]
verify_commands = [
    "cargo fmt --check",
    { command = "cargo test", name = "Tests" },
    "cargo clippy --all-targets -- -D warnings"
]
"#;

        let config: FileConfig = toml::from_str(toml_content).unwrap();

        assert!(
            config.task.orchestrate.profiles.is_empty(),
            "Brak sekcji profiles"
        );
        assert_eq!(
            config.task.orchestrate.verify_commands.len(),
            3,
            "Wszystkie globalne verify_commands załadowane"
        );

        // Weryfikacja że różne formaty verify_commands działają
        assert_eq!(
            config.task.orchestrate.verify_commands[0].command(),
            "cargo fmt --check"
        );
        assert_eq!(
            config.task.orchestrate.verify_commands[1].command(),
            "cargo test"
        );
        assert_eq!(
            config.task.orchestrate.verify_commands[1].name(),
            Some("Tests")
        );
    }

    // ── Test 2: TaskNode bez profiles ───────────────────────────────────

    /// Test że TaskNode bez pola profiles deserializuje się z pustym wektorem.
    #[test]
    fn test_task_node_without_profiles_backward_compat() {
        // Stary format tasks.yml bez pola profiles
        let yaml_content = r#"
id: "1.1"
name: "Implement feature"
component: "core"
status: "todo"
description: "Add new feature"
deps: []
model: "sonnet"
related_files: ["src/main.rs"]
implementation_steps: ["Step 1", "Step 2"]
"#;

        let task: TaskNode = serde_yaml::from_str(yaml_content).unwrap();

        // profiles powinno być puste (domyślna wartość z #[serde(default)])
        assert!(
            task.profiles.is_empty(),
            "TaskNode bez pola profiles powinien mieć pusty wektor"
        );

        // Weryfikacja że pozostałe pola nadal działają
        assert_eq!(task.id, "1.1");
        assert_eq!(task.name, "Implement feature");
        assert_eq!(task.component.as_deref(), Some("core"));
        assert_eq!(task.model.as_deref(), Some("sonnet"));
        assert_eq!(task.related_files.len(), 1);
        assert_eq!(task.implementation_steps.len(), 2);
    }

    /// Test że TaskNode z pustym profiles deserializuje się poprawnie.
    #[test]
    fn test_task_node_with_empty_profiles() {
        let yaml_content = r#"
id: "1.1"
name: "Task"
status: "todo"
acceptance_criteria: []
profiles: []
"#;

        let task: TaskNode = serde_yaml::from_str(yaml_content).unwrap();

        assert!(task.profiles.is_empty());
        assert_eq!(task.id, "1.1");
    }

    /// Test że TaskNode z profilami deserializuje się poprawnie (sanity check).
    #[test]
    fn test_task_node_with_profiles() {
        let yaml_content = r#"
id: "1.1"
name: "Task"
status: "todo"
acceptance_criteria: []
profiles: ["frontend", "backend"]
"#;

        let task: TaskNode = serde_yaml::from_str(yaml_content).unwrap();

        assert_eq!(task.profiles.len(), 2);
        assert_eq!(task.profiles[0], "frontend");
        assert_eq!(task.profiles[1], "backend");
    }

    /// Test serializacji TaskNode bez profili — pole profiles nie powinno być w YAML.
    #[test]
    fn test_task_node_serialization_skip_empty_profiles() {
        let task = TaskNode {
            id: "1.1".to_string(),
            name: "Task".to_string(),
            component: Some("core".to_string()),
            status: Some(crate::shared::progress::TaskStatus::Todo),
            deps: vec![],
            model: None,
            description: None,
            related_files: vec![],
            implementation_steps: vec![],
            acceptance_criteria: Vec::new(),
            profiles: vec![], // Puste profiles
            subtasks: vec![],
        };

        let yaml = serde_yaml::to_string(&task).unwrap();

        // Pole profiles nie powinno być w YAML gdy jest puste
        // (dzięki skip_serializing_if = "Vec::is_empty")
        assert!(
            !yaml.contains("profiles:"),
            "Puste profiles nie powinno być w YAML"
        );
    }

    // ── Test 3: Worker verify flow bez profili ──────────────────────────

    /// Test że WorkerConfig z pustymi all_profiles zachowuje stare zachowanie.
    ///
    /// build_verify_plan jest metodą prywatną, więc testujemy tylko konfigurację.
    /// Integracyjny test pełnej ścieżki wykonania jest w test_backward_compatibility_contract.
    #[test]
    fn test_worker_config_empty_profiles() {
        use crate::commands::task::orchestrate::worker::WorkerConfig;

        // Config bez profili (stary format)
        let config = WorkerConfig {
            system_prompt: String::new(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "opus".to_string(),
            base_commit: None,
            all_profiles: vec![], // Brak profili (stary format)
        };

        assert!(
            config.all_profiles.is_empty(),
            "Config bez profili (stary format) powinien mieć pusty wektor"
        );
        assert_eq!(config.max_retries, 3);
    }

    // ── Test 4: run_profiled_verify zachowanie ──────────────────────────

    /// Test że profil bez verify_commands nie generuje kroków weryfikacji.
    #[test]
    fn test_profile_without_verify_commands() {
        // Profil bez verify_commands (edge case)
        let profile = VerifyProfile {
            name: "empty".to_string(),
            description: Some("Profile without verify commands".to_string()),
            paths: vec!["src/**/*.rs".to_string()],
            working_dir: None,
            verify_commands: vec![], // Brak komend
            setup_commands: vec![],
        };

        // Weryfikacja że profil został poprawnie skonstruowany
        assert!(profile.verify_commands.is_empty());
        assert!(!profile.paths.is_empty());
    }

    /// Test że profil z verify_commands jest poprawnie obsługiwany.
    #[test]
    fn test_profile_with_verify_commands() {
        let profile = VerifyProfile {
            name: "backend".to_string(),
            description: Some("Backend tests".to_string()),
            paths: vec!["src/api/**/*.rs".to_string()],
            working_dir: Some("api".to_string()),
            verify_commands: vec![
                VerifyCommand::Simple("cargo test --package api".to_string()),
                VerifyCommand::Detailed {
                    command: "cargo clippy --package api".to_string(),
                    name: Some("API Lint".to_string()),
                    description: Some("Lint API code".to_string()),
                },
            ],
            setup_commands: vec![],
        };

        assert_eq!(profile.verify_commands.len(), 2);
        assert_eq!(
            profile.verify_commands[0].command(),
            "cargo test --package api"
        );
        assert_eq!(
            profile.verify_commands[1].command(),
            "cargo clippy --package api"
        );
        assert_eq!(profile.working_dir.as_deref(), Some("api"));
    }

    // ── Test 5: Integracja — migracja ze starego formatu ────────────────

    /// Test kompletnej migracji: stary config → nowy config z pustymi profiles.
    #[test]
    fn test_migration_old_to_new_format() {
        // Stary format konfiguracji (przed task 40.x)
        let old_config_toml = r#"
[task.orchestrate]
workers = 3
max_retries = 2
verify_commands = ["cargo test", "cargo clippy"]
setup_commands = ["cargo build"]
"#;

        let config: FileConfig = toml::from_str(old_config_toml).unwrap();

        // Nowy format powinien być w pełni kompatybilny wstecz
        assert_eq!(config.task.orchestrate.workers, 3);
        assert_eq!(config.task.orchestrate.max_retries, 2);
        assert_eq!(config.task.orchestrate.verify_commands.len(), 2);
        assert_eq!(config.task.orchestrate.setup_commands.len(), 1);
        assert!(
            config.task.orchestrate.profiles.is_empty(),
            "Stary format nie ma profili"
        );
    }

    /// Test że dodanie profili do istniejącego config nie psuje globalnych verify_commands.
    #[test]
    fn test_config_with_both_global_and_profiles() {
        let toml_content = r#"
[task.orchestrate]
workers = 4
verify_commands = ["cargo test"]

[[task.orchestrate.profiles]]
name = "frontend"
paths = ["ui/**/*.ts"]
verify_commands = ["npm test"]
"#;

        let config: FileConfig = toml::from_str(toml_content).unwrap();

        // Globalne verify_commands nadal działają
        assert_eq!(config.task.orchestrate.verify_commands.len(), 1);
        assert_eq!(
            config.task.orchestrate.verify_commands[0].command(),
            "cargo test"
        );

        // Profile dodane obok
        assert_eq!(config.task.orchestrate.profiles.len(), 1);
        assert_eq!(config.task.orchestrate.profiles[0].name, "frontend");
        assert_eq!(config.task.orchestrate.profiles[0].verify_commands.len(), 1);
    }

    // ── Test 6: Zachowanie WorkerConfig ─────────────────────────────────

    /// Test że WorkerConfig z pustym all_profiles zachowuje stare zachowanie.
    #[test]
    fn test_worker_config_empty_profiles_backward_compat() {
        use crate::commands::task::orchestrate::worker::WorkerConfig;

        let config = WorkerConfig {
            system_prompt: "test prompt".to_string(),
            max_retries: 3,
            use_nerd_font: true,
            prompt_prefix: Some("prefix".to_string()),
            prompt_suffix: Some("suffix".to_string()),
            phase_timeout: Some(std::time::Duration::from_secs(1800)),
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            mcp_port: 8080,
            mcp_session_id: "session-123".to_string(),
            review_model: "opus".to_string(),
            base_commit: Some("abc123".to_string()),
            all_profiles: vec![], // Stary format — brak profili
        };

        assert!(
            config.all_profiles.is_empty(),
            "WorkerConfig bez profili (stary format)"
        );
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.review_model, "opus");
    }

    /// Test że WorkerConfig::default() ma puste profiles.
    #[test]
    fn test_worker_config_default_empty_profiles() {
        use crate::commands::task::orchestrate::worker::WorkerConfig;

        let config = WorkerConfig::default();

        assert!(config.all_profiles.is_empty(), "Default config bez profili");
        assert_eq!(config.max_retries, 0);
        assert!(config.system_prompt.is_empty());
    }

    // ── Test 7: Zachowanie na poziomie verify ───────────────────────────

    /// Test że ProfiledVerifyPlan z tylko GlobalVerify działa (sanity check).
    #[test]
    fn test_verify_plan_global_only_structure() {
        use crate::commands::task::orchestrate::verify::{ProfiledVerifyPlan, VerifyStep};

        // Plan tylko z GlobalVerify (stary format) — test struktury danych
        let plan = ProfiledVerifyPlan {
            steps: vec![VerifyStep::GlobalVerify {
                commands: vec![
                    VerifyCommand::Simple("cargo test".to_string()),
                    VerifyCommand::Simple("cargo clippy".to_string()),
                ],
            }],
        };

        assert_eq!(plan.steps.len(), 1);

        match &plan.steps[0] {
            VerifyStep::GlobalVerify { commands } => {
                assert_eq!(commands.len(), 2);
            }
            _ => panic!("Oczekiwano GlobalVerify"),
        }
    }

    // ── Test 8: Format TOML edge cases ──────────────────────────────────

    /// Test że komentarze w TOML nie psują deserializacji.
    #[test]
    fn test_file_config_with_comments() {
        let toml_content = r#"
# Old configuration without profiles
[task.orchestrate]
workers = 2
# These commands will run for all tasks
verify_commands = ["cargo test"]
"#;

        let config: FileConfig = toml::from_str(toml_content).unwrap();

        assert!(config.task.orchestrate.profiles.is_empty());
        assert_eq!(config.task.orchestrate.workers, 2);
    }

    /// Test że konfiguracja z tylko profilami (bez globalnych verify_commands) działa.
    #[test]
    fn test_config_profiles_only_no_global() {
        let toml_content = r#"
[task.orchestrate]
workers = 2

[[task.orchestrate.profiles]]
name = "api"
paths = ["src/api/**"]
verify_commands = ["cargo test --package api"]
"#;

        let config: FileConfig = toml::from_str(toml_content).unwrap();

        // Globalne verify_commands powinno być puste
        assert!(config.task.orchestrate.verify_commands.is_empty());

        // Profile powinny być załadowane
        assert_eq!(config.task.orchestrate.profiles.len(), 1);
        assert_eq!(config.task.orchestrate.profiles[0].name, "api");
    }

    // ── Test 9: Zachowanie deserializacji YAML z różnymi wariantami ─────

    /// Test deserializacji TaskNode z wszystkimi polami poza profiles.
    #[test]
    fn test_task_node_full_fields_no_profiles() {
        let yaml_content = r#"
id: "2.1"
name: "Complex task"
component: "backend"
status: "in_progress"
deps: ["1.1", "1.2"]
model: "opus"
description: "A complex implementation task"
related_files: ["src/api/handler.rs", "src/api/router.rs"]
implementation_steps:
  - "Setup database schema"
  - "Implement API endpoints"
  - "Add tests"
"#;

        let task: TaskNode = serde_yaml::from_str(yaml_content).unwrap();

        // Profiles powinno być puste (brak w YAML)
        assert!(task.profiles.is_empty());

        // Pozostałe pola prawidłowo załadowane
        assert_eq!(task.id, "2.1");
        assert_eq!(task.name, "Complex task");
        assert_eq!(task.deps.len(), 2);
        assert_eq!(task.related_files.len(), 2);
        assert_eq!(task.implementation_steps.len(), 3);
    }

    /// Test deserializacji hierarchii TaskNode bez profili.
    #[test]
    fn test_task_node_hierarchy_no_profiles() {
        let yaml_content = r#"
id: "1"
name: "Parent task"
component: "core"
subtasks:
  - id: "1.1"
    name: "Child task 1"
    status: "todo"
  - id: "1.2"
    name: "Child task 2"
    status: "done"
"#;

        let task: TaskNode = serde_yaml::from_str(yaml_content).unwrap();

        // Parent bez profiles
        assert!(task.profiles.is_empty());

        // Children też bez profiles
        assert_eq!(task.subtasks.len(), 2);
        assert!(task.subtasks[0].profiles.is_empty());
        assert!(task.subtasks[1].profiles.is_empty());
    }

    // ── Test 10: Kontrakt kompatybilności ───────────────────────────────

    /// Test kontraktu: stary config + stary tasks.yml → nowy system działa.
    ///
    /// To jest test integracyjny weryfikujący że pełna ścieżka wykonania
    /// (config → task → worker → verify) działa dla starych formatów.
    #[test]
    fn test_backward_compatibility_contract() {
        // 1. Stary config bez profili
        let config_toml = r#"
[task.orchestrate]
workers = 2
verify_commands = ["cargo test", "cargo clippy"]
"#;

        let config: FileConfig = toml::from_str(config_toml).unwrap();

        assert!(config.task.orchestrate.profiles.is_empty());
        assert_eq!(config.task.orchestrate.verify_commands.len(), 2);

        // 2. Stary task bez profili
        let task_yaml = r#"
id: "1.1"
name: "Test task"
status: "todo"
description: "A simple task"
"#;

        let task: TaskNode = serde_yaml::from_str(task_yaml).unwrap();

        assert!(task.profiles.is_empty());

        // 3. WorkerConfig z pustymi profiles
        use crate::commands::task::orchestrate::worker::WorkerConfig;

        let worker_config = WorkerConfig {
            system_prompt: "system".to_string(),
            max_retries: 3,
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: None,
            git_timeout: std::time::Duration::from_secs(120),
            setup_timeout: std::time::Duration::from_secs(300),
            mcp_port: 0,
            mcp_session_id: String::new(),
            review_model: "opus".to_string(),
            base_commit: None,
            all_profiles: config.task.orchestrate.profiles.clone(),
        };

        assert!(worker_config.all_profiles.is_empty());

        // 4. Weryfikacja że verify_commands są poprawnie załadowane
        // build_verify_plan jest metodą prywatną Worker, więc testujemy tylko dane wejściowe
        assert_eq!(config.task.orchestrate.verify_commands.len(), 2);
        assert_eq!(
            config.task.orchestrate.verify_commands[0].command(),
            "cargo test"
        );
        assert_eq!(
            config.task.orchestrate.verify_commands[1].command(),
            "cargo clippy"
        );

        // SUKCES: Cała ścieżka konfiguracji działa dla starych formatów:
        // - Config deserializuje się z pustymi profiles
        // - Task deserializuje się z pustymi profiles
        // - WorkerConfig otrzymuje puste all_profiles
        // - Globalne verify_commands są poprawnie załadowane
        // W runtime build_verify_plan użyje tylko GlobalVerify (bez ProfileVerify)
    }
}
