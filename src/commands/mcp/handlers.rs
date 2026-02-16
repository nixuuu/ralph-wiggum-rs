use std::path::Path;

use serde_json::{Value, json};

use crate::shared::file_config::FileConfig;
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
    let mut tf = TasksFile::load_or_init(tasks_path).map_err(|e| e.to_string())?;
    let result = f(&mut tf)?;
    tf.validate()
        .map_err(|e| format!("Validation failed: {e}"))?;
    tf.save(tasks_path).map_err(|e| e.to_string())?;
    Ok(result)
}

// ── Query tools ────────────────────────────────────────────────────

/// tasks_list: List tasks with optional filters.
pub fn tasks_list(tasks_path: &Path, params: &Value) -> Result<Value, String> {
    let tf = TasksFile::load_or_init(tasks_path).map_err(|e| e.to_string())?;
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
                    let mut obj = json!({
                        "id": l.id,
                        "name": l.name,
                        "component": l.component,
                        "status": l.status,
                        "deps": l.deps,
                        "model": l.model,
                        "description": l.description,
                    });
                    // Include profiles only when non-empty (consistent with YAML skip_serializing_if)
                    if !l.profiles.is_empty() {
                        obj["profiles"] = json!(l.profiles);
                    }
                    obj
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

    let tf = TasksFile::load_or_init(tasks_path).map_err(|e| e.to_string())?;
    let node = tf
        .find_node(id)
        .ok_or_else(|| format!("Task not found: {id}"))?;

    serde_json::to_value(node).map_err(|e| e.to_string())
}

