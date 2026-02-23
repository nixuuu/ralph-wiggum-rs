//! TaskCommandApp - shared state for all task commands
//!
//! Provides:
//! - Output ring buffer for scrollable command output
//! - Status bar with command name and model info
//! - Header with command metadata
//! - Optional ask_user widget for inline user interaction
//!
//! Simplified version of RunApp (from commands/run/ui.rs) without:
//! - Iteration tracking
//! - Promise detection
//! - Worker panel/sidebar
//! - Pause functionality

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Widget};

use tokio::sync::oneshot;

use crate::commands::mcp::ask_user::{Answer, Question, QuestionType};
use crate::tui::app::AppState;
use crate::tui::events::{AppEvent, EventResult};
use crate::tui::ring_buffer::OutputRingBuffer;
use crate::tui::theme::DEFAULT_THEME;
use crate::tui::widgets::ask_user::{AskUserAction, AskUserQuestion, AskUserWidget, QuestionKind};
use crate::tui::widgets::ask_user_choice::QuestionOption as TuiQuestionOption;
use crate::tui::widgets::{OutputView, OutputViewState};

// ── TaskHeaderData ──────────────────────────────────────────────────
// Named TaskHeaderData to avoid collision with tui::widgets::header::HeaderData
// which has different fields (iteration, elapsed, is_running).

/// Data displayed in the header line of task commands.
///
/// Simplified header — shows only command name and optional model.
#[derive(Debug, Clone)]
pub struct TaskHeaderData {
    /// Command name (e.g., "task prd", "task plan")
    pub command_name: String,
    /// Model name (e.g., "claude-sonnet-4-5")
    pub model: Option<String>,
}

impl TaskHeaderData {
    pub fn new(command_name: impl Into<String>) -> Self {
        Self {
            command_name: command_name.into(),
            model: None,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Format header as a styled Line
    fn to_line(&self) -> Line<'static> {
        let mut spans = vec![
            Span::styled(
                "ralph ".to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(self.command_name.clone(), Style::default().fg(Color::White)),
        ];

        if let Some(ref model) = self.model {
            spans.push(Span::raw(" │ ".to_string()));
            spans.push(Span::styled(
                format!("model: {}", model),
                Style::default().fg(Color::DarkGray),
            ));
        }

        Line::from(spans)
    }
}

// ── StatusData ──────────────────────────────────────────────────────

/// Data displayed in the status bar.
///
/// Simplified status — shows message and optional progress percentage.
/// For full metrics (tokens, cost, time) use `tui::widgets::StatusBarData`.
#[derive(Debug, Clone, Default)]
pub struct StatusData {
    /// Status message (e.g., "Generating PRD...", "Ready")
    pub message: String,
    /// Optional progress indicator (0.0 - 1.0)
    pub progress: Option<f32>,
}

impl StatusData {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            progress: None,
        }
    }

    /// Set progress (clamped to 0.0 - 1.0)
    pub fn with_progress(mut self, progress: f32) -> Self {
        self.progress = Some(progress.clamp(0.0, 1.0));
        self
    }

    /// Format status as a styled Line
    fn to_line(&self) -> Line<'static> {
        let mut spans = vec![Span::styled(
            self.message.clone(),
            Style::default().fg(Color::Yellow),
        )];

        if let Some(progress) = self.progress {
            let pct = (progress * 100.0) as u32;
            spans.push(Span::raw(" │ ".to_string()));
            spans.push(Span::styled(
                format!("{}%", pct),
                Style::default().fg(Color::Cyan),
            ));
        }

        Line::from(spans)
    }
}

// ── QuestionTracker ──────────────────────────────────────────────────

/// Tracks multi-question ask_user session.
///
/// Holds MCP questions, TUI-converted equivalents, collected answers,
/// and a oneshot channel to send answers back to the question handler.
pub struct QuestionTracker {
    /// Oryginalne pytania MCP (do budowania Answer z question text)
    questions: Vec<Question>,
    /// Skonwertowane pytania TUI (do budowania AskUserWidget)
    tui_questions: Vec<AskUserQuestion>,
    /// Zebrane odpowiedzi
    answers: Vec<Answer>,
    /// Aktualny indeks pytania
    current_index: usize,
    /// Kanał do odesłania odpowiedzi do question handler
    response_tx: Option<oneshot::Sender<Vec<Answer>>>,
}

impl QuestionTracker {
    /// Advance to next question. Returns true if there are more questions.
    fn advance(&mut self, answer_text: String) -> bool {
        if self.current_index < self.questions.len() {
            self.answers.push(Answer {
                question: self.questions[self.current_index].question.clone(),
                answer: answer_text,
            });
            self.current_index += 1;
        }
        self.current_index < self.questions.len()
    }

    /// Send collected answers through oneshot channel and consume tracker.
    fn finish(&mut self) {
        if let Some(tx) = self.response_tx.take() {
            let _ = tx.send(self.answers.clone());
        }
    }

    /// Cancel: send empty answers through oneshot channel.
    fn cancel(&mut self) {
        if let Some(tx) = self.response_tx.take() {
            let _ = tx.send(Vec::new());
        }
    }
}

