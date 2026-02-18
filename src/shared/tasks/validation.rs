use std::collections::HashSet;

use crate::shared::dag::TaskDag;
use crate::shared::error::{RalphError, Result};

use super::node::TaskNode;

/// Validate the entire task tree structure.
pub fn validate(tasks: &[TaskNode]) -> Result<()> {
    let mut seen_ids: HashSet<&str> = HashSet::new();
    let mut all_leaf_ids: HashSet<&str> = HashSet::new();

    for node in tasks {
        validate_node(node, &mut seen_ids, &mut all_leaf_ids)?;
    }

    // Validate deps reference existing leaves
    for node in tasks {
        validate_deps(node, &all_leaf_ids)?;
    }

    // Validate no dependency cycles
    validate_no_cycles(tasks)?;

    Ok(())
}

/// Validate a single node recursively: check for duplicate IDs, ensure leaves have status,
/// ensure parents don't have status or deps.
fn validate_node<'a>(
    node: &'a TaskNode,
    seen_ids: &mut HashSet<&'a str>,
    all_leaf_ids: &mut HashSet<&'a str>,
) -> Result<()> {
    // Check duplicate IDs - using &str avoids cloning node.id
    if !seen_ids.insert(&node.id) {
        return Err(RalphError::Config(format!(
            "Duplicate task ID: {}",
            node.id
        )));
    }

    if node.is_leaf() {
        // Using &str avoids cloning node.id
        all_leaf_ids.insert(&node.id);

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
            validate_node(child, seen_ids, all_leaf_ids)?;
        }
    }

    Ok(())
}

/// Validate that all deps reference existing leaf tasks.
fn validate_deps(node: &TaskNode, all_leaf_ids: &HashSet<&str>) -> Result<()> {
    if node.is_leaf() {
        for dep in &node.deps {
            // Use dep.as_str() to compare with HashSet<&str>
            if !all_leaf_ids.contains(dep.as_str()) {
                return Err(RalphError::Config(format!(
                    "Task {} depends on non-existent leaf: {}",
                    node.id, dep
                )));
            }
        }
    } else {
        for child in &node.subtasks {
            validate_deps(child, all_leaf_ids)?;
        }
    }
    Ok(())
}

/// Validate that there are no dependency cycles.
fn validate_no_cycles(tasks: &[TaskNode]) -> Result<()> {
    // Build deps map from leaf tasks
    let mut deps_map = std::collections::HashMap::new();
    collect_deps(tasks, &mut deps_map);

    // Use TaskDag to detect cycles
    let dag = TaskDag::from_deps_map(&deps_map);
    if let Some(cycle) = dag.detect_cycles() {
        return Err(RalphError::Config(format!(
            "Dependency cycle detected: {}",
            cycle.join(" → ")
        )));
    }

    Ok(())
}

/// Collect all leaf task dependencies into a map.
fn collect_deps(nodes: &[TaskNode], deps_map: &mut std::collections::HashMap<String, Vec<String>>) {
    for node in nodes {
        if node.is_leaf() {
            // Required clones: HashMap<String, Vec<String>> needs owned data
            // for use in TaskDag::from_deps_map which stores the map
            deps_map.insert(node.id.clone(), node.deps.clone());
        } else {
            collect_deps(&node.subtasks, deps_map);
        }
    }
}