/// tasks_summary: Overview of task statuses.
pub fn tasks_summary(tasks_path: &Path, _params: &Value) -> Result<Value, String> {
    let tf = TasksFile::load_or_init(tasks_path).map_err(|e| e.to_string())?;
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
    let tf = TasksFile::load_or_init(tasks_path).map_err(|e| e.to_string())?;
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
        if let Some(profiles_val) = params.get("profiles") {
            if profiles_val.is_null() {
                node.profiles = Vec::new();
            } else if let Some(arr) = profiles_val.as_array() {
                node.profiles = arr
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

    // Walidacja: nie można przenieść taska pod siebie samego
    if let Some(parent_id) = new_parent_id
        && id == parent_id
    {
        return Err(format!(
            "Cannot move task {} under itself — this would create an invalid tree structure",
            id
        ));
    }

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

/// list_profiles: List verification profiles from .ralph.toml configuration.
///
/// # Arguments
///
/// * `config_path` - Path to .ralph.toml configuration file
/// * `_params` - Unused (no parameters required)
///
/// # Returns
///
/// JSON object with array of profiles containing:
/// - name: Profile name
/// - description: Optional profile description
/// - paths: Array of path patterns to monitor
/// - working_dir: Optional working directory for commands
/// - verify_commands: Array of verification command labels
/// - setup_commands: Array of setup command labels
///
/// # Errors
///
/// Returns error if .ralph.toml cannot be loaded or parsed.
pub fn list_profiles(config_path: &Path, _params: &Value) -> Result<Value, String> {
    // Load FileConfig from .ralph.toml
    let config = FileConfig::load_from_path(config_path).map_err(|e| e.to_string())?;

    // Convert profiles to JSON-serializable format
    let profiles: Vec<Value> = config
        .task
        .orchestrate
        .profiles
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "description": p.description,
                "paths": p.paths,
                "working_dir": p.working_dir,
                "verify_commands": p.verify_commands.iter().map(|cmd| cmd.label()).collect::<Vec<_>>(),
                "setup_commands": p.setup_commands.iter().map(|cmd| cmd.label()).collect::<Vec<_>>(),
            })
        })
        .collect();

    Ok(json!({
        "profiles": profiles
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
        // Task 52.5: tasks_create z duplikatem istniejącego ID
        let (path, _dir) = setup_test_file();

        // Precondition: task 1.1 exists
        let tf_before = TasksFile::load(&path).unwrap();
        assert!(tf_before.find_node("1.1").is_some());

        // Attempt to create a task with duplicate ID 1.1
        let err = tasks_create(
            &path,
            &json!({
                "parent_id": "1",
                "tasks": "- id: \"1.1\"\n  name: \"Duplicate Task\"\n  status: in_progress"
            }),
        )
        .unwrap_err();

        assert!(err.contains("already exists"), "got: {err}");
        assert!(err.contains("1.1"), "got: {err}");

        // Verify original task is unchanged (error didn't corrupt the file)
        let tf_after = TasksFile::load(&path).unwrap();
        let original = tf_after.find_node("1.1").unwrap();
        assert_eq!(original.name, "Task A");
        assert_eq!(original.status, Some(TaskStatus::Todo));
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
    fn test_tasks_move_to_self() {
        let (path, _dir) = setup_test_file();
        // Próba przeniesienia taska pod siebie samego (id == new_parent_id)
        let result = tasks_move(&path, &json!({"id": "1", "new_parent_id": "1"}));

        // Sprawdź że zwrócony został błąd
        assert!(
            result.is_err(),
            "Expected error when moving task under itself"
        );

        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("Cannot move task 1 under itself"),
            "Expected clear error message about moving task under itself, got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("invalid tree structure"),
            "Error message should mention invalid tree structure, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_tasks_move_to_root() {
        // Scenariusz: Tree: 1 → [1.1, 1.2]
        // Move 1.1 na root level (new_parent_id=null)
        // Po operacji: root=[1, 1.1], 1→[1.2]

        // Krok 1: Stwórz tree: parent 1 z subtasks 1.1, 1.2
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.yml");
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929
tasks:
  - id: "1"
    name: "Parent 1"
    component: api
    subtasks:
      - id: "1.1"
        name: "Task 1.1"
        status: todo
      - id: "1.2"
        name: "Task 1.2"
        status: todo
"#;
        std::fs::write(&path, yaml).unwrap();

        // Krok 2: Wywołaj tasks_move(1.1, new_parent_id=null)
        // W JSON-RPC, null jest reprezentowane przez brak klucza lub wartość null
        let result = tasks_move(&path, &json!({"id": "1.1", "new_parent_id": null})).unwrap();
        assert_eq!(result["moved"], "1.1");
        assert!(result["new_parent"].is_null());

        // Krok 3: Sprawdź że 1.1 jest teraz na root level
        let tf = TasksFile::load(&path).unwrap();

        // Sprawdź że root ma 2 nodes: 1 i 1.1
        assert_eq!(
            tf.tasks.len(),
            2,
            "Root level powinien mieć 2 nodes (1 i 1.1), ma: {}",
            tf.tasks.len()
        );

        // Sprawdź że 1.1 jest na root level
        let root_ids: Vec<&str> = tf.tasks.iter().map(|n| n.id.as_str()).collect();
        assert!(
            root_ids.contains(&"1.1"),
            "Task 1.1 powinien być na root level, root_ids: {:?}",
            root_ids
        );
        assert!(
            root_ids.contains(&"1"),
            "Task 1 powinien nadal być na root level, root_ids: {:?}",
            root_ids
        );

        // Krok 4: Sprawdź że 1 ma tylko subtask 1.2
        let parent = tf.find_node("1").unwrap();
        assert_eq!(
            parent.subtasks.len(),
            1,
            "Parent 1 powinien mieć 1 subtask (1.2), ma: {}",
            parent.subtasks.len()
        );
        assert_eq!(
            parent.subtasks[0].id, "1.2",
            "Jedyny subtask parenta 1 powinien być 1.2, jest: {}",
            parent.subtasks[0].id
        );

        // Dodatkowa weryfikacja: sprawdź że 1.1 nie ma subtasków
        let moved_node = tf.find_node("1.1").unwrap();
        assert_eq!(
            moved_node.subtasks.len(),
            0,
            "Przeniesiony task 1.1 nie powinien mieć subtasków"
        );
        assert_eq!(moved_node.status, Some(TaskStatus::Todo));
        assert_eq!(
            moved_node.name, "Task 1.1",
            "Nazwa taska powinna być zachowana po move"
        );
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

    // ── Tests for auto-init when file doesn't exist ────────────────────

    #[test]
    fn test_tasks_create_no_file() {
        // tasks_create should auto-initialize TasksFile when file doesn't exist
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.yml");

        // File doesn't exist yet
        assert!(!path.exists());

        // Create a task — should auto-init empty TasksFile
        let result = tasks_create(
            &path,
            &json!({
                "tasks": "- id: \"1\"\n  name: \"First Task\"\n  status: todo"
            }),
        )
        .unwrap();

        assert!(result["created"].as_array().unwrap().contains(&json!("1")));

        // Verify file was created and task was added
        let tf = TasksFile::load(&path).unwrap();
        assert!(tf.find_node("1").is_some());
        assert_eq!(
            tf.default_model.as_deref(),
            Some("claude-sonnet-4-5-20250929")
        );
    }

    #[test]
    fn test_tasks_list_no_file() {
        // tasks_list should return empty list when file doesn't exist
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.yml");

        // File doesn't exist yet
        assert!(!path.exists());

        // List tasks in flat format
        let result = tasks_list(&path, &json!({"format": "flat"})).unwrap();
        let tasks = result["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 0);

        // Verify file was created with empty task list
        assert!(path.exists());
        let tf = TasksFile::load(&path).unwrap();
        assert_eq!(tf.tasks.len(), 0);
        assert_eq!(
            tf.default_model.as_deref(),
            Some("claude-sonnet-4-5-20250929")
        );
    }

    #[test]
    fn test_tasks_summary_no_file() {
        // tasks_summary should return zero values when file doesn't exist
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.yml");

        // File doesn't exist yet
        assert!(!path.exists());

        // Get summary
        let result = tasks_summary(&path, &json!({})).unwrap();

        // Should return all zeros
        assert_eq!(result["total"], 0);
        assert_eq!(result["done"], 0);
        assert_eq!(result["in_progress"], 0);
        assert_eq!(result["todo"], 0);
        assert_eq!(result["blocked"], 0);
        assert_eq!(result["progress_percent"], 0.0);
        assert!(result["current_task"].is_null());
        assert_eq!(result["default_model"], "claude-sonnet-4-5-20250929");

        // Verify file was created
        assert!(path.exists());
        let tf = TasksFile::load(&path).unwrap();
        assert_eq!(tf.tasks.len(), 0);
    }

    // ── Tests for profiles support in MCP ────────────────────────────

    #[test]
    fn test_tasks_update_profiles() {
        let (path, _dir) = setup_test_file();
        let result = tasks_update(
            &path,
            &json!({"id": "1.1", "profiles": ["production", "staging"]}),
        )
        .unwrap();
        assert_eq!(result["updated"], "1.1");

        let tf = TasksFile::load(&path).unwrap();
        let node = tf.find_node("1.1").unwrap();
        assert_eq!(node.profiles, vec!["production", "staging"]);
    }

    #[test]
    fn test_tasks_update_profiles_clear_with_null() {
        let (path, _dir) = setup_test_file();
        // First set profiles
        tasks_update(&path, &json!({"id": "1.1", "profiles": ["production"]})).unwrap();

        // Then clear with null
        tasks_update(&path, &json!({"id": "1.1", "profiles": null})).unwrap();

        let tf = TasksFile::load(&path).unwrap();
        let node = tf.find_node("1.1").unwrap();
        assert!(node.profiles.is_empty());
    }

    #[test]
    fn test_tasks_list_flat_includes_profiles() {
        let (path, _dir) = setup_test_file();
        // Set profiles on task 1.1
        tasks_update(&path, &json!({"id": "1.1", "profiles": ["production"]})).unwrap();

        let result = tasks_list(&path, &json!({"format": "flat"})).unwrap();
        let tasks = result["tasks"].as_array().unwrap();

        // Task 1.1 should have profiles
        let task_11 = tasks.iter().find(|t| t["id"] == "1.1").unwrap();
        assert_eq!(task_11["profiles"], json!(["production"]));

        // Task 1.2 should not have profiles key (empty profiles omitted)
        let task_12 = tasks.iter().find(|t| t["id"] == "1.2").unwrap();
        assert!(task_12.get("profiles").is_none());
    }

    #[test]
    fn test_tasks_create_with_profiles() {
        let (path, _dir) = setup_test_file();
        let result = tasks_create(
            &path,
            &json!({
                "parent_id": "1",
                "tasks": "- id: \"1.3\"\n  name: \"New Task with Profiles\"\n  status: todo\n  profiles: [production, staging]"
            }),
        )
        .unwrap();
        assert!(
            result["created"]
                .as_array()
                .unwrap()
                .contains(&json!("1.3"))
        );

        // Verify profiles were saved
        let tf = TasksFile::load(&path).unwrap();
        let node = tf.find_node("1.3").unwrap();
        assert_eq!(node.profiles, vec!["production", "staging"]);
    }

    #[test]
    fn test_tasks_get_includes_profiles() {
        let (path, _dir) = setup_test_file();
        // Set profiles on task 1.1
        tasks_update(
            &path,
            &json!({"id": "1.1", "profiles": ["production", "dev"]}),
        )
        .unwrap();

        // Get task and verify profiles are included
        let result = tasks_get(&path, &json!({"id": "1.1"})).unwrap();
        assert_eq!(result["id"], "1.1");
        assert_eq!(result["profiles"], json!(["production", "dev"]));
    }

    #[test]
    fn test_tasks_get_omits_empty_profiles() {
        let (path, _dir) = setup_test_file();
        // Task 1.1 has no profiles by default
        let result = tasks_get(&path, &json!({"id": "1.1"})).unwrap();
        assert_eq!(result["id"], "1.1");
        // Empty profiles should be omitted from JSON (skip_serializing_if)
        assert!(result.get("profiles").is_none());
    }

    #[test]
    fn test_tasks_set_deps_cycle_detection() {
        // Setup: Stworzyć tasks.yml z A deps=[B], B bez deps
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.yml");
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929
tasks:
  - id: "A"
    name: "Task A"
    status: todo
    deps: ["B"]
  - id: "B"
    name: "Task B"
    status: todo
    deps: []
"#;
        std::fs::write(&path, yaml).unwrap();

        // Zapisz oryginalną zawartość pliku do porównania
        let original_content = std::fs::read_to_string(&path).unwrap();

        // Wywołaj tasks_set_deps(B, [A]) — to powinno stworzyć cykl A→B→A
        let result = tasks_set_deps(&path, &json!({"id": "B", "deps": ["A"]}));

        // Sprawdź że zwrócony został błąd
        assert!(
            result.is_err(),
            "Expected error when creating dependency cycle"
        );

        let err_msg = result.unwrap_err();
        // Sprawdź że błąd zawiera informację o cyklu
        assert!(
            err_msg.contains("Validation failed"),
            "Expected validation failure, got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("cycle"),
            "Expected cycle detection error, got: {}",
            err_msg
        );

        // Sprawdź że plik nie został zmieniony (rollback)
        let current_content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            current_content, original_content,
            "File should not be modified when validation fails (rollback should occur)"
        );

        // Dodatkowa weryfikacja: sprawdź że B nadal ma puste deps
        let tf = TasksFile::load(&path).unwrap();
        let node_b = tf.find_node("B").unwrap();
        assert!(
            node_b.deps.is_empty(),
            "Task B deps should remain empty after failed update"
        );
    }

    #[test]
    fn test_tasks_update_profiles_with_empty_array() {
        let (path, _dir) = setup_test_file();
        // First set profiles
        tasks_update(&path, &json!({"id": "1.1", "profiles": ["production"]})).unwrap();

        // Then clear with empty array
        tasks_update(&path, &json!({"id": "1.1", "profiles": []})).unwrap();

        let tf = TasksFile::load(&path).unwrap();
        let node = tf.find_node("1.1").unwrap();
        assert!(node.profiles.is_empty());
    }

    // ── list_profiles tests ───────────────────────────────────────────

    #[test]
    fn test_list_profiles_empty() {
        // Test with .ralph.toml that has no profiles
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ralph.toml");
        let toml_content = r#"
[task.orchestrate]
workers = 2
"#;
        std::fs::write(&config_path, toml_content).unwrap();

        let result = list_profiles(&config_path, &json!({})).unwrap();
        let profiles = result["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 0);
    }

    #[test]
    fn test_list_profiles_single_profile() {
        // Test with one profile
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ralph.toml");
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "frontend"
description = "Frontend tests"
paths = ["src/ui/**/*.rs", "assets/**/*"]
working_dir = "frontend"
verify_commands = ["npm test", "npm run lint"]
setup_commands = ["npm install"]
"#;
        std::fs::write(&config_path, toml_content).unwrap();

        let result = list_profiles(&config_path, &json!({})).unwrap();
        let profiles = result["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 1);

        let profile = &profiles[0];
        assert_eq!(profile["name"], "frontend");
        assert_eq!(profile["description"], "Frontend tests");
        assert_eq!(profile["paths"], json!(["src/ui/**/*.rs", "assets/**/*"]));
        assert_eq!(profile["working_dir"], "frontend");
        assert_eq!(
            profile["verify_commands"],
            json!(["npm test", "npm run lint"])
        );
        assert_eq!(profile["setup_commands"], json!(["npm install"]));
    }

    #[test]
    fn test_list_profiles_multiple_profiles() {
        // Test with multiple profiles
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ralph.toml");
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "backend"
paths = ["src/api/**/*.rs"]
verify_commands = ["cargo test --package api"]

[[task.orchestrate.profiles]]
name = "database"
description = "Database migrations"
paths = ["migrations/**/*.sql"]
verify_commands = ["sqlx migrate run"]
setup_commands = ["sqlx database create"]
"#;
        std::fs::write(&config_path, toml_content).unwrap();

        let result = list_profiles(&config_path, &json!({})).unwrap();
        let profiles = result["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 2);

        // Check backend profile
        let backend = &profiles[0];
        assert_eq!(backend["name"], "backend");
        assert!(backend["description"].is_null());
        assert_eq!(backend["paths"], json!(["src/api/**/*.rs"]));
        assert!(backend["working_dir"].is_null());
        assert_eq!(
            backend["verify_commands"],
            json!(["cargo test --package api"])
        );
        assert_eq!(backend["setup_commands"], json!([]));

        // Check database profile
        let database = &profiles[1];
        assert_eq!(database["name"], "database");
        assert_eq!(database["description"], "Database migrations");
        assert_eq!(database["paths"], json!(["migrations/**/*.sql"]));
        assert_eq!(database["verify_commands"], json!(["sqlx migrate run"]));
        assert_eq!(database["setup_commands"], json!(["sqlx database create"]));
    }

    #[test]
    fn test_list_profiles_with_detailed_commands() {
        // Test with profiles using detailed command format
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ralph.toml");
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "mixed"
paths = ["**/*.rs"]
verify_commands = [
    "cargo fmt --check",
    { command = "cargo test", name = "Tests", description = "Run test suite" }
]
setup_commands = [
    "cargo build",
    { run = "cp .env.example .env", name = "Setup env" }
]
"#;
        std::fs::write(&config_path, toml_content).unwrap();

        let result = list_profiles(&config_path, &json!({})).unwrap();
        let profiles = result["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 1);

        let profile = &profiles[0];
        assert_eq!(profile["name"], "mixed");
        // Labels should use name when available, otherwise command
        assert_eq!(
            profile["verify_commands"],
            json!(["cargo fmt --check", "Tests"])
        );
        assert_eq!(
            profile["setup_commands"],
            json!(["cargo build", "Setup env"])
        );
    }

    #[test]
    fn test_list_profiles_minimal_profile() {
        // Test with minimal profile (only name)
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ralph.toml");
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "minimal"
"#;
        std::fs::write(&config_path, toml_content).unwrap();

        let result = list_profiles(&config_path, &json!({})).unwrap();
        let profiles = result["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 1);

        let profile = &profiles[0];
        assert_eq!(profile["name"], "minimal");
        assert!(profile["description"].is_null());
        assert_eq!(profile["paths"], json!([]));
        assert!(profile["working_dir"].is_null());
        assert_eq!(profile["verify_commands"], json!([]));
        assert_eq!(profile["setup_commands"], json!([]));
    }

    #[test]
    fn test_list_profiles_nonexistent_file() {
        // Test with nonexistent .ralph.toml (should return default config with empty profiles)
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ralph.toml");

        let result = list_profiles(&config_path, &json!({})).unwrap();
        let profiles = result["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 0);
    }

    #[test]
    fn test_list_profiles_invalid_toml() {
        // Test with invalid TOML file
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ralph.toml");
        let toml_content = "invalid toml [[[";
        std::fs::write(&config_path, toml_content).unwrap();

        let err = list_profiles(&config_path, &json!({})).unwrap_err();
        assert!(err.contains("Failed to parse"));
    }

    // ── Additional integration tests for profiles ──────────────────────

    #[test]
    fn test_tasks_create_multiple_tasks_with_different_profiles() {
        // Test creating multiple tasks with different profile configurations
        let (path, _dir) = setup_test_file();
        let result = tasks_create(
            &path,
            &json!({
                "parent_id": "1",
                "tasks": r#"
- id: "1.3"
  name: "Task with one profile"
  status: todo
  profiles: [production]
- id: "1.4"
  name: "Task with multiple profiles"
  status: todo
  profiles: [production, staging, dev]
- id: "1.5"
  name: "Task without profiles"
  status: todo
"#
            }),
        )
        .unwrap();
        assert_eq!(result["created"].as_array().unwrap().len(), 3);

        // Verify each task has correct profiles
        let tf = TasksFile::load(&path).unwrap();
        let task_13 = tf.find_node("1.3").unwrap();
        assert_eq!(task_13.profiles, vec!["production"]);
        let task_14 = tf.find_node("1.4").unwrap();
        assert_eq!(task_14.profiles, vec!["production", "staging", "dev"]);
        let task_15 = tf.find_node("1.5").unwrap();
        assert!(task_15.profiles.is_empty());
    }

    #[test]
    fn test_tasks_update_profiles_replaces_existing() {
        // Test that updating profiles replaces (not merges) existing profiles
        let (path, _dir) = setup_test_file();
        // Set initial profiles
        tasks_update(
            &path,
            &json!({"id": "1.1", "profiles": ["production", "staging"]}),
        )
        .unwrap();

        // Update with different profiles (should replace, not merge)
        tasks_update(&path, &json!({"id": "1.1", "profiles": ["dev"]})).unwrap();

        let tf = TasksFile::load(&path).unwrap();
        let node = tf.find_node("1.1").unwrap();
        assert_eq!(node.profiles, vec!["dev"]);
    }

    #[test]
    fn test_tasks_update_profiles_preserves_other_fields() {
        // Test that updating profiles doesn't affect other fields
        let (path, _dir) = setup_test_file();
        let node_before = {
            let tf = TasksFile::load(&path).unwrap();
            tf.find_node("1.1").unwrap().clone()
        };

        // Update only profiles
        tasks_update(&path, &json!({"id": "1.1", "profiles": ["production"]})).unwrap();

        let tf = TasksFile::load(&path).unwrap();
        let node_after = tf.find_node("1.1").unwrap();
        // Check that other fields are unchanged
        assert_eq!(node_after.name, node_before.name);
        assert_eq!(node_after.status, node_before.status);
        assert_eq!(node_after.component, node_before.component);
        assert_eq!(node_after.deps, node_before.deps);
        // But profiles should be updated
        assert_eq!(node_after.profiles, vec!["production"]);
    }

    #[test]
    fn test_tasks_list_flat_omits_empty_profiles() {
        // Test that flat list omits profiles field when empty (consistent with YAML)
        let (path, _dir) = setup_test_file();
        // Set profiles on one task only
        tasks_update(&path, &json!({"id": "1.1", "profiles": ["production"]})).unwrap();

        let result = tasks_list(&path, &json!({"format": "flat"})).unwrap();
        let tasks = result["tasks"].as_array().unwrap();

        // Task 1.1 should have profiles
        let task_11 = tasks.iter().find(|t| t["id"] == "1.1").unwrap();
        assert!(task_11.get("profiles").is_some());

        // Tasks 1.2 and 2 should NOT have profiles key
        let task_12 = tasks.iter().find(|t| t["id"] == "1.2").unwrap();
        assert!(task_12.get("profiles").is_none());
        let task_2 = tasks.iter().find(|t| t["id"] == "2").unwrap();
        assert!(task_2.get("profiles").is_none());
    }

    #[test]
    fn test_tasks_update_profiles_with_special_characters() {
        // Test profiles with special characters in names
        let (path, _dir) = setup_test_file();
        let result = tasks_update(
            &path,
            &json!({"id": "1.1", "profiles": ["prod-us-west-1", "staging_env", "dev.local"]}),
        )
        .unwrap();
        assert_eq!(result["updated"], "1.1");

        let tf = TasksFile::load(&path).unwrap();
        let node = tf.find_node("1.1").unwrap();
        assert_eq!(
            node.profiles,
            vec!["prod-us-west-1", "staging_env", "dev.local"]
        );
    }

    #[test]
    fn test_tasks_create_parent_with_profiles_in_subtasks() {
        // Test creating parent task where only subtasks have profiles (not parent)
        let (path, _dir) = setup_test_file();
        let result = tasks_create(
            &path,
            &json!({
                "tasks": r#"
- id: "3"
  name: "Parent without profiles"
  subtasks:
    - id: "3.1"
      name: "Child with profiles"
      status: todo
      profiles: [frontend]
    - id: "3.2"
      name: "Child without profiles"
      status: todo
"#
            }),
        )
        .unwrap();
        assert_eq!(result["created"].as_array().unwrap().len(), 3);

        // Verify structure
        let tf = TasksFile::load(&path).unwrap();
        let parent = tf.find_node("3").unwrap();
        assert!(parent.profiles.is_empty());
        let child_31 = tf.find_node("3.1").unwrap();
        assert_eq!(child_31.profiles, vec!["frontend"]);
        let child_32 = tf.find_node("3.2").unwrap();
        assert!(child_32.profiles.is_empty());
    }

    #[test]
    fn test_tasks_update_profiles_non_array_value() {
        // Test that updating profiles with non-array value fails gracefully
        let (path, _dir) = setup_test_file();

        // Try updating with string instead of array - should not panic, but not update
        let result = tasks_update(&path, &json!({"id": "1.1", "profiles": "production"}));

        // Should succeed (profiles field ignored if not array or null)
        assert!(result.is_ok());

        // Verify profiles unchanged
        let tf = TasksFile::load(&path).unwrap();
        let node = tf.find_node("1.1").unwrap();
        assert!(node.profiles.is_empty());
    }

    #[test]
    fn test_tasks_update_profiles_array_with_non_strings() {
        // Test that profiles array with non-string values filters them out
        let (path, _dir) = setup_test_file();
        let result = tasks_update(
            &path,
            &json!({"id": "1.1", "profiles": ["production", 123, null, "staging", true]}),
        )
        .unwrap();
        assert_eq!(result["updated"], "1.1");

        // Should only keep string values
        let tf = TasksFile::load(&path).unwrap();
        let node = tf.find_node("1.1").unwrap();
        assert_eq!(node.profiles, vec!["production", "staging"]);
    }

    #[test]
    fn test_tasks_batch_status_partial_failure_atomicity() {
        // Test: batch_update_statuses z [valid_update, invalid_update]
        // Cała operacja powinna fail — pierwszy valid update NIE jest zapisany.
        let (path, _dir) = setup_test_file();

        // Sprawdź początkowy stan (task 1.1 ma status: todo)
        {
            let tf = TasksFile::load(&path).unwrap();
            let node = tf.find_node("1.1").unwrap();
            assert_eq!(node.status, Some(TaskStatus::Todo));
        }

        // Wywołaj batch_update z [(1.1, done), (999, done)]
        // 1.1 istnieje, 999 nie istnieje
        let result = tasks_batch_status(
            &path,
            &json!({
                "updates": [
                    {"id": "1.1", "status": "done"},
                    {"id": "999", "status": "done"}
                ]
            }),
        );

        // Sprawdź że zwrócony został error (999 not found)
        assert!(result.is_err(), "Expected error for non-existent task 999");
        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("999"),
            "Error should mention task 999, got: {}",
            err_msg
        );

        // Odczytaj plik ponownie — 1.1 nadal todo (NIE zapisano partial update)
        {
            let tf = TasksFile::load(&path).unwrap();
            let node = tf.find_node("1.1").unwrap();
            assert_eq!(
                node.status,
                Some(TaskStatus::Todo),
                "Task 1.1 should remain Todo — partial update should NOT be saved"
            );
        }
    }

    #[test]
    fn test_list_profiles_preserves_order() {
        // Test that list_profiles returns profiles in config order
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ralph.toml");
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "first"
paths = ["**/*.rs"]

[[task.orchestrate.profiles]]
name = "second"
paths = ["**/*.ts"]

[[task.orchestrate.profiles]]
name = "third"
paths = ["**/*.py"]
"#;
        std::fs::write(&config_path, toml_content).unwrap();

        let result = list_profiles(&config_path, &json!({})).unwrap();
        let profiles = result["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 3);
        assert_eq!(profiles[0]["name"], "first");
        assert_eq!(profiles[1]["name"], "second");
        assert_eq!(profiles[2]["name"], "third");
    }

    #[test]
    fn test_tasks_get_with_profiles_returns_full_node() {
        // Test that tasks_get returns complete node structure with profiles
        let (path, _dir) = setup_test_file();
        tasks_update(
            &path,
            &json!({
                "id": "1.1",
                "profiles": ["prod"],
                "description": "Test description",
                "related_files": ["test.rs"]
            }),
        )
        .unwrap();

        let result = tasks_get(&path, &json!({"id": "1.1"})).unwrap();

        // Verify all fields present
        assert_eq!(result["id"], "1.1");
        assert_eq!(result["name"], "Task A");
        assert_eq!(result["status"], "todo");
        assert_eq!(result["profiles"], json!(["prod"]));
        assert_eq!(result["description"], "Test description");
        assert_eq!(result["related_files"], json!(["test.rs"]));
    }

    #[test]
    fn test_tasks_update_with_unknown_field() {
        // Test that tasks_update silently ignores unknown fields without error.
        // Unknown fields should not cause failure or modify the task.
        let (path, _dir) = setup_test_file();

        // Store original state of task 1.1
        let tf_before = TasksFile::load(&path).unwrap();
        let node_before = tf_before.find_node("1.1").unwrap().clone();

        // Update task 1.1 with an unknown_field parameter
        let result = tasks_update(
            &path,
            &json!({"id": "1.1", "unknown_field": "some_value", "name": "Updated Name"}),
        );

        // Should succeed without error
        assert!(
            result.is_ok(),
            "tasks_update should not error on unknown fields"
        );
        assert_eq!(result.unwrap()["updated"], "1.1");

        // Verify that the known field (name) was updated
        let tf_after = TasksFile::load(&path).unwrap();
        let node_after = tf_after.find_node("1.1").unwrap();
        assert_eq!(node_after.name, "Updated Name");

        // Verify that other fields were not affected (status, component, deps, etc.)
        assert_eq!(node_after.status, node_before.status);
        assert_eq!(node_after.component, node_before.component);
        assert_eq!(node_after.deps, node_before.deps);
        assert_eq!(node_after.model, node_before.model);
        assert_eq!(node_after.description, node_before.description);
        assert_eq!(node_after.profiles, node_before.profiles);
    }

    #[test]
    fn test_tasks_update_with_multiple_unknown_fields() {
        // Test that tasks_update ignores multiple unknown fields simultaneously.
        let (path, _dir) = setup_test_file();

        // Store original state
        let tf_before = TasksFile::load(&path).unwrap();
        let node_before = tf_before.find_node("1.1").unwrap().clone();

        // Update with multiple unknown fields
        let result = tasks_update(
            &path,
            &json!({
                "id": "1.1",
                "unknown_field_1": "value1",
                "unknown_field_2": 123,
                "unknown_field_3": true,
                "status": "done"
            }),
        );

        // Should succeed without error
        assert!(
            result.is_ok(),
            "tasks_update should not error on multiple unknown fields"
        );

        // Verify known field was updated
        let tf_after = TasksFile::load(&path).unwrap();
        let node_after = tf_after.find_node("1.1").unwrap();
        assert_eq!(node_after.status, Some(TaskStatus::Done));

        // Verify other fields unchanged
        assert_eq!(node_after.name, node_before.name);
        assert_eq!(node_after.component, node_before.component);
        assert_eq!(node_after.deps, node_before.deps);
    }

    #[test]
    fn test_tasks_update_only_unknown_fields() {
        // Test that tasks_update succeeds even when params contain only unknown fields.
        let (path, _dir) = setup_test_file();

        // Store original state
        let tf_before = TasksFile::load(&path).unwrap();
        let node_before = tf_before.find_node("1.1").unwrap().clone();

        // Update with only unknown fields (no known fields)
        let result = tasks_update(
            &path,
            &json!({
                "id": "1.1",
                "custom_attr": "custom_value",
                "proprietary_field": ["array", "of", "values"]
            }),
        );

        // Should succeed without error
        assert!(
            result.is_ok(),
            "tasks_update should not error when only unknown fields are provided"
        );
        assert_eq!(result.unwrap()["updated"], "1.1");

        // Verify task was not modified at all
        let tf_after = TasksFile::load(&path).unwrap();
        let node_after = tf_after.find_node("1.1").unwrap();
        assert_eq!(node_after.name, node_before.name);
        assert_eq!(node_after.status, node_before.status);
        assert_eq!(node_after.component, node_before.component);
        assert_eq!(node_after.deps, node_before.deps);
        assert_eq!(node_after.model, node_before.model);
        assert_eq!(node_after.description, node_before.description);
        assert_eq!(node_after.profiles, node_before.profiles);
    }

    #[test]
    fn test_tasks_update_status_on_parent_node() {
        // Test: Próba ustawienia status na parent node (ma subtasks) przez MCP.
        // Walidacja powinna to odrzucić.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.yml");

        // Stwórz tree: parent 1 z subtasks 1.1, 1.2
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929
tasks:
  - id: "1"
    name: "Parent task"
    component: tests
    subtasks:
      - id: "1.1"
        name: "Child task 1"
        status: todo
      - id: "1.2"
        name: "Child task 2"
        status: todo
"#;
        std::fs::write(&path, yaml).unwrap();

        // Sprawdź że parent nie ma status
        {
            let tf = TasksFile::load(&path).unwrap();
            let parent = tf.find_node("1").unwrap();
            assert!(
                parent.status.is_none(),
                "Parent should not have status initially"
            );
            assert!(!parent.is_leaf(), "Task 1 should be a parent node");
        }

        // Wywołaj tasks_update(id=1, status=done)
        let result = tasks_update(&path, &json!({"id": "1", "status": "done"}));

        // Sprawdź error walidacji (parent nie może mieć status)
        assert!(
            result.is_err(),
            "Expected error when setting status on parent node"
        );

        let err_msg = result.unwrap_err();
        // Sprawdź że błąd zawiera informację o walidacji i statusie
        assert!(
            err_msg.contains("Validation failed"),
            "Expected validation failure, got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("status"),
            "Error should mention status, got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("Task 1"),
            "Error should mention task ID 1, got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("leaves"),
            "Error should mention leaves, got: {}",
            err_msg
        );

        // Sprawdź że plik nie został zmieniony (rollback)
        {
            let tf = TasksFile::load(&path).unwrap();
            let parent = tf.find_node("1").unwrap();
            assert!(
                parent.status.is_none(),
                "Parent should still not have status after failed update (rollback)"
            );
        }
    }

    #[test]
    fn test_with_tasks_file_validation_failure_rollback() {
        // Test: with_tasks_file — mutacja powodująca validation failure.
        // Zmiany powinny być odrzucone (nie zapisane).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.yml");

        // Stwórz poprawne drzewo: A deps=[B], B bez deps
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929
tasks:
  - id: "A"
    name: "Task A"
    status: todo
    deps: ["B"]
  - id: "B"
    name: "Task B"
    status: todo
    deps: []
"#;
        std::fs::write(&path, yaml).unwrap();

        // Zapisz oryginalną zawartość do porównania
        let original_content = std::fs::read_to_string(&path).unwrap();

        // Sprawdź początkowy stan
        {
            let tf = TasksFile::load(&path).unwrap();
            let node_a = tf.find_node("A").unwrap();
            let node_b = tf.find_node("B").unwrap();
            assert_eq!(node_a.deps, vec!["B"]);
            assert!(node_b.deps.is_empty());
        }

        // Wywołaj with_tasks_file z mutacją tworzącą cykl: B.deps = ["A"]
        // To powinno stworzyć cykl A→B→A
        let result = with_tasks_file(&path, |tf| {
            let node_b = tf
                .find_node_mut("B")
                .ok_or_else(|| "Task B not found".to_string())?;
            node_b.deps = vec!["A".to_string()];
            Ok(json!({"mutated": "B"}))
        });

        // Sprawdź że zwrócony został błąd walidacji
        assert!(
            result.is_err(),
            "Expected validation error when creating dependency cycle"
        );

        let err_msg = result.unwrap_err();
        // Sprawdź że błąd zawiera informację o walidacji i cyklu
        assert!(
            err_msg.contains("Validation failed"),
            "Expected validation failure, got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("cycle"),
            "Expected cycle detection error, got: {}",
            err_msg
        );

        // Sprawdź że plik NIE został zmieniony (rollback)
        let current_content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            current_content, original_content,
            "File should not be modified when validation fails — rollback should occur"
        );

        // Dodatkowa weryfikacja: sprawdź że B nadal ma puste deps
        {
            let tf = TasksFile::load(&path).unwrap();
            let node_b = tf.find_node("B").unwrap();
            assert!(
                node_b.deps.is_empty(),
                "Task B deps should remain empty after failed mutation (rollback)"
            );
            // Sprawdź że A nadal ma deps=[B]
            let node_a = tf.find_node("A").unwrap();
            assert_eq!(
                node_a.deps,
                vec!["B"],
                "Task A deps should remain unchanged after failed mutation (rollback)"
            );
        }
    }
}
