use std::collections::HashMap;

use crossterm::style::Stylize;

use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use crate::shared::progress::{self, TaskStatus};
use crate::shared::tasks::{TaskNode, TasksFile};

pub async fn execute(file_config: &FileConfig) -> Result<()> {
    let progress_path = &file_config.task.progress_file;
    if !progress_path.exists() {
        return Err(RalphError::MissingFile(format!(
            "{} not found. Nothing to migrate.",
            progress_path.display()
        )));
    }

    let tasks_path = &file_config.task.tasks_file;
    if tasks_path.exists() {
        return Err(RalphError::TaskSetup(format!(
            "{} already exists. Remove it first to re-migrate.",
            tasks_path.display()
        )));
    }

    // 1. Load PROGRESS.md
    let summary = progress::load_progress(progress_path)?;

    // 2. Parse task IDs into segments and build a trie
    let mut trie: HashMap<String, TrieNode> = HashMap::new();

    for task in &summary.tasks {
        let segments: Vec<&str> = task.id.split('.').collect();
        insert_trie(&mut trie, &segments, task);
    }

    // 3. Convert trie to TaskNode tree
    let task_nodes = trie_to_nodes(&trie);

    // 4. Move deps from frontmatter onto leaves
    let deps_map = summary
        .frontmatter
        .as_ref()
        .map(|fm| &fm.deps)
        .cloned()
        .unwrap_or_default();

    let models_map = summary
        .frontmatter
        .as_ref()
        .map(|fm| &fm.models)
        .cloned()
        .unwrap_or_default();

    let default_model = summary
        .frontmatter
        .as_ref()
        .and_then(|fm| fm.default_model.clone());

    // 5. Apply deps and models to leaf nodes
    let task_nodes = apply_deps_and_models(task_nodes, &deps_map, &models_map);

    // 6. Build TasksFile
    let tasks_file = TasksFile {
        default_model,
        tasks: task_nodes,
    };

    // 7. Validate
    tasks_file.validate()?;

    // 8. Ensure .ralph/ exists and save
    if let Some(parent) = tasks_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    tasks_file.save(tasks_path)?;

    // 9. Print summary
    let leaves = tasks_file.flatten_leaves();
    let done = leaves
        .iter()
        .filter(|l| l.status == TaskStatus::Done)
        .count();
    let todo = leaves
        .iter()
        .filter(|l| l.status == TaskStatus::Todo)
        .count();
    let in_progress = leaves
        .iter()
        .filter(|l| l.status == TaskStatus::InProgress)
        .count();
    let blocked = leaves
        .iter()
        .filter(|l| l.status == TaskStatus::Blocked)
        .count();

    println!("{}", "━".repeat(60).dark_grey());
    println!(
        "{} Migrated {} tasks to {}",
        "✓".green().bold(),
        leaves.len(),
        tasks_path.display()
    );
    println!(
        "  {} {} done, {} todo, {} in progress, {} blocked",
        "Tasks:".dark_grey(),
        done,
        todo,
        in_progress,
        blocked
    );
    if !deps_map.is_empty() {
        let deps_count = deps_map.values().filter(|v| !v.is_empty()).count();
        println!("  {} {} deps migrated", "Deps:".dark_grey(), deps_count);
    }
    if !models_map.is_empty() {
        println!(
            "  {} {} model overrides migrated",
            "Models:".dark_grey(),
            models_map.len()
        );
    }
    println!("{}", "━".repeat(60).dark_grey());

    Ok(())
}

// ── Trie for building hierarchical structure ────────────────────────

struct TrieNode {
    /// Original task data (if this exact ID had a task in PROGRESS.md)
    task: Option<TrieLeaf>,
    /// Children keyed by segment
    children: HashMap<String, TrieNode>,
}

struct TrieLeaf {
    id: String,
    name: String,
    component: String,
    status: TaskStatus,
}

fn insert_trie(
    trie: &mut HashMap<String, TrieNode>,
    segments: &[&str],
    task: &progress::ProgressTask,
) {
    if segments.is_empty() {
        return;
    }

    let first = segments[0];
    let node = trie.entry(first.to_string()).or_insert_with(|| TrieNode {
        task: None,
        children: HashMap::new(),
    });

    if segments.len() == 1 {
        // This is the actual task
        node.task = Some(TrieLeaf {
            id: task.id.clone(),
            name: task.name.clone(),
            component: task.component.clone(),
            status: task.status.clone(),
        });
    } else {
        // Recurse deeper
        insert_trie(&mut node.children, &segments[1..], task);
    }
}

fn trie_to_nodes(trie: &HashMap<String, TrieNode>) -> Vec<TaskNode> {
    let mut keys: Vec<&String> = trie.keys().collect();
    keys.sort_by(|a, b| compare_segments(a, b));

    keys.into_iter()
        .map(|key| trie_node_to_task_node(key, &trie[key], ""))
        .collect()
}