// Note: batch_update_statuses is kept in TasksFile impl in mod.rs
// as it's tightly coupled to load/save operations.

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tasks() -> Vec<TaskNode> {
        serde_yaml::from_str(
            r#"
- id: "1"
  name: "Epic"
  component: api
  subtasks:
    - id: "1.1"
      name: "Child"
      status: todo
      deps: []
- id: "2"
  name: "Other"
  status: done
  deps: ["1.1"]
"#,
        )
        .unwrap()
    }

    #[test]
    fn test_validate_ok() {
        let tasks = sample_tasks();
        assert!(validate(&tasks).is_ok());
    }

    #[test]
    fn test_validate_duplicate_id() {
        let tasks: Vec<TaskNode> = serde_yaml::from_str(
            r#"
- id: "1"
  name: "A"
  status: todo
- id: "1"
  name: "B"
  status: todo
"#,
        )
        .unwrap();
        assert!(validate(&tasks).is_err());
    }

    #[test]
    fn test_validate_status_on_parent() {
        let tasks: Vec<TaskNode> = serde_yaml::from_str(
            r#"
- id: "1"
  name: "Parent"
  status: done
  subtasks:
    - id: "1.1"
      name: "Child"
      status: todo
"#,
        )
        .unwrap();
        let err = validate(&tasks).unwrap_err();
        assert!(err.to_string().contains("status"));
    }

    #[test]
    fn test_validate_deps_on_parent() {
        let tasks: Vec<TaskNode> = serde_yaml::from_str(
            r#"
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
"#,
        )
        .unwrap();
        let err = validate(&tasks).unwrap_err();
        assert!(err.to_string().contains("deps"));
    }

    #[test]
    fn test_validate_deps_nonexistent() {
        let tasks: Vec<TaskNode> = serde_yaml::from_str(
            r#"
- id: "1"
  name: "A"
  status: todo
  deps: ["999"]
"#,
        )
        .unwrap();
        let err = validate(&tasks).unwrap_err();
        assert!(err.to_string().contains("non-existent"));
    }

    #[test]
    fn test_validate_cycle_simple() {
        let tasks: Vec<TaskNode> = serde_yaml::from_str(
            r#"
- id: "1"
  name: "Task 1"
  status: todo
  deps: ["2"]
- id: "2"
  name: "Task 2"
  status: todo
  deps: ["1"]
"#,
        )
        .unwrap();
        let err = validate(&tasks).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn test_validate_cycle_complex() {
        let tasks: Vec<TaskNode> = serde_yaml::from_str(
            r#"
- id: "1"
  name: "Epic"
  component: api
  subtasks:
    - id: "1.1"
      name: "Task A"
      status: todo
      deps: ["1.2"]
    - id: "1.2"
      name: "Task B"
      status: todo
      deps: ["1.3"]
    - id: "1.3"
      name: "Task C"
      status: todo
      deps: ["1.1"]
"#,
        )
        .unwrap();
        let err = validate(&tasks).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn test_validate_self_cycle() {
        let tasks: Vec<TaskNode> = serde_yaml::from_str(
            r#"
- id: "1"
  name: "Task 1"
  status: todo
  deps: ["1"]
"#,
        )
        .unwrap();
        let err = validate(&tasks).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn test_validate_deps_on_parent_node() {
        // Leaf 1.1 ma deps=[2], ale 2 jest parentem z subtaskami 2.1 i 2.2
        // validate_deps powinno zwrócić błąd bo parent nie jest w all_leaf_ids
        let tasks: Vec<TaskNode> = serde_yaml::from_str(
            r#"
- id: "1"
  name: "Epic 1"
  component: api
  subtasks:
    - id: "1.1"
      name: "Task with dependency on parent"
      status: todo
      deps: ["2"]
- id: "2"
  name: "Epic 2"
  component: tests
  subtasks:
    - id: "2.1"
      name: "Child task"
      status: todo
    - id: "2.2"
      name: "Another child task"
      status: todo
"#,
        )
        .unwrap();

        let err = validate(&tasks).unwrap_err();
        let err_msg = err.to_string();

        // Sprawdź pełny komunikat błędu — format: "Task {id} depends on non-existent leaf: {dep}"
        assert!(
            err_msg.contains("Task 1.1 depends on non-existent leaf: 2"),
            "Expected exact dep validation error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_validate_duplicate_id_at_different_levels() {
        // Dwa root nody, każdy ma subtask o tym samym ID "1.1"
        // validate() powinno wykryć duplikat ID nawet gdy są na różnych poziomach
        let tasks: Vec<TaskNode> = serde_yaml::from_str(
            r#"
- id: "1"
  name: "Root 1"
  component: api
  subtasks:
    - id: "1.1"
      name: "Child of Root 1"
      status: todo
- id: "2"
  name: "Root 2"
  component: tests
  subtasks:
    - id: "1.1"
      name: "Child of Root 2"
      status: todo
"#,
        )
        .unwrap();

        let err = validate(&tasks).unwrap_err();
        let err_msg = err.to_string();

        // Sprawdź że błąd dotyczy duplikatu ID 1.1
        assert!(
            err_msg.contains("Duplicate task ID: 1.1"),
            "Expected duplicate ID error for 1.1, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_validate_parent_with_status_and_subtasks() {
        // Parent node z jednocześnie status i niepustymi subtasks powinien być odrzucony
        use crate::shared::progress::TaskStatus;

        let child = TaskNode {
            id: "1.1".to_string(),
            name: "Child task".to_string(),
            component: None,
            status: Some(TaskStatus::Todo),
            deps: vec![],
            model: None,
            description: None,
            related_files: vec![],
            implementation_steps: vec![],
            acceptance_criteria: Vec::new(),
            profiles: vec![],
            subtasks: vec![],
        };

        let parent = TaskNode {
            id: "1".to_string(),
            name: "Parent with status".to_string(),
            component: Some("tests".to_string()),
            status: Some(TaskStatus::Todo), // Parent ma status
            deps: vec![],
            model: None,
            description: None,
            related_files: vec![],
            implementation_steps: vec![],
            acceptance_criteria: Vec::new(),
            profiles: vec![],
            subtasks: vec![child], // Parent ma niepuste subtasks
        };

        let tasks = vec![parent];
        let err = validate(&tasks).unwrap_err();
        let err_msg = err.to_string();

        // Sprawdź komunikat błędu: "Task 1 has both status and subtasks — status is only for leaves"
        assert!(
            err_msg.contains("Task 1 has both status and subtasks"),
            "Expected status validation error for parent node, got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("status is only for leaves"),
            "Expected 'status is only for leaves' in error message, got: {}",
            err_msg
        );
    }
}
