use std::collections::HashSet;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::StatefulWidget,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Widget},
};

use crate::shared::progress::TaskStatus;
use crate::shared::tasks::{TaskNode, TasksFile};
use crate::tui::theme::DEFAULT_THEME;
use crate::tui::widgets::task_tree::{self, FlatRow};

// ── Constants ──────────────────────────────────────────────────────

const MIN_WIDTH: u16 = 15;
const MAX_WIDTH: u16 = 60;
const DEFAULT_WIDTH: u16 = 40;

// ── State ──────────────────────────────────────────────────────────

/// Mutable state for the task sidebar.
///
/// Tracks which nodes are expanded, current selection, scroll offset,
/// sidebar width, and visibility. Separated from rendering widget
/// to follow ratatui's state/widget pattern.
#[derive(Debug, Clone)]
pub struct TaskSidebarState {
    /// Current task tree data
    tasks: Vec<TaskNode>,
    /// Set of node IDs whose children are visible (expanded)
    expanded_nodes: HashSet<String>,
    /// Index of the selected item in the flat (visible) list
    pub selected_index: usize,
    /// Vertical scroll offset
    pub scroll_offset: usize,
    /// Sidebar column width in characters
    width: u16,
    /// Whether the sidebar is visible
    pub visible: bool,
    /// Total count of done tasks (cached for footer)
    done_count: usize,
    /// Total count of all leaf tasks (cached for footer)
    total_count: usize,
    /// ID of the task being worked on (for highlighting)
    current_task_id: Option<String>,
}

impl Default for TaskSidebarState {
    fn default() -> Self {
        Self {
            tasks: Vec::new(),
            expanded_nodes: HashSet::new(),
            selected_index: 0,
            scroll_offset: 0,
            width: DEFAULT_WIDTH,
            visible: true,
            done_count: 0,
            total_count: 0,
            current_task_id: None,
        }
    }
}

impl TaskSidebarState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace task data with fresh snapshot from tasks.yml.
    /// Preserves expanded_nodes, selected_index, scroll_offset.
    pub fn refresh(&mut self, tasks_file: &TasksFile) {
        self.tasks = tasks_file.tasks.clone();

        // Recalculate cached counters
        let summary = tasks_file.to_summary();
        self.done_count = summary.done;
        self.total_count = summary.total();

        // Clamp selection to new visible list length
        let visible_len = self.visible_rows().len();
        if visible_len == 0 {
            self.selected_index = 0;
            self.scroll_offset = 0;
        } else if self.selected_index >= visible_len {
            self.selected_index = visible_len - 1;
        }
    }

    /// Increase sidebar width by 1, up to MAX_WIDTH.
    pub fn grow(&mut self) {
        self.width = (self.width + 1).min(MAX_WIDTH);
    }

    /// Decrease sidebar width by 1, down to MIN_WIDTH.
    pub fn shrink(&mut self) {
        self.width = self.width.saturating_sub(1).max(MIN_WIDTH);
    }

    /// Current sidebar width.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Toggle visibility on/off.
    pub fn toggle_visible(&mut self) {
        self.visible = !self.visible;
    }

    /// Set current task ID for highlighting.
    pub fn set_current_task(&mut self, task_id: Option<String>) {
        self.current_task_id = task_id;
    }

    /// Move selection up by one row.
    pub fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection down by one row.
    pub fn select_next(&mut self) {
        let max = self.visible_rows().len().saturating_sub(1);
        if self.selected_index < max {
            self.selected_index += 1;
        }
    }

    /// Toggle expand/collapse for the currently selected node.
    /// Only works on parent nodes (those with subtasks).
    pub fn toggle_expand(&mut self) {
        let rows = self.visible_rows();
        if let Some(row) = rows.get(self.selected_index)
            && row.has_children
        {
            if self.expanded_nodes.contains(&row.id) {
                self.expanded_nodes.remove(&row.id);
            } else {
                self.expanded_nodes.insert(row.id.clone());
            }
        }
    }

    /// Build the flat list of currently visible rows by walking the tree.
    /// Collapsed parent nodes hide their children.
    /// Reuses flatten_nodes from task_tree to avoid duplication.
    fn visible_rows(&self) -> Vec<FlatRow> {
        task_tree::flatten_nodes(&self.tasks, &self.expanded_nodes)
    }

    /// Adjust scroll_offset so that selected_index is visible within `viewport_height`.
    fn adjust_scroll(&mut self, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + viewport_height {
            self.scroll_offset = self.selected_index - viewport_height + 1;
        }
    }
}

