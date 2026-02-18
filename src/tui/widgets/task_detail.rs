use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph, Widget},
};

use crate::shared::tasks::TaskNode;
use crate::tui::theme::DEFAULT_THEME;

// ── Constants ──────────────────────────────────────────────────────

#[allow(dead_code)] // TUI component — will be used when full TUI is integrated
const MIN_HEIGHT: u16 = 5;

// ── State ──────────────────────────────────────────────────────────

/// Mutable state for the task detail panel.
///
/// Tracks scroll offset for long content. Separated from rendering widget
/// to follow ratatui's state/widget pattern.
#[derive(Debug, Clone, Default)]
pub struct TaskDetailState {
    /// Vertical scroll offset (number of lines scrolled down)
    pub scroll_offset: usize,
}

impl TaskDetailState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scroll down by one line.
    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    /// Scroll up by one line.
    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Reset scroll to top.
    pub fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
    }

    /// Clamp scroll offset to valid range based on content height and viewport.
    fn clamp_scroll(&mut self, content_height: usize, viewport_height: usize) {
        if content_height <= viewport_height {
            self.scroll_offset = 0;
        } else {
            let max_offset = content_height.saturating_sub(viewport_height);
            self.scroll_offset = self.scroll_offset.min(max_offset);
        }
    }
}

// ── Widget ─────────────────────────────────────────────────────────

/// Rendering widget for the task detail panel.
///
/// Displays comprehensive information about a selected task:
/// - ID and Name (bold header)
/// - Status (colored)
/// - Component
/// - Model
/// - Dependencies (with arrows)
/// - Related files
/// - Implementation steps (numbered)
/// - Description (wrapped text)
///
/// Content is scrollable via `TaskDetailState.scroll_offset`.
pub struct TaskDetail<'a> {
    /// Reference to the task node to display
    task: Option<&'a TaskNode>,
    /// Mutable state for scrolling
    state: &'a mut TaskDetailState,
    /// Whether the panel has focus
    focused: bool,
}

impl<'a> TaskDetail<'a> {
    pub fn new(task: Option<&'a TaskNode>, state: &'a mut TaskDetailState, focused: bool) -> Self {
        Self {
            task,
            state,
            focused,
        }
    }

    /// Render the detail panel into a buffer area.
    pub fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 3 || area.height < MIN_HEIGHT {
            return;
        }

        let theme = &DEFAULT_THEME;

        let block = Block::default()
            .padding(Padding::uniform(1))
            .style(Style::default().bg(theme.panel_bg(self.focused)))
            .title(Span::styled(" Task Details ", theme.header_style()));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // Build content lines
        let lines = if let Some(task) = self.task {
            self.build_content_lines(task, inner.width)
        } else {
            vec![Line::from(Span::styled(
                "No task selected",
                theme.muted_style(),
            ))]
        };

        let content_height = lines.len();
        let viewport_height = inner.height as usize;

        // Clamp scroll offset
        self.state.clamp_scroll(content_height, viewport_height);

        // Render visible slice
        let visible_lines: Vec<Line> = lines
            .into_iter()
            .skip(self.state.scroll_offset)
            .take(viewport_height)
            .collect();

