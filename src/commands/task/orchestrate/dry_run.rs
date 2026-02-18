use std::collections::{HashMap, HashSet};

use crate::shared::dag::TaskDag;
use crate::shared::file_config::VerifyProfile;
use crate::shared::progress::{ProgressSummary, TaskStatus};
use crate::shared::tasks::TasksFile;

/// Layered representation of the DAG for visualization.
///
/// Tasks are grouped by their "distance from root" (longest path from any root node).
/// Layer 0 = tasks with no dependencies, Layer 1 = depends only on Layer 0, etc.
#[derive(Debug)]
pub struct DagVisualization {
    pub layers: Vec<Vec<String>>,
    pub task_status: HashMap<String, String>,
}

/// Generate a layered DAG visualization for dry-run output.
pub fn visualize_dag(dag: &TaskDag, progress: &ProgressSummary) -> DagVisualization {
    let all_tasks = dag.tasks();
    let mut task_layer: HashMap<String, usize> = HashMap::new();

    // Build status lookup
    let task_status: HashMap<String, String> = progress
        .tasks
        .iter()
        .map(|t| {
            let status = match &t.status {
                TaskStatus::Done => "x",
                TaskStatus::InProgress => "~",
                TaskStatus::Blocked => "!",
                TaskStatus::Todo => " ",
            };
            (t.id.clone(), status.to_string())
        })
        .collect();

    // Compute layers via BFS-like approach: layer = max(dep layers) + 1
    let mut changed = true;
    while changed {
        changed = false;
        for task in all_tasks {
            let deps = dag.task_deps(task);
            let layer = if deps.is_empty() {
                0
            } else {
                let max_dep_layer = deps.iter().filter_map(|d| task_layer.get(d)).max().copied();
                match max_dep_layer {
                    Some(l) => l + 1,
                    None => continue, // deps not computed yet
                }
            };

            let prev = task_layer.get(task).copied();
            if prev.is_none_or(|p| layer > p) {
                task_layer.insert(task.to_string(), layer);
                changed = true;
            }
        }
    }

    // Group tasks by layer
    let max_layer = task_layer.values().max().copied().unwrap_or(0);
    let mut layers: Vec<Vec<String>> = vec![Vec::new(); max_layer + 1];
    for (task, layer) in &task_layer {
        layers[*layer].push(task.clone());
    }

    // Sort each layer by task ID
    for layer in &mut layers {
        layer.sort();
    }

    DagVisualization {
        layers,
        task_status,
    }
}

/// Format the DAG visualization as a printable string.
pub fn format_dag(viz: &DagVisualization, workers: u32) -> String {
    let mut lines = Vec::new();

    lines.push(format!("DAG Visualization ({} workers)\n", workers));

    let total_tasks: usize = viz.layers.iter().map(|l| l.len()).sum();
    let done_tasks: usize = viz
        .task_status
        .values()
        .filter(|s| s.as_str() == "x")
        .count();

    lines.push(format!("Tasks: {total_tasks} total, {done_tasks} done\n"));

    for (i, layer) in viz.layers.iter().enumerate() {
        let tasks_str: Vec<String> = layer
            .iter()
            .map(|t| {
                let status = viz.task_status.get(t).map(|s| s.as_str()).unwrap_or(" ");
                format!("[{status}] {t}")
            })
            .collect();
        lines.push(format!("  Layer {i}: {}", tasks_str.join("  ")));
    }

    // Estimated serial rounds
    let serial_rounds = estimate_rounds(&viz.layers, workers);
    lines.push(format!(
        "\nEstimated serial rounds with {workers} workers: {serial_rounds}"
    ));

    lines.join("\n")
}

/// Estimate number of serial rounds needed with N workers.
///
/// Each layer can run min(tasks_in_layer, workers) tasks in parallel.
/// The round count is ceil(tasks_in_layer / workers) per layer, summed.
fn estimate_rounds(layers: &[Vec<String>], workers: u32) -> usize {
    layers
        .iter()
        .map(|layer| {
            let tasks = layer.len() as u32;
            tasks.div_ceil(workers) as usize
        })
        .sum()
}

