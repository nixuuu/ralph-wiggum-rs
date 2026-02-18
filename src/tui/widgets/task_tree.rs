use std::collections::HashSet;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, StatefulWidget, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::shared::progress::TaskStatus;
use crate::shared::tasks::TaskNode;
use crate::tui::theme::DEFAULT_THEME;

// ── Status icons ────────────────────────────────────────────────────

const ICON_DONE: &str = "✓ ";
const ICON_IN_PROGRESS: &str = "◎ ";
const ICON_TODO: &str = "○ ";
const ICON_BLOCKED: &str = "⊘ ";
const ICON_NONE: &str = "  ";

const EXPAND_OPEN: &str = "▾ ";
const EXPAND_CLOSED: &str = "▸ ";

/// Indent step per nesting level (2 spaces).
const INDENT_WIDTH: usize = 2;

// ── TreeState ───────────────────────────────────────────────────────

/// Mutable state for the task tree widget.
///
/// Tracks expanded nodes, selection, and scroll offset.
/// Separated from rendering to follow ratatui's StatefulWidget pattern.
#[derive(Debug, Clone, Default)]
pub struct TreeState {
    /// Set of node IDs whose children are visible (expanded)
    pub expanded: HashSet<String>,
    /// Index of selected item in the flat (visible) list
    pub selected: usize,
    /// Vertical scroll offset
    pub scroll_offset: usize,
}

impl TreeState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Move selection up by one row.
    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move selection down by one row (needs visible row count).
    pub fn select_next(&mut self, visible_count: usize) {
        let max = visible_count.saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
    }

    /// Toggle expand/collapse for the node at `selected` index.
    /// Returns true if a toggle actually happened.
    pub fn toggle_expand(&mut self, rows: &[FlatRow]) -> bool {
        if let Some(row) = rows.get(self.selected)
            && row.has_children
        {
            if self.expanded.contains(&row.id) {
                self.expanded.remove(&row.id);
            } else {
                self.expanded.insert(row.id.clone());
            }
            return true;
        }
        false
    }

    /// Clamp selection to valid range after data changes.
    pub fn clamp(&mut self, visible_count: usize) {
        if visible_count == 0 {
            self.selected = 0;
            self.scroll_offset = 0;
        } else if self.selected >= visible_count {
            self.selected = visible_count - 1;
        }
    }

    /// Adjust scroll_offset so that `selected` is visible within `viewport_height`.
    pub fn adjust_scroll(&mut self, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + viewport_height {
            self.scroll_offset = self.selected - viewport_height + 1;
        }
    }
}

// ── FlatRow ─────────────────────────────────────────────────────────

/// A single visible row in the flattened tree.
#[derive(Debug, Clone)]
pub struct FlatRow {
    pub id: String,
    pub name: String,
    pub depth: usize,
    pub status: Option<TaskStatus>,
    pub has_children: bool,
    pub is_expanded: bool,
    pub component: Option<String>,
    pub deps: Vec<String>,
}

/// Recursively flatten task nodes into visible rows, respecting expanded state.
pub fn flatten_nodes(nodes: &[TaskNode], expanded: &HashSet<String>) -> Vec<FlatRow> {
    let mut rows = Vec::new();
    for node in nodes {
        flatten_node(node, 0, expanded, &mut rows);
    }
    rows
}

fn flatten_node(node: &TaskNode, depth: usize, expanded: &HashSet<String>, out: &mut Vec<FlatRow>) {
    let has_children = !node.subtasks.is_empty();
    let is_expanded = expanded.contains(&node.id);

    out.push(FlatRow {
        id: node.id.clone(),
        name: node.name.clone(),
        depth,
        status: node.status.clone(),
        has_children,
        is_expanded,
        component: node.component.clone(),
        deps: node.deps.clone(),
    });

    if has_children && is_expanded {
        for child in &node.subtasks {
            flatten_node(child, depth + 1, expanded, out);
        }
    }
}

// ── TaskTreeWidget ──────────────────────────────────────────────────

