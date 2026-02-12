//! Tree manipulation operations for TasksFile.
//!
//! This module provides functions for traversing and modifying the task tree,
//! including finding nodes, adding/removing tasks, and collecting IDs.

use std::collections::HashSet;

use crate::shared::error::{RalphError, Result};

use super::node::TaskNode;

// ── Find operations ─────────────────────────────────────────────────

/// Find a node (leaf or parent) by ID (immutable).
pub fn find_node<'a>(nodes: &'a [TaskNode], id: &str) -> Option<&'a TaskNode> {
    for node in nodes {
        if node.id == id {
            return Some(node);
        }
        if let Some(found) = find_node(&node.subtasks, id) {
            return Some(found);
        }
    }
    None
}

/// Find a node (leaf or parent) by ID (mutable).
pub fn find_node_mut<'a>(nodes: &'a mut [TaskNode], id: &str) -> Option<&'a mut TaskNode> {
    for node in nodes {
        if node.id == id {
            return Some(node);
        }
        if let Some(found) = find_node_mut(&mut node.subtasks, id) {
            return Some(found);
        }
    }
    None
}

// ── Add/Remove operations ───────────────────────────────────────────

/// Add a task node to a parent's subtasks (or root level).
/// Optionally insert at a specific position index.
pub fn add_task(
    root_tasks: &mut Vec<TaskNode>,
    parent_id: Option<&str>,
    node: TaskNode,
    position: Option<usize>,
) -> Result<()> {
    let target = match parent_id {
        Some(pid) => {
            let parent = find_node_mut(root_tasks, pid)
                .ok_or_else(|| RalphError::Config(format!("Parent task not found: {}", pid)))?;
            &mut parent.subtasks
        }
        None => root_tasks,
    };
    let pos = position.unwrap_or(target.len()).min(target.len());
    target.insert(pos, node);
    Ok(())
}

/// Remove a task (and its subtasks) by ID. Returns the removed node.
/// Also cleans up dep references pointing to the removed task.
pub fn remove_task(root_tasks: &mut Vec<TaskNode>, id: &str) -> Option<TaskNode> {
    fn remove_from(nodes: &mut Vec<TaskNode>, id: &str) -> Option<TaskNode> {
        if let Some(pos) = nodes.iter().position(|n| n.id == id) {
            return Some(nodes.remove(pos));
        }
        for node in nodes.iter_mut() {
            if let Some(found) = remove_from(&mut node.subtasks, id) {
                return Some(found);
            }
        }
        None
    }

    let removed = remove_from(root_tasks, id)?;

    // Collect all IDs being removed (the node + its subtasks)
    let mut removed_ids = HashSet::new();
    fn collect_ids(node: &TaskNode, ids: &mut HashSet<String>) {
        // Required clone: HashSet<String> needs owned values for storage
        ids.insert(node.id.clone());
        for child in &node.subtasks {
            collect_ids(child, ids);
        }
    }
    collect_ids(&removed, &mut removed_ids);

    // Clean up deps referencing removed IDs
    fn clean_deps(nodes: &mut [TaskNode], removed_ids: &HashSet<String>) {
        for node in nodes.iter_mut() {
            node.deps.retain(|d| !removed_ids.contains(d));
            clean_deps(&mut node.subtasks, removed_ids);
        }
    }
    clean_deps(root_tasks, &removed_ids);

    Some(removed)
}

// ── Collection operations ───────────────────────────────────────────

