//! Widget panelu załączników — lista plików w stylu klienta pocztowego.
//!
//! Wyświetla każdy załącznik w formacie: 📎 nazwa (rozmiar) ✕
//! Podświetla zaznaczony element gdy panel jest focused.
//! Panel nie renderuje się gdy lista jest pusta.
//!
//! Użycie:
//! ```ignore
//! use ratatui::widgets::StatefulWidget;
//! use ralph_wiggum::tui::widgets::attachment_panel::{
//!     AttachmentInfo, AttachmentPanelState, AttachmentPanelWidget,
//! };
//! use std::path::PathBuf;
//!
//! let mut state = AttachmentPanelState {
//!     items: vec![AttachmentInfo {
//!         filename: "screenshot.png".into(),
//!         size_display: "42 KB".into(),
//!         path: PathBuf::from("/tmp/screenshot.png"),
//!     }],
//!     selected: 0,
//!     focused: true,
//! };
//! // frame.render_stateful_widget(AttachmentPanelWidget, area, &mut state);
//! ```

use std::path::PathBuf;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::StatefulWidget,
};

use crate::tui::theme::DEFAULT_THEME;

// ── AttachmentInfo ────────────────────────────────────────────────────

/// Metadane pojedynczego załącznika wyświetlanego w panelu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentInfo {
    /// Nazwa pliku (wyświetlana jako etykieta)
    pub filename: String,
    /// Czytelny dla człowieka rozmiar (np. "42 KB", "1.2 MB")
    pub size_display: String,
    /// Ścieżka do pliku (do otwarcia / usunięcia)
    pub path: PathBuf,
}

// ── AttachmentPanelState ──────────────────────────────────────────────

/// Stan panelu załączników — lista plików, zaznaczony indeks, fokus.
#[derive(Debug, Clone, Default)]
pub struct AttachmentPanelState {
    /// Lista załączników do wyświetlenia
    pub items: Vec<AttachmentInfo>,
    /// Indeks aktualnie zaznaczonego elementu
    pub selected: usize,
    /// Czy panel jest w stanie focused (aktywny input focus)
    pub focused: bool,
}

impl AttachmentPanelState {
    /// Tworzy nowy, pusty stan panelu.
    pub fn new() -> Self {
        Self::default()
    }

    /// Tworzy stan z podaną listą załączników.
    pub fn with_items(items: Vec<AttachmentInfo>) -> Self {
        Self {
            items,
            selected: 0,
            focused: false,
        }
    }

    /// Czy panel zawiera jakiekolwiek załączniki (warunkuje renderowanie).
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Przesuwa zaznaczenie w górę (z wrappingiem).
    pub fn move_up(&mut self) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        self.selected = if self.selected == 0 {
            n - 1
        } else {
            self.selected - 1
        };
    }

    /// Przesuwa zaznaczenie w dół (z wrappingiem).
    pub fn move_down(&mut self) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected + 1) % n;
    }

    /// Usuwa zaznaczony element i zwraca go (None jeśli lista pusta).
    pub fn remove_selected(&mut self) -> Option<AttachmentInfo> {
        if self.items.is_empty() {
            return None;
        }
        let removed = self.items.remove(self.selected);
        // Przesuń zaznaczenie żeby nie wyjść poza zakres
        if !self.items.is_empty() && self.selected >= self.items.len() {
            self.selected = self.items.len() - 1;
        }
        Some(removed)
    }

    /// Naprawia `selected` jeśli wyszedł poza zakres (np. po modyfikacji listy).
    pub fn clamp(&mut self) {
        if self.items.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.items.len() {
            self.selected = self.items.len() - 1;
        }
    }

    /// Dodaje załącznik do listy.
    pub fn add_attachment(&mut self, info: AttachmentInfo) {
        self.items.push(info);
    }

    /// Ustawia stan fokusa panelu.
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Zwraca true jeśli panel jest aktywnie sfokusowany.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Zwraca true jeśli panel zawiera jakiekolwiek załączniki.
    pub fn has_items(&self) -> bool {
        !self.items.is_empty()
    }
}

