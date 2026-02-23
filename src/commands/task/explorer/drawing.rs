//! Rendering TaskExplorerApp — rysowanie paneli i progress bar.
//!
//! Funkcje draw_*() są wywoływane z `AppState::draw()` impl.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, Padding, Paragraph, Wrap};

use crate::shared::progress::TaskStatus;
use crate::shared::tasks::TaskNode;
use crate::tui::responsive::Breakpoint;
use crate::tui::theme::DEFAULT_THEME;
use crate::tui::widgets::task_tree::TaskTreeWidget;

use super::state::{InputMode, Panel, TaskExplorerApp};

// ── Main draw ────────────────────────────────────────────────────────

/// Rysuj cały layout: Tree (lewy) + Detail (prawy) + Filter/Progress (dół).
/// Layout responsywny:
/// - Large/Medium (≥80 cols): Tree (40%) + Detail (60%) + bottom bar
/// - Small (<80 cols): Tree only + bottom bar
///
/// Bottom bar: Filter input (gdy InputMode::Filter) lub Progress bar (gdy Normal)
pub(crate) fn draw_all(frame: &mut Frame, area: Rect, app: &mut TaskExplorerApp) {
    // Wyczyść cache rectów z poprzedniego draw() przed renderowaniem nowej klatki
    app.task_row_rects.clear();

    let breakpoint = Breakpoint::detect(area.width);

    // Layout vertikalny: Content (Min) + Bottom bar (1 line)
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // main content
            Constraint::Length(1), // bottom bar (filter lub progress)
        ])
        .split(area);

    let content_area = vertical[0];
    let bottom_area = vertical[1];

    // Content area: zależnie od breakpoint
    match breakpoint {
        Breakpoint::Small => {
            // Small: tylko tree panel, detail ukryty
            draw_tree_panel(frame, content_area, app);
        }
        Breakpoint::Medium | Breakpoint::Large => {
            // Medium/Large: tree (40%) + detail (60%)
            let horizontal = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
                .split(content_area);

            let tree_area = horizontal[0];
            let detail_area = horizontal[1];

            draw_tree_panel(frame, tree_area, app);
            draw_detail_panel(frame, detail_area, app);
        }
    }

    // Bottom bar: filter input lub progress bar
    match app.input_mode {
        InputMode::Filter => draw_filter_input(frame, bottom_area, app),
        InputMode::Normal => draw_progress_bar(frame, bottom_area, app),
    }
}

// ── Tree panel ───────────────────────────────────────────────────────

