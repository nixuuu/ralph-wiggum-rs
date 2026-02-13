//! Testy integracyjne dla konfiguracji workera z MCP i disallowed_tools.
//!
//! Ten moduł testuje pełny przepływ konfiguracji workera:
//! - WorkerRunnerConfig z mcp_port + disallowed_tools jednocześnie
//! - Spójność MCP_MUTATION_TOOLS z MUTATION_TOOLS w tools.rs
//! - Format stringa MCP_MUTATION_TOOLS

#[cfg(test)]
mod tests {
    use crate::commands::task::orchestrate::worker_runner::WorkerRunnerConfig;
    use crate::shared::mcp::MCP_MUTATION_TOOLS;

    /// Test 1: WorkerRunnerConfig z mcp_port>0 + disallowed_tools generuje poprawny runner config.
    ///
    /// Weryfikuje że konfiguracja workera może jednocześnie:
    /// - Łączyć się z MCP serverem (mcp_port > 0)
    /// - Blokować mutation tools (disallowed_tools zawiera MCP_MUTATION_TOOLS)
    ///
    /// To kluczowa funkcjonalność dla zadania 24 — workery mogą czytać zadania przez MCP
    /// ale nie mogą ich modyfikować (blokada mutation tools).
    #[test]
    fn test_worker_runner_config_with_mcp_and_disallowed_tools() {
        // Konfiguracja jak w Worker::execute_task() (linie 132-140)
        let config = WorkerRunnerConfig {
            use_nerd_font: false,
            prompt_prefix: Some("Test prefix".to_string()),
            prompt_suffix: Some("Test suffix".to_string()),
            phase_timeout: Some(std::time::Duration::from_secs(1800)),
            mcp_port: 8080,                           // MCP server włączony
            mcp_session_id: "worker-123".to_string(), // Sesja workera
            disallowed_tools: Some(MCP_MUTATION_TOOLS.to_string()), // Blokada mutation tools
        };

        // Sprawdź że wszystkie pola są poprawnie ustawione
        assert_eq!(config.mcp_port, 8080, "MCP port powinien być 8080");
        assert_eq!(
            config.mcp_session_id, "worker-123",
            "Session ID powinno być ustawione"
        );
        assert!(
            config.disallowed_tools.is_some(),
            "disallowed_tools powinno być ustawione"
        );

        let disallowed = config.disallowed_tools.unwrap();
        assert_eq!(
            disallowed, MCP_MUTATION_TOOLS,
            "disallowed_tools powinno zawierać MCP_MUTATION_TOOLS"
        );

        // Sprawdź że string zawiera oczekiwane narzędzia
        assert!(
            disallowed.contains("mcp__ralph-tasks__tasks_create"),
            "Powinno zawierać tasks_create"
        );
        assert!(
            disallowed.contains("mcp__ralph-tasks__tasks_update"),
            "Powinno zawierać tasks_update"
        );
        assert!(
            disallowed.contains("mcp__ralph-tasks__tasks_delete"),
            "Powinno zawierać tasks_delete"
        );
        assert!(
            disallowed.contains("mcp__ralph-tasks__tasks_move"),
            "Powinno zawierać tasks_move"
        );
        assert!(
            disallowed.contains("mcp__ralph-tasks__tasks_batch_status"),
            "Powinno zawierać tasks_batch_status"
        );
        assert!(
            disallowed.contains("mcp__ralph-tasks__tasks_set_deps"),
            "Powinno zawierać tasks_set_deps"
        );
        assert!(
            disallowed.contains("mcp__ralph-tasks__tasks_set_default_model"),
            "Powinno zawierać tasks_set_default_model"
        );
    }