        // No Wrap here — lines are already pre-wrapped by build_content_lines(),
        // adding Wrap would cause double-wrapping and break scroll offset calculation.
        let paragraph = Paragraph::new(visible_lines);
        paragraph.render(inner, buf);
    }

    /// Build all content lines for the task detail view.
    fn build_content_lines(&self, task: &TaskNode, width: u16) -> Vec<Line<'static>> {
        let theme = &DEFAULT_THEME;
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Header: ID + Name (bold)
        let header = format!("{} — {}", task.id, task.name);
        lines.push(Line::from(Span::styled(
            header,
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));

        // Status (colored)
        if let Some(ref status) = task.status {
            let status_text = format!("{:?}", status);
            let status_color = theme.state_color(status);
            lines.push(Line::from(vec![
                Span::styled("Status: ", theme.muted_style()),
                Span::styled(status_text, Style::default().fg(status_color)),
            ]));
        }

        // Component
        if let Some(ref component) = task.component {
            lines.push(Line::from(vec![
                Span::styled("Component: ", theme.muted_style()),
                Span::raw(component.clone()),
            ]));
        }

        // Model
        if let Some(ref model) = task.model {
            lines.push(Line::from(vec![
                Span::styled("Model: ", theme.muted_style()),
                Span::raw(model.clone()),
            ]));
        }

        // Dependencies
        if !task.deps.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Dependencies:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            for dep in &task.deps {
                lines.push(Line::from(vec![
                    Span::raw("  → "),
                    Span::styled(dep.clone(), theme.primary),
                ]));
            }
        }

        // Related files
        if !task.related_files.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Related Files:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            for file in &task.related_files {
                lines.push(Line::from(vec![
                    Span::raw("  • "),
                    Span::styled(file.clone(), theme.muted_style()),
                ]));
            }
        }

        // Implementation steps
        if !task.implementation_steps.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Implementation Steps:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            for (i, step) in task.implementation_steps.iter().enumerate() {
                // Wrap long steps across multiple lines
                let wrapped = wrap_text(step, width.saturating_sub(6) as usize);
                for (j, line_text) in wrapped.iter().enumerate() {
                    if j == 0 {
                        // First line: show number
                        lines.push(Line::from(vec![
                            Span::raw(format!("  {}. ", i + 1)),
                            Span::raw(line_text.clone()),
                        ]));
                    } else {
                        // Continuation lines: indent
                        lines.push(Line::from(vec![
                            Span::raw("     "),
                            Span::raw(line_text.clone()),
                        ]));
                    }
                }
            }
        }

        // Description
        if let Some(ref desc) = task.description {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Description:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            // Wrap description text
            let wrapped = wrap_text(desc, width.saturating_sub(2) as usize);
            for line_text in wrapped {
                lines.push(Line::from(Span::raw(line_text)));
            }
        }

        lines
    }
}

// ── Helpers ────────────────────────────────────────────────────────

