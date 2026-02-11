use std::path::Path;

use serde_json::{Value, json};

use super::handlers;

/// Return MCP tools/list response with all 11 tool definitions.
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
            }
        ]
    })
}

/// Dispatch a tool call to the appropriate handler.
pub fn dispatch(tool_name: &str, params: &Value, tasks_path: &Path) -> Result<Value, String> {
    match tool_name {
        "tasks_list" => handlers::tasks_list(tasks_path, params),
        "tasks_get" => handlers::tasks_get(tasks_path, params),
        "tasks_summary" => handlers::tasks_summary(tasks_path, params),
        "tasks_tree" => handlers::tasks_tree(tasks_path, params),
        "tasks_create" => handlers::tasks_create(tasks_path, params),
        "tasks_update" => handlers::tasks_update(tasks_path, params),
        "tasks_delete" => handlers::tasks_delete(tasks_path, params),
        "tasks_move" => handlers::tasks_move(tasks_path, params),
        "tasks_batch_status" => handlers::tasks_batch_status(tasks_path, params),
        "tasks_set_deps" => handlers::tasks_set_deps(tasks_path, params),
        "tasks_set_default_model" => handlers::tasks_set_default_model(tasks_path, params),
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
        assert_eq!(arr.len(), 11);
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
    }

    #[test]
    fn test_dispatch_unknown_tool() {
        let path = std::path::PathBuf::from("/nonexistent");
        let err = dispatch("unknown_tool", &json!({}), &path).unwrap_err();
        assert!(err.contains("Unknown tool"));
    }
}
