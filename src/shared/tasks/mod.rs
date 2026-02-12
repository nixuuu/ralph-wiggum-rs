mod helpers;
mod node;
mod tree_ops;
mod validation;

pub use helpers::{format_task_prompt, resolve_model_alias, reverse_model_alias};
pub use node::{LeafTask, TaskNode};

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::shared::error::{RalphError, Result};
use crate::shared::progress::{ProgressFrontmatter, ProgressSummary, ProgressTask, TaskStatus};

// ── Types ───────────────────────────────────────────────────────────

/// Root structure for `.ralph/tasks.yml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TasksFile {
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub tasks: Vec<TaskNode>,
}

// ── TasksFile ───────────────────────────────────────────────────────

impl TasksFile {
    /// Load and validate from a YAML file path.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| RalphError::MissingFile(format!("{}: {}", path.display(), e)))?;
        let tf: Self = serde_yaml::from_str(&content)?;
        tf.validate()?;
        Ok(tf)
    }

    /// Atomic save: write to .tmp then rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        let yaml = serde_yaml::to_string(self)?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let tmp_path = path.with_extension("yml.tmp");
        std::fs::write(&tmp_path, &yaml).map_err(|e| {
            RalphError::Orchestrate(format!("Failed to write {}: {e}", tmp_path.display()))
        })?;
        std::fs::rename(&tmp_path, path).map_err(|e| {
            RalphError::Orchestrate(format!("Failed to rename to {}: {e}", path.display()))
        })?;
        Ok(())
    }

    /// Validate the tree structure.
    pub fn validate(&self) -> Result<()> {
        validation::validate(&self.tasks)
    }

    /// Recursively collect all leaf tasks.
    pub fn flatten_leaves(&self) -> Vec<LeafTask> {
        let mut leaves = Vec::new();
        for node in &self.tasks {
            Self::collect_leaves(node, None, &mut leaves);
        }
        leaves
    }

    fn collect_leaves(node: &TaskNode, parent_component: Option<&str>, out: &mut Vec<LeafTask>) {
        let component = node
            .component
            .as_deref()
            .or(parent_component)
            .unwrap_or("general");

        if node.is_leaf() {
            // Clone overhead: 9 fields copied to create owned LeafTask.
            // Alternative would be LeafTask<'a> with borrowed fields, but that
            // complicates the API for all consumers. These clones happen only
            // during task collection (not in hot paths), so the trade-off favors
            // simpler API over micro-optimization.
            out.push(LeafTask {
                id: node.id.clone(),
                name: node.name.clone(),
                component: component.to_string(),
                status: node.status.clone().unwrap_or(TaskStatus::Todo),
                deps: node.deps.clone(),
                model: node.model.clone(),
                description: node.description.clone(),
                related_files: node.related_files.clone(),
                implementation_steps: node.implementation_steps.clone(),
            });
        } else {
            for child in &node.subtasks {
                Self::collect_leaves(child, Some(component), out);
            }
        }
    }

    /// Convert to ProgressSummary for backward-compatible consumers.
    pub fn to_summary(&self) -> ProgressSummary {
        let leaves = self.flatten_leaves();
        let mut done = 0;
        let mut in_progress = 0;
        let mut blocked = 0;
        let mut todo = 0;

        // Clone overhead: 4 fields per leaf cloned to create ProgressTask.
        // Could be avoided with ProgressTask<'a>, but that would break
        // backward compatibility with existing consumers expecting owned data.
        let tasks: Vec<ProgressTask> = leaves
            .iter()
            .map(|leaf| {
                match leaf.status {
                    TaskStatus::Done => done += 1,
                    TaskStatus::InProgress => in_progress += 1,
                    TaskStatus::Blocked => blocked += 1,
                    TaskStatus::Todo => todo += 1,
                }
                ProgressTask {
                    id: leaf.id.clone(),
                    component: leaf.component.clone(),
                    name: leaf.name.clone(),
                    status: leaf.status.clone(),
                }
            })
            .collect();

        // Build frontmatter equivalent for consumers that need it
        let frontmatter = ProgressFrontmatter {
            deps: self.deps_map(),
            models: self.models_map(),
            default_model: self.default_model.clone(),
        };

        ProgressSummary {
            tasks,
            done,
            in_progress,
            blocked,
            todo,
            frontmatter: Some(frontmatter),
        }
    }

    /// Extract deps map from leaves: task_id → Vec<dep_ids>.
    pub fn deps_map(&self) -> HashMap<String, Vec<String>> {
        let mut map = HashMap::new();
        Self::collect_deps_map(&self.tasks, &mut map);
        map
    }

    fn collect_deps_map(nodes: &[TaskNode], map: &mut HashMap<String, Vec<String>>) {
        for node in nodes {
            if node.is_leaf() {
                map.insert(node.id.clone(), node.deps.clone());
            } else {
                Self::collect_deps_map(&node.subtasks, map);
            }
        }
    }

    /// Extract model overrides from leaves: task_id → model.
    pub fn models_map(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        Self::collect_models_map(&self.tasks, &mut map);
        map
    }

    fn collect_models_map(nodes: &[TaskNode], map: &mut HashMap<String, String>) {
        for node in nodes {
            if node.is_leaf() {
                if let Some(model) = &node.model {
                    map.insert(node.id.clone(), model.clone());
                }
            } else {
                Self::collect_models_map(&node.subtasks, map);
            }
        }
    }

    /// Update the status of a specific leaf task by ID. Returns true if found.
    #[must_use]
    pub fn update_status(&mut self, task_id: &str, new_status: TaskStatus) -> bool {
        for node in &mut self.tasks {
            if Self::update_node_status(node, task_id, &new_status) {
                return true;
            }
        }
        false
    }

    fn update_node_status(node: &mut TaskNode, task_id: &str, new_status: &TaskStatus) -> bool {
        if node.id == task_id && node.is_leaf() {
            // Required clone: TaskStatus doesn't impl Copy, must clone for Option::Some
            node.status = Some(new_status.clone());
            return true;
        }
        for child in &mut node.subtasks {
            if Self::update_node_status(child, task_id, new_status) {
                return true;
            }
        }
        false
    }

    /// Load, apply multiple status updates, save.
    /// Returns an error if any task ID is not found.
    pub fn batch_update_statuses(path: &Path, updates: &[(String, TaskStatus)]) -> Result<()> {
        let mut tf = Self::load(path)?;
        for (id, status) in updates {
            // Required clone: update_status takes owned TaskStatus
            if !tf.update_status(id, status.clone()) {
                // Required clone: RalphError::TaskNotFound takes owned String
                return Err(RalphError::TaskNotFound(id.clone()));
            }
        }
        tf.save(path)
    }

    /// Find a specific leaf task by ID.
    pub fn find_leaf(&self, task_id: &str) -> Option<LeafTask> {
        self.flatten_leaves().into_iter().find(|l| l.id == task_id)
    }

    /// Find a node (leaf or parent) by ID (immutable).
    pub fn find_node(&self, id: &str) -> Option<&TaskNode> {
        tree_ops::find_node(&self.tasks, id)
    }

    /// Find a node (leaf or parent) by ID (mutable).
    pub fn find_node_mut(&mut self, id: &str) -> Option<&mut TaskNode> {
        tree_ops::find_node_mut(&mut self.tasks, id)
    }

    /// Add a task node under a parent (or at root if parent_id is None).
    /// Optionally insert at a specific position index.
    pub fn add_task(
        &mut self,
        parent_id: Option<&str>,
        node: TaskNode,
        position: Option<usize>,
    ) -> Result<()> {
        tree_ops::add_task(&mut self.tasks, parent_id, node, position)
    }

    /// Remove a task (and its subtasks) by ID. Returns the removed node.
    /// Also cleans up dep references pointing to the removed task.
    pub fn remove_task(&mut self, id: &str) -> Option<TaskNode> {
        tree_ops::remove_task(&mut self.tasks, id)
    }

    /// Collect all task IDs in the tree.
    pub fn all_ids(&self) -> HashSet<String> {
        tree_ops::all_ids(&self.tasks)
    }

    /// Get the current task: first in_progress, fallback to first todo.
    pub fn current_task(&self) -> Option<LeafTask> {
        let leaves = self.flatten_leaves();
        leaves
            .iter()
            .find(|t| t.status == TaskStatus::InProgress)
            .or_else(|| leaves.iter().find(|t| t.status == TaskStatus::Todo))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_yaml() -> &'static str {
        r#"
default_model: claude-sonnet-4-5-20250929

tasks:
  - id: "1"
    name: "Epic 1: YAML Parser"
    component: parser
    subtasks:
      - id: "1.1"
        name: "Frontmatter Parsing"
        component: progress
        subtasks:
          - id: "1.1.1"
            name: "Define ProgressFrontmatter struct"
            status: done
            component: progress
          - id: "1.1.2"
            name: "Implement parse_frontmatter()"
            status: todo
            component: progress
            deps: ["1.1.1"]
            model: claude-opus-4-6
            description: "Parse YAML frontmatter from PROGRESS.md including deps, models, and default_model fields"
            related_files:
              - "src/shared/progress.rs"
              - "docs/frontmatter-spec.md"
            implementation_steps:
              - "Add serde structs for frontmatter"
              - "Implement YAML parser"
              - "Add validation logic"
      - id: "1.2"
        name: "Task Line Parsing"
        component: progress
        subtasks:
          - id: "1.2.1"
            name: "Parse task line format"
            status: in_progress
          - id: "1.2.2"
            name: "Handle backtick components"
            status: todo
            deps: ["1.2.1"]
  - id: "2"
    name: "Epic 2: DAG"
    component: dag
    subtasks:
      - id: "2.1"
        name: "Cycle detection"
        status: done
      - id: "2.2"
        name: "Topological sort"
        status: blocked
        deps: ["2.1", "1.1.2"]
"#
    }

    #[test]
    fn test_deserialize_roundtrip() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        assert_eq!(
            tf.default_model.as_deref(),
            Some("claude-sonnet-4-5-20250929")
        );
        assert_eq!(tf.tasks.len(), 2);

        // Serialize and re-parse
        let yaml = serde_yaml::to_string(&tf).unwrap();
        let tf2: TasksFile = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(tf2.tasks.len(), 2);
    }

    #[test]
    fn test_flatten_leaves() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let leaves = tf.flatten_leaves();

        assert_eq!(leaves.len(), 6);
        assert_eq!(leaves[0].id, "1.1.1");
        assert_eq!(leaves[0].status, TaskStatus::Done);
        assert_eq!(leaves[0].component, "progress");
        assert_eq!(leaves[1].id, "1.1.2");
        assert_eq!(leaves[1].deps, vec!["1.1.1"]);
        assert_eq!(leaves[1].model.as_deref(), Some("claude-opus-4-6"));
    }

    #[test]
    fn test_to_summary() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let summary = tf.to_summary();

        assert_eq!(summary.total(), 6);
        assert_eq!(summary.done, 2); // 1.1.1, 2.1
        assert_eq!(summary.in_progress, 1); // 1.2.1
        assert_eq!(summary.todo, 2); // 1.1.2, 1.2.2
        assert_eq!(summary.blocked, 1); // 2.2
        assert_eq!(summary.remaining(), 3); // 2 todo + 1 in_progress
    }

    #[test]
    fn test_deps_map() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let deps = tf.deps_map();

        assert_eq!(deps["1.1.2"], vec!["1.1.1"]);
        assert_eq!(deps["2.2"], vec!["2.1", "1.1.2"]);
        assert!(deps["1.1.1"].is_empty());
    }

    #[test]
    fn test_models_map() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let models = tf.models_map();

        assert_eq!(models.len(), 1);
        assert_eq!(models["1.1.2"], "claude-opus-4-6");
    }

    #[test]
    fn test_update_status_todo_to_done() {
        let mut tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();

        // Update leaf task status from Todo to Done
        assert!(tf.update_status("1.1.2", TaskStatus::Done));
        let leaves = tf.flatten_leaves();
        let updated = leaves.iter().find(|l| l.id == "1.1.2").unwrap();
        assert_eq!(updated.status, TaskStatus::Done);
    }

    #[test]
    fn test_update_status_todo_to_in_progress() {
        let mut tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();

        // Update leaf task status from Todo to InProgress
        assert!(tf.update_status("1.1.1", TaskStatus::InProgress));
        let leaves = tf.flatten_leaves();
        let updated = leaves.iter().find(|l| l.id == "1.1.1").unwrap();
        assert_eq!(updated.status, TaskStatus::InProgress);
    }

    #[test]
    fn test_update_status_returns_true_when_found() {
        let mut tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();

        // Returns true when task is found
        let result = tf.update_status("1.1.2", TaskStatus::Done);
        assert!(result);
    }

    #[test]
    fn test_update_status_returns_false_for_nonexistent() {
        let mut tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();

        // Returns false when task does not exist
        let result = tf.update_status("9.9.9", TaskStatus::Done);
        assert!(!result);
    }

    #[test]
    fn test_update_status_does_not_modify_parent_node() {
        let mut tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();

        // Parent nodes should not be updated (they have no status)
        // "1" and "1.1" are parent nodes with subtasks
        assert!(!tf.update_status("1", TaskStatus::Done));
        assert!(!tf.update_status("1.1", TaskStatus::Done));

        // Verify parent nodes still have None status
        // Parent "1" still has no status in its original position
        assert!(tf.tasks[0].status.is_none());
        assert!(tf.tasks[0].subtasks[0].status.is_none());
    }

    #[test]
    fn test_update_status_deeply_nested() {
        // Test with deeply nested structure like "9.1.3"
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929

tasks:
  - id: "9"
    name: "Epic 9"
    component: orchestrate
    subtasks:
      - id: "9.1"
        name: "Feature 9.1"
        component: orchestrate
        subtasks:
          - id: "9.1.1"
            name: "Task 9.1.1"
            component: orchestrate
            status: todo
          - id: "9.1.2"
            name: "Task 9.1.2"
            component: orchestrate
            status: todo
          - id: "9.1.3"
            name: "Task 9.1.3"
            component: orchestrate
            status: todo
"#;
        let mut tf: TasksFile = serde_yaml::from_str(yaml).unwrap();

        // Update the deeply nested task
        assert!(tf.update_status("9.1.3", TaskStatus::Done));

        // Verify the status was updated
        let leaves = tf.flatten_leaves();
        let updated = leaves.iter().find(|l| l.id == "9.1.3").unwrap();
        assert_eq!(updated.status, TaskStatus::Done);

        // Verify other nested tasks remain unchanged
        let sibling = leaves.iter().find(|l| l.id == "9.1.1").unwrap();
        assert_eq!(sibling.status, TaskStatus::Todo);
        let sibling2 = leaves.iter().find(|l| l.id == "9.1.2").unwrap();
        assert_eq!(sibling2.status, TaskStatus::Todo);
    }

    #[test]
    fn test_update_status_empty_tree() {
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929
tasks: []
"#;
        let mut tf: TasksFile = serde_yaml::from_str(yaml).unwrap();
        assert!(!tf.update_status("any-id", TaskStatus::Done));
    }

    #[test]
    fn test_batch_update_statuses_error_on_not_found() {
        use std::io::Write;
        let dir = std::env::temp_dir().join("ralph_test_batch_error");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tasks.yml");

        let yaml = sample_yaml();
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(yaml.as_bytes()).unwrap();
        drop(file);

        let updates = vec![
            ("1.1.2".to_string(), TaskStatus::Done),
            ("9.9.9".to_string(), TaskStatus::Done), // Does not exist
        ];

        let result = TasksFile::batch_update_statuses(&path, &updates);
        assert!(result.is_err());

        // Verify the error is TaskNotFound
        match result {
            Err(RalphError::TaskNotFound(id)) => assert_eq!(id, "9.9.9"),
            _ => panic!("Expected TaskNotFound error"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_current_task() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let current = tf.current_task().unwrap();
        // First in_progress: 1.2.1
        assert_eq!(current.id, "1.2.1");
    }

    #[test]
    fn test_current_task_fallback_to_todo() {
        let yaml = r#"
tasks:
  - id: "1"
    name: "Done task"
    status: done
  - id: "2"
    name: "Todo task"
    status: todo
  - id: "3"
    name: "Another todo"
    status: todo
"#;
        let tf: TasksFile = serde_yaml::from_str(yaml).unwrap();
        let current = tf.current_task().unwrap();
        assert_eq!(current.id, "2");
    }

    #[test]
    fn test_new_fields_deserialize() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let leaves = tf.flatten_leaves();

        let task_112 = leaves.iter().find(|l| l.id == "1.1.2").unwrap();
        assert_eq!(
            task_112.description.as_deref(),
            Some(
                "Parse YAML frontmatter from PROGRESS.md including deps, models, and default_model fields"
            )
        );
        assert_eq!(
            task_112.related_files,
            vec!["src/shared/progress.rs", "docs/frontmatter-spec.md"]
        );
        assert_eq!(task_112.implementation_steps.len(), 3);

        // Tasks without new fields should have None/empty
        let task_111 = leaves.iter().find(|l| l.id == "1.1.1").unwrap();
        assert!(task_111.description.is_none());
        assert!(task_111.related_files.is_empty());
        assert!(task_111.implementation_steps.is_empty());
    }

    #[test]
    fn test_find_leaf() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let leaf = tf.find_leaf("1.1.2").unwrap();
        assert_eq!(leaf.name, "Implement parse_frontmatter()");
        assert!(tf.find_leaf("999").is_none());
    }

    #[test]
    fn test_validate_ok() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        assert!(tf.validate().is_ok());
    }

    #[test]
    fn test_validate_duplicate_id() {
        let yaml = r#"
tasks:
  - id: "1"
    name: "A"
    status: todo
  - id: "1"
    name: "B"
    status: todo
"#;
        let tf: TasksFile = serde_yaml::from_str(yaml).unwrap();
        assert!(tf.validate().is_err());
    }

    #[test]
    fn test_validate_status_on_parent() {
        let yaml = r#"
tasks:
  - id: "1"
    name: "Parent"
    status: done
    subtasks:
      - id: "1.1"
        name: "Child"
        status: todo
"#;
        let tf: TasksFile = serde_yaml::from_str(yaml).unwrap();
        let err = tf.validate().unwrap_err();
        assert!(err.to_string().contains("status"));
    }

    #[test]
    fn test_validate_deps_on_parent() {
        let yaml = r#"
tasks:
  - id: "1"
    name: "Parent"
    deps: ["2"]
    subtasks:
      - id: "1.1"
        name: "Child"
        status: todo
  - id: "2"
    name: "Other"
    status: todo
"#;
        let tf: TasksFile = serde_yaml::from_str(yaml).unwrap();
        let err = tf.validate().unwrap_err();
        assert!(err.to_string().contains("deps"));
    }

    #[test]
    fn test_validate_deps_nonexistent() {
        let yaml = r#"
tasks:
  - id: "1"
    name: "A"
    status: todo
    deps: ["999"]
"#;
        let tf: TasksFile = serde_yaml::from_str(yaml).unwrap();
        let err = tf.validate().unwrap_err();
        assert!(err.to_string().contains("non-existent"));
    }

    #[test]
    fn test_component_inheritance() {
        let yaml = r#"
tasks:
  - id: "1"
    name: "Epic"
    component: api
    subtasks:
      - id: "1.1"
        name: "Child without component"
        status: todo
      - id: "1.2"
        name: "Child with component"
        component: ui
        status: todo
"#;
        let tf: TasksFile = serde_yaml::from_str(yaml).unwrap();
        let leaves = tf.flatten_leaves();
        assert_eq!(leaves[0].component, "api"); // inherited
        assert_eq!(leaves[1].component, "ui"); // overridden
    }

    #[test]
    fn test_save_load_roundtrip() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();

        let dir = std::env::temp_dir().join("ralph_test_tasks_roundtrip");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tasks.yml");

        tf.save(&path).unwrap();
        let loaded = TasksFile::load(&path).unwrap();

        assert_eq!(loaded.flatten_leaves().len(), tf.flatten_leaves().len());
        assert_eq!(loaded.default_model, tf.default_model);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_to_summary_frontmatter() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let summary = tf.to_summary();

        let fm = summary.frontmatter.as_ref().unwrap();
        assert_eq!(fm.deps["1.1.2"], vec!["1.1.1"]);
        assert_eq!(fm.models["1.1.2"], "claude-opus-4-6");
        assert_eq!(
            fm.default_model.as_deref(),
            Some("claude-sonnet-4-5-20250929")
        );
    }

    #[test]
    fn test_flat_tasks_file() {
        let yaml = r#"
tasks:
  - id: "T01"
    name: "First"
    component: api
    status: todo
  - id: "T02"
    name: "Second"
    component: api
    status: done
    deps: ["T01"]
"#;
        let tf: TasksFile = serde_yaml::from_str(yaml).unwrap();
        assert!(tf.validate().is_ok());
        let leaves = tf.flatten_leaves();
        assert_eq!(leaves.len(), 2);
    }

    // ── Tests for find_node / all_ids ──────────────

    #[test]
    fn test_find_node_leaf() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let node = tf.find_node("1.1.2").unwrap();
        assert_eq!(node.name, "Implement parse_frontmatter()");
        assert!(node.is_leaf());
    }

    #[test]
    fn test_find_node_parent() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let node = tf.find_node("1.1").unwrap();
        assert_eq!(node.name, "Frontmatter Parsing");
        assert!(!node.is_leaf());
    }

    #[test]
    fn test_find_node_nonexistent() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        assert!(tf.find_node("999").is_none());
    }

    #[test]
    fn test_all_ids() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let ids = tf.all_ids();
        // 2 epics + 2 mid-levels + 6 leaves = 10
        assert_eq!(ids.len(), 10);
        assert!(ids.contains("1"));
        assert!(ids.contains("1.1"));
        assert!(ids.contains("1.1.1"));
        assert!(ids.contains("2.2"));
    }
}
