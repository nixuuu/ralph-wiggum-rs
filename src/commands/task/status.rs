use std::path::Path;
use std::time::Duration;

use crate::commands::task::explorer::TaskExplorerApp;
use crate::shared::error::Result;
use crate::shared::file_config::FileConfig;
use crate::shared::tasks::TasksFile;
use crate::tui::app::App;

pub fn execute(file_config: &FileConfig) -> Result<()> {
    let tasks_path = &file_config.task.tasks_file;

    // Auto-initialize if file doesn't exist
    let tasks_file = TasksFile::load_or_init(tasks_path)?;

    if tasks_file.tasks.is_empty() {
        return print_empty_state();
    }

    launch_explorer(tasks_path, file_config.tui.scroll_step)
}

/// Wyświetl komunikat gdy brak tasków.
fn print_empty_state() -> Result<()> {
    println!();
    println!("{}", "━".repeat(60));
    println!("ℹ No tasks yet.");
    println!("  Run 'task add' or 'task plan' to get started.");
    println!("{}", "━".repeat(60));
    println!();
    Ok(())
}

/// Uruchom fullscreen TUI explorer.
fn launch_explorer(tasks_path: &Path, scroll_step: u16) -> Result<()> {
    let mut explorer = TaskExplorerApp::load(tasks_path)?.with_scroll_step(scroll_step);
    let mut tui_app = App::new(Duration::from_millis(100))?;
    tui_app.run(&mut explorer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::file_config::TaskConfig;
    use tempfile::TempDir;

    fn make_config(temp_dir: &TempDir, tasks_content: &str) -> FileConfig {
        let tasks_file = temp_dir.path().join("tasks.yml");
        std::fs::write(&tasks_file, tasks_content).unwrap();
        FileConfig {
            task: TaskConfig {
                tasks_file,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_execute_empty_tasks_prints_message() {
        let temp_dir = TempDir::new().unwrap();
        let config = make_config(&temp_dir, "tasks: []");
        // Puste taski → print_empty_state, nie uruchamia TUI
        assert!(execute(&config).is_ok());
    }

    #[test]
    fn test_execute_auto_inits_missing_file() {
        let temp_dir = TempDir::new().unwrap();
        let tasks_path = temp_dir.path().join("nonexistent.yml");
        let config = FileConfig {
            task: TaskConfig {
                tasks_file: tasks_path.clone(),
                ..Default::default()
            },
            ..Default::default()
        };
        // Plik nie istnieje → auto-init → puste taski → empty state
        assert!(execute(&config).is_ok());
        assert!(tasks_path.exists(), "Auto-init powinien stworzyć plik");
    }

    #[test]
    fn test_explorer_loads_single_task() {
        let temp_dir = TempDir::new().unwrap();
        let tasks_file = temp_dir.path().join("tasks.yml");
        std::fs::write(
            &tasks_file,
            "tasks:\n  - id: '1'\n    name: Test\n    component: core\n    status: todo\n",
        )
        .unwrap();

        let explorer = TaskExplorerApp::load(&tasks_file).unwrap();
        assert_eq!(explorer.tasks.tasks.len(), 1);
    }

    #[test]
    fn test_explorer_loads_multiple_tasks_with_statuses() {
        let temp_dir = TempDir::new().unwrap();
        let tasks_file = temp_dir.path().join("tasks.yml");
        let yaml = r#"tasks:
  - id: "1"
    name: "Done task"
    component: "core"
    status: done
  - id: "2"
    name: "WIP task"
    component: "ui"
    status: in_progress
  - id: "3"
    name: "Todo task"
    component: "api"
    status: todo
"#;
        std::fs::write(&tasks_file, yaml).unwrap();

        let explorer = TaskExplorerApp::load(&tasks_file).unwrap();
        assert_eq!(explorer.tasks.tasks.len(), 3);
        // Pierwszy task powinien być zaznaczony domyślnie
        assert_eq!(explorer.selected_id.as_deref(), Some("1"));
    }

    #[test]
    fn test_print_empty_state() {
        assert!(print_empty_state().is_ok());
    }

    // ── Integration tests: Task Explorer Navigation ──────────────────────

    use crate::tui::test_helpers::{TestApp, make_key};
    use crossterm::event::KeyCode;

    fn sample_explorer_yaml() -> &'static str {
        r#"
default_model: claude-sonnet-4-5-20250929

tasks:
  - id: "1"
    name: "Epic One"
    component: parser
    subtasks:
      - id: "1.1"
        name: "Subtask A"
        status: done
        component: parser
      - id: "1.2"
        name: "Subtask B"
        status: in_progress
        component: parser
  - id: "2"
    name: "Epic Two"
    component: api
    subtasks:
      - id: "2.1"
        name: "API feature"
        status: todo
        component: api
      - id: "2.2"
        name: "API tests"
        status: todo
        component: api
"#
    }

    fn make_test_explorer() -> TaskExplorerApp {
        use crate::shared::tasks::TasksFile;
        use crate::tui::widgets::task_tree::TreeState;
        use std::collections::HashSet;
        use std::path::PathBuf;

        let tasks: TasksFile = serde_yaml::from_str(sample_explorer_yaml()).unwrap();

        // Rozwiń root nodes (epiki)
        let mut expanded = HashSet::new();
        for node in &tasks.tasks {
            expanded.insert(node.id.clone());
        }

        let tree_state = TreeState {
            expanded,
            selected: 0,
            scroll_offset: 0,
        };

        let rows =
            crate::tui::widgets::task_tree::flatten_nodes(&tasks.tasks, &tree_state.expanded);
        let selected_id = rows.first().map(|r| r.id.clone());

        TaskExplorerApp {
            tasks,
            tasks_path: PathBuf::from(".ralph/tasks.yml"),
            tree_state,
            selected_id,
            focus: crate::commands::task::explorer::state::Panel::Tree,
            input_mode: crate::commands::task::explorer::state::InputMode::Normal,
            filter: String::new(),
            sort_mode: crate::commands::task::explorer::state::SortMode::Id,
            detail_scroll: 0,
            scroll_step: 3,
            task_row_rects: Vec::new(),
        }
    }

    #[test]
    fn test_navigation_down_3_times_selects_4th_item() {
        let explorer = make_test_explorer();
        let mut app = TestApp::new(explorer, 80, 24);

        // Stan początkowy: zaznaczony "1" (Epic One)
        app.assert_state(|s| s.selected_id.as_deref() == Some("1"));

        // ↓ 1 raz → "1.1" (Subtask A)
        app.inject_key(make_key(KeyCode::Down));
        app.step();
        app.assert_state(|s| s.selected_id.as_deref() == Some("1.1"));

        // ↓ 2 raz → "1.2" (Subtask B)
        app.inject_key(make_key(KeyCode::Down));
        app.step();
        app.assert_state(|s| s.selected_id.as_deref() == Some("1.2"));

        // ↓ 3 raz → "2" (Epic Two) — 4ty element
        app.inject_key(make_key(KeyCode::Down));
        app.step();
        app.assert_state(|s| s.selected_id.as_deref() == Some("2"));
    }

    #[test]
    fn test_enter_on_parent_toggles_expand_collapse() {
        let mut explorer = make_test_explorer();
        // Zacznij ze zwiniętym drzewem
        explorer.tree_state.expanded.clear();

        let mut app = TestApp::new(explorer, 80, 24);

        // Stan początkowy: zaznaczony "1", brak rozszerzonych children
        app.assert_state(|s| s.tree_state.expanded.is_empty());
        app.assert_state(|s| s.selected_id.as_deref() == Some("1"));

        // Enter → expand "1"
        app.inject_key(make_key(KeyCode::Enter));
        app.step();
        app.assert_state(|s| s.tree_state.expanded.contains("1"));

        // Widoczne children: "1.1" i "1.2"
        app.assert_state(|s| {
            let rows = s.visible_rows();
            rows.len() > 2 && rows.iter().any(|r| r.id == "1.1")
        });

        // Enter ponownie → collapse "1"
        app.inject_key(make_key(KeyCode::Enter));
        app.step();
        app.assert_state(|s| !s.tree_state.expanded.contains("1"));

        // Children schowane
        app.assert_state(|s| {
            let rows = s.visible_rows();
            rows.iter().all(|r| r.id != "1.1")
        });
    }

    #[test]
    fn test_filter_by_api_component() {
        let mut explorer = make_test_explorer();
        // Ustaw filtr na "api"
        explorer.filter = "api".to_string();

        let app = TestApp::new(explorer, 80, 24);

        // Widoczne tylko taski z komponentem "api" (node "2" też ma component=api)
        app.assert_state(|s| {
            let rows = s.visible_rows();
            !rows.is_empty() && rows.iter().all(|r| r.component.as_deref() == Some("api"))
        });

        // "1" i "1.1", "1.2" (parser) powinny być odfiltrowane
        app.assert_state(|s| {
            let rows = s.visible_rows();
            !rows.iter().any(|r| r.id == "1" || r.id == "1.1")
        });
    }

    #[test]
    fn test_sort_by_component_2_times() {
        let explorer = make_test_explorer();
        let mut app = TestApp::new(explorer, 80, 24);

        // Stan początkowy: SortMode::Id
        app.assert_state(|s| {
            matches!(
                s.sort_mode,
                crate::commands::task::explorer::state::SortMode::Id
            )
        });

        // 's' 1 raz → SortMode::Status
        app.inject_key(make_key(KeyCode::Char('s')));
        app.step();
        app.assert_state(|s| {
            matches!(
                s.sort_mode,
                crate::commands::task::explorer::state::SortMode::Status
            )
        });

        // 's' 2 raz → SortMode::Component
        app.inject_key(make_key(KeyCode::Char('s')));
        app.step();
        app.assert_state(|s| {
            matches!(
                s.sort_mode,
                crate::commands::task::explorer::state::SortMode::Component
            )
        });

        // Sprawdź czy sortowanie działa: "api" przed "parser" (alfabetycznie)
        app.assert_state(|s| {
            let rows = s.visible_rows();
            let depth0: Vec<_> = rows.iter().filter(|r| r.depth == 0).collect();
            if depth0.len() >= 2 {
                // Epic Two (api) powinien być przed Epic One (parser)
                depth0[0].component.as_deref() == Some("api")
                    && depth0[1].component.as_deref() == Some("parser")
            } else {
                false
            }
        });
    }

    #[test]
    fn test_tab_switches_focus_to_detail_panel() {
        let explorer = make_test_explorer();
        let mut app = TestApp::new(explorer, 80, 24);

        // Stan początkowy: focus na Tree
        app.assert_state(|s| s.focus == crate::commands::task::explorer::state::Panel::Tree);

        // Tab → switch to Detail
        app.inject_key(make_key(KeyCode::Tab));
        app.step();
        app.assert_state(|s| s.focus == crate::commands::task::explorer::state::Panel::Detail);

        // Tab ponownie → switch back to Tree
        app.inject_key(make_key(KeyCode::Tab));
        app.step();
        app.assert_state(|s| s.focus == crate::commands::task::explorer::state::Panel::Tree);
    }
}