/// Reusable hierarchical tree view for task nodes.
///
/// Renders a flat list of visible rows with:
/// - Indentation per nesting level (2 spaces)
/// - ▾/▸ icons for expandable parents
/// - Status icons: ✓ done (green), ◎ in_progress (cyan), ○ todo (white), ⊘ blocked (red)
/// - Component tag `[comp]` in DarkGray after the name
/// - Deps arrows `→ dep_id` in DarkGray on the right
/// - Selected item: reverse video (bg=primary, fg=black)
///
/// Implements `StatefulWidget` with `TreeState`.
/// Supports two modes:
/// - `new(nodes)` — flatten nodes internally (default order)
/// - `from_rows(rows)` — use pre-computed rows (e.g. sorted/filtered)
pub struct TaskTreeWidget<'a> {
    nodes: Option<&'a [TaskNode]>,
    pre_rows: Option<Vec<FlatRow>>,
}

impl<'a> TaskTreeWidget<'a> {
    pub fn new(nodes: &'a [TaskNode]) -> Self {
        Self {
            nodes: Some(nodes),
            pre_rows: None,
        }
    }

    /// Create widget from pre-computed rows (sorted, filtered, etc.).
    pub fn from_rows(rows: Vec<FlatRow>) -> Self {
        Self {
            nodes: None,
            pre_rows: Some(rows),
        }
    }
}

impl StatefulWidget for TaskTreeWidget<'_> {
    type State = TreeState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut TreeState) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let rows = match self.pre_rows {
            Some(r) => r,
            None => {
                let nodes = self
                    .nodes
                    .expect("TaskTreeWidget: nodes or pre_rows required");
                flatten_nodes(nodes, &state.expanded)
            }
        };
        let viewport_height = area.height as usize;

        // Clamp selection to valid range (tree data may have changed since last render)
        state.clamp(rows.len());
        state.adjust_scroll(viewport_height);

        let visible_slice = rows.iter().skip(state.scroll_offset).take(viewport_height);

        for (i, row) in visible_slice.enumerate() {
            let abs_index = state.scroll_offset + i;
            let is_selected = abs_index == state.selected;
            let y = area.y + i as u16;

            let line = build_row_line(row, is_selected, area.width as usize);
            let row_area = Rect::new(area.x, y, area.width, 1);
            Paragraph::new(line).render(row_area, buf);
        }
    }
}

// ── Row rendering ───────────────────────────────────────────────────

/// Build a styled Line for a single tree row.
fn build_row_line(row: &FlatRow, is_selected: bool, max_width: usize) -> Line<'static> {
    let theme = &DEFAULT_THEME;
    let mut spans: Vec<Span<'static>> = Vec::new();

    // Indentation
    let indent = " ".repeat(row.depth * INDENT_WIDTH);
    spans.push(Span::raw(indent));

    // Expand/collapse indicator or status icon
    let (icon, icon_style) = if row.has_children {
        let icon = if row.is_expanded {
            EXPAND_OPEN
        } else {
            EXPAND_CLOSED
        };
        (icon, theme.muted_style())
    } else {
        let icon = status_icon(row.status.as_ref());
        (icon, status_style(row.status.as_ref()))
    };
    spans.push(Span::styled(icon.to_string(), icon_style));

    // Task ID (dimmed) + name
    spans.push(Span::styled(format!("{} ", row.id), theme.muted_style()));

    let name_style = if is_selected {
        Style::default()
            .fg(theme.primary)
            .add_modifier(Modifier::BOLD)
    } else if row.has_children {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    spans.push(Span::styled(row.name.clone(), name_style));

    // Component tag [comp] — dimmed, after name
    if let Some(ref comp) = row.component {
        spans.push(Span::styled(format!(" [{}]", comp), theme.muted_style()));
    }

    // Deps arrows: → dep_id
    if !row.deps.is_empty() {
        let deps_text = row
            .deps
            .iter()
            .map(|d| format!("→ {d}"))
            .collect::<Vec<_>>()
            .join(" ");
        // Right-align deps if there's room
        let content_width: usize = spans.iter().map(|s| s.width()).sum();
        let deps_width = deps_text.width() + 1; // +1 for leading space
        if content_width + deps_width < max_width {
            let padding = max_width - content_width - deps_width;
            spans.push(Span::raw(" ".repeat(padding)));
        } else {
            spans.push(Span::raw(" ".to_string()));
        }
        spans.push(Span::styled(deps_text, theme.muted_style()));
    }

    // Selection: reverse video
    let mut line = Line::from(spans);
    if is_selected {
        line = line.style(
            Style::default()
                .bg(theme.primary)
                .fg(ratatui::style::Color::Black),
        );
    }

    line
}