/// Rysuj panel drzewa zadań (lewy).
fn draw_tree_panel(frame: &mut Frame, area: Rect, app: &mut TaskExplorerApp) {
    let theme = &DEFAULT_THEME;
    let is_focused = app.focus == Panel::Tree;

    let title = if app.filter.is_empty() {
        format!(" Tasks [sort: {}] ", app.sort_mode.label())
    } else {
        format!(
            " Tasks [sort: {}] [filter: {}] ",
            app.sort_mode.label(),
            app.filter
        )
    };
    let block = Block::default()
        .title(title)
        .padding(Padding::uniform(1))
        .style(Style::default().bg(theme.panel_bg(is_focused)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Render tree widget z posortowanymi i przefiltrowanymi wierszami.
    // with_hover() przekazuje indeks wiersza pod kursorem myszy — wizualny feedback.
    if inner.width > 0 && inner.height > 0 {
        let rows = app.visible_rows();
        let row_count = rows.len();
        let widget = TaskTreeWidget::from_rows(rows).with_hover(app.hovered_row);
        frame.render_stateful_widget(widget, inner, &mut app.tree_state);

        // Cache rect każdego widocznego wiersza drzewa (po render, gdy scroll_offset jest aktualny)
        let viewport_height = inner.height as usize;
        let scroll_offset = app.tree_state.scroll_offset;
        // Zarezerwuj pojemność z góry — eliminuje realokacje przy pierwszym renderze
        app.task_row_rects.reserve(viewport_height);
        for i in 0..viewport_height {
            let abs_index = scroll_offset + i;
            if abs_index >= row_count {
                break;
            }
            let row_rect = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            app.task_row_rects.push((abs_index, row_rect));
        }
    }
}

// ── Detail panel ─────────────────────────────────────────────────────

/// Rysuj panel szczegółów (prawy).
fn draw_detail_panel(frame: &mut Frame, area: Rect, app: &TaskExplorerApp) {
    let theme = &DEFAULT_THEME;
    let is_focused = app.focus == Panel::Detail;

    let block = Block::default()
        .title(" Detail ")
        .padding(Padding::uniform(1))
        .style(Style::default().bg(theme.panel_bg(is_focused)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let lines = match app.selected_node() {
        Some(node) => build_detail_lines(node),
        None => vec![Line::from(Span::styled(
            "No task selected",
            theme.muted_style(),
        ))],
    };

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll as u16, 0));
    frame.render_widget(paragraph, inner);
}

/// Zbuduj linie szczegółów dla wybranego node.
fn build_detail_lines(node: &TaskNode) -> Vec<Line<'static>> {
    let theme = &DEFAULT_THEME;
    let mut lines = Vec::new();

    // Nagłówek: ID + nazwa
    lines.push(Line::from(vec![
        Span::styled(
            format!("{} ", node.id),
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            node.name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]));

    // Status
    if let Some(ref status) = node.status {
        let (label, color) = match status {
            TaskStatus::Done => ("Done", theme.success),
            TaskStatus::InProgress => ("In Progress", theme.warning),
            TaskStatus::Todo => ("Todo", theme.muted),
            TaskStatus::Blocked => ("Blocked", theme.error),
        };
        lines.push(Line::from(vec![
            Span::styled("Status: ", theme.muted_style()),
            Span::styled(label.to_string(), Style::default().fg(color)),
        ]));
    }

    // Component
    if let Some(ref comp) = node.component {
        lines.push(Line::from(vec![
            Span::styled("Component: ", theme.muted_style()),
            Span::raw(comp.clone()),
        ]));
    }

    // Model
    if let Some(ref model) = node.model {
        lines.push(Line::from(vec![
            Span::styled("Model: ", theme.muted_style()),
            Span::raw(model.clone()),
        ]));
    }

    // Dependencies
    if !node.deps.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Deps: ", theme.muted_style()),
            Span::raw(node.deps.join(", ")),
        ]));
    }

    // Description
    if let Some(ref desc) = node.description {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Description:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(desc.clone()));
    }

    // Related files
    if !node.related_files.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Related files:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for file in &node.related_files {
            lines.push(Line::from(format!("  • {file}")));
        }
    }

    // Implementation steps
    if !node.implementation_steps.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Steps:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for (i, step) in node.implementation_steps.iter().enumerate() {
            lines.push(Line::from(format!("  {}. {step}", i + 1)));
        }
    }

    // Profiles
    if !node.profiles.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Profiles: ", theme.muted_style()),
            Span::raw(node.profiles.join(", ")),
        ]));
    }

    // Subtasks count (dla parent nodes)
    if !node.subtasks.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Subtasks: ", theme.muted_style()),
            Span::raw(format!("{}", node.subtasks.len())),
        ]));
    }

    lines
}

/// Policz linie detail panel dla danego node (do ograniczenia scroll).
pub(crate) fn detail_line_count(node: &TaskNode) -> usize {
    build_detail_lines(node).len()
}

// ── Filter input ─────────────────────────────────────────────────────

/// Rysuj pole tekstowe filtra na dole ekranu (tryb InputMode::Filter).
fn draw_filter_input(frame: &mut Frame, area: Rect, app: &TaskExplorerApp) {
    let theme = &DEFAULT_THEME;

    let prompt = "Filter: ";
    let text = format!("{}{}", prompt, app.filter);
    let cursor_pos = prompt.len() + app.filter.len();

    let paragraph = Paragraph::new(text).style(Style::default().fg(theme.primary));
    frame.render_widget(paragraph, area);

    // Ustaw pozycję kursora (migający kursor na końcu tekstu)
    if let Some(x) = area.x.checked_add(cursor_pos as u16)
        && x < area.x + area.width
    {
        frame.set_cursor_position((x, area.y));
    }
}

// ── Progress bar ─────────────────────────────────────────────────────

