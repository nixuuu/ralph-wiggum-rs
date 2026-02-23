//! Key i mouse handling dla TaskExplorerApp.
//!
//! Dispatch klawiszy do odpowiedniego panelu (Tree/Detail).
//! Globalne skróty (Tab, s, r) obsługiwane niezależnie od focus.
//! Obsługa kliknięć myszy: lewy klik na wiersz → zaznacz task lub toggle expand.

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};

use crate::tui::events::EventResult;

use super::state::{InputMode, Panel, TaskExplorerApp};

// ── Key dispatch ─────────────────────────────────────────────────────

impl TaskExplorerApp {
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> EventResult {
        // W trybie Filter, obsługuj tylko klawisze edycji tekstu
        if self.input_mode == InputMode::Filter {
            return self.handle_filter_key(key);
        }

        // Esc: w Detail → wróć do Tree (unfocus), w Tree → quit
        if key.code == KeyCode::Esc {
            return match self.focus {
                Panel::Detail => {
                    self.focus = Panel::Tree;
                    EventResult::Consumed
                }
                Panel::Tree => EventResult::Quit,
            };
        }

        // Tab przełącza focus niezależnie od aktywnego panelu
        if key.code == KeyCode::Tab {
            self.focus = match self.focus {
                Panel::Tree => Panel::Detail,
                Panel::Detail => Panel::Tree,
            };
            return EventResult::Consumed;
        }

        // 's' cyklicznie zmienia tryb sortowania (zachowuje selekcję na tym samym ID)
        if key.code == KeyCode::Char('s') && key.modifiers.is_empty() {
            self.sort_mode = self.sort_mode.next();
            self.restore_selection_by_id();
            return EventResult::Consumed;
        }

        // 'r' reload z dysku
        if key.code == KeyCode::Char('r') && key.modifiers.is_empty() {
            let _ = self.reload();
            return EventResult::Consumed;
        }

        // 'f' aktywuj tryb filtrowania
        if key.code == KeyCode::Char('f') && key.modifiers.is_empty() {
            self.input_mode = InputMode::Filter;
            return EventResult::Consumed;
        }

        // '/' aktywuj tryb filtrowania (alternatywa dla 'f')
        if key.code == KeyCode::Char('/') {
            self.input_mode = InputMode::Filter;
            return EventResult::Consumed;
        }

        // Dispatch per panel
        match self.focus {
            Panel::Tree => self.handle_tree_key(key),
            Panel::Detail => self.handle_detail_key(key),
        }
    }

