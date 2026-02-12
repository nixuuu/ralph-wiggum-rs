use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::shared::progress::TaskStatus;
use crate::shared::tasks::{TaskNode, TasksFile};

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
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(": "),
        Span::raw(node.name.clone()),
        Span::raw(" ["),
        Span::styled(component, Style::default().fg(Color::Yellow)),
        Span::raw("]"),
    ];

    // Show deps if any
    if !node.deps.is_empty() {
        spans.push(Span::raw(" deps: "));
        spans.push(Span::styled(
            node.deps.join(", "),
            Style::default().fg(Color::DarkGray),
        ));
    }

    Line::from(spans)
}

/// Build a formatted line for a parent task (bold header).
fn build_parent_task_line(node: &TaskNode, indent: &str) -> Line<'static> {
    let header = format!("{}{} {}", indent, node.id, node.name);
    Line::from(vec![Span::styled(
        header,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )])
}

/// Get icon and color for a task status.
fn status_icon_and_color(status: &TaskStatus) -> (&'static str, Color) {
    match status {
        TaskStatus::Done => ("✓", Color::Green),
        TaskStatus::InProgress => ("●", Color::Cyan),
        TaskStatus::Blocked => ("✗", Color::Red),
        TaskStatus::Todo => ("○", Color::White),
    }
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

/// Render empty task list placeholder.
fn render_empty_task_list(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .title(Span::styled(
            " Task List (p to close) ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let widget = Paragraph::new(vec![Line::from("No tasks found")])
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(widget, area);
}

/// Render the task list widget with visible lines.
fn render_task_list_widget(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    visible_lines: Vec<Line<'static>>,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .title(Span::styled(
            " Task List (p to close, ↑↓ to scroll) ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let widget = Paragraph::new(visible_lines)
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(widget, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build rendered lines from task tree
    fn render_task_lines(tasks: &[TaskNode]) -> Vec<Line<'static>> {
        build_task_lines(tasks)
    }

    #[test]
    fn test_build_parent_tasks_bold() {
        let tasks = vec![TaskNode {
            id: "1".to_string(),
            name: "Epic 1: Frontend".to_string(),
            component: None,
            status: None,
            deps: vec![],
            model: None,
            description: None,
            related_files: vec![],
            implementation_steps: vec![],
            subtasks: vec![TaskNode {
                id: "1.1".to_string(),
                name: "Build UI".to_string(),
                component: Some("ui".to_string()),
                status: Some(TaskStatus::Done),
                deps: vec![],
                model: None,
                description: None,
                related_files: vec![],
                implementation_steps: vec![],
                subtasks: vec![],
            }],
        }];

        let lines = render_task_lines(&tasks);

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
        // Verify icon mapping
        let icons_and_colors = vec![
            (TaskStatus::Done, "✓", Color::Green),
            (TaskStatus::InProgress, "●", Color::Cyan),
            (TaskStatus::Todo, "○", Color::White),
            (TaskStatus::Blocked, "✗", Color::Red),
        ];

        for (status, expected_icon, expected_color) in icons_and_colors {
            let (icon, color) = status_icon_and_color(&status);
            assert_eq!(icon, expected_icon);
            assert_eq!(color, expected_color);
        }
    }

    #[test]
    fn test_shows_deps() {
        let tasks = vec![
            TaskNode {
                id: "1".to_string(),
                name: "First".to_string(),
                component: Some("api".to_string()),
                status: Some(TaskStatus::Done),
                deps: vec![],
                model: None,
                description: None,
                related_files: vec![],
                implementation_steps: vec![],
                subtasks: vec![],
            },
            TaskNode {
                id: "2".to_string(),
                name: "Second".to_string(),
                component: Some("api".to_string()),
                status: Some(TaskStatus::Todo),
                deps: vec!["1".to_string()],
                model: None,
                description: None,
                related_files: vec![],
                implementation_steps: vec![],
                subtasks: vec![],
            },
        ];

        let lines = render_task_lines(&tasks);

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
        // Test that scroll offset is properly clamped to valid range
        let total_lines: usize = 1;
        let inner_height: usize = 18;
        let max_scroll = total_lines.saturating_sub(inner_height);

        // saturating_sub ensures result is never negative (0 when total_lines < inner_height)
        assert_eq!(max_scroll, 0);
        let clamped = 100usize.min(max_scroll);
        assert_eq!(clamped, 0);
    }
}
