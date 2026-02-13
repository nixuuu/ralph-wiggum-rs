use std::path::Path;

use serde_json::{Value, json};

use crate::shared::progress::TaskStatus;
use crate::shared::tasks::{TaskNode, TasksFile};

/// Load → modify → validate → save helper.
///
/// Standardowy wzorzec transakcyjny dla operacji na tasks.yml:
/// 1. Ładuje TasksFile z dysku
/// 2. Wykonuje modyfikację przez closure
/// 3. Waliduje integralność (deps, cycles, status)
/// 4. Zapisuje z powrotem na dysk
///
/// Automatycznie rollbackuje zmiany jeśli walidacja lub zapis się nie powiedzie.
fn with_tasks_file<F>(tasks_path: &Path, f: F) -> Result<Value, String>
where
    F: FnOnce(&mut TasksFile) -> Result<Value, String>,
{
    let mut tf = TasksFile::load(tasks_path).map_err(|e| e.to_string())?;
    let result = f(&mut tf)?;
    tf.validate()
        .map_err(|e| format!("Validation failed: {e}"))?;
    tf.save(tasks_path).map_err(|e| e.to_string())?;
    Ok(result)
}

// ── Query tools ────────────────────────────────────────────────────

/// tasks_list: List tasks with optional filters.
pub fn tasks_list(tasks_path: &Path, params: &Value) -> Result<Value, String> {
    let tf = TasksFile::load(tasks_path).map_err(|e| e.to_string())?;
    let format = params
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("tree");
    let status_filter = params.get("status").and_then(|v| v.as_str());
    let component_filter = params.get("component").and_then(|v| v.as_str());

    match format {
        "flat" => {
            let leaves = tf.flatten_leaves();
            let filtered: Vec<Value> = leaves
                .iter()
                .filter(|l| {
                    if let Some(sf) = status_filter {
                        let status_str = serde_json::to_value(&l.status)
                            .ok()
                            .and_then(|v| v.as_str().map(String::from))
                            .unwrap_or_default();
                        if status_str != sf {
                            return false;
                        }
                    }
                    if let Some(cf) = component_filter
                        && l.component != cf
                    {
                        return false;
                    }
                    true
                })
                .map(|l| {
                    json!({
                        "id": l.id,
                        "name": l.name,
                        "component": l.component,
                        "status": l.status,
                        "deps": l.deps,
                        "model": l.model,
                        "description": l.description,
                    })
                })
                .collect();
            Ok(json!({ "tasks": filtered }))
        }
        _ => {
            // tree format: return YAML representation
            let yaml = serde_yaml::to_string(&tf).map_err(|e| e.to_string())?;
            Ok(json!({ "yaml": yaml }))
        }
    }
}

/// tasks_get: Get a single task by ID.
pub fn tasks_get(tasks_path: &Path, params: &Value) -> Result<Value, String> {
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required param: id")?;

    let tf = TasksFile::load(tasks_path).map_err(|e| e.to_string())?;
    let node = tf
        .find_node(id)
        .ok_or_else(|| format!("Task not found: {id}"))?;

    serde_json::to_value(node).map_err(|e| e.to_string())
}

/// tasks_summary: Overview of task statuses.
pub fn tasks_summary(tasks_path: &Path, _params: &Value) -> Result<Value, String> {
    let tf = TasksFile::load(tasks_path).map_err(|e| e.to_string())?;
    let summary = tf.to_summary();
    let current = tf.current_task();

    Ok(json!({
        "total": summary.total(),
        "done": summary.done,
        "in_progress": summary.in_progress,
        "todo": summary.todo,
        "blocked": summary.blocked,
        "progress_percent": if summary.total() > 0 {
            (summary.done as f64 / summary.total() as f64 * 100.0).round()
        } else {
            0.0
        },
        "current_task": current.map(|t| json!({
            "id": t.id,
            "name": t.name,
            "component": t.component,
        })),
        "default_model": tf.default_model,
    }))
}