    /// Test 2: Spójność MCP_MUTATION_TOOLS z MUTATION_TOOLS z tools.rs.
    ///
    /// Weryfikuje że każde narzędzie w MCP_MUTATION_TOOLS ma odpowiednik
    /// w MUTATION_TOOLS z src/commands/mcp/tools.rs (z prefixem mcp__ralph-tasks__).
    ///
    /// Format transformacji: `{short_name}` → `mcp__ralph-tasks__{short_name}`
    ///
    /// Przykład:
    /// - MUTATION_TOOLS: ["tasks_create", "tasks_update", ...]
    /// - MCP_MUTATION_TOOLS: "mcp__ralph-tasks__tasks_create,mcp__ralph-tasks__tasks_update,..."
    #[test]
    fn test_mcp_mutation_tools_consistency_with_tools_rs() {
        // Lista narzędzi mutation z src/commands/mcp/tools.rs (linie 10-18)
        // Musi być zsynchronizowana z MUTATION_TOOLS constant
        let expected_mutation_tools = vec![
            "tasks_create",
            "tasks_update",
            "tasks_delete",
            "tasks_move",
            "tasks_batch_status",
            "tasks_set_deps",
            "tasks_set_default_model",
        ];

        // Parsuj MCP_MUTATION_TOOLS (comma-separated)
        let mcp_tools: Vec<&str> = MCP_MUTATION_TOOLS.split(',').collect();

        // Sprawdź że liczba narzędzi się zgadza
        assert_eq!(
            mcp_tools.len(),
            expected_mutation_tools.len(),
            "Liczba narzędzi w MCP_MUTATION_TOOLS powinna być równa MUTATION_TOOLS"
        );

        // Sprawdź że każde narzędzie z MUTATION_TOOLS ma odpowiednik w MCP_MUTATION_TOOLS
        for short_name in &expected_mutation_tools {
            let full_name = format!("mcp__ralph-tasks__{}", short_name);
            assert!(
                mcp_tools.contains(&full_name.as_str()),
                "MCP_MUTATION_TOOLS powinno zawierać '{}' (full name: '{}')",
                short_name,
                full_name
            );
        }

        // Sprawdź że nie ma dodatkowych narzędzi (reverse check)
        for mcp_tool in &mcp_tools {
            // Wyciągnij short name z pełnej nazwy
            let short_name = mcp_tool
                .strip_prefix("mcp__ralph-tasks__")
                .unwrap_or_else(|| panic!("Niepoprawny prefix w '{}'", mcp_tool));

            assert!(
                expected_mutation_tools.contains(&short_name),
                "Nieoczekiwane narzędzie '{}' w MCP_MUTATION_TOOLS",
                mcp_tool
            );
        }
    }

    /// Test 3a: Format stringa MCP_MUTATION_TOOLS - brak spacji po przecinkach.
    ///
    /// Weryfikuje że MCP_MUTATION_TOOLS jest poprawnie sformatowanym
    /// comma-separated stringiem bez białych znaków (spacji, tabów).
    ///
    /// Format poprawny: "tool1,tool2,tool3"
    /// Format niepoprawny: "tool1, tool2 , tool3" (spacje po przecinkach)
    #[test]
    fn test_mcp_mutation_tools_format_no_whitespace() {
        // Parsuj narzędzia
        let tools: Vec<&str> = MCP_MUTATION_TOOLS.split(',').collect();

        // Sprawdź że każde narzędzie nie zawiera białych znaków (przed/po)
        for tool in tools {
            assert_eq!(
                tool,
                tool.trim(),
                "Narzędzie '{}' nie powinno zawierać białych znaków na początku/końcu",
                tool
            );

            // Sprawdź że wewnątrz nazwy też nie ma spacji
            assert!(
                !tool.contains(' '),
                "Narzędzie '{}' nie powinno zawierać spacji wewnątrz",
                tool
            );
            assert!(
                !tool.contains('\t'),
                "Narzędzie '{}' nie powinno zawierać tabów",
                tool
            );
            assert!(
                !tool.contains('\n'),
                "Narzędzie '{}' nie powinno zawierać newline",
                tool
            );
        }
    }

