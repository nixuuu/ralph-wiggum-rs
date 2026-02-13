use std::path::Path;

use serde_json::{Value, json};

use super::handlers;

/// Lista mutation tools — narzędzia modyfikujące stan zadań
///
/// Te narzędzia są blokowane w sesjach read-only.
const MUTATION_TOOLS: &[&str] = &[
    "tasks_create",
    "tasks_update",
    "tasks_delete",
    "tasks_move",
    "tasks_batch_status",
    "tasks_set_deps",
    "tasks_set_default_model",
];

/// Sprawdza czy narzędzie jest mutation tool (modyfikuje stan)
///
/// # Arguments
///
/// * `name` - Nazwa narzędzia do sprawdzenia
///
/// # Returns
///
/// `true` jeśli narzędzie modyfikuje stan, `false` w przeciwnym razie
///
/// # Example
///
/// ```
/// use ralph_wiggum::commands::mcp::tools::is_mutation_tool;
///
/// assert!(is_mutation_tool("tasks_create"));
/// assert!(is_mutation_tool("tasks_update"));
/// assert!(!is_mutation_tool("tasks_list"));
/// assert!(!is_mutation_tool("ask_user"));
/// ```
pub fn is_mutation_tool(name: &str) -> bool {
    MUTATION_TOOLS.contains(&name)
}

/// Return MCP tools/list response with only read-only tools (query tools + ask_user).
///
/// Używane dla sesji read-only — zwraca tylko narzędzia bez efektów ubocznych:
/// - Query tools: tasks_list, tasks_get, tasks_summary, tasks_tree
/// - Interactive tool: ask_user
///
/// # Returns
///
/// JSON Value z polem "tools" zawierającym tablicę tylko read-only tools
///
/// # Example
///
/// ```
/// use ralph_wiggum::commands::mcp::tools::list_tools_readonly;
///
/// let tools = list_tools_readonly();
/// let tool_names: Vec<&str> = tools["tools"]
///     .as_array()
///     .unwrap()
///     .iter()
///     .map(|t| t["name"].as_str().unwrap())
///     .collect();
///
/// assert!(tool_names.contains(&"tasks_list"));
/// assert!(tool_names.contains(&"ask_user"));
/// assert!(!tool_names.contains(&"tasks_create"));
/// ```
pub fn list_tools_readonly() -> Value {
    let all_tools = list_tools();
    let tools_array = all_tools["tools"].as_array().unwrap();

    // Filtruj tylko read-only tools (nie-mutation)
    let readonly_tools: Vec<Value> = tools_array
        .iter()
        .filter(|tool| {
            let name = tool["name"].as_str().unwrap();
            !is_mutation_tool(name)
        })
        .cloned()
        .collect();

    json!({
        "tools": readonly_tools
    })
}

