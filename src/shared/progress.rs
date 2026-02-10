use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::shared::error::{RalphError, Result};

/// YAML frontmatter from PROGRESS.md containing dependency graph,
/// per-task model overrides, and default model.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProgressFrontmatter {
    /// Task dependency map: task_id → list of task_ids it depends on
    #[serde(default)]
    pub deps: HashMap<String, Vec<String>>,
    /// Per-task model overrides: task_id → model name
    #[serde(default)]
    pub models: HashMap<String, String>,
    /// Default model for all tasks (overridden by `models` entries)
    #[serde(default)]
    pub default_model: Option<String>,
}

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
    /// Parsed YAML frontmatter (None if no frontmatter present)
    #[allow(dead_code)]
    pub frontmatter: Option<ProgressFrontmatter>,
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

/// Extract YAML frontmatter between `---` markers and return it with the remaining body.
///
/// Returns `(Some(frontmatter), body)` if valid YAML frontmatter is found,
/// or `(None, original_content)` on missing/malformed frontmatter.
pub fn parse_frontmatter(content: &str) -> (Option<ProgressFrontmatter>, &str) {
    // Frontmatter must start with "---" on the first line
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content);
    }

    // Skip opening "---" and the newline after it
    let after_opening = &trimmed[3..];
    let after_opening = after_opening.strip_prefix('\n').unwrap_or(after_opening);

    // Find the closing "---" at the start of a line
    // It can be at the very beginning (empty frontmatter) or after a newline
    let (yaml_str, body) = if let Some(rest) = after_opening.strip_prefix("---") {
        // Empty frontmatter: ---\n---
        ("", rest.strip_prefix('\n').unwrap_or(rest))
    } else if let Some(close_pos) = after_opening.find("\n---") {
        let yaml = &after_opening[..close_pos];
        let rest = &after_opening[close_pos + 4..]; // skip "\n---"
        (yaml, rest.strip_prefix('\n').unwrap_or(rest))
    } else {
        return (None, content);
    };

    // Empty YAML block → default frontmatter
    if yaml_str.trim().is_empty() {
        return (Some(ProgressFrontmatter::default()), body);
    }

    match serde_yaml::from_str::<ProgressFrontmatter>(yaml_str) {
        Ok(fm) => (Some(fm), body),
        Err(_) => {
            // Tolerant: malformed YAML → treat as no frontmatter
            (None, content)
        }
    }
}

/// Serialize a ProgressFrontmatter back to a YAML string with `---` delimiters.
///
/// Produces a string like:
/// ```text
/// ---
/// deps:
///   T02:
///   - T01
/// ---
/// ```
#[allow(dead_code)]
pub fn write_frontmatter(fm: &ProgressFrontmatter) -> String {
    let yaml = serde_yaml::to_string(fm).unwrap_or_default();
    format!("---\n{}---\n", yaml)
}

/// Tolerant parser for PROGRESS.md content.
/// Parses YAML frontmatter (if present) and task lines.
/// Lines that don't match the expected format are silently skipped.
pub fn parse_progress(content: &str) -> ProgressSummary {
    let (frontmatter, body) = parse_frontmatter(content);

    let mut tasks = Vec::new();
    let mut done = 0;
    let mut in_progress = 0;
    let mut blocked = 0;
    let mut todo = 0;

    for line in body.lines() {
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
        frontmatter,
    }
}

