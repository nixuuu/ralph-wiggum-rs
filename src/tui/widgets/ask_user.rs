// Widget container dla ask_user — opakowuje 4 typy pytań (text/choice/multi/confirm)
// w ramkę z tytułem "❓ Question".
//
// Dwa stany:
// - Active: interaktywny widget z obsługą klawiszy (przyklejony na dole output area)
// - Answered: statyczny tekst z pytaniem i odpowiedzią (część output area)
//
// handle_key() zwraca AskUserAction { Continue, Submit(String), Cancel }
// height() zwraca dynamiczną wysokość w zależności od typu pytania i stanu
//
// Ten moduł nie zależy od `commands::mcp::ask_user` — definiuje własne typy TUI-level.
// Konwersja z MCP Question → AskUserQuestion odbywa się w warstwie integracyjnej.

use ansi_to_tui::IntoText;
use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Widget},
};

use crate::shared::markdown::{render_markdown_for_width};
use crate::tui::theme::DEFAULT_THEME;
use crate::tui::widgets::ask_user_choice::QuestionOption;
use crate::tui::widgets::{
    ChoiceState, ConfirmState, MultiSelectOption, MultiSelectState, TextInputState,
};

// ── AskUserAction ────────────────────────────────────────────────────

/// Wynik obsługi klawisza w AskUserWidget
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskUserAction {
    /// Kontynuuj interakcję (klawisz obsłużony, brak wyniku)
    Continue,
    /// Użytkownik zaakceptował odpowiedź
    Submit(String),
    /// Użytkownik anulował (Esc / Ctrl+C)
    Cancel,
}

// ── QuestionKind ─────────────────────────────────────────────────────

/// Typ pytania — TUI-level enum (niezależny od MCP protocol types)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionKind {
    /// Pytanie otwarte z polem tekstowym
    Text,
    /// Wybór jednej opcji z listy
    Choice,
    /// Wybór wielu opcji z listy
    MultiChoice,
    /// Pytanie tak/nie
    Confirm,
}

// ── AskUserQuestion ──────────────────────────────────────────────────

/// Dane pytania — TUI-level struct (niezależny od MCP protocol types).
/// Konwersja z MCP `Question` odbywa się w warstwie integracyjnej.
#[derive(Debug, Clone)]
pub struct AskUserQuestion {
    /// Treść pytania (markdown text)
    pub question: String,
    /// Typ pytania
    pub kind: QuestionKind,
    /// Lista opcji (wymagana dla Choice/MultiChoice, pusta dla Text/Confirm)
    pub options: Vec<QuestionOption>,
    /// Wartość domyślna (opcjonalna)
    pub default: Option<String>,
    /// Placeholder tekstu (opcjonalny, dla Text)
    pub placeholder: Option<String>,
}

// ── InnerState ───────────────────────────────────────────────────────

/// Wewnętrzny stan interakcji — odpowiada typowi pytania
#[derive(Debug, Clone)]
pub(super) enum InnerState {
    Text(TextInputState),
    Choice(ChoiceState),
    Multi(MultiSelectState),
    Confirm(ConfirmState),
}

// ── AskUserState ─────────────────────────────────────────────────────

/// Stan widgetu ask_user: aktywny (interaktywny) lub odpowiedziany (statyczny).
///
/// InnerState jest ukryty — interakcja przez metody AskUserWidget.
#[derive(Debug, Clone)]
pub(super) enum AskUserState {
    /// Widget aktywny — użytkownik odpowiada na pytanie
    Active(InnerState),
    /// Widget odpowiedziany — wyświetla pytanie + odpowiedź jako statyczny tekst
    Answered(String),
}

// ── AskUserWidget ────────────────────────────────────────────────────

/// Widget container dla ask_user — opakowuje pytanie w ramkę z tytułem.
///
/// W stanie Active renderuje odpowiedni sub-widget (TextInput, Choice, Multi, Confirm).
/// W stanie Answered renderuje statyczny tekst z pytaniem i odpowiedzią.
///
/// Tworzony przez `AskUserWidget::new()`, stan modyfikowany przez `handle_key()`.
#[derive(Debug, Clone)]
pub struct AskUserWidget {
    /// Dane pytania
    question: AskUserQuestion,
    /// Stan widgetu (Active/Answered)
    state: AskUserState,
}

impl AskUserWidget {
    /// Tworzy nowy widget w stanie Active, inicjalizując odpowiedni sub-widget
    pub fn new(question: AskUserQuestion) -> Self {
        let inner = match question.kind {
            QuestionKind::Text => {
                InnerState::Text(TextInputState::new(question.placeholder.clone()))
            }
            QuestionKind::Choice => {
                let default_index = question
                    .default
                    .as_ref()
                    .and_then(|d| question.options.iter().position(|o| o.label == *d))
                    .unwrap_or(0);
                InnerState::Choice(ChoiceState::with_selected(default_index))
            }
            QuestionKind::MultiChoice => {
                let options: Vec<MultiSelectOption> = question
                    .options
                    .iter()
                    .map(|o| MultiSelectOption {
                        label: o.label.clone(),
                        description: o.description.clone(),
                    })
                    .collect();
                InnerState::Multi(MultiSelectState::new(options))
            }
            QuestionKind::Confirm => {
                let default_yes = question
                    .default
                    .as_ref()
                    .is_some_and(|d: &String| matches!(d.to_lowercase().as_str(), "yes" | "true"));
                InnerState::Confirm(ConfirmState::new(default_yes))
            }
        };

        Self {
            question,
            state: AskUserState::Active(inner),
        }
    }