// ── MCP → TUI conversion ───────────────────────────────────────────

/// Konwertuje MCP Question na TUI AskUserQuestion.
fn mcp_to_tui_question(q: &Question) -> AskUserQuestion {
    let kind = match q.question_type {
        QuestionType::Text => QuestionKind::Text,
        QuestionType::Choice => QuestionKind::Choice,
        QuestionType::MultiChoice => QuestionKind::MultiChoice,
        QuestionType::Confirm => QuestionKind::Confirm,
    };

    let options: Vec<TuiQuestionOption> = q
        .options
        .iter()
        .map(|o| TuiQuestionOption {
            label: o.label.clone(),
            description: o.description.clone(),
        })
        .collect();

    AskUserQuestion {
        question: q.question.clone(),
        kind,
        options,
        default: q.default.clone(),
        placeholder: q.placeholder.clone(),
    }
}

// ── TaskCommandApp ──────────────────────────────────────────────────

/// Shared state for task commands with TUI output.
///
/// Layout:
/// ```text
/// ┌──────────────────────────────────────┐
/// │ ralph task prd │ model: sonnet-4-5   │ ← Header (1 line)
/// ├──────────────────────────────────────┤
/// │                                      │
/// │  Output ring buffer (scrollable)     │ ← OutputView + OutputViewState
/// │                                      │
/// ├──────────────────────────────────────┤
/// │ [optional ask_user widget]           │ ← Inline widget (5 lines or 0)
/// ├──────────────────────────────────────┤
/// │ Generating PRD... │ 45%              │ ← Status bar (1 line)
/// └──────────────────────────────────────┘
/// ```
pub struct TaskCommandApp {
    /// Output ring buffer (scrollable)
    pub ring_buffer: OutputRingBuffer,
    /// Header data (command name, model)
    pub header_data: TaskHeaderData,
    /// Status bar data (message, progress)
    pub status_data: StatusData,
    /// Aktywny widget ask_user (ratatui widget z interakcją)
    active_widget: Option<AskUserWidget>,
    /// Tracker postępu pytań (które pytanie, zebrane odpowiedzi, oneshot tx)
    question_tracker: Option<QuestionTracker>,
    /// Scroll state — delegates to OutputViewState for auto_follow and clamping
    output_state: OutputViewState,
    /// Whether the Claude runner has completed (TUI stays open for browsing)
    runner_completed: bool,
    /// Viewport height dostępny dla ask_user widget (obliczony przy draw, 0 = nieznany)
    ask_user_viewport_height: u16,
    /// Szerokość terminala (obliczona przy draw, 80 = domyślna)
    last_known_width: u16,
}

impl TaskCommandApp {
    pub fn new(command_name: impl Into<String>) -> Self {
        Self {
            ring_buffer: OutputRingBuffer::new(),
            header_data: TaskHeaderData::new(command_name),
            status_data: StatusData::default(),
            active_widget: None,
            question_tracker: None,
            output_state: OutputViewState::default(),
            runner_completed: false,
            ask_user_viewport_height: 0,
            last_known_width: 80,
        }
    }

    /// Set model name in header
    pub fn set_model(&mut self, model: impl Into<String>) {
        self.header_data.model = Some(model.into());
    }