// ── AttachmentPanelWidget ─────────────────────────────────────────────

/// Widget listy załączników w stylu klienta pocztowego.
///
/// Każdy wiersz ma format: `📎 nazwa (rozmiar) ✕`
///
/// - Zaznaczony element (focused=true) podświetlony primary kolorem + bold
/// - Niezaznaczone elementy — tekst muted
/// - Ikona ✕ — kolor error (sygnalizuje możliwość usunięcia)
/// - Panel nie rysuje nic gdy lista jest pusta
pub struct AttachmentPanelWidget;

impl StatefulWidget for AttachmentPanelWidget {
    type State = AttachmentPanelState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Panel nie renderuje się gdy brak załączników
        if state.items.is_empty() {
            return;
        }

        // Napraw selected przed renderowaniem
        state.clamp();

        // Oblicz scroll offset — selected element zawsze musi być widoczny.
        // Jeśli selected > ostatni widoczny wiersz, przesuń widok w dół.
        let height = area.height as usize;
        let visible_start = if height > 0 && state.selected >= height {
            state.selected - height + 1
        } else {
            0
        };

        for (idx, item) in state.items.iter().enumerate().skip(visible_start) {
            let row = idx - visible_start;
            let y = area.y + row as u16;
            // Wyjdź jeśli wyszliśmy poza obszar
            if y >= area.y + area.height {
                break;
            }

            let is_selected = idx == state.selected && state.focused;

            // Style dla elementu: focused+selected = primary+bold, reszta = muted
            let item_style = if is_selected {
                Style::default()
                    .fg(DEFAULT_THEME.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                DEFAULT_THEME.muted_style()
            };

            // Tło podświetlonego wiersza
            let bg_style = if is_selected {
                Style::default().bg(DEFAULT_THEME.panel_bg_focused)
            } else {
                Style::default()
            };

            // ── Buduj spany wiersza: 📎 nazwa (rozmiar) ✕ ──
            let mut spans = vec![
                Span::styled("📎 ", item_style),
                Span::styled(item.filename.as_str(), item_style),
                Span::styled(" ", item_style),
                Span::styled(
                    format!("({})", item.size_display),
                    if is_selected {
                        Style::default()
                            .fg(DEFAULT_THEME.secondary)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        DEFAULT_THEME.muted_style()
                    },
                ),
                Span::styled(" ", item_style),
                Span::styled(
                    "✕",
                    if is_selected {
                        Style::default()
                            .fg(DEFAULT_THEME.error)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        DEFAULT_THEME.muted_style()
                    },
                ),
            ];

            // Aplikuj tło do wszystkich spanów gdy selected
            if is_selected {
                for span in &mut spans {
                    span.style = span.style.patch(bg_style);
                }
                // Wypełnij tło całego wiersza (żeby podświetlenie sięgało do końca)
                for col in area.x..area.x + area.width {
                    if let Some(cell) = buf.cell_mut((col, y)) {
                        cell.set_style(bg_style);
                    }
                }
            }

            let line = Line::from(spans);
            buf.set_line(area.x, y, &line, area.width);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::snap;
    use ratatui::{Terminal, backend::TestBackend};

    /// Helper: tworzy przykładowe załączniki do testów.
    fn sample_items() -> Vec<AttachmentInfo> {
        vec![
            AttachmentInfo {
                filename: "screenshot.png".into(),
                size_display: "42 KB".into(),
                path: PathBuf::from("/tmp/screenshot.png"),
            },
            AttachmentInfo {
                filename: "report.pdf".into(),
                size_display: "1.2 MB".into(),
                path: PathBuf::from("/tmp/report.pdf"),
            },
            AttachmentInfo {
                filename: "data.csv".into(),
                size_display: "8 KB".into(),
                path: PathBuf::from("/tmp/data.csv"),
            },
        ]
    }

    /// Helper: renderuje widget do stringa snapshot.
    fn render(state: &mut AttachmentPanelState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = ratatui::layout::Rect::new(0, 0, width, height);
                frame.render_stateful_widget(AttachmentPanelWidget, area, state);
            })
            .expect("draw");
        snap(terminal.backend().buffer())
    }