/// Generate a simple dependency list for dry-run preview.
///
/// Now includes profile information for each task that has profiles assigned.
/// For each profile, shows setup and verify commands.
pub fn format_dep_list(
    dag: &TaskDag,
    progress: &ProgressSummary,
    tasks_file: &TasksFile,
    profiles: &[VerifyProfile],
) -> String {
    let mut lines = Vec::new();
    let done: HashSet<String> = progress
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Done)
        .map(|t| t.id.clone())
        .collect();

    for task in &progress.tasks {
        let deps = dag.task_deps(&task.id);
        let status = match &task.status {
            TaskStatus::Done => "x",
            TaskStatus::InProgress => "~",
            TaskStatus::Blocked => "!",
            TaskStatus::Todo => " ",
        };
        let deps_str = if deps.is_empty() {
            "none".to_string()
        } else {
            deps.iter()
                .map(|d| {
                    if done.contains(d) {
                        format!("{d} ✓")
                    } else {
                        d.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        lines.push(format!(
            "  [{status}] {} [{}] {} ← deps: {deps_str}",
            task.id, task.component, task.name
        ));

        // Show profile information if task has profiles assigned
        if let Some(task_node) = tasks_file.find_node(&task.id)
            && !task_node.profiles.is_empty()
        {
            lines.push(format!("    Profiles: [{}]", task_node.profiles.join(", ")));

            // For each profile, show setup and verify commands
            for profile_name in &task_node.profiles {
                if let Some(profile) = profiles.iter().find(|p| &p.name == profile_name) {
                    // Setup commands
                    if !profile.setup_commands.is_empty() {
                        let setup_labels: Vec<&str> = profile
                            .setup_commands
                            .iter()
                            .map(|cmd| cmd.label())
                            .collect();
                        lines.push(format!(
                            "      Setup ({}): {}",
                            profile_name,
                            setup_labels.join(", ")
                        ));
                    }

                    // Verify commands
                    if !profile.verify_commands.is_empty() {
                        let verify_labels: Vec<&str> = profile
                            .verify_commands
                            .iter()
                            .map(|cmd| cmd.label())
                            .collect();
                        lines.push(format!(
                            "      Verify ({}): {}",
                            profile_name,
                            verify_labels.join(", ")
                        ));
                    }
                }
            }
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::file_config::{SetupCommand, VerifyCommand, VerifyProfile};
    use crate::shared::progress::{ProgressFrontmatter, ProgressTask};
    use crate::shared::tasks::TaskNode;

    fn make_dag(deps: Vec<(&str, Vec<&str>)>) -> TaskDag {
        let mut fm = ProgressFrontmatter::default();
        for (task, task_deps) in deps {
            fm.deps.insert(
                task.to_string(),
                task_deps.into_iter().map(|s| s.to_string()).collect(),
            );
        }
        TaskDag::from_frontmatter(&fm)
    }

    fn make_progress(tasks: Vec<(&str, &str, TaskStatus)>) -> ProgressSummary {
        let task_vec: Vec<ProgressTask> = tasks
            .into_iter()
            .map(|(id, comp, status)| ProgressTask {
                id: id.to_string(),
                component: comp.to_string(),
                name: format!("Task {id}"),
                status,
            })
            .collect();

        let done = task_vec
            .iter()
            .filter(|t| t.status == TaskStatus::Done)
            .count();
        let todo = task_vec
            .iter()
            .filter(|t| t.status == TaskStatus::Todo)
            .count();

        ProgressSummary {
            tasks: task_vec,
            done,
            in_progress: 0,
            blocked: 0,
            todo,
            frontmatter: None,
        }
    }

    fn make_empty_tasks_file() -> TasksFile {
        TasksFile {
            default_model: None,
            tasks: Vec::new(),
        }
    }

    #[test]
    fn test_visualize_linear_dag() {
        let dag = make_dag(vec![
            ("T01", vec![]),
            ("T02", vec!["T01"]),
            ("T03", vec!["T02"]),
        ]);
        let progress = make_progress(vec![
            ("T01", "api", TaskStatus::Todo),
            ("T02", "api", TaskStatus::Todo),
            ("T03", "api", TaskStatus::Todo),
        ]);

        let viz = visualize_dag(&dag, &progress);
        assert_eq!(viz.layers.len(), 3);
        assert_eq!(viz.layers[0], vec!["T01"]);
        assert_eq!(viz.layers[1], vec!["T02"]);
        assert_eq!(viz.layers[2], vec!["T03"]);
    }

    #[test]
    fn test_visualize_diamond_dag() {
        let dag = make_dag(vec![
            ("T01", vec![]),
            ("T02", vec!["T01"]),
            ("T03", vec!["T01"]),
            ("T04", vec!["T02", "T03"]),
        ]);
        let progress = make_progress(vec![
            ("T01", "api", TaskStatus::Todo),
            ("T02", "api", TaskStatus::Todo),
            ("T03", "ui", TaskStatus::Todo),
            ("T04", "api", TaskStatus::Todo),
        ]);

        let viz = visualize_dag(&dag, &progress);
        assert_eq!(viz.layers.len(), 3);
        assert_eq!(viz.layers[0], vec!["T01"]);
        assert!(viz.layers[1].contains(&"T02".to_string()));
        assert!(viz.layers[1].contains(&"T03".to_string()));
        assert_eq!(viz.layers[2], vec!["T04"]);
    }

    #[test]
    fn test_estimate_rounds() {
        // 3 layers: [2 tasks], [3 tasks], [1 task]
        let layers = vec![
            vec!["T01".to_string(), "T02".to_string()],
            vec!["T03".to_string(), "T04".to_string(), "T05".to_string()],
            vec!["T06".to_string()],
        ];

        // With 2 workers: ceil(2/2) + ceil(3/2) + ceil(1/2) = 1 + 2 + 1 = 4
        assert_eq!(estimate_rounds(&layers, 2), 4);
        // With 3 workers: ceil(2/3) + ceil(3/3) + ceil(1/3) = 1 + 1 + 1 = 3
        assert_eq!(estimate_rounds(&layers, 3), 3);
    }

    #[test]
    fn test_format_dag_output() {
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec!["T01"])]);
        let progress = make_progress(vec![
            ("T01", "api", TaskStatus::Done),
            ("T02", "api", TaskStatus::Todo),
        ]);

        let viz = visualize_dag(&dag, &progress);
        let output = format_dag(&viz, 2);

        assert!(output.contains("Layer 0"));
        assert!(output.contains("Layer 1"));
        assert!(output.contains("[x] T01"));
        assert!(output.contains("[ ] T02"));
        assert!(output.contains("Estimated serial rounds"));
    }

    #[test]
    fn test_format_dep_list() {
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec!["T01"])]);
        let progress = make_progress(vec![
            ("T01", "api", TaskStatus::Done),
            ("T02", "api", TaskStatus::Todo),
        ]);
        let tasks_file = make_empty_tasks_file();
        let profiles: Vec<VerifyProfile> = Vec::new();

        let output = format_dep_list(&dag, &progress, &tasks_file, &profiles);
        assert!(output.contains("[x] T01"));
        assert!(output.contains("[ ] T02"));
        assert!(output.contains("T01 ✓"));
    }

    #[test]
    fn test_empty_dag() {
        let dag = make_dag(vec![]);
        let progress = make_progress(vec![]);
        let viz = visualize_dag(&dag, &progress);
        assert!(viz.layers.is_empty() || viz.layers.iter().all(|l| l.is_empty()));
    }

    #[test]
    fn test_format_dep_list_with_profiles() {
        let dag = make_dag(vec![("T01", vec![]), ("T02", vec!["T01"])]);
        let progress = make_progress(vec![
            ("T01", "frontend", TaskStatus::Todo),
            ("T02", "backend", TaskStatus::Todo),
        ]);

        // Create tasks with profiles
        let mut tasks_file = make_empty_tasks_file();
        tasks_file.tasks = vec![
            TaskNode {
                id: "T01".to_string(),
                name: "Frontend task".to_string(),
                component: Some("frontend".to_string()),
                status: Some(TaskStatus::Todo),
                deps: Vec::new(),
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                acceptance_criteria: Vec::new(),
                profiles: vec!["frontend".to_string()],
                subtasks: Vec::new(),
            },
            TaskNode {
                id: "T02".to_string(),
                name: "Backend task".to_string(),
                component: Some("backend".to_string()),
                status: Some(TaskStatus::Todo),
                deps: vec!["T01".to_string()],
                model: None,
                description: None,
                related_files: Vec::new(),
                implementation_steps: Vec::new(),
                acceptance_criteria: Vec::new(),
                profiles: vec!["backend".to_string()],
                subtasks: Vec::new(),
            },
        ];

        // Create profiles
        let profiles = vec![
            VerifyProfile {
                name: "frontend".to_string(),
                description: Some("Frontend tests".to_string()),
                paths: vec!["src/ui/**/*.rs".to_string()],
                working_dir: None,
                verify_commands: vec![
                    VerifyCommand::Simple("npm test".to_string()),
                    VerifyCommand::Detailed {
                        command: "npm run lint".to_string(),
                        name: Some("Lint".to_string()),
                        description: None,
                    },
                ],
                setup_commands: vec![SetupCommand::Simple("npm install".to_string())],
            },
            VerifyProfile {
                name: "backend".to_string(),
                description: Some("Backend tests".to_string()),
                paths: vec!["src/api/**/*.rs".to_string()],
                working_dir: None,
                verify_commands: vec![VerifyCommand::Simple("cargo test".to_string())],
                setup_commands: vec![SetupCommand::Detailed {
                    run: "cargo build".to_string(),
                    name: Some("Build".to_string()),
                }],
            },
        ];

        let output = format_dep_list(&dag, &progress, &tasks_file, &profiles);

        // Check basic task info
        assert!(output.contains("[ ] T01"));
        assert!(output.contains("[ ] T02"));

        // Check profile info for T01
        assert!(output.contains("Profiles: [frontend]"));
        assert!(output.contains("Setup (frontend): npm install"));
        assert!(output.contains("Verify (frontend): npm test, Lint"));

        // Check profile info for T02
        assert!(output.contains("Profiles: [backend]"));
        assert!(output.contains("Setup (backend): Build"));
        assert!(output.contains("Verify (backend): cargo test"));
    }

    #[test]
    fn test_format_dep_list_with_multiple_profiles() {
        let dag = make_dag(vec![("T01", vec![])]);
        let progress = make_progress(vec![("T01", "fullstack", TaskStatus::Todo)]);

        // Create task with multiple profiles
        let mut tasks_file = make_empty_tasks_file();
        tasks_file.tasks = vec![TaskNode {
            id: "T01".to_string(),
            name: "Fullstack task".to_string(),
            component: Some("fullstack".to_string()),
            status: Some(TaskStatus::Todo),
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            acceptance_criteria: Vec::new(),
            profiles: vec!["frontend".to_string(), "backend".to_string()],
            subtasks: Vec::new(),
        }];

        // Create profiles
        let profiles = vec![
            VerifyProfile {
                name: "frontend".to_string(),
                description: None,
                paths: vec![],
                working_dir: None,
                verify_commands: vec![VerifyCommand::Simple("npm test".to_string())],
                setup_commands: vec![SetupCommand::Simple("npm install".to_string())],
            },
            VerifyProfile {
                name: "backend".to_string(),
                description: None,
                paths: vec![],
                working_dir: None,
                verify_commands: vec![VerifyCommand::Simple("cargo test".to_string())],
                setup_commands: vec![SetupCommand::Simple("cargo build".to_string())],
            },
        ];

        let output = format_dep_list(&dag, &progress, &tasks_file, &profiles);

        // Check that both profiles are listed
        assert!(output.contains("Profiles: [frontend, backend]"));

        // Check that both profiles' commands are shown
        assert!(output.contains("Setup (frontend): npm install"));
        assert!(output.contains("Verify (frontend): npm test"));
        assert!(output.contains("Setup (backend): cargo build"));
        assert!(output.contains("Verify (backend): cargo test"));
    }

    #[test]
    fn test_format_dep_list_with_no_profiles() {
        let dag = make_dag(vec![("T01", vec![])]);
        let progress = make_progress(vec![("T01", "api", TaskStatus::Todo)]);

        // Task without profiles
        let mut tasks_file = make_empty_tasks_file();
        tasks_file.tasks = vec![TaskNode {
            id: "T01".to_string(),
            name: "Simple task".to_string(),
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

        let profiles: Vec<VerifyProfile> = Vec::new();
        let output = format_dep_list(&dag, &progress, &tasks_file, &profiles);

        // Should not contain profile info
        assert!(!output.contains("Profiles:"));
        assert!(!output.contains("Setup"));
        assert!(!output.contains("Verify"));
    }
}
