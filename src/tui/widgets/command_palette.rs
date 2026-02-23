//! VS Code-style command palette widget — centered modal overlay.
//!
//! Wyświetla pole wyszukiwania na górze i scrollowalną listę wyników poniżej.
//! Obsługuje fuzzy matching z podświetleniem dopasowanych znaków.
//!
//! Keys: chars → filter, ↑↓ → navigate, Enter → select, Esc → close.

use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, StatefulWidget, Widget},
};

use crate::tui::fuzzy::{FuzzyMatch, fuzzy_match};
use crate::tui::theme::DEFAULT_THEME;

// ── PaletteItem ──────────────────────────────────────────────────────

/// Element w command palette — akcja, komenda, task itp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    /// Unikalny identyfikator (np. "task.add", "worker.stop")
    pub id: String,
    /// Wyświetlana etykieta (tekst do fuzzy match)
    pub label: String,
    /// Opcjonalny opis pod etykietą
    pub description: Option<String>,
    /// Opcjonalna ikona (Nerd Font char)
    pub icon: Option<char>,
    /// Kategoria wyświetlana jako prefix (np. "Task", "Worker", "Setting")
    pub category: String,
}

// ── PaletteAction ────────────────────────────────────────────────────

/// Wynik obsługi zdarzenia w command palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteAction {
    /// Użytkownik wybrał element (zwraca id)
    Select(String),
    /// Zamknij paletę (Esc)
    Close,
    /// Kontynuuj — klawisz obsłużony, brak akcji zewnętrznej
    Continue,
}

// ── CommandPaletteState ──────────────────────────────────────────────

/// Stan command palette — query, selekcja, filtrowane wyniki.
pub struct CommandPaletteState {
    /// Tekst wpisany przez użytkownika (query do fuzzy match)
    query: String,
    /// Pozycja kursora w query (char index)
    cursor_pos: usize,
    /// Indeks wybranego elementu w filtrowanej liście
    selected_index: usize,
    /// Wszystkie dostępne elementy
    items: Vec<PaletteItem>,
    /// Wyniki po fuzzy filter (indeks w items + FuzzyMatch)
    filtered: Vec<(usize, FuzzyMatch)>,
    /// Offset scrollowania listy wyników
    scroll_offset: usize,
    /// Maksymalna liczba widocznych elementów (default 10)
    visible_count: usize,
}

impl CommandPaletteState {
    /// Tworzy nowy state z podanymi elementami.
    pub fn new(items: Vec<PaletteItem>) -> Self {
        let mut state = Self {
            query: String::new(),
            cursor_pos: 0,
            selected_index: 0,
            items,
            filtered: Vec::new(),
            scroll_offset: 0,
            visible_count: 10,
        };
        state.refilter();
        state
    }

    /// Tworzy state z custom visible_count.
    #[cfg(test)]
    pub fn with_visible_count(items: Vec<PaletteItem>, visible_count: usize) -> Self {
        let mut state = Self {
            visible_count,
            ..Self::new(items)
        };
        state.refilter();
        state
    }