    // ── AttachmentPanelState: unit tests ─────────────────────────────

    #[test]
    fn new_state_is_empty_unfocused() {
        let state = AttachmentPanelState::new();
        assert!(state.is_empty());
        assert_eq!(state.selected, 0);
        assert!(!state.focused);
    }

    #[test]
    fn with_items_sets_items_and_defaults() {
        let state = AttachmentPanelState::with_items(sample_items());
        assert_eq!(state.items.len(), 3);
        assert_eq!(state.selected, 0);
        assert!(!state.focused);
    }

    #[test]
    fn is_empty_true_when_no_items() {
        let state = AttachmentPanelState::new();
        assert!(state.is_empty());
    }

    #[test]
    fn is_empty_false_when_has_items() {
        let state = AttachmentPanelState::with_items(sample_items());
        assert!(!state.is_empty());
    }

    #[test]
    fn move_down_advances_selection() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.move_down();
        assert_eq!(state.selected, 1);
        state.move_down();
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn move_down_wraps_at_end() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.selected = 2;
        state.move_down();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn move_up_decrements_selection() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.selected = 2;
        state.move_up();
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn move_up_wraps_at_start() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.selected = 0;
        state.move_up();
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn move_up_empty_is_noop() {
        let mut state = AttachmentPanelState::new();
        state.move_up(); // nie panikuje
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn move_down_empty_is_noop() {
        let mut state = AttachmentPanelState::new();
        state.move_down();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn remove_selected_removes_correct_item() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.selected = 1;
        let removed = state.remove_selected().expect("should remove");
        assert_eq!(removed.filename, "report.pdf");
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.items[0].filename, "screenshot.png");
        assert_eq!(state.items[1].filename, "data.csv");
    }

    #[test]
    fn remove_selected_clamps_index_after_last_item() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.selected = 2; // ostatni
        state.remove_selected();
        // Po usunięciu ostatniego, selected powinien wskazywać na nowy ostatni
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn remove_selected_on_empty_returns_none() {
        let mut state = AttachmentPanelState::new();
        assert!(state.remove_selected().is_none());
    }