/// Return MCP tools/list response with all 12 tool definitions.
pub fn list_tools() -> Value {
    json!({
        "tools": [
            {
                "name": "tasks_list",
                "description": "List tasks. Returns tree YAML or flat list of leaves with optional filtering.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "format": {
                            "type": "string",
                            "enum": ["tree", "flat"],
                            "description": "Output format: 'tree' (YAML hierarchy) or 'flat' (leaf list). Default: tree"
                        },
                        "status": {
                            "type": "string",
                            "enum": ["todo", "in_progress", "done", "blocked"],
                            "description": "Filter by status (flat format only)"
                        },
                        "component": {
                            "type": "string",
                            "description": "Filter by component (flat format only)"
                        }
                    }
                }
            },
            {
                "name": "tasks_get",
                "description": "Get details of a single task by ID. Returns the full node including subtasks if it's a parent.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Task ID (e.g., '1.2.3')"
                        }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "tasks_summary",
                "description": "Get task progress summary: counts per status, current task, progress percentage, default model.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "tasks_tree",
                "description": "Get the full tasks.yml content as YAML. Use for bulk review of all tasks.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "tasks_create",
                "description": "Create new tasks. Provide a YAML string defining the task hierarchy. Supports creating parent + subtasks in one call.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "parent_id": {
                            "type": "string",
                            "description": "ID of parent task to add under. Omit for root level."
                        },
                        "tasks": {
                            "type": "string",
                            "description": "YAML string with task list. Each task needs: id, name, status (for leaves). Example:\n- id: \"3.1\"\n  name: \"Implement feature\"\n  status: todo\n  component: api"
                        }
                    },
                    "required": ["tasks"]
                }
            },
            {
                "name": "tasks_update",
                "description": "Update fields of an existing task. Only provided fields are changed. Set a field to null to remove it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Task ID to update"
                        },
                        "name": {
                            "type": "string",
                            "description": "New task name"
                        },
                        "status": {
                            "type": "string",
                            "enum": ["todo", "in_progress", "done", "blocked"],
                            "description": "New status (leaves only)"
                        },
                        "component": {
                            "type": "string",
                            "description": "New component"
                        },
                        "deps": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Dependency task IDs (leaves only)"
                        },
                        "model": {
                            "type": "string",
                            "description": "Model override for this task"
                        },
                        "description": {
                            "type": "string",
                            "description": "Task description"
                        },
                        "related_files": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Related file paths"
                        },
                        "implementation_steps": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Ordered implementation steps"
                        }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "tasks_delete",
                "description": "Delete a task and all its subtasks. Also cleans up dependency references.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Task ID to delete"
                        }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "tasks_move",
                "description": "Move a task under a different parent. Set new_parent_id to null to move to root.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Task ID to move"
                        },
                        "new_parent_id": {
                            "type": "string",
                            "description": "New parent task ID. Null/omit for root level."
                        },
                        "position": {
                            "type": "integer",
                            "description": "Position index in the new parent's children list"
                        }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "tasks_batch_status",
                "description": "Batch update statuses for multiple tasks at once.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "updates": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": {"type": "string"},
                                    "status": {
                                        "type": "string",
                                        "enum": ["todo", "in_progress", "done", "blocked"]
                                    }
                                },
                                "required": ["id", "status"]
                            },
                            "description": "Array of {id, status} updates"
                        }
                    },
                    "required": ["updates"]
                }
            },
            {
                "name": "tasks_set_deps",
                "description": "Set dependency task IDs for a leaf task. Replaces existing deps.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Leaf task ID"
                        },
                        "deps": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Array of dependency task IDs"
                        }
                    },
                    "required": ["id", "deps"]
                }
            },
            {
                "name": "tasks_set_default_model",
                "description": "Set the global default model for all tasks.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "model": {
                            "type": "string",
                            "description": "Model name (e.g., 'claude-sonnet-4-5-20250929', 'claude-opus-4-6')"
                        }
                    },
                    "required": ["model"]
                }
            },
            {
                "name": "ask_user",
                "description": "Ask the user a question or request clarification. The response will be captured and returned.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string",
                            "description": "The question or message to display to the user"
                        },
                        "options": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Optional list of suggested responses for the user to choose from"
                        },
                        "multiSelect": {
                            "type": "boolean",
                            "description": "When true and options are provided, allows selecting multiple options instead of one"
                        }
                    },
                    "required": ["question"]
                }
            }
        ]
    })
}

/// Obsługuje pełne wywołanie tools/call — parsuje params, dispatchuje i formatuje response.
///
/// # Workflow
///
/// 1. **Parse tool name and arguments** z JSON-RPC params
///    - Wyciąga `name` (wymagane) i `arguments` (opcjonalne, default: `{}`)
/// 2. **Dispatch do odpowiedniego handlera** na podstawie tool name
///    - 11 CRUD tools (tasks_*) → handler z `handlers.rs` → JSON response
///    - ask_user → handler z `handlers.rs` → placeholder (TODO: SSE stream w Phase 5)
/// 3. **Format response**
///    - Success: MCP content array z JSON result (text type)
///    - Error: MCP content array z error message + `isError: true` flag
///    - Missing name: JSON-RPC error response (INVALID_PARAMS)
///
/// # Arguments
///
/// * `id` - JSON-RPC request ID
/// * `params` - JSON-RPC params (zawiera `name` i opcjonalnie `arguments`)
/// * `tasks_path` - Ścieżka do pliku tasks.yml (dla CRUD tools)
///
/// # Returns
///
/// JsonRpcResponse w formacie MCP (tools/call result)
pub fn handle_tool_call(
    id: Option<Value>,
    params: Option<&Value>,
    tasks_path: &Path,
) -> super::protocol::JsonRpcResponse {
    // Step 1: Parse tool name and arguments from params
    let tool_name = params.and_then(|p| p.get("name")).and_then(|n| n.as_str());
    let tool_args = params
        .and_then(|p| p.get("arguments"))
        .cloned()
        .unwrap_or(json!({}));

    match tool_name {
        Some(name) => {
            // Step 2: Dispatch to appropriate handler
            let result = dispatch(name, &tool_args, tasks_path);

            // Step 3: Format response based on result
            match result {
                Ok(value) => super::protocol::JsonRpcResponse::success(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string(&value).unwrap_or_default()
                        }]
                    }),
                ),
                Err(err) => super::protocol::JsonRpcResponse::success(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": err
                        }],
                        "isError": true
                    }),
                ),
            }
        }
        None => super::protocol::JsonRpcResponse::error(
            id,
            super::protocol::INVALID_PARAMS,
            "Missing tool name in tools/call".into(),
        ),
    }
}

