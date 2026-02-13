use serde_json::{Value, json};
use urlencoding::encode;

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
}
