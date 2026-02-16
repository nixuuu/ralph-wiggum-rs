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
                profiles: Vec::new(),
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
                        profiles: Vec::new(),
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
                        profiles: Vec::new(),
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
                profiles: Vec::new(),
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
                    profiles: Vec::new(),
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
            profiles: Vec::new(),
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
            profiles: Vec::new(),
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
            profiles: Vec::new(),
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

    #[test]
    fn test_add_task_to_leaf_creates_validation_error() {
        use crate::shared::tasks::validation::validate;

        // Krok 1: Stwórz drzewo z leafem 1.1 (ma status: todo)
        let mut tree = vec![TaskNode {
            id: "1".to_string(),
            name: "Epic 1".to_string(),
            component: Some("api".to_string()),
            status: None,
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            profiles: Vec::new(),
            subtasks: vec![TaskNode {
                id: "1.1".to_string(),
                name: "Original leaf task".to_string(),
                component: None,
                status: Some(TaskStatus::Todo), // To jest leaf ze statusem
                deps: Vec::new(),
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                profiles: Vec::new(),
                subtasks: Vec::new(), // Pusty wektor — jest leafem
            }],
        }];

        // Krok 2: Dodaj child do 1.1 (leaf staje się parentem)
        let new_child = TaskNode {
            id: "1.1.1".to_string(),
            name: "New child task".to_string(),
            component: None,
            status: Some(TaskStatus::Todo),
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            profiles: Vec::new(),
            subtasks: Vec::new(),
        };

        add_task(&mut tree, Some("1.1"), new_child, None).unwrap();

        // Krok 3: Sprawdź że 1.1 ma teraz subtasks
        let node_1_1 = find_node(&tree, "1.1").unwrap();
        assert_eq!(node_1_1.subtasks.len(), 1);
        assert_eq!(node_1_1.subtasks[0].id, "1.1.1");

        // Krok 4: Sprawdź że 1.1 nadal ma status (to jest problem)
        assert!(
            node_1_1.status.is_some(),
            "Node 1.1 powinien nadal mieć status po dodaniu subtasks"
        );

        // Krok 5: Uruchom validate() i sprawdź błąd walidacji
        let validation_result = validate(&tree);
        assert!(
            validation_result.is_err(),
            "Walidacja powinna wykryć że parent ma status"
        );

        let err_msg = validation_result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Task 1.1 has both status and subtasks"),
            "Oczekiwany błąd walidacji o status na parent node, otrzymano: {}",
            err_msg
        );
    }

    #[test]
    fn test_add_task_with_position_clamping() {
        let mut tree = sample_tree();
        let new_task = TaskNode {
            id: "1.99".to_string(),
            name: "Task 1.99".to_string(),
            component: None,
            status: Some(TaskStatus::Todo),
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            profiles: Vec::new(),
            subtasks: Vec::new(),
        };
        // Parent "1" has 2 children (1.1 and 1.2).
        // Request position=100 should clamp to end (position 2).
        add_task(&mut tree, Some("1"), new_task, Some(100)).unwrap();
        let parent = find_node(&tree, "1").unwrap();
        assert_eq!(parent.subtasks.len(), 3);
        // Existing children remain in original positions
        assert_eq!(parent.subtasks[0].id, "1.1");
        assert_eq!(parent.subtasks[1].id, "1.2");
        // Clamped node appended at end
        assert_eq!(parent.subtasks[2].id, "1.99");
    }

    #[test]
    fn test_remove_task_last_task_empty_tree() {
        // Stwórz drzewo z jednym root task
        let mut tree = vec![TaskNode {
            id: "1".to_string(),
            name: "Single task".to_string(),
            component: Some("test".to_string()),
            status: Some(TaskStatus::Todo),
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            profiles: Vec::new(),
            subtasks: Vec::new(),
        }];

        // Usuń jedyny task
        let removed = remove_task(&mut tree, "1");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, "1");

        // Sprawdź że drzewo jest puste
        assert!(tree.is_empty());

        // Sprawdź all_ids() == pustą kolekcję
        let ids = all_ids(&tree);
        assert!(ids.is_empty());

        // Sprawdź że flatten_leaves z modułu TasksFile też zwraca pustą listę
        use crate::shared::tasks::TasksFile;
        let tasks_file = TasksFile {
            default_model: Some("claude-sonnet-4-5-20250929".to_string()),
            tasks: tree,
        };
        let leaves = tasks_file.flatten_leaves();
        assert!(leaves.is_empty());
    }

    #[test]
    fn test_remove_parent_cleans_subtask_deps() {
        // Krok 1: Stwórz tree: parent 2 z subtasks 2.1, 2.2; leaf 3.1 z deps=[2.1, 2.2]
        let mut tree = vec![
            TaskNode {
                id: "2".to_string(),
                name: "Parent 2".to_string(),
                component: Some("api".to_string()),
                status: None,
                deps: Vec::new(),
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                profiles: Vec::new(),
                subtasks: vec![
                    TaskNode {
                        id: "2.1".to_string(),
                        name: "Task 2.1".to_string(),
                        component: None,
                        status: Some(TaskStatus::Done),
                        deps: Vec::new(),
                        model: None,
                        description: None,
                        related_files: Vec::new(),
                        implementation_steps: Vec::new(),
                        profiles: Vec::new(),
                        subtasks: Vec::new(),
                    },
                    TaskNode {
                        id: "2.2".to_string(),
                        name: "Task 2.2".to_string(),
                        component: None,
                        status: Some(TaskStatus::Done),
                        deps: Vec::new(),
                        model: None,
                        description: None,
                        related_files: Vec::new(),
                        implementation_steps: Vec::new(),
                        profiles: Vec::new(),
                        subtasks: Vec::new(),
                    },
                ],
            },
            TaskNode {
                id: "3".to_string(),
                name: "Epic 3".to_string(),
                component: Some("ui".to_string()),
                status: None,
                deps: Vec::new(),
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                profiles: Vec::new(),
                subtasks: vec![TaskNode {
                    id: "3.1".to_string(),
                    name: "Task 3.1".to_string(),
                    component: None,
                    status: Some(TaskStatus::Todo),
                    deps: vec!["2.1".to_string(), "2.2".to_string()], // Zależności od subtasków 2
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    profiles: Vec::new(),
                    subtasks: Vec::new(),
                }],
            },
        ];

        // Krok 2: Wywołaj remove_task(2)
        let removed = remove_task(&mut tree, "2");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, "2");

        // Krok 3: Sprawdź że parent 2 i subtaski zniknęły z drzewa
        assert!(
            find_node(&tree, "2").is_none(),
            "Node 2 powinien być usunięty"
        );
        assert!(
            find_node(&tree, "2.1").is_none(),
            "Node 2.1 powinien być usunięty"
        );
        assert!(
            find_node(&tree, "2.2").is_none(),
            "Node 2.2 powinien być usunięty"
        );

        // Krok 4: Sprawdź że 3.1.deps nie zawiera 2.1 ani 2.2
        let task_3_1 = find_node(&tree, "3.1").unwrap();
        assert!(
            task_3_1.deps.is_empty(),
            "Task 3.1 powinien mieć puste deps po usunięciu parenta 2, ale ma: {:?}",
            task_3_1.deps
        );
    }

    #[test]
    fn test_remove_task_nonexistent_id() {
        let mut tree = sample_tree();
        let initial_ids = all_ids(&tree);

        // Try to remove a non-existent task with ID "999"
        let result = remove_task(&mut tree, "999");

        // Should return None
        assert!(result.is_none());

        // Tree should remain unchanged — all nodes at every depth preserved
        assert_eq!(all_ids(&tree), initial_ids);
    }

    #[test]
    fn test_move_parent_under_own_child_fails() {
        // Scenariusz: Tree: 1 → [1.1, 1.2]
        // Move 1 pod 1.1 = remove(1) + add(1.1, node=1, ...)
        // Po remove(1) subtree 1.1 znika, więc add_task(1.1, ...) zwraca error

        // Krok 1: Stwórz tree: parent 1 z subtasks 1.1, 1.2
        let mut tree = vec![TaskNode {
            id: "1".to_string(),
            name: "Parent 1".to_string(),
            component: Some("api".to_string()),
            status: None,
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            profiles: Vec::new(),
            subtasks: vec![
                TaskNode {
                    id: "1.1".to_string(),
                    name: "Task 1.1".to_string(),
                    component: None,
                    status: Some(TaskStatus::Todo),
                    deps: Vec::new(),
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    profiles: Vec::new(),
                    subtasks: Vec::new(),
                },
                TaskNode {
                    id: "1.2".to_string(),
                    name: "Task 1.2".to_string(),
                    component: None,
                    status: Some(TaskStatus::Todo),
                    deps: Vec::new(),
                    model: None,
                    description: None,
                    related_files: Vec::new(),
                    implementation_steps: Vec::new(),
                    profiles: Vec::new(),
                    subtasks: Vec::new(),
                },
            ],
        }];

        // Krok 2: Symuluj operację move: usuń node 1
        let removed_node = remove_task(&mut tree, "1");
        assert!(
            removed_node.is_some(),
            "Remove powinien zwrócić usunięty node"
        );
        let removed = removed_node.unwrap();
        assert_eq!(removed.id, "1");

        // Krok 3: Sprawdź że node 1.1 też został usunięty (subtree cleanup)
        assert!(
            find_node(&tree, "1.1").is_none(),
            "Node 1.1 powinien być usunięty razem z parentem 1"
        );
        assert!(
            find_node(&tree, "1.2").is_none(),
            "Node 1.2 powinien być usunięty razem z parentem 1"
        );

        // Krok 4: Próba dodania usuniętego node pod nieistniejący parent 1.1
        let result = add_task(&mut tree, Some("1.1"), removed, None);
        assert!(
            result.is_err(),
            "Add_task powinien zwrócić error gdy parent nie istnieje"
        );

        // Krok 5: Sprawdź komunikat błędu
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("Parent task not found: 1.1"),
            "Error powinien zawierać 'Parent task not found: 1.1', otrzymano: {}",
            err_msg
        );

        // Krok 6: Sprawdź że drzewo jest puste (nie uszkodzone)
        assert!(
            tree.is_empty(),
            "Drzewo powinno być puste po nieudanej operacji move"
        );
    }
}