/// Simple text wrapping: split text into lines that fit within the given width.
/// Preserves words (splits on whitespace).
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_len = 0;

    for word in text.split_whitespace() {
        let word_len = word.len();
        if current_len == 0 {
            // First word on the line
            current_line = word.to_string();
            current_len = word_len;
        } else if current_len + 1 + word_len <= max_width {
            // Fits on current line
            current_line.push(' ');
            current_line.push_str(word);
            current_len += 1 + word_len;
        } else {
            // Start a new line
            lines.push(current_line);
            current_line = word.to_string();
            current_len = word_len;
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::progress::TaskStatus;
    use crate::test_helpers::snap;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn sample_task() -> TaskNode {
        TaskNode {
            id: "4.3".to_string(),
            name: "Detail panel widget".to_string(),
            component: Some("tui".to_string()),
            status: Some(TaskStatus::InProgress),
            deps: vec!["1.1".to_string()],
            model: Some("sonnet".to_string()),
            description: Some("Panel z detalami wybranego taska: description, status, model, deps, related_files, implementation_steps. Scrollable.".to_string()),
            related_files: vec![
                "src/tui/widgets/task_detail.rs".to_string(),
                "src/shared/tasks/node.rs".to_string(),
            ],
            implementation_steps: vec![
                "Utwórz src/tui/widgets/task_detail.rs".to_string(),
                "Zdefiniuj TaskDetailWidget z ref do TaskNode".to_string(),
                "Renderuj: ID + Name (bold), Status (colored), Model, Component".to_string(),
            ],
            profiles: vec![],
            subtasks: vec![],
        }
    }

    fn minimal_task() -> TaskNode {
        TaskNode {
            id: "1.1".to_string(),
            name: "Minimal task".to_string(),
            component: None,
            status: Some(TaskStatus::Todo),
            deps: vec![],
            model: None,
            description: None,
            related_files: vec![],
            implementation_steps: vec![],
            profiles: vec![],
            subtasks: vec![],
        }
    }

    /// Helper: renders TaskDetail into a buffer.
    fn render_detail(
        task: Option<&TaskNode>,
        state: &mut TaskDetailState,
        width: u16,
        height: u16,
    ) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("Failed to create test terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, height);
                let detail = TaskDetail::new(task, state, true);
                detail.render(area, frame.buffer_mut());
            })
            .expect("Failed to draw");
        terminal.backend().buffer().clone()
    }

    #[test]
    fn test_state_scroll_down() {
        let mut state = TaskDetailState::new();
        assert_eq!(state.scroll_offset, 0);
        state.scroll_down();
        assert_eq!(state.scroll_offset, 1);
        state.scroll_down();
        assert_eq!(state.scroll_offset, 2);
    }

    #[test]
    fn test_state_scroll_up() {
        let mut state = TaskDetailState::new();
        state.scroll_offset = 5;
        state.scroll_up();
        assert_eq!(state.scroll_offset, 4);
        state.scroll_up();
        assert_eq!(state.scroll_offset, 3);
    }

    #[test]
    fn test_state_scroll_up_clamps_at_zero() {
        let mut state = TaskDetailState::new();
        state.scroll_offset = 1;
        state.scroll_up();
        assert_eq!(state.scroll_offset, 0);
        state.scroll_up();
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_state_reset_scroll() {
        let mut state = TaskDetailState::new();
        state.scroll_offset = 10;
        state.reset_scroll();
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_wrap_text_single_line() {
        let result = wrap_text("Hello world", 20);
        assert_eq!(result, vec!["Hello world"]);
    }

    #[test]
    fn test_wrap_text_multiple_lines() {
        let result = wrap_text("This is a long line that should wrap", 15);
        assert_eq!(result.len(), 3);
        assert!(result[0].len() <= 15);
        assert!(result[1].len() <= 15);
    }

    #[test]
    fn test_wrap_text_empty() {
        let result = wrap_text("", 10);
        assert_eq!(result, vec![""]);
    }

    #[test]
    fn test_wrap_text_single_long_word() {
        let result = wrap_text("Supercalifragilisticexpialidocious", 10);
        // Word doesn't fit → goes on its own line
        assert_eq!(result, vec!["Supercalifragilisticexpialidocious"]);
    }

    #[test]
    fn test_snapshot_no_task_selected() {
        let mut state = TaskDetailState::new();
        let buffer = render_detail(None, &mut state, 40, 8);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_full_task() {
        let task = sample_task();
        let mut state = TaskDetailState::new();
        let buffer = render_detail(Some(&task), &mut state, 60, 30);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_minimal_task() {
        let task = minimal_task();
        let mut state = TaskDetailState::new();
        let buffer = render_detail(Some(&task), &mut state, 40, 10);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_scrolled_content() {
        let task = sample_task();
        let mut state = TaskDetailState::new();
        state.scroll_offset = 5;
        let buffer = render_detail(Some(&task), &mut state, 60, 15);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_narrow_width() {
        let task = sample_task();
        let mut state = TaskDetailState::new();
        let buffer = render_detail(Some(&task), &mut state, 35, 25);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_clamp_scroll_content_fits() {
        let mut state = TaskDetailState::new();
        state.scroll_offset = 5;
        state.clamp_scroll(10, 15); // content 10 lines, viewport 15 lines
        assert_eq!(state.scroll_offset, 0); // reset to 0 when content fits
    }

    #[test]
    fn test_clamp_scroll_content_exceeds() {
        let mut state = TaskDetailState::new();
        state.scroll_offset = 20;
        state.clamp_scroll(30, 10); // content 30 lines, viewport 10 lines
        // max_offset = 30 - 10 = 20
        assert_eq!(state.scroll_offset, 20);
    }

    #[test]
    fn test_clamp_scroll_exceeds_max() {
        let mut state = TaskDetailState::new();
        state.scroll_offset = 100;
        state.clamp_scroll(25, 10); // content 25, viewport 10
        // max_offset = 25 - 10 = 15
        assert_eq!(state.scroll_offset, 15);
    }
}