// ── Widget ─────────────────────────────────────────────────────────

/// Rendering widget for the task sidebar.
/// Borrows state mutably to adjust scroll offset during render.
pub struct TaskSidebar<'a> {
    state: &'a mut TaskSidebarState,
    /// Whether the sidebar panel has focus
    focused: bool,
}

impl<'a> TaskSidebar<'a> {
    pub fn new(state: &'a mut TaskSidebarState, focused: bool) -> Self {
        Self { state, focused }
    }

    /// Render the sidebar into a buffer area.
    pub fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.state.visible || area.width < 5 || area.height < 5 {
            return;
        }

        let theme = &DEFAULT_THEME;

        // Progress footer text: [3/10 30%]
        let pct = if self.state.total_count > 0 {
            (self.state.done_count as f64 / self.state.total_count as f64 * 100.0).round() as u32
        } else {
            0
        };
        let footer_text = format!(
            " {}/{} {}% ",
            self.state.done_count, self.state.total_count, pct
        );

        let block = Block::default()
            .padding(Padding::new(2, 2, 2, 2))
            .style(Style::default().bg(theme.sidebar_bg))
            .title(Span::styled(" Tasks ", theme.header_style()))
            .title_bottom(Line::from(vec![Span::styled(
                footer_text,
                theme.muted_style(),
            )]));

        let inner = block.inner(area);
        block.render(area, buf);

        // Right edge: thick border strip (full-height column with border_normal bg)
        if area.width > 0 {
            let x = area.x + area.width - 1;
            let border_style = Style::default().fg(theme.secondary).bg(theme.sidebar_bg);
            for y in area.y..area.y + area.height {
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, y)) {
                    cell.set_symbol("▐").set_style(border_style);
                }
            }
        }

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let rows = self.state.visible_rows();
        let viewport_height = inner.height as usize;

        // Adjust scroll to keep selection visible
        self.state.adjust_scroll(viewport_height);

        let visible_slice = rows
            .iter()
            .skip(self.state.scroll_offset)
            .take(viewport_height);

        for (i, row) in visible_slice.enumerate() {
            let abs_index = self.state.scroll_offset + i;
            let is_selected = abs_index == self.state.selected_index;
            let is_current = self.state.current_task_id.as_ref() == Some(&row.id);
            let y = inner.y + i as u16;

            let line = build_row_line(row, is_selected, is_current);
            let paragraph = Paragraph::new(line);
            let row_area = Rect::new(inner.x, y, inner.width, 1);
            paragraph.render(row_area, buf);
        }

        // Scrollbar (VerticalRight) — widoczny tylko gdy więcej wierszy niż viewport
        if rows.len() > viewport_height {
            let max_scroll = rows.len().saturating_sub(viewport_height);
            let mut sb_state = ScrollbarState::default()
                .content_length(max_scroll + 1)
                .viewport_content_length(viewport_height)
                .position(self.state.scroll_offset);
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_symbol("▐")
                .track_symbol(Some("▐"))
                .thumb_style(Style::default().fg(DEFAULT_THEME.secondary))
                .track_style(Style::default().fg(DEFAULT_THEME.border_normal))
                .begin_symbol(None)
                .end_symbol(None)
                .render(area, buf, &mut sb_state);
        }
    }
}

// ── Row rendering ──────────────────────────────────────────────────