    /// Obsługuje zdarzenie klawiatury, zwraca akcję.
    pub fn handle_key(&mut self, key: KeyCode) -> PaletteAction {
        match key {
            KeyCode::Esc => PaletteAction::Close,
            KeyCode::Enter => {
                if let Some((item_idx, _)) = self.filtered.get(self.selected_index) {
                    PaletteAction::Select(self.items[*item_idx].id.clone())
                } else {
                    PaletteAction::Close
                }
            }
            KeyCode::Up => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                    // Scroll up jeśli selekcja wychodzi poza viewport
                    if self.selected_index < self.scroll_offset {
                        self.scroll_offset = self.selected_index;
                    }
                }
                PaletteAction::Continue
            }
            KeyCode::Down => {
                if !self.filtered.is_empty() && self.selected_index < self.filtered.len() - 1 {
                    self.selected_index += 1;
                    // Scroll down jeśli selekcja wychodzi poza viewport
                    if self.selected_index >= self.scroll_offset + self.visible_count {
                        self.scroll_offset = self.selected_index - self.visible_count + 1;
                    }
                }
                PaletteAction::Continue
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    // Usuń cały znak przed kursorem (drain obsługuje multi-byte)
                    if let Some((start, ch)) = self.query.char_indices().nth(self.cursor_pos - 1) {
                        self.query.drain(start..start + ch.len_utf8());
                    }
                    self.cursor_pos -= 1;
                    self.refilter();
                }
                PaletteAction::Continue
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
                PaletteAction::Continue
            }
            KeyCode::Right => {
                if self.cursor_pos < self.query.chars().count() {
                    self.cursor_pos += 1;
                }
                PaletteAction::Continue
            }
            KeyCode::Home => {
                self.cursor_pos = 0;
                PaletteAction::Continue
            }
            KeyCode::End => {
                self.cursor_pos = self.query.chars().count();
                PaletteAction::Continue
            }
            KeyCode::Char(c) => {
                // Wstaw znak na pozycji kursora
                let byte_pos = self
                    .query
                    .char_indices()
                    .nth(self.cursor_pos)
                    .map(|(i, _)| i)
                    .unwrap_or(self.query.len());
                self.query.insert(byte_pos, c);
                self.cursor_pos += 1;
                self.refilter();
                PaletteAction::Continue
            }
            _ => PaletteAction::Continue,
        }
    }

    /// Przefiltruj items na podstawie query.
    fn refilter(&mut self) {
        if self.query.is_empty() {
            // Puste query — pokaż wszystkie elementy w oryginalnej kolejności
            self.filtered = self
                .items
                .iter()
                .enumerate()
                .map(|(idx, item)| {
                    (
                        idx,
                        FuzzyMatch {
                            text: item.label.clone(),
                            score: 0,
                            matched_indices: vec![],
                        },
                    )
                })
                .collect();
        } else {
            // Filtruj z enumerate — indeksy bezpośrednio z iteratora
            self.filtered = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(idx, item)| fuzzy_match(&self.query, &item.label).map(|m| (idx, m)))
                .collect();
            self.filtered.sort_by(|a, b| b.1.score.cmp(&a.1.score));
        }

        // Reset selekcji po refilter
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    // ── Accessory getters (publiczne dla testów) ──

    /// Zwraca aktualny query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Zwraca indeks wybranego elementu.
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Zwraca liczbę przefiltrowanych wyników.
    pub fn filtered_count(&self) -> usize {
        self.filtered.len()
    }
}

// ── CommandPaletteWidget ─────────────────────────────────────────────

/// Widget renderujący command palette jako centered modal overlay.
///
/// Implementuje `StatefulWidget` z `CommandPaletteState`.
/// Rysuje: backdrop → centered box → input line → separator → wyniki.
pub struct CommandPaletteWidget;

impl StatefulWidget for CommandPaletteWidget {
    type State = CommandPaletteState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Backdrop — przyciemnia tło pod overlay
        let backdrop_style = Style::default().bg(DEFAULT_THEME.border_normal);
        buf.set_style(area, backdrop_style);

        // Rozmiar overlay: 60% szerokości, dynamiczna wysokość
        let overlay_width = (area.width * 6 / 10).max(30).min(area.width);
        // Wysokość: 3 (header + input + separator) + min(filtered, visible_count) + 1 (padding)
        let results_height = state.filtered.len().min(state.visible_count) as u16;
        let overlay_height = (3 + results_height + 1).min(area.height);

        let overlay_area = centered_rect(overlay_width, overlay_height, area);

        // Panel z tłem
        let block = Block::default()
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(DEFAULT_THEME.panel_bg_focused));

        let inner = block.inner(overlay_area);
        block.render(overlay_area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let mut y = inner.y;

        // ── Input line ──
        render_input_line(state, inner.x, y, inner.width, buf);
        y += 1;

        // ── Separator ──
        if y < inner.y + inner.height {
            let separator = "─".repeat(inner.width as usize);
            let sep_line = Line::from(Span::styled(&separator, DEFAULT_THEME.muted_style()));
            buf.set_line(inner.x, y, &sep_line, inner.width);
            y += 1;
        }

        // ── Wyniki ──
        let visible_results: Vec<_> = state
            .filtered
            .iter()
            .skip(state.scroll_offset)
            .take(state.visible_count)
            .enumerate()
            .collect();

        for (visible_idx, (item_idx, fuzzy)) in visible_results {
            if y >= inner.y + inner.height {
                break;
            }

            let actual_idx = state.scroll_offset + visible_idx;
            let is_selected = actual_idx == state.selected_index;
            let item = &state.items[*item_idx];

            render_result_line(item, fuzzy, is_selected, inner.x, y, inner.width, buf);
            y += 1;
        }
    }
}

