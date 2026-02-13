use serde_json::{Value, json};
use urlencoding::encode;

/// Lista pełnych nazw MCP mutation tools w formacie Claude CLI.
///
/// Format: comma-separated lista 7 narzędzi MCP modyfikujących stan zadań.
/// Używane przez `--allowed-tools` w CLI do ograniczania dostępu w sesjach read-only.
///
/// # Narzędzia
///
/// 1. `tasks_create` - Tworzenie nowych zadań
/// 2. `tasks_update` - Aktualizacja pól zadania
/// 3. `tasks_delete` - Usuwanie zadania i jego subtasków
/// 4. `tasks_move` - Przenoszenie zadania pod inny parent
/// 5. `tasks_batch_status` - Batch update statusów
/// 6. `tasks_set_deps` - Ustawianie zależności zadania
/// 7. `tasks_set_default_model` - Ustawianie domyślnego modelu
///
/// # Spójność
///
/// Stała musi być zgodna z `MUTATION_TOOLS` w `src/commands/mcp/tools.rs`.
/// Każde narzędzie z `MUTATION_TOOLS` ma odpowiednik w pełnym formacie:
/// `{short_name}` → `mcp__ralph-tasks__{short_name}`
#[allow(dead_code)] // Będzie używana w zadaniu 24.2 (--allowed-tools flag)
pub const MCP_MUTATION_TOOLS: &str = "mcp__ralph-tasks__tasks_create,mcp__ralph-tasks__tasks_update,mcp__ralph-tasks__tasks_delete,mcp__ralph-tasks__tasks_move,mcp__ralph-tasks__tasks_batch_status,mcp__ralph-tasks__tasks_set_deps,mcp__ralph-tasks__tasks_set_default_model";

/// Build the MCP config JSON for `--mcp-config` CLI flag (HTTP transport).
///
/// Generates a config that points Claude CLI to the HTTP MCP server
/// running on the specified port at `http://127.0.0.1:{port}/mcp`.
///
/// # Arguments
///
/// * `port` - Port number where the MCP server is listening (1-65535)
///
/// # Returns
///
/// JSON configuration object with HTTP transport settings
///
/// # Panics
///
/// Panics if port is 0 (reserved/invalid for HTTP servers)
pub fn build_mcp_config(port: u16) -> Value {
    assert!(port > 0, "Port must be greater than 0");

    json!({
        "mcpServers": {
            "ralph-tasks": {
                "type": "http",
                "url": format!("http://127.0.0.1:{}/mcp", port)
            }
        }
    })
}

