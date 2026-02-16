use serde::{Deserialize, Serialize};

use crate::shared::progress::TaskStatus;

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
    pub profiles: Vec<String>,
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
    pub profiles: Vec<String>,
}

// ── TaskNode helpers ────────────────────────────────────────────────

impl TaskNode {
    /// Whether this node is a leaf (has status, no subtasks).
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        self.subtasks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that a node with explicit empty subtasks list is treated as a leaf.
    /// YAML: {id: 1, name: Test, status: todo, subtasks: []}
    /// Expected: is_leaf() == true, status == Some(Todo)
    #[test]
    fn test_node_with_explicit_empty_subtasks_is_leaf() {
        let yaml = r#"
id: "1"
name: "Test Task"
status: todo
subtasks: []
"#;

        let node: TaskNode = serde_yaml::from_str(yaml).expect("Failed to deserialize YAML");

        // Verify it's a leaf
        assert!(
            node.is_leaf(),
            "Node with explicit empty subtasks should be a leaf"
        );

        // Verify status is present
        assert_eq!(
            node.status,
            Some(TaskStatus::Todo),
            "Node should have status Todo"
        );

        // Verify subtasks is empty
        assert!(node.subtasks.is_empty(), "Subtasks should be empty");
    }

    /// Test that validation accepts a leaf node with explicit empty subtasks.
    #[test]
    fn test_validate_accepts_explicit_empty_subtasks() {
        let yaml = r#"
- id: "1"
  name: "Test Task"
  status: todo
  subtasks: []
"#;

        let tasks: Vec<TaskNode> = serde_yaml::from_str(yaml).expect("Failed to deserialize YAML");

        // Validation should succeed
        crate::shared::tasks::validation::validate(&tasks).expect("Validation should succeed");
    }

    /// Test that a node with no subtasks field is equivalent to explicit empty subtasks.
    #[test]
    fn test_node_without_subtasks_field_equals_empty_subtasks() {
        let yaml_explicit = r#"
id: "1"
name: "Test"
status: todo
subtasks: []
"#;

        let yaml_implicit = r#"
id: "1"
name: "Test"
status: todo
"#;

        let node_explicit: TaskNode = serde_yaml::from_str(yaml_explicit).unwrap();
        let node_implicit: TaskNode = serde_yaml::from_str(yaml_implicit).unwrap();

        // Both should be leaves
        assert!(node_explicit.is_leaf());
        assert!(node_implicit.is_leaf());

        // Both should have empty subtasks
        assert_eq!(node_explicit.subtasks.len(), 0);
        assert_eq!(node_implicit.subtasks.len(), 0);
    }

    // ── Task 48.2: Backward compatibility tests (TaskNode without profiles) ──

    /// Test deserializing TaskNode YAML without profiles field.
    /// Should default to empty profiles vec.
    #[test]
    fn test_task_node_deserialize_without_profiles() {
        let yaml_content = r#"
id: "1.1"
name: "Test task"
component: "tests"
status: "todo"
deps: []
"#;
        let node: TaskNode = serde_yaml::from_str(yaml_content).unwrap();
        assert_eq!(node.id, "1.1");
        assert_eq!(node.name, "Test task");
        assert_eq!(node.component.as_deref(), Some("tests"));
        // Profiles should default to empty vec
        assert!(
            node.profiles.is_empty(),
            "TaskNode without profiles field should have empty profiles vec"
        );
    }

    /// Test deserializing TaskNode with explicit empty profiles array.
    #[test]
    fn test_task_node_deserialize_with_empty_profiles() {
        let yaml_content = r#"
id: "2.1"
name: "Test task with empty profiles"
component: "tests"
status: "in_progress"
deps: []
profiles: []
"#;
        let node: TaskNode = serde_yaml::from_str(yaml_content).unwrap();
        assert_eq!(node.id, "2.1");
        assert!(node.profiles.is_empty());
    }

    /// Test deserializing minimal TaskNode (only required fields).
    #[test]
    fn test_task_node_deserialize_minimal() {
        let yaml_content = r#"
id: "3.1"
name: "Minimal task"
status: "todo"
"#;
        let node: TaskNode = serde_yaml::from_str(yaml_content).unwrap();
        assert_eq!(node.id, "3.1");
        assert_eq!(node.name, "Minimal task");
        // Optional fields should have defaults
        assert!(node.component.is_none());
        assert!(node.deps.is_empty());
        assert!(node.profiles.is_empty());
        assert!(node.related_files.is_empty());
        assert!(node.implementation_steps.is_empty());
    }

    /// Test deserializing TaskNode with profiles field (new behavior).
    #[test]
    fn test_task_node_deserialize_with_profiles() {
        let yaml_content = r#"
id: "4.1"
name: "Task with profiles"
component: "backend"
status: "todo"
deps: []
profiles: ["backend", "api"]
"#;
        let node: TaskNode = serde_yaml::from_str(yaml_content).unwrap();
        assert_eq!(node.id, "4.1");
        assert_eq!(node.profiles.len(), 2);
        assert_eq!(node.profiles, vec!["backend", "api"]);
    }

    /// Test serializing TaskNode without profiles (skip_serializing_if empty).
    #[test]
    fn test_task_node_serialize_without_profiles() {
        let node = TaskNode {
            id: "5.1".to_string(),
            name: "Serialize test".to_string(),
            component: Some("tests".to_string()),
            status: Some(TaskStatus::Todo),
            deps: vec![],
            model: None,
            description: None,
            related_files: vec![],
            implementation_steps: vec![],
            profiles: vec![], // Empty profiles should be skipped
            subtasks: vec![],
        };

        let yaml = serde_yaml::to_string(&node).unwrap();
        // Profiles field should NOT appear in YAML (skip_serializing_if)
        assert!(
            !yaml.contains("profiles:"),
            "Empty profiles field should be skipped during serialization"
        );
    }

    /// Test serializing TaskNode with profiles (should appear in YAML).
    #[test]
    fn test_task_node_serialize_with_profiles() {
        let node = TaskNode {
            id: "6.1".to_string(),
            name: "Serialize with profiles".to_string(),
            component: Some("backend".to_string()),
            status: Some(TaskStatus::Todo),
            deps: vec![],
            model: None,
            description: None,
            related_files: vec![],
            implementation_steps: vec![],
            profiles: vec!["backend".to_string(), "api".to_string()],
            subtasks: vec![],
        };

        let yaml = serde_yaml::to_string(&node).unwrap();
        // Profiles field should appear in YAML
        assert!(
            yaml.contains("profiles:"),
            "Non-empty profiles field should appear in serialized YAML"
        );
        assert!(yaml.contains("backend"));
        assert!(yaml.contains("api"));
    }

    /// Test that omitted profiles field and explicit empty profiles are equivalent.
    #[test]
    fn test_task_node_profiles_omitted_vs_empty() {
        let yaml_omitted = r#"
id: "7.1"
name: "Omitted profiles"
status: "todo"
"#;
        let node_omitted: TaskNode = serde_yaml::from_str(yaml_omitted).unwrap();

        let yaml_empty = r#"
id: "7.2"
name: "Empty profiles"
status: "todo"
profiles: []
"#;
        let node_empty: TaskNode = serde_yaml::from_str(yaml_empty).unwrap();

        // Both should have empty profiles vec
        assert_eq!(node_omitted.profiles.len(), node_empty.profiles.len());
        assert!(node_omitted.profiles.is_empty());
        assert!(node_empty.profiles.is_empty());
    }
}