// ── Rendering helpers ────────────────────────────────────────────────

/// Renderuje linię input z query i kursorem.
fn render_input_line(state: &CommandPaletteState, x: u16, y: u16, width: u16, buf: &mut Buffer) {
    let prompt = "> ";
    let mut spans = vec![Span::styled(prompt, DEFAULT_THEME.primary_style())];

    if state.query.is_empty() {
        // Placeholder z kursorem
        spans.push(Span::styled(
            "│",
            DEFAULT_THEME
                .primary_style()
                .add_modifier(Modifier::SLOW_BLINK),
        ));
    } else {
        // Query z kursorem
        let query_chars: Vec<char> = state.query.chars().collect();
        let before: String = query_chars[..state.cursor_pos].iter().collect();
        let after: String = query_chars[state.cursor_pos..].iter().collect();

        if !before.is_empty() {
            spans.push(Span::styled(before, DEFAULT_THEME.text_style()));
        }

        if after.is_empty() {
            spans.push(Span::styled(
                "│",
                DEFAULT_THEME
                    .primary_style()
                    .add_modifier(Modifier::SLOW_BLINK),
            ));
        } else {
            // Kursor na pierwszym znaku `after`
            let cursor_char: String = after.chars().take(1).collect();
            let rest: String = after.chars().skip(1).collect();
            spans.push(Span::styled(
                cursor_char,
                DEFAULT_THEME.text_style().add_modifier(Modifier::REVERSED),
            ));
            if !rest.is_empty() {
                spans.push(Span::styled(rest, DEFAULT_THEME.text_style()));
            }
        }
    }

    let line = Line::from(spans);
    buf.set_line(x, y, &line, width);
}

/// Renderuje linię wyniku z podświetleniem dopasowanych znaków.
fn render_result_line(
    item: &PaletteItem,
    fuzzy: &FuzzyMatch,
    is_selected: bool,
    x: u16,
    y: u16,
    width: u16,
    buf: &mut Buffer,
) {
    let mut spans: Vec<Span> = Vec::new();

    // Tło dla zaznaczonego elementu
    let base_style = if is_selected {
        Style::default()
            .bg(DEFAULT_THEME.header_bg)
            .fg(DEFAULT_THEME.text)
    } else {
        DEFAULT_THEME.text_style()
    };

    let highlight_style = if is_selected {
        Style::default()
            .bg(DEFAULT_THEME.header_bg)
            .fg(DEFAULT_THEME.primary)
            .add_modifier(Modifier::BOLD)
    } else {
        DEFAULT_THEME.primary_style().add_modifier(Modifier::BOLD)
    };

    // Ikona (jeśli jest)
    if let Some(icon) = item.icon {
        spans.push(Span::styled(
            format!("{icon} "),
            DEFAULT_THEME.muted_style(),
        ));
    }

    // Kategoria jako prefix: [Category]
    if !item.category.is_empty() {
        let cat_style = if is_selected {
            Style::default()
                .bg(DEFAULT_THEME.header_bg)
                .fg(DEFAULT_THEME.muted)
        } else {
            DEFAULT_THEME.muted_style()
        };
        spans.push(Span::styled(format!("[{}] ", item.category), cat_style));
    }

    // Label z podświetleniem dopasowanych znaków
    let label_chars: Vec<char> = item.label.chars().collect();
    let matched_set: std::collections::HashSet<usize> =
        fuzzy.matched_indices.iter().copied().collect();

    for (char_idx, &ch) in label_chars.iter().enumerate() {
        let style = if matched_set.contains(&char_idx) {
            highlight_style
        } else {
            base_style
        };
        spans.push(Span::styled(ch.to_string(), style));
    }

    // Description (jeśli jest i zmieści się)
    if let Some(ref desc) = item.description {
        let desc_style = if is_selected {
            Style::default()
                .bg(DEFAULT_THEME.header_bg)
                .fg(DEFAULT_THEME.muted)
        } else {
            DEFAULT_THEME.muted_style()
        };
        spans.push(Span::styled(format!("  {desc}"), desc_style));
    }

    // Wypełnij tło dla selected do końca wiersza
    if is_selected {
        let line = Line::from(spans);
        // Najpierw ustaw tło na cały wiersz
        let bg_style = Style::default().bg(DEFAULT_THEME.header_bg);
        for col in x..x + width {
            if let Some(cell) = buf.cell_mut((col, y)) {
                cell.set_style(bg_style);
            }
        }
        buf.set_line(x, y, &line, width);
    } else {
        let line = Line::from(spans);
        buf.set_line(x, y, &line, width);
    }
}

