use std::collections::{HashMap, HashSet, VecDeque};

use crate::shared::progress::ProgressFrontmatter;

/// Directed Acyclic Graph of task dependencies.
///
/// Maintains both forward (deps) and reverse (dependents) adjacency lists
/// for efficient traversal in both directions.
#[allow(dead_code)]
pub struct TaskDag {
    /// task_id → list of tasks it depends on (predecessors)
    deps: HashMap<String, Vec<String>>,
    /// task_id → list of tasks that depend on it (successors)
    reverse: HashMap<String, Vec<String>>,
    /// All known task IDs in the DAG
    all_tasks: HashSet<String>,
}

#[allow(dead_code)]
impl TaskDag {
    /// Create a TaskDag from parsed YAML frontmatter.
    ///
    /// Tasks mentioned in deps (either as keys or values) are automatically
    /// registered. Tasks with no deps entry are treated as independent.
    pub fn from_frontmatter(fm: &ProgressFrontmatter) -> Self {
        let mut deps: HashMap<String, Vec<String>> = HashMap::new();
        let mut reverse: HashMap<String, Vec<String>> = HashMap::new();
        let mut all_tasks = HashSet::new();

        for (task_id, task_deps) in &fm.deps {
            all_tasks.insert(task_id.clone());
            deps.insert(task_id.clone(), task_deps.clone());

            for dep in task_deps {
                all_tasks.insert(dep.clone());
                reverse
                    .entry(dep.clone())
                    .or_default()
                    .push(task_id.clone());
            }
        }

        Self {
            deps,
            reverse,
            all_tasks,
        }
    }

    /// Create an empty DAG.
    pub fn empty() -> Self {
        Self {
            deps: HashMap::new(),
            reverse: HashMap::new(),
            all_tasks: HashSet::new(),
        }
    }

    /// Register additional task IDs (e.g. from PROGRESS.md task list)
    /// that may not appear in the deps map.
    pub fn register_tasks(&mut self, task_ids: impl IntoIterator<Item = String>) {
        for id in task_ids {
            self.all_tasks.insert(id);
        }
    }

    /// Return all task IDs in the DAG.
    pub fn tasks(&self) -> &HashSet<String> {
        &self.all_tasks
    }

    /// Return deps for a given task (empty slice if none).
    pub fn task_deps(&self, task_id: &str) -> &[String] {
        self.deps.get(task_id).map_or(&[], |v| v.as_slice())
    }

    /// Return tasks that depend on the given task (successors).
    pub fn dependents(&self, task_id: &str) -> &[String] {
        self.reverse.get(task_id).map_or(&[], |v| v.as_slice())
    }

    /// Detect cycles using DFS with 3-color marking.
    ///
    /// Returns `Some(cycle_path)` if a cycle is found, `None` if the graph is acyclic.
    /// The cycle path contains the task IDs forming the cycle.
    pub fn detect_cycles(&self) -> Option<Vec<String>> {
        // White=unvisited, Gray=in current path, Black=fully processed
        let mut white: HashSet<&str> = self.all_tasks.iter().map(|s| s.as_str()).collect();
        let mut gray: HashSet<&str> = HashSet::new();
        let mut black: HashSet<&str> = HashSet::new();
        let mut path: Vec<String> = Vec::new();

        while let Some(&node) = white.iter().next() {
            if let Some(cycle) = self.dfs_cycle(node, &mut white, &mut gray, &mut black, &mut path)
            {
                return Some(cycle);
            }
        }
        None
    }

    fn dfs_cycle<'a>(
        &'a self,
        node: &'a str,
        white: &mut HashSet<&'a str>,
        gray: &mut HashSet<&'a str>,
        black: &mut HashSet<&'a str>,
        path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        white.remove(node);
        gray.insert(node);
        path.push(node.to_string());