    /// Test 3b: Format stringa MCP_MUTATION_TOOLS - brak trailing comma.
    ///
    /// Weryfikuje że MCP_MUTATION_TOOLS nie kończy się przecinkiem.
    ///
    /// Format poprawny: "tool1,tool2,tool3"
    /// Format niepoprawny: "tool1,tool2,tool3," (trailing comma)
    #[test]
    fn test_mcp_mutation_tools_format_no_trailing_comma() {
        // Sprawdź że string nie kończy się przecinkiem
        assert!(
            !MCP_MUTATION_TOOLS.ends_with(','),
            "MCP_MUTATION_TOOLS nie powinno kończyć się przecinkiem"
        );

        // Sprawdź że po split nie ma pustych stringów na końcu
        let tools: Vec<&str> = MCP_MUTATION_TOOLS.split(',').collect();
        let last_tool = tools.last().expect("Lista narzędzi nie powinna być pusta");
        assert!(
            !last_tool.is_empty(),
            "Ostatnie narzędzie nie powinno być pustym stringiem (trailing comma)"
        );
    }

    /// Test 3c: Format stringa MCP_MUTATION_TOOLS - brak leading comma.
    ///
    /// Weryfikuje że MCP_MUTATION_TOOLS nie zaczyna się od przecinka.
    ///
    /// Format poprawny: "tool1,tool2,tool3"
    /// Format niepoprawny: ",tool1,tool2,tool3" (leading comma)
    #[test]
    fn test_mcp_mutation_tools_format_no_leading_comma() {
        // Sprawdź że string nie zaczyna się od przecinka
        assert!(
            !MCP_MUTATION_TOOLS.starts_with(','),
            "MCP_MUTATION_TOOLS nie powinno zaczynać się od przecinka"
        );

        // Sprawdź że po split nie ma pustych stringów na początku
        let tools: Vec<&str> = MCP_MUTATION_TOOLS.split(',').collect();
        let first_tool = tools.first().expect("Lista narzędzi nie powinna być pusta");
        assert!(
            !first_tool.is_empty(),
            "Pierwsze narzędzie nie powinno być pustym stringiem (leading comma)"
        );
    }

    /// Test 3d: Format stringa MCP_MUTATION_TOOLS - brak pustych elementów.
    ///
    /// Weryfikuje że MCP_MUTATION_TOOLS nie zawiera pustych elementów
    /// (np. przez podwójne przecinki: "tool1,,tool2").
    ///
    /// Format poprawny: "tool1,tool2,tool3"
    /// Format niepoprawny: "tool1,,tool2,tool3" (podwójny przecinek)
    #[test]
    fn test_mcp_mutation_tools_format_no_empty_elements() {
        // Parsuj narzędzia
        let tools: Vec<&str> = MCP_MUTATION_TOOLS.split(',').collect();

        // Sprawdź że nie ma pustych elementów
        for (idx, tool) in tools.iter().enumerate() {
            assert!(
                !tool.is_empty(),
                "Element {} nie powinien być pusty (podwójny przecinek?)",
                idx
            );
        }
    }

    /// Test 3e: Format stringa MCP_MUTATION_TOOLS - poprawna liczba narzędzi.
    ///
    /// Weryfikuje że MCP_MUTATION_TOOLS zawiera dokładnie 7 narzędzi
    /// (zgodnie z dokumentacją w src/shared/mcp.rs linie 4-17).
    #[test]
    fn test_mcp_mutation_tools_format_correct_count() {
        let tools: Vec<&str> = MCP_MUTATION_TOOLS.split(',').collect();
        assert_eq!(
            tools.len(),
            7,
            "MCP_MUTATION_TOOLS powinno zawierać dokładnie 7 narzędzi"
        );
    }

    /// Test 4: Pełny test integracyjny - symulacja użycia w Worker::execute_task().
    ///
    /// Weryfikuje pełny przepływ:
    /// 1. Stworzenie WorkerRunnerConfig z MCP + disallowed_tools
    /// 2. Sprawdzenie że konfiguracja jest spójna i poprawnie sformatowana
    /// 3. Symulacja przekazania do WorkerRunner (bez uruchomienia workera)
    #[test]
    fn test_full_integration_worker_runner_config() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use tokio::sync::mpsc;

        use crate::commands::task::orchestrate::worker_runner::WorkerRunner;

