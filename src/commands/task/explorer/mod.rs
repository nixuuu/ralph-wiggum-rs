//! TaskExplorerApp — TUI stan dla interaktywnego eksploratora zadań.
//!
//! Implementuje `AppState` trait z `tui::app`. Zarządza:
//! - Drzewem zadań (TasksFile) z expand/collapse
//! - Panelem szczegółów wybranego taska
//! - Paskiem postępu na dole
//! - Nawigacją klawiaturową i przełączaniem focus między panelami
//! - Sortowaniem sibling nodes (wg ID, statusu, komponentu)
//! - Filtrowaniem wg nazwy, ID lub komponentu

mod drawing;
mod keys;
pub(crate) mod state;

pub use state::TaskExplorerApp;

// Re-exporty dla testów
#[cfg(test)]
#[allow(unused_imports)]
pub use state::{InputMode, Panel, SortMode};

use crate::tui::app::AppState;
use crate::tui::events::{AppEvent, EventResult};
use ratatui::Frame;
use ratatui::layout::Rect;

// ── AppState trait impl ──────────────────────────────────────────────

impl AppState for TaskExplorerApp {
    fn draw(&mut self, frame: &mut Frame, area: Rect) {
        drawing::draw_all(frame, area, self);
    }

    fn handle_event(&mut self, event: AppEvent) -> EventResult {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Resize(_, _) => EventResult::Consumed,
            AppEvent::Mouse(_) => EventResult::Ignored,
            AppEvent::Tick => EventResult::Ignored,
        }
    }
}