        // Visit dependencies (predecessors) — if a dep is gray, we found a cycle
        for dep in self.task_deps(node) {
            if black.contains(dep.as_str()) {
                continue;
            }
            if gray.contains(dep.as_str()) {
                // Found cycle: extract the cycle from path
                let cycle_start = path.iter().position(|s| s == dep).unwrap();
                let mut cycle: Vec<String> = path[cycle_start..].to_vec();
                cycle.push(dep.clone()); // close the cycle
                return Some(cycle);
            }
            if white.contains(dep.as_str())
                && let Some(cycle) = self.dfs_cycle(dep, white, gray, black, path)
            {
                return Some(cycle);
            }
        }

        path.pop();
        gray.remove(node);
        black.insert(node);
        None
    }

    /// Topological sort using Kahn's algorithm.
    ///
    /// Returns tasks in dependency order (tasks with no deps first).
    /// Returns `Err` with cycle path if the graph contains cycles.
    pub fn topological_sort(&self) -> Result<Vec<String>, Vec<String>> {
        // Compute in-degree for each task (number of deps)
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for task in &self.all_tasks {
            in_degree.insert(task.as_str(), self.task_deps(task).len());
        }

        // Start with tasks that have zero in-degree (no deps)
        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(&task, _)| task.to_string())
            .collect();
        // Sort queue for deterministic order
        let mut sorted_queue: Vec<String> = queue.drain(..).collect();
        sorted_queue.sort_by(|a, b| compare_task_ids(a, b));
        queue.extend(sorted_queue);

        let mut result = Vec::new();

        while let Some(task) = queue.pop_front() {
            result.push(task.clone());

            // For each task that depends on this one, decrement in-degree
            for dependent in self.dependents(&task) {
                if let Some(deg) = in_degree.get_mut(dependent.as_str()) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dependent.clone());
                    }
                }
            }
        }

        if result.len() != self.all_tasks.len() {
            // Cycle detected — find and return cycle path
            if let Some(cycle) = self.detect_cycles() {
                Err(cycle)
            } else {
                Err(vec!["unknown cycle".to_string()])
            }
        } else {
            Ok(result)
        }
    }

    /// Return tasks that are ready to execute: all deps are in `done` set,
    /// not currently `in_progress`, and not already `done`.
    ///
    /// Results are sorted by task ID (numeric-aware comparison).
    pub fn ready_tasks(
        &self,
        done: &HashSet<String>,
        in_progress: &HashSet<String>,
    ) -> Vec<String> {
        let mut ready: Vec<String> = self
            .all_tasks
            .iter()
            .filter(|task| {
                // Not already done or in progress
                !done.contains(task.as_str()) && !in_progress.contains(task.as_str())
            })
            .filter(|task| {
                // All deps must be done
                self.task_deps(task)
                    .iter()
                    .all(|dep| done.contains(dep.as_str()))
            })
            .cloned()
            .collect();

        ready.sort_by(|a, b| compare_task_ids(a, b));
        ready
    }
}