/// Tworzy wyśrodkowany prostokąt o podanych wymiarach w area.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::snap;
    use ratatui::{Terminal, backend::TestBackend};

    /// Helper: tworzy testowe PaletteItem.
    fn test_items() -> Vec<PaletteItem> {
        vec![
            PaletteItem {
                id: "task.add".into(),
                label: "Add Task".into(),
                description: Some("Create a new task".into()),
                icon: None,
                category: "Task".into(),
            },
            PaletteItem {
                id: "task.edit".into(),
                label: "Edit Task".into(),
                description: Some("Modify existing task".into()),
                icon: None,
                category: "Task".into(),
            },
            PaletteItem {
                id: "task.delete".into(),
                label: "Delete Task".into(),
                description: None,
                icon: None,
                category: "Task".into(),
            },
            PaletteItem {
                id: "worker.start".into(),
                label: "Start Worker".into(),
                description: Some("Launch a new worker".into()),
                icon: None,
                category: "Worker".into(),
            },
            PaletteItem {
                id: "worker.stop".into(),
                label: "Stop Worker".into(),
                description: None,
                icon: None,
                category: "Worker".into(),
            },
            PaletteItem {
                id: "setting.theme".into(),
                label: "Change Theme".into(),
                description: Some("Switch color theme".into()),
                icon: None,
                category: "Setting".into(),
            },
        ]
    }

    // ── State tests ──────────────────────────────────────────────────

    #[test]
    fn test_new_state_shows_all_items() {
        let state = CommandPaletteState::new(test_items());
        assert_eq!(state.query(), "");
        assert_eq!(state.selected_index(), 0);
        assert_eq!(state.filtered_count(), 6);
    }

    #[test]
    fn test_typing_filters_items() {
        let mut state = CommandPaletteState::new(test_items());
        state.handle_key(KeyCode::Char('a'));
        state.handle_key(KeyCode::Char('d'));
        state.handle_key(KeyCode::Char('d'));
        assert_eq!(state.query(), "add");
        // "Add Task" powinien być dopasowany
        assert!(state.filtered_count() >= 1);
    }

    #[test]
    fn test_backspace_restores_results() {
        let mut state = CommandPaletteState::new(test_items());
        state.handle_key(KeyCode::Char('x'));
        state.handle_key(KeyCode::Char('y'));
        state.handle_key(KeyCode::Char('z'));
        let filtered_with_xyz = state.filtered_count();

        state.handle_key(KeyCode::Backspace);
        state.handle_key(KeyCode::Backspace);
        state.handle_key(KeyCode::Backspace);
        assert_eq!(state.query(), "");
        assert!(state.filtered_count() > filtered_with_xyz);
    }

    #[test]
    fn test_esc_returns_close() {
        let mut state = CommandPaletteState::new(test_items());
        assert_eq!(state.handle_key(KeyCode::Esc), PaletteAction::Close);
    }

    #[test]
    fn test_enter_returns_select_with_id() {
        let mut state = CommandPaletteState::new(test_items());
        // Pierwszy element w filtrowanej liście (puste query = wszystkie)
        let action = state.handle_key(KeyCode::Enter);
        match action {
            PaletteAction::Select(id) => {
                // Powinien zwrócić id pierwszego elementu
                assert!(!id.is_empty());
            }
            _ => panic!("Expected PaletteAction::Select, got {action:?}"),
        }
    }

    #[test]
    fn test_enter_on_empty_results_closes() {
        let mut state = CommandPaletteState::new(test_items());
        // Wpisz coś co nie pasuje
        for c in "zzzzzzzzz".chars() {
            state.handle_key(KeyCode::Char(c));
        }
        assert_eq!(state.filtered_count(), 0);
        assert_eq!(state.handle_key(KeyCode::Enter), PaletteAction::Close);
    }

    #[test]
    fn test_arrow_down_moves_selection() {
        let mut state = CommandPaletteState::new(test_items());
        assert_eq!(state.selected_index(), 0);

        state.handle_key(KeyCode::Down);
        assert_eq!(state.selected_index(), 1);

        state.handle_key(KeyCode::Down);
        assert_eq!(state.selected_index(), 2);
    }

    #[test]
    fn test_arrow_up_moves_selection() {
        let mut state = CommandPaletteState::new(test_items());
        state.handle_key(KeyCode::Down);
        state.handle_key(KeyCode::Down);
        assert_eq!(state.selected_index(), 2);

        state.handle_key(KeyCode::Up);
        assert_eq!(state.selected_index(), 1);
    }

    #[test]
    fn test_arrow_up_at_top_stays() {
        let mut state = CommandPaletteState::new(test_items());
        assert_eq!(state.selected_index(), 0);
        state.handle_key(KeyCode::Up);
        assert_eq!(state.selected_index(), 0);
    }

    #[test]
    fn test_arrow_down_at_bottom_stays() {
        let mut state = CommandPaletteState::new(test_items());
        let count = state.filtered_count();
        for _ in 0..count + 5 {
            state.handle_key(KeyCode::Down);
        }
        assert_eq!(state.selected_index(), count - 1);
    }

    #[test]
    fn test_selection_resets_on_refilter() {
        let mut state = CommandPaletteState::new(test_items());
        state.handle_key(KeyCode::Down);
        state.handle_key(KeyCode::Down);
        assert_eq!(state.selected_index(), 2);

        // Wpisanie znaku resetuje selekcję
        state.handle_key(KeyCode::Char('a'));
        assert_eq!(state.selected_index(), 0);
    }

    #[test]
    fn test_cursor_left_right() {
        let mut state = CommandPaletteState::new(test_items());
        state.handle_key(KeyCode::Char('a'));
        state.handle_key(KeyCode::Char('b'));
        state.handle_key(KeyCode::Char('c'));
        assert_eq!(state.cursor_pos, 3);

        state.handle_key(KeyCode::Left);
        assert_eq!(state.cursor_pos, 2);

        state.handle_key(KeyCode::Left);
        assert_eq!(state.cursor_pos, 1);

        state.handle_key(KeyCode::Right);
        assert_eq!(state.cursor_pos, 2);
    }

    #[test]
    fn test_cursor_bounds() {
        let mut state = CommandPaletteState::new(test_items());
        // Left na pusty query
        state.handle_key(KeyCode::Left);
        assert_eq!(state.cursor_pos, 0);

        state.handle_key(KeyCode::Char('a'));
        // Right poza koniec
        state.handle_key(KeyCode::Right);
        assert_eq!(state.cursor_pos, 1); // nie wychodzi poza
    }

    #[test]
    fn test_scroll_offset_on_down_navigation() {
        let items: Vec<PaletteItem> = (0..20)
            .map(|i| PaletteItem {
                id: format!("item.{i}"),
                label: format!("Item {i}"),
                description: None,
                icon: None,
                category: "Test".into(),
            })
            .collect();
        let mut state = CommandPaletteState::with_visible_count(items, 5);

        // Nawiguj w dół 7 razy
        for _ in 0..7 {
            state.handle_key(KeyCode::Down);
        }
        assert_eq!(state.selected_index(), 7);
        // scroll_offset powinien się dostosować: 7 - 5 + 1 = 3
        assert_eq!(state.scroll_offset, 3);
    }

    #[test]
    fn test_scroll_offset_on_up_navigation() {
        let items: Vec<PaletteItem> = (0..20)
            .map(|i| PaletteItem {
                id: format!("item.{i}"),
                label: format!("Item {i}"),
                description: None,
                icon: None,
                category: "Test".into(),
            })
            .collect();
        let mut state = CommandPaletteState::with_visible_count(items, 5);

        // Nawiguj w dół 7 razy, potem w górę 5 razy
        for _ in 0..7 {
            state.handle_key(KeyCode::Down);
        }
        for _ in 0..5 {
            state.handle_key(KeyCode::Up);
        }
        assert_eq!(state.selected_index(), 2);
        // scroll_offset powinien się cofnąć do selected_index
        assert_eq!(state.scroll_offset, 2);
    }

    // ── Widget snapshot tests ────────────────────────────────────────

    /// Helper: renderuje palette do bufora z Terminal::draw.
    fn render_palette(state: &mut CommandPaletteState, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_stateful_widget(CommandPaletteWidget, area, state);
            })
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    #[test]
    fn test_snapshot_empty_query_all_items() {
        let mut state = CommandPaletteState::new(test_items());
        let buffer = render_palette(&mut state, 80, 20);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_filtered_results() {
        let mut state = CommandPaletteState::new(test_items());
        state.handle_key(KeyCode::Char('s'));
        state.handle_key(KeyCode::Char('t'));
        state.handle_key(KeyCode::Char('a'));
        state.handle_key(KeyCode::Char('r'));
        state.handle_key(KeyCode::Char('t'));

        let buffer = render_palette(&mut state, 80, 20);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_selected_item() {
        let mut state = CommandPaletteState::new(test_items());
        state.handle_key(KeyCode::Down);
        state.handle_key(KeyCode::Down);

        let buffer = render_palette(&mut state, 80, 20);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_no_results() {
        let mut state = CommandPaletteState::new(test_items());
        for c in "zzzzz".chars() {
            state.handle_key(KeyCode::Char(c));
        }

        let buffer = render_palette(&mut state, 80, 20);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_narrow_terminal() {
        let mut state = CommandPaletteState::new(test_items());
        let buffer = render_palette(&mut state, 40, 15);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_with_icons() {
        let items = vec![
            PaletteItem {
                id: "add".into(),
                label: "Add Task".into(),
                description: Some("Create new".into()),
                icon: Some('+'),
                category: "Task".into(),
            },
            PaletteItem {
                id: "del".into(),
                label: "Delete Task".into(),
                description: None,
                icon: Some('-'),
                category: "Task".into(),
            },
        ];
        let mut state = CommandPaletteState::new(items);
        let buffer = render_palette(&mut state, 60, 12);
        insta::assert_snapshot!(snap(&buffer));
    }

    #[test]
    fn test_snapshot_fuzzy_highlight() {
        // Wpisz "et" — dopasuje "Edit Task" i "Delete Task"
        let mut state = CommandPaletteState::new(test_items());
        state.handle_key(KeyCode::Char('e'));
        state.handle_key(KeyCode::Char('t'));

        let buffer = render_palette(&mut state, 80, 15);
        insta::assert_snapshot!(snap(&buffer));
    }

    // ── Edge case tests: brak wyników, jeden wynik, pełna lista ─────

    /// Edge case: jeden wynik po filtrowaniu.
    ///
    /// Gdy filtrowanie zwraca dokładnie 1 wynik:
    /// - selected_index = 0
    /// - Down nie przesuwa (brak kolejnych)
    /// - Enter zwraca ten właśnie element
    #[test]
    fn test_single_result_navigation() {
        let mut state = CommandPaletteState::new(test_items());
        // "start" pasuje tylko do "Start Worker"
        for c in "start".chars() {
            state.handle_key(KeyCode::Char(c));
        }
        assert_eq!(state.filtered_count(), 1);
        assert_eq!(state.selected_index(), 0);

        // Down nie przesuwa za granicę
        state.handle_key(KeyCode::Down);
        assert_eq!(state.selected_index(), 0);

        // Up też nie przesuwa (jesteśmy na 0)
        state.handle_key(KeyCode::Up);
        assert_eq!(state.selected_index(), 0);

        // Enter zwraca id jedynego wyniku
        let action = state.handle_key(KeyCode::Enter);
        assert_eq!(action, PaletteAction::Select("worker.start".into()));
    }

    /// Edge case: brak wyników — Enter zamyka zamiast wybierać.
    #[test]
    fn test_no_results_enter_closes() {
        let mut state = CommandPaletteState::new(test_items());
        for c in "xyzabc".chars() {
            state.handle_key(KeyCode::Char(c));
        }
        assert_eq!(state.filtered_count(), 0);

        // Down/Up nic nie robią
        state.handle_key(KeyCode::Down);
        state.handle_key(KeyCode::Up);
        assert_eq!(state.selected_index(), 0);

        // Enter zamyka (brak wyniku do wybrania)
        assert_eq!(state.handle_key(KeyCode::Enter), PaletteAction::Close);
    }

    /// Edge case: pełna lista (10 itemów).
    ///
    /// Weryfikuje scroll, nawigację i selekcję dla maxymalnej liczby elementów
    /// wyświetlanych jednocześnie (visible_count = 10).
    #[test]
    fn test_full_list_ten_items() {
        let items: Vec<PaletteItem> = (0..10)
            .map(|i| PaletteItem {
                id: format!("item.{i}"),
                label: format!("Item {i}"),
                description: None,
                icon: None,
                category: "Test".into(),
            })
            .collect();

        let mut state = CommandPaletteState::new(items);
        assert_eq!(state.filtered_count(), 10);

        // Nawiguj przez wszystkie elementy (visible_count=10, więc brak scrollu)
        for expected_idx in 1..10 {
            state.handle_key(KeyCode::Down);
            assert_eq!(state.selected_index(), expected_idx);
        }
        // scroll_offset = 0 (wszystkie 10 elementów się mieszczą)
        assert_eq!(state.scroll_offset, 0);

        // Down na ostatnim nie przesuwa
        state.handle_key(KeyCode::Down);
        assert_eq!(state.selected_index(), 9);

        // Cofnij do początku
        for expected_idx in (0..9).rev() {
            state.handle_key(KeyCode::Up);
            assert_eq!(state.selected_index(), expected_idx);
        }

        // Enter na pierwszym elemencie zwraca item.0
        assert_eq!(
            state.handle_key(KeyCode::Enter),
            PaletteAction::Select("item.0".into())
        );
    }

    /// Snapshot: pełna lista 10 elementów, zero query.
    #[test]
    fn test_snapshot_full_ten_items() {
        let items: Vec<PaletteItem> = (0..10)
            .map(|i| PaletteItem {
                id: format!("item.{i}"),
                label: format!("Item {i}"),
                description: None,
                icon: None,
                category: "Test".into(),
            })
            .collect();
        let mut state = CommandPaletteState::new(items);
        let buffer = render_palette(&mut state, 80, 24);
        insta::assert_snapshot!(snap(&buffer));
    }

    /// Snapshot: jeden wynik po filtrowaniu (single result).
    #[test]
    fn test_snapshot_single_result() {
        let mut state = CommandPaletteState::new(test_items());
        // "start" → tylko "Start Worker"
        for c in "start".chars() {
            state.handle_key(KeyCode::Char(c));
        }
        assert_eq!(state.filtered_count(), 1);
        let buffer = render_palette(&mut state, 80, 20);
        insta::assert_snapshot!(snap(&buffer));
    }

    // ── Multi-byte & Home/End tests ─────────────────────────────────

    #[test]
    fn test_backspace_multibyte_chars() {
        let mut state = CommandPaletteState::new(test_items());
        // Wpisz polskie znaki (multi-byte UTF-8)
        state.handle_key(KeyCode::Char('ż'));
        state.handle_key(KeyCode::Char('ó'));
        state.handle_key(KeyCode::Char('ł'));
        assert_eq!(state.query(), "żół");
        assert_eq!(state.cursor_pos, 3);

        // Backspace powinien usunąć cały znak 'ł', nie tylko bajt
        state.handle_key(KeyCode::Backspace);
        assert_eq!(state.query(), "żó");
        assert_eq!(state.cursor_pos, 2);

        state.handle_key(KeyCode::Backspace);
        assert_eq!(state.query(), "ż");
        assert_eq!(state.cursor_pos, 1);

        state.handle_key(KeyCode::Backspace);
        assert_eq!(state.query(), "");
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_home_key_moves_to_start() {
        let mut state = CommandPaletteState::new(test_items());
        state.handle_key(KeyCode::Char('a'));
        state.handle_key(KeyCode::Char('b'));
        state.handle_key(KeyCode::Char('c'));
        assert_eq!(state.cursor_pos, 3);

        state.handle_key(KeyCode::Home);
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_end_key_moves_to_end() {
        let mut state = CommandPaletteState::new(test_items());
        state.handle_key(KeyCode::Char('a'));
        state.handle_key(KeyCode::Char('b'));
        state.handle_key(KeyCode::Char('c'));
        state.handle_key(KeyCode::Home);
        assert_eq!(state.cursor_pos, 0);

        state.handle_key(KeyCode::End);
        assert_eq!(state.cursor_pos, 3);
    }
}