    #[test]
    fn clamp_corrects_out_of_bounds_selected() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.selected = 10; // poza zakresem
        state.clamp();
        assert_eq!(state.selected, 2); // max index dla 3 elementów
    }

    #[test]
    fn clamp_on_empty_resets_to_zero() {
        let mut state = AttachmentPanelState::new();
        state.selected = 5;
        state.clamp();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn add_attachment_appends_item() {
        let mut state = AttachmentPanelState::new();
        assert!(state.is_empty());
        state.add_attachment(AttachmentInfo {
            filename: "file.txt".into(),
            size_display: "1 KB".into(),
            path: PathBuf::from("/tmp/file.txt"),
        });
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].filename, "file.txt");
    }

    #[test]
    fn add_attachment_multiple_preserves_order() {
        let mut state = AttachmentPanelState::new();
        for i in 0..3u8 {
            state.add_attachment(AttachmentInfo {
                filename: format!("file{i}.txt"),
                size_display: "1 KB".into(),
                path: PathBuf::from(format!("/tmp/file{i}.txt")),
            });
        }
        assert_eq!(state.items.len(), 3);
        assert_eq!(state.items[0].filename, "file0.txt");
        assert_eq!(state.items[2].filename, "file2.txt");
    }

    #[test]
    fn set_focused_changes_focus_state() {
        let mut state = AttachmentPanelState::new();
        assert!(!state.is_focused());
        state.set_focused(true);
        assert!(state.is_focused());
        state.set_focused(false);
        assert!(!state.is_focused());
    }

    #[test]
    fn is_focused_reflects_focused_field() {
        let mut state = AttachmentPanelState::new();
        state.focused = true;
        assert!(state.is_focused());
        state.focused = false;
        assert!(!state.is_focused());
    }

    #[test]
    fn has_items_false_when_empty() {
        let state = AttachmentPanelState::new();
        assert!(!state.has_items());
    }

    #[test]
    fn has_items_true_when_not_empty() {
        let state = AttachmentPanelState::with_items(sample_items());
        assert!(state.has_items());
    }

    #[test]
    fn has_items_and_is_empty_are_complementary() {
        let mut state = AttachmentPanelState::new();
        assert_eq!(state.has_items(), !state.is_empty());
        state.add_attachment(AttachmentInfo {
            filename: "x.txt".into(),
            size_display: "1 B".into(),
            path: PathBuf::from("/tmp/x.txt"),
        });
        assert_eq!(state.has_items(), !state.is_empty());
    }

    #[test]
    fn remove_selected_returns_path_of_removed_item() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.selected = 0;
        let removed = state.remove_selected().expect("should return item");
        assert_eq!(removed.path, PathBuf::from("/tmp/screenshot.png"));
    }

    #[test]
    fn add_then_select_then_remove_roundtrip() {
        let mut state = AttachmentPanelState::new();
        // Dodaj załącznik
        state.add_attachment(AttachmentInfo {
            filename: "round.txt".into(),
            size_display: "1 KB".into(),
            path: PathBuf::from("/tmp/round.txt"),
        });
        assert!(!state.is_empty());
        // Zaznacz jedyny element
        state.selected = 0;
        // Usuń go i sprawdź że wrócił pusty stan
        let removed = state.remove_selected().expect("should return item");
        assert_eq!(removed.filename, "round.txt");
        assert!(state.is_empty());
        assert_eq!(state.selected, 0);
    }

    // ── Widget rendering: snapshot tests ─────────────────────────────

    #[test]
    fn snapshot_empty_list_renders_nothing() {
        let mut state = AttachmentPanelState::new();
        let output = render(&mut state, 60, 5);
        // Puste linie — panel nie rysuje nic
        assert!(!output.contains("📎"));
    }

    #[test]
    fn snapshot_single_item_unfocused() {
        let mut state = AttachmentPanelState::with_items(vec![AttachmentInfo {
            filename: "image.png".into(),
            size_display: "128 KB".into(),
            path: PathBuf::from("/tmp/image.png"),
        }]);
        let output = render(&mut state, 60, 3);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_three_items_first_selected_focused() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.focused = true;
        state.selected = 0;
        let output = render(&mut state, 60, 5);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_three_items_second_selected_focused() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.focused = true;
        state.selected = 1;
        let output = render(&mut state, 60, 5);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_unfocused_no_highlight() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.focused = false;
        state.selected = 1;
        let output = render(&mut state, 60, 5);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_narrow_terminal() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.focused = true;
        state.selected = 0;
        let output = render(&mut state, 25, 5);
        insta::assert_snapshot!(output);
    }

    // ── Widget rendering: property tests ─────────────────────────────

    #[test]
    fn focused_selected_item_has_primary_fg() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.focused = true;
        state.selected = 0;

        let backend = TestBackend::new(60, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = ratatui::layout::Rect::new(0, 0, 60, 5);
                frame.render_stateful_widget(AttachmentPanelWidget, area, &mut state);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        // Pierwszy wiersz (selected, focused) powinien mieć primary fg
        let first_cell = buffer.cell((0, 0)).expect("cell");
        assert_eq!(
            first_cell.fg, DEFAULT_THEME.primary,
            "Zaznaczony focused element ma primary fg"
        );
    }

    #[test]
    fn unfocused_item_has_muted_fg() {
        let mut state = AttachmentPanelState::with_items(sample_items());
        state.focused = false; // nie focused → muted
        state.selected = 0;

        let backend = TestBackend::new(60, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = ratatui::layout::Rect::new(0, 0, 60, 5);
                frame.render_stateful_widget(AttachmentPanelWidget, area, &mut state);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        let first_cell = buffer.cell((0, 0)).expect("cell");
        assert_eq!(
            first_cell.fg, DEFAULT_THEME.muted,
            "Unfocused element ma muted fg"
        );
    }

    #[test]
    fn empty_panel_leaves_buffer_unchanged() {
        let mut state = AttachmentPanelState::new();

        let backend = TestBackend::new(30, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = ratatui::layout::Rect::new(0, 0, 30, 3);
                frame.render_stateful_widget(AttachmentPanelWidget, area, &mut state);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        // Wszystkie komórki powinny być puste (domyślny stan)
        for y in 0..3u16 {
            for x in 0..30u16 {
                let cell = buffer.cell((x, y)).expect("cell");
                assert_eq!(cell.symbol(), " ", "Pusta lista → brak tekstu w bufferze");
            }
        }
    }

    #[test]
    fn items_truncated_when_exceed_height() {
        let mut state = AttachmentPanelState::with_items(sample_items()); // 3 items
        state.focused = true;

        let backend = TestBackend::new(60, 2); // tylko 2 wiersze → mieści 2 z 3
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = ratatui::layout::Rect::new(0, 0, 60, 2);
                frame.render_stateful_widget(AttachmentPanelWidget, area, &mut state);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        // Wiersz 0 i 1 zawierają dane, wiersz 2 nie istnieje (height=2)
        let row0 = buffer.cell((0, 0)).expect("cell");
        assert_ne!(
            row0.symbol(),
            " ",
            "Pierwszy wiersz powinien mieć zawartość"
        );
    }

    #[test]
    fn scroll_keeps_selected_item_visible() {
        // 5 elementów, wysokość 2 — selected wskazuje na 4. element (idx=4)
        // Scroll offset powinien wynosić 3 → widoczne elementy to [3, 4]
        let items: Vec<AttachmentInfo> = (0..5)
            .map(|i| AttachmentInfo {
                filename: format!("file{i}.txt"),
                size_display: "1 KB".into(),
                path: PathBuf::from(format!("/tmp/file{i}.txt")),
            })
            .collect();

        let mut state = AttachmentPanelState::with_items(items);
        state.focused = true;
        state.selected = 4; // ostatni element, poza widocznym zakresem bez scrolla

        let backend = TestBackend::new(40, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = ratatui::layout::Rect::new(0, 0, 40, 2);
                frame.render_stateful_widget(AttachmentPanelWidget, area, &mut state);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();

        // Wiersz 1 (dolny) to zaznaczony element — powinien zawierać "file4.txt"
        let row_content: String = (0..40u16)
            .map(|x| buffer.cell((x, 1)).expect("cell").symbol().to_string())
            .collect();
        assert!(
            row_content.contains("file4"),
            "Zaznaczony element (file4.txt) powinien być widoczny po scrollowaniu, got: {row_content:?}"
        );

        // Wiersz 0 to element poprzedni — "file3.txt"
        let top_row: String = (0..40u16)
            .map(|x| buffer.cell((x, 0)).expect("cell").symbol().to_string())
            .collect();
        assert!(
            top_row.contains("file3"),
            "Element powyżej selected (file3.txt) powinien być widoczny, got: {top_row:?}"
        );
    }

    #[test]
    fn scroll_selected_at_zero_shows_top() {
        // Gdy selected=0, scroll_offset=0 — powinny być widoczne pierwsze elementy
        let items: Vec<AttachmentInfo> = (0..5)
            .map(|i| AttachmentInfo {
                filename: format!("item{i}.txt"),
                size_display: "1 KB".into(),
                path: PathBuf::from(format!("/tmp/item{i}.txt")),
            })
            .collect();

        let mut state = AttachmentPanelState::with_items(items);
        state.selected = 0;

        let backend = TestBackend::new(40, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = ratatui::layout::Rect::new(0, 0, 40, 2);
                frame.render_stateful_widget(AttachmentPanelWidget, area, &mut state);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        let top_row: String = (0..40u16)
            .map(|x| buffer.cell((x, 0)).expect("cell").symbol().to_string())
            .collect();
        assert!(
            top_row.contains("item0"),
            "Gdy selected=0, pierwszy element powinien być na górze, got: {top_row:?}"
        );
    }
}
