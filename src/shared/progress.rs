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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
        assert_eq!(
            fm.default_model.as_deref(),
            Some("claude-sonnet-4-5-20250929")
        );
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
        assert_eq!(
            fm.default_model.as_deref(),
            Some("claude-sonnet-4-5-20250929")
        );
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

    #[test]
    fn test_task_line_with_component_but_no_name() {
        // Component jest, ale name jest pusty — parse_task_line powinno zwrócić None
        let line1 = "- [ ] 1.1 [api]";
        assert!(
            parse_task_line(line1).is_none(),
            "Linia z pustym name powinna zwrócić None"
        );

        // Test także z trailing spaces
        let line2 = "- [ ] 1.1 [api]  ";
        assert!(
            parse_task_line(line2).is_none(),
            "Linia z pustym name i trailing spaces powinna zwrócić None"
        );

        // Test z samym componentem i spacją
        let line3 = "- [ ] 1.1 [api] ";
        assert!(
            parse_task_line(line3).is_none(),
            "Linia z samą spacją po component powinna zwrócić None"
        );

        // Test z różnymi statusami
        let line4 = "- [x] 2.1 [ui]";
        assert!(
            parse_task_line(line4).is_none(),
            "Done task z pustym name powinna zwrócić None"
        );

        let line5 = "- [~] 3.1 [infra]";
        assert!(
            parse_task_line(line5).is_none(),
            "In-progress task z pustym name powinna zwrócić None"
        );

        let line6 = "- [!] 4.1 [backend]";
        assert!(
            parse_task_line(line6).is_none(),
            "Blocked task z pustym name powinna zwrócić None"
        );
    }

    #[test]
    fn test_empty_progress_graceful_handling() {
        // Rozszerzenie test_empty_content — dodaje weryfikację frontmatter i tasks.is_empty()
        let summary = parse_progress("");
        assert!(
            summary.tasks.is_empty(),
            "Pusty string powinien dać pustą listę tasków"
        );
        assert_eq!(summary.total(), 0);
        assert_eq!(summary.remaining(), 0);
        assert!(summary.frontmatter.is_none());
    }

    #[test]
    fn test_only_empty_frontmatter() {
        // Test 2: Tylko pusty frontmatter bez tasków
        // Format: ---\n---
        let content = "---\n---";
        let summary = parse_progress(content);
        assert!(
            summary.tasks.is_empty(),
            "Tylko frontmatter powinien dać pustą listę tasków"
        );
        assert_eq!(summary.total(), 0);
        assert_eq!(summary.remaining(), 0);
        assert!(
            summary.frontmatter.is_some(),
            "Pusty frontmatter powinien być sparsowany"
        );
        let fm = summary.frontmatter.unwrap();
        assert!(fm.deps.is_empty());
        assert!(fm.models.is_empty());
        assert!(fm.default_model.is_none());
    }

    #[test]
    fn test_empty_frontmatter_with_markdown_header() {
        // Test 3: Pusty frontmatter z nagłówkiem markdown, ale bez tasków
        let content = "\
---
---

# My Progress

Some random text here.";
        let summary = parse_progress(content);
        assert!(
            summary.tasks.is_empty(),
            "Frontmatter + markdown bez tasków powinien dać pustą listę"
        );
        assert_eq!(summary.total(), 0);
        assert_eq!(summary.remaining(), 0);
        assert!(
            summary.frontmatter.is_some(),
            "Frontmatter powinien być sparsowany"
        );
    }

    #[test]
    fn test_empty_progress_no_panic() {
        // Test 4: Upewniamy się że nie ma paniki dla różnych wariantów pustych danych
        let test_cases = vec![
            "",
            "---\n---",
            "\n\n\n",
            "---\n---\n\n\n",
            "# Only headers\n## And subheaders",
            "---\n---\n\nSome text\nMore text",
        ];

        for content in test_cases {
            // To nie powinna panic
            let summary = parse_progress(content);
            // Countery powinny być spójne z wektorem tasks
            assert_eq!(
                summary.tasks.len(),
                summary.total(),
                "tasks.len() != total() dla: {:?}",
                content
            );
        }
    }

    /// Test: Task ID z myślnikami w nazwie (np. "1.1-beta")
    /// Sprawdza że parser poprawnie wyodrębnia ID ze znakami specjalnymi
    #[test]
    fn test_parse_task_id_with_hyphens() {
        let content = "\
- [ ] 1.1-beta [api] Add beta feature
- [x] 2.0-rc1 [ui] Release candidate";

        let summary = parse_progress(content);
        assert_eq!(summary.tasks.len(), 2);
        assert_eq!(summary.tasks[0].id, "1.1-beta");
        assert_eq!(summary.tasks[0].component, "api");
        assert_eq!(summary.tasks[0].name, "Add beta feature");
        assert_eq!(summary.tasks[0].status, TaskStatus::Todo);

        assert_eq!(summary.tasks[1].id, "2.0-rc1");
        assert_eq!(summary.tasks[1].component, "ui");
        assert_eq!(summary.tasks[1].name, "Release candidate");
        assert_eq!(summary.tasks[1].status, TaskStatus::Done);
    }

    /// Test: Task ID z podkreśleniami (np. "T01_draft", "T01_v2")
    /// Sprawdza że parser poprawnie rozpoznaje ID z underscores
    #[test]
    fn test_parse_task_id_with_underscores() {
        let content = "\
- [~] T01_draft [api] Draft implementation
- [ ] T01_v2 [api] Version 2
- [!] TASK_001_FINAL [infra] Final task";

        let summary = parse_progress(content);
        assert_eq!(summary.tasks.len(), 3);

        assert_eq!(summary.tasks[0].id, "T01_draft");
        assert_eq!(summary.tasks[0].status, TaskStatus::InProgress);

        assert_eq!(summary.tasks[1].id, "T01_v2");
        assert_eq!(summary.tasks[1].status, TaskStatus::Todo);

        assert_eq!(summary.tasks[2].id, "TASK_001_FINAL");
        assert_eq!(summary.tasks[2].status, TaskStatus::Blocked);
    }

    /// Test: Mieszane ID z literami, cyframi, myślnikami i podkreśleniami
    /// Sprawdza obsługę różnorodnych formatów ID
    #[test]
    fn test_parse_task_id_mixed_special_chars() {
        let content = "\
- [ ] alpha-1_beta [api] Mixed format
- [x] T2_1-RC [ui] Complex ID
- [~] v1.2.3-beta_1 [infra] Semantic versioning";

        let summary = parse_progress(content);
        assert_eq!(summary.tasks.len(), 3);

        assert_eq!(summary.tasks[0].id, "alpha-1_beta");
        assert_eq!(summary.tasks[1].id, "T2_1-RC");
        assert_eq!(summary.tasks[2].id, "v1.2.3-beta_1");
    }

    #[test]
    fn test_mixed_indentation_task_lines() {
        // Linie z różnymi wcięciami: 2-space, 4-space, tab, brak wcięcia.
        // Parser używa trim(), więc wszystkie powinny być sparsowane.
        let content = "\
# Epic 1
- [ ] 1.1 [api] No indent
  - [x] 1.1.1 [api] Two spaces
    - [~] 1.1.1.1 [api] Four spaces
\t- [!] 1.1.1.1.1 [api] Tab indent
      - [ ] 1.1.1.1.1.1 [api] Six spaces
\t\t- [x] 1.1.1.1.1.1.1 [api] Two tabs";

        let summary = parse_progress(content);

        // Wszystkie 6 linii powinno być sparsowane
        assert_eq!(
            summary.tasks.len(),
            6,
            "Wszystkie linie powinny być sparsowane niezależnie od wcięć"
        );

        // Sprawdź hierarchię ID (nie jest enforced przez parser, ale ID powinny być poprawne)
        assert_eq!(summary.tasks[0].id, "1.1");
        assert_eq!(summary.tasks[1].id, "1.1.1");
        assert_eq!(summary.tasks[2].id, "1.1.1.1");
        assert_eq!(summary.tasks[3].id, "1.1.1.1.1");
        assert_eq!(summary.tasks[4].id, "1.1.1.1.1.1");
        assert_eq!(summary.tasks[5].id, "1.1.1.1.1.1.1");

        // Sprawdź statusy
        assert_eq!(summary.tasks[0].status, TaskStatus::Todo);
        assert_eq!(summary.tasks[1].status, TaskStatus::Done);
        assert_eq!(summary.tasks[2].status, TaskStatus::InProgress);
        assert_eq!(summary.tasks[3].status, TaskStatus::Blocked);
        assert_eq!(summary.tasks[4].status, TaskStatus::Todo);
        assert_eq!(summary.tasks[5].status, TaskStatus::Done);

        // Sprawdź komponenty i nazwy
        assert_eq!(summary.tasks[0].component, "api");
        assert_eq!(summary.tasks[0].name, "No indent");
        assert_eq!(summary.tasks[1].name, "Two spaces");
        assert_eq!(summary.tasks[2].name, "Four spaces");
        assert_eq!(summary.tasks[3].name, "Tab indent");
        assert_eq!(summary.tasks[4].name, "Six spaces");
        assert_eq!(summary.tasks[5].name, "Two tabs");
    }

    #[test]
    fn test_mixed_indentation_with_frontmatter_deps() {
        // Frontmatter YAML z różnymi wcięciami dla deps + linie zadań z mieszanymi wcięciami
        let content = "\
---
deps:
  1.1.1: [1.1]
  1.1.1.1: [1.1.1]
  1.1.1.1.1: [1.1.1.1]
---
- [ ] 1.1 [api] Root task
  - [ ] 1.1.1 [api] Child with 2-space indent
\t- [ ] 1.1.1.1 [api] Grandchild with tab
    - [ ] 1.1.1.1.1 [api] Great-grandchild with 4-space";

        let summary = parse_progress(content);

        // Sprawdź że wszystkie zadania zostały sparsowane
        assert_eq!(summary.tasks.len(), 4);

        // Sprawdź frontmatter
        let fm = summary
            .frontmatter
            .as_ref()
            .expect("Frontmatter powinien być sparsowany");
        assert_eq!(fm.deps.len(), 3);
        assert_eq!(fm.deps["1.1.1"], vec!["1.1"]);
        assert_eq!(fm.deps["1.1.1.1"], vec!["1.1.1"]);
        assert_eq!(fm.deps["1.1.1.1.1"], vec!["1.1.1.1"]);

        // Sprawdź że deps frontmatter są niezależne od wcięć linii zadań
        assert_eq!(summary.tasks[0].id, "1.1");
        assert_eq!(summary.tasks[1].id, "1.1.1");
        assert_eq!(summary.tasks[2].id, "1.1.1.1");
        assert_eq!(summary.tasks[3].id, "1.1.1.1.1");
    }

    #[test]
    fn test_yaml_frontmatter_with_mixed_indentation() {
        // YAML z różnymi wcięciami dla deps (2-space, 4-space)
        // YAML standard wymaga konsystentnych wcięć w obrębie jednego poziomu,
        // ale różne poziomy mogą używać różnych wielokrotności
        let content = "\
---
deps:
  1.2: [1.1]
  1.3:
    - 1.1
    - 1.2
models:
  1.1: claude-opus-4-6
default_model: claude-sonnet-4-5-20250929
---
- [ ] 1.1 [api] First
- [ ] 1.2 [api] Second
- [ ] 1.3 [api] Third";

        let summary = parse_progress(content);

        // Frontmatter powinien być sparsowany poprawnie
        let fm = summary
            .frontmatter
            .as_ref()
            .expect("YAML powinien być poprawny");
        assert_eq!(fm.deps.len(), 2);
        assert_eq!(fm.deps["1.2"], vec!["1.1"]);
        assert_eq!(fm.deps["1.3"], vec!["1.1", "1.2"]);
        assert_eq!(fm.models["1.1"], "claude-opus-4-6");
        assert_eq!(
            fm.default_model.as_deref(),
            Some("claude-sonnet-4-5-20250929")
        );

        // Zadania powinny być sparsowane
        assert_eq!(summary.tasks.len(), 3);
    }
}