/// Map task status to a display icon.
fn status_icon(status: Option<&TaskStatus>) -> &'static str {
    match status {
        Some(TaskStatus::Done) => ICON_DONE,
        Some(TaskStatus::InProgress) => ICON_IN_PROGRESS,
        Some(TaskStatus::Todo) => ICON_TODO,
        Some(TaskStatus::Blocked) => ICON_BLOCKED,
        None => ICON_NONE,
    }
}

/// Map task status to a color style.
fn status_style(status: Option<&TaskStatus>) -> Style {
    let theme = &DEFAULT_THEME;
    match status {
        Some(s) => Style::default().fg(theme.state_color(s)),
        None => theme.muted_style(),
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::tasks::TasksFile;
    use crate::test_helpers::snap;
    use ratatui::{Terminal, backend::TestBackend};

    fn sample_tasks() -> Vec<TaskNode> {
        let yaml = r#"
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
        let tf: TasksFile = serde_yaml::from_str(yaml).unwrap();
        tf.tasks
    }

    /// Helper: render tree widget into a buffer.
    fn render_tree(nodes: &[TaskNode], state: &mut TreeState, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("Failed to create test terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, height);
                let widget = TaskTreeWidget::new(nodes);
                frame.render_stateful_widget(widget, area, state);
            })
            .expect("Failed to draw");
        terminal.backend().buffer().clone()
    }

    // ── Unit tests ──────────────────────────────────────────────────

    #[test]
    fn test_tree_state_default() {
        let state = TreeState::new();
        assert_eq!(state.selected, 0);
        assert_eq!(state.scroll_offset, 0);
        assert!(state.expanded.is_empty());
    }

    #[test]
    fn test_select_navigation() {
        let mut state = TreeState::new();
        state.select_next(5);
        assert_eq!(state.selected, 1);
        state.select_next(5);
        assert_eq!(state.selected, 2);
        state.select_prev();
        assert_eq!(state.selected, 1);
        state.select_prev();
        assert_eq!(state.selected, 0);
        // Can't go below 0
        state.select_prev();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_select_next_at_end() {
        let mut state = TreeState::new();
        state.selected = 4;
        state.select_next(5);
        assert_eq!(state.selected, 4); // Already at max
    }

    #[test]
    fn test_clamp() {
        let mut state = TreeState::new();
        state.selected = 10;
        state.clamp(3);
        assert_eq!(state.selected, 2);

        state.clamp(0);
        assert_eq!(state.selected, 0);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_toggle_expand() {
        let tasks = sample_tasks();
        let rows = flatten_nodes(&tasks, &HashSet::new());
        let mut state = TreeState::new();

        // Toggle expand on root "1"
        assert!(state.toggle_expand(&rows));
        assert!(state.expanded.contains("1"));

        // Collapse it
        let rows_expanded = flatten_nodes(&tasks, &state.expanded);
        state.selected = 0;
        assert!(state.toggle_expand(&rows_expanded));
        assert!(!state.expanded.contains("1"));
    }

    #[test]
    fn test_toggle_expand_leaf_is_noop() {
        let tasks = sample_tasks();
        let mut state = TreeState::new();
        state.expanded.insert("1".to_string());

        let rows = flatten_nodes(&tasks, &state.expanded);
        // Select leaf "1.1" (index 1)
        state.selected = 1;
        assert!(!state.toggle_expand(&rows));
    }

    #[test]
    fn test_flatten_collapsed() {
        let tasks = sample_tasks();
        let rows = flatten_nodes(&tasks, &HashSet::new());
        // Only root nodes visible when collapsed
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "1");
        assert_eq!(rows[1].id, "2");
    }

    #[test]
    fn test_flatten_expanded() {
        let tasks = sample_tasks();
        let mut expanded = HashSet::new();
        expanded.insert("1".to_string());
        expanded.insert("2".to_string());

        let rows = flatten_nodes(&tasks, &expanded);
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0].id, "1");
        assert_eq!(rows[1].id, "1.1");
        assert_eq!(rows[2].id, "1.2");
        assert_eq!(rows[3].id, "2");
        assert_eq!(rows[4].id, "2.1");
        assert_eq!(rows[5].id, "2.2");
    }

    #[test]
    fn test_flatten_depth() {
        let tasks = sample_tasks();
        let mut expanded = HashSet::new();
        expanded.insert("1".to_string());

        let rows = flatten_nodes(&tasks, &expanded);
        assert_eq!(rows[0].depth, 0); // "1"
        assert_eq!(rows[1].depth, 1); // "1.1"
        assert_eq!(rows[2].depth, 1); // "1.2"
        assert_eq!(rows[3].depth, 0); // "2"
    }

    #[test]
    fn test_adjust_scroll() {
        let mut state = TreeState::new();
        state.selected = 5;
        state.adjust_scroll(3);
        assert_eq!(state.scroll_offset, 3); // 5 - 3 + 1

        state.selected = 1;
        state.adjust_scroll(3);
        assert_eq!(state.scroll_offset, 1);
    }

    #[test]
    fn test_status_icons() {
        assert_eq!(status_icon(Some(&TaskStatus::Done)), ICON_DONE);
        assert_eq!(status_icon(Some(&TaskStatus::InProgress)), ICON_IN_PROGRESS);
        assert_eq!(status_icon(Some(&TaskStatus::Todo)), ICON_TODO);
        assert_eq!(status_icon(Some(&TaskStatus::Blocked)), ICON_BLOCKED);
        assert_eq!(status_icon(None), ICON_NONE);
    }

    // ── Snapshot tests ──────────────────────────────────────────────

    #[test]
    fn test_snapshot_collapsed_tree() {
        let tasks = sample_tasks();
        let mut state = TreeState::new();
        let buffer = render_tree(&tasks, &mut state, 50, 6);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_fully_expanded() {
        let tasks = sample_tasks();
        let mut state = TreeState::new();
        state.expanded.insert("1".to_string());
        state.expanded.insert("2".to_string());

        let buffer = render_tree(&tasks, &mut state, 60, 10);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_with_selection() {
        let tasks = sample_tasks();
        let mut state = TreeState::new();
        state.expanded.insert("1".to_string());
        state.selected = 2; // "1.2" (in_progress with deps)

        let buffer = render_tree(&tasks, &mut state, 55, 8);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_empty_tree() {
        let tasks: Vec<TaskNode> = Vec::new();
        let mut state = TreeState::new();
        let buffer = render_tree(&tasks, &mut state, 40, 4);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_deep_nesting() {
        let yaml = r#"
default_model: claude-sonnet-4-5-20250929
tasks:
  - id: "1"
    name: "Root"
    component: core
    subtasks:
      - id: "1.1"
        name: "Child"
        component: core
        subtasks:
          - id: "1.1.1"
            name: "Grandchild"
            status: done
            component: core
          - id: "1.1.2"
            name: "Grandchild 2"
            status: todo
            component: core
            deps: ["1.1.1"]
"#;
        let tf: TasksFile = serde_yaml::from_str(yaml).unwrap();
        let mut state = TreeState::new();
        state.expanded.insert("1".to_string());
        state.expanded.insert("1.1".to_string());

        let buffer = render_tree(&tf.tasks, &mut state, 60, 8);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_all_statuses() {
        let yaml = r#"
tasks:
  - id: "1"
    name: "Done task"
    status: done
    component: api
  - id: "2"
    name: "In progress task"
    status: in_progress
    component: api
  - id: "3"
    name: "Todo task"
    status: todo
    component: tui
  - id: "4"
    name: "Blocked task"
    status: blocked
    component: tui
    deps: ["2", "3"]
"#;
        let tf: TasksFile = serde_yaml::from_str(yaml).unwrap();
        let mut state = TreeState::new();

        let buffer = render_tree(&tf.tasks, &mut state, 60, 6);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_scrolled() {
        let tasks = sample_tasks();
        let mut state = TreeState::new();
        state.expanded.insert("1".to_string());
        state.expanded.insert("2".to_string());
        // 6 rows total, viewport 3 rows, select last item
        state.selected = 5;

        let buffer = render_tree(&tasks, &mut state, 55, 3);
        insta::assert_snapshot!(snap(&buffer));
    }
}
