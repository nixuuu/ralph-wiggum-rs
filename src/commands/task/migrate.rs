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
                acceptance_criteria: Vec::new(),
                profiles: Vec::new(),
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
            acceptance_criteria: Vec::new(),
            profiles: Vec::new(),
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
        acceptance_criteria: Vec::new(),
        profiles: Vec::new(),
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
            acceptance_criteria: Vec::new(),
            profiles: Vec::new(),
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

    #[test]
    fn test_flat_single_level_ids() {
        // PROGRESS.md z ID: 1, 2, 3 (bez kropek)
        let summary = make_summary(
            vec![
                ("1", "api", "First task", TaskStatus::Todo),
                ("2", "api", "Second task", TaskStatus::InProgress),
                ("3", "ui", "Third task", TaskStatus::Done),
            ],
            vec![("2", vec!["1"]), ("3", vec!["1", "2"])],
        );

        let mut trie = HashMap::new();
        for task in &summary.tasks {
            let segments: Vec<&str> = task.id.split('.').collect();
            insert_trie(&mut trie, &segments, task);
        }

        let nodes = trie_to_nodes(&trie);

        // Powinny być 3 root-level taski
        assert_eq!(nodes.len(), 3, "Expected 3 root-level tasks");

        // Sprawdź ID i brak subtasków
        assert_eq!(nodes[0].id, "1");
        assert_eq!(nodes[0].subtasks.len(), 0, "Task 1 should be a leaf");
        assert_eq!(nodes[1].id, "2");
        assert_eq!(nodes[1].subtasks.len(), 0, "Task 2 should be a leaf");
        assert_eq!(nodes[2].id, "3");
        assert_eq!(nodes[2].subtasks.len(), 0, "Task 3 should be a leaf");

        // Sprawdź statusy
        assert_eq!(
            nodes[0].status,
            Some(TaskStatus::Todo),
            "Task 1 should be Todo"
        );
        assert_eq!(
            nodes[1].status,
            Some(TaskStatus::InProgress),
            "Task 2 should be InProgress"
        );
        assert_eq!(
            nodes[2].status,
            Some(TaskStatus::Done),
            "Task 3 should be Done"
        );

        // Sprawdź komponenty
        assert_eq!(nodes[0].component.as_deref(), Some("api"));
        assert_eq!(nodes[1].component.as_deref(), Some("api"));
        assert_eq!(nodes[2].component.as_deref(), Some("ui"));

        // Sprawdź nazwy
        assert_eq!(nodes[0].name, "First task");
        assert_eq!(nodes[1].name, "Second task");
        assert_eq!(nodes[2].name, "Third task");

        // Sprawdź deps po apply_deps_and_models
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

        let nodes_with_deps = apply_deps_and_models(nodes, &deps_map, &models_map);

        assert_eq!(nodes_with_deps[0].deps.len(), 0, "Task 1 has no deps");
        assert_eq!(nodes_with_deps[1].deps, vec!["1"], "Task 2 depends on 1");
        assert_eq!(
            nodes_with_deps[2].deps,
            vec!["1", "2"],
            "Task 3 depends on 1 and 2"
        );
    }

    #[test]
    fn test_migrate_empty_progress() {
        use std::fs;
        use tempfile::TempDir;

        // Tworzymy tymczasowy katalog
        let temp = TempDir::new().unwrap();
        let temp_path = temp.path();

        // Tworzymy pusty PROGRESS.md
        let progress_path = temp_path.join("PROGRESS.md");
        fs::write(&progress_path, "").unwrap();

        // Konfiguracja
        let mut config = FileConfig::default();
        config.task.progress_file = progress_path.clone();
        config.task.tasks_file = temp_path.join(".ralph").join("tasks.yml");

        // Wykonaj migrację
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(execute(&config));

        // Migracja powinna przejść bez błędu
        assert!(
            result.is_ok(),
            "Migracja pustego PROGRESS.md powinna przejść bez błędu"
        );

        // Sprawdź że tasks.yml istnieje
        assert!(
            config.task.tasks_file.exists(),
            "tasks.yml powinien zostać utworzony"
        );

        // Wczytaj tasks.yml
        let tasks_file = TasksFile::load(&config.task.tasks_file).unwrap();

        // Sprawdź że lista tasków jest pusta
        assert!(
            tasks_file.tasks.is_empty(),
            "tasks.yml powinien zawierać pustą listę tasków"
        );

        // Sprawdź że default_model jest None
        assert!(
            tasks_file.default_model.is_none(),
            "default_model powinien być None"
        );
    }

    #[test]
    fn test_migrate_only_empty_frontmatter() {
        use std::fs;
        use tempfile::TempDir;

        // Tworzymy tymczasowy katalog
        let temp = TempDir::new().unwrap();
        let temp_path = temp.path();

        // PROGRESS.md z samym pustym frontmatterem
        let progress_path = temp_path.join("PROGRESS.md");
        fs::write(&progress_path, "---\n---\n").unwrap();

        // Konfiguracja
        let mut config = FileConfig::default();
        config.task.progress_file = progress_path.clone();
        config.task.tasks_file = temp_path.join(".ralph").join("tasks.yml");

        // Wykonaj migrację
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(execute(&config));

        // Migracja powinna przejść bez błędu
        assert!(
            result.is_ok(),
            "Migracja PROGRESS.md z samym frontmatterem powinna przejść"
        );

        // Sprawdź że tasks.yml istnieje
        assert!(config.task.tasks_file.exists());

        // Wczytaj tasks.yml
        let tasks_file = TasksFile::load(&config.task.tasks_file).unwrap();

        // Sprawdź że lista tasków jest pusta
        assert!(
            tasks_file.tasks.is_empty(),
            "tasks.yml powinien zawierać pustą listę tasków"
        );

        assert!(tasks_file.default_model.is_none());
    }

    #[test]
    fn test_migrate_frontmatter_with_default_model_no_tasks() {
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let temp_path = temp.path();

        // PROGRESS.md z frontmatterem zawierającym default_model, ale bez tasków
        let content = "---\ndefault_model: claude-opus-4-6\n---\n\n# No tasks here";
        let progress_path = temp_path.join("PROGRESS.md");
        fs::write(&progress_path, content).unwrap();

        let mut config = FileConfig::default();
        config.task.progress_file = progress_path.clone();
        config.task.tasks_file = temp_path.join(".ralph").join("tasks.yml");

        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(execute(&config));

        assert!(result.is_ok());
        assert!(config.task.tasks_file.exists());

        let tasks_file = TasksFile::load(&config.task.tasks_file).unwrap();

        // Lista tasków pusta
        assert!(tasks_file.tasks.is_empty());

        // default_model powinien być zachowany
        assert_eq!(
            tasks_file.default_model.as_deref(),
            Some("claude-opus-4-6"),
            "default_model z frontmattera powinien być zachowany"
        );
    }

    /// Test: ID '1' jest zarówno samodzielnym taskiem jak i prefixem dla '1.1'
    /// Trie builder powinien stworzyć hierarchię: '1' parent z '1.1' jako subtask
    #[test]
    fn test_id_as_prefix_and_leaf() {
        // Scenariusz: task '1' (done) i task '1.1' (todo)
        let summary = make_summary(
            vec![
                ("1", "core", "Parent task", TaskStatus::Done),
                ("1.1", "core", "Child task", TaskStatus::Todo),
            ],
            vec![],
        );

        let mut trie = HashMap::new();
        for task in &summary.tasks {
            let segments: Vec<&str> = task.id.split('.').collect();
            insert_trie(&mut trie, &segments, task);
        }

        let nodes = trie_to_nodes(&trie);

        // Powinien powstać jeden węzeł '1' z jednym subtaskiem '1.1'
        assert_eq!(nodes.len(), 1, "Powinien być jeden root node (ID '1')");
        assert_eq!(nodes[0].id, "1");
        assert_eq!(
            nodes[0].name, "Parent task",
            "Nazwa powinna pochodzić z taska '1'"
        );
        assert_eq!(
            nodes[0].component.as_deref(),
            Some("core"),
            "Komponent powinien pochodzić z taska '1'"
        );

        // '1' jest parentem, więc status powinien być None (computed)
        assert_eq!(
            nodes[0].status, None,
            "Parent node nie powinien mieć statusu (computed)"
        );

        // '1' powinien mieć jeden subtask
        assert_eq!(
            nodes[0].subtasks.len(),
            1,
            "Node '1' powinien mieć jeden subtask"
        );

        // Sprawdź subtask '1.1'
        let subtask = &nodes[0].subtasks[0];
        assert_eq!(subtask.id, "1.1");
        assert_eq!(subtask.name, "Child task");
        assert_eq!(subtask.component.as_deref(), Some("core"));
        assert_eq!(
            subtask.status,
            Some(TaskStatus::Todo),
            "Leaf node powinien mieć status"
        );
        assert!(subtask.subtasks.is_empty(), "1.1 powinien być leaf node");
    }
}