/// Build the MCP config JSON with session ID for orchestrator workers.
///
/// Similar to `build_mcp_config()` but includes a session identifier
/// for multi-worker orchestration scenarios where each worker needs
/// to maintain isolated session state. Session ID is properly URL-encoded.
///
/// # Arguments
///
/// * `port` - Port number where the MCP server is listening (1-65535)
/// * `session_id` - Unique session identifier for this worker (non-empty)
///
/// # Returns
///
/// JSON configuration object with HTTP transport and session ID
///
/// # Panics
///
/// Panics if port is 0 or session_id is empty
pub fn build_mcp_config_with_session(port: u16, session_id: &str) -> Value {
    assert!(port > 0, "Port must be greater than 0");
    assert!(!session_id.is_empty(), "Session ID cannot be empty");

    // URL-encode session_id to prevent injection attacks
    let encoded_session = encode(session_id);

    json!({
        "mcpServers": {
            "ralph-tasks": {
                "type": "http",
                "url": format!("http://127.0.0.1:{}/mcp?session={}", port, encoded_session)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_mcp_config_structure() {
        let config = build_mcp_config(8080);
        let servers = config.get("mcpServers").unwrap();
        let ralph = servers.get("ralph-tasks").unwrap();

        // Sprawdź typ transportu
        assert_eq!(ralph.get("type").unwrap().as_str(), Some("http"));

        // Sprawdź URL
        let url = ralph.get("url").unwrap().as_str().unwrap();
        assert_eq!(url, "http://127.0.0.1:8080/mcp");
    }

    #[test]
    fn test_build_mcp_config_with_session() {
        let config = build_mcp_config_with_session(9000, "worker-123");
        let servers = config.get("mcpServers").unwrap();
        let ralph = servers.get("ralph-tasks").unwrap();

        // Sprawdź typ transportu
        assert_eq!(ralph.get("type").unwrap().as_str(), Some("http"));

        // Sprawdź URL z session ID
        let url = ralph.get("url").unwrap().as_str().unwrap();
        assert_eq!(url, "http://127.0.0.1:9000/mcp?session=worker-123");
    }

    #[test]
    fn test_build_mcp_config_different_ports() {
        let config1 = build_mcp_config(3000);
        let config2 = build_mcp_config(5000);

        let url1 = config1["mcpServers"]["ralph-tasks"]["url"]
            .as_str()
            .unwrap();
        let url2 = config2["mcpServers"]["ralph-tasks"]["url"]
            .as_str()
            .unwrap();

        assert_eq!(url1, "http://127.0.0.1:3000/mcp");
        assert_eq!(url2, "http://127.0.0.1:5000/mcp");
    }

    #[test]
    #[should_panic(expected = "Port must be greater than 0")]
    fn test_build_mcp_config_zero_port() {
        build_mcp_config(0);
    }

    #[test]
    #[should_panic(expected = "Port must be greater than 0")]
    fn test_build_mcp_config_with_session_zero_port() {
        build_mcp_config_with_session(0, "test-session");
    }

    #[test]
    #[should_panic(expected = "Session ID cannot be empty")]
    fn test_build_mcp_config_with_session_empty_id() {
        build_mcp_config_with_session(8080, "");
    }

    #[test]
    fn test_build_mcp_config_with_session_special_chars() {
        // Test URL encoding of special characters
        let config = build_mcp_config_with_session(8080, "worker/123 & test");
        let url = config["mcpServers"]["ralph-tasks"]["url"].as_str().unwrap();

        // URL-encoded: space=%20, /=%2F, &=%26
        assert_eq!(
            url,
            "http://127.0.0.1:8080/mcp?session=worker%2F123%20%26%20test"
        );
    }

    #[test]
    fn test_build_mcp_config_max_port() {
        // Test max valid port (65535)
        let config = build_mcp_config(65535);
        let url = config["mcpServers"]["ralph-tasks"]["url"].as_str().unwrap();

        assert_eq!(url, "http://127.0.0.1:65535/mcp");
    }

    // ── MCP_MUTATION_TOOLS tests ──────────────────────────────────────

    #[test]
    fn test_mcp_mutation_tools_count() {
        // Stała powinna zawierać dokładnie 7 narzędzi oddzielonych przecinkami
        let tools: Vec<&str> = MCP_MUTATION_TOOLS.split(',').collect();
        assert_eq!(
            tools.len(),
            7,
            "MCP_MUTATION_TOOLS powinno zawierać 7 narzędzi"
        );
    }

    #[test]
    fn test_mcp_mutation_tools_prefix() {
        // Każda nazwa powinna zaczynać się od mcp__ralph-tasks__tasks_
        let tools: Vec<&str> = MCP_MUTATION_TOOLS.split(',').collect();
        for tool in tools {
            assert!(
                tool.starts_with("mcp__ralph-tasks__tasks_"),
                "Narzędzie '{}' nie zaczyna się od 'mcp__ralph-tasks__tasks_'",
                tool
            );
        }
    }

    #[test]
    fn test_mcp_mutation_tools_consistent_with_tools_rs() {
        // Sprawdź spójność z MUTATION_TOOLS w src/commands/mcp/tools.rs
        // Oczekiwane krótkie nazwy z tools.rs (bez prefiksu)
        let expected_short_names = vec![
            "tasks_create",
            "tasks_update",
            "tasks_delete",
            "tasks_move",
            "tasks_batch_status",
            "tasks_set_deps",
            "tasks_set_default_model",
        ];

        let tools: Vec<&str> = MCP_MUTATION_TOOLS.split(',').collect();
        assert_eq!(
            tools.len(),
            expected_short_names.len(),
            "Liczba narzędzi nie zgadza się z MUTATION_TOOLS"
        );

        // Sprawdź czy każde oczekiwane narzędzie występuje w pełnej nazwie
        for short_name in expected_short_names {
            let full_name = format!("mcp__ralph-tasks__{}", short_name);
            assert!(
                tools.contains(&full_name.as_str()),
                "Brak narzędzia '{}' (pełna nazwa: '{}')",
                short_name,
                full_name
            );
        }
    }

    #[test]
    fn test_mcp_mutation_tools_no_duplicates() {
        // Sprawdź że nie ma duplikatów w liście
        let tools: Vec<&str> = MCP_MUTATION_TOOLS.split(',').collect();
        let unique_tools: std::collections::HashSet<&str> = tools.iter().copied().collect();
        assert_eq!(
            tools.len(),
            unique_tools.len(),
            "MCP_MUTATION_TOOLS zawiera duplikaty"
        );
    }

    #[test]
    fn test_mcp_mutation_tools_no_whitespace() {
        // Sprawdź że nazwy nie zawierają białych znaków (przed/po przecinku)
        let tools: Vec<&str> = MCP_MUTATION_TOOLS.split(',').collect();
        for tool in tools {
            assert_eq!(
                tool,
                tool.trim(),
                "Narzędzie '{}' zawiera białe znaki",
                tool
            );
        }
    }
}