    /// Key handling dla trybu filtrowania (InputMode::Filter).
    fn handle_filter_key(&mut self, key: KeyEvent) -> EventResult {
        match key.code {
            KeyCode::Esc => {
                // Esc → wyczyść filtr i wróć do Normal mode
                self.filter.clear();
                self.input_mode = InputMode::Normal;
                self.sync_selected_id();
                EventResult::Consumed
            }
            KeyCode::Enter => {
                // Enter → zaakceptuj filtr, wróć do Normal mode
                self.input_mode = InputMode::Normal;
                self.sync_selected_id();
                EventResult::Consumed
            }
            KeyCode::Backspace => {
                // Backspace → usuń ostatni znak
                self.filter.pop();
                self.sync_selected_id();
                EventResult::Consumed
            }
            KeyCode::Char(c) => {
                // Wpisz znak do filtra
                self.filter.push(c);
                self.sync_selected_id();
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }

    /// Key handling dla panelu Tree.
    fn handle_tree_key(&mut self, key: KeyEvent) -> EventResult {
        let rows = self.visible_rows();
        let row_count = rows.len();

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.tree_state.select_prev();
                self.sync_selected_id();
                EventResult::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.tree_state.select_next(row_count);
                self.sync_selected_id();
                EventResult::Consumed
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                // Expand lub przełącz na Detail
                if self.tree_state.toggle_expand(&rows) {
                    self.sync_selected_id();
                } else {
                    self.focus = Panel::Detail;
                }
                EventResult::Consumed
            }
            KeyCode::Left | KeyCode::Char('h') => {
                // Collapse expanded parent, noop on collapsed/leaf
                let rows_fresh = self.visible_rows();
                if let Some(row) = rows_fresh.get(self.tree_state.selected)
                    && row.is_expanded
                {
                    self.tree_state.toggle_expand(&rows_fresh);
                }
                self.sync_selected_id();
                EventResult::Consumed
            }
            KeyCode::Home => {
                self.tree_state.selected = 0;
                self.sync_selected_id();
                EventResult::Consumed
            }
            KeyCode::End => {
                if row_count > 0 {
                    self.tree_state.selected = row_count - 1;
                }
                self.sync_selected_id();
                EventResult::Consumed
            }
            // 'e' expand all
            KeyCode::Char('e') if key.modifiers.is_empty() => {
                self.expand_all();
                self.sync_selected_id();
                EventResult::Consumed
            }
            // 'c' collapse all
            KeyCode::Char('c') if key.modifiers.is_empty() => {
                self.tree_state.expanded.clear();
                self.sync_selected_id();
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }

    /// Key handling dla panelu Detail (scroll z upper bound).
    fn handle_detail_key(&mut self, key: KeyEvent) -> EventResult {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.detail_scroll = self.detail_scroll.saturating_sub(self.scroll_step);
                EventResult::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                // Upper bound: nie scrolluj poza zawartość detail lines
                let max_scroll = self.detail_line_count().saturating_sub(1);
                self.detail_scroll = (self.detail_scroll + self.scroll_step).min(max_scroll);
                EventResult::Consumed
            }
            KeyCode::Left | KeyCode::Char('h') => {
                // Wróć do tree panel
                self.focus = Panel::Tree;
                EventResult::Consumed
            }
            KeyCode::Home => {
                self.detail_scroll = 0;
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }

    /// Policz linie w detail panel (do ograniczenia scrollu).
    fn detail_line_count(&self) -> usize {
        match self.selected_node() {
            Some(node) => super::drawing::detail_line_count(node),
            None => 1,
        }
    }

    /// Obsłuż zdarzenie myszy.
    ///
    /// MouseMoved → aktualizuj `hovered_row` (niezależnie od `tree_state.selected`).
    /// Lewy klik na wiersz drzewa → zaznacz task (zmień selected_index).
    /// Kliknięcie na już zaznaczony task → toggle expand/collapse.
    /// Klik poza wierszami drzewa → bez zmian.
    /// ScrollUp/ScrollDown → nawigacja po task tree (select_prev/select_next).
    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) -> EventResult {
        // Scroll wheel → nawigacja po task tree (krok = 1 per event)
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.tree_state.select_prev();
                self.sync_selected_id();
                return EventResult::Consumed;
            }
            MouseEventKind::ScrollDown => {
                let row_count = self.visible_rows().len();
                self.tree_state.select_next(row_count);
                self.sync_selected_id();
                return EventResult::Consumed;
            }
            MouseEventKind::Moved => {
                // Aktualizuj hovered_row — hover != selection.
                let col = mouse.column;
                let row = mouse.row;
                let new_hover = self
                    .task_row_rects
                    .iter()
                    .find(|(_, rect)| {
                        row >= rect.y
                            && row < rect.y + rect.height
                            && col >= rect.x
                            && col < rect.x + rect.width
                    })
                    .map(|&(abs_index, _)| abs_index);

                if self.hovered_row != new_hover {
                    self.hovered_row = new_hover;
                    return EventResult::Consumed;
                }
                return EventResult::Ignored;
            }
            _ => {}
        }

        // Obsługujemy tylko lewy MouseDown
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return EventResult::Ignored;
        }

        let col = mouse.column;
        let row = mouse.row;

        // Hit-test na cache'owane recty wierszy drzewa.
        // Kopiujemy abs_index (usize) żeby nie trzymać referencji do self.
        let clicked_index = self
            .task_row_rects
            .iter()
            .find(|(_, rect)| {
                row >= rect.y
                    && row < rect.y + rect.height
                    && col >= rect.x
                    && col < rect.x + rect.width
            })
            .map(|&(abs_index, _)| abs_index);

        let Some(abs_index) = clicked_index else {
            // Klik poza taskami — brak zmiany
            return EventResult::Ignored;
        };

        // Klik na drzewo zawsze przenosi focus na panel Tree
        self.focus = Panel::Tree;

        if abs_index == self.tree_state.selected {
            // Klik na zaznaczony task → toggle expand/collapse
            let rows = self.visible_rows();
            self.tree_state.toggle_expand(&rows);
            self.sync_selected_id();
        } else {
            // Klik na inny task → zaznacz go
            self.tree_state.selected = abs_index;
            self.sync_selected_id();
        }

        EventResult::Consumed
    }
}
