use std::path::Path;

use crate::shared::error::{RalphError, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    Todo,
    Done,
    InProgress,
    Blocked,
}

#[derive(Debug, Clone)]
pub struct ProgressTask {
    pub id: String,
    pub component: String,
    pub name: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone)]
pub struct ProgressSummary {
    pub tasks: Vec<ProgressTask>,
    pub done: usize,
    pub in_progress: usize,
    pub blocked: usize,
    pub todo: usize,
}

impl ProgressSummary {
    pub fn total(&self) -> usize {
        self.done + self.in_progress + self.blocked + self.todo
    }

    pub fn remaining(&self) -> usize {
        self.todo + self.in_progress
    }
}

/// Parse a single PROGRESS.md line into a ProgressTask.
///
/// Expected format: `- [S] ID [component] name`
/// where S is one of: ` ` (todo), `x` (done), `~` (in progress), `!` (blocked)
fn parse_task_line(line: &str) -> Option<ProgressTask> {
    let trimmed = line.trim();

    // Must start with "- ["
    if !trimmed.starts_with("- [") {
        return None;
    }

    // Extract status char at position 3
    let rest = &trimmed[3..];
    let status_char = rest.chars().next()?;

    // Must be followed by "] "
    if rest.get(1..3).is_none_or(|s| s != "] ") {
        return None;
    }

    let status = match status_char {
        ' ' => TaskStatus::Todo,
        'x' => TaskStatus::Done,
        '~' => TaskStatus::InProgress,
        '!' => TaskStatus::Blocked,
        _ => return None,
    };

    // Rest after "- [S] " is "ID [component] name"
    let after_status = &rest[3..];

    // Find ID: first whitespace-delimited token (supports 1.2.3, H.1, etc.)
    let id_end = after_status.find(' ')?;
    let id = after_status[..id_end].to_string();

    // After ID, expect " [component] name" — strip optional backticks
    let after_id = after_status[id_end..].trim_start();
    let after_id = after_id.trim_start_matches('`');

    // Extract [component]
    if !after_id.starts_with('[') {
        return None;
    }
    let bracket_end = after_id.find(']')?;
    let component = after_id[1..bracket_end].to_string();

    // Rest is the task name — skip trailing backtick if present
    let name = after_id[bracket_end + 1..]
        .trim_start_matches('`')
        .trim()
        .to_string();
    if name.is_empty() {
        return None;
    }

    Some(ProgressTask {
        id,
        component,
        name,
        status,
    })
}

/// Tolerant parser for PROGRESS.md content.
/// Lines that don't match the expected format are silently skipped.
pub fn parse_progress(content: &str) -> ProgressSummary {
    let mut tasks = Vec::new();
    let mut done = 0;
    let mut in_progress = 0;
    let mut blocked = 0;
    let mut todo = 0;

    for line in content.lines() {
        if let Some(task) = parse_task_line(line) {
            match task.status {
                TaskStatus::Done => done += 1,
                TaskStatus::InProgress => in_progress += 1,
                TaskStatus::Blocked => blocked += 1,
                TaskStatus::Todo => todo += 1,
            }
            tasks.push(task);
        }
    }

    ProgressSummary {
        tasks,
        done,
        in_progress,
        blocked,
        todo,
    }
}

/// Get the current task: first in-progress [~], fallback to first todo [ ].
pub fn current_task(summary: &ProgressSummary) -> Option<&ProgressTask> {
    summary
        .tasks
        .iter()
        .find(|t| t.status == TaskStatus::InProgress)
        .or_else(|| {
            summary
                .tasks
                .iter()
                .find(|t| t.status == TaskStatus::Todo)
        })
}