/// Build a styled Line for a single tree row.
///
/// # Arguments
/// - `row` — task data
/// - `is_selected` — true if this row is currently selected (focus indicator)
/// - `is_current` — true if this row is the task being worked on (bold + primary)
fn build_row_line(row: &FlatRow, is_selected: bool, is_current: bool) -> Line<'static> {
    let theme = &DEFAULT_THEME;
    let mut spans: Vec<Span<'static>> = Vec::new();

    // Indentation: 2 spaces per depth level
    let indent = "  ".repeat(row.depth);
    spans.push(Span::raw(indent));

    // Expand/collapse indicator or status icon
    let icon = if row.has_children {
        if row.is_expanded { "▼ " } else { "▶ " }
    } else {
        status_icon(row.status.as_ref())
    };

    let icon_style = if row.has_children {
        theme.muted_style()
    } else {
        status_style(row.status.as_ref())
    };
    spans.push(Span::styled(icon.to_string(), icon_style));

    // Task name — highlight current task being worked on
    let name_style = if is_current {
        // Current task: yellow bold (visually distinct from selection)
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD)
    } else if is_selected {
        // Selected: primary + bold (overridden by line-level reverse below)
        Style::default()
            .fg(theme.primary)
            .add_modifier(Modifier::BOLD)
    } else if row.has_children {
        // Parent nodes: bold
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        // Regular leaf tasks: default
        Style::default()
    };
    spans.push(Span::styled(row.name.clone(), name_style));

    // Component tag [component] — dimmed, after name
    if let Some(ref comp) = row.component {
        spans.push(Span::styled(format!(" [{}]", comp), theme.muted_style()));
    }

    // Deps arrows on the right: → dep_id
    if !row.deps.is_empty() {
        let deps_text = row
            .deps
            .iter()
            .map(|d| format!("→{d}"))
            .collect::<Vec<_>>()
            .join(" ");
        spans.push(Span::styled(format!(" {}", deps_text), theme.muted_style()));
    }

    // Selection indicator: override every span's fg for readability on highlighted bg
    if is_selected {
        let highlight_fg = ratatui::style::Color::Black;
        let highlight_bg = theme.primary;
        for span in &mut spans {
            span.style = span.style.fg(highlight_fg).bg(highlight_bg);
        }
        Line::from(spans).style(Style::default().bg(highlight_bg).fg(highlight_fg))
    } else {
        Line::from(spans)
    }
}

/// Map task status to a display icon.
fn status_icon(status: Option<&TaskStatus>) -> &'static str {
    match status {
        Some(TaskStatus::Done) => "✓ ",
        Some(TaskStatus::InProgress) => "~ ",
        Some(TaskStatus::Todo) => "○ ",
        Some(TaskStatus::Blocked) => "! ",
        None => "  ", // Parent nodes without status
    }
}

/// Map task status to a color style.
/// Uses theme.state_color() for consistency with other TUI components.
fn status_style(status: Option<&TaskStatus>) -> Style {
    let theme = &DEFAULT_THEME;
    match status {
        Some(s) => Style::default().fg(theme.state_color(s)),
        None => theme.muted_style(),
    }
}

// ── Overlay rendering ──────────────────────────────────────────────

