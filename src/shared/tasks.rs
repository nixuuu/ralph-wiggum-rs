use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::shared::error::{RalphError, Result};
use crate::shared::progress::{
    ProgressFrontmatter, ProgressSummary, ProgressTask, TaskStatus,
};

// ── Types ───────────────────────────────────────────────────────────

/// Root structure for `.ralph/tasks.yml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TasksFile {
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub tasks: Vec<TaskNode>,
}

/// Recursive task node — can be a leaf (has `status`) or a parent (has `subtasks`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNode {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implementation_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtasks: Vec<TaskNode>,
}

/// Flattened view of a leaf task (no subtasks).
#[derive(Debug, Clone)]
pub struct LeafTask {
    pub id: String,
    pub name: String,
    pub component: String,
    pub status: TaskStatus,
    pub deps: Vec<String>,
    pub model: Option<String>,
    pub description: Option<String>,
    pub related_files: Vec<String>,
    pub implementation_steps: Vec<String>,
}

// ── TaskNode helpers ────────────────────────────────────────────────

impl TaskNode {
    /// Whether this node is a leaf (has status, no subtasks).
    pub fn is_leaf(&self) -> bool {
        self.subtasks.is_empty()
    }

    /// Compute status from children recursively.
    ///
    /// Public API for hierarchical task status computation in UI/reports.
    /// Currently used only in tests and internal recursion, but preserved for future features.
    #[allow(dead_code)] // Public API for future UI/report features showing hierarchical status
    pub fn computed_status(&self) -> TaskStatus {
        if self.is_leaf() {
            return self.status.clone().unwrap_or(TaskStatus::Todo);
        }

        let child_statuses: Vec<TaskStatus> = self
            .subtasks
            .iter()
            .map(|c| c.computed_status())
            .collect();

        if child_statuses.iter().all(|s| *s == TaskStatus::Done) {
            TaskStatus::Done
        } else if child_statuses.contains(&TaskStatus::Blocked) {
            TaskStatus::Blocked
        } else if child_statuses.contains(&TaskStatus::InProgress) {
            TaskStatus::InProgress
        } else {
            TaskStatus::Todo
        }
    }
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
        let mut seen_ids = HashSet::new();
        let mut all_leaf_ids = HashSet::new();

        for node in &self.tasks {
            Self::validate_node(node, &mut seen_ids, &mut all_leaf_ids)?;
        }

        // Validate deps reference existing leaves
        for node in &self.tasks {
            Self::validate_deps(node, &all_leaf_ids)?;
        }