    /// Set status message
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_data.message = message.into();
    }

    /// Set status with progress (clamped to 0.0 - 1.0)
    pub fn set_status_with_progress(&mut self, message: impl Into<String>, progress: f32) {
        self.status_data.message = message.into();
        self.status_data.progress = Some(progress.clamp(0.0, 1.0));
    }

    /// Mark runner as completed — TUI stays open for browsing output.
    /// Updates status bar to show "Done" message.
    pub fn set_runner_completed(&mut self) {
        self.runner_completed = true;
        self.status_data.message = "Done — press q to exit".to_string();
        self.status_data.progress = None;
    }

    /// Check if runner has completed
    pub fn is_runner_completed(&self) -> bool {
        self.runner_completed
    }

    /// Push formatted Lines to ring buffer (from OutputFormatter)
    pub fn push_event(&mut self, lines: Vec<Line<'static>>) {
        for line in lines {
            self.ring_buffer.push(line);
        }
    }

    /// Push plain text to ring buffer (converts ANSI if present)
    pub fn push_text(&mut self, text: &str) {
        self.ring_buffer.push_str(text);
    }

    /// Start interactive ask_user session with MCP questions.
    ///
    /// Creates AskUserWidget for the first question and sets up QuestionTracker
    /// with oneshot channel to deliver answers back to the question handler.
    pub fn start_ask_user(
        &mut self,
        questions: Vec<Question>,
        response_tx: oneshot::Sender<Vec<Answer>>,
    ) {
        let tui_questions: Vec<AskUserQuestion> =
            questions.iter().map(mcp_to_tui_question).collect();

        let first_widget = AskUserWidget::new(tui_questions[0].clone());
        self.active_widget = Some(first_widget);

        self.question_tracker = Some(QuestionTracker {
            questions,
            tui_questions,
            answers: Vec::new(),
            current_index: 0,
            response_tx: Some(response_tx),
        });
    }

    /// Check if ask_user widget is active (waiting for user input)
    pub fn has_active_question(&self) -> bool {
        self.active_widget.is_some()
    }

    /// Advance to the next question after user submits an answer.
    /// If all questions answered, sends answers through the oneshot channel.
    pub(crate) fn advance_question(&mut self, answer_text: String) {
        let tracker = match self.question_tracker.as_mut() {
            Some(t) => t,
            None => return,
        };

        let has_more = tracker.advance(answer_text);

        if has_more {
            // Create widget for next question (reset scroll to top)
            let next_question = tracker.tui_questions[tracker.current_index].clone();
            let mut next_widget = AskUserWidget::new(next_question);
            next_widget.reset_scroll();
            self.active_widget = Some(next_widget);
        } else {
            // All questions answered — send answers and cleanup
            tracker.finish();
            self.active_widget = None;
            self.question_tracker = None;
        }
    }

    /// Cancel ask_user session (Esc pressed).
    /// Sends empty answers through the oneshot channel.
    fn cancel_ask_user(&mut self) {
        if let Some(ref mut tracker) = self.question_tracker {
            tracker.cancel();
        }
        self.active_widget = None;
        self.question_tracker = None;
    }

    /// Get current scroll offset (visual rows from bottom, 0 = at bottom)
    pub fn scroll_offset(&self) -> usize {
        self.output_state.scroll_offset
    }

    /// Scroll up by delta visual rows
    pub fn scroll_up(&mut self, delta: usize) {
        self.output_state.scroll_up(delta);
    }

    /// Scroll down by delta visual rows
    pub fn scroll_down(&mut self, delta: usize) {
        self.output_state.scroll_down(delta);
    }

    /// Scroll to bottom (re-enables auto_follow)
    pub fn scroll_to_bottom(&mut self) {
        self.output_state.scroll_end();
    }
}

// ── AppState Implementation ─────────────────────────────────────────

impl AppState for TaskCommandApp {
    fn draw(&mut self, frame: &mut Frame, area: Rect) {
        // Max wysokość dla ask_user widget: 75% terminala, min 5 wierszy, max area.height-3
        // Zapisujemy dla handle_event() (scroll keys)
        let max_ask_height = ((area.height * 3) / 4)
            .max(5)
            .min(area.height.saturating_sub(3));
        self.ask_user_viewport_height = max_ask_height;
        self.last_known_width = area.width;

        // Calculate dynamic ask_user height from AskUserWidget (width-aware wrapping, capped)
        let ask_user_height = self
            .active_widget
            .as_ref()
            .map(|w| w.height_for_width_capped(area.width, max_ask_height))
            .unwrap_or(0);

        let constraints = vec![
            Constraint::Length(1),               // Header
            Constraint::Min(1),                  // Output
            Constraint::Length(ask_user_height), // Ask user widget (0 if hidden)
            Constraint::Length(1),               // Status bar
        ];

        let chunks = Layout::vertical(constraints).split(area);

        // ── Header (1 line: command name + model) ──
        let header_line = self.header_data.to_line();
        frame.render_widget(Paragraph::new(header_line), chunks[0]);

        // ── Output (delegated to OutputView StatefulWidget, fill remaining space) ──
        // Rezerwujemy 1 kolumnę po prawej dla scrollbara — tekst nie wchodzi pod scrollbar
        let output_content_area = Rect {
            width: chunks[1].width.saturating_sub(1),
            ..chunks[1]
        };
        let output_view = OutputView::new(&self.ring_buffer).dimmed(self.active_widget.is_some());
        frame.render_stateful_widget(output_view, output_content_area, &mut self.output_state);

        // ── Output scrollbar (VerticalRight, widoczny tylko gdy content > viewport) ──
        {
            let total_visual = self
                .ring_buffer
                .total_visual_rows(output_content_area.width);
            let viewport_h = output_content_area.height as usize;
            if total_visual > viewport_h {
                let max_scroll = total_visual.saturating_sub(viewport_h);
                let pos = max_scroll.saturating_sub(self.output_state.scroll_offset);
                let mut sb = ScrollbarState::default()
                    .content_length(max_scroll + 1)
                    .viewport_content_length(viewport_h)
                    .position(pos);
                frame.render_stateful_widget(
                    Scrollbar::new(ScrollbarOrientation::VerticalRight)
                        .thumb_symbol("▐")
                        .track_symbol(Some("▐"))
                        .thumb_style(Style::default().fg(DEFAULT_THEME.secondary))
                        .track_style(Style::default().fg(DEFAULT_THEME.border_normal))
                        .begin_symbol(None)
                        .end_symbol(None),
                    chunks[1],
                    &mut sb,
                );
            }
        }

        // ── Ask user widget (if active, rendered via AskUserWidget::render) ──
        if let Some(ref widget) = self.active_widget {
            let widget_clone = widget.clone();
            widget_clone.render(chunks[2], frame.buffer_mut());
        }

        // ── Status bar (1 line: tokens, cost, elapsed) ──
        let status_line = self.status_data.to_line();
        frame.render_widget(Paragraph::new(status_line), chunks[3]);
    }