/// Rysuj progress bar na dole ekranu (1 linia: gauge z done/total i procentem).
fn draw_progress_bar(frame: &mut Frame, area: Rect, app: &TaskExplorerApp) {
    let theme = &DEFAULT_THEME;
    let (done, total) = app.progress_counts();

    let ratio = if total > 0 {
        done as f64 / total as f64
    } else {
        0.0
    };

    let label = format!("{done}/{total} done ({:.0}%)", ratio * 100.0);
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(theme.success))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(label);
    frame.render_widget(gauge, area);
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::state::{InputMode, Panel, SortMode, TaskExplorerApp};
    use super::*;
    use crate::shared::progress::TaskStatus;
    use crate::shared::tasks::{TaskNode, TasksFile};
    use crate::tui::widgets::task_tree::{self, TreeState};
    use ratatui::{Terminal, backend::TestBackend};
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Helper: utwórz app z SAMPLE_YAML (root nodes expanded).
    fn make_sample_app() -> TaskExplorerApp {
        let tasks: TasksFile = serde_yaml::from_str(SAMPLE_YAML).unwrap();
        let mut expanded = HashSet::new();
        for node in &tasks.tasks {
            expanded.insert(node.id.clone());
        }
        let tree_state = TreeState {
            expanded,
            selected: 0,
            scroll_offset: 0,
        };
        let rows = task_tree::flatten_nodes(&tasks.tasks, &tree_state.expanded);
        let selected_id = rows.first().map(|r| r.id.clone());

        TaskExplorerApp {
            tasks,
            tasks_path: PathBuf::from("test.yml"),
            tree_state,
            selected_id,
            focus: Panel::Tree,
            input_mode: InputMode::Normal,
            filter: String::new(),
            sort_mode: SortMode::Id,
            detail_scroll: 0,
            scroll_step: 3,
            task_row_rects: Vec::new(),
            hovered_row: None,
        }
    }

    #[test]
    fn build_detail_lines_includes_all_fields() {
        let node = TaskNode {
            id: "1.1".to_string(),
            name: "Test task".to_string(),
            component: Some("parser".to_string()),
            status: Some(TaskStatus::InProgress),
            deps: vec!["1.0".to_string()],
            model: Some("claude-opus-4-6".to_string()),
            description: Some("A test description".to_string()),
            related_files: vec!["src/test.rs".to_string()],
            implementation_steps: vec!["Step one".to_string(), "Step two".to_string()],
            acceptance_criteria: Vec::new(),
            profiles: vec!["backend".to_string()],
            subtasks: vec![],
        };

        let lines = build_detail_lines(&node);
        let text: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("1.1"));
        assert!(text.contains("Test task"));
        assert!(text.contains("In Progress"));
        assert!(text.contains("parser"));
        assert!(text.contains("claude-opus-4-6"));
        assert!(text.contains("1.0"));
        assert!(text.contains("A test description"));
        assert!(text.contains("src/test.rs"));
        assert!(text.contains("Step one"));
        assert!(text.contains("Step two"));
        assert!(text.contains("backend"));
    }

    #[test]
    fn build_detail_lines_parent_shows_subtask_count() {
        let node = TaskNode {
            id: "1".to_string(),
            name: "Parent".to_string(),
            component: Some("core".to_string()),
            status: None,
            deps: vec![],
            model: None,
            description: None,
            related_files: vec![],
            implementation_steps: vec![],
            acceptance_criteria: Vec::new(),
            profiles: vec![],
            subtasks: vec![
                TaskNode {
                    id: "1.1".to_string(),
                    name: "Child A".to_string(),
                    component: None,
                    status: Some(TaskStatus::Todo),
                    deps: vec![],
                    model: None,
                    description: None,
                    related_files: vec![],
                    implementation_steps: vec![],
                    acceptance_criteria: Vec::new(),
                    profiles: vec![],
                    subtasks: vec![],
                },
                TaskNode {
                    id: "1.2".to_string(),
                    name: "Child B".to_string(),
                    component: None,
                    status: Some(TaskStatus::Done),
                    deps: vec![],
                    model: None,
                    description: None,
                    related_files: vec![],
                    implementation_steps: vec![],
                    acceptance_criteria: Vec::new(),
                    profiles: vec![],
                    subtasks: vec![],
                },
            ],
        };

        let lines = build_detail_lines(&node);
        let text: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Subtasks: 2"));
    }

    #[test]
    fn detail_line_count_matches_build() {
        let node = TaskNode {
            id: "1".to_string(),
            name: "Test".to_string(),
            component: Some("core".to_string()),
            status: Some(TaskStatus::Todo),
            deps: vec!["0.1".to_string()],
            model: None,
            description: Some("A description".to_string()),
            related_files: vec!["a.rs".to_string(), "b.rs".to_string()],
            implementation_steps: vec!["Step 1".to_string()],
            acceptance_criteria: Vec::new(),
            profiles: vec![],
            subtasks: vec![],
        };

        let lines = build_detail_lines(&node);
        assert_eq!(detail_line_count(&node), lines.len());
    }

    // ── Render smoke tests ──

    #[test]
    fn draw_does_not_panic() {
        let mut app = make_sample_app();

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_all(frame, area, &mut app);
            })
            .unwrap();
    }

    #[test]
    fn draw_with_empty_tasks_does_not_panic() {
        let mut app = TaskExplorerApp {
            tasks: TasksFile {
                default_model: None,
                tasks: vec![],
            },
            tasks_path: PathBuf::from("test.yml"),
            tree_state: TreeState::default(),
            selected_id: None,
            focus: Panel::Tree,
            input_mode: InputMode::Normal,
            filter: String::new(),
            sort_mode: SortMode::Id,
            detail_scroll: 0,
            scroll_step: 3,
            task_row_rects: Vec::new(),
            hovered_row: None,
        };

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_all(frame, area, &mut app);
            })
            .unwrap();
    }

    #[test]
    fn draw_narrow_terminal_does_not_panic() {
        let mut app = make_sample_app();

        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_all(frame, area, &mut app);
            })
            .unwrap();
    }

    const SAMPLE_YAML: &str = r#"
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
        deps: ["1.1"]
  - id: "2"
    name: "Epic Two"
    component: dag
    subtasks:
      - id: "2.1"
        name: "Cycle detect"
        status: todo
        component: dag
      - id: "2.2"
        name: "Topo sort"
        status: blocked
        component: dag
        deps: ["2.1", "1.2"]