        Ok(())
    }

    fn validate_node(
        node: &TaskNode,
        seen_ids: &mut HashSet<String>,
        all_leaf_ids: &mut HashSet<String>,
    ) -> Result<()> {
        // Check duplicate IDs
        // Clone required: HashSet::insert takes ownership. Using &str keys would
        // require HashSet<&str> with lifetime tied to node, complicating the API.
        if !seen_ids.insert(node.id.clone()) {
            return Err(RalphError::Config(format!(
                "Duplicate task ID: {}",
                node.id
            )));
        }

        if node.is_leaf() {
            // Clone required: same reason as above (HashSet::insert ownership)
            all_leaf_ids.insert(node.id.clone());

            // Leaves should not have status on parent nodes
            // (this is fine — leaves have status)
        } else {
            // Parents must not have status
            if node.status.is_some() {
                return Err(RalphError::Config(format!(
                    "Task {} has both status and subtasks — status is only for leaves",
                    node.id
                )));
            }
            // Parents must not have deps
            if !node.deps.is_empty() {
                return Err(RalphError::Config(format!(
                    "Task {} has both deps and subtasks — deps are only for leaves",
                    node.id
                )));
            }

            for child in &node.subtasks {
                Self::validate_node(child, seen_ids, all_leaf_ids)?;
            }
        }

        Ok(())
    }

    fn validate_deps(node: &TaskNode, all_leaf_ids: &HashSet<String>) -> Result<()> {
        if node.is_leaf() {
            for dep in &node.deps {
                if !all_leaf_ids.contains(dep) {
                    return Err(RalphError::Config(format!(
                        "Task {} depends on non-existent leaf: {}",
                        node.id, dep
                    )));
                }
            }
        } else {
            for child in &node.subtasks {
                Self::validate_deps(child, all_leaf_ids)?;
            }
        }
        Ok(())
    }

    /// Recursively collect all leaf tasks.
    pub fn flatten_leaves(&self) -> Vec<LeafTask> {
        let mut leaves = Vec::new();
        for node in &self.tasks {
            Self::collect_leaves(node, None, &mut leaves);
        }
        leaves
    }

    fn collect_leaves(
        node: &TaskNode,
        parent_component: Option<&str>,
        out: &mut Vec<LeafTask>,
    ) {
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
        for leaf in self.flatten_leaves() {
            map.insert(leaf.id, leaf.deps);
        }
        map
    }

    /// Extract model overrides from leaves: task_id → model.
    pub fn models_map(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for leaf in self.flatten_leaves() {
            if let Some(model) = leaf.model {
                map.insert(leaf.id, model);
            }
        }
        map
    }

    /// Update the status of a specific leaf task by ID. Returns true if found.
    pub fn update_status(&mut self, task_id: &str, new_status: TaskStatus) -> bool {
        for node in &mut self.tasks {
            if Self::update_node_status(node, task_id, &new_status) {
                return true;
            }
        }
        false
    }

    fn update_node_status(
        node: &mut TaskNode,
        task_id: &str,
        new_status: &TaskStatus,
    ) -> bool {
        if node.id == task_id && node.is_leaf() {
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
    pub fn batch_update_statuses(
        path: &Path,
        updates: &[(String, TaskStatus)],
    ) -> Result<()> {
        let mut tf = Self::load(path)?;
        for (id, status) in updates {
            tf.update_status(id, status.clone());
        }
        tf.save(path)
    }

    /// Find a specific leaf task by ID.
    pub fn find_leaf(&self, task_id: &str) -> Option<LeafTask> {
        self.flatten_leaves().into_iter().find(|l| l.id == task_id)
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

// ── Standalone helpers ──────────────────────────────────────────────

/// Resolve short model aliases to full model IDs.
pub fn resolve_model_alias(alias: &str) -> String {
    match alias {
        "opus" => "claude-opus-4-6".to_string(),
        "sonnet" => "claude-sonnet-4-5-20250929".to_string(),
        "haiku" => "claude-haiku-4-5-20251001".to_string(),
        other => other.to_string(),
    }
}

/// Build a rich task description from a LeafTask for use in prompts.
pub fn format_task_prompt(leaf: &LeafTask) -> String {
    let mut parts = Vec::new();
    parts.push(format!("**ID:** {}", leaf.id));
    parts.push(format!("**Component:** {}", leaf.component));
    parts.push(format!("**Name:** {}", leaf.name));

    if let Some(ref desc) = leaf.description {
        parts.push(String::new());
        parts.push(format!("**Description:**\n{desc}"));
    }

    if !leaf.related_files.is_empty() {
        parts.push(String::new());
        let files: Vec<String> = leaf.related_files.iter().map(|f| format!("- {f}")).collect();
        parts.push(format!("**Related files:**\n{}", files.join("\n")));
    }

    if !leaf.implementation_steps.is_empty() {
        parts.push(String::new());
        let steps: Vec<String> = leaf
            .implementation_steps
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}. {s}", i + 1))
            .collect();
        parts.push(format!("**Implementation steps:**\n{}", steps.join("\n")));
    }

    parts.join("\n")
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
        assert_eq!(tf.default_model.as_deref(), Some("claude-sonnet-4-5-20250929"));
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
        assert_eq!(summary.done, 2);    // 1.1.1, 2.1
        assert_eq!(summary.in_progress, 1); // 1.2.1
        assert_eq!(summary.todo, 2);    // 1.1.2, 1.2.2
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
    fn test_computed_status() {
        let tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();

        // Epic 1 has mixed statuses → in_progress (because 1.2.1 is in_progress)
        assert_eq!(tf.tasks[0].computed_status(), TaskStatus::InProgress);

        // Epic 2 has done + blocked → blocked
        assert_eq!(tf.tasks[1].computed_status(), TaskStatus::Blocked);
    }

    #[test]
    fn test_update_status() {
        let mut tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();

        assert!(tf.update_status("1.1.2", TaskStatus::Done));
        let leaves = tf.flatten_leaves();
        let updated = leaves.iter().find(|l| l.id == "1.1.2").unwrap();
        assert_eq!(updated.status, TaskStatus::Done);
    }

    #[test]
    fn test_update_status_not_found() {
        let mut tf: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        assert!(!tf.update_status("9.9.9", TaskStatus::Done));
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
            Some("Parse YAML frontmatter from PROGRESS.md including deps, models, and default_model fields")
        );
        assert_eq!(task_112.related_files, vec!["src/shared/progress.rs", "docs/frontmatter-spec.md"]);
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
    fn test_resolve_model_alias() {
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("sonnet"), "claude-sonnet-4-5-20250929");
        assert_eq!(resolve_model_alias("haiku"), "claude-haiku-4-5-20251001");
        assert_eq!(resolve_model_alias("claude-opus-4-6"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("custom-model"), "custom-model");
    }

    #[test]
    fn test_format_task_prompt_minimal() {
        let leaf = LeafTask {
            id: "1.1".to_string(),
            name: "Simple task".to_string(),
            component: "api".to_string(),
            status: TaskStatus::Todo,
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
        };
        let prompt = format_task_prompt(&leaf);
        assert!(prompt.contains("**ID:** 1.1"));
        assert!(prompt.contains("**Component:** api"));
        assert!(prompt.contains("**Name:** Simple task"));
        assert!(!prompt.contains("**Description:**"));
        assert!(!prompt.contains("**Related files:**"));
        assert!(!prompt.contains("**Implementation steps:**"));
    }

    #[test]
    fn test_format_task_prompt_full() {
        let leaf = LeafTask {
            id: "2.3.1".to_string(),
            name: "Implement auth endpoints".to_string(),
            component: "api".to_string(),
            status: TaskStatus::Todo,
            deps: Vec::new(),
            model: None,
            description: Some("Create REST endpoints for user authentication".to_string()),
            related_files: vec!["src/routes/auth.rs".to_string(), "docs/auth-spec.md".to_string()],
            implementation_steps: vec![
                "Create auth route handler module".to_string(),
                "Implement POST /login endpoint".to_string(),
            ],
        };
        let prompt = format_task_prompt(&leaf);
        assert!(prompt.contains("**ID:** 2.3.1"));
        assert!(prompt.contains("**Description:**\nCreate REST endpoints"));
        assert!(prompt.contains("- src/routes/auth.rs"));
        assert!(prompt.contains("- docs/auth-spec.md"));
        assert!(prompt.contains("1. Create auth route handler module"));
        assert!(prompt.contains("2. Implement POST /login endpoint"));
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
        assert_eq!(leaves[1].component, "ui");  // overridden
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
        assert_eq!(fm.default_model.as_deref(), Some("claude-sonnet-4-5-20250929"));
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
}