/// tasks_tree: Full YAML dump.
pub fn tasks_tree(tasks_path: &Path, _params: &Value) -> Result<Value, String> {
    let tf = TasksFile::load(tasks_path).map_err(|e| e.to_string())?;
    let yaml = serde_yaml::to_string(&tf).map_err(|e| e.to_string())?;
    Ok(json!({ "yaml": yaml }))
}

// ── Mutation tools ─────────────────────────────────────────────────

/// tasks_create: Create tasks under a parent (or root).
/// Params: parent_id (optional), tasks (YAML string with task hierarchy).
pub fn tasks_create(tasks_path: &Path, params: &Value) -> Result<Value, String> {
    let parent_id = params.get("parent_id").and_then(|v| v.as_str());
    let tasks_yaml = params
        .get("tasks")
        .and_then(|v| v.as_str())
        .ok_or("Missing required param: tasks (YAML string)")?;

    // Parse the tasks YAML — it should be a list of TaskNode
    let new_nodes: Vec<TaskNode> =
        serde_yaml::from_str(tasks_yaml).map_err(|e| format!("Invalid tasks YAML: {e}"))?;

    if new_nodes.is_empty() {
        return Err("No tasks provided".to_string());
    }

    with_tasks_file(tasks_path, |tf| {
        // Check for ID conflicts before adding
        let existing_ids = tf.all_ids();
        let mut new_ids = Vec::new();
        fn collect_new_ids(node: &TaskNode, ids: &mut Vec<String>) {
            ids.push(node.id.clone());
            for child in &node.subtasks {
                collect_new_ids(child, ids);
            }
        }
        for node in &new_nodes {
            collect_new_ids(node, &mut new_ids);
        }
        for id in &new_ids {
            if existing_ids.contains(id) {
                return Err(format!("Task ID already exists: {id}"));
            }
        }

        for node in new_nodes {
            tf.add_task(parent_id, node, None)
                .map_err(|e| e.to_string())?;
        }

        Ok(json!({ "created": new_ids }))
    })
}

/// tasks_update: Update fields of a single task.
pub fn tasks_update(tasks_path: &Path, params: &Value) -> Result<Value, String> {
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required param: id")?;

    with_tasks_file(tasks_path, |tf| {
        let node = tf
            .find_node_mut(id)
            .ok_or_else(|| format!("Task not found: {id}"))?;

        if let Some(name) = params.get("name").and_then(|v| v.as_str()) {
            node.name = name.to_string();
        }
        if let Some(status_val) = params.get("status") {
            if status_val.is_null() {
                node.status = None;
            } else if let Some(s) = status_val.as_str() {
                let status: TaskStatus =
                    serde_json::from_value(json!(s)).map_err(|e| format!("Invalid status: {e}"))?;
                node.status = Some(status);
            }
        }
        if let Some(component_val) = params.get("component") {
            if component_val.is_null() {
                node.component = None;
            } else if let Some(c) = component_val.as_str() {
                node.component = Some(c.to_string());
            }
        }
        if let Some(deps_val) = params.get("deps") {
            if deps_val.is_null() {
                node.deps = Vec::new();
            } else if let Some(arr) = deps_val.as_array() {
                node.deps = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
            }
        }
        if let Some(model_val) = params.get("model") {
            if model_val.is_null() {
                node.model = None;
            } else if let Some(m) = model_val.as_str() {
                node.model = Some(m.to_string());
            }
        }
        if let Some(desc_val) = params.get("description") {
            if desc_val.is_null() {
                node.description = None;
            } else if let Some(d) = desc_val.as_str() {
                node.description = Some(d.to_string());
            }
        }
        if let Some(rf_val) = params.get("related_files") {
            if rf_val.is_null() {
                node.related_files = Vec::new();
            } else if let Some(arr) = rf_val.as_array() {
                node.related_files = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
            }
        }
        if let Some(is_val) = params.get("implementation_steps") {
            if is_val.is_null() {
                node.implementation_steps = Vec::new();
            } else if let Some(arr) = is_val.as_array() {
                node.implementation_steps = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
            }
        }

        Ok(json!({ "updated": id }))
    })
}

