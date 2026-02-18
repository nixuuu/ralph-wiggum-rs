use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Wrap};

use crate::shared::progress::TaskStatus;
use crate::shared::tasks::{TaskNode, TasksFile};
use crate::tui::theme::DEFAULT_THEME;

/// Render task preview overlay as fullscreen panel showing all tasks.
///
/// Displays a hierarchical tree of tasks with status icons, components,
/// and dependencies. Supports scrolling when the task list exceeds the
/// visible area.
pub fn render_task_preview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    tasks_file: &TasksFile,
    scroll_offset: usize,
) {
    // Fill entire area (including title row) with background before any widget renders
    let theme = &DEFAULT_THEME;
    frame
        .buffer_mut()
        .set_style(area, Style::default().bg(theme.panel_bg));

    // Build lines from the task tree
    let lines = build_task_lines(&tasks_file.tasks);

    // Handle empty task list
    if lines.is_empty() {
        render_empty_task_list(frame, area);
        return;
    }

    // Apply scrolling and render
    let visible_lines = apply_scroll_offset(&lines, area, scroll_offset);
    render_task_list_widget(frame, area, visible_lines);
}

/// Build list of formatted lines from task tree.
fn build_task_lines(tasks: &[TaskNode]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for node in tasks {
        traverse_node(node, &mut lines, 0);
    }

    lines
}

/// Recursively traverse a task node and its subtasks, building formatted lines.
fn traverse_node(node: &TaskNode, lines: &mut Vec<Line<'static>>, depth: usize) {
    let indent = "  ".repeat(depth);

    if node.is_leaf() {
        // Leaf task: show status icon + id + name + component + deps
        let line = build_leaf_task_line(node, &indent);
        lines.push(line);
    } else {
        // Parent task: show as bold header
        let line = build_parent_task_line(node, &indent);
        lines.push(line);
    }

    // Recurse into subtasks
    for child in &node.subtasks {
        traverse_node(child, lines, depth + 1);
    }
}

/// Build a formatted line for a leaf task.
fn build_leaf_task_line(node: &TaskNode, indent: &str) -> Line<'static> {
    let theme = &DEFAULT_THEME;
    let status = node.status.as_ref().unwrap_or(&TaskStatus::Todo);
    let (icon, icon_color) = status_icon_and_color(status);

    let component = node.component.as_deref().unwrap_or("general").to_string();

    let mut spans = vec![
        Span::raw(indent.to_string()),
        Span::styled(icon.to_string(), Style::default().fg(icon_color)),
        Span::raw(" "),
        Span::styled(
            node.id.clone(),
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(": "),
        Span::raw(node.name.clone()),
        Span::raw(" ["),
        Span::styled(component, Style::default().fg(theme.warning)),
        Span::raw("]"),
    ];

    // Show deps if any
    if !node.deps.is_empty() {
        spans.push(Span::raw(" deps: "));
        spans.push(Span::styled(node.deps.join(", "), theme.muted_style()));
    }

    Line::from(spans)
}

/// Build a formatted line for a parent task (bold header).
fn build_parent_task_line(node: &TaskNode, indent: &str) -> Line<'static> {
    let header = format!("{}{} {}", indent, node.id, node.name);
    Line::from(vec![Span::styled(
        header,
        Style::default()
            .fg(DEFAULT_THEME.primary)
            .add_modifier(Modifier::BOLD),
    )])
}

/// Get icon and color for a task status.
/// Uses `theme.state_color()` for color consistency with other TUI widgets.
fn status_icon_and_color(status: &TaskStatus) -> (&'static str, ratatui::style::Color) {
    let theme = &DEFAULT_THEME;
    let icon = match status {
        TaskStatus::Done => "✓",
        TaskStatus::InProgress => "●",
        TaskStatus::Blocked => "✗",
        TaskStatus::Todo => "○",
    };
    (icon, theme.state_color(status))
}