fn trie_node_to_task_node(segment: &str, node: &TrieNode, parent_prefix: &str) -> TaskNode {
    let id = if parent_prefix.is_empty() {
        segment.to_string()
    } else {
        format!("{parent_prefix}.{segment}")
    };

    if node.children.is_empty() {
        // Leaf node
        if let Some(leaf) = &node.task {
            return TaskNode {
                id: leaf.id.clone(),
                name: leaf.name.clone(),
                component: Some(leaf.component.clone()),
                status: Some(leaf.status.clone()),
                deps: Vec::new(),
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                subtasks: Vec::new(),
            };
        }
        // Synthetic leaf (shouldn't happen, but handle gracefully)
        return TaskNode {
            id,
            name: format!("Task {segment}"),
            component: None,
            status: Some(TaskStatus::Todo),
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            subtasks: Vec::new(),
        };
    }

    // Parent node with children
    let subtasks = {
        let mut keys: Vec<&String> = node.children.keys().collect();
        keys.sort_by(|a, b| compare_segments(a, b));
        keys.into_iter()
            .map(|k| trie_node_to_task_node(k, &node.children[k], &id))
            .collect()
    };

    // Use task data for name/component if this ID was an actual task
    let (name, component) = if let Some(leaf) = &node.task {
        (leaf.name.clone(), Some(leaf.component.clone()))
    } else {
        (format!("Group {id}"), None)
    };

    TaskNode {
        id,
        name,
        component,
        status: None, // parents have computed status
        deps: Vec::new(),
        model: None,
        description: None,
        related_files: Vec::new(),
        implementation_steps: Vec::new(),
        subtasks,
    }
}

fn compare_segments(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(an), Ok(bn)) => an.cmp(&bn),
        _ => a.cmp(b),
    }
}

/// Apply deps and models from frontmatter to leaf nodes.
fn apply_deps_and_models(
    nodes: Vec<TaskNode>,
    deps: &HashMap<String, Vec<String>>,
    models: &HashMap<String, String>,
) -> Vec<TaskNode> {
    nodes
        .into_iter()
        .map(|mut node| {
            if node.is_leaf() {
                if let Some(d) = deps.get(&node.id) {
                    node.deps = d.clone();
                }
                if let Some(m) = models.get(&node.id) {
                    node.model = Some(m.clone());
                }
            } else {
                node.subtasks = apply_deps_and_models(node.subtasks, deps, models);
            }
            node
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::progress::{ProgressFrontmatter, ProgressSummary, ProgressTask};

    fn make_summary(
        tasks: Vec<(&str, &str, &str, TaskStatus)>,
        deps: Vec<(&str, Vec<&str>)>,
    ) -> ProgressSummary {
        let mut done = 0;
        let mut in_progress = 0;
        let mut blocked = 0;
        let mut todo = 0;
        let task_vec: Vec<ProgressTask> = tasks
            .into_iter()
            .map(|(id, comp, name, status)| {
                match &status {
                    TaskStatus::Done => done += 1,
                    TaskStatus::InProgress => in_progress += 1,
                    TaskStatus::Blocked => blocked += 1,
                    TaskStatus::Todo => todo += 1,
                }
                ProgressTask {
                    id: id.to_string(),
                    component: comp.to_string(),
                    name: name.to_string(),
                    status,
                }
            })
            .collect();

        let mut fm = ProgressFrontmatter::default();
        for (task, task_deps) in deps {
            fm.deps.insert(
                task.to_string(),
                task_deps.into_iter().map(|s| s.to_string()).collect(),
            );
        }

        ProgressSummary {
            tasks: task_vec,
            done,
            in_progress,
            blocked,
            todo,
            frontmatter: Some(fm),
        }
    }

    #[test]
    fn test_trie_flat_tasks() {
        let summary = make_summary(
            vec![
                ("1.1", "api", "First", TaskStatus::Todo),
                ("1.2", "api", "Second", TaskStatus::Done),
                ("2.1", "ui", "Third", TaskStatus::Todo),
            ],
            vec![("1.2", vec!["1.1"])],
        );

        let mut trie = HashMap::new();
        for task in &summary.tasks {
            let segments: Vec<&str> = task.id.split('.').collect();
            insert_trie(&mut trie, &segments, task);
        }

        let nodes = trie_to_nodes(&trie);
        assert_eq!(nodes.len(), 2); // group "1" and group "2"
        assert_eq!(nodes[0].subtasks.len(), 2); // 1.1, 1.2
        assert_eq!(nodes[1].subtasks.len(), 1); // 2.1
    }

    #[test]
    fn test_apply_deps() {
        let nodes = vec![TaskNode {
            id: "1.1".to_string(),
            name: "Test".to_string(),
            component: Some("api".to_string()),
            status: Some(TaskStatus::Todo),
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            subtasks: Vec::new(),
        }];

        let mut deps = HashMap::new();
        deps.insert("1.1".to_string(), vec!["1.0".to_string()]);
        let mut models = HashMap::new();
        models.insert("1.1".to_string(), "claude-opus-4-6".to_string());

        let result = apply_deps_and_models(nodes, &deps, &models);
        assert_eq!(result[0].deps, vec!["1.0"]);
        assert_eq!(result[0].model.as_deref(), Some("claude-opus-4-6"));
    }

    #[test]
    fn test_housekeeping_ids() {
        let summary = make_summary(
            vec![
                ("H.1", "all", "Scan duplication", TaskStatus::Todo),
                ("H.2", "all", "Update CLAUDE.md", TaskStatus::Todo),
            ],
            vec![],
        );

        let mut trie = HashMap::new();
        for task in &summary.tasks {
            let segments: Vec<&str> = task.id.split('.').collect();
            insert_trie(&mut trie, &segments, task);
        }

        let nodes = trie_to_nodes(&trie);
        // "H" group with 2 children
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "H");
        assert_eq!(nodes[0].subtasks.len(), 2);
    }
}