/// Load and parse PROGRESS.md from a file path.
pub fn load_progress(path: &Path) -> Result<ProgressSummary> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| RalphError::MissingFile(format!("{}: {}", path.display(), e)))?;
    Ok(parse_progress(&content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_all_statuses() {
        let content = "\
- [ ] 1.1 [api] Create endpoint
- [x] 1.2 [api] Add tests
- [~] 1.3 [ui] Build form
- [!] 1.4 [infra] Deploy to prod";

        let summary = parse_progress(content);
        assert_eq!(summary.total(), 4);
        assert_eq!(summary.todo, 1);
        assert_eq!(summary.done, 1);
        assert_eq!(summary.in_progress, 1);
        assert_eq!(summary.blocked, 1);
    }

    #[test]
    fn test_parse_deep_ids() {
        let content = "\
- [ ] 1.1.1 [api] Nested task
- [ ] 1.1.1.1 [api] Deep nested task
- [ ] 2.3.4.5 [ui] Very deep";

        let summary = parse_progress(content);
        assert_eq!(summary.tasks.len(), 3);
        assert_eq!(summary.tasks[0].id, "1.1.1");
        assert_eq!(summary.tasks[1].id, "1.1.1.1");
        assert_eq!(summary.tasks[2].id, "2.3.4.5");
    }

    #[test]
    fn test_parse_housekeeping_ids() {
        let content = "\
- [ ] H.1 [all] Scan for code duplication
- [ ] H.2 [all] Update CLAUDE.md";

        let summary = parse_progress(content);
        assert_eq!(summary.tasks.len(), 2);
        assert_eq!(summary.tasks[0].id, "H.1");
        assert_eq!(summary.tasks[1].id, "H.2");
    }

    #[test]
    fn test_tolerant_parsing() {
        let content = "\
# PHASE 1: FOUNDATION

## Epic 0: Project Setup

### 0.1 Structure
- [ ] 0.1.1 [infra] Create directory structure
- [x] 0.1.2 [infra] Initialize version control

Some random text here
---

- [ ] 0.2.1 [infra] Set up deps";

        let summary = parse_progress(content);
        assert_eq!(summary.tasks.len(), 3);
        assert_eq!(summary.done, 1);
        assert_eq!(summary.todo, 2);
    }

    #[test]
    fn test_empty_content() {
        let summary = parse_progress("");
        assert_eq!(summary.total(), 0);
        assert_eq!(summary.remaining(), 0);
    }

    #[test]
    fn test_no_tasks() {
        let content = "# Progress\n\nNo tasks here.\n---\n";
        let summary = parse_progress(content);
        assert_eq!(summary.total(), 0);
    }

    #[test]
    fn test_current_task_in_progress_first() {
        let content = "\
- [ ] 1.1 [api] Todo task
- [~] 1.2 [api] In progress task
- [ ] 1.3 [api] Another todo";

        let summary = parse_progress(content);
        let current = current_task(&summary);
        assert!(current.is_some());
        assert_eq!(current.unwrap().id, "1.2");
    }

    #[test]
    fn test_current_task_fallback_to_todo() {
        let content = "\
- [x] 1.1 [api] Done task
- [ ] 1.2 [api] First todo
- [ ] 1.3 [api] Second todo";

        let summary = parse_progress(content);
        let current = current_task(&summary);
        assert!(current.is_some());
        assert_eq!(current.unwrap().id, "1.2");
    }

    #[test]
    fn test_current_task_all_done() {
        let content = "\
- [x] 1.1 [api] Done
- [x] 1.2 [api] Also done";

        let summary = parse_progress(content);
        let current = current_task(&summary);
        assert!(current.is_none());
    }

    #[test]
    fn test_remaining() {
        let content = "\
- [x] 1.1 [api] Done
- [~] 1.2 [api] Working
- [ ] 1.3 [api] Todo
- [!] 1.4 [api] Blocked
- [ ] 1.5 [api] Todo2";

        let summary = parse_progress(content);
        assert_eq!(summary.remaining(), 3); // 1 in_progress + 2 todo
    }

    #[test]
    fn test_component_extraction() {
        let content = "- [ ] 1.1 [backend-api] Create REST endpoint";
        let summary = parse_progress(content);
        assert_eq!(summary.tasks[0].component, "backend-api");
        assert_eq!(summary.tasks[0].name, "Create REST endpoint");
    }

    #[test]
    fn test_parse_backtick_component() {
        let content = "\
- [x] 0.0.1 `[infra]` Configure Playwright
- [ ] 0.0.2 `[ui]` Build landing page
- [~] 0.0.3 `[api]` Setup REST endpoints";

        let summary = parse_progress(content);
        assert_eq!(summary.tasks.len(), 3);
        assert_eq!(summary.tasks[0].id, "0.0.1");
        assert_eq!(summary.tasks[0].component, "infra");
        assert_eq!(summary.tasks[0].name, "Configure Playwright");
        assert_eq!(summary.done, 1);
        assert_eq!(summary.tasks[1].component, "ui");
        assert_eq!(summary.tasks[2].component, "api");
        assert_eq!(summary.in_progress, 1);
        assert_eq!(summary.todo, 1);
    }

    #[test]
    fn test_parse_mixed_backtick_and_plain() {
        let content = "\
- [x] 1.1 [api] Plain component
- [ ] 1.2 `[ui]` Backtick component";

        let summary = parse_progress(content);
        assert_eq!(summary.tasks.len(), 2);
        assert_eq!(summary.tasks[0].component, "api");
        assert_eq!(summary.tasks[1].component, "ui");
    }

    #[test]
    fn test_invalid_status_char_skipped() {
        let content = "\
- [?] 1.1 [api] Unknown status
- [ ] 1.2 [api] Valid task";

        let summary = parse_progress(content);
        assert_eq!(summary.tasks.len(), 1);
        assert_eq!(summary.tasks[0].id, "1.2");
    }
}