/// Apply scroll offset and return visible lines for the given area.
fn apply_scroll_offset(
    lines: &[Line<'static>],
    area: Rect,
    scroll_offset: usize,
) -> Vec<Line<'static>> {
    let inner_height = area.height.saturating_sub(2) as usize; // Account for borders
    let total_lines = lines.len();

    // Clamp scroll offset to valid range
    let max_scroll = total_lines.saturating_sub(inner_height);
    let clamped_offset = scroll_offset.min(max_scroll);
    let end = (clamped_offset + inner_height).min(total_lines);

    lines
        .iter()
        .skip(clamped_offset)
        .take(end - clamped_offset)
        .cloned()
        .collect()
}

/// Build the border block with a given title.
fn task_list_block(title: &'static str) -> Block<'static> {
    let theme = &DEFAULT_THEME;
    let style = theme.header_style();

    Block::default()
        .padding(Padding::uniform(1))
        .title(Span::styled(title, style))
}

/// Render empty task list placeholder.
fn render_empty_task_list(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let widget = Paragraph::new(vec![Line::from("No tasks found")])
        .block(task_list_block(" Task List (p to close) "))
        .wrap(Wrap { trim: false });

    frame.render_widget(widget, area);
}

/// Render the task list widget with visible lines.
fn render_task_list_widget(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    visible_lines: Vec<Line<'static>>,
) {
    let widget = Paragraph::new(visible_lines)
        .block(task_list_block(" Task List (p to close, ↑↓ to scroll) "))
        .wrap(Wrap { trim: false });

    frame.render_widget(widget, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::snap;
    use ratatui::{Terminal, backend::TestBackend};

    /// Helper: tworzy leaf TaskNode z minimalnymi polami
    fn make_leaf(id: &str, name: &str, component: &str, status: TaskStatus) -> TaskNode {
        TaskNode {
            id: id.to_string(),
            name: name.to_string(),
            component: Some(component.to_string()),
            status: Some(status),
            deps: vec![],
            model: None,
            description: None,
            related_files: vec![],
            implementation_steps: vec![],
            acceptance_criteria: Vec::new(),
            profiles: vec![],
            subtasks: vec![],
        }
    }

    /// Helper: tworzy parent TaskNode (bez statusu, z subtaskami)
    fn make_parent(id: &str, name: &str, subtasks: Vec<TaskNode>) -> TaskNode {
        TaskNode {
            id: id.to_string(),
            name: name.to_string(),
            component: None,
            status: None,
            deps: vec![],
            model: None,
            description: None,
            related_files: vec![],
            implementation_steps: vec![],
            acceptance_criteria: Vec::new(),
            profiles: vec![],
            subtasks,
        }
    }

    /// Helper: renderuje task preview overlay do buffera dla snapshot testów
    fn render_task_preview_to_snapshot(
        tasks_file: &TasksFile,
        scroll_offset: usize,
        width: u16,
        height: u16,
    ) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("Failed to create test terminal");

        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, height);
                render_task_preview(frame, area, tasks_file, scroll_offset);
            })
            .expect("Failed to draw widget");

        snap(terminal.backend().buffer())
    }

    #[test]
    fn test_build_parent_tasks_bold() {
        let tasks = vec![make_parent(
            "1",
            "Epic 1: Frontend",
            vec![make_leaf("1.1", "Build UI", "ui", TaskStatus::Done)],
        )];

        let lines = build_task_lines(&tasks);

        // Check: parent task "1" is bold
        assert_eq!(lines.len(), 2);
        // First line should be parent "1"
        let parent_line = &lines[0];
        assert!(parent_line.spans[0].content.contains("1 Epic 1: Frontend"));
        // Verify parent has bold modifier
        assert!(
            parent_line.spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );

        // Second line should be leaf "1.1" with Done icon (✓)
        let leaf_line = &lines[1];
        assert_eq!(leaf_line.spans[1].content, "✓");
    }

    #[test]
    fn test_status_icons_and_colors() {
        let theme = &DEFAULT_THEME;
        // Verify icon mapping
        let icons_and_colors = vec![
            (TaskStatus::Done, "✓", theme.success),
            (TaskStatus::InProgress, "●", theme.warning),
            (TaskStatus::Todo, "○", theme.muted),
            (TaskStatus::Blocked, "✗", theme.error),
        ];

        for (status, expected_icon, expected_color) in icons_and_colors {
            let (icon, color) = status_icon_and_color(&status);
            assert_eq!(icon, expected_icon);
            assert_eq!(color, expected_color);
        }
    }

    #[test]
    fn test_shows_deps() {
        let mut task2 = make_leaf("2", "Second", "api", TaskStatus::Todo);
        task2.deps = vec!["1".to_string()];

        let tasks = vec![make_leaf("1", "First", "api", TaskStatus::Done), task2];

        let lines = build_task_lines(&tasks);

        // Check: task "2" has deps shown
        assert_eq!(lines.len(), 2);
        let task2_line = &lines[1];
        let task2_text = task2_line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(
            task2_text.contains("deps: 1"),
            "Expected 'deps: 1' in task 2 line: {}",
            task2_text
        );
    }

    #[test]
    fn test_scroll_offset_clamping() {
        // Build 3 lines, area with inner_height=20 (height 22 minus 2 borders)
        let lines: Vec<Line<'static>> = (1..=3).map(|i| Line::from(format!("line {i}"))).collect();
        let area = Rect::new(0, 0, 40, 22);

        // Scroll offset 100 should be clamped — all 3 lines still visible
        let visible = apply_scroll_offset(&lines, area, 100);
        assert_eq!(
            visible.len(),
            3,
            "All lines should be visible when offset exceeds total"
        );

        // Scroll offset 0 should return all lines
        let visible = apply_scroll_offset(&lines, area, 0);
        assert_eq!(visible.len(), 3);
    }

    // ========== SNAPSHOT TESTS ==========

    #[test]
    fn test_snapshot_flat_tree_3_tasks() {
        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![
                make_leaf("1", "First task", "api", TaskStatus::Done),
                make_leaf("2", "Second task", "ui", TaskStatus::InProgress),
                make_leaf("3", "Third task", "tests", TaskStatus::Todo),
            ],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 60, 10);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_tree_2_levels() {
        let mut api_task = make_leaf("1.2", "API endpoints", "api", TaskStatus::InProgress);
        api_task.deps = vec!["1.1".to_string()];

        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![make_parent(
                "1",
                "Epic: Backend",
                vec![
                    make_leaf("1.1", "Database schema", "db", TaskStatus::Done),
                    api_task,
                ],
            )],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 70, 12);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_tree_3_levels() {
        let mut jwt_task = make_leaf("1.1.2", "JWT validation", "api", TaskStatus::InProgress);
        jwt_task.deps = vec!["1.1.1".to_string()];

        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![make_parent(
                "1",
                "Project: E-commerce",
                vec![make_parent(
                    "1.1",
                    "Module: Auth",
                    vec![
                        make_leaf("1.1.1", "Login form", "ui", TaskStatus::Done),
                        jwt_task,
                    ],
                )],
            )],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 70, 14);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_all_status_icons() {
        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![
                make_leaf("1", "Completed task", "api", TaskStatus::Done),
                make_leaf("2", "In progress task", "ui", TaskStatus::InProgress),
                make_leaf("3", "Pending task", "tests", TaskStatus::Todo),
                make_leaf("4", "Blocked task", "infra", TaskStatus::Blocked),
            ],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 60, 12);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_component_and_deps() {
        let mut service = make_leaf("2", "Service layer", "backend", TaskStatus::InProgress);
        service.deps = vec!["1".to_string()];

        let mut ui = make_leaf("3", "UI integration", "frontend", TaskStatus::Todo);
        ui.deps = vec!["1".to_string(), "2".to_string()];

        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![
                make_leaf("1", "Foundation layer", "infrastructure", TaskStatus::Done),
                service,
                ui,
            ],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 80, 12);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_scroll_offset() {
        let tasks_file = TasksFile {
            default_model: None,
            tasks: (1..=20)
                .map(|i| {
                    let status = match i % 4 {
                        0 => TaskStatus::Done,
                        1 => TaskStatus::InProgress,
                        2 => TaskStatus::Todo,
                        _ => TaskStatus::Blocked,
                    };
                    make_leaf(
                        &i.to_string(),
                        &format!("Task number {i}"),
                        "testing",
                        status,
                    )
                })
                .collect(),
        };

        // Scroll offset 5 - powinno pokazać taski od 6 do 15 (wysokość 12 minus 2 dla ramki = 10 linii)
        let snapshot = render_task_preview_to_snapshot(&tasks_file, 5, 60, 12);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_empty_tree() {
        // Test snapshot: puste drzewo tasków
        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 50, 8);
        insta::assert_snapshot!(snapshot);
    }

    // ── Narrow terminal tests (width=30) ────────────────────────────────

    #[test]
    fn test_snapshot_narrow_width_30_hierarchy_3_levels() {
        // Test snapshot: 3-level hierarchy na width=30
        // Sprawdza czy indentation + task name nie overflow-uje
        let mut jwt_task = make_leaf("1.1.2", "JWT validation", "api", TaskStatus::InProgress);
        jwt_task.deps = vec!["1.1.1".to_string()];

        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![make_parent(
                "1",
                "E-commerce",
                vec![make_parent(
                    "1.1",
                    "Auth module",
                    vec![
                        make_leaf("1.1.1", "Login", "ui", TaskStatus::Done),
                        jwt_task,
                    ],
                )],
            )],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 30, 12);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_narrow_width_30_long_task_name() {
        // Test snapshot: task z długim name (40 chars) na width=30
        // Sprawdza czy długa nazwa nie powoduje overflow-u
        let long_name = "Very long task name will overflow here!!";
        assert_eq!(long_name.len(), 40);

        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![make_leaf("1", long_name, "api", TaskStatus::InProgress)],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 30, 8);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_narrow_width_30_component_and_deps() {
        // Test snapshot: task z component i deps na width=30
        // Sprawdza czy component + deps mieszczą się w linii
        let mut task = make_leaf("2", "Backend API", "infrastructure", TaskStatus::Todo);
        task.deps = vec!["1".to_string(), "3".to_string()];

        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![
                make_leaf("1", "Database", "db", TaskStatus::Done),
                task,
                make_leaf("3", "Auth", "security", TaskStatus::InProgress),
            ],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 30, 12);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_narrow_width_30_status_icons_visible() {
        // Test snapshot: wszystkie ikony statusu na width=30
        // Sprawdza że ikony statusu nie są obcięte
        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![
                make_leaf("1", "Done task", "api", TaskStatus::Done),
                make_leaf("2", "In progress", "ui", TaskStatus::InProgress),
                make_leaf("3", "Todo task", "test", TaskStatus::Todo),
                make_leaf("4", "Blocked", "infra", TaskStatus::Blocked),
            ],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 30, 14);
        insta::assert_snapshot!(snapshot);
    }

    // ── Unicode / Polish characters tests ───────────────────────────────────

    #[test]
    fn test_snapshot_unicode_polish_task_name() {
        // Test snapshot: task name z polskimi znakami (ż, ó, ł)
        // Sprawdza czy multi-byte characters nie psują alignment-u
        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![
                make_leaf(
                    "1",
                    "Implementacja żółtych alertów",
                    "ui",
                    TaskStatus::InProgress,
                ),
                make_leaf("2", "Testy końcowe", "tests", TaskStatus::Todo),
                make_leaf("3", "Konfiguracja środowiska", "infra", TaskStatus::Done),
            ],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 70, 12);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_unicode_emoji_component() {
        // Test snapshot: component z emoji
        // Sprawdza czy emoji (multi-byte) w component field renderuje się poprawnie
        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![
                make_leaf("1", "Setup build tools", "🔧 narzędzia", TaskStatus::Done),
                make_leaf(
                    "2",
                    "Create API schema",
                    "📡 backend",
                    TaskStatus::InProgress,
                ),
                make_leaf("3", "Design UI mockups", "🎨 design", TaskStatus::Todo),
            ],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 70, 12);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_unicode_mixed_hierarchy() {
        // Test snapshot: mixed ASCII/unicode w hierarchii
        // Sprawdza czy indentation i alignment jest spójny z multi-byte characters
        let mut polish_task = make_leaf(
            "1.1.2",
            "Walidacja użytkownika",
            "🔐 auth",
            TaskStatus::InProgress,
        );
        polish_task.deps = vec!["1.1.1".to_string()];

        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![make_parent(
                "1",
                "Projekt: Sklep",
                vec![make_parent(
                    "1.1",
                    "Moduł: Logowanie",
                    vec![
                        make_leaf(
                            "1.1.1",
                            "Formularz logowania",
                            "🎨 frontend",
                            TaskStatus::Done,
                        ),
                        polish_task,
                        make_leaf("1.1.3", "Obsługa błędów", "⚠️ errors", TaskStatus::Todo),
                    ],
                )],
            )],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 80, 16);
        insta::assert_snapshot!(snapshot);
    }

    // ── Deep hierarchy tests (5-6 levels) ───────────────────────────────────

    #[test]
    fn test_snapshot_hierarchy_5_levels() {
        // Test snapshot: 5-level hierarchy (1 → 1.1 → 1.1.1 → 1.1.1.1 → 1.1.1.1.1)
        // Sprawdza czy deep nesting renderuje się poprawnie z odpowiednim indentation
        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![make_parent(
                "1",
                "Root Project",
                vec![make_parent(
                    "1.1",
                    "Module A",
                    vec![make_parent(
                        "1.1.1",
                        "Feature X",
                        vec![make_parent(
                            "1.1.1.1",
                            "Component Y",
                            vec![make_leaf(
                                "1.1.1.1.1",
                                "Implementation detail",
                                "core",
                                TaskStatus::InProgress,
                            )],
                        )],
                    )],
                )],
            )],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 80, 14);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_hierarchy_5_levels_narrow_width_60() {
        // Test snapshot: 5-level hierarchy na width=60
        // Sprawdza czy indentation nie zjada całej szerokości terminalu
        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![make_parent(
                "1",
                "Project",
                vec![make_parent(
                    "1.1",
                    "Mod A",
                    vec![make_parent(
                        "1.1.1",
                        "Feature",
                        vec![make_parent(
                            "1.1.1.1",
                            "Comp",
                            vec![make_leaf("1.1.1.1.1", "Detail", "core", TaskStatus::Done)],
                        )],
                    )],
                )],
            )],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 60, 14);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_hierarchy_6_levels_with_siblings() {
        // Test snapshot: 6-level hierarchy z siblings na każdym poziomie
        // Sprawdza czy rendering deep tree z wieloma węzłami jest czytelny
        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![
                make_parent(
                    "1",
                    "Root A",
                    vec![make_parent(
                        "1.1",
                        "Module 1",
                        vec![make_parent(
                            "1.1.1",
                            "Feature 1",
                            vec![make_parent(
                                "1.1.1.1",
                                "Component 1",
                                vec![make_parent(
                                    "1.1.1.1.1",
                                    "Subcomponent 1",
                                    vec![make_leaf(
                                        "1.1.1.1.1.1",
                                        "Final task",
                                        "impl",
                                        TaskStatus::Done,
                                    )],
                                )],
                            )],
                        )],
                    )],
                ),
                make_parent(
                    "2",
                    "Root B",
                    vec![make_leaf("2.1", "Simple task", "tests", TaskStatus::Todo)],
                ),
            ],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 80, 18);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_hierarchy_5_levels_indentation_overflow_check() {
        // Test snapshot: 5 levels z długim task name — sprawdza czy indentation + content mieści się
        // Każdy poziom dodaje 2 spacje indentation (depth * 2)
        // Poziom 4 (leaf) = 8 spacji indentation
        let long_name = "Very long task name that should still fit";

        let tasks_file = TasksFile {
            default_model: None,
            tasks: vec![make_parent(
                "1",
                "L0",
                vec![make_parent(
                    "1.1",
                    "L1",
                    vec![make_parent(
                        "1.1.1",
                        "L2",
                        vec![make_parent(
                            "1.1.1.1",
                            "L3",
                            vec![make_leaf(
                                "1.1.1.1.1",
                                long_name,
                                "component",
                                TaskStatus::InProgress,
                            )],
                        )],
                    )],
                )],
            )],
        };

        let snapshot = render_task_preview_to_snapshot(&tasks_file, 0, 80, 14);
        insta::assert_snapshot!(snapshot);
    }
}
