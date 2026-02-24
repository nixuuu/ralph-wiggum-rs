//! Key i mouse handling dla TaskExplorerApp.
//!
//! Dispatch klawiszy do odpowiedniego panelu (Tree/Detail).
//! Globalne skróty (Tab, s, r) obsługiwane niezależnie od focus.
//! Obsługa kliknięć myszy: lewy klik na wiersz → zaznacz task lub toggle expand.

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};

use crate::tui::events::EventResult;
use crate::tui::keybindings::{KeyAction, KeybindingResolver, View};

use super::state::{InputMode, Panel, TaskExplorerApp};

// ── Key dispatch ─────────────────────────────────────────────────────

impl TaskExplorerApp {
    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        resolver: &KeybindingResolver,
    ) -> EventResult {
        // W trybie Filter, obsługuj tylko klawisze edycji tekstu
        if self.input_mode == InputMode::Filter {
            return self.handle_filter_key(key);
        }

        match resolver.resolve(&key, View::Explorer) {
            Some(KeyAction::Cancel) => {
                // Esc: w Detail → wróć do Tree (unfocus), w Tree → quit
                match self.focus {
                    Panel::Detail => {
                        self.focus = Panel::Tree;
                        EventResult::Consumed
                    }
                    Panel::Tree => EventResult::Quit,
                }
            }
            Some(KeyAction::SwitchFocus) => {
                // Tab przełącza focus niezależnie od aktywnego panelu
                self.focus = match self.focus {
                    Panel::Tree => Panel::Detail,
                    Panel::Detail => Panel::Tree,
                };
                EventResult::Consumed
            }
            Some(KeyAction::CycleSort) => {
                // 's' cyklicznie zmienia tryb sortowania (zachowuje selekcję na tym samym ID)
                self.sort_mode = self.sort_mode.next();
                self.restore_selection_by_id();
                EventResult::Consumed
            }
            Some(KeyAction::ReloadTasks) => {
                // 'r' reload z dysku
                let _ = self.reload();
                EventResult::Consumed
            }
            Some(KeyAction::EnterFilter) => {
                // 'f' aktywuj tryb filtrowania
                self.input_mode = InputMode::Filter;
                EventResult::Consumed
            }
            _ => {
                // '/' jako alternatywa dla 'f' — brak mapowania w resolverze
                if key.code == KeyCode::Char('/') {
                    self.input_mode = InputMode::Filter;
                    return EventResult::Consumed;
                }
                // Dispatch per panel
                match self.focus {
                    Panel::Tree => self.handle_tree_key(key, resolver),
                    Panel::Detail => self.handle_detail_key(key, resolver),
                }
            }
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
    fn handle_tree_key(&mut self, key: KeyEvent, resolver: &KeybindingResolver) -> EventResult {
        let rows = self.visible_rows();
        let row_count = rows.len();

        // Traktuj strzałki Left/Right jako VimLeft/VimRight — niekonfigurowalne nawigacyjne skróty.
        // Nie są w resolverze celowo: to stałe klawisze nawigacji, nie customizable bindingi.
        let action = match key.code {
            KeyCode::Right => Some(KeyAction::VimRight),
            KeyCode::Left => Some(KeyAction::VimLeft),
            _ => resolver.resolve(&key, View::Explorer),
        };

        match action {
            Some(KeyAction::ScrollUp) | Some(KeyAction::VimUp) => {
                self.tree_state.select_prev();
                self.sync_selected_id();
                EventResult::Consumed
            }
            Some(KeyAction::ScrollDown) | Some(KeyAction::VimDown) => {
                self.tree_state.select_next(row_count);
                self.sync_selected_id();
                EventResult::Consumed
            }
            Some(KeyAction::ExpandOrEnter) | Some(KeyAction::VimRight) => {
                // Expand lub przełącz na Detail
                if self.tree_state.toggle_expand(&rows) {
                    self.sync_selected_id();
                } else {
                    self.focus = Panel::Detail;
                }
                EventResult::Consumed
            }
            Some(KeyAction::VimLeft) => {
                // Collapse expanded parent, noop on collapsed/leaf
                if let Some(row) = rows.get(self.tree_state.selected)
                    && row.is_expanded
                {
                    self.tree_state.toggle_expand(&rows);
                }
                self.sync_selected_id();
                EventResult::Consumed
            }
            Some(KeyAction::ScrollToTop) => {
                self.tree_state.selected = 0;
                self.sync_selected_id();
                EventResult::Consumed
            }
            Some(KeyAction::ScrollToBottom) => {
                if row_count > 0 {
                    self.tree_state.selected = row_count - 1;
                }
                self.sync_selected_id();
                EventResult::Consumed
            }
            Some(KeyAction::ExpandAll) => {
                self.expand_all();
                self.sync_selected_id();
                EventResult::Consumed
            }
            Some(KeyAction::CollapseAll) => {
                self.tree_state.expanded.clear();
                self.sync_selected_id();
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }

    /// Key handling dla panelu Detail (scroll z upper bound).
    fn handle_detail_key(&mut self, key: KeyEvent, resolver: &KeybindingResolver) -> EventResult {
        // Left arrow traktowany jak VimLeft — niekonfigurowalny skrót powrotu do tree.
        let action = match key.code {
            KeyCode::Left => Some(KeyAction::VimLeft),
            _ => resolver.resolve(&key, View::Explorer),
        };

        match action {
            Some(KeyAction::ScrollUp) | Some(KeyAction::VimUp) => {
                self.detail_scroll = self.detail_scroll.saturating_sub(self.scroll_step);
                EventResult::Consumed
            }
            Some(KeyAction::ScrollDown) | Some(KeyAction::VimDown) => {
                // Upper bound: nie scrolluj poza zawartość detail lines
                let max_scroll = self.detail_line_count().saturating_sub(1);
                self.detail_scroll = (self.detail_scroll + self.scroll_step).min(max_scroll);
                EventResult::Consumed
            }
            Some(KeyAction::VimLeft) => {
                // 'h'/Left — wróć do tree panel
                self.focus = Panel::Tree;
                EventResult::Consumed
            }
            Some(KeyAction::ScrollToTop) => {
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

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use crate::commands::task::explorer::state::{InputMode, Panel, SortMode, TaskExplorerApp};
    use crate::shared::tasks::TasksFile;
    use crate::tui::events::EventResult;
    use crate::tui::keybindings::KeybindingResolver;
    use crate::tui::widgets::task_tree::{self, TreeState};

    fn sample_yaml() -> &'static str {
        r#"
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
        status: todo
        component: parser
  - id: "2"
    name: "Epic Two"
    component: dag
    subtasks:
      - id: "2.1"
        name: "Cycle detect"
        status: todo
        component: dag
"#
    }

    fn make_app() -> TaskExplorerApp {
        let tasks: TasksFile = serde_yaml::from_str(sample_yaml()).unwrap();
        let mut expanded = HashSet::new();
        for node in &tasks.tasks {
            expanded.insert(node.id.clone());
        }
        let tree_state = TreeState {
            expanded,
            selected: 0,
            scroll_offset: 0,
        };
        let rows = task_tree::flatten_nodes(&tasks.tasks, &tree_state.expanded);
        let selected_id = rows.first().map(|r| r.id.clone());
        TaskExplorerApp {
            tasks,
            tasks_path: PathBuf::from(".ralph/tasks.yml"),
            tree_state,
            selected_id,
            focus: Panel::Tree,
            input_mode: InputMode::Normal,
            filter: String::new(),
            sort_mode: SortMode::Id,
            detail_scroll: 0,
            scroll_step: 3,
            task_row_rects: Vec::new(),
            hovered_row: None,
        }
    }

    fn make_key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    // ── Custom keybinding tests ─────────────────────────────────────

    /// Test: zmiana keybindingu 'cycle_sort' z 's' na 'z' —
    /// 's' powinno być ignorowane, 'z' powinno cyklicznie zmieniać sort_mode.
    #[test]
    fn custom_keybinding_cycle_sort_remap_works() {
        use crate::tui::keybindings::{ExplorerBindings, KeyCombo, KeybindingsConfig};

        let custom_config = KeybindingsConfig {
            explorer: ExplorerBindings {
                cycle_sort: KeyCombo::new(KeyCode::Char('z'), KeyModifiers::NONE),
                ..ExplorerBindings::default()
            },
            ..KeybindingsConfig::default()
        };
        let custom_resolver = KeybindingResolver::from_user_config(custom_config);

        let mut app = make_app();
        let initial_sort = app.sort_mode;

        // 's' z custom resolverem → nie jest już CycleSort → Ignored
        let key_s = make_key(KeyCode::Char('s'));
        let result = app.handle_key(key_s, &custom_resolver);
        assert_eq!(
            result,
            EventResult::Ignored,
            "'s' powinno być ignorowane gdy cycle_sort = 'z'"
        );
        assert_eq!(
            app.sort_mode, initial_sort,
            "sort_mode nie powinien się zmienić"
        );

        // 'z' z custom resolverem → CycleSort → sort_mode się zmienia
        let key_z = make_key(KeyCode::Char('z'));
        let result = app.handle_key(key_z, &custom_resolver);
        assert_eq!(
            result,
            EventResult::Consumed,
            "'z' powinno cyklicznie zmieniać sort_mode"
        );
        assert_ne!(
            app.sort_mode, initial_sort,
            "sort_mode powinien się zmienić"
        );
    }
}