    /// Tworzy widget w stanie Answered (statyczny)
    pub fn answered(question: AskUserQuestion, answer: String) -> Self {
        Self {
            question,
            state: AskUserState::Answered(answer),
        }
    }

    /// Zwraca czy widget jest w stanie aktywnym
    pub fn is_active(&self) -> bool {
        matches!(self.state, AskUserState::Active(_))
    }

    /// Dynamiczna wysokość widgetu (w wierszach) w zależności od stanu, typu pytania i szerokości.
    ///
    /// Zawiera: border top (1) + question lines + separator (1) + content + border bottom (1).
    /// `width` — dostępna szerokość widgetu (potrzebna do obliczenia zawijania tekstu).
    pub fn height_for_width(&self, width: u16) -> u16 {
        // Inner width: area - border (1+1) - padding (1+1) = width - 4
        let inner_width = width.saturating_sub(4).max(1) as usize;
        let question_lines = count_rendered_lines_for_width(&self.question.question, inner_width);

        match &self.state {
            AskUserState::Active(inner) => {
                let content_height = match inner {
                    // Text: pytanie + separator + input line + hint line
                    InnerState::Text(_) => question_lines + 1 + 1 + 1,
                    // Choice: pytanie + separator + opcje
                    InnerState::Choice(_) => {
                        question_lines + 1 + self.question.options.len() as u16
                    }
                    // Multi: pytanie + separator + opcje + hint line
                    InnerState::Multi(state) => question_lines + 1 + state.options.len() as u16 + 1,
                    // Confirm: pytanie + separator + buttons line
                    InnerState::Confirm(_) => question_lines + 1 + 1,
                };
                // border top + content + border bottom
                content_height + 2
            }
            AskUserState::Answered(answer) => {
                let answer_lines = if answer.is_empty() {
                    1
                } else {
                    answer.lines().count().max(1) as u16
                };
                // border top + question + separator + answer + border bottom
                question_lines + 1 + answer_lines + 2
            }
        }
    }

    /// Obsługuje zdarzenie klawiatury w stanie Active.
    ///
    /// Zwraca `AskUserAction`:
    /// - `Continue` — klawisz obsłużony, kontynuuj interakcję
    /// - `Submit(answer)` — użytkownik zaakceptował odpowiedź (Enter)
    /// - `Cancel` — użytkownik anulował (Esc)
    pub fn handle_key(&mut self, key: KeyCode) -> AskUserAction {
        let inner = match &mut self.state {
            AskUserState::Active(inner) => inner,
            AskUserState::Answered(_) => return AskUserAction::Continue,
        };

        // Esc → anuluj (wspólne dla wszystkich typów)
        if key == KeyCode::Esc {
            return AskUserAction::Cancel;
        }

        match inner {
            InnerState::Text(state) => handle_text_key(state, key),
            InnerState::Choice(state) => handle_choice_key(state, &self.question.options, key),
            InnerState::Multi(state) => handle_multi_key(state, key),
            InnerState::Confirm(state) => handle_confirm_key(state, key),
        }
    }

    /// Przechodzi do stanu Answered z podaną odpowiedzią
    pub fn set_answered(&mut self, answer: String) {
        self.state = AskUserState::Answered(answer);
    }
}

// ── Key handlers per question type ─────────────────────────────────

fn handle_text_key(state: &mut TextInputState, key: KeyCode) -> AskUserAction {
    match key {
        KeyCode::Enter => {
            let value = state.value().to_string();
            AskUserAction::Submit(value)
        }
        KeyCode::Backspace => {
            state.delete_char();
            AskUserAction::Continue
        }
        KeyCode::Left => {
            state.move_cursor_left();
            AskUserAction::Continue
        }
        KeyCode::Right => {
            state.move_cursor_right();
            AskUserAction::Continue
        }
        KeyCode::Home => {
            state.move_cursor_home();
            AskUserAction::Continue
        }
        KeyCode::End => {
            state.move_cursor_end();
            AskUserAction::Continue
        }
        KeyCode::Char(c) => {
            state.insert_char(c);
            AskUserAction::Continue
        }
        _ => AskUserAction::Continue,
    }
}

fn handle_choice_key(
    state: &mut ChoiceState,
    options: &[QuestionOption],
    key: KeyCode,
) -> AskUserAction {
    match key {
        KeyCode::Up => {
            state.move_up(options.len());
            AskUserAction::Continue
        }
        KeyCode::Down => {
            state.move_down(options.len());
            AskUserAction::Continue
        }
        KeyCode::Enter => {
            if let Some(label) = state.get_selected_label(options) {
                AskUserAction::Submit(label.to_string())
            } else {
                AskUserAction::Continue
            }
        }
        _ => AskUserAction::Continue,
    }
}