/// Get the current task: first in-progress [~], fallback to first todo [ ].
pub fn current_task(summary: &ProgressSummary) -> Option<&ProgressTask> {
    summary
        .tasks
        .iter()
        .find(|t| t.status == TaskStatus::InProgress)
        .or_else(|| summary.tasks.iter().find(|t| t.status == TaskStatus::Todo))
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

    // --- Frontmatter tests ---

    #[test]
    fn test_frontmatter_deps_only() {
        let content = "\
---
deps:
  T02: [T01]
  T03: [T01, T02]
---
- [ ] T01 [api] First
- [ ] T02 [api] Second
- [ ] T03 [ui] Third";

        let summary = parse_progress(content);
        assert_eq!(summary.tasks.len(), 3);
        let fm = summary.frontmatter.as_ref().unwrap();
        assert_eq!(fm.deps.len(), 2);
        assert_eq!(fm.deps["T02"], vec!["T01"]);
        assert_eq!(fm.deps["T03"], vec!["T01", "T02"]);
        assert!(fm.models.is_empty());
        assert!(fm.default_model.is_none());
    }

    #[test]
    fn test_frontmatter_models_only() {
        let content = "\
---
models:
  T01: claude-opus-4-6
  T03: claude-haiku-4-5-20251001
---
- [ ] T01 [api] First";

        let summary = parse_progress(content);
        let fm = summary.frontmatter.as_ref().unwrap();
        assert!(fm.deps.is_empty());
        assert_eq!(fm.models.len(), 2);
        assert_eq!(fm.models["T01"], "claude-opus-4-6");
    }

    #[test]
    fn test_frontmatter_all_fields() {
        let content = "\
---
deps:
  T02: [T01]
models:
  T01: claude-opus-4-6
default_model: claude-sonnet-4-5-20250929
---
- [ ] T01 [api] First
- [ ] T02 [api] Second";

        let summary = parse_progress(content);
        let fm = summary.frontmatter.as_ref().unwrap();
        assert_eq!(fm.deps.len(), 1);
        assert_eq!(fm.models.len(), 1);
        assert_eq!(fm.default_model.as_deref(), Some("claude-sonnet-4-5-20250929"));
    }

    #[test]
    fn test_frontmatter_empty_yaml() {
        let content = "\
---
---
- [ ] T01 [api] First";

        let summary = parse_progress(content);
        let fm = summary.frontmatter.as_ref().unwrap();
        assert!(fm.deps.is_empty());
        assert!(fm.models.is_empty());
        assert!(fm.default_model.is_none());
    }

    #[test]
    fn test_frontmatter_backward_compat_no_frontmatter() {
        let content = "\
# PROGRESS
- [ ] T01 [api] First
- [x] T02 [api] Second";

        let summary = parse_progress(content);
        assert!(summary.frontmatter.is_none());
        assert_eq!(summary.tasks.len(), 2);
    }

    #[test]
    fn test_frontmatter_malformed_yaml() {
        let content = "\
---
deps: [invalid: yaml: structure
  broken
---
- [ ] T01 [api] First";

        let summary = parse_progress(content);
        // Malformed YAML → no frontmatter, tasks still parsed from full content
        assert!(summary.frontmatter.is_none());
        assert_eq!(summary.tasks.len(), 1);
    }

    #[test]
    fn test_frontmatter_empty_deps() {
        let content = "\
---
deps:
  T01: []
  T02: []
---
- [ ] T01 [api] First
- [ ] T02 [api] Second";

        let summary = parse_progress(content);
        let fm = summary.frontmatter.as_ref().unwrap();
        assert_eq!(fm.deps.len(), 2);
        assert!(fm.deps["T01"].is_empty());
        assert!(fm.deps["T02"].is_empty());
    }

    #[test]
    fn test_write_frontmatter_roundtrip() {
        let mut fm = ProgressFrontmatter::default();
        fm.deps.insert("T02".to_string(), vec!["T01".to_string()]);
        fm.default_model = Some("claude-sonnet-4-5-20250929".to_string());

        let written = write_frontmatter(&fm);
        assert!(written.starts_with("---\n"));
        assert!(written.ends_with("---\n"));

        // Parse it back
        let (parsed, body) = parse_frontmatter(&written);
        let parsed = parsed.unwrap();
        assert_eq!(parsed.deps["T02"], vec!["T01"]);
        assert_eq!(parsed.default_model.as_deref(), Some("claude-sonnet-4-5-20250929"));
        assert!(body.trim().is_empty());
    }

    #[test]
    fn test_frontmatter_with_real_progress() {
        // Test with the actual PROGRESS.md format from this project
        let content = "\
---
deps:
  1.1.2: [1.1.1]
  1.1.3: [1.1.1]
  1.1.4: [1.1.2, 1.1.3]
models:
  3.2.1: claude-opus-4-6
default_model: claude-sonnet-4-5-20250929
---

# PROGRESS

## Epic 1: YAML Frontmatter Parser
- [ ] 1.1.1 [progress] Define struct
- [ ] 1.1.2 [progress] Parse frontmatter
- [ ] 1.1.3 [progress] Write frontmatter
- [ ] 1.1.4 [progress] Unit tests";

        let summary = parse_progress(content);
        assert_eq!(summary.tasks.len(), 4);
        let fm = summary.frontmatter.as_ref().unwrap();
        assert_eq!(fm.deps.len(), 3);
        assert_eq!(fm.deps["1.1.2"], vec!["1.1.1"]);
        assert_eq!(fm.deps["1.1.4"], vec!["1.1.2", "1.1.3"]);
        assert_eq!(fm.models["3.2.1"], "claude-opus-4-6");
        assert_eq!(fm.default_model.as_deref(), Some("claude-sonnet-4-5-20250929"));
    }

    #[test]
    fn test_frontmatter_no_closing_marker() {
        let content = "\
---
deps:
  T02: [T01]
- [ ] T01 [api] First";

        let summary = parse_progress(content);
        // No closing --- → no frontmatter
        assert!(summary.frontmatter.is_none());
    }
}
