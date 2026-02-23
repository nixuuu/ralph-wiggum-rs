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
use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::StatefulWidget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Scrollbar, ScrollbarOrientation, ScrollbarState, Widget},
};

use crate::shared::markdown::render_markdown_for_width;
use crate::tui::theme::DEFAULT_THEME;
use crate::tui::widgets::ask_user_choice::QuestionOption;
use crate::tui::widgets::multiline_text_input::MultilineTextInput;
use crate::tui::widgets::text_input_overlay::InputAction;
use crate::tui::widgets::{
    ChoiceState, ConfirmState, MultiSelectOption, MultiSelectState, MultilineTextInputState,
    TextInputState,
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
    /// Multiline text input (Shift+Enter = newline, Enter = submit)
    Text(MultilineTextInputState),
    /// (choice state, text input when "Other" option is being typed)
    Choice(ChoiceState, Option<TextInputState>),
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
/// Gdy treść pytania nie mieści się w dostępnym obszarze, widget scrolluje wirtualnie
/// przez tymczasowy bufor. Scroll obsługiwany przez `scroll_up()`/`scroll_down()`.
#[derive(Debug, Clone)]
pub struct AskUserWidget {
    /// Dane pytania
    question: AskUserQuestion,
    /// Stan widgetu (Active/Answered)
    state: AskUserState,
    /// Scroll offset w wierszach od góry (0 = brak scrollowania)
    scroll_offset: u16,
}

impl AskUserWidget {
    /// Tworzy nowy widget w stanie Active, inicjalizując odpowiedni sub-widget
    pub fn new(question: AskUserQuestion) -> Self {
        let inner = match question.kind {
            QuestionKind::Text => {
                // MultilineTextInputState: Shift+Enter = newline, Enter = submit
                InnerState::Text(MultilineTextInputState::new())
            }
            QuestionKind::Choice => {
                let default_index = question
                    .default
                    .as_ref()
                    .and_then(|d| question.options.iter().position(|o| o.label == *d))
                    .unwrap_or(0);
                InnerState::Choice(ChoiceState::with_selected(default_index), None)
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
            scroll_offset: 0,
        }
    }

    /// Tworzy widget w stanie Answered (statyczny)
    pub fn answered(question: AskUserQuestion, answer: String) -> Self {
        Self {
            question,
            state: AskUserState::Answered(answer),
            scroll_offset: 0,
        }
    }

    /// Zwraca czy widget jest w stanie aktywnym
    pub fn is_active(&self) -> bool {
        matches!(self.state, AskUserState::Active(_))
    }

    /// Pełna wysokość widgetu (w wierszach) potrzebna do wyświetlenia całej treści.
    ///
    /// Zawiera: border top (1) + padding.top (1) + question lines + content + padding.bottom (1).
    /// `width` — dostępna szerokość widgetu (potrzebna do obliczenia zawijania tekstu).
    pub fn full_height_for_width(&self, width: u16) -> u16 {
        // Inner width: area - padding.left(2) - padding.right(2) = width - 4
        // (Borders::TOP has no left/right sides, so only padding reduces width)
        let inner_width = width.saturating_sub(4).max(1) as usize;
        let question_lines =
            count_rendered_lines_for_width(self.question.question.trim_end(), inner_width);

        match &self.state {
            AskUserState::Active(inner) => {
                let content_height = match inner {
                    // Text: pytanie + linie multiline contentu + hint line
                    // Wrap width = inner_width - 2 (odejmujemy "> " prefix)
                    InnerState::Text(state) => {
                        let content_width = inner_width.saturating_sub(2).max(1);
                        let vl_count = if state.buffer().is_empty() {
                            1u16
                        } else {
                            let mut tmp = state.clone();
                            tmp.set_wrap_width(content_width);
                            tmp.visual_lines().len().max(1) as u16
                        };
                        question_lines + vl_count + 1 // +1 dla hint line
                    }
                    // Choice: pytanie + opcje + wiersz "Other"
                    //         + 2 dodatkowe (text input + hint) gdy tryb "Other" aktywny
                    InnerState::Choice(_, other_input) => {
                        let base = question_lines + self.question.options.len() as u16 + 1;
                        if other_input.is_some() {
                            base + 2
                        } else {
                            base
                        }
                    }
                    // Multi: pytanie + opcje + hint line
                    InnerState::Multi(state) => question_lines + state.options.len() as u16 + 1,
                    // Confirm: pytanie + buttons line
                    InnerState::Confirm(_) => question_lines + 1,
                };
                // border_top/title(1) + padding.top(1) + content + padding.bottom(1)
                content_height + 3
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

    /// Dynamiczna wysokość widgetu — backward-compat alias dla `full_height_for_width()`.
    ///
    /// Używany przez testy i kod który nie potrzebuje cappowania.
    pub fn height_for_width(&self, width: u16) -> u16 {
        self.full_height_for_width(width)
    }

    /// Wysokość widgetu ograniczona do `max_height` wierszy.
    ///
    /// Gdy treść jest wyższa niż `max_height`, widget scrolluje.
    /// Używany przez `command_app.rs` przy obliczaniu miejsca w layoutcie.
    pub fn height_for_width_capped(&self, width: u16, max_height: u16) -> u16 {
        self.full_height_for_width(width).min(max_height)
    }

    /// Scrolluje widok o `n` wierszy w górę (odsłania wcześniejszą treść).
    pub fn scroll_up(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scrolluje widok o `n` wierszy w dół (z clampingiem do końca treści).
    pub fn scroll_down(&mut self, n: u16, width: u16, max_height: u16) {
        let full = self.full_height_for_width(width);
        let max_offset = full.saturating_sub(max_height);
        self.scroll_offset = (self.scroll_offset + n).min(max_offset);
    }

    /// Resetuje scroll do góry (używane przy przejściu do nowego pytania).
    pub fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
    }

    /// Obsługuje zdarzenie klawiatury w stanie Active.
    ///
    /// Przyjmuje `KeyEvent` (z modyfikatorami) — konieczne dla obsługi Shift+Enter
    /// w trybie multiline text input.
    ///
    /// Zwraca `AskUserAction`:
    /// - `Continue` — klawisz obsłużony, kontynuuj interakcję
    /// - `Submit(answer)` — użytkownik zaakceptował odpowiedź (Enter)
    /// - `Cancel` — użytkownik anulował (Esc)
    pub fn handle_key(&mut self, key: KeyEvent) -> AskUserAction {
        let inner = match &mut self.state {
            AskUserState::Active(inner) => inner,
            AskUserState::Answered(_) => return AskUserAction::Continue,
        };

        // Esc → anuluj (wspólne dla wszystkich typów)
        // Wyjątek: gdy jesteśmy w trybie "Other" text input, Esc musi dotrzeć
        // do handle_choice_key żeby wrócić do listy (nie anulować całego widgetu)
        let in_other_mode = matches!(inner, InnerState::Choice(_, Some(_)));
        if key.code == KeyCode::Esc && !in_other_mode {
            return AskUserAction::Cancel;
        }

        match inner {
            // Deleguj cały KeyEvent do multiline handler (obsługuje Shift+Enter)
            InnerState::Text(state) => handle_multiline_text_key(state, key),
            // Pozostałe typy obsługują tylko KeyCode (nie potrzebują modyfikatorów)
            InnerState::Choice(state, other_input) => {
                handle_choice_key(state, other_input, &self.question.options, key.code)
            }
            InnerState::Multi(state) => handle_multi_key(state, key.code),
            InnerState::Confirm(state) => handle_confirm_key(state, key.code),
        }
    }

    /// Obsługuje kliknięcie myszą.
    ///
    /// Deleguje do powidgetu dla Confirm (klik na [Yes]/[No]).
    /// Inne typy pytań nie obsługują kliknięć (ignorują).
    ///
    /// `area` — obszar, w którym widget jest renderowany (pełny rect z ramką).
    ///
    /// Returns `AskUserAction`:
    /// - `Submit(answer)` — klik na przycisk Yes/No
    /// - `Continue` — klik poza przyciskami lub nie-Confirm type
    pub fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) -> AskUserAction {
        let AskUserState::Active(InnerState::Confirm(ref mut state)) = self.state else {
            return AskUserAction::Continue;
        };

        // Odtwarzamy layout z render_widget_inner dla stanu Active:
        // - Borders::TOP: 1 wiersz border na górze
        // - Padding::new(left=2, right=2, top=1, bottom=1)
        // inner_y = area.y + border_top(1) + padding_top(1) = area.y + 2
        // inner_x = area.x + padding_left(2) = area.x + 2
        // inner_width = area.width - padding_left(2) - padding_right(2)
        let inner_x = area.x.saturating_add(2);
        let inner_y = area.y.saturating_add(2);
        let inner_width = area.width.saturating_sub(4).max(1) as usize;

        // Wysokość pytania — liczba wierszy po markdown renderingu
        let q_height =
            count_rendered_lines_for_width(self.question.question.trim_end(), inner_width);

        // Area przycisków = pierwszy wiersz content area (bezpośrednio po pytaniu).
        // Uwzględniamy scroll_offset: przy scrollowaniu w dół treść przesuwa się ku górze,
        // więc efektywna pozycja przycisków na ekranie = inner_y + q_height - scroll_offset.
        let button_y = inner_y
            .saturating_add(q_height)
            .saturating_sub(self.scroll_offset);

        // Jeśli przyciski zostały wyprzesunięte poza widoczny obszar — ignoruj klik
        let widget_bottom = area.y.saturating_add(area.height);
        if button_y < inner_y || button_y >= widget_bottom {
            return AskUserAction::Continue;
        }

        let button_area = Rect {
            x: inner_x,
            y: button_y,
            width: inner_width as u16,
            height: 1,
        };

        match state.handle_mouse(mouse, button_area) {
            Some(InputAction::Send(answer)) => AskUserAction::Submit(answer),
            _ => AskUserAction::Continue,
        }
    }

    /// Przechodzi do stanu Answered z podaną odpowiedzią
    pub fn set_answered(&mut self, answer: String) {
        self.state = AskUserState::Answered(answer);
    }
}

// ── Key handlers per question type ─────────────────────────────────

/// Obsługuje key event dla multiline text input.
///
/// Deleguje do `MultilineTextInputState::handle_key_event`:
/// - Shift+Enter → wstawia newline (nie submituje)
/// - Enter → submituje (zwraca zawartość bufora)
/// - Pozostałe → edycja / nawigacja
fn handle_multiline_text_key(state: &mut MultilineTextInputState, key: KeyEvent) -> AskUserAction {
    match state.handle_key_event(key) {
        Some(value) => AskUserAction::Submit(value),
        None => AskUserAction::Continue,
    }
}

fn handle_choice_key(
    state: &mut ChoiceState,
    other_input: &mut Option<TextInputState>,
    options: &[QuestionOption],
    key: KeyCode,
) -> AskUserAction {
    // Tryb "Other": user wybrał "Other" i teraz wpisuje tekst
    if let Some(text_state) = other_input {
        return match key {
            KeyCode::Enter => AskUserAction::Submit(text_state.value().to_string()),
            KeyCode::Esc => {
                *other_input = None; // wróć do listy opcji
                AskUserAction::Continue
            }
            KeyCode::Backspace => {
                text_state.delete_char();
                AskUserAction::Continue
            }
            KeyCode::Left => {
                text_state.move_cursor_left();
                AskUserAction::Continue
            }
            KeyCode::Right => {
                text_state.move_cursor_right();
                AskUserAction::Continue
            }
            KeyCode::Home => {
                text_state.move_cursor_home();
                AskUserAction::Continue
            }
            KeyCode::End => {
                text_state.move_cursor_end();
                AskUserAction::Continue
            }
            KeyCode::Char(c) => {
                text_state.insert_char(c);
                AskUserAction::Continue
            }
            _ => AskUserAction::Continue,
        };
    }

    // Normalny tryb: nawigacja po N opcjach + wirtualny slot "Other" (indeks N)
    let total = options.len() + 1;
    match key {
        KeyCode::Up => {
            state.move_up(total);
            AskUserAction::Continue
        }
        KeyCode::Down => {
            state.move_down(total);
            AskUserAction::Continue
        }
        KeyCode::Enter => {
            if state.selected == options.len() {
                // "Other" — aktywuj text input
                *other_input = Some(TextInputState::new(None));
                AskUserAction::Continue
            } else if let Some(label) = state.get_selected_label(options) {
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

        let full_height = self.full_height_for_width(area.width);
        let scroll_offset = self.scroll_offset;
        let is_active = matches!(self.state, AskUserState::Active(_));

        // Scroll indicators w tytule
        let can_scroll_up = scroll_offset > 0;
        let can_scroll_down = scroll_offset + area.height < full_height;
        let title = build_title(is_active, can_scroll_up, can_scroll_down);

        // Destrukturyzacja — potrzebujemy `mut state` dla MultilineTextInputWidget (StatefulWidget)
        let AskUserWidget {
            question,
            mut state,
            ..
        } = self;

        if full_height <= area.height {
            // Treść mieści się — renderuj bezpośrednio (bez virtual buffera)
            render_widget_inner(&question, &mut state, &title, area, buf);
            return;
        }

        // Stwórz virtual buffer o pełnej wymaganej wysokości (zaczyna od 0,0)
        let virt_rect = Rect {
            x: 0,
            y: 0,
            width: area.width,
            height: full_height,
        };
        let mut virt_buf = Buffer::empty(virt_rect);
        render_widget_inner(&question, &mut state, &title, virt_rect, &mut virt_buf);

        // Kopiuj slice [scroll_offset .. scroll_offset + area.height] do realnego buf
        let copy_rows = area.height.min(full_height.saturating_sub(scroll_offset));
        for row in 0..copy_rows {
            let src_y = scroll_offset + row;
            let dst_y = area.y + row;
            for col in 0..area.width {
                if let Some(src_cell) = virt_buf.cell((col, src_y))
                    && let Some(dst_cell) = buf.cell_mut((area.x + col, dst_y))
                {
                    *dst_cell = src_cell.clone();
                }
            }
        }

        // Scrollbar (VerticalRight) — renderowany na real buf po skopiowaniu treści
        let max_scroll = full_height.saturating_sub(area.height) as usize;
        let mut sb_state = ScrollbarState::default()
            .content_length(max_scroll + 1)
            .viewport_content_length(area.height as usize)
            .position(scroll_offset as usize);
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

// ── Render helpers ─────────────────────────────────────────────────

/// Buduje tytuł bloku z opcjonalnymi wskaźnikami scrollowania.
fn build_title(is_active: bool, can_up: bool, can_down: bool) -> String {
    if is_active {
        let up = if can_up { " ▲" } else { "" };
        let down = if can_down { " ▼" } else { "" };
        format!(" Question{}{} ", up, down)
    } else {
        " Answered ".to_string()
    }
}

/// Renderuje widget (Block + treść) do podanego buffera.
///
/// Wywoływany zarówno przy renderowaniu bezpośrednim jak i przez virtual buffer.
/// Przyjmuje `state: &mut AskUserState` — konieczne dla `MultilineTextInputWidget`
/// (StatefulWidget), który aktualizuje wrap_width i viewport_height przy renderowaniu.
fn render_widget_inner(
    question: &AskUserQuestion,
    state: &mut AskUserState,
    title: &str,
    area: Rect,
    buf: &mut Buffer,
) {
    let theme = &DEFAULT_THEME;
    let is_active = matches!(state, AskUserState::Active(_));

    let title_style = if is_active {
        theme.header_style()
    } else {
        theme.muted_style()
    };

    let block = if is_active {
        Block::default()
            .borders(Borders::TOP)
            .border_style(theme.secondary_style())
            .padding(Padding::new(2, 2, 1, 1))
            .style(Style::default().bg(theme.panel_bg(true)))
            .title(Span::styled(title.to_string(), title_style))
    } else {
        Block::default()
            .padding(Padding::uniform(1))
            .style(Style::default().bg(theme.panel_bg(false)))
            .title(Span::styled(title.to_string(), title_style))
    };

    let inner_area = block.inner(area);
    block.render(area, buf);

    match state {
        AskUserState::Active(inner) => {
            render_active(inner, question, inner_area, buf);
        }
        AskUserState::Answered(answer) => {
            render_answered(&question.question, answer, inner_area, buf);
        }
    }
}

/// Renderuje aktywny stan z odpowiednim sub-widgetem
fn render_active(inner: &mut InnerState, question: &AskUserQuestion, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    // Normalizuj trailing whitespace — zapobiega dodatkowym pustym liniom z Claude
    let question_text =
        render_markdown_for_width(question.question.trim_end(), area.width as usize);
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

    // Content area bezpośrednio po pytaniu (bez separatora)
    let content_y = area.y + q_height;
    let content_height = area.height.saturating_sub(q_height);
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
        InnerState::Choice(state, other_input) => {
            render_choice_active(state, other_input, question, content_area, buf);
        }
        InnerState::Multi(state) => render_multi_active(state, content_area, buf),
        InnerState::Confirm(state) => render_confirm_active(state, content_area, buf),
    }
}

/// Renderuje multiline text input: `> ` + content z kursorem + hint line.
///
/// Używa `MultilineTextInput` (Widget z prefiksem `> `) do renderowania pola tekstowego.
/// Przyjmuje `&mut state` — architekturonicznie poprawne, umożliwia przyszłą optymalizację.
/// Hint line informuje o Shift+Enter (newline) i Enter (submit).
fn render_text_active(
    state: &mut MultilineTextInputState,
    question: &AskUserQuestion,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.height == 0 {
        return;
    }

    // Input zajmuje wszystkie wiersze oprócz ostatniego (hint line)
    let input_height = area.height.saturating_sub(1).max(1);
    let input_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: input_height,
    };

    // Buduj widget z opcjonalnym placeholderem z pytania
    // MultilineTextInput renderuje z prefiksem "> " (2 kolumny) na pierwszej linii
    let mut input_widget = MultilineTextInput::new(state.clone());
    if let Some(ref placeholder) = question.placeholder {
        input_widget = input_widget.with_placeholder(placeholder.clone());
    }
    input_widget.render(input_area, buf);

    // Hint line poniżej pola input
    if area.height > 1 {
        let hint = Line::from(Span::styled(
            "Enter: submit │ Shift+Enter: newline │ Esc: cancel",
            DEFAULT_THEME.muted_style(),
        ));
        buf.set_line(area.x, area.y + input_height, &hint, area.width);
    }
}

/// Renderuje sam wiersz text input (kursor + buffer) bez hint line.
/// Używany przez render_choice_active (tryb "Other") dla single-line input.
fn render_text_input_line(state: &TextInputState, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }

    let mut spans = Vec::new();
    spans.push(Span::styled("> ", DEFAULT_THEME.header_style()));

    if state.buffer.is_empty() {
        spans.push(Span::styled("█", DEFAULT_THEME.primary_style()));
    } else {
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

        if cursor_pos == chars.len() {
            spans.push(Span::styled("█", DEFAULT_THEME.primary_style()));
        }
    }

    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
}

/// Renderuje choice select: radio buttons z opcjami + wirtualna opcja "Other"
fn render_choice_active(
    state: &ChoiceState,
    other_input: &Option<TextInputState>,
    question: &AskUserQuestion,
    area: Rect,
    buf: &mut Buffer,
) {
    let options = &question.options;

    // Renderuj predefiniowane opcje
    for (idx, option) in options.iter().enumerate() {
        let y = area.y + idx as u16;
        if y >= area.y + area.height {
            break;
        }

        let is_selected = idx == state.selected && other_input.is_none();
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

    // Renderuj wirtualną opcję "Other"
    let other_y = area.y + options.len() as u16;
    if other_y < area.y + area.height {
        let is_other_selected = state.selected == options.len() && other_input.is_none();
        let is_other_active = other_input.is_some();
        let radio = if is_other_selected || is_other_active {
            "●"
        } else {
            "○"
        };
        let style = if is_other_selected || is_other_active {
            Style::default()
                .fg(DEFAULT_THEME.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            DEFAULT_THEME.muted_style()
        };

        let spans = vec![
            Span::styled(radio, style),
            Span::raw(" "),
            Span::styled("Other (type your answer...)", style),
        ];
        buf.set_line(area.x, other_y, &Line::from(spans), area.width);
    }

    // Jeśli tryb "Other" aktywny — renderuj text input i hint
    if let Some(text_state) = other_input {
        let text_y = area.y + options.len() as u16 + 1;
        if text_y < area.y + area.height {
            let text_area = Rect {
                x: area.x,
                y: text_y,
                width: area.width,
                height: 1,
            };
            render_text_input_line(text_state, text_area, buf);
        }

        let hint_y = area.y + options.len() as u16 + 2;
        if hint_y < area.y + area.height {
            let hint = Line::from(Span::styled(
                "Enter: submit │ Esc: back to list",
                DEFAULT_THEME.muted_style(),
            ));
            buf.set_line(area.x, hint_y, &hint, area.width);
        }
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
    use crossterm::event::{KeyEventKind, KeyEventState, KeyModifiers};

    use super::*;
    use crate::test_helpers::{render_widget_to_buffer, snap};

    // ── Test helpers ──────────────────────────────────────────────

    /// Tworzy KeyEvent z KeyCode bez modyfikatorów (helper dla testów).
    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// Tworzy KeyEvent z KeyCode i modyfikatorem SHIFT.
    fn k_shift(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

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
            AskUserState::Active(InnerState::Choice(_, None))
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
        if let AskUserState::Active(InnerState::Choice(state, _)) = &widget.state {
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
        if let AskUserState::Active(InnerState::Choice(state, _)) = &widget.state {
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
        if let AskUserState::Active(InnerState::Choice(state, _)) = &widget.state {
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
        // border_top/title(1) + padding.top(1) + question(1) + input(1) + hint(1) + padding.bottom(1) = 6
        assert_eq!(widget.height_for_width(50), 6);
    }

    #[test]
    fn test_height_choice_active() {
        let widget = AskUserWidget::new(choice_question());
        // border_top/title(1) + padding.top(1) + question(1) + 2 opcje + "Other"(1) + padding.bottom(1) = 7
        assert_eq!(widget.height_for_width(50), 7);
    }

    #[test]
    fn test_height_multi_active() {
        let widget = AskUserWidget::new(multi_question());
        // border_top/title(1) + padding.top(1) + question(1) + 3 opcje + hint(1) + padding.bottom(1) = 8
        assert_eq!(widget.height_for_width(50), 8);
    }

    #[test]
    fn test_height_confirm_active() {
        let widget = AskUserWidget::new(confirm_question());
        // border_top/title(1) + padding.top(1) + question(1) + buttons(1) + padding.bottom(1) = 5
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
        assert_eq!(widget.handle_key(k(KeyCode::Esc)), AskUserAction::Cancel);
    }

    #[test]
    fn test_handle_key_shift_enter_inserts_newline() {
        let mut widget = AskUserWidget::new(text_question());
        // Wpisz "line1", Shift+Enter, "line2", Enter → submit "line1\nline2"
        for ch in "line1".chars() {
            widget.handle_key(k(KeyCode::Char(ch)));
        }
        // Shift+Enter → newline (nie submit)
        assert_eq!(
            widget.handle_key(k_shift(KeyCode::Enter)),
            AskUserAction::Continue
        );
        for ch in "line2".chars() {
            widget.handle_key(k(KeyCode::Char(ch)));
        }
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("line1\nline2".into())
        );
    }

    #[test]
    fn test_handle_key_enter_submits_not_newline() {
        let mut widget = AskUserWidget::new(text_question());
        // Enter bez Shift → submit (nie wstawia newline)
        widget.handle_key(k(KeyCode::Char('A')));
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("A".into())
        );
    }

    #[test]
    fn test_handle_key_text_multiline_height_grows() {
        let mut widget = AskUserWidget::new(text_question());
        let h1 = widget.height_for_width(50);

        // Wpisz tekst + Shift+Enter → wysokość powinna wzrosnąć
        for ch in "line1".chars() {
            widget.handle_key(k(KeyCode::Char(ch)));
        }
        widget.handle_key(k_shift(KeyCode::Enter));

        let h2 = widget.height_for_width(50);
        assert!(h2 > h1, "Wysokość powinna wzrosnąć po dodaniu nowej linii");
    }

    #[test]
    fn test_handle_key_text_enter_submits() {
        let mut widget = AskUserWidget::new(text_question());
        widget.handle_key(k(KeyCode::Char('A')));
        widget.handle_key(k(KeyCode::Char('l')));
        widget.handle_key(k(KeyCode::Char('i')));
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("Ali".into())
        );
    }

    #[test]
    fn test_handle_key_text_empty_submit() {
        let mut widget = AskUserWidget::new(text_question());
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("".into())
        );
    }

    #[test]
    fn test_handle_key_text_backspace() {
        let mut widget = AskUserWidget::new(text_question());
        widget.handle_key(k(KeyCode::Char('A')));
        widget.handle_key(k(KeyCode::Char('B')));
        widget.handle_key(k(KeyCode::Backspace));
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("A".into())
        );
    }

    #[test]
    fn test_handle_key_text_cursor_movement() {
        let mut widget = AskUserWidget::new(text_question());
        widget.handle_key(k(KeyCode::Char('A')));
        widget.handle_key(k(KeyCode::Char('B')));
        widget.handle_key(k(KeyCode::Char('C')));
        // Kursor na Home → 0, wstaw D na początku
        widget.handle_key(k(KeyCode::Home));
        widget.handle_key(k(KeyCode::Char('D')));
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("DABC".into())
        );
    }

    #[test]
    fn test_handle_key_choice_navigate_and_submit() {
        let mut widget = AskUserWidget::new(choice_question());
        assert_eq!(widget.handle_key(k(KeyCode::Down)), AskUserAction::Continue);
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("Session".into())
        );
    }

    #[test]
    fn test_handle_key_choice_submit_first() {
        let mut widget = AskUserWidget::new(choice_question());
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("JWT".into())
        );
    }

    #[test]
    fn test_handle_key_choice_wrap_up() {
        let mut widget = AskUserWidget::new(choice_question()); // JWT(0), Session(1), Other(2)
        // Na JWT (0), Up → wrap do "Other" (indeks 2 = options.len())
        widget.handle_key(k(KeyCode::Up));
        if let AskUserState::Active(InnerState::Choice(state, _)) = &widget.state {
            assert_eq!(state.selected, 2); // "Other" jest na indeksie 2
        } else {
            panic!("Expected Active Choice state");
        }
    }

    // ── Choice "Other" option tests ───────────────────────────────

    #[test]
    fn test_handle_key_choice_other_is_last_option() {
        // "Other" jest za ostatnią opcją (indeks N)
        let mut widget = AskUserWidget::new(choice_question()); // 2 opcje: JWT, Session
        // Down 2x → "Other" (indeks 2)
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Down));
        // Stan choice.selected == 2 (options.len())
        if let AskUserState::Active(InnerState::Choice(state, _)) = &widget.state {
            assert_eq!(state.selected, 2);
        } else {
            panic!("Expected Active Choice state");
        }
    }

    #[test]
    fn test_handle_key_choice_enter_on_other_activates_text_input() {
        let mut widget = AskUserWidget::new(choice_question());
        // Down 2x → "Other"
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Down));
        // Enter → aktywuje text input
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Continue
        );
        // Sprawdź że other_input jest Some
        if let AskUserState::Active(InnerState::Choice(_, other_input)) = &widget.state {
            assert!(other_input.is_some());
        } else {
            panic!("Expected Active Choice state");
        }
    }

    #[test]
    fn test_handle_key_choice_other_text_input_submit() {
        let mut widget = AskUserWidget::new(choice_question());
        // Down 2x → "Other", Enter → aktywuje text input
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Enter));
        // Wpisz "custom answer"
        for ch in "custom".chars() {
            widget.handle_key(k(KeyCode::Char(ch)));
        }
        // Enter → Submit z wpisanym tekstem
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("custom".into())
        );
    }

    #[test]
    fn test_handle_key_choice_other_esc_returns_to_list() {
        let mut widget = AskUserWidget::new(choice_question());
        // Przejdź do "Other" i aktywuj text input
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Enter));
        // Esc → powrót do listy (nie Cancel!)
        assert_eq!(widget.handle_key(k(KeyCode::Esc)), AskUserAction::Continue);
        // other_input powinno być None
        if let AskUserState::Active(InnerState::Choice(_, other_input)) = &widget.state {
            assert!(other_input.is_none());
        } else {
            panic!("Expected Active Choice state");
        }
        // Widget nadal aktywny (nie anulowany)
        assert!(widget.is_active());
    }

    #[test]
    fn test_handle_key_choice_other_height_expands() {
        let mut widget = AskUserWidget::new(choice_question());
        let base_height = widget.height_for_width(50);
        // Down 2x → "Other", Enter
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Enter));
        // height powinno wzrosnąć o 2 (text input + hint)
        assert_eq!(widget.height_for_width(50), base_height + 2);
    }

    #[test]
    fn test_handle_key_choice_wrap_includes_other() {
        let mut widget = AskUserWidget::new(choice_question()); // JWT, Session + Other
        // Up z indeksu 0 → wrap do "Other" (indeks 2)
        widget.handle_key(k(KeyCode::Up));
        if let AskUserState::Active(InnerState::Choice(state, _)) = &widget.state {
            assert_eq!(state.selected, 2); // "Other" jako ostatni
        } else {
            panic!("Expected Active Choice state");
        }
    }

    #[test]
    fn test_handle_key_choice_other_text_backspace_and_submit() {
        let mut widget = AskUserWidget::new(choice_question());
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Enter));
        // Wpisz "abc", usuń 'c', dodaj 'd'
        widget.handle_key(k(KeyCode::Char('a')));
        widget.handle_key(k(KeyCode::Char('b')));
        widget.handle_key(k(KeyCode::Char('c')));
        widget.handle_key(k(KeyCode::Backspace));
        widget.handle_key(k(KeyCode::Char('d')));
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("abd".into())
        );
    }

    #[test]
    fn test_handle_key_choice_other_empty_submit() {
        let mut widget = AskUserWidget::new(choice_question());
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Enter));
        // Submit bez wpisywania czegokolwiek → pusty string
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("".into())
        );
    }

    #[test]
    fn test_handle_key_choice_normal_options_still_work() {
        // Upewnij się że normalne opcje nadal działają po dodaniu "Other"
        let mut widget = AskUserWidget::new(choice_question());
        // JWT (domyślne, indeks 0) → Enter
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("JWT".into())
        );

        let mut widget2 = AskUserWidget::new(choice_question());
        // Down → Session → Enter
        widget2.handle_key(k(KeyCode::Down));
        assert_eq!(
            widget2.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("Session".into())
        );
    }

    #[test]
    fn test_handle_key_choice_main_esc_still_cancels() {
        // Esc w trybie normalnym (nie text input) nadal cancels
        let mut widget = AskUserWidget::new(choice_question());
        assert_eq!(widget.handle_key(k(KeyCode::Esc)), AskUserAction::Cancel);
    }

    #[test]
    fn test_handle_key_multi_toggle_and_submit() {
        let mut widget = AskUserWidget::new(multi_question());
        widget.handle_key(k(KeyCode::Char(' ')));
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Char(' ')));
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("Auth, API".into())
        );
    }

    #[test]
    fn test_handle_key_multi_empty_submit() {
        let mut widget = AskUserWidget::new(multi_question());
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("".into())
        );
    }

    #[test]
    fn test_handle_key_confirm_yes() {
        let mut widget = AskUserWidget::new(confirm_question());
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("yes".into())
        );
    }

    #[test]
    fn test_handle_key_confirm_no() {
        let mut widget = AskUserWidget::new(confirm_question());
        widget.handle_key(k(KeyCode::Char('n')));
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("no".into())
        );
    }

    #[test]
    fn test_handle_key_confirm_toggle() {
        let mut widget = AskUserWidget::new(confirm_question());
        widget.handle_key(k(KeyCode::Tab));
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("no".into())
        );
    }

    #[test]
    fn test_handle_key_confirm_y_shortcut() {
        let mut widget = AskUserWidget::new(confirm_question());
        // Start na yes, przejdź na no, potem y
        widget.handle_key(k(KeyCode::Char('n')));
        widget.handle_key(k(KeyCode::Char('y')));
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("yes".into())
        );
    }

    #[test]
    fn test_handle_key_on_answered_is_noop() {
        let mut widget = AskUserWidget::answered(text_question(), "Alice".into());
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Continue
        );
        assert_eq!(widget.handle_key(k(KeyCode::Esc)), AskUserAction::Continue);
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
            widget.handle_key(k(KeyCode::Char('A'))),
            AskUserAction::Continue
        );
        assert_eq!(
            widget.handle_key(k(KeyCode::Char('l'))),
            AskUserAction::Continue
        );
        assert_eq!(
            widget.handle_key(k(KeyCode::Char('i'))),
            AskUserAction::Continue
        );
        assert_eq!(
            widget.handle_key(k(KeyCode::Char('c'))),
            AskUserAction::Continue
        );
        assert_eq!(
            widget.handle_key(k(KeyCode::Char('e'))),
            AskUserAction::Continue
        );

        // Submit
        let result = widget.handle_key(k(KeyCode::Enter));
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
        widget.handle_key(k(KeyCode::Char('A')));
        widget.handle_key(k(KeyCode::Char('l')));
        widget.handle_key(k(KeyCode::Char('i')));
        widget.handle_key(k(KeyCode::Char('c')));
        widget.handle_key(k(KeyCode::Char('e')));
        widget.handle_key(k(KeyCode::Char('e')));

        // Cofnij jedno "e"
        assert_eq!(
            widget.handle_key(k(KeyCode::Backspace)),
            AskUserAction::Continue
        );

        // Submit
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("Alice".into())
        );
    }

    #[test]
    fn test_integration_text_input_cursor_navigation() {
        let mut widget = AskUserWidget::new(text_question());

        // Wpisz "AC"
        widget.handle_key(k(KeyCode::Char('A')));
        widget.handle_key(k(KeyCode::Char('C')));

        // Home → kursor na początek
        widget.handle_key(k(KeyCode::Home));

        // Right → kursor na pozycję 1
        widget.handle_key(k(KeyCode::Right));

        // Wstaw 'B' między A i C
        widget.handle_key(k(KeyCode::Char('B')));

        // Submit → "ABC"
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("ABC".into())
        );
    }

    #[test]
    fn test_integration_text_input_cancel() {
        let mut widget = AskUserWidget::new(text_question());

        widget.handle_key(k(KeyCode::Char('T')));
        widget.handle_key(k(KeyCode::Char('e')));
        widget.handle_key(k(KeyCode::Char('s')));
        widget.handle_key(k(KeyCode::Char('t')));

        // Esc → anuluj
        assert_eq!(widget.handle_key(k(KeyCode::Esc)), AskUserAction::Cancel);
    }

    #[test]
    fn test_integration_choice_navigate_and_submit() {
        let mut widget = AskUserWidget::new(choice_question());

        // Start na JWT (default, index 0)
        // Down → Session (index 1)
        assert_eq!(widget.handle_key(k(KeyCode::Down)), AskUserAction::Continue);

        // Snapshot z zaznaczonym Session
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget.clone(), 60, h);
        insta::assert_snapshot!(snap(&buffer));

        // Submit Session
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("Session".into())
        );
    }

    #[test]
    fn test_integration_choice_wrap_around() {
        let mut widget = AskUserWidget::new(choice_question()); // JWT(0), Session(1), Other(2)

        // Start na JWT (0)
        // Up → wrap do "Other" (indeks 2 = last slot)
        widget.handle_key(k(KeyCode::Up));
        // Enter na "Other" → aktywuje text input
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Continue
        );
        if let AskUserState::Active(InnerState::Choice(_, other_input)) = &widget.state {
            assert!(other_input.is_some()); // "Other" text mode aktywny
        } else {
            panic!("Expected Active Choice state");
        }
    }

    #[test]
    fn test_integration_choice_cancel() {
        let mut widget = AskUserWidget::new(choice_question());

        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Down));

        assert_eq!(widget.handle_key(k(KeyCode::Esc)), AskUserAction::Cancel);
    }

    #[test]
    fn test_integration_multi_toggle_and_submit() {
        let mut widget = AskUserWidget::new(multi_question());

        // Zaznacz Auth (index 0)
        assert_eq!(
            widget.handle_key(k(KeyCode::Char(' '))),
            AskUserAction::Continue
        );

        // Down → API (index 1)
        assert_eq!(widget.handle_key(k(KeyCode::Down)), AskUserAction::Continue);

        // Zaznacz API
        assert_eq!(
            widget.handle_key(k(KeyCode::Char(' '))),
            AskUserAction::Continue
        );

        // Down → Logging (index 2)
        widget.handle_key(k(KeyCode::Down));

        // Zaznacz Logging
        widget.handle_key(k(KeyCode::Char(' ')));

        // Snapshot z zaznaczonymi Auth, API, Logging (kursor na Logging)
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget.clone(), 60, h);
        insta::assert_snapshot!(snap(&buffer));

        // Submit
        let result = widget.handle_key(k(KeyCode::Enter));
        assert_eq!(result, AskUserAction::Submit("Auth, API, Logging".into()));
    }

    #[test]
    fn test_integration_multi_toggle_deselect() {
        let mut widget = AskUserWidget::new(multi_question());

        // Zaznacz Auth
        widget.handle_key(k(KeyCode::Char(' ')));

        // Down → API
        widget.handle_key(k(KeyCode::Down));

        // Zaznacz API
        widget.handle_key(k(KeyCode::Char(' ')));

        // Up → Auth
        widget.handle_key(k(KeyCode::Up));

        // Odznacz Auth (drugi Space na tym samym indeksie)
        widget.handle_key(k(KeyCode::Char(' ')));

        // Submit → tylko "API"
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("API".into())
        );
    }

    #[test]
    fn test_integration_multi_submit_empty() {
        let mut widget = AskUserWidget::new(multi_question());

        // Submit bez zaznaczania czegokolwiek
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("".into())
        );
    }

    #[test]
    fn test_integration_multi_cancel() {
        let mut widget = AskUserWidget::new(multi_question());

        widget.handle_key(k(KeyCode::Char(' ')));
        widget.handle_key(k(KeyCode::Down));
        widget.handle_key(k(KeyCode::Char(' ')));

        assert_eq!(widget.handle_key(k(KeyCode::Esc)), AskUserAction::Cancel);
    }

    #[test]
    fn test_integration_confirm_toggle_and_submit() {
        let mut widget = AskUserWidget::new(confirm_question());

        // Start na Yes (default)
        // Tab → toggle do No
        assert_eq!(widget.handle_key(k(KeyCode::Tab)), AskUserAction::Continue);

        // Snapshot z zaznaczonym No
        let h = widget.height_for_width(50);
        let buffer = render_widget_to_buffer(widget.clone(), 50, h);
        insta::assert_snapshot!(snap(&buffer));

        // Submit No
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("no".into())
        );
    }

    #[test]
    fn test_integration_confirm_keyboard_shortcuts() {
        let mut widget = AskUserWidget::new(confirm_question());

        // Start na Yes
        // Naciśnij 'n' → przejdź na No
        widget.handle_key(k(KeyCode::Char('n')));

        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("no".into())
        );

        // Nowy widget
        let mut widget2 = AskUserWidget::new(confirm_question());

        // Naciśnij 'y' (lub 'Y')
        widget2.handle_key(k(KeyCode::Char('Y')));

        assert_eq!(
            widget2.handle_key(k(KeyCode::Enter)),
            AskUserAction::Submit("yes".into())
        );
    }

    #[test]
    fn test_integration_confirm_cancel() {
        let mut widget = AskUserWidget::new(confirm_question());

        widget.handle_key(k(KeyCode::Tab));

        assert_eq!(widget.handle_key(k(KeyCode::Esc)), AskUserAction::Cancel);
    }

    #[test]
    fn test_integration_answered_no_interaction() {
        let mut widget = AskUserWidget::answered(text_question(), "Bob".into());

        // W stanie Answered wszystkie klawisze są ignorowane
        assert_eq!(
            widget.handle_key(k(KeyCode::Enter)),
            AskUserAction::Continue
        );
        assert_eq!(
            widget.handle_key(k(KeyCode::Char('x'))),
            AskUserAction::Continue
        );
        assert_eq!(widget.handle_key(k(KeyCode::Esc)), AskUserAction::Continue);

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
        widget.handle_key(k(KeyCode::Char('T')));
        widget.handle_key(k(KeyCode::Char('e')));
        widget.handle_key(k(KeyCode::Char('s')));
        widget.handle_key(k(KeyCode::Char('t')));

        // Submit → dostajemy Answer
        let result = widget.handle_key(k(KeyCode::Enter));
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

    // ── handle_mouse Tests ─────────────────────────────────────────

    fn make_left_click(col: u16, row: u16) -> MouseEvent {
        use crossterm::event::{MouseButton, MouseEventKind};
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Layout widgetu: border_top(1) + padding_top(1) = inner_y = area.y + 2
    /// Pytanie "Are you sure?" = 1 wiersz → button_y = inner_y + 1 = area.y + 3
    #[test]
    fn test_handle_mouse_confirm_click_yes() {
        let mut widget = AskUserWidget::new(confirm_question());
        // area = (x=0, y=0, w=80, h=20)
        // inner_y = 2, q_height = 1 → button_y = 3
        // button_x = inner_x = 2, yes_rect = x=2..6
        let area = Rect { x: 0, y: 0, width: 80, height: 20 };
        let result = widget.handle_mouse(make_left_click(3, 3), area);
        assert_eq!(result, AskUserAction::Submit("yes".into()));
    }

    #[test]
    fn test_handle_mouse_confirm_click_no() {
        let mut widget = AskUserWidget::new(confirm_question());
        // no_rect = x = inner_x + 7 = 9, width=4 → x=9..12
        let area = Rect { x: 0, y: 0, width: 80, height: 20 };
        let result = widget.handle_mouse(make_left_click(10, 3), area);
        assert_eq!(result, AskUserAction::Submit("no".into()));
    }

    #[test]
    fn test_handle_mouse_confirm_click_outside_ignored() {
        let mut widget = AskUserWidget::new(confirm_question());
        let area = Rect { x: 0, y: 0, width: 80, height: 20 };
        // Klik poza przyciskami (x=50)
        let result = widget.handle_mouse(make_left_click(50, 3), area);
        assert_eq!(result, AskUserAction::Continue);
    }

    #[test]
    fn test_handle_mouse_confirm_scroll_offset_shifts_buttons() {
        let mut widget = AskUserWidget::new(confirm_question());
        let area = Rect { x: 0, y: 0, width: 80, height: 20 };

        // scroll_offset=1 → button_y = inner_y(2) + q_height(1) - scroll_offset(1) = 2
        // Ustawiamy bezpośrednio — scroll_down klampuje do full_height - max_height,
        // więc przy małej treści i dużym obszarze nie scrolluje.
        widget.scroll_offset = 1;

        // Klik na starą pozycję (row=3) → teraz poza przyciskami (przyciski są w row=2)
        let result_old = widget.handle_mouse(make_left_click(3, 3), area);
        assert_eq!(
            result_old,
            AskUserAction::Continue,
            "Klik na starą pozycję po scrollu powinien być ignorowany"
        );

        // Klik na nową pozycję (row=2) → przyciski na ekranie
        let result_new = widget.handle_mouse(make_left_click(3, 2), area);
        assert_eq!(
            result_new,
            AskUserAction::Submit("yes".into()),
            "Klik na nową pozycję po scrollu powinien trafić w Yes"
        );
    }

    #[test]
    fn test_handle_mouse_confirm_scrolled_out_of_view_ignored() {
        let mut widget = AskUserWidget::new(confirm_question());
        let area = Rect { x: 0, y: 0, width: 80, height: 20 };

        // scroll_offset >= q_height+1 → button_y < inner_y → poza widocznym obszarem
        // inner_y=2, q_height=1 → button_y = 2 + 1 - 2 = 1 < inner_y=2 → ignoruj
        widget.scroll_offset = 2;

        let result = widget.handle_mouse(make_left_click(3, 1), area);
        assert_eq!(
            result,
            AskUserAction::Continue,
            "Klik na obszar gdzie przyciski są poza content area powinien być ignorowany"
        );
    }
}