fn handle_multi_key(state: &mut MultiSelectState, key: KeyCode) -> AskUserAction {
    match key {
        KeyCode::Up => {
            state.move_up();
            AskUserAction::Continue
        }
        KeyCode::Down => {
            state.move_down();
            AskUserAction::Continue
        }
        KeyCode::Char(' ') => {
            state.toggle_current();
            AskUserAction::Continue
        }
        KeyCode::Enter => {
            let selected = state.get_selected_labels();
            AskUserAction::Submit(selected)
        }
        _ => AskUserAction::Continue,
    }
}

fn handle_confirm_key(state: &mut ConfirmState, key: KeyCode) -> AskUserAction {
    match key {
        KeyCode::Left | KeyCode::Char('y') | KeyCode::Char('Y') => {
            state.select_yes();
            AskUserAction::Continue
        }
        KeyCode::Right | KeyCode::Char('n') | KeyCode::Char('N') => {
            state.select_no();
            AskUserAction::Continue
        }
        KeyCode::Tab => {
            state.toggle();
            AskUserAction::Continue
        }
        KeyCode::Enter => AskUserAction::Submit(state.value().to_string()),
        _ => AskUserAction::Continue,
    }
}

// ── Widget implementation ──────────────────────────────────────────

impl Widget for AskUserWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let theme = &DEFAULT_THEME;
        let is_active = matches!(self.state, AskUserState::Active(_));

        let title_style = if is_active {
            theme.header_style()
        } else {
            theme.muted_style()
        };

        let title = if is_active {
            " ❓ Question "
        } else {
            " ✅ Answered "
        };

        let block = Block::default()
            .padding(Padding::uniform(1))
            .style(Style::default().bg(theme.panel_bg(is_active)))
            .title(Span::styled(title, title_style));

        let inner_area = block.inner(area);
        block.render(area, buf);

        match &self.state {
            AskUserState::Active(inner) => {
                render_active(inner, &self.question, inner_area, buf);
            }
            AskUserState::Answered(answer) => {
                render_answered(&self.question.question, answer, inner_area, buf);
            }
        }
    }
}

// ── Render helpers ─────────────────────────────────────────────────

/// Renderuje aktywny stan z odpowiednim sub-widgetem
fn render_active(inner: &InnerState, question: &AskUserQuestion, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let question_text = render_markdown_for_width(&question.question, area.width as usize);
    let parsed_lines = ansi_to_lines(&question_text);
    let q_height = parsed_lines.len() as u16;

    // Renderuj pytanie (ANSI-styled lines z markdown renderer)
    for (i, line) in parsed_lines.iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height {
            break;
        }
        buf.set_line(area.x, y, line, area.width);
    }

    // Separator (pusta linia)
    let sep_y = area.y + q_height;
    if sep_y >= area.y + area.height {
        return;
    }

    // Content area pod pytaniem i separatorem
    let content_y = sep_y + 1;
    let content_height = area.height.saturating_sub(q_height + 1);
    if content_height == 0 {
        return;
    }

    let content_area = Rect {
        x: area.x,
        y: content_y,
        width: area.width,
        height: content_height,
    };

    match inner {
        InnerState::Text(state) => render_text_active(state, question, content_area, buf),
        InnerState::Choice(state) => render_choice_active(state, question, content_area, buf),
        InnerState::Multi(state) => render_multi_active(state, content_area, buf),
        InnerState::Confirm(state) => render_confirm_active(state, content_area, buf),
    }
}

/// Renderuje text input: prompt "> " + buffer z kursorem + hint line
fn render_text_active(
    state: &TextInputState,
    question: &AskUserQuestion,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.height == 0 {
        return;
    }

    let mut spans = Vec::new();
    spans.push(Span::styled("> ", DEFAULT_THEME.header_style()));

    if state.buffer.is_empty() {
        // Kursor block + placeholder
        spans.push(Span::styled("█", DEFAULT_THEME.primary_style()));
        if let Some(ref placeholder) = question.placeholder {
            let rest: String = placeholder.chars().skip(1).collect();
            if !rest.is_empty() {
                spans.push(Span::styled(
                    rest,
                    DEFAULT_THEME.muted_style().add_modifier(Modifier::ITALIC),
                ));
            }
        }
    } else {
        // Buffer z kursorem na pozycji
        let chars: Vec<char> = state.buffer.chars().collect();
        let cursor_pos = state.cursor_pos;

        for (i, &ch) in chars.iter().enumerate() {
            if i == cursor_pos {
                spans.push(Span::styled(
                    ch.to_string(),
                    DEFAULT_THEME
                        .primary_style()
                        .add_modifier(Modifier::REVERSED),
                ));
            } else {
                spans.push(Span::raw(ch.to_string()));
            }
        }

        // Kursor na końcu tekstu
        if cursor_pos == chars.len() {
            spans.push(Span::styled("█", DEFAULT_THEME.primary_style()));
        }
    }

    buf.set_line(area.x, area.y, &Line::from(spans), area.width);

    // Hint line
    if area.height > 1 {
        let hint = Line::from(Span::styled(
            "Enter: submit │ Esc: cancel",
            DEFAULT_THEME.muted_style(),
        ));
        buf.set_line(area.x, area.y + 1, &hint, area.width);
    }
}