"#;

    // ── Cache tests ──

    #[test]
    fn task_row_rects_populated_after_draw() {
        let mut app = make_sample_app();

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_all(frame, area, &mut app);
            })
            .unwrap();

        // Cache powinien zawierać recty dla widocznych wierszy
        assert!(!app.task_row_rects.is_empty());

        // Każdy rect ma wysokość 1 (jeden wiersz drzewa)
        for (_, rect) in &app.task_row_rects {
            assert_eq!(rect.height, 1);
        }

        // Indeksy są sekwencyjne (scroll_offset=0, więc abs_index = i)
        for (i, (abs_idx, _)) in app.task_row_rects.iter().enumerate() {
            assert_eq!(*abs_idx, i);
        }
    }

    #[test]
    fn task_row_rects_cleared_between_draws() {
        let mut app = make_sample_app();

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        // Pierwszy draw
        terminal
            .draw(|frame| draw_all(frame, frame.area(), &mut app))
            .unwrap();
        let count_after_first = app.task_row_rects.len();
        assert!(count_after_first > 0);

        // Drugi draw — cache powinien zostać odświeżony (nie zduplikowany)
        terminal
            .draw(|frame| draw_all(frame, frame.area(), &mut app))
            .unwrap();
        let count_after_second = app.task_row_rects.len();
        assert_eq!(count_after_first, count_after_second);
    }

    #[test]
    fn task_row_rects_empty_when_no_tasks() {
        let mut app = TaskExplorerApp {
            tasks: TasksFile {
                default_model: None,
                tasks: vec![],
            },
            tasks_path: PathBuf::from("test.yml"),
            tree_state: TreeState::default(),
            selected_id: None,
            focus: Panel::Tree,
            input_mode: InputMode::Normal,
            filter: String::new(),
            sort_mode: SortMode::Id,
            detail_scroll: 0,
            scroll_step: 3,
            task_row_rects: Vec::new(),
            hovered_row: None,
        };

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_all(frame, frame.area(), &mut app))
            .unwrap();

        // Brak tasków → brak rectów w cache
        assert!(app.task_row_rects.is_empty());
    }

    // ── Snapshot tests: Responsive layout ──

    /// Helper: renderuj app do bufora i zwróć snapshot string.
    fn render_snapshot(app: &mut TaskExplorerApp, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_all(frame, area, app);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();

        // Konwertuj buffer na snapshot string (każda linia z newline)
        let mut lines = Vec::new();
        for y in 0..buffer.area.height {
            let mut line = String::new();
            for x in 0..buffer.area.width {
                let cell = buffer.cell((x, y)).unwrap();
                line.push_str(cell.symbol());
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }

    #[test]
    fn snapshot_layout_large_breakpoint() {
        let mut app = make_sample_app();
        let snapshot = render_snapshot(&mut app, 120, 30);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_layout_medium_breakpoint() {
        let mut app = make_sample_app();
        let snapshot = render_snapshot(&mut app, 100, 25);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_layout_small_breakpoint() {
        let mut app = make_sample_app();
        let snapshot = render_snapshot(&mut app, 70, 20);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_layout_small_with_selection() {
        let mut app = make_sample_app();
        app.tree_state.selected = 2; // Select "1.2"
        app.selected_id = Some("1.2".to_string());

        let snapshot = render_snapshot(&mut app, 70, 20);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_progress_bar_multiple_done() {
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929
tasks:
  - id: "1"
    name: "Epic"
    component: test
    subtasks:
      - id: "1.1"
        name: "Task A"
        status: done
        component: test
      - id: "1.2"
        name: "Task B"
        status: done
        component: test
      - id: "1.3"
        name: "Task C"
        status: in_progress
        component: test
      - id: "1.4"
        name: "Task D"
        status: todo
        component: test
"#;
        let tasks: TasksFile = serde_yaml::from_str(yaml).unwrap();
        let mut expanded = HashSet::new();
        expanded.insert("1".to_string());
        let tree_state = TreeState {
            expanded,
            selected: 0,
            scroll_offset: 0,
        };
        let rows = task_tree::flatten_nodes(&tasks.tasks, &tree_state.expanded);
        let selected_id = rows.first().map(|r| r.id.clone());

        let mut app = TaskExplorerApp {
            tasks,
            tasks_path: PathBuf::from("test.yml"),
            tree_state,
            selected_id,
            focus: Panel::Tree,
            input_mode: InputMode::Normal,
            filter: String::new(),
            sort_mode: SortMode::Id,
            detail_scroll: 0,
            scroll_step: 3,
            task_row_rects: Vec::new(),
            hovered_row: None,
        };

        // Progress: 2/4 done (50%)
        let snapshot = render_snapshot(&mut app, 80, 15);
        insta::assert_snapshot!(snapshot);
    }

    // ── Snapshot tests: Filtered tree ──

    #[test]
    fn snapshot_tree_filtered_by_name() {
        let mut app = make_sample_app();
        app.filter = "Cycle".to_string(); // Matches "2.1 Cycle detect"

        let snapshot = render_snapshot(&mut app, 80, 20);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_tree_filtered_by_component() {
        let mut app = make_sample_app();
        app.filter = "dag".to_string(); // Matches component "dag"

        let snapshot = render_snapshot(&mut app, 80, 20);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_tree_filtered_by_status() {
        let mut app = make_sample_app();
        app.filter = "progress".to_string(); // Matches status "in_progress"

        let snapshot = render_snapshot(&mut app, 80, 20);
        insta::assert_snapshot!(snapshot);
    }

    // ── Snapshot tests: Sorted tree ──

    #[test]
    fn snapshot_tree_sorted_by_status() {
        let mut app = make_sample_app();
        app.sort_mode = SortMode::Status;

        let snapshot = render_snapshot(&mut app, 100, 25);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn snapshot_tree_sorted_by_component() {
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929
tasks:
  - id: "1"
    name: "Zebra epic"
    component: zebra
    subtasks:
      - id: "1.1"
        name: "Z-child"
        status: todo
        component: zebra
  - id: "2"
    name: "Beta epic"
    component: beta
    subtasks:
      - id: "2.1"
        name: "B-child"
        status: done
        component: beta
  - id: "3"
    name: "Alpha epic"
    component: alpha
    subtasks:
      - id: "3.1"
        name: "A-child"
        status: in_progress
        component: alpha
"#;
        let tasks: TasksFile = serde_yaml::from_str(yaml).unwrap();
        let mut expanded = HashSet::new();
        for node in &tasks.tasks {
            expanded.insert(node.id.clone());
        }
        let tree_state = TreeState {
            expanded,
            selected: 0,
            scroll_offset: 0,
        };

        let mut app = TaskExplorerApp {
            tasks,
            tasks_path: PathBuf::from("test.yml"),
            tree_state,
            selected_id: Some("1".to_string()),
            focus: Panel::Tree,
            input_mode: InputMode::Normal,
            filter: String::new(),
            sort_mode: SortMode::Component,
            detail_scroll: 0,
            scroll_step: 3,
            task_row_rects: Vec::new(),
            hovered_row: None,
        };

        // Powinno być: alpha (3) → beta (2) → zebra (1)
        let snapshot = render_snapshot(&mut app, 100, 25);
        insta::assert_snapshot!(snapshot);
    }
}