/// Compare task IDs with numeric-aware sorting.
///
/// Splits IDs by `.` and compares segments numerically when possible,
/// falling back to lexicographic comparison for non-numeric segments.
/// Example: "1.2.10" > "1.2.2" (numeric), "H.1" > "1.1" (lexicographic)
fn compare_task_ids(a: &str, b: &str) -> std::cmp::Ordering {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();

    for (ap, bp) in a_parts.iter().zip(b_parts.iter()) {
        let ord = match (ap.parse::<u64>(), bp.parse::<u64>()) {
            (Ok(an), Ok(bn)) => an.cmp(&bn),
            _ => ap.cmp(bp),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    a_parts.len().cmp(&b_parts.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frontmatter(deps: Vec<(&str, Vec<&str>)>) -> ProgressFrontmatter {
        let mut fm = ProgressFrontmatter::default();
        for (task, task_deps) in deps {
            fm.deps.insert(
                task.to_string(),
                task_deps.into_iter().map(|s| s.to_string()).collect(),
            );
        }
        fm
    }

    // --- 1.2.1: TaskDag struct ---

    #[test]
    fn test_empty_dag() {
        let dag = TaskDag::empty();
        assert!(dag.tasks().is_empty());
        assert!(dag.detect_cycles().is_none());
    }

    #[test]
    fn test_from_frontmatter_basic() {
        let fm = make_frontmatter(vec![
            ("T02", vec!["T01"]),
            ("T03", vec!["T01", "T02"]),
            ("T01", vec![]),
        ]);
        let dag = TaskDag::from_frontmatter(&fm);
        assert_eq!(dag.tasks().len(), 3);
        assert!(dag.task_deps("T01").is_empty());
        assert_eq!(dag.task_deps("T02"), &["T01"]);
        assert_eq!(dag.dependents("T01").len(), 2); // T02 and T03 depend on T01
    }

    #[test]
    fn test_register_additional_tasks() {
        let fm = make_frontmatter(vec![("T02", vec!["T01"])]);
        let mut dag = TaskDag::from_frontmatter(&fm);
        dag.register_tasks(vec!["T03".to_string(), "T04".to_string()]);
        assert_eq!(dag.tasks().len(), 4);
    }

    // --- 1.2.2: detect_cycles ---

    #[test]
    fn test_no_cycle_linear() {
        // T01 → T02 → T03
        let fm = make_frontmatter(vec![
            ("T01", vec![]),
            ("T02", vec!["T01"]),
            ("T03", vec!["T02"]),
        ]);
        let dag = TaskDag::from_frontmatter(&fm);
        assert!(dag.detect_cycles().is_none());
    }

    #[test]
    fn test_no_cycle_diamond() {
        //     T01
        //    /    \
        //  T02   T03
        //    \    /
        //     T04
        let fm = make_frontmatter(vec![
            ("T01", vec![]),
            ("T02", vec!["T01"]),
            ("T03", vec!["T01"]),
            ("T04", vec!["T02", "T03"]),
        ]);
        let dag = TaskDag::from_frontmatter(&fm);
        assert!(dag.detect_cycles().is_none());
    }

    #[test]
    fn test_cycle_detection() {
        // T01 → T02 → T03 → T01 (cycle)
        let fm = make_frontmatter(vec![
            ("T01", vec!["T03"]),
            ("T02", vec!["T01"]),
            ("T03", vec!["T02"]),
        ]);
        let dag = TaskDag::from_frontmatter(&fm);
        let cycle = dag.detect_cycles();
        assert!(cycle.is_some());
        let cycle = cycle.unwrap();
        assert!(cycle.len() >= 3); // at least 3 nodes in the cycle
    }

    #[test]
    fn test_self_cycle() {
        // T01 depends on itself
        let fm = make_frontmatter(vec![("T01", vec!["T01"])]);
        let dag = TaskDag::from_frontmatter(&fm);
        let cycle = dag.detect_cycles();
        assert!(cycle.is_some());
    }

    // --- 1.2.3: topological_sort ---

    #[test]
    fn test_topological_sort_linear() {
        let fm = make_frontmatter(vec![
            ("T01", vec![]),
            ("T02", vec!["T01"]),
            ("T03", vec!["T02"]),
        ]);
        let dag = TaskDag::from_frontmatter(&fm);
        let sorted = dag.topological_sort().unwrap();
        assert_eq!(sorted, vec!["T01", "T02", "T03"]);
    }

    #[test]
    fn test_topological_sort_diamond() {
        let fm = make_frontmatter(vec![
            ("T01", vec![]),
            ("T02", vec!["T01"]),
            ("T03", vec!["T01"]),
            ("T04", vec!["T02", "T03"]),
        ]);
        let dag = TaskDag::from_frontmatter(&fm);
        let sorted = dag.topological_sort().unwrap();
        // T01 must come first, T04 last
        assert_eq!(sorted[0], "T01");
        assert_eq!(*sorted.last().unwrap(), "T04");
        // T02 and T03 can be in any order but both after T01
        let t02_pos = sorted.iter().position(|s| s == "T02").unwrap();
        let t03_pos = sorted.iter().position(|s| s == "T03").unwrap();
        assert!(t02_pos > 0 && t03_pos > 0);
    }

    #[test]
    fn test_topological_sort_cycle_error() {
        let fm = make_frontmatter(vec![
            ("T01", vec!["T03"]),
            ("T02", vec!["T01"]),
            ("T03", vec!["T02"]),
        ]);
        let dag = TaskDag::from_frontmatter(&fm);
        assert!(dag.topological_sort().is_err());
    }

    #[test]
    fn test_topological_sort_disconnected() {
        // Two independent chains
        let fm = make_frontmatter(vec![
            ("A1", vec![]),
            ("A2", vec!["A1"]),
            ("B1", vec![]),
            ("B2", vec!["B1"]),
        ]);
        let dag = TaskDag::from_frontmatter(&fm);
        let sorted = dag.topological_sort().unwrap();
        assert_eq!(sorted.len(), 4);
        // A1 before A2, B1 before B2
        let a1 = sorted.iter().position(|s| s == "A1").unwrap();
        let a2 = sorted.iter().position(|s| s == "A2").unwrap();
        let b1 = sorted.iter().position(|s| s == "B1").unwrap();
        let b2 = sorted.iter().position(|s| s == "B2").unwrap();
        assert!(a1 < a2);
        assert!(b1 < b2);
    }

    // --- 1.2.4: ready_tasks ---

    #[test]
    fn test_ready_tasks_initial() {
        let fm = make_frontmatter(vec![
            ("T01", vec![]),
            ("T02", vec!["T01"]),
            ("T03", vec![]),
            ("T04", vec!["T02", "T03"]),
        ]);
        let dag = TaskDag::from_frontmatter(&fm);
        let done = HashSet::new();
        let in_progress = HashSet::new();
        let ready = dag.ready_tasks(&done, &in_progress);
        // Only T01 and T03 have no deps
        assert_eq!(ready, vec!["T01", "T03"]);
    }

    #[test]
    fn test_ready_tasks_partial_completion() {
        let fm = make_frontmatter(vec![
            ("T01", vec![]),
            ("T02", vec!["T01"]),
            ("T03", vec!["T01"]),
            ("T04", vec!["T02", "T03"]),
        ]);
        let dag = TaskDag::from_frontmatter(&fm);

        let done: HashSet<String> = ["T01"].iter().map(|s| s.to_string()).collect();
        let in_progress: HashSet<String> = ["T02"].iter().map(|s| s.to_string()).collect();

        let ready = dag.ready_tasks(&done, &in_progress);
        // T03 is ready (T01 done), T02 is in_progress, T04 not ready (T02 not done)
        assert_eq!(ready, vec!["T03"]);
    }

    #[test]
    fn test_ready_tasks_all_done() {
        let fm = make_frontmatter(vec![("T01", vec![]), ("T02", vec!["T01"])]);
        let dag = TaskDag::from_frontmatter(&fm);
        let done: HashSet<String> = ["T01", "T02"].iter().map(|s| s.to_string()).collect();
        let ready = dag.ready_tasks(&done, &HashSet::new());
        assert!(ready.is_empty());
    }

    // --- compare_task_ids ---

    #[test]
    fn test_compare_task_ids_numeric() {
        assert!(compare_task_ids("1.2", "1.10") == std::cmp::Ordering::Less);
        assert!(compare_task_ids("1.1.1", "1.1.2") == std::cmp::Ordering::Less);
        assert!(compare_task_ids("2.1", "1.9") == std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_compare_task_ids_mixed() {
        // H prefix sorts after numeric (lexicographic)
        assert!(compare_task_ids("H.1", "1.1") == std::cmp::Ordering::Greater);
        assert!(compare_task_ids("1.1", "H.1") == std::cmp::Ordering::Less);
    }

    #[test]
    fn test_compare_task_ids_depth() {
        assert!(compare_task_ids("1.1", "1.1.1") == std::cmp::Ordering::Less);
        assert!(compare_task_ids("1.1.1", "1.1") == std::cmp::Ordering::Greater);
    }
}