/// tasks_delete: Remove a task and its subtasks.
pub fn tasks_delete(tasks_path: &Path, params: &Value) -> Result<Value, String> {
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required param: id")?;

    with_tasks_file(tasks_path, |tf| {
        tf.remove_task(id)
            .ok_or_else(|| format!("Task not found: {id}"))?;
        Ok(json!({ "deleted": id }))
    })
}

/// tasks_move: Move a task under a new parent.
pub fn tasks_move(tasks_path: &Path, params: &Value) -> Result<Value, String> {
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required param: id")?;
    let new_parent_id = params.get("new_parent_id").and_then(|v| v.as_str());
    let position = params
        .get("position")
        .and_then(|v| v.as_u64())
        .map(|p| p as usize);

    with_tasks_file(tasks_path, |tf| {
        let node = tf
            .remove_task(id)
            .ok_or_else(|| format!("Task not found: {id}"))?;
        tf.add_task(new_parent_id, node, position)
            .map_err(|e| e.to_string())?;
        Ok(json!({ "moved": id, "new_parent": new_parent_id }))
    })
}

/// tasks_batch_status: Batch update statuses.
pub fn tasks_batch_status(tasks_path: &Path, params: &Value) -> Result<Value, String> {
    let updates = params
        .get("updates")
        .and_then(|v| v.as_array())
        .ok_or("Missing required param: updates (array)")?;

    let parsed: Vec<(String, TaskStatus)> = updates
        .iter()
        .map(|u| {
            let id = u
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or("Each update needs 'id'")?;
            let status_str = u
                .get("status")
                .and_then(|v| v.as_str())
                .ok_or("Each update needs 'status'")?;
            let status: TaskStatus = serde_json::from_value(json!(status_str))
                .map_err(|e| format!("Invalid status '{}': {}", status_str, e))?;
            Ok((id.to_string(), status))
        })
        .collect::<Result<Vec<_>, String>>()?;

    TasksFile::batch_update_statuses(tasks_path, &parsed).map_err(|e| e.to_string())?;

    let ids: Vec<&str> = parsed.iter().map(|(id, _)| id.as_str()).collect();
    Ok(json!({ "updated": ids }))
}

/// tasks_set_deps: Set deps for a leaf task.
pub fn tasks_set_deps(tasks_path: &Path, params: &Value) -> Result<Value, String> {
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required param: id")?;
    let deps = params
        .get("deps")
        .and_then(|v| v.as_array())
        .ok_or("Missing required param: deps (array)")?;

    let dep_ids: Vec<String> = deps
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    with_tasks_file(tasks_path, |tf| {
        let node = tf
            .find_node_mut(id)
            .ok_or_else(|| format!("Task not found: {id}"))?;
        if !node.is_leaf() {
            return Err(format!("Task {id} is a parent — deps are only for leaves"));
        }
        node.deps = dep_ids.clone();
        Ok(json!({ "id": id, "deps": dep_ids }))
    })
}

/// tasks_set_default_model: Set the global default_model.
pub fn tasks_set_default_model(tasks_path: &Path, params: &Value) -> Result<Value, String> {
    let model = params
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or("Missing required param: model")?;

    with_tasks_file(tasks_path, |tf| {
        tf.default_model = Some(model.to_string());
        Ok(json!({ "default_model": model }))
    })
}

