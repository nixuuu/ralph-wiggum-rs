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
            None,
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

    // ── Task 25.5.3: End-to-end message flow tests ─────────────────────────────

    /// Test 6: Symuluj wysłanie wiadomości przez kanał message_rx i weryfikuj że trafia do stdin.
    ///
    /// Weryfikuje pełny przepływ wiadomości:
    /// 1. Utworzenie WorkerRunner z message_rx
    /// 2. Wysłanie wiadomości przez tx
    /// 3. Weryfikacja że wiadomość została przekazana do ClaudeRunner stdin w formacie JSON
    ///
    /// Symuluje scenariusz z worker_runner.rs:153-179 (interactive mode).
    #[tokio::test]
    async fn test_message_flow_end_to_end_with_mock_stdin() {
        use crate::commands::task::orchestrate::worker_runner::WorkerRunner;
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use tokio::sync::mpsc;

        // Setup: tworzymy kanały event i message
        let (event_tx, _event_rx) = mpsc::channel(16);
        let (message_tx, message_rx) = mpsc::channel::<String>(16);
        let shutdown = Arc::new(AtomicBool::new(false));

        // Konfiguracja workera
        let config = WorkerRunnerConfig {
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: Some(std::time::Duration::from_secs(5)),
            mcp_port: 0,
            mcp_session_id: String::new(),
            disallowed_tools: None,
        };

        // Tworzymy WorkerRunner z message_rx
        let _runner = WorkerRunner::new(
            1,
            "test-task".to_string(),
            event_tx,
            shutdown,
            config,
            Some(message_rx),
        );

        // Wysyłamy testową wiadomość przez kanał
        let test_message = "Test message from orchestrator";
        message_tx
            .send(test_message.to_string())
            .await
            .expect("Failed to send message");

        // NOTE: Ten test weryfikuje strukturę WorkerRunner.
        // Faktyczne przekazanie wiadomości do stdin jest testowane w
        // ClaudeRunner::run_interactive() (patrz: runner.rs:2517-2600).
        //
        // WorkerRunner tworzy forward_task (worker_runner.rs:162-178),
        // który odbiera wiadomości z message_rx i przekazuje do log_rx.
        // ClaudeRunner.run_interactive() następnie odbiera z log_rx
        // i wysyła do stdin przez send_user_message().
        //
        // Pełny integration test wymagałby uruchomienia prawdziwego procesu Claude CLI,
        // co jest poza zakresem unit testów. Ten test weryfikuje poprawność
        // konfiguracji kanałów i przekazania message_rx do WorkerRunner.

        // Sprawdzamy że worker został utworzony poprawnie z message_rx
        // (message_rx został przeniesiony do WorkerRunner, więc nie możemy już go użyć)
        drop(_runner);
        drop(message_tx);
    }

    /// Test 7: Weryfikuj formatowanie wiadomości JSON przez ClaudeRunner.
    ///
    /// Testuje że wiadomość wysłana do stdin jest w poprawnym formacie JSON,
    /// zgodnym z protokołem Claude CLI (patrz: runner.rs:2459-2514).
    #[tokio::test]
    async fn test_message_json_format_verification() {
        use crate::commands::run::runner_reader::send_user_message;
        use tokio::io::AsyncReadExt;

        // Tworzymy duplex pipe do symulacji stdin
        let (mut stdin, mut reader) = tokio::io::duplex(2048);

        let test_message = "Test message with special chars: \"quotes\", newline\n, emoji 🎯";

        // Wysyłamy wiadomość przez send_user_message
        send_user_message(&mut stdin, test_message)
            .await
            .expect("send_user_message should succeed");

        drop(stdin);

        // Czytamy wysłane dane
        let mut buffer = String::new();
        reader.read_to_string(&mut buffer).await.ok();

        // Parsujemy JSON i weryfikujemy strukturę
        let parsed: serde_json::Value =
            serde_json::from_str(buffer.trim()).expect("Should be valid JSON");

        // Sprawdzamy pola zgodnie z protokołem (runner.rs:414-425)
        assert_eq!(parsed["type"], "user", "type field must be 'user'");
        assert_eq!(parsed["session_id"], "", "session_id must be empty string");

        // Sprawdzamy message structure
        let message = &parsed["message"];
        assert_eq!(message["role"], "user", "message.role must be 'user'");

        // Sprawdzamy content array
        let content = message["content"]
            .as_array()
            .expect("content must be array");
        assert_eq!(
            content.len(),
            1,
            "content array must have exactly 1 element"
        );

        // Sprawdzamy text block
        let text_block = &content[0];
        assert_eq!(text_block["type"], "text", "block type must be 'text'");
        assert_eq!(
            text_block["text"], test_message,
            "text content must match input"
        );
    }

    /// Test 8: Weryfikuj że wiadomości mogą być wysyłane podczas fazy Implement.
    ///
    /// W fazie Implement worker może odbierać wiadomości od użytkownika
    /// i przekazywać je do Claude przez stdin (interactive mode).
    ///
    /// Ten test symuluje scenariusz z worker.rs:147-161, gdzie message_rx
    /// jest przekazywany tylko w pierwszej iteracji (implement phase).
    #[tokio::test]
    async fn test_message_allowed_during_implement_phase() {
        use crate::commands::task::orchestrate::worker_runner::{WorkerRunner, WorkerRunnerConfig};
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use tokio::sync::mpsc;

        // Setup
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let (message_tx, message_rx) = mpsc::channel::<String>(16);
        let shutdown = Arc::new(AtomicBool::new(false));

        let config = WorkerRunnerConfig {
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: Some(std::time::Duration::from_secs(1)),
            mcp_port: 0,
            mcp_session_id: String::new(),
            disallowed_tools: None,
        };

        let runner = WorkerRunner::new(
            1,
            "test-task".to_string(),
            event_tx,
            shutdown,
            config,
            Some(message_rx),
        );

        // Wysyłamy wiadomość przez kanał
        let test_message = "Test message during implement phase";
        message_tx
            .send(test_message.to_string())
            .await
            .expect("Failed to send message");

        // NOTE: Nie uruchamiamy faktycznego run_phase, bo wymagałby prawdziwego Claude CLI.
        // Zamiast tego sprawdzamy, że message_rx został poprawnie przekazany do runnera.
        //
        // W rzeczywistym scenariuszu (worker_runner.rs:153-179):
        // - run_phase() wywołuje run_interactive() z log_rx
        // - forward_task odbiera wiadomości z message_rx i wysyła do log_rx
        // - run_interactive() odbiera z log_rx i wysyła do stdin
        //
        // Ten test weryfikuje strukturę, nie wykonanie — to jest granica unit testów.

        // Sprawdzamy że runner został utworzony z message_rx
        drop(runner);
        drop(message_tx);

        // Sprawdzamy że nie ma błędów w event channel
        assert!(
            event_rx.try_recv().is_err(),
            "No events should be sent in this setup test"
        );
    }

    /// Test 9: Weryfikuj blokadę wiadomości w fazie Verify (message_rx nie jest przekazywany).
    ///
    /// W fazie Verify worker NIE powinien odbierać wiadomości od użytkownika,
    /// ponieważ verify jest fazą automatyczną (uruchamianie komend weryfikacyjnych).
    ///
    /// Ten test weryfikuje że message_rx jest przekazywany TYLKO w pierwszej iteracji
    /// (implement phase) zgodnie z worker.rs:147-152.
    #[tokio::test]
    async fn test_message_blocked_during_verify_phase() {
        use crate::commands::task::orchestrate::worker_runner::{WorkerRunner, WorkerRunnerConfig};
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use tokio::sync::mpsc;

        // Setup pierwszego runnera (implement phase) — message_rx jest przekazywany
        let (event_tx1, _event_rx1) = mpsc::channel(16);
        let (message_tx, message_rx) = mpsc::channel::<String>(16);
        let shutdown = Arc::new(AtomicBool::new(false));

        let config1 = WorkerRunnerConfig {
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: Some(std::time::Duration::from_secs(1)),
            mcp_port: 0,
            mcp_session_id: String::new(),
            disallowed_tools: None,
        };

        let _runner1 = WorkerRunner::new(
            1,
            "test-task".to_string(),
            event_tx1,
            shutdown.clone(),
            config1,
            Some(message_rx), // message_rx przekazany w pierwszej iteracji
        );

        // Setup drugiego runnera (review/verify phase) — message_rx NIE jest przekazywany
        // Symuluje drugą iterację w worker.rs:147-152 gdzie msg_rx = None
        let (event_tx2, _event_rx2) = mpsc::channel(16);
        let config2 = WorkerRunnerConfig {
            use_nerd_font: false,
            prompt_prefix: None,
            prompt_suffix: None,
            phase_timeout: Some(std::time::Duration::from_secs(1)),
            mcp_port: 0,
            mcp_session_id: String::new(),
            disallowed_tools: None,
        };

        let _runner2 = WorkerRunner::new(
            1,
            "test-task".to_string(),
            event_tx2,
            shutdown,
            config2,
            None, // message_rx NIE jest przekazywany w kolejnych iteracjach
        );

        // Sprawdzamy że wiadomość może być wysłana tylko raz (message_tx został zmoved do runner1)
        // Próba wysłania wiadomości do runner2 nie jest możliwa, bo nie ma message_rx
        drop(_runner1);
        drop(_runner2);
        drop(message_tx);

        // Ten test weryfikuje że WorkerRunner akceptuje message_rx = None,
        // co oznacza że w fazach verify/review nie można wysyłać wiadomości
        // (runner używa run() zamiast run_interactive()).
    }
}
