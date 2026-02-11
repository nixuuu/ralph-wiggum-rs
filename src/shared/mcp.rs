use std::path::Path;

use serde_json::{json, Value};

/// Build the MCP config JSON for `--mcp-config` CLI flag.
///
/// Generates a config that points Claude CLI to this binary's MCP server
/// subcommand with the given tasks file path.
pub fn build_mcp_config(tasks_path: &Path) -> Value {
    let exe = std::env::current_exe()
        .unwrap_or_else(|_| "ralph-wiggum".into());

    json!({
        "mcpServers": {
            "ralph-tasks": {
                "command": exe.to_string_lossy(),
                "args": [
                    "mcp-server",
                    "--tasks-file",
                    tasks_path.to_string_lossy()
                ]
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_build_mcp_config_structure() {
        let config = build_mcp_config(&PathBuf::from(".ralph/tasks.yml"));
        let servers = config.get("mcpServers").unwrap();
        let ralph = servers.get("ralph-tasks").unwrap();
        assert!(ralph.get("command").is_some());
        let args = ralph.get("args").unwrap().as_array().unwrap();
        assert_eq!(args[0], "mcp-server");
        assert_eq!(args[1], "--tasks-file");
        assert_eq!(args[2], ".ralph/tasks.yml");
    }
}