    fn handle_event(&mut self, event: AppEvent) -> EventResult {
        // Obsługa scroll myszki — priorytet przed key handling
        if let AppEvent::Mouse(mouse) = event {
            use crossterm::event::MouseEventKind;
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if self.active_widget.is_some() {
                        if let Some(w) = self.active_widget.as_mut() {
                            w.scroll_up(3);
                        }
                    } else {
                        self.output_state.scroll_up(3);
                    }
                    return EventResult::Consumed;
                }
                MouseEventKind::ScrollDown => {
                    if self.active_widget.is_some() {
                        let max_h = self.ask_user_viewport_height;
                        let width = self.last_known_width;
                        if let Some(w) = self.active_widget.as_mut() {
                            w.scroll_down(3, width, max_h);
                        }
                    } else {
                        self.output_state.scroll_down(3);
                    }
                    return EventResult::Consumed;
                }
                _ => return EventResult::Ignored,
            }
        }

        // When ask_user widget is active, intercept keys for it
        if self.active_widget.is_some()
            && let AppEvent::Key(key) = event
        {
            let max_h = self.ask_user_viewport_height;
            let width = self.last_known_width;
            // page = viewport - 2 (żeby zachować kontekst), min 1
            let page = max_h.saturating_sub(2).max(1);

            // Obsłuż scroll keys PRZED delegacją do widgetu
            match key.code {
                crossterm::event::KeyCode::PageUp => {
                    if let Some(w) = self.active_widget.as_mut() {
                        w.scroll_up(page);
                    }
                    return EventResult::Consumed;
                }
                crossterm::event::KeyCode::PageDown => {
                    if let Some(w) = self.active_widget.as_mut() {
                        w.scroll_down(page, width, max_h);
                    }
                    return EventResult::Consumed;
                }
                _ => {}
            }

            // Przekazujemy cały KeyEvent (z modyfikatorami) — konieczne dla Shift+Enter
            let action = self.active_widget.as_mut().unwrap().handle_key(key);
            match action {
                AskUserAction::Submit(answer) => {
                    self.advance_question(answer);
                    return EventResult::Consumed;
                }
                AskUserAction::Cancel => {
                    self.cancel_ask_user();
                    return EventResult::Consumed;
                }
                AskUserAction::Continue => return EventResult::Consumed,
            }
        }

        // Standard key handling (scroll, quit, etc.)
        match event {
            AppEvent::Key(key) if key.code == crossterm::event::KeyCode::Up => {
                self.output_state.scroll_up(1);
                EventResult::Consumed
            }
            AppEvent::Key(key) if key.code == crossterm::event::KeyCode::Down => {
                self.output_state.scroll_down(1);
                EventResult::Consumed
            }
            AppEvent::Key(key) if key.code == crossterm::event::KeyCode::PageUp => {
                self.output_state.scroll_up(10);
                EventResult::Consumed
            }
            AppEvent::Key(key) if key.code == crossterm::event::KeyCode::PageDown => {
                self.output_state.scroll_down(10);
                EventResult::Consumed
            }
            AppEvent::Key(key) if key.code == crossterm::event::KeyCode::Home => {
                let total = self.ring_buffer.total_visual_rows(80);
                self.output_state.scroll_home(total, 20);
                EventResult::Consumed
            }
            AppEvent::Key(key) if key.code == crossterm::event::KeyCode::End => {
                self.output_state.scroll_end();
                EventResult::Consumed
            }
            // q/Ctrl+C: defense-in-depth (App::run_shared handles these with higher priority)
            AppEvent::Key(key)
                if key.code == crossterm::event::KeyCode::Char('q') && key.modifiers.is_empty() =>
            {
                EventResult::Quit
            }
            AppEvent::Key(key)
                if key.code == crossterm::event::KeyCode::Char('c')
                    && key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                EventResult::Shutdown
            }
            _ => EventResult::Ignored,
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::mcp::ask_user::QuestionOption;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    // ── TaskHeaderData tests ──

    #[test]
    fn test_header_data_new() {
        let header = TaskHeaderData::new("task prd");
        assert_eq!(header.command_name, "task prd");
        assert!(header.model.is_none());
    }

    #[test]
    fn test_header_data_with_model() {
        let header = TaskHeaderData::new("task plan").with_model("claude-sonnet-4-5");
        assert_eq!(header.command_name, "task plan");
        assert_eq!(header.model, Some("claude-sonnet-4-5".to_string()));
    }

    #[test]
    fn test_header_to_line_without_model() {
        let header = TaskHeaderData::new("task prd");
        let line = header.to_line();
        // "ralph " + "task prd" = 2 spans
        assert_eq!(line.spans.len(), 2);
    }

    #[test]
    fn test_header_to_line_with_model() {
        let header = TaskHeaderData::new("task prd").with_model("sonnet-4-5");
        let line = header.to_line();
        // "ralph " + "task prd" + " │ " + "model: sonnet-4-5" = 4 spans
        assert_eq!(line.spans.len(), 4);
    }

    // ── StatusData tests ──

    #[test]
    fn test_status_data_new() {
        let status = StatusData::new("Ready");
        assert_eq!(status.message, "Ready");
        assert!(status.progress.is_none());
    }

    #[test]
    fn test_status_data_with_progress() {
        let status = StatusData::new("Generating").with_progress(0.45);
        assert_eq!(status.message, "Generating");
        assert_eq!(status.progress, Some(0.45));
    }

    #[test]
    fn test_status_data_progress_clamped() {
        let status1 = StatusData::new("Test").with_progress(-0.5);
        assert_eq!(status1.progress, Some(0.0));

        let status2 = StatusData::new("Test").with_progress(1.5);
        assert_eq!(status2.progress, Some(1.0));
    }

    #[test]
    fn test_status_to_line_without_progress() {
        let status = StatusData::new("Processing");
        let line = status.to_line();
        // Only message span
        assert_eq!(line.spans.len(), 1);
    }

    #[test]
    fn test_status_to_line_with_progress() {
        let status = StatusData::new("Working").with_progress(0.75);
        let line = status.to_line();
        // message + " │ " + "75%" = 3 spans
        assert_eq!(line.spans.len(), 3);
    }

    // ── QuestionTracker tests ──

    fn make_text_question(text: &str) -> Question {
        Question {
            question: text.into(),
            question_type: QuestionType::Text,
            options: vec![],
            default: None,
            placeholder: None,
            required: true,
        }
    }

    #[test]
    fn test_question_tracker_advance() {
        let (tx, _rx) = oneshot::channel();
        let mut tracker = QuestionTracker {
            questions: vec![make_text_question("Q1?"), make_text_question("Q2?")],
            tui_questions: vec![
                mcp_to_tui_question(&make_text_question("Q1?")),
                mcp_to_tui_question(&make_text_question("Q2?")),
            ],
            answers: Vec::new(),
            current_index: 0,
            response_tx: Some(tx),
        };

        assert!(tracker.advance("A1".into()));
        assert_eq!(tracker.current_index, 1);
        assert_eq!(tracker.answers.len(), 1);

        assert!(!tracker.advance("A2".into()));
        assert_eq!(tracker.current_index, 2);
        assert_eq!(tracker.answers.len(), 2);
    }

    #[test]
    fn test_question_tracker_finish_sends_answers() {
        let (tx, rx) = oneshot::channel();
        let mut tracker = QuestionTracker {
            questions: vec![make_text_question("Q1?")],
            tui_questions: vec![mcp_to_tui_question(&make_text_question("Q1?"))],
            answers: vec![Answer {
                question: "Q1?".into(),
                answer: "A1".into(),
            }],
            current_index: 1,
            response_tx: Some(tx),
        };

        tracker.finish();
        let answers = rx.blocking_recv().unwrap();
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].answer, "A1");
    }

    #[test]
    fn test_question_tracker_cancel_sends_empty() {
        let (tx, rx) = oneshot::channel();
        let mut tracker = QuestionTracker {
            questions: vec![make_text_question("Q1?")],
            tui_questions: vec![mcp_to_tui_question(&make_text_question("Q1?"))],
            answers: Vec::new(),
            current_index: 0,
            response_tx: Some(tx),
        };

        tracker.cancel();
        let answers = rx.blocking_recv().unwrap();
        assert!(answers.is_empty());
    }

    // ── mcp_to_tui_question conversion tests ──

    #[test]
    fn test_mcp_to_tui_text_question() {
        let mcp_q = make_text_question("What is your name?");
        let tui_q = mcp_to_tui_question(&mcp_q);
        assert_eq!(tui_q.question, "What is your name?");
        assert_eq!(tui_q.kind, QuestionKind::Text);
        assert!(tui_q.options.is_empty());
    }

    #[test]
    fn test_mcp_to_tui_choice_question() {
        let mcp_q = Question {
            question: "Choose one".into(),
            question_type: QuestionType::Choice,
            options: vec![
                QuestionOption {
                    label: "A".into(),
                    description: Some("First".into()),
                },
                QuestionOption {
                    label: "B".into(),
                    description: None,
                },
            ],
            default: Some("A".into()),
            placeholder: None,
            required: true,
        };
        let tui_q = mcp_to_tui_question(&mcp_q);
        assert_eq!(tui_q.kind, QuestionKind::Choice);
        assert_eq!(tui_q.options.len(), 2);
        assert_eq!(tui_q.options[0].label, "A");
        assert_eq!(tui_q.options[0].description, Some("First".into()));
        assert_eq!(tui_q.default, Some("A".into()));
    }

    #[test]
    fn test_mcp_to_tui_confirm_question() {
        let mcp_q = Question {
            question: "Sure?".into(),
            question_type: QuestionType::Confirm,
            options: vec![],
            default: Some("yes".into()),
            placeholder: None,
            required: true,
        };
        let tui_q = mcp_to_tui_question(&mcp_q);
        assert_eq!(tui_q.kind, QuestionKind::Confirm);
        assert_eq!(tui_q.default, Some("yes".into()));
    }

    // ── TaskCommandApp tests ──

    #[test]
    fn test_task_command_app_new() {
        let app = TaskCommandApp::new("task prd");
        assert_eq!(app.header_data.command_name, "task prd");
        assert!(!app.has_active_question());
        assert_eq!(app.scroll_offset(), 0);
    }

    #[test]
    fn test_task_command_app_set_model() {
        let mut app = TaskCommandApp::new("task plan");
        app.set_model("claude-sonnet-4-5");
        assert_eq!(app.header_data.model, Some("claude-sonnet-4-5".to_string()));
    }

    #[test]
    fn test_task_command_app_set_status() {
        let mut app = TaskCommandApp::new("task add");
        app.set_status("Processing...");
        assert_eq!(app.status_data.message, "Processing...");
    }

    #[test]
    fn test_task_command_app_set_status_with_progress() {
        let mut app = TaskCommandApp::new("task edit");
        app.set_status_with_progress("Generating", 0.75);
        assert_eq!(app.status_data.message, "Generating");
        assert_eq!(app.status_data.progress, Some(0.75));
    }

    #[test]
    fn test_task_command_app_push_text() {
        let mut app = TaskCommandApp::new("task test");
        app.push_text("Hello world");
        assert!(!app.ring_buffer.tail(10).is_empty());
    }

    #[test]
    fn test_task_command_app_push_event() {
        let mut app = TaskCommandApp::new("task test");
        let lines = vec![
            Line::raw("line 1".to_string()),
            Line::raw("line 2".to_string()),
        ];
        app.push_event(lines);
        assert_eq!(app.ring_buffer.tail(10).len(), 2);
    }

    #[test]
    fn test_task_command_app_start_ask_user() {
        let mut app = TaskCommandApp::new("task test");
        let (tx, _rx) = oneshot::channel();

        app.start_ask_user(vec![make_text_question("Test?")], tx);
        assert!(app.has_active_question());
        assert!(app.active_widget.is_some());
        assert!(app.question_tracker.is_some());
    }

    #[test]
    fn test_task_command_app_cancel_ask_user() {
        let mut app = TaskCommandApp::new("task test");
        let (tx, rx) = oneshot::channel();

        app.start_ask_user(vec![make_text_question("Test?")], tx);
        app.cancel_ask_user();

        assert!(!app.has_active_question());
        // Should receive empty answers
        let answers = rx.blocking_recv().unwrap();
        assert!(answers.is_empty());
    }

    #[test]
    fn test_task_command_app_advance_question_single() {
        let mut app = TaskCommandApp::new("task test");
        let (tx, rx) = oneshot::channel();

        app.start_ask_user(vec![make_text_question("Name?")], tx);
        app.advance_question("Alice".into());

        // Single question — should be done
        assert!(!app.has_active_question());
        let answers = rx.blocking_recv().unwrap();
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].answer, "Alice");
    }

    #[test]
    fn test_task_command_app_advance_question_multi() {
        let mut app = TaskCommandApp::new("task test");
        let (tx, rx) = oneshot::channel();

        app.start_ask_user(
            vec![make_text_question("Q1?"), make_text_question("Q2?")],
            tx,
        );

        // Answer first question
        app.advance_question("A1".into());
        assert!(app.has_active_question()); // Still has Q2

        // Answer second question
        app.advance_question("A2".into());
        assert!(!app.has_active_question());

        let answers = rx.blocking_recv().unwrap();
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0].answer, "A1");
        assert_eq!(answers[1].answer, "A2");
    }

    #[test]
    fn test_task_command_app_scroll() {
        let mut app = TaskCommandApp::new("task test");

        app.scroll_up(5);
        assert_eq!(app.scroll_offset(), 5);

        app.scroll_down(2);
        assert_eq!(app.scroll_offset(), 3);

        app.scroll_to_bottom();
        assert_eq!(app.scroll_offset(), 0);
    }

    #[test]
    fn test_task_command_app_scroll_no_underflow() {
        let mut app = TaskCommandApp::new("task test");
        app.scroll_down(10);
        assert_eq!(app.scroll_offset(), 0);
    }

    #[test]
    fn test_auto_follow_re_enabled_on_scroll_to_bottom() {
        let mut app = TaskCommandApp::new("task test");
        app.scroll_up(5);
        assert!(!app.output_state.auto_follow);

        app.scroll_to_bottom();
        assert!(app.output_state.auto_follow);
    }

    #[test]
    fn test_scroll_down_re_enables_auto_follow_at_zero() {
        let mut app = TaskCommandApp::new("task test");
        app.scroll_up(2);
        assert!(!app.output_state.auto_follow);

        app.scroll_down(2);
        assert_eq!(app.scroll_offset(), 0);
        assert!(app.output_state.auto_follow);
    }

    // ── AppState handle_event tests ──

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> AppEvent {
        AppEvent::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    #[test]
    fn test_handle_event_arrow_up() {
        let mut app = TaskCommandApp::new("task test");
        let result = app.handle_event(make_key(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.scroll_offset(), 1);
    }

    #[test]
    fn test_handle_event_arrow_down() {
        let mut app = TaskCommandApp::new("task test");
        app.scroll_up(5);

        let result = app.handle_event(make_key(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.scroll_offset(), 4);
    }

    #[test]
    fn test_handle_event_page_up() {
        let mut app = TaskCommandApp::new("task test");
        let result = app.handle_event(make_key(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.scroll_offset(), 10);
    }

    #[test]
    fn test_handle_event_page_down() {
        let mut app = TaskCommandApp::new("task test");
        app.scroll_up(20);

        let result = app.handle_event(make_key(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.scroll_offset(), 10);
    }

    #[test]
    fn test_handle_event_home() {
        let mut app = TaskCommandApp::new("task test");
        // Push some content so home has somewhere to scroll
        for i in 0..50 {
            app.push_text(&format!("line {}", i));
        }

        let result = app.handle_event(make_key(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(result, EventResult::Consumed);
        // scroll_home sets offset based on total_visual_rows
        assert!(app.scroll_offset() > 0);
        assert!(!app.output_state.auto_follow);
    }

    #[test]
    fn test_handle_event_end() {
        let mut app = TaskCommandApp::new("task test");
        app.scroll_up(50);

        let result = app.handle_event(make_key(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.scroll_offset(), 0);
        assert!(app.output_state.auto_follow);
    }

    #[test]
    fn test_handle_event_quit() {
        let mut app = TaskCommandApp::new("task test");
        let result = app.handle_event(make_key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(result, EventResult::Quit);
    }

    #[test]
    fn test_handle_event_ctrl_c() {
        let mut app = TaskCommandApp::new("task test");
        let result = app.handle_event(make_key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(result, EventResult::Shutdown);
    }

    #[test]
    fn test_handle_event_ignored() {
        let mut app = TaskCommandApp::new("task test");
        let result = app.handle_event(make_key(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(result, EventResult::Ignored);
    }

    #[test]
    fn test_handle_event_tick_ignored() {
        let mut app = TaskCommandApp::new("task test");
        let result = app.handle_event(AppEvent::Tick);
        assert_eq!(result, EventResult::Ignored);
    }

    #[test]
    fn test_handle_event_resize_ignored() {
        let mut app = TaskCommandApp::new("task test");
        let result = app.handle_event(AppEvent::Resize(80, 24));
        assert_eq!(result, EventResult::Ignored);
    }

    // ── Layout validation tests ──

    #[test]
    fn test_layout_without_ask_user() {
        let app = TaskCommandApp::new("task prd");
        assert!(!app.has_active_question());
    }

    #[test]
    fn test_layout_with_text_ask_user() {
        let mut app = TaskCommandApp::new("task prd");
        let (tx, _rx) = oneshot::channel();
        app.start_ask_user(vec![make_text_question("Test question?")], tx);

        assert!(app.has_active_question());
        let height = app.active_widget.as_ref().unwrap().height_for_width(80);
        // AskUserWidget text: border_top/title(1) + padding.top(1) + question(1) + input(1) + hint(1) + padding.bottom(1) = 6
        assert_eq!(height, 6u16);
    }

    #[test]
    fn test_layout_with_choice_ask_user() {
        let mut app = TaskCommandApp::new("task prd");
        let (tx, _rx) = oneshot::channel();
        let choice_q = Question {
            question: "Pick one?".into(),
            question_type: QuestionType::Choice,
            options: vec![
                QuestionOption {
                    label: "Opt A".into(),
                    description: None,
                },
                QuestionOption {
                    label: "Opt B".into(),
                    description: None,
                },
                QuestionOption {
                    label: "Opt C".into(),
                    description: None,
                },
            ],
            default: None,
            placeholder: None,
            required: true,
        };
        app.start_ask_user(vec![choice_q], tx);

        assert!(app.has_active_question());
        let height = app.active_widget.as_ref().unwrap().height_for_width(80);
        // AskUserWidget choice: title(1) + padding.top(1) + question(1) + 3 options + Other(1) + padding.bottom(1) = 8
        assert_eq!(height, 8u16);
    }

    #[test]
    fn test_layout_cancel_clears_widget() {
        let mut app = TaskCommandApp::new("task prd");
        let (tx, _rx) = oneshot::channel();
        app.start_ask_user(vec![make_text_question("Question?")], tx);
        assert!(app.has_active_question());

        app.cancel_ask_user();
        assert!(!app.has_active_question());
    }

    // ── Full layout snapshot tests (integration) ──────────────────────

    use crate::test_helpers::snap;
    use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect, prelude::Terminal};

    /// Renderuje całą aplikację TaskCommandApp do buffera (używa AppState::draw)
    fn render_app_to_buffer(app: &mut TaskCommandApp, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("Failed to create test terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, height);
                app.draw(frame, area);
            })
            .expect("Failed to draw app");
        terminal.backend().buffer().clone()
    }

    #[test]
    fn test_snapshot_layout_basic_header_output_status() {
        let mut app = TaskCommandApp::new("task prd");
        app.set_model("claude-sonnet-4-5");
        app.push_text("Generating PRD...");
        app.push_text("Analysis complete.");
        app.set_status_with_progress("Processing", 0.45);

        let buffer = render_app_to_buffer(&mut app, 80, 10);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_layout_without_model() {
        let mut app = TaskCommandApp::new("task plan");
        app.push_text("Planning task execution...");
        app.set_status("Ready");

        let buffer = render_app_to_buffer(&mut app, 80, 8);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_layout_empty_output() {
        let mut app = TaskCommandApp::new("task add");
        app.set_status("Waiting for input");

        let buffer = render_app_to_buffer(&mut app, 60, 6);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_layout_with_text_ask_user_active() {
        let mut app = TaskCommandApp::new("task prd");
        app.set_model("sonnet-4-5");
        app.push_text("Please provide additional context.");
        {
            let (tx, _rx) = oneshot::channel();
            app.start_ask_user(vec![make_text_question("What is the project name?")], tx);
        }
        app.set_status("Waiting for user input");

        let buffer = render_app_to_buffer(&mut app, 80, 15);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_layout_with_choice_ask_user_active() {
        let mut app = TaskCommandApp::new("task plan");
        app.push_text("Planning phase complete.");
        let choice_q = Question {
            question: "Select authentication method".into(),
            question_type: QuestionType::Choice,
            options: vec![
                QuestionOption {
                    label: "JWT".into(),
                    description: Some("Token-based auth".into()),
                },
                QuestionOption {
                    label: "Session".into(),
                    description: Some("Cookie-based auth".into()),
                },
                QuestionOption {
                    label: "OAuth2".into(),
                    description: Some("Third-party auth".into()),
                },
            ],
            default: Some("JWT".into()),
            placeholder: None,
            required: true,
        };
        {
            let (tx, _rx) = oneshot::channel();
            app.start_ask_user(vec![choice_q], tx);
        }
        app.set_status("Awaiting selection");

        let buffer = render_app_to_buffer(&mut app, 80, 18);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_layout_with_confirm_ask_user_active() {
        let mut app = TaskCommandApp::new("task edit");
        app.push_text("Task 2.3 ready to update.");
        let confirm_q = Question {
            question: "Proceed with changes?".into(),
            question_type: QuestionType::Confirm,
            options: vec![],
            default: Some("yes".into()),
            placeholder: None,
            required: true,
        };
        {
            let (tx, _rx) = oneshot::channel();
            app.start_ask_user(vec![confirm_q], tx);
        }
        app.set_status("Confirm action");

        let buffer = render_app_to_buffer(&mut app, 70, 12);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_layout_with_multi_ask_user_active() {
        let mut app = TaskCommandApp::new("task add");
        app.push_text("Adding new feature...");
        let multi_q = Question {
            question: "Select components to include".into(),
            question_type: QuestionType::MultiChoice,
            options: vec![
                QuestionOption {
                    label: "Authentication".into(),
                    description: None,
                },
                QuestionOption {
                    label: "API Gateway".into(),
                    description: None,
                },
                QuestionOption {
                    label: "Database".into(),
                    description: None,
                },
                QuestionOption {
                    label: "Logging".into(),
                    description: None,
                },
            ],
            default: None,
            placeholder: None,
            required: true,
        };
        {
            let (tx, _rx) = oneshot::channel();
            app.start_ask_user(vec![multi_q], tx);
        }
        app.set_status("Multi-select active");

        let buffer = render_app_to_buffer(&mut app, 80, 16);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_layout_ask_user_answered_text() {
        let mut app = TaskCommandApp::new("task prd");
        app.push_text("Context collected.");
        {
            let (tx, _rx) = oneshot::channel();
            app.start_ask_user(vec![make_text_question("Project name?")], tx);
        }
        // Simulate answering — advance_question sends answer and clears widget
        // For snapshot: we want to see the post-answer state (widget cleared)
        app.advance_question("My Awesome Project".into());
        app.set_status("Answer received");

        let buffer = render_app_to_buffer(&mut app, 80, 12);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_layout_scrolled_output() {
        let mut app = TaskCommandApp::new("task test");
        // Wypełnij buffer większą ilością linii
        for i in 0..30 {
            app.push_text(&format!("Output line {}", i));
        }
        app.scroll_up(5);
        app.set_status("Scrolled up 5 lines");

        let buffer = render_app_to_buffer(&mut app, 80, 15);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_layout_narrow_terminal() {
        let mut app = TaskCommandApp::new("task prd");
        app.set_model("sonnet");
        app.push_text("Testing narrow terminal layout.");
        app.set_status_with_progress("Working", 0.75);

        let buffer = render_app_to_buffer(&mut app, 40, 10);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_layout_minimal_height() {
        let mut app = TaskCommandApp::new("task");
        app.set_status("Minimal");

        // Minimalna wysokość: header(1) + output(1) + status(1) = 3
        let buffer = render_app_to_buffer(&mut app, 60, 3);
        insta::assert_snapshot!(snap(&buffer));
    }
}