/// Dispatch a tool call to the appropriate handler based on tool name.
///
/// # Tool Routing
///
/// ## CRUD Tools (11 total)
/// Wszystkie CRUD tools operują na pliku tasks.yml i zwracają JSON response:
/// - `tasks_list` - Lista tasków (tree lub flat format z filtrowaniem)
/// - `tasks_get` - Szczegóły pojedynczego taska
/// - `tasks_summary` - Podsumowanie statusów tasków
/// - `tasks_tree` - Pełna hierarchia tasków jako YAML
/// - `tasks_create` - Tworzenie nowych tasków (YAML input)
/// - `tasks_update` - Aktualizacja pól taska
/// - `tasks_delete` - Usuwanie taska i jego subtasków
/// - `tasks_move` - Przenoszenie taska pod inny parent
/// - `tasks_batch_status` - Batch update statusów
/// - `tasks_set_deps` - Ustawianie zależności taska
/// - `tasks_set_default_model` - Ustawianie domyślnego modelu
///
/// ## Interactive Tool (1 total)
/// - `ask_user` - Zadaj pytanie użytkownikowi (placeholder, TODO: SSE stream w Phase 5)
///
/// # Arguments
///
/// * `tool_name` - Nazwa narzędzia do wywołania
/// * `params` - Parametry wywołania (JSON Value)
/// * `tasks_path` - Ścieżka do pliku tasks.yml (używana przez CRUD tools)
///
/// # Returns
///
/// * `Ok(Value)` - JSON result z handlera
/// * `Err(String)` - Error message (unknown tool lub błąd handlera)
///
/// # Errors
///
/// Zwraca błąd jeśli:
/// - Tool name jest nieznany (nie pasuje do żadnego z 12 narzędzi)
/// - Handler zwróci błąd (np. brak parametru, walidacja nie powiodła się)
pub fn dispatch(tool_name: &str, params: &Value, tasks_path: &Path) -> Result<Value, String> {
    match tool_name {
        // CRUD Tools: Query operations
        "tasks_list" => handlers::tasks_list(tasks_path, params),
        "tasks_get" => handlers::tasks_get(tasks_path, params),
        "tasks_summary" => handlers::tasks_summary(tasks_path, params),
        "tasks_tree" => handlers::tasks_tree(tasks_path, params),

        // CRUD Tools: Mutation operations
        "tasks_create" => handlers::tasks_create(tasks_path, params),
        "tasks_update" => handlers::tasks_update(tasks_path, params),
        "tasks_delete" => handlers::tasks_delete(tasks_path, params),
        "tasks_move" => handlers::tasks_move(tasks_path, params),
        "tasks_batch_status" => handlers::tasks_batch_status(tasks_path, params),
        "tasks_set_deps" => handlers::tasks_set_deps(tasks_path, params),
        "tasks_set_default_model" => handlers::tasks_set_default_model(tasks_path, params),

        // Interactive Tool: ask_user
        // TODO (Phase 5): Return SSE stream instead of JSON placeholder
        "ask_user" => handlers::ask_user(params),

        // Unknown tool
        _ => Err(format!("Unknown tool: {tool_name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_tools_count() {
        let tools = list_tools();
        let arr = tools["tools"].as_array().unwrap();
        assert_eq!(arr.len(), 12);
    }

    #[test]
    fn test_list_tools_names() {
        let tools = list_tools();
        let names: Vec<&str> = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"tasks_list"));
        assert!(names.contains(&"tasks_create"));
        assert!(names.contains(&"tasks_delete"));
        assert!(names.contains(&"tasks_set_default_model"));
        assert!(names.contains(&"ask_user"));
    }

    #[test]
    fn test_dispatch_unknown_tool() {
        let path = std::path::PathBuf::from("/nonexistent");
        let err = dispatch("unknown_tool", &json!({}), &path).unwrap_err();
        assert!(err.contains("Unknown tool"));
    }

    #[test]
    fn test_ask_user_tool_schema() {
        let tools = list_tools();
        let arr = tools["tools"].as_array().unwrap();
        let ask_user = arr
            .iter()
            .find(|t| t["name"].as_str() == Some("ask_user"))
            .expect("ask_user tool not found");

        // Verify name
        assert_eq!(ask_user["name"].as_str().unwrap(), "ask_user");

        // Verify description exists
        let desc = ask_user["description"].as_str().unwrap();
        assert!(!desc.is_empty());

        // Verify inputSchema
        let schema = &ask_user["inputSchema"];
        assert_eq!(schema["type"].as_str().unwrap(), "object");

        // Verify properties
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("question"));
        assert!(props.contains_key("options"));

        // Verify question is required
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("question")));
    }

    #[test]
    fn test_all_tools_have_valid_schema() {
        let tools = list_tools();
        let arr = tools["tools"].as_array().unwrap();

        for tool in arr {
            // Check name exists and is non-empty
            let name = tool["name"].as_str().expect("Tool name is not a string");
            assert!(!name.is_empty(), "Tool has empty name");

            // Check description exists and is non-empty
            let desc = tool["description"]
                .as_str()
                .expect("Tool description is not a string");
            assert!(!desc.is_empty(), "Tool '{}' has empty description", name);

            // Check inputSchema exists and is an object
            let schema = &tool["inputSchema"];
            assert!(
                schema.is_object(),
                "Tool '{}' inputSchema is not an object",
                name
            );

            // Check inputSchema has type field
            let schema_type = schema["type"].as_str();
            assert_eq!(
                schema_type,
                Some("object"),
                "Tool '{}' inputSchema type is not 'object'",
                name
            );
        }
    }

    // ── Dispatcher Tests ───────────────────────────────────────────────

    #[test]
    fn test_handle_tool_call_missing_name() {
        let path = std::path::PathBuf::from("/nonexistent");
        let params = json!({"arguments": {}});
        let response = handle_tool_call(Some(json!(1)), Some(&params), &path);

        // Should return JSON-RPC error for missing name
        assert!(response.error.is_some());
        let err = response.error.unwrap();
        assert_eq!(err.code, super::super::protocol::INVALID_PARAMS);
        assert!(err.message.contains("Missing tool name"));
    }

    #[test]
    fn test_handle_tool_call_unknown_tool() {
        let path = std::path::PathBuf::from("/nonexistent");
        let params = json!({"name": "unknown_tool", "arguments": {}});
        let response = handle_tool_call(Some(json!(1)), Some(&params), &path);

        // Should return success response with isError flag
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Unknown tool")
        );
    }

    #[test]
    fn test_handle_tool_call_with_valid_tool() {
        // Create temporary tasks file for testing
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.yml");
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929
tasks:
  - id: "1"
    name: "Test Task"
    status: todo
"#;
        std::fs::write(&path, yaml).unwrap();

        let params = json!({
            "name": "tasks_summary",
            "arguments": {}
        });
        let response = handle_tool_call(Some(json!(1)), Some(&params), &path);

        // Should return success response
        assert!(response.result.is_some());
        assert!(response.error.is_none());
        let result = response.result.unwrap();
        assert!(result["content"].is_array());
        assert_eq!(result["content"][0]["type"], "text");
    }

    #[test]
    fn test_handle_tool_call_default_empty_arguments() {
        let path = std::path::PathBuf::from("/nonexistent");
        // Missing "arguments" field should default to {}
        let params = json!({"name": "unknown_tool"});
        let response = handle_tool_call(Some(json!(1)), Some(&params), &path);

        // Should still dispatch (even though tool is unknown)
        assert!(response.result.is_some());
    }

    #[test]
    fn test_dispatch_all_crud_tools_exist() {
        // Test that all 11 CRUD tools are registered in dispatch
        let path = std::path::PathBuf::from("/nonexistent");
        let crud_tools = vec![
            "tasks_list",
            "tasks_get",
            "tasks_summary",
            "tasks_tree",
            "tasks_create",
            "tasks_update",
            "tasks_delete",
            "tasks_move",
            "tasks_batch_status",
            "tasks_set_deps",
            "tasks_set_default_model",
        ];

        for tool in crud_tools {
            let result = dispatch(tool, &json!({}), &path);
            // All tools should be recognized (even if they fail with invalid params)
            // They should NOT return "Unknown tool" error
            if let Err(e) = result {
                assert!(
                    !e.contains("Unknown tool"),
                    "Tool '{}' not registered in dispatch",
                    tool
                );
            }
        }
    }

    #[test]
    fn test_dispatch_ask_user_exists() {
        // Test that ask_user is registered in dispatch
        let path = std::path::PathBuf::from("/nonexistent");
        let result = dispatch("ask_user", &json!({}), &path);

        // ask_user should be recognized (even if it fails with invalid params)
        if let Err(e) = result {
            assert!(
                !e.contains("Unknown tool"),
                "ask_user not registered in dispatch"
            );
        }
    }

    #[test]
    fn test_dispatch_returns_error_for_unknown_tool() {
        let path = std::path::PathBuf::from("/nonexistent");
        let result = dispatch("totally_unknown_tool_xyz", &json!({}), &path);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Unknown tool"));
        assert!(err.contains("totally_unknown_tool_xyz"));
    }

    #[test]
    fn test_handle_tool_call_formats_success_as_mcp_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.yml");
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929
tasks: []
"#;
        std::fs::write(&path, yaml).unwrap();

        let params = json!({
            "name": "tasks_summary",
            "arguments": {}
        });
        let response = handle_tool_call(Some(json!(1)), Some(&params), &path);

        // Check MCP content format
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert!(result["content"].is_array());
        assert_eq!(result["content"].as_array().unwrap().len(), 1);

        let content_item = &result["content"][0];
        assert_eq!(content_item["type"], "text");
        assert!(content_item["text"].is_string());
        // Should be valid JSON (compact format)
        let text = content_item["text"].as_str().unwrap();
        assert!(serde_json::from_str::<Value>(text).is_ok());
    }

    #[test]
    fn test_handle_tool_call_formats_error_with_is_error_flag() {
        let path = std::path::PathBuf::from("/nonexistent");
        let params = json!({
            "name": "tasks_get",
            "arguments": {} // Missing required "id" param
        });
        let response = handle_tool_call(Some(json!(1)), Some(&params), &path);

        // Check error format
        assert!(response.result.is_some());
        assert!(response.error.is_none());

        let result = response.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"].is_array());

        let content_item = &result["content"][0];
        assert_eq!(content_item["type"], "text");
        let text = content_item["text"].as_str().unwrap();
        assert!(text.contains("Missing") || text.contains("required"));
    }

    #[test]
    fn test_handle_tool_call_with_null_arguments() {
        let path = std::path::PathBuf::from("/nonexistent");
        let params = json!({"name": "unknown_tool", "arguments": null});
        let response = handle_tool_call(Some(json!(1)), Some(&params), &path);

        // Should treat null arguments as empty object {}
        assert!(response.result.is_some());
    }

    #[test]
    fn test_handle_tool_call_ask_user_with_invalid_options() {
        let path = std::path::PathBuf::from("/nonexistent");
        let params = json!({
            "name": "ask_user",
            "arguments": {
                "question": "Test",
                "options": "not_an_array"
            }
        });
        let response = handle_tool_call(Some(json!(1)), Some(&params), &path);

        // Should return error with isError flag
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("must be an array")
        );
    }

    #[test]
    fn test_dispatch_with_extra_unknown_params() {
        // Handlers should ignore extra unknown parameters
        let path = std::path::PathBuf::from("/nonexistent");
        let params = json!({
            "question": "Test",
            "garbage": "ignore_me",
            "random": 123
        });
        let result = dispatch("ask_user", &params, &path);

        // Should succeed and ignore extra params
        assert!(result.is_ok());
    }

    #[test]
    fn test_all_12_tools_can_be_dispatched() {
        // Comprehensive test: all 12 tools (11 CRUD + ask_user) are dispatchable
        let path = std::path::PathBuf::from("/nonexistent");
        let all_tools = vec![
            "tasks_list",
            "tasks_get",
            "tasks_summary",
            "tasks_tree",
            "tasks_create",
            "tasks_update",
            "tasks_delete",
            "tasks_move",
            "tasks_batch_status",
            "tasks_set_deps",
            "tasks_set_default_model",
            "ask_user",
        ];

        for tool in all_tools {
            let result = dispatch(tool, &json!({}), &path);
            // Tool should be recognized (not "Unknown tool")
            if let Err(e) = result {
                assert!(
                    !e.contains("Unknown tool"),
                    "Tool '{}' should be registered but got error: {}",
                    tool,
                    e
                );
            }
        }
    }

    // ── Read-only session tests ───────────────────────────────────

    #[test]
    fn test_list_tools_readonly_excludes_mutation_tools() {
        let tools = list_tools_readonly();
        let tool_names: Vec<&str> = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();

        // Sprawdź że mutation tools NIE są obecne
        assert!(!tool_names.contains(&"tasks_create"));
        assert!(!tool_names.contains(&"tasks_update"));
        assert!(!tool_names.contains(&"tasks_delete"));
        assert!(!tool_names.contains(&"tasks_move"));
        assert!(!tool_names.contains(&"tasks_batch_status"));
        assert!(!tool_names.contains(&"tasks_set_deps"));
        assert!(!tool_names.contains(&"tasks_set_default_model"));
    }

    #[test]
    fn test_list_tools_readonly_includes_query_tools() {
        let tools = list_tools_readonly();
        let tool_names: Vec<&str> = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();

        // Sprawdź że query tools SĄ obecne
        assert!(tool_names.contains(&"tasks_list"));
        assert!(tool_names.contains(&"tasks_get"));
        assert!(tool_names.contains(&"tasks_summary"));
        assert!(tool_names.contains(&"tasks_tree"));

        // Sprawdź że ask_user JEST obecne (interactive tool)
        assert!(tool_names.contains(&"ask_user"));
    }

    #[test]
    fn test_list_tools_readonly_returns_exactly_5_tools() {
        // 4 query tools + 1 interactive tool (ask_user) = 5 total
        let tools = list_tools_readonly();
        let arr = tools["tools"].as_array().unwrap();
        assert_eq!(arr.len(), 5);
    }

    #[test]
    fn test_is_mutation_tool_returns_true_for_mutation_tools() {
        // Sprawdź wszystkie mutation tools
        assert!(is_mutation_tool("tasks_create"));
        assert!(is_mutation_tool("tasks_update"));
        assert!(is_mutation_tool("tasks_delete"));
        assert!(is_mutation_tool("tasks_move"));
        assert!(is_mutation_tool("tasks_batch_status"));
        assert!(is_mutation_tool("tasks_set_deps"));
        assert!(is_mutation_tool("tasks_set_default_model"));
    }

    #[test]
    fn test_is_mutation_tool_returns_false_for_query_tools() {
        // Sprawdź query tools
        assert!(!is_mutation_tool("tasks_list"));
        assert!(!is_mutation_tool("tasks_get"));
        assert!(!is_mutation_tool("tasks_summary"));
        assert!(!is_mutation_tool("tasks_tree"));
    }

    #[test]
    fn test_is_mutation_tool_returns_false_for_ask_user() {
        // ask_user nie jest mutation tool (interactive, ale read-only safe)
        assert!(!is_mutation_tool("ask_user"));
    }

    #[test]
    fn test_is_mutation_tool_returns_false_for_unknown_tool() {
        // Narzędzia nieznane nie są mutation tools
        assert!(!is_mutation_tool("unknown_tool"));
        assert!(!is_mutation_tool("fake_mutation_tool"));
    }
}