/// Renderuje choice select: radio buttons z opcjami
fn render_choice_active(
    state: &ChoiceState,
    question: &AskUserQuestion,
    area: Rect,
    buf: &mut Buffer,
) {
    for (idx, option) in question.options.iter().enumerate() {
        let y = area.y + idx as u16;
        if y >= area.y + area.height {
            break;
        }

        let is_selected = idx == state.selected;
        let radio = if is_selected { "●" } else { "○" };
        let style = if is_selected {
            Style::default()
                .fg(DEFAULT_THEME.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            DEFAULT_THEME.muted_style()
        };

        let mut spans = vec![
            Span::styled(radio, style),
            Span::raw(" "),
            Span::styled(&option.label, style),
        ];

        if let Some(ref desc) = option.description {
            spans.push(Span::styled(
                format!("  {desc}"),
                DEFAULT_THEME.muted_style(),
            ));
        }

        buf.set_line(area.x, y, &Line::from(spans), area.width);
    }
}

/// Renderuje multi-select: checkboxy z opcjami + hint line
fn render_multi_active(state: &MultiSelectState, area: Rect, buf: &mut Buffer) {
    for (idx, option) in state.options.iter().enumerate() {
        let y = area.y + idx as u16;
        if y >= area.y + area.height {
            break;
        }

        let is_cursor = idx == state.cursor;
        let is_checked = idx < state.checked.len() && state.checked[idx];

        let prefix = if is_cursor { "> " } else { "  " };
        let checkbox = if is_checked { "[✓] " } else { "[ ] " };

        let style = if is_cursor {
            DEFAULT_THEME.header_style().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let mut spans = vec![
            Span::styled(prefix, style),
            Span::styled(checkbox, style),
            Span::styled(&option.label, style),
        ];

        if let Some(ref desc) = option.description {
            spans.push(Span::styled(
                format!("  {desc}"),
                DEFAULT_THEME.muted_style(),
            ));
        }

        buf.set_line(area.x, y, &Line::from(spans), area.width);
    }

    // Hint line pod opcjami
    let hint_y = area.y + state.options.len() as u16;
    if hint_y < area.y + area.height {
        let hint = Line::from(Span::styled(
            "Space: toggle │ Enter: submit │ Esc: cancel",
            DEFAULT_THEME.muted_style(),
        ));
        buf.set_line(area.x, hint_y, &hint, area.width);
    }
}

/// Renderuje confirm: przyciski [Yes] [No]
fn render_confirm_active(state: &ConfirmState, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }

    let yes_style = if state.selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let no_style = if !state.selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let line = Line::from(vec![
        Span::styled("[Yes]", yes_style),
        Span::raw("  "),
        Span::styled("[No]", no_style),
    ]);

    buf.set_line(area.x, area.y, &line, area.width);
}

/// Renderuje stan Answered — pytanie + separator + odpowiedź (statyczny tekst)
fn render_answered(question_text: &str, answer: &str, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let rendered_question = render_markdown_for_width(question_text, area.width as usize);
    let parsed_lines = ansi_to_lines(&rendered_question);

    // Pytanie (ANSI-styled lines z markdown renderer)
    for (i, line) in parsed_lines.iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height {
            return;
        }
        buf.set_line(area.x, y, line, area.width);
    }

    // Separator (pusta linia)
    let sep_y = area.y + parsed_lines.len() as u16;
    if sep_y >= area.y + area.height {
        return;
    }

    // Odpowiedź (zielony tekst)
    let answer_y = sep_y + 1;
    if answer.is_empty() {
        if answer_y < area.y + area.height {
            let line = Line::from(Span::styled("(no answer)", DEFAULT_THEME.muted_style()));
            buf.set_line(area.x, answer_y, &line, area.width);
        }
    } else {
        for (i, line_text) in answer.lines().enumerate() {
            let y = answer_y + i as u16;
            if y >= area.y + area.height {
                break;
            }
            let line = Line::from(Span::styled(
                line_text,
                Style::default().fg(DEFAULT_THEME.success),
            ));
            buf.set_line(area.x, y, &line, area.width);
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────

/// Konwertuje tekst z kodami ANSI na `Vec<Line<'static>>` ratatui.
///
/// Markdown renderer (termimad) zwraca tekst z kodami ANSI — ta funkcja
/// parsuje escape sequences na styled spany ratatui.
/// Fallback: gdy parsowanie ANSI zawiedzie, traktuje tekst jako raw lines.
fn ansi_to_lines(text: &str) -> Vec<Line<'static>> {
    match text.into_text() {
        Ok(parsed) => parsed.lines.into_iter().collect(),
        Err(_) => text.lines().map(|l| Line::raw(l.to_string())).collect(),
    }
}

/// Zlicza linie po wyrenderowaniu markdown tekstu pytania z uwzględnieniem szerokości.
fn count_rendered_lines_for_width(question_text: &str, width: usize) -> u16 {
    if question_text.is_empty() {
        return 1;
    }
    let rendered = render_markdown_for_width(question_text, width);
    ansi_to_lines(&rendered).len().max(1) as u16
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{render_widget_to_buffer, snap};

    // ── Test helpers ──────────────────────────────────────────────

    fn text_question() -> AskUserQuestion {
        AskUserQuestion {
            question: "What is your name?".into(),
            kind: QuestionKind::Text,
            options: vec![],
            default: None,
            placeholder: Some("John Doe".into()),
        }
    }

    fn choice_question() -> AskUserQuestion {
        AskUserQuestion {
            question: "Choose auth".into(),
            kind: QuestionKind::Choice,
            options: vec![
                QuestionOption {
                    label: "JWT".into(),
                    description: Some("Token-based".into()),
                },
                QuestionOption {
                    label: "Session".into(),
                    description: Some("Cookie-based".into()),
                },
            ],
            default: Some("JWT".into()),
            placeholder: None,
        }
    }

    fn multi_question() -> AskUserQuestion {
        AskUserQuestion {
            question: "Select features".into(),
            kind: QuestionKind::MultiChoice,
            options: vec![
                QuestionOption {
                    label: "Auth".into(),
                    description: None,
                },
                QuestionOption {
                    label: "API".into(),
                    description: None,
                },
                QuestionOption {
                    label: "Logging".into(),
                    description: None,
                },
            ],
            default: None,
            placeholder: None,
        }
    }

    fn confirm_question() -> AskUserQuestion {
        AskUserQuestion {
            question: "Are you sure?".into(),
            kind: QuestionKind::Confirm,
            options: vec![],
            default: Some("yes".into()),
            placeholder: None,
        }
    }

    // ── Constructor Tests ──────────────────────────────────────────

    #[test]
    fn test_new_text_creates_active_state() {
        let widget = AskUserWidget::new(text_question());
        assert!(widget.is_active());
        assert!(matches!(
            widget.state,
            AskUserState::Active(InnerState::Text(_))
        ));
    }

    #[test]
    fn test_new_choice_creates_active_state() {
        let widget = AskUserWidget::new(choice_question());
        assert!(widget.is_active());
        assert!(matches!(
            widget.state,
            AskUserState::Active(InnerState::Choice(_))
        ));
    }

    #[test]
    fn test_new_multi_creates_active_state() {
        let widget = AskUserWidget::new(multi_question());
        assert!(widget.is_active());
        assert!(matches!(
            widget.state,
            AskUserState::Active(InnerState::Multi(_))
        ));
    }

    #[test]
    fn test_new_confirm_creates_active_state() {
        let widget = AskUserWidget::new(confirm_question());
        assert!(widget.is_active());
        assert!(matches!(
            widget.state,
            AskUserState::Active(InnerState::Confirm(_))
        ));
    }

    #[test]
    fn test_answered_creates_answered_state() {
        let widget = AskUserWidget::answered(text_question(), "Alice".into());
        assert!(!widget.is_active());
        assert!(matches!(widget.state, AskUserState::Answered(ref s) if s == "Alice"));
    }

    // ── Choice default index ──────────────────────────────────────

    #[test]
    fn test_choice_default_selects_correct_option() {
        let widget = AskUserWidget::new(choice_question());
        if let AskUserState::Active(InnerState::Choice(ref state)) = widget.state {
            assert_eq!(state.selected, 0); // "JWT" jest na indeksie 0
        } else {
            panic!("Expected Active Choice state");
        }
    }

    #[test]
    fn test_choice_default_second_option() {
        let mut q = choice_question();
        q.default = Some("Session".into());
        let widget = AskUserWidget::new(q);
        if let AskUserState::Active(InnerState::Choice(ref state)) = widget.state {
            assert_eq!(state.selected, 1);
        } else {
            panic!("Expected Active Choice state");
        }
    }

    #[test]
    fn test_choice_default_unknown_falls_back_to_zero() {
        let mut q = choice_question();
        q.default = Some("Unknown".into());
        let widget = AskUserWidget::new(q);
        if let AskUserState::Active(InnerState::Choice(ref state)) = widget.state {
            assert_eq!(state.selected, 0);
        } else {
            panic!("Expected Active Choice state");
        }
    }

    // ── Confirm default ──────────────────────────────────────────

    #[test]
    fn test_confirm_default_yes() {
        let widget = AskUserWidget::new(confirm_question());
        if let AskUserState::Active(InnerState::Confirm(ref state)) = widget.state {
            assert!(state.selected);
        } else {
            panic!("Expected Active Confirm state");
        }
    }

    #[test]
    fn test_confirm_default_no() {
        let mut q = confirm_question();
        q.default = Some("no".into());
        let widget = AskUserWidget::new(q);
        if let AskUserState::Active(InnerState::Confirm(ref state)) = widget.state {
            assert!(!state.selected);
        } else {
            panic!("Expected Active Confirm state");
        }
    }

    // ── Height Tests ──────────────────────────────────────────────

    #[test]
    fn test_height_text_active() {
        let widget = AskUserWidget::new(text_question());
        // border(1) + question(1) + sep(1) + input(1) + hint(1) + border(1) = 6
        assert_eq!(widget.height_for_width(50), 6);
    }

    #[test]
    fn test_height_choice_active() {
        let widget = AskUserWidget::new(choice_question());
        // border(1) + question(1) + sep(1) + 2 opcje + border(1) = 6
        assert_eq!(widget.height_for_width(50), 6);
    }

    #[test]
    fn test_height_multi_active() {
        let widget = AskUserWidget::new(multi_question());
        // border(1) + question(1) + sep(1) + 3 opcje + hint(1) + border(1) = 8
        assert_eq!(widget.height_for_width(50), 8);
    }

    #[test]
    fn test_height_confirm_active() {
        let widget = AskUserWidget::new(confirm_question());
        // border(1) + question(1) + separator(1) + buttons(1) + border(1) = 5
        assert_eq!(widget.height_for_width(50), 5);
    }

    #[test]
    fn test_height_answered() {
        let widget = AskUserWidget::answered(text_question(), "Alice".into());
        // border(1) + question(1) + sep(1) + answer(1) + border(1) = 5
        assert_eq!(widget.height_for_width(50), 5);
    }

    #[test]
    fn test_height_answered_multiline() {
        let widget = AskUserWidget::answered(text_question(), "Line 1\nLine 2\nLine 3".into());
        // border(1) + question(1) + sep(1) + 3 answer lines + border(1) = 7
        assert_eq!(widget.height_for_width(50), 7);
    }

    // ── Handle Key Tests ──────────────────────────────────────────

    #[test]
    fn test_handle_key_esc_cancels() {
        let mut widget = AskUserWidget::new(text_question());
        assert_eq!(widget.handle_key(KeyCode::Esc), AskUserAction::Cancel);
    }

    #[test]
    fn test_handle_key_text_enter_submits() {
        let mut widget = AskUserWidget::new(text_question());
        widget.handle_key(KeyCode::Char('A'));
        widget.handle_key(KeyCode::Char('l'));
        widget.handle_key(KeyCode::Char('i'));
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("Ali".into())
        );
    }

    #[test]
    fn test_handle_key_text_empty_submit() {
        let mut widget = AskUserWidget::new(text_question());
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("".into())
        );
    }

    #[test]
    fn test_handle_key_text_backspace() {
        let mut widget = AskUserWidget::new(text_question());
        widget.handle_key(KeyCode::Char('A'));
        widget.handle_key(KeyCode::Char('B'));
        widget.handle_key(KeyCode::Backspace);
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("A".into())
        );
    }

    #[test]
    fn test_handle_key_text_cursor_movement() {
        let mut widget = AskUserWidget::new(text_question());
        widget.handle_key(KeyCode::Char('A'));
        widget.handle_key(KeyCode::Char('B'));
        widget.handle_key(KeyCode::Char('C'));
        // Kursor na Home → 0, wstaw D na początku
        widget.handle_key(KeyCode::Home);
        widget.handle_key(KeyCode::Char('D'));
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("DABC".into())
        );
    }

    #[test]
    fn test_handle_key_choice_navigate_and_submit() {
        let mut widget = AskUserWidget::new(choice_question());
        assert_eq!(widget.handle_key(KeyCode::Down), AskUserAction::Continue);
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("Session".into())
        );
    }

    #[test]
    fn test_handle_key_choice_submit_first() {
        let mut widget = AskUserWidget::new(choice_question());
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("JWT".into())
        );
    }

    #[test]
    fn test_handle_key_choice_wrap_up() {
        let mut widget = AskUserWidget::new(choice_question());
        // Na JWT (0), Up → wrap do Session (1)
        widget.handle_key(KeyCode::Up);
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("Session".into())
        );
    }

    #[test]
    fn test_handle_key_multi_toggle_and_submit() {
        let mut widget = AskUserWidget::new(multi_question());
        widget.handle_key(KeyCode::Char(' '));
        widget.handle_key(KeyCode::Down);
        widget.handle_key(KeyCode::Char(' '));
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("Auth, API".into())
        );
    }

    #[test]
    fn test_handle_key_multi_empty_submit() {
        let mut widget = AskUserWidget::new(multi_question());
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("".into())
        );
    }

    #[test]
    fn test_handle_key_confirm_yes() {
        let mut widget = AskUserWidget::new(confirm_question());
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("yes".into())
        );
    }

    #[test]
    fn test_handle_key_confirm_no() {
        let mut widget = AskUserWidget::new(confirm_question());
        widget.handle_key(KeyCode::Char('n'));
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("no".into())
        );
    }

    #[test]
    fn test_handle_key_confirm_toggle() {
        let mut widget = AskUserWidget::new(confirm_question());
        widget.handle_key(KeyCode::Tab);
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("no".into())
        );
    }

    #[test]
    fn test_handle_key_confirm_y_shortcut() {
        let mut widget = AskUserWidget::new(confirm_question());
        // Start na yes, przejdź na no, potem y
        widget.handle_key(KeyCode::Char('n'));
        widget.handle_key(KeyCode::Char('y'));
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("yes".into())
        );
    }

    #[test]
    fn test_handle_key_on_answered_is_noop() {
        let mut widget = AskUserWidget::answered(text_question(), "Alice".into());
        assert_eq!(widget.handle_key(KeyCode::Enter), AskUserAction::Continue);
        assert_eq!(widget.handle_key(KeyCode::Esc), AskUserAction::Continue);
    }

    // ── set_answered Tests ──────────────────────────────────────

    #[test]
    fn test_set_answered_transitions_state() {
        let mut widget = AskUserWidget::new(text_question());
        assert!(widget.is_active());
        widget.set_answered("Alice".into());
        assert!(!widget.is_active());
    }

    // ── Render Snapshot Tests ──────────────────────────────────────

    #[test]
    fn test_snapshot_text_active() {
        let widget = AskUserWidget::new(text_question());
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget, 50, h);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_choice_active() {
        let widget = AskUserWidget::new(choice_question());
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget, 50, h);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_multi_active() {
        let widget = AskUserWidget::new(multi_question());
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget, 50, h);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_confirm_active() {
        let widget = AskUserWidget::new(confirm_question());
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget, 50, h);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_text_answered() {
        let widget = AskUserWidget::answered(text_question(), "Alice".into());
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget, 50, h);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_choice_answered() {
        let widget = AskUserWidget::answered(choice_question(), "JWT".into());
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget, 50, h);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_answered_empty() {
        let widget = AskUserWidget::answered(text_question(), String::new());
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget, 50, h);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_answered_multiline() {
        let widget = AskUserWidget::answered(text_question(), "Line 1\nLine 2\nLine 3".into());
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget, 50, h);
        insta::assert_snapshot!(snap(&buffer));
    }

    // ── Render edge cases ──────────────────────────────────────────

    #[test]
    fn test_render_zero_area() {
        let widget = AskUserWidget::new(text_question());
        let buffer = render_widget_to_buffer(widget, 0, 0);
        assert_eq!(snap(&buffer), "");
    }

    #[test]
    fn test_render_very_narrow() {
        let widget = AskUserWidget::new(text_question());
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget, 10, h);
        let output = snap(&buffer);
        assert!(!output.is_empty());
    }

    // ── Integration tests: key sequences ────────────────────────────

    #[test]
    fn test_integration_text_input_full_flow() {
        let mut widget = AskUserWidget::new(text_question());

        // Symuluj wpisywanie "Alice"
        assert_eq!(
            widget.handle_key(KeyCode::Char('A')),
            AskUserAction::Continue
        );
        assert_eq!(
            widget.handle_key(KeyCode::Char('l')),
            AskUserAction::Continue
        );
        assert_eq!(
            widget.handle_key(KeyCode::Char('i')),
            AskUserAction::Continue
        );
        assert_eq!(
            widget.handle_key(KeyCode::Char('c')),
            AskUserAction::Continue
        );
        assert_eq!(
            widget.handle_key(KeyCode::Char('e')),
            AskUserAction::Continue
        );

        // Submit
        let result = widget.handle_key(KeyCode::Enter);
        assert_eq!(result, AskUserAction::Submit("Alice".into()));

        // Snapshot po submicie (widget nadal Active do momentu set_answered)
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget.clone(), 50, h);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_integration_text_input_edit_and_submit() {
        let mut widget = AskUserWidget::new(text_question());

        // Wpisz "Alicee" (błąd)
        widget.handle_key(KeyCode::Char('A'));
        widget.handle_key(KeyCode::Char('l'));
        widget.handle_key(KeyCode::Char('i'));
        widget.handle_key(KeyCode::Char('c'));
        widget.handle_key(KeyCode::Char('e'));
        widget.handle_key(KeyCode::Char('e'));

        // Cofnij jedno "e"
        assert_eq!(
            widget.handle_key(KeyCode::Backspace),
            AskUserAction::Continue
        );

        // Submit
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("Alice".into())
        );
    }

    #[test]
    fn test_integration_text_input_cursor_navigation() {
        let mut widget = AskUserWidget::new(text_question());

        // Wpisz "AC"
        widget.handle_key(KeyCode::Char('A'));
        widget.handle_key(KeyCode::Char('C'));

        // Home → kursor na początek
        widget.handle_key(KeyCode::Home);

        // Right → kursor na pozycję 1
        widget.handle_key(KeyCode::Right);

        // Wstaw 'B' między A i C
        widget.handle_key(KeyCode::Char('B'));

        // Submit → "ABC"
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("ABC".into())
        );
    }

    #[test]
    fn test_integration_text_input_cancel() {
        let mut widget = AskUserWidget::new(text_question());

        widget.handle_key(KeyCode::Char('T'));
        widget.handle_key(KeyCode::Char('e'));
        widget.handle_key(KeyCode::Char('s'));
        widget.handle_key(KeyCode::Char('t'));

        // Esc → anuluj
        assert_eq!(widget.handle_key(KeyCode::Esc), AskUserAction::Cancel);
    }

    #[test]
    fn test_integration_choice_navigate_and_submit() {
        let mut widget = AskUserWidget::new(choice_question());

        // Start na JWT (default, index 0)
        // Down → Session (index 1)
        assert_eq!(widget.handle_key(KeyCode::Down), AskUserAction::Continue);

        // Snapshot z zaznaczonym Session
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget.clone(), 60, h);
        insta::assert_snapshot!(snap(&buffer));

        // Submit Session
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("Session".into())
        );
    }

    #[test]
    fn test_integration_choice_wrap_around() {
        let mut widget = AskUserWidget::new(choice_question());

        // Start na JWT (0)
        // Up → wrap do Session (1)
        widget.handle_key(KeyCode::Up);

        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("Session".into())
        );
    }

    #[test]
    fn test_integration_choice_cancel() {
        let mut widget = AskUserWidget::new(choice_question());

        widget.handle_key(KeyCode::Down);
        widget.handle_key(KeyCode::Down);

        assert_eq!(widget.handle_key(KeyCode::Esc), AskUserAction::Cancel);
    }

    #[test]
    fn test_integration_multi_toggle_and_submit() {
        let mut widget = AskUserWidget::new(multi_question());

        // Zaznacz Auth (index 0)
        assert_eq!(
            widget.handle_key(KeyCode::Char(' ')),
            AskUserAction::Continue
        );

        // Down → API (index 1)
        assert_eq!(widget.handle_key(KeyCode::Down), AskUserAction::Continue);

        // Zaznacz API
        assert_eq!(
            widget.handle_key(KeyCode::Char(' ')),
            AskUserAction::Continue
        );

        // Down → Logging (index 2)
        widget.handle_key(KeyCode::Down);

        // Zaznacz Logging
        widget.handle_key(KeyCode::Char(' '));

        // Snapshot z zaznaczonymi Auth, API, Logging (kursor na Logging)
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget.clone(), 60, h);
        insta::assert_snapshot!(snap(&buffer));

        // Submit
        let result = widget.handle_key(KeyCode::Enter);
        assert_eq!(result, AskUserAction::Submit("Auth, API, Logging".into()));
    }

    #[test]
    fn test_integration_multi_toggle_deselect() {
        let mut widget = AskUserWidget::new(multi_question());

        // Zaznacz Auth
        widget.handle_key(KeyCode::Char(' '));

        // Down → API
        widget.handle_key(KeyCode::Down);

        // Zaznacz API
        widget.handle_key(KeyCode::Char(' '));

        // Up → Auth
        widget.handle_key(KeyCode::Up);

        // Odznacz Auth (drugi Space na tym samym indeksie)
        widget.handle_key(KeyCode::Char(' '));

        // Submit → tylko "API"
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("API".into())
        );
    }

    #[test]
    fn test_integration_multi_submit_empty() {
        let mut widget = AskUserWidget::new(multi_question());

        // Submit bez zaznaczania czegokolwiek
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("".into())
        );
    }

    #[test]
    fn test_integration_multi_cancel() {
        let mut widget = AskUserWidget::new(multi_question());

        widget.handle_key(KeyCode::Char(' '));
        widget.handle_key(KeyCode::Down);
        widget.handle_key(KeyCode::Char(' '));

        assert_eq!(widget.handle_key(KeyCode::Esc), AskUserAction::Cancel);
    }

    #[test]
    fn test_integration_confirm_toggle_and_submit() {
        let mut widget = AskUserWidget::new(confirm_question());

        // Start na Yes (default)
        // Tab → toggle do No
        assert_eq!(widget.handle_key(KeyCode::Tab), AskUserAction::Continue);

        // Snapshot z zaznaczonym No
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget.clone(), 50, h);
        insta::assert_snapshot!(snap(&buffer));

        // Submit No
        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("no".into())
        );
    }

    #[test]
    fn test_integration_confirm_keyboard_shortcuts() {
        let mut widget = AskUserWidget::new(confirm_question());

        // Start na Yes
        // Naciśnij 'n' → przejdź na No
        widget.handle_key(KeyCode::Char('n'));

        assert_eq!(
            widget.handle_key(KeyCode::Enter),
            AskUserAction::Submit("no".into())
        );

        // Nowy widget
        let mut widget2 = AskUserWidget::new(confirm_question());

        // Naciśnij 'y' (lub 'Y')
        widget2.handle_key(KeyCode::Char('Y'));

        assert_eq!(
            widget2.handle_key(KeyCode::Enter),
            AskUserAction::Submit("yes".into())
        );
    }

    #[test]
    fn test_integration_confirm_cancel() {
        let mut widget = AskUserWidget::new(confirm_question());

        widget.handle_key(KeyCode::Tab);

        assert_eq!(widget.handle_key(KeyCode::Esc), AskUserAction::Cancel);
    }

    #[test]
    fn test_integration_answered_no_interaction() {
        let mut widget = AskUserWidget::answered(text_question(), "Bob".into());

        // W stanie Answered wszystkie klawisze są ignorowane
        assert_eq!(widget.handle_key(KeyCode::Enter), AskUserAction::Continue);
        assert_eq!(
            widget.handle_key(KeyCode::Char('x')),
            AskUserAction::Continue
        );
        assert_eq!(widget.handle_key(KeyCode::Esc), AskUserAction::Continue);

        // Snapshot stanu Answered
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget, 50, h);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_integration_set_answered_transition() {
        let mut widget = AskUserWidget::new(text_question());

        // Widget Active
        assert!(widget.is_active());

        // Wpisz odpowiedź
        widget.handle_key(KeyCode::Char('T'));
        widget.handle_key(KeyCode::Char('e'));
        widget.handle_key(KeyCode::Char('s'));
        widget.handle_key(KeyCode::Char('t'));

        // Submit → dostajemy Answer
        let result = widget.handle_key(KeyCode::Enter);
        assert_eq!(result, AskUserAction::Submit("Test".into()));

        // Aplikacja wywołuje set_answered() po otrzymaniu Submit
        widget.set_answered("Test".into());

        // Widget teraz Answered
        assert!(!widget.is_active());

        // Snapshot stanu Answered
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget, 50, h);
        insta::assert_snapshot!(snap(&buffer));
    }
}