/// ask_user: Ask the user a question and capture their response.
///
/// # Arguments
///
/// * `params` - JSON object with:
///   - `question` (required): Question text to display to the user
///   - `options` (optional): Array of string options for user to choose from
///
/// # Returns
///
/// Placeholder JSON response with question echo and status.
///
/// # Note
///
/// This is a placeholder implementation. In Phase 5, this will be replaced
/// with SSE stream handler for real-time user interaction through TUI widgets.
pub fn ask_user(params: &Value) -> Result<Value, String> {
    let question = params
        .get("question")
        .and_then(|v| v.as_str())
        .ok_or("Missing required param: question")?;

    // Validate options if provided (must be array)
    let options = if let Some(opts) = params.get("options") {
        if !opts.is_array() {
            return Err("Parameter 'options' must be an array of strings".into());
        }
        opts.as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let multi_select = params
        .get("multiSelect")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Placeholder response - Phase 5 will implement SSE stream
    Ok(json!({
        "question": question,
        "options": options,
        "multiSelect": multi_select,
        "status": "not_implemented",
        "note": "SSE stream implementation pending in Phase 5"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn setup_test_file() -> (PathBuf, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.yml");
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929
tasks:
  - id: "1"
    name: "Epic 1"
    component: api
    subtasks:
      - id: "1.1"
        name: "Task A"
        status: todo
      - id: "1.2"
        name: "Task B"
        status: done
        deps: ["1.1"]
  - id: "2"
    name: "Task C"
    status: todo
    component: ui
"#;
        std::fs::write(&path, yaml).unwrap();
        (path, dir)
    }

    #[test]
    fn test_tasks_list_flat() {
        let (path, _dir) = setup_test_file();
        let result = tasks_list(&path, &json!({"format": "flat"})).unwrap();
        let tasks = result["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 3);
    }

    #[test]
    fn test_tasks_list_with_status_filter() {
        let (path, _dir) = setup_test_file();
        let result = tasks_list(&path, &json!({"format": "flat", "status": "todo"})).unwrap();
        let tasks = result["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 2); // 1.1 and 2
    }

    #[test]
    fn test_tasks_get() {
        let (path, _dir) = setup_test_file();
        let result = tasks_get(&path, &json!({"id": "1.1"})).unwrap();
        assert_eq!(result["name"], "Task A");
    }

    #[test]
    fn test_tasks_get_not_found() {
        let (path, _dir) = setup_test_file();
        let err = tasks_get(&path, &json!({"id": "999"})).unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_tasks_summary() {
        let (path, _dir) = setup_test_file();
        let result = tasks_summary(&path, &json!({})).unwrap();
        assert_eq!(result["total"], 3);
        assert_eq!(result["done"], 1);
        assert_eq!(result["todo"], 2);
    }

    #[test]
    fn test_tasks_create() {
        let (path, _dir) = setup_test_file();
        let result = tasks_create(
            &path,
            &json!({
                "parent_id": "1",
                "tasks": "- id: \"1.3\"\n  name: \"New Task\"\n  status: todo"
            }),
        )
        .unwrap();
        assert!(
            result["created"]
                .as_array()
                .unwrap()
                .contains(&json!("1.3"))
        );

        // Verify it was saved
        let tf = TasksFile::load(&path).unwrap();
        assert!(tf.find_node("1.3").is_some());
    }

    #[test]
    fn test_tasks_create_duplicate_id() {
        let (path, _dir) = setup_test_file();
        let err = tasks_create(
            &path,
            &json!({
                "tasks": "- id: \"1.1\"\n  name: \"Duplicate\"\n  status: todo"
            }),
        )
        .unwrap_err();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn test_tasks_update() {
        let (path, _dir) = setup_test_file();
        let result = tasks_update(
            &path,
            &json!({"id": "1.1", "name": "Updated Task", "status": "in_progress"}),
        )
        .unwrap();
        assert_eq!(result["updated"], "1.1");

        let tf = TasksFile::load(&path).unwrap();
        let node = tf.find_node("1.1").unwrap();
        assert_eq!(node.name, "Updated Task");
        assert_eq!(node.status, Some(TaskStatus::InProgress));
    }

    #[test]
    fn test_tasks_delete() {
        let (path, _dir) = setup_test_file();
        let result = tasks_delete(&path, &json!({"id": "2"})).unwrap();
        assert_eq!(result["deleted"], "2");

        let tf = TasksFile::load(&path).unwrap();
        assert!(tf.find_node("2").is_none());
    }

    #[test]
    fn test_tasks_delete_cleans_deps() {
        let (path, _dir) = setup_test_file();
        // Delete 1.1 which is a dep of 1.2
        tasks_delete(&path, &json!({"id": "1.1"})).unwrap();

        let tf = TasksFile::load(&path).unwrap();
        let node = tf.find_node("1.2").unwrap();
        assert!(node.deps.is_empty());
    }

    #[test]
    fn test_tasks_move() {
        let (path, _dir) = setup_test_file();
        // Move "2" under "1"
        let result = tasks_move(&path, &json!({"id": "2", "new_parent_id": "1"})).unwrap();
        assert_eq!(result["moved"], "2");

        let tf = TasksFile::load(&path).unwrap();
        let parent = tf.find_node("1").unwrap();
        assert!(parent.subtasks.iter().any(|n| n.id == "2"));
    }

    #[test]
    fn test_tasks_batch_status() {
        let (path, _dir) = setup_test_file();
        let result = tasks_batch_status(
            &path,
            &json!({
                "updates": [
                    {"id": "1.1", "status": "done"},
                    {"id": "2", "status": "in_progress"}
                ]
            }),
        )
        .unwrap();
        let updated = result["updated"].as_array().unwrap();
        assert_eq!(updated.len(), 2);
    }

    #[test]
    fn test_tasks_set_deps() {
        let (path, _dir) = setup_test_file();
        let result = tasks_set_deps(&path, &json!({"id": "2", "deps": ["1.1", "1.2"]})).unwrap();
        assert_eq!(result["id"], "2");

        let tf = TasksFile::load(&path).unwrap();
        let node = tf.find_node("2").unwrap();
        assert_eq!(node.deps, vec!["1.1", "1.2"]);
    }

    #[test]
    fn test_ask_user_with_question_only() {
        let params = json!({"question": "What is your name?"});
        let result = ask_user(&params).unwrap();
        assert_eq!(result["question"], "What is your name?");
        assert_eq!(result["options"].as_array().unwrap().len(), 0);
        assert_eq!(result["status"], "not_implemented");
    }

    #[test]
    fn test_ask_user_with_options() {
        let params = json!({
            "question": "Choose one",
            "options": ["A", "B", "C"]
        });
        let result = ask_user(&params).unwrap();
        assert_eq!(result["question"], "Choose one");
        let opts = result["options"].as_array().unwrap();
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[0], "A");
    }

    #[test]
    fn test_ask_user_missing_question() {
        let params = json!({"options": ["A", "B"]});
        let err = ask_user(&params).unwrap_err();
        assert!(err.contains("Missing required param: question"));
    }

    #[test]
    fn test_ask_user_options_not_array() {
        let params = json!({
            "question": "Test",
            "options": "invalid_string"
        });
        let err = ask_user(&params).unwrap_err();
        assert!(err.contains("must be an array"));
    }

    #[test]
    fn test_ask_user_options_with_non_string_values() {
        let params = json!({
            "question": "Test",
            "options": ["A", 123, null, "B"]
        });
        let result = ask_user(&params).unwrap();
        // Should filter out non-string values
        let opts = result["options"].as_array().unwrap();
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0], "A");
        assert_eq!(opts[1], "B");
    }

    #[test]
    fn test_tasks_set_default_model() {
        let (path, _dir) = setup_test_file();
        let result = tasks_set_default_model(&path, &json!({"model": "claude-opus-4-6"})).unwrap();
        assert_eq!(result["default_model"], "claude-opus-4-6");

        let tf = TasksFile::load(&path).unwrap();
        assert_eq!(tf.default_model.as_deref(), Some("claude-opus-4-6"));
    }
}