        // Konfiguracja jak w Worker::execute_task()
        let runner_config = WorkerRunnerConfig {
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: Some(std::time::Duration::from_secs(1800)),
            mcp_port: 8080,
            mcp_session_id: "integration-test-worker".to_string(),
            disallowed_tools: Some(MCP_MUTATION_TOOLS.to_string()),
        };

        // Sprawdź konfigurację
        assert_eq!(runner_config.mcp_port, 8080);
        assert_eq!(runner_config.mcp_session_id, "integration-test-worker");
        assert!(runner_config.disallowed_tools.is_some());

        let disallowed = runner_config.disallowed_tools.as_ref().unwrap();
        assert_eq!(disallowed, MCP_MUTATION_TOOLS);

        // Symulacja stworzenia WorkerRunner (bez faktycznego uruchomienia)
        let (tx, _rx) = mpsc::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));

        // Runner akceptuje konfigurację - nie musimy sprawdzać prywatnych pól
        let _runner = WorkerRunner::new(
            1,
            "test-task-id".to_string(),
            tx,
            shutdown,
            runner_config.clone(),
        );

        // Sprawdź logikę generowania MCP config (symulacja WorkerRunner::mcp_config())
        // Warunek z worker_runner.rs:74-76
        let should_have_mcp =
            runner_config.mcp_port > 0 && !runner_config.mcp_session_id.is_empty();
        assert!(
            should_have_mcp,
            "Runner powinien mieć MCP config przy mcp_port>0 i niepustym session_id"
        );

        // Sprawdź że konfiguracja została przekazana poprawnie (przez publiczne API)
        // WorkerRunner przechowuje config, więc jeśli stworzenie się udało, config został zaakceptowany
        assert_eq!(runner_config.mcp_port, 8080);
        assert_eq!(
            runner_config.disallowed_tools,
            Some(MCP_MUTATION_TOOLS.to_string())
        );
    }

    /// Test 5: Weryfikacja że format MCP_MUTATION_TOOLS jest kompatybilny z Claude CLI --disallowedTools.
    ///
    /// Claude CLI oczekuje comma-separated listy narzędzi bez spacji:
    /// --disallowedTools "tool1,tool2,tool3"
    ///
    /// Ten test weryfikuje że MCP_MUTATION_TOOLS spełnia wymagania CLI.
    #[test]
    fn test_mcp_mutation_tools_claude_cli_compatibility() {
        // Format musi być zgodny z Claude CLI --disallowedTools
        // Nie może zawierać:
        // - Spacji: "tool1, tool2" (niepoprawne)
        // - Trailing comma: "tool1,tool2," (niepoprawne)
        // - Leading comma: ",tool1,tool2" (niepoprawne)
        // - Pustych elementów: "tool1,,tool2" (niepoprawne)

        // Parsuj string
        let tools: Vec<&str> = MCP_MUTATION_TOOLS.split(',').collect();

        // Sprawdź że wszystkie narzędzia są poprawnie sformatowane
        for tool in &tools {
            // Nie może być puste
            assert!(!tool.is_empty(), "Narzędzie nie może być puste");

            // Nie może zawierać białych znaków
            assert_eq!(
                *tool,
                tool.trim(),
                "Narzędzie '{}' nie może zawierać białych znaków",
                tool
            );

            // Musi zaczynać się od prefiksu
            assert!(
                tool.starts_with("mcp__ralph-tasks__"),
                "Narzędzie '{}' musi zaczynać się od 'mcp__ralph-tasks__'",
                tool
            );
        }

        // Sprawdź że string można bezpośrednio użyć w CLI
        // (symulacja: --disallowedTools "{MCP_MUTATION_TOOLS}")
        assert!(
            !MCP_MUTATION_TOOLS.is_empty(),
            "MCP_MUTATION_TOOLS nie może być pustym stringiem"
        );

        // Sprawdź że liczba przecinków = liczba narzędzi - 1
        let comma_count = MCP_MUTATION_TOOLS.matches(',').count();
        assert_eq!(
            comma_count,
            tools.len() - 1,
            "Liczba przecinków powinna być równa liczbie narzędzi - 1"
        );
    }
}