/// Collect all task IDs in the tree.
pub fn all_ids(nodes: &[TaskNode]) -> HashSet<String> {
    let mut ids = HashSet::new();
    fn collect(nodes: &[TaskNode], ids: &mut HashSet<String>) {
        for node in nodes {
            // Required clone: HashSet<String> needs owned values for storage
            ids.insert(node.id.clone());
            collect(&node.subtasks, ids);
        }
    }
    collect(nodes, &mut ids);
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::progress::TaskStatus;

    fn sample_tree() -> Vec<TaskNode> {
        vec![
            TaskNode {
                id: "1".to_string(),
                name: "Epic 1".to_string(),
                component: Some("api".to_string()),
                status: None,
                deps: Vec::new(),
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                subtasks: vec![
                    TaskNode {
                        id: "1.1".to_string(),
                        name: "Task 1.1".to_string(),
                        component: None,
                        status: Some(TaskStatus::Done),
                        deps: Vec::new(),
                        model: None,
                        description: None,
                        related_files: Vec::new(),
                        implementation_steps: Vec::new(),
                        subtasks: Vec::new(),
                    },
                    TaskNode {
                        id: "1.2".to_string(),
                        name: "Task 1.2".to_string(),
                        component: None,
                        status: Some(TaskStatus::Todo),
                        deps: vec!["1.1".to_string()],
                        model: None,
                        description: None,
                        related_files: Vec::new(),
                        implementation_steps: Vec::new(),
                        subtasks: Vec::new(),
                    },
                ],
            },
            TaskNode {
                id: "2".to_string(),
                name: "Epic 2".to_string(),
                component: Some("ui".to_string()),
                status: None,
                deps: Vec::new(),
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                subtasks: vec![TaskNode {
                    id: "2.1".to_string(),
                    name: "Task 2.1".to_string(),
                    component: None,
                    status: Some(TaskStatus::InProgress),
                    deps: vec!["1.2".to_string()],
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    subtasks: Vec::new(),
                }],
            },
        ]
    }

    #[test]
    fn test_find_node() {
        let tree = sample_tree();
        let node = find_node(&tree, "1.2").unwrap();
        assert_eq!(node.name, "Task 1.2");
        assert!(find_node(&tree, "999").is_none());
    }

    #[test]
    fn test_find_node_mut() {
        let mut tree = sample_tree();
        let node = find_node_mut(&mut tree, "1.2").unwrap();
        node.name = "Modified".to_string();
        assert_eq!(find_node(&tree, "1.2").unwrap().name, "Modified");
    }

    #[test]
    fn test_add_task_to_root() {
        let mut tree = sample_tree();
        let new_task = TaskNode {
            id: "3".to_string(),
            name: "Epic 3".to_string(),
            component: None,
            status: Some(TaskStatus::Todo),
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            subtasks: Vec::new(),
        };
        add_task(&mut tree, None, new_task, None).unwrap();
        assert_eq!(tree.len(), 3);
        assert_eq!(tree[2].id, "3");
    }

    #[test]
    fn test_add_task_to_parent() {
        let mut tree = sample_tree();
        let new_task = TaskNode {
            id: "1.3".to_string(),
            name: "Task 1.3".to_string(),
            component: None,
            status: Some(TaskStatus::Todo),
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            subtasks: Vec::new(),
        };
        add_task(&mut tree, Some("1"), new_task, None).unwrap();
        let parent = find_node(&tree, "1").unwrap();
        assert_eq!(parent.subtasks.len(), 3);
        assert_eq!(parent.subtasks[2].id, "1.3");
    }

    #[test]
    fn test_add_task_with_position() {
        let mut tree = sample_tree();
        let new_task = TaskNode {
            id: "1.0".to_string(),
            name: "Task 1.0".to_string(),
            component: None,
            status: Some(TaskStatus::Todo),
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            subtasks: Vec::new(),
        };
        add_task(&mut tree, Some("1"), new_task, Some(0)).unwrap();
        let parent = find_node(&tree, "1").unwrap();
        assert_eq!(parent.subtasks[0].id, "1.0");
    }

    #[test]
    fn test_remove_task_leaf() {
        let mut tree = sample_tree();
        let removed = remove_task(&mut tree, "1.2").unwrap();
        assert_eq!(removed.id, "1.2");
        let parent = find_node(&tree, "1").unwrap();
        assert_eq!(parent.subtasks.len(), 1);
    }

    #[test]
    fn test_remove_task_parent() {
        let mut tree = sample_tree();
        let removed = remove_task(&mut tree, "1").unwrap();
        assert_eq!(removed.id, "1");
        assert_eq!(removed.subtasks.len(), 2);
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn test_remove_task_cleans_deps() {
        let mut tree = sample_tree();
        // Remove "1.2" which is depended on by "2.1"
        remove_task(&mut tree, "1.2");
        let task_21 = find_node(&tree, "2.1").unwrap();
        assert!(task_21.deps.is_empty());
    }

    #[test]
    fn test_all_ids() {
        let tree = sample_tree();
        let ids = all_ids(&tree);
        assert_eq!(ids.len(), 5); // 1, 1.1, 1.2, 2, 2.1
        assert!(ids.contains("1"));
        assert!(ids.contains("1.1"));
        assert!(ids.contains("1.2"));
        assert!(ids.contains("2"));
        assert!(ids.contains("2.1"));
    }
}