/// Render sidebar as an overlay "sheet" on top of content area.
///
/// Used in Small breakpoint where sidebar doesn't fit as inline panel.
/// Renders: dim backdrop → sidebar with solid background.
/// Width: min(state.width(), 70% of content), at least MIN_WIDTH.
pub fn render_sidebar_overlay(
    state: &mut TaskSidebarState,
    focused: bool,
    content_area: Rect,
    buf: &mut Buffer,
) {
    if !state.visible || content_area.width < MIN_WIDTH + 2 || content_area.height < 3 {
        return;
    }

    // Sidebar width: min(state width, 70% of content), clamped to MIN_WIDTH
    let max_overlay_width = (content_area.width as f64 * 0.7) as u16;
    let sidebar_width = state.width().min(max_overlay_width).max(MIN_WIDTH);

    let sidebar_rect = Rect {
        x: content_area.x,
        y: content_area.y,
        width: sidebar_width,
        height: content_area.height,
    };

    // Step 1: Dim the entire content area (backdrop)
    let dim_style = Style::default().add_modifier(Modifier::DIM);
    for y in content_area.y..content_area.y + content_area.height {
        for x in content_area.x..content_area.x + content_area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(dim_style);
            }
        }
    }

    // Step 2: Reset sidebar cells completely (removes DIM from backdrop + prevents bleed-through)
    for y in sidebar_rect.y..sidebar_rect.y + sidebar_rect.height {
        for x in sidebar_rect.x..sidebar_rect.x + sidebar_rect.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
            }
        }
    }

    // Step 3: Render sidebar on top of cleared background
    let sidebar = TaskSidebar::new(state, focused);
    sidebar.render(sidebar_rect, buf);
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::snap;

    fn sample_tasks_file() -> TasksFile {
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
        serde_yaml::from_str(yaml).unwrap()
    }

    /// Helper: creates state + renders sidebar into a buffer.
    fn render_sidebar(state: &mut TaskSidebarState, width: u16, height: u16) -> Buffer {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).expect("Failed to create test terminal");
        terminal
            .draw(|frame| {
                let area = ratatui::layout::Rect::new(0, 0, width, height);
                let sidebar = TaskSidebar::new(state, true);
                sidebar.render(area, frame.buffer_mut());
            })
            .expect("Failed to draw");
        terminal.backend().buffer().clone()
    }

    #[test]
    fn test_default_state() {
        let state = TaskSidebarState::new();
        assert!(state.visible);
        assert_eq!(state.width(), DEFAULT_WIDTH);
        assert_eq!(state.selected_index, 0);
        assert_eq!(state.scroll_offset, 0);
        assert!(state.expanded_nodes.is_empty());
    }

    #[test]
    fn test_refresh_updates_counters() {
        let mut state = TaskSidebarState::new();
        let tf = sample_tasks_file();
        state.refresh(&tf);

        // 4 leaves: 1 done, 1 in_progress, 1 todo, 1 blocked
        assert_eq!(state.done_count, 1);
        assert_eq!(state.total_count, 4);
    }

    #[test]
    fn test_grow_shrink_bounds() {
        let mut state = TaskSidebarState::new();
        state.width = MAX_WIDTH;
        state.grow();
        assert_eq!(state.width(), MAX_WIDTH);

        state.width = MIN_WIDTH;
        state.shrink();
        assert_eq!(state.width(), MIN_WIDTH);
    }

    #[test]
    fn test_toggle_visible() {
        let mut state = TaskSidebarState::new();
        assert!(state.visible);
        state.toggle_visible();
        assert!(!state.visible);
        state.toggle_visible();
        assert!(state.visible);
    }

    #[test]
    fn test_select_navigation() {
        let mut state = TaskSidebarState::new();
        let tf = sample_tasks_file();
        state.refresh(&tf);

        // Initially: 2 root nodes collapsed → 2 visible rows
        assert_eq!(state.visible_rows().len(), 2);
        assert_eq!(state.selected_index, 0);

        state.select_next();
        assert_eq!(state.selected_index, 1);

        state.select_next(); // Already at end
        assert_eq!(state.selected_index, 1);

        state.select_prev();
        assert_eq!(state.selected_index, 0);

        state.select_prev(); // Already at top
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn test_expand_collapse() {
        let mut state = TaskSidebarState::new();
        let tf = sample_tasks_file();
        state.refresh(&tf);

        // Initially collapsed: 2 rows
        assert_eq!(state.visible_rows().len(), 2);

        // Expand first epic
        state.toggle_expand();
        assert_eq!(state.visible_rows().len(), 4); // "1", "1.1", "1.2", "2"

        // Collapse it back
        state.toggle_expand();
        assert_eq!(state.visible_rows().len(), 2);
    }

    #[test]
    fn test_expand_leaf_is_noop() {
        let mut state = TaskSidebarState::new();
        let tf = sample_tasks_file();
        state.refresh(&tf);

        // Expand epic "1" first
        state.toggle_expand();
        assert_eq!(state.visible_rows().len(), 4);

        // Select leaf "1.1" (index 1) and try to expand — should be noop
        state.selected_index = 1;
        state.toggle_expand();
        assert_eq!(state.visible_rows().len(), 4); // unchanged
    }

    #[test]
    fn test_snapshot_collapsed_tree() {
        let mut state = TaskSidebarState::new();
        let tf = sample_tasks_file();
        state.refresh(&tf);

        let buffer = render_sidebar(&mut state, 35, 8);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_expanded_tree() {
        let mut state = TaskSidebarState::new();
        let tf = sample_tasks_file();
        state.refresh(&tf);

        // Expand both epics
        state.toggle_expand(); // expand "1"
        state.selected_index = 3; // select "2"
        state.toggle_expand(); // expand "2"
        state.selected_index = 0; // back to top

        let buffer = render_sidebar(&mut state, 45, 12);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_with_selection() {
        let mut state = TaskSidebarState::new();
        let tf = sample_tasks_file();
        state.refresh(&tf);

        // Expand epic "1", select "1.2"
        state.toggle_expand();
        state.selected_index = 2; // "1.2" (in_progress)

        let buffer = render_sidebar(&mut state, 40, 8);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_hidden_sidebar() {
        let mut state = TaskSidebarState::new();
        state.visible = false;

        let buffer = render_sidebar(&mut state, 30, 8);
        // Should be empty — nothing rendered
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_empty_tasks() {
        let mut state = TaskSidebarState::new();
        let tf: TasksFile = serde_yaml::from_str("tasks: []").unwrap();
        state.refresh(&tf);

        let buffer = render_sidebar(&mut state, 30, 6);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_scroll_offset_adjusts() {
        let mut state = TaskSidebarState::new();
        let tf = sample_tasks_file();
        state.refresh(&tf);

        // Expand both epics for 6 rows
        state.toggle_expand();
        state.selected_index = 3;
        state.toggle_expand();
        state.selected_index = 0;

        // Viewport of height 3 (inner area = 1 due to borders = too small for 6 rows)
        // Use height 5 for inner=3
        state.selected_index = 5; // last row
        state.adjust_scroll(3);
        assert_eq!(state.scroll_offset, 3); // 5 - 3 + 1 = 3

        state.selected_index = 1;
        state.adjust_scroll(3);
        assert_eq!(state.scroll_offset, 1);
    }

    #[test]
    fn test_refresh_clamps_selection() {
        let mut state = TaskSidebarState::new();
        let tf = sample_tasks_file();
        state.refresh(&tf);

        // Expand to get more rows
        state.toggle_expand();
        state.selected_index = 3; // select "2"

        // Refresh with fewer tasks
        let tf2: TasksFile = serde_yaml::from_str(
            r#"
tasks:
  - id: "1"
    name: "Only"
    status: done
"#,
        )
        .unwrap();
        state.refresh(&tf2);

        // Selection should be clamped to last visible row (0)
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn test_flat_row_status_icons() {
        assert_eq!(status_icon(Some(&TaskStatus::Done)), "✓ ");
        assert_eq!(status_icon(Some(&TaskStatus::InProgress)), "~ ");
        assert_eq!(status_icon(Some(&TaskStatus::Todo)), "○ ");
        assert_eq!(status_icon(Some(&TaskStatus::Blocked)), "! ");
        assert_eq!(status_icon(None), "  ");
    }

    #[test]
    fn test_visible_rows_deep_nesting() {
        let yaml = r#"
tasks:
  - id: "1"
    name: "Root"
    subtasks:
      - id: "1.1"
        name: "Child"
        subtasks:
          - id: "1.1.1"
            name: "Grandchild"
            status: todo
"#;
        let tf: TasksFile = serde_yaml::from_str(yaml).unwrap();
        let mut state = TaskSidebarState::new();
        state.refresh(&tf);

        // All collapsed: only root visible
        assert_eq!(state.visible_rows().len(), 1);

        // Expand root
        state.toggle_expand();
        assert_eq!(state.visible_rows().len(), 2); // "1", "1.1"

        // Expand child
        state.selected_index = 1;
        state.toggle_expand();
        assert_eq!(state.visible_rows().len(), 3); // "1", "1.1", "1.1.1"

        // Verify depths
        let rows = state.visible_rows();
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].depth, 2);
    }

    #[test]
    fn test_set_current_task() {
        let mut state = TaskSidebarState::new();
        let tf = sample_tasks_file();
        state.refresh(&tf);

        // Initially no current task
        assert_eq!(state.current_task_id, None);

        // Set current task
        state.set_current_task(Some("1.1".to_string()));
        assert_eq!(state.current_task_id, Some("1.1".to_string()));

        // Clear current task
        state.set_current_task(None);
        assert_eq!(state.current_task_id, None);
    }

    #[test]
    fn test_snapshot_with_current_task_highlight() {
        let mut state = TaskSidebarState::new();
        let tf = sample_tasks_file();
        state.refresh(&tf);

        // Expand epic "1", set current task to "1.2"
        state.toggle_expand();
        state.set_current_task(Some("1.2".to_string()));
        state.selected_index = 0; // select "1" (different from current)

        let buffer = render_sidebar(&mut state, 40, 8);
        insta::assert_snapshot!(snap(&buffer));
    }
}
