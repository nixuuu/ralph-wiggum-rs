//! Key handling for OrchestrateApp.
//!
//! Migrated from `dashboard_input.rs` — full keyboard routing:
//! quit flow, focus navigation, restart, preview, scroll, overlay.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use crate::commands::task::orchestrate::app::{OrchestrateApp, QuitState, RestartState};
use crate::commands::task::orchestrate::command_registry::{
    OrchestrateAction, execute_palette_action,
};
use crate::tui::events::EventResult;
use crate::tui::keybindings::{KeyAction, View};
use crate::tui::widgets::{InputAction, PaletteAction};

impl OrchestrateApp {
    /// Top-level key event handler (migrated from dashboard_input.rs).
    ///
    /// Note: Ctrl+C is intercepted by `App::run` before reaching this handler,
    /// producing `EventResult::Shutdown` directly. This method only handles
    /// application-level keys (quit flow, navigation, restart, overlay).
    ///
    /// Priorytet routingu:
    /// 1. Command palette (gdy otwarta) — pochłania wszystkie klawisze
    /// 2. Text input overlay (gdy aktywny) — pochłania wszystkie klawisze
    /// 3. Sidebar (gdy focused) — routing do sidebar
    /// 4. Globalne klawisze aplikacji
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> EventResult {
        // Priorytet 1: Command palette pochłania wszystkie klawisze gdy jest otwarta
        if self.is_palette_open() {
            return self.handle_palette_key(key);
        }

        // Priorytet 2: Text input overlay pochłania wszystkie klawisze gdy jest aktywny
        if self.is_overlay_active() {
            return self.handle_overlay_key(key);
        }

        // Priorytet 3: Command palette — keybinding przez KeybindingResolver (domyślnie Ctrl+P,
        // konfiguralny przez .ralph.toml)
        if matches!(
            self.resolver.resolve(&key, View::Orchestrate),
            Some(KeyAction::CommandPalette)
        ) {
            self.open_command_palette();
            return EventResult::Consumed;
        }

        // Priorytet 4: Sidebar (gdy focused)
        if self.sidebar_focused && self.sidebar_state.visible {
            return self.handle_sidebar_key(key);
        }

        match key.code {
            KeyCode::Char('q') => self.handle_quit_key(),
            KeyCode::Enter => self.handle_enter_key(),
            KeyCode::Tab if !key.modifiers.contains(KeyModifiers::SHIFT) => self.handle_tab(true),
            KeyCode::BackTab => self.handle_tab(false),
            KeyCode::Esc => self.handle_esc_key(),
            KeyCode::Char(ch) if ch.is_ascii_digit() && ch != '0' => {
                self.handle_direct_focus(ch as u32 - '0' as u32)
            }
            KeyCode::Char('p') => self.handle_toggle_preview(),
            KeyCode::Char('t') => self.handle_toggle_sidebar(),
            KeyCode::Char('i') => self.handle_input_overlay_key(),
            KeyCode::Up => self.handle_scroll(-(self.scroll_step as i32)),
            KeyCode::Down => self.handle_scroll(self.scroll_step as i32),
            KeyCode::Left => self.handle_scroll(i32::MIN),
            KeyCode::Right => self.handle_scroll(i32::MAX),
            KeyCode::Char('r') => {
                self.reload_requested = true;
                EventResult::Consumed
            }
            KeyCode::Char('R') => self.handle_restart_key(),
            KeyCode::Char('y') => self.handle_confirm_restart(),
            KeyCode::Char('n') => self.handle_cancel_restart(),
            KeyCode::Char('h') => {
                self.show_idle = !self.show_idle;
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }

    fn handle_quit_key(&mut self) -> EventResult {
        self.cancel_restart_pending();

        if self.graceful_shutdown {
            // Already draining — force shutdown
            return EventResult::Shutdown;
        }

        match self.quit_state {
            QuitState::Pending => {
                // Second 'q' — confirm graceful shutdown
                self.graceful_shutdown = true;
                self.quit_state = QuitState::Normal;
                EventResult::Quit
            }
            QuitState::Normal => {
                // First 'q' — enter pending state
                self.quit_state = QuitState::Pending;
                EventResult::Consumed
            }
        }
    }

    fn handle_enter_key(&mut self) -> EventResult {
        if self.quit_state == QuitState::Pending {
            self.graceful_shutdown = true;
            self.quit_state = QuitState::Normal;
            EventResult::Quit
        } else {
            EventResult::Consumed
        }
    }

    /// Obsługa klawiszy gdy command palette jest otwarta.
    ///
    /// Deleguje do `CommandPaletteState::handle_key()`, a następnie:
    /// - `PaletteAction::Select(id)` → parsuje ID → wykonuje `OrchestrateAction`
    /// - `PaletteAction::Close` → zamyka paletę
    /// - `PaletteAction::Continue` → pochłonięto, bez efektów zewnętrznych
    ///
    /// Specjalny przypadek: Ctrl+P gdy palette otwarta → toggle close (zamknięcie palette).
    fn handle_palette_key(&mut self, key: KeyEvent) -> EventResult {
        // Ctrl+P gdy palette otwarta → toggle close
        if matches!(
            self.resolver.resolve(&key, View::Orchestrate),
            Some(KeyAction::CommandPalette)
        ) {
            self.close_command_palette();
            return EventResult::Consumed;
        }

        // Pobierz aktualne elementy by umożliwić mutable borrow na state
        let palette_action = {
            let Some(ref mut palette) = self.command_palette else {
                return EventResult::Consumed;
            };
            palette.handle_key(key.code)
        };

        match palette_action {
            PaletteAction::Select(id) => {
                // Zamknij paletę przed wykonaniem akcji (akcja może modyfikować stan)
                self.close_command_palette();

                if let Some(action) = OrchestrateAction::from_palette_id(&id) {
                    execute_palette_action(action, self);
                }
                EventResult::Consumed
            }
            PaletteAction::Close => {
                self.close_command_palette();
                EventResult::Consumed
            }
            PaletteAction::Continue => EventResult::Consumed,
        }
    }

    fn handle_esc_key(&mut self) -> EventResult {
        // Priority 1: cancel restart pending
        if self.cancel_restart_pending() {
            return EventResult::Consumed;
        }
        // Priority 2: cancel quit pending
        if self.cancel_quit_pending() {
            return EventResult::Consumed;
        }
        // Priority 3: unfocus sidebar
        if self.sidebar_focused {
            self.sidebar_focused = false;
            return EventResult::Consumed;
        }
        // Priority 4: close task preview
        if self.show_task_preview {
            self.show_task_preview = false;
            return EventResult::Consumed;
        }
        // Priority 5: unfocus worker
        self.focused_worker = None;
        EventResult::Consumed
    }

    fn handle_tab(&mut self, forward: bool) -> EventResult {
        self.cancel_restart_pending();
        self.cancel_quit_pending();

        if self.active_worker_ids.is_empty() {
            self.focused_worker = None;
            return EventResult::Consumed;
        }

        let current = self.focused_worker.unwrap_or(0);
        let pos = self.active_worker_ids.iter().position(|&id| id == current);

        let next = if forward {
            match pos {
                Some(p) => self.active_worker_ids[(p + 1) % self.active_worker_ids.len()],
                None => self.active_worker_ids[0],
            }
        } else {
            match pos {
                Some(0) => *self.active_worker_ids.last().unwrap(),
                Some(p) => self.active_worker_ids[p - 1],
                None => *self.active_worker_ids.last().unwrap(),
            }
        };

        self.focused_worker = Some(next);
        // Reset scroll on focus change
        if let Some(panel) = self.panels.get_mut(&next) {
            panel.scroll_offset = 0;
        }
        EventResult::Consumed
    }

    fn handle_direct_focus(&mut self, n: u32) -> EventResult {
        self.cancel_restart_pending();
        self.cancel_quit_pending();

        let index = (n as usize).saturating_sub(1);
        if index < self.active_worker_ids.len() {
            let worker_id = self.active_worker_ids[index];
            self.focused_worker = Some(worker_id);
            if let Some(panel) = self.panels.get_mut(&worker_id) {
                panel.scroll_offset = 0;
            }
        }
        EventResult::Consumed
    }

    fn handle_toggle_preview(&mut self) -> EventResult {
        self.cancel_restart_pending();
        self.cancel_quit_pending();
        self.show_task_preview = !self.show_task_preview;
        EventResult::Consumed
    }

    fn handle_toggle_sidebar(&mut self) -> EventResult {
        self.cancel_restart_pending();
        self.cancel_quit_pending();
        self.toggle_sidebar();
        EventResult::Consumed
    }

    fn handle_scroll(&mut self, delta: i32) -> EventResult {
        self.cancel_restart_pending();
        self.cancel_quit_pending();
        self.apply_scroll(delta);
        EventResult::Consumed
    }

    fn handle_restart_key(&mut self) -> EventResult {
        self.cancel_quit_pending();

        if self.graceful_shutdown {
            return EventResult::Consumed;
        }

        let focused = match self.focused_worker {
            Some(id) if id > 0 => id,
            _ => return EventResult::Consumed,
        };

        // Ignore if restart already pending
        if matches!(self.restart_state, RestartState::Pending { .. }) {
            return EventResult::Consumed;
        }

        // Ignore if focused worker not in active set
        if !self.active_worker_ids.contains(&focused) {
            return EventResult::Consumed;
        }

        self.restart_state = RestartState::Pending { worker_id: focused };
        EventResult::Consumed
    }

    fn handle_confirm_restart(&mut self) -> EventResult {
        if let RestartState::Pending { worker_id } = self.restart_state {
            self.restart_state = RestartState::Confirmed { worker_id };
        }
        EventResult::Consumed
    }

    fn handle_cancel_restart(&mut self) -> EventResult {
        if matches!(self.restart_state, RestartState::Pending { .. }) {
            self.restart_state = RestartState::None;
        }
        EventResult::Consumed
    }

    // ── Sidebar key handling ───────────────────────────────────────

    fn handle_sidebar_key(&mut self, key: KeyEvent) -> EventResult {
        match key.code {
            // Navigation
            KeyCode::Up | KeyCode::Char('k') => {
                self.sidebar_state.select_prev();
                EventResult::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.sidebar_state.select_next();
                EventResult::Consumed
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.sidebar_state.toggle_expand();
                EventResult::Consumed
            }
            // Resize sidebar
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char(']') => {
                self.sidebar_state.grow();
                EventResult::Consumed
            }
            KeyCode::Char('-') | KeyCode::Char('[') => {
                self.sidebar_state.shrink();
                EventResult::Consumed
            }
            // Esc — unfocus sidebar
            KeyCode::Esc => {
                self.unfocus_sidebar();
                EventResult::Consumed
            }
            // Pass-through: global keys
            KeyCode::Char('q') => self.handle_quit_key(),
            KeyCode::Char('t') => self.handle_toggle_sidebar(),
            KeyCode::Char('r') => {
                self.reload_requested = true;
                EventResult::Consumed
            }
            KeyCode::Char('R') => self.handle_restart_key(),
            KeyCode::Char('p') => self.handle_toggle_preview(),
            KeyCode::Tab if !key.modifiers.contains(KeyModifiers::SHIFT) => self.handle_tab(true),
            KeyCode::BackTab => self.handle_tab(false),
            KeyCode::Char(ch) if ch.is_ascii_digit() && ch != '0' => {
                self.handle_direct_focus(ch as u32 - '0' as u32)
            }
            _ => EventResult::Ignored,
        }
    }

    // ── Mouse handling ──────────────────────────────────────────────

    /// Zwraca worker_id pierwszego panelu, którego rect zawiera punkt (column, row).
    ///
    /// Używa cache `grid_rects` z ostatniego draw().
    fn hit_test_worker(&self, column: u16, row: u16) -> Option<u32> {
        use ratatui::layout::Position;
        let pos = Position::new(column, row);
        self.grid_rects
            .iter()
            .find(|&&(_, rect)| rect.contains(pos))
            .map(|&(worker_id, _)| worker_id)
    }

    /// Obsługa zdarzeń myszy.
    ///
    /// Routing:
    /// - `MouseMoved` → hit-test na `grid_rects` → focus na worker panel pod kursorem
    /// - `ScrollUp`/`ScrollDown` → hit-test na grid_rects → scroll panelu pod kursorem
    /// - Pozostałe zdarzenia → ignorowane
    ///
    /// `MouseDown Left` wykonuje hit-test i ustawia focus jak `MouseMoved`,
    /// ale dodatkowo anuluje `quit_pending` i `restart_pending`.
    /// Scroll offset resetuje się do 0 (follow tail) przez `set_focus()`.
    ///
    /// Guard: gdy command palette lub text input overlay jest aktywny, zdarzenia myszy są ignorowane.
    /// Spójna blokada z `handle_key()` (palette → priorytet 1, overlay → priorytet 2).
    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) -> EventResult {
        // Guard: overlay lub palette aktywne — ignoruj zdarzenia myszy.
        // Zapobiega zmianie focusu workera gdy użytkownik pisze w overlay lub nawiguje paletą.
        if self.is_palette_open() || self.is_overlay_active() {
            return EventResult::Ignored;
        }

        match mouse.kind {
            MouseEventKind::Moved => {
                if let Some(worker_id) = self.hit_test_worker(mouse.column, mouse.row) {
                    self.set_focus(Some(worker_id));
                    EventResult::Consumed
                } else {
                    // Kursor poza wszystkimi panelami — brak zmiany focusu
                    EventResult::Ignored
                }
            }
            // Left click na worker panel → focus, anuluje quit/restart pending
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(worker_id) = self.hit_test_worker(mouse.column, mouse.row) {
                    // Klik na worker panel — anuluj oczekujące dialogi i ustaw focus.
                    // set_focus() resetuje scroll_offset do 0 (follow tail) przy zmianie focusu.
                    self.cancel_quit_pending();
                    self.cancel_restart_pending();
                    self.set_focus(Some(worker_id));
                    EventResult::Consumed
                } else {
                    EventResult::Ignored
                }
            }
            MouseEventKind::ScrollUp => self.handle_mouse_scroll(mouse.column, mouse.row, true),
            MouseEventKind::ScrollDown => {
                self.handle_mouse_scroll(mouse.column, mouse.row, false)
            }
            _ => EventResult::Ignored,
        }
    }

    // ── Overlay handling ────────────────────────────────────────────

    pub(crate) fn is_overlay_active(&self) -> bool {
        self.shared_overlay
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) -> EventResult {
        let mut overlay_guard = match self.shared_overlay.lock() {
            Ok(g) => g,
            Err(_) => return EventResult::Consumed,
        };

        if let Some(ref mut overlay) = *overlay_guard {
            let worker_id = overlay.target_worker_id();
            let action = overlay.handle_key(key);
            match action {
                InputAction::Send(message) => {
                    // Buforuj wiadomość — orchestrator loop konsumuje przez take_pending_messages()
                    self.pending_user_messages.push_back((worker_id, message));
                    *overlay_guard = None;
                    EventResult::Consumed
                }
                InputAction::Cancel => {
                    *overlay_guard = None;
                    EventResult::Consumed
                }
                InputAction::Continue => EventResult::Consumed,
            }
        } else {
            EventResult::Consumed
        }
    }

    /// Obsługa scroll myszy na pozycji (col, row).
    ///
    /// Priorytet:
    /// 1. Gdy overlay aktywny — ignoruj (overlay obsługuje własny input)
    /// 2. Gdy task preview aktywny — scrolluj preview
    /// 3. Hit-test na grid_rects → scroll panelu pod kursorem
    /// 4. Kursor poza panelami → ignoruj
    ///
    /// `scroll_up=true` → w górę (ku starszym liniom, offset rośnie).
    /// `scroll_up=false` → w dół (ku nowszym liniom, offset maleje).
    fn handle_mouse_scroll(&mut self, col: u16, row: u16, scroll_up: bool) -> EventResult {
        // Overlay aktywny — nie obsługuj scrollu myszy (overlay ma własny input)
        if self.is_overlay_active() {
            return EventResult::Ignored;
        }

        let pos = Position { x: col, y: row };
        let step = self.scroll_step as i32;
        // delta < 0 = w górę (ku starszym), delta > 0 = w dół (ku nowszym)
        let delta = if scroll_up { -step } else { step };

        // Gdy task preview aktywny — scrolluj preview niezależnie od pozycji kursora
        if self.show_task_preview {
            match delta {
                d if d < 0 => {
                    let up = (-d) as usize;
                    self.preview_scroll_offset = self.preview_scroll_offset.saturating_sub(up);
                }
                d if d > 0 => {
                    let down = d as usize;
                    self.preview_scroll_offset = self.preview_scroll_offset.saturating_add(down);
                }
                _ => {}
            }
            return EventResult::Consumed;
        }

        // Hit-test: znajdź worker panel pod kursorem.
        // Pobieramy worker_id przed modyfikacją panels — kończy borrow grid_rects.
        let worker_id = self
            .grid_rects
            .iter()
            .find(|(_, rect)| rect.contains(pos))
            .map(|(id, _)| *id);

        if let Some(wid) = worker_id {
            if let Some(panel) = self.panels.get_mut(&wid) {
                panel.scroll_output(delta);
            }
            return EventResult::Consumed;
        }

        // Kursor poza jakimkolwiek panelem — ignoruj
        EventResult::Ignored
    }

    fn handle_input_overlay_key(&mut self) -> EventResult {
        use crate::commands::task::orchestrate::events::WorkerPhase;

        let focused = match self.focused_worker {
            Some(id) if id > 0 => id,
            _ => return EventResult::Consumed,
        };

        // Guard: focused worker not in active set
        if !self.active_worker_ids.contains(&focused) {
            return EventResult::Consumed;
        }

        // Guard: worker must be in Claude phase (Implement, Review, Fix, or ReviewFix)
        let is_claude_phase = self
            .panels
            .get(&focused)
            .and_then(|p| p.status.phase.as_ref())
            .map(|phase| {
                matches!(
                    phase,
                    WorkerPhase::Implement
                        | WorkerPhase::Review
                        | WorkerPhase::Fix
                        | WorkerPhase::ReviewFix
                )
            })
            .unwrap_or(false);

        if !is_claude_phase {
            return EventResult::Consumed;
        }

        // Create overlay instance
        *self.shared_overlay.lock().expect("shared_overlay poisoned") =
            Some(crate::tui::widgets::TextInputOverlay::new(focused));
        EventResult::Consumed
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseEvent, MouseEventKind,
    };

    use crate::commands::task::orchestrate::app::OrchestrateApp;
    use crate::commands::task::orchestrate::app::{QuitState, RestartState};
    use crate::commands::task::orchestrate::worker_status::WorkerState;
    use crate::tui::events::{AppEvent, EventResult};
    use crate::tui::widgets::TextInputOverlay;

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn make_app(worker_count: u32) -> OrchestrateApp {
        let mut app = OrchestrateApp::new(worker_count, Arc::new(Mutex::new(None)));
        // Disable sidebar focus for most tests — sidebar-specific tests enable it explicitly
        app.sidebar_focused = false;
        app
    }

    /// Helper: make all workers active (Implementing state) for navigation tests.
    fn activate_workers(app: &mut OrchestrateApp, ids: &[u32]) {
        for &id in ids {
            if let Some(panel) = app.panels.get_mut(&id) {
                panel.status.state = WorkerState::Implementing;
            }
        }
        app.refresh_active_worker_ids();
    }

    // ── Quit flow tests ─────────────────────────────────────────────

    #[test]
    fn quit_first_q_enters_pending() {
        let mut app = make_app(1);
        let key = make_key(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = app.handle_key(key);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.quit_state, QuitState::Pending);
    }

    #[test]
    fn quit_second_q_triggers_graceful_shutdown() {
        let mut app = make_app(1);
        app.quit_state = QuitState::Pending;
        let key = make_key(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = app.handle_key(key);
        assert_eq!(result, EventResult::Quit);
        assert!(app.graceful_shutdown);
    }

    #[test]
    fn quit_enter_confirms_pending() {
        let mut app = make_app(1);
        app.quit_state = QuitState::Pending;
        let key = make_key(KeyCode::Enter, KeyModifiers::NONE);
        let result = app.handle_key(key);
        assert_eq!(result, EventResult::Quit);
        assert!(app.graceful_shutdown);
    }

    #[test]
    fn quit_esc_cancels_pending() {
        let mut app = make_app(1);
        app.quit_state = QuitState::Pending;
        let key = make_key(KeyCode::Esc, KeyModifiers::NONE);
        let result = app.handle_key(key);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.quit_state, QuitState::Normal);
    }

    #[test]
    fn quit_during_shutdown_forces() {
        let mut app = make_app(1);
        app.graceful_shutdown = true;
        let key = make_key(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = app.handle_key(key);
        assert_eq!(result, EventResult::Shutdown);
    }

    // ── Tab navigation tests ────────────────────────────────────────

    #[test]
    fn tab_cycles_through_active_workers() {
        let mut app = make_app(3);
        activate_workers(&mut app, &[1, 2, 3]);

        let tab = make_key(KeyCode::Tab, KeyModifiers::NONE);

        app.handle_key(tab);
        assert_eq!(app.focused_worker, Some(1));

        app.handle_key(tab);
        assert_eq!(app.focused_worker, Some(2));

        app.handle_key(tab);
        assert_eq!(app.focused_worker, Some(3));

        // Wrap around
        app.handle_key(tab);
        assert_eq!(app.focused_worker, Some(1));
    }

    #[test]
    fn backtab_cycles_backward() {
        let mut app = make_app(3);
        activate_workers(&mut app, &[1, 2, 3]);

        let backtab = make_key(KeyCode::BackTab, KeyModifiers::NONE);

        // From None → last active
        app.handle_key(backtab);
        assert_eq!(app.focused_worker, Some(3));

        app.handle_key(backtab);
        assert_eq!(app.focused_worker, Some(2));

        app.handle_key(backtab);
        assert_eq!(app.focused_worker, Some(1));

        // Wrap around
        app.handle_key(backtab);
        assert_eq!(app.focused_worker, Some(3));
    }

    #[test]
    fn tab_with_no_active_workers_unfocuses() {
        let mut app = make_app(3);
        // All workers are idle by default
        app.refresh_active_worker_ids();
        app.focused_worker = Some(1);

        let tab = make_key(KeyCode::Tab, KeyModifiers::NONE);
        app.handle_key(tab);
        assert_eq!(app.focused_worker, None);
    }

    #[test]
    fn digit_key_focuses_nth_active_worker() {
        let mut app = make_app(5);
        activate_workers(&mut app, &[2, 4, 5]);

        // '1' → first active (worker 2)
        let key1 = make_key(KeyCode::Char('1'), KeyModifiers::NONE);
        app.handle_key(key1);
        assert_eq!(app.focused_worker, Some(2));

        // '2' → second active (worker 4)
        let key2 = make_key(KeyCode::Char('2'), KeyModifiers::NONE);
        app.handle_key(key2);
        assert_eq!(app.focused_worker, Some(4));

        // '3' → third active (worker 5)
        let key3 = make_key(KeyCode::Char('3'), KeyModifiers::NONE);
        app.handle_key(key3);
        assert_eq!(app.focused_worker, Some(5));
    }

    #[test]
    fn digit_key_out_of_range_no_change() {
        let mut app = make_app(3);
        activate_workers(&mut app, &[1, 2]);
        app.focused_worker = Some(1);

        // '5' — out of range
        let key5 = make_key(KeyCode::Char('5'), KeyModifiers::NONE);
        app.handle_key(key5);
        assert_eq!(app.focused_worker, Some(1)); // unchanged
    }

    // ── Restart flow tests ──────────────────────────────────────────

    #[test]
    fn restart_key_initiates_restart() {
        let mut app = make_app(3);
        activate_workers(&mut app, &[1, 2, 3]);
        app.focused_worker = Some(2);

        let key_r = make_key(KeyCode::Char('R'), KeyModifiers::NONE);
        app.handle_key(key_r);

        assert_eq!(app.restart_state, RestartState::Pending { worker_id: 2 });
    }

    #[test]
    fn restart_key_ignored_no_focus() {
        let mut app = make_app(3);
        activate_workers(&mut app, &[1, 2, 3]);

        let key_r = make_key(KeyCode::Char('R'), KeyModifiers::NONE);
        app.handle_key(key_r);

        assert_eq!(app.restart_state, RestartState::None);
    }

    #[test]
    fn restart_confirm_y() {
        let mut app = make_app(3);
        app.restart_state = RestartState::Pending { worker_id: 2 };

        let key_y = make_key(KeyCode::Char('y'), KeyModifiers::NONE);
        app.handle_key(key_y);

        assert_eq!(app.restart_state, RestartState::Confirmed { worker_id: 2 });
    }

    #[test]
    fn restart_cancel_n() {
        let mut app = make_app(3);
        app.restart_state = RestartState::Pending { worker_id: 2 };

        let key_n = make_key(KeyCode::Char('n'), KeyModifiers::NONE);
        app.handle_key(key_n);

        assert_eq!(app.restart_state, RestartState::None);
    }

    #[test]
    fn restart_cancel_esc() {
        let mut app = make_app(3);
        app.restart_state = RestartState::Pending { worker_id: 2 };

        let key_esc = make_key(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(key_esc);

        assert_eq!(app.restart_state, RestartState::None);
    }

    #[test]
    fn restart_ignored_during_shutdown() {
        let mut app = make_app(3);
        activate_workers(&mut app, &[1, 2, 3]);
        app.focused_worker = Some(1);
        app.graceful_shutdown = true;

        let key_r = make_key(KeyCode::Char('R'), KeyModifiers::NONE);
        app.handle_key(key_r);

        assert_eq!(app.restart_state, RestartState::None);
    }

    #[test]
    fn restart_double_r_ignored() {
        let mut app = make_app(3);
        activate_workers(&mut app, &[1, 2, 3]);
        app.focused_worker = Some(1);

        let key_r = make_key(KeyCode::Char('R'), KeyModifiers::NONE);
        app.handle_key(key_r);
        assert_eq!(app.restart_state, RestartState::Pending { worker_id: 1 });

        // Focus different worker and press R again
        app.focused_worker = Some(2);
        app.handle_key(key_r);
        // Should still be worker 1
        assert_eq!(app.restart_state, RestartState::Pending { worker_id: 1 });
    }

    // ── Mutual exclusion tests ──────────────────────────────────────

    #[test]
    fn quit_cancels_restart() {
        let mut app = make_app(3);
        app.restart_state = RestartState::Pending { worker_id: 1 };

        let key_q = make_key(KeyCode::Char('q'), KeyModifiers::NONE);
        app.handle_key(key_q);

        assert_eq!(app.restart_state, RestartState::None);
        assert_eq!(app.quit_state, QuitState::Pending);
    }

    #[test]
    fn restart_cancels_quit() {
        let mut app = make_app(3);
        activate_workers(&mut app, &[1, 2, 3]);
        app.quit_state = QuitState::Pending;
        app.focused_worker = Some(1);

        let key_r = make_key(KeyCode::Char('R'), KeyModifiers::NONE);
        app.handle_key(key_r);

        assert_eq!(app.quit_state, QuitState::Normal);
        assert_eq!(app.restart_state, RestartState::Pending { worker_id: 1 });
    }

    #[test]
    fn tab_cancels_both_quit_and_restart() {
        let mut app = make_app(3);
        activate_workers(&mut app, &[1, 2, 3]);
        app.quit_state = QuitState::Pending;
        app.restart_state = RestartState::Pending { worker_id: 1 };
        app.focused_worker = Some(1);

        let tab = make_key(KeyCode::Tab, KeyModifiers::NONE);
        app.handle_key(tab);

        assert_eq!(app.quit_state, QuitState::Normal);
        assert_eq!(app.restart_state, RestartState::None);
    }

    // ── Esc priority tests ──────────────────────────────────────────

    #[test]
    fn esc_priority_restart_over_quit() {
        let mut app = make_app(3);
        app.restart_state = RestartState::Pending { worker_id: 1 };
        app.quit_state = QuitState::Pending;

        let esc = make_key(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(esc);

        // Restart cancelled first
        assert_eq!(app.restart_state, RestartState::None);
        // Quit still pending
        assert_eq!(app.quit_state, QuitState::Pending);
    }

    #[test]
    fn esc_unfocuses_when_no_pending() {
        let mut app = make_app(3);
        app.focused_worker = Some(2);

        let esc = make_key(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(esc);

        assert_eq!(app.focused_worker, None);
    }

    // ── Scroll tests ────────────────────────────────────────────────

    #[test]
    fn scroll_cancels_quit_pending() {
        let mut app = make_app(3);
        app.quit_state = QuitState::Pending;

        let up = make_key(KeyCode::Up, KeyModifiers::NONE);
        app.handle_key(up);

        assert_eq!(app.quit_state, QuitState::Normal);
    }

    #[test]
    fn scroll_cancels_restart_pending() {
        let mut app = make_app(3);
        app.restart_state = RestartState::Pending { worker_id: 1 };

        let down = make_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_key(down);

        assert_eq!(app.restart_state, RestartState::None);
    }

    // ── Preview toggle tests ────────────────────────────────────────

    #[test]
    fn p_key_toggles_preview() {
        let mut app = make_app(1);
        assert!(!app.show_task_preview);

        let key_p = make_key(KeyCode::Char('p'), KeyModifiers::NONE);
        app.handle_key(key_p);
        assert!(app.show_task_preview);

        app.handle_key(key_p);
        assert!(!app.show_task_preview);
    }

    #[test]
    fn esc_closes_preview() {
        let mut app = make_app(1);
        app.show_task_preview = true;

        let esc = make_key(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(esc);

        assert!(!app.show_task_preview);
    }

    // ── Focus cycling edge cases ────────────────────────────────────

    #[test]
    fn focus_cycling_non_sequential_workers() {
        let mut app = make_app(7);
        activate_workers(&mut app, &[2, 5, 7]);

        let tab = make_key(KeyCode::Tab, KeyModifiers::NONE);

        app.handle_key(tab);
        assert_eq!(app.focused_worker, Some(2));
        app.handle_key(tab);
        assert_eq!(app.focused_worker, Some(5));
        app.handle_key(tab);
        assert_eq!(app.focused_worker, Some(7));
        app.handle_key(tab);
        assert_eq!(app.focused_worker, Some(2)); // wrap
    }

    #[test]
    fn focus_cycling_single_worker() {
        let mut app = make_app(3);
        activate_workers(&mut app, &[2]);

        let tab = make_key(KeyCode::Tab, KeyModifiers::NONE);

        app.handle_key(tab);
        assert_eq!(app.focused_worker, Some(2));
        app.handle_key(tab);
        assert_eq!(app.focused_worker, Some(2)); // wraps to itself
    }

    // ── Restart from inactive worker ────────────────────────────────

    #[test]
    fn restart_ignored_for_inactive_worker() {
        let mut app = make_app(3);
        activate_workers(&mut app, &[1, 3]);
        app.focused_worker = Some(2); // Worker 2 is idle

        let key_r = make_key(KeyCode::Char('R'), KeyModifiers::NONE);
        app.handle_key(key_r);

        assert_eq!(app.restart_state, RestartState::None);
    }

    // ── AppState trait compliance ───────────────────────────────────

    #[test]
    fn handle_event_tick_returns_consumed() {
        use crate::tui::app::AppState;
        use crate::tui::keybindings::KeybindingResolver;
        let mut app = make_app(1);
        let resolver = KeybindingResolver::with_defaults();
        let result = app.handle_event(AppEvent::Tick, &resolver);
        assert_eq!(result, EventResult::Consumed);
    }

    #[test]
    fn handle_event_resize_returns_consumed() {
        use crate::tui::app::AppState;
        use crate::tui::keybindings::KeybindingResolver;
        let mut app = make_app(1);
        let resolver = KeybindingResolver::with_defaults();
        let result = app.handle_event(AppEvent::Resize(120, 40), &resolver);
        assert_eq!(result, EventResult::Consumed);
    }

    #[test]
    fn handle_event_unknown_key_returns_ignored() {
        use crate::tui::app::AppState;
        use crate::tui::keybindings::KeybindingResolver;
        let mut app = make_app(1);
        let resolver = KeybindingResolver::with_defaults();
        let key = make_key(KeyCode::F(12), KeyModifiers::NONE);
        let result = app.handle_event(AppEvent::Key(key), &resolver);
        assert_eq!(result, EventResult::Ignored);
    }

    // ── Overlay routing ─────────────────────────────────────────────

    #[test]
    fn overlay_active_routes_all_keys_to_overlay() {
        let overlay = Arc::new(Mutex::new(Some(TextInputOverlay::new(1))));
        let mut app = OrchestrateApp::new(1, overlay);

        // 'q' should NOT trigger quit when overlay is active
        let key_q = make_key(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = app.handle_key(key_q);
        assert_eq!(result, EventResult::Consumed);
        // Quit state should remain Normal
        assert_eq!(app.quit_state, QuitState::Normal);
    }

    // ── Reload requested tests ────────────────────────────────────

    #[test]
    fn r_key_sets_reload_requested() {
        let mut app = make_app(1);
        assert!(!app.reload_requested);

        let key_r = make_key(KeyCode::Char('r'), KeyModifiers::NONE);
        app.handle_key(key_r);

        assert!(app.reload_requested);
    }

    #[test]
    fn take_reload_requested_resets_flag() {
        let mut app = make_app(1);
        app.reload_requested = true;

        assert!(app.take_reload_requested());
        assert!(!app.reload_requested);
        // Second call returns false
        assert!(!app.take_reload_requested());
    }

    // ── Pending user messages tests ───────────────────────────────

    #[test]
    fn overlay_send_buffers_message() {
        let overlay = Arc::new(Mutex::new(Some(TextInputOverlay::new(2))));
        let mut app = OrchestrateApp::new(3, overlay);

        // Type some text into the overlay
        let key_h = make_key(KeyCode::Char('h'), KeyModifiers::NONE);
        let key_i = make_key(KeyCode::Char('i'), KeyModifiers::NONE);
        app.handle_key(key_h);
        app.handle_key(key_i);

        // Send with Ctrl+Enter
        let ctrl_enter = make_key(KeyCode::Enter, KeyModifiers::CONTROL);
        app.handle_key(ctrl_enter);

        // Verify message buffered
        let messages = app.take_pending_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, 2); // worker_id
        assert_eq!(messages[0].1, "hi"); // message text
    }

    #[test]
    fn overlay_cancel_no_message() {
        let overlay = Arc::new(Mutex::new(Some(TextInputOverlay::new(1))));
        let mut app = OrchestrateApp::new(1, overlay);

        // Type text
        let key_a = make_key(KeyCode::Char('a'), KeyModifiers::NONE);
        app.handle_key(key_a);

        // Cancel with Esc
        let esc = make_key(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(esc);

        // No messages should be buffered
        assert!(app.take_pending_messages().is_empty());
        // Overlay should be deactivated
        assert!(!app.is_overlay_active());
    }

    #[test]
    fn take_pending_messages_drains_buffer() {
        let mut app = make_app(1);
        app.pending_user_messages.push_back((1, "msg1".into()));
        app.pending_user_messages.push_back((2, "msg2".into()));

        let msgs = app.take_pending_messages();
        assert_eq!(msgs.len(), 2);

        // Buffer should be empty after take
        assert!(app.take_pending_messages().is_empty());
    }

    // ── Input overlay validation tests ────────────────────────────

    #[test]
    fn input_overlay_requires_claude_phase() {
        use crate::commands::task::orchestrate::events::WorkerPhase;

        let mut app = make_app(3);
        activate_workers(&mut app, &[1]);
        app.focused_worker = Some(1);

        // Worker in Setup phase — 'i' should not activate overlay
        app.panels.get_mut(&1).unwrap().status.phase = Some(WorkerPhase::Setup);
        let key_i = make_key(KeyCode::Char('i'), KeyModifiers::NONE);
        app.handle_key(key_i);
        assert!(!app.is_overlay_active());

        // Worker in Implement phase — 'i' should activate overlay
        app.panels.get_mut(&1).unwrap().status.phase = Some(WorkerPhase::Implement);
        app.handle_key(key_i);
        assert!(app.is_overlay_active());
    }

    #[test]
    fn input_overlay_requires_active_worker() {
        use crate::commands::task::orchestrate::events::WorkerPhase;

        let mut app = make_app(3);
        // Worker 1 is idle (not activated), but in Implement phase
        app.refresh_active_worker_ids();
        app.focused_worker = Some(1);
        app.panels.get_mut(&1).unwrap().status.phase = Some(WorkerPhase::Implement);

        let key_i = make_key(KeyCode::Char('i'), KeyModifiers::NONE);
        app.handle_key(key_i);
        assert!(!app.is_overlay_active());
    }

    #[test]
    fn input_overlay_requires_focus() {
        let mut app = make_app(3);
        activate_workers(&mut app, &[1]);
        // No focus
        app.focused_worker = None;

        let key_i = make_key(KeyCode::Char('i'), KeyModifiers::NONE);
        app.handle_key(key_i);
        assert!(!app.is_overlay_active());
    }

    // ── Scroll home/end tests ─────────────────────────────────────

    #[test]
    fn left_arrow_scrolls_home() {
        let mut app = make_app(1);
        app.focused_worker = Some(1);
        app.panels.get_mut(&1).unwrap().scroll_offset = 5;

        let left = make_key(KeyCode::Left, KeyModifiers::NONE);
        app.handle_key(left);

        // Left = scroll home (i32::MIN via apply_scroll)
        // For worker panel: scroll_offset = usize::MAX (oldest)
        assert_eq!(app.panels[&1].scroll_offset, usize::MAX);
    }

    #[test]
    fn right_arrow_scrolls_end() {
        let mut app = make_app(1);
        app.focused_worker = Some(1);
        app.panels.get_mut(&1).unwrap().scroll_offset = 100;

        let right = make_key(KeyCode::Right, KeyModifiers::NONE);
        app.handle_key(right);

        // Right = scroll end (i32::MAX via apply_scroll → offset 0 = follow tail)
        assert_eq!(app.panels[&1].scroll_offset, 0);
    }

    // ── Enter key in normal state ─────────────────────────────────

    #[test]
    fn enter_key_ignored_in_normal_state() {
        let mut app = make_app(1);
        assert_eq!(app.quit_state, QuitState::Normal);

        let enter = make_key(KeyCode::Enter, KeyModifiers::NONE);
        let result = app.handle_key(enter);

        assert_eq!(result, EventResult::Consumed);
        assert!(!app.graceful_shutdown);
    }

    // ── Ctrl+C handled by App::run ────────────────────────────────

    #[test]
    fn ctrl_c_handled_by_app_run_not_handle_key() {
        use crate::tui::app::AppState;
        use crate::tui::events::is_ctrl_c;

        let mut app = make_app(1);
        let ctrl_c = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        // App::run interceptuje Ctrl+C przed wywołaniem handle_event
        assert!(is_ctrl_c(&ctrl_c));

        // handle_event deleguje do handle_key, ale Ctrl+C nie pasuje do żadnego matcha
        let resolver = crate::tui::keybindings::KeybindingResolver::with_defaults();
        let result = app.handle_event(AppEvent::Key(ctrl_c), &resolver);
        assert_eq!(result, EventResult::Ignored);
    }

    // ── Y/N keys without restart pending ──────────────────────────

    #[test]
    fn y_key_ignored_without_restart_pending() {
        let mut app = make_app(1);
        assert_eq!(app.restart_state, RestartState::None);

        let key_y = make_key(KeyCode::Char('y'), KeyModifiers::NONE);
        app.handle_key(key_y);

        assert_eq!(app.restart_state, RestartState::None);
    }

    #[test]
    fn n_key_ignored_without_restart_pending() {
        let mut app = make_app(1);
        assert_eq!(app.restart_state, RestartState::None);

        let key_n = make_key(KeyCode::Char('n'), KeyModifiers::NONE);
        app.handle_key(key_n);

        assert_eq!(app.restart_state, RestartState::None);
    }

    // ── Command palette tests ───────────────────────────────────────

    #[test]
    fn ctrl_p_opens_command_palette() {
        let mut app = make_app(2);
        assert!(!app.is_palette_open());

        let ctrl_p = make_key(KeyCode::Char('p'), KeyModifiers::CONTROL);
        let result = app.handle_key(ctrl_p);

        assert_eq!(result, EventResult::Consumed);
        assert!(app.is_palette_open());
    }

    #[test]
    fn palette_open_absorbs_all_keys() {
        let mut app = make_app(1);
        app.open_command_palette();
        assert!(app.is_palette_open());

        // 'q' nie powinno triggerować quit gdy palette jest otwarta
        let key_q = make_key(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = app.handle_key(key_q);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.quit_state, QuitState::Normal);
    }

    // ── Sidebar toggle tests ────────────────────────────────────────

    #[test]
    fn t_key_three_state_toggle() {
        let mut app = make_app(1);
        app.sidebar_state.visible = true;
        app.sidebar_focused = true; // enable for this test
        // Initially visible + focused
        assert!(app.sidebar_state.visible);
        assert!(app.sidebar_focused);

        let key_t = make_key(KeyCode::Char('t'), KeyModifiers::NONE);

        // Sidebar focused → hide + unfocus
        app.handle_key(key_t);
        assert!(!app.sidebar_state.visible);
        assert!(!app.sidebar_focused);

        // Hidden → show + focus
        app.handle_key(key_t);
        assert!(app.sidebar_state.visible);
        assert!(app.sidebar_focused);

        // Unfocus first, then 't' → focus (visible but not focused → focus)
        app.sidebar_focused = false;
        app.handle_key(key_t);
        assert!(app.sidebar_state.visible);
        assert!(app.sidebar_focused);
    }

    #[test]
    fn sidebar_toggle_cancels_quit_pending() {
        let mut app = make_app(1);
        app.quit_state = QuitState::Pending;

        let key_t = make_key(KeyCode::Char('t'), KeyModifiers::NONE);
        app.handle_key(key_t);

        // Quit state should be cancelled
        assert_eq!(app.quit_state, QuitState::Normal);
    }

    #[test]
    fn sidebar_toggle_cancels_restart_pending() {
        let mut app = make_app(3);
        app.restart_state = RestartState::Pending { worker_id: 1 };

        let key_t = make_key(KeyCode::Char('t'), KeyModifiers::NONE);
        app.handle_key(key_t);

        // Restart state should be cancelled
        assert_eq!(app.restart_state, RestartState::None);
    }

    // ── Sidebar focused navigation tests ───────────────────────────

    #[test]
    fn sidebar_focused_up_down_navigates() {
        let mut app = make_app(1);
        app.sidebar_state.visible = true;
        app.sidebar_focused = true;
        assert!(app.sidebar_focused);
        assert!(app.sidebar_state.visible);

        let up = make_key(KeyCode::Up, KeyModifiers::NONE);
        let down = make_key(KeyCode::Down, KeyModifiers::NONE);

        // Should be consumed (not passed to worker panel scroll)
        let result = app.handle_key(up);
        assert_eq!(result, EventResult::Consumed);

        let result = app.handle_key(down);
        assert_eq!(result, EventResult::Consumed);

        // j/k should also work
        let k = make_key(KeyCode::Char('k'), KeyModifiers::NONE);
        let j = make_key(KeyCode::Char('j'), KeyModifiers::NONE);

        let result = app.handle_key(k);
        assert_eq!(result, EventResult::Consumed);

        let result = app.handle_key(j);
        assert_eq!(result, EventResult::Consumed);
    }

    #[test]
    fn sidebar_focused_space_toggles_expand() {
        let mut app = make_app(1);
        app.sidebar_state.visible = true;
        app.sidebar_focused = true;
        assert!(app.sidebar_focused);

        let space = make_key(KeyCode::Char(' '), KeyModifiers::NONE);
        let result = app.handle_key(space);
        assert_eq!(result, EventResult::Consumed);

        let enter = make_key(KeyCode::Enter, KeyModifiers::NONE);
        let result = app.handle_key(enter);
        assert_eq!(result, EventResult::Consumed);
    }

    #[test]
    fn sidebar_focused_esc_unfocuses() {
        let mut app = make_app(1);
        app.sidebar_state.visible = true;
        app.sidebar_focused = true;
        assert!(app.sidebar_focused);

        let esc = make_key(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(esc);

        assert!(!app.sidebar_focused);
        // Sidebar still visible
        assert!(app.sidebar_state.visible);
    }

    #[test]
    fn sidebar_focused_passthrough_quit() {
        let mut app = make_app(1);
        app.sidebar_focused = true;
        assert!(app.sidebar_focused);

        let key_q = make_key(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = app.handle_key(key_q);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.quit_state, QuitState::Pending);
    }

    #[test]
    fn sidebar_unfocused_arrows_scroll_worker() {
        let mut app = make_app(1);
        activate_workers(&mut app, &[1]);
        app.focused_worker = Some(1);
        app.sidebar_focused = false; // explicitly unfocus sidebar

        let up = make_key(KeyCode::Up, KeyModifiers::NONE);
        app.handle_key(up);

        // Should scroll worker panel by scroll_step (default=3), not sidebar
        assert_eq!(app.panels[&1].scroll_offset, 3);
    }

    #[test]
    fn sidebar_focused_resize_keys() {
        let mut app = make_app(1);
        app.sidebar_state.visible = true;
        app.sidebar_focused = true;
        assert!(app.sidebar_focused);

        let initial_width = app.sidebar_state.width();

        let plus = make_key(KeyCode::Char('+'), KeyModifiers::NONE);
        let result = app.handle_key(plus);
        assert_eq!(result, EventResult::Consumed);
        assert!(app.sidebar_state.width() >= initial_width);

        let minus = make_key(KeyCode::Char('-'), KeyModifiers::NONE);
        let result = app.handle_key(minus);
        assert_eq!(result, EventResult::Consumed);
    }

    // ── Mouse hover → focus tests ───────────────────────────────────

    fn make_mouse_move(column: u16, row: u16) -> crossterm::event::MouseEvent {
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn make_mouse_click(column: u16, row: u16) -> crossterm::event::MouseEvent {
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_moved_over_worker_rect_sets_focus() {
        let mut app = make_app(3);
        // Symuluj cache grid_rects: worker 2 w obszarze (10,5)-(29,24)
        app.grid_rects = vec![
            (1, ratatui::layout::Rect::new(0, 0, 10, 5)),
            (2, ratatui::layout::Rect::new(10, 5, 20, 20)),
            (3, ratatui::layout::Rect::new(30, 0, 10, 5)),
        ];

        // Kursor w środku recta workera 2
        let mouse = make_mouse_move(15, 10);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focused_worker, Some(2));
    }

    #[test]
    fn mouse_moved_over_first_worker_sets_focus() {
        let mut app = make_app(3);
        app.grid_rects = vec![
            (1, ratatui::layout::Rect::new(0, 0, 10, 10)),
            (2, ratatui::layout::Rect::new(10, 0, 10, 10)),
        ];

        let mouse = make_mouse_move(5, 5);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focused_worker, Some(1));
    }

    #[test]
    fn mouse_moved_outside_all_rects_does_not_change_focus() {
        let mut app = make_app(3);
        app.focused_worker = Some(1);
        app.grid_rects = vec![
            (1, ratatui::layout::Rect::new(0, 0, 10, 5)),
            (2, ratatui::layout::Rect::new(10, 0, 10, 5)),
        ];

        // Kursor poza wszystkimi rectami
        let mouse = make_mouse_move(50, 50);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Ignored);
        // Focus nie zmieniony
        assert_eq!(app.focused_worker, Some(1));
    }

    #[test]
    fn mouse_moved_empty_grid_rects_returns_ignored() {
        let mut app = make_app(1);
        app.focused_worker = Some(1);
        // grid_rects puste (np. gdy aktywny preview)
        app.grid_rects = vec![];

        let mouse = make_mouse_move(10, 10);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Ignored);
        assert_eq!(app.focused_worker, Some(1));
    }

    #[test]
    fn mouse_left_click_on_worker_sets_focus() {
        let mut app = make_app(2);
        app.focused_worker = Some(1);
        app.grid_rects = vec![
            (1, ratatui::layout::Rect::new(0, 0, 20, 20)),
            (2, ratatui::layout::Rect::new(20, 0, 20, 20)),
        ];

        // Left click w obszarze workera 2 — powinien zmienić focus
        let mouse = make_mouse_click(25, 5);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focused_worker, Some(2));
    }

    #[test]
    fn mouse_left_click_cancels_quit_pending() {
        let mut app = make_app(2);
        app.quit_state = crate::commands::task::orchestrate::app::QuitState::Pending;
        app.grid_rects = vec![(1, ratatui::layout::Rect::new(0, 0, 40, 20))];

        let mouse = make_mouse_click(10, 5);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(
            app.quit_state,
            crate::commands::task::orchestrate::app::QuitState::Normal
        );
        assert_eq!(app.focused_worker, Some(1));
    }

    #[test]
    fn mouse_left_click_cancels_restart_pending() {
        let mut app = make_app(2);
        app.restart_state = RestartState::Pending { worker_id: 2 };
        app.grid_rects = vec![
            (1, ratatui::layout::Rect::new(0, 0, 20, 20)),
            (2, ratatui::layout::Rect::new(20, 0, 20, 20)),
        ];

        let mouse = make_mouse_click(5, 5);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.restart_state, RestartState::None);
        assert_eq!(app.focused_worker, Some(1));
    }

    #[test]
    fn mouse_left_click_resets_scroll_offset() {
        let mut app = make_app(2);
        // Worker 2 ma scroll offset > 0
        app.panels.get_mut(&2).unwrap().scroll_offset = 10;
        app.focused_worker = Some(1);
        app.grid_rects = vec![
            (1, ratatui::layout::Rect::new(0, 0, 20, 20)),
            (2, ratatui::layout::Rect::new(20, 0, 20, 20)),
        ];

        // Click na worker 2 — powinien zresetować scroll offset
        let mouse = make_mouse_click(25, 5);
        app.handle_mouse(mouse);

        assert_eq!(app.focused_worker, Some(2));
        assert_eq!(app.panels[&2].scroll_offset, 0);
    }

    #[test]
    fn mouse_left_click_outside_rects_returns_ignored() {
        let mut app = make_app(2);
        app.focused_worker = Some(1);
        app.grid_rects = vec![
            (1, ratatui::layout::Rect::new(0, 0, 20, 20)),
            (2, ratatui::layout::Rect::new(20, 0, 20, 20)),
        ];

        // Click poza wszystkimi panelami
        let mouse = make_mouse_click(100, 100);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Ignored);
        // Focus nie zmieniony
        assert_eq!(app.focused_worker, Some(1));
    }

    #[test]
    fn mouse_left_click_ignored_when_palette_open() {
        let mut app = make_app(2);
        app.grid_rects = vec![(1, ratatui::layout::Rect::new(0, 0, 40, 20))];
        app.open_command_palette();

        let mouse = make_mouse_click(10, 5);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Ignored);
        assert_eq!(app.focused_worker, None);
    }

    #[test]
    fn mouse_left_click_ignored_when_overlay_active() {
        let overlay = Arc::new(Mutex::new(Some(TextInputOverlay::new(1))));
        let mut app = OrchestrateApp::new(2, overlay);
        app.grid_rects = vec![(1, ratatui::layout::Rect::new(0, 0, 40, 20))];

        let mouse = make_mouse_click(10, 5);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Ignored);
        assert_eq!(app.focused_worker, None);
    }

    #[test]
    fn mouse_moved_on_rect_boundary_hits_correct_worker() {
        let mut app = make_app(2);
        // Worker 1: x=0..9, y=0..9
        // Worker 2: x=10..19, y=0..9
        app.grid_rects = vec![
            (1, ratatui::layout::Rect::new(0, 0, 10, 10)),
            (2, ratatui::layout::Rect::new(10, 0, 10, 10)),
        ];

        // Punkt dokładnie na lewej krawędzi workera 2
        let mouse = make_mouse_move(10, 5);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focused_worker, Some(2));
    }

    #[test]
    fn mouse_hover_ignored_when_palette_open() {
        let mut app = make_app(2);
        app.grid_rects = vec![
            (1, ratatui::layout::Rect::new(0, 0, 20, 20)),
            (2, ratatui::layout::Rect::new(20, 0, 20, 20)),
        ];
        app.open_command_palette();
        assert!(app.is_palette_open());

        // Hover nad workerem 1 — powinien zostać zignorowany gdy palette otwarta
        let mouse = make_mouse_move(5, 5);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Ignored);
        // Focus nie zmieniony
        assert_eq!(app.focused_worker, None);
    }

    #[test]
    fn mouse_hover_ignored_when_overlay_active() {
        let overlay = Arc::new(Mutex::new(Some(TextInputOverlay::new(1))));
        let mut app = OrchestrateApp::new(2, overlay);
        app.grid_rects = vec![
            (1, ratatui::layout::Rect::new(0, 0, 20, 20)),
            (2, ratatui::layout::Rect::new(20, 0, 20, 20)),
        ];
        assert!(app.is_overlay_active());

        // Hover nad workerem 2 — powinien zostać zignorowany gdy overlay aktywny
        let mouse = make_mouse_move(25, 5);
        let result = app.handle_mouse(mouse);

        assert_eq!(result, EventResult::Ignored);
        assert_eq!(app.focused_worker, None);
    }

    #[test]
    fn handle_event_mouse_moved_delegates_to_handle_mouse() {
        use crate::tui::app::AppState;
        use crate::tui::keybindings::KeybindingResolver;

        let mut app = make_app(2);
        app.grid_rects = vec![(1, ratatui::layout::Rect::new(0, 0, 40, 20))];

        let mouse = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column: 10,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        let resolver = KeybindingResolver::with_defaults();
        let result = app.handle_event(AppEvent::Mouse(mouse), &resolver);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.focused_worker, Some(1));
    }

    // ── Mouse scroll tests ──────────────────────────────────────────

    fn make_scroll(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Pomocnik: ustaw grid_rects symulując panel workera w danym obszarze.
    fn set_worker_rect(app: &mut OrchestrateApp, worker_id: u32, rect: ratatui::layout::Rect) {
        app.grid_rects.retain(|(id, _)| *id != worker_id);
        app.grid_rects.push((worker_id, rect));
    }

    #[test]
    fn mouse_scroll_up_over_panel_scrolls_that_panel() {
        let mut app = make_app(2);
        // Worker 1 zajmuje kolumny 0-49, wiersze 0-19
        set_worker_rect(&mut app, 1, ratatui::layout::Rect::new(0, 0, 50, 20));
        // Worker 2 zajmuje kolumny 50-99, wiersze 0-19
        set_worker_rect(&mut app, 2, ratatui::layout::Rect::new(50, 0, 50, 20));

        // Scroll up nad worker 1 (kursor: 10, 5)
        let scroll_up = make_scroll(MouseEventKind::ScrollUp, 10, 5);
        let result = app.handle_mouse(scroll_up);

        assert_eq!(result, EventResult::Consumed);
        // Worker 1 powinien być przewinięty w górę (scroll_step=3)
        assert_eq!(app.panels[&1].scroll_offset, 3);
        // Worker 2 bez zmian
        assert_eq!(app.panels[&2].scroll_offset, 0);
    }

    #[test]
    fn mouse_scroll_down_over_panel_scrolls_that_panel() {
        let mut app = make_app(2);
        set_worker_rect(&mut app, 1, ratatui::layout::Rect::new(0, 0, 50, 20));

        // Ustaw offset > 0 żeby było co zmniejszać
        app.panels.get_mut(&1).unwrap().scroll_offset = 10;

        // Scroll down nad worker 1 (kursor: 10, 5)
        let scroll_down = make_scroll(MouseEventKind::ScrollDown, 10, 5);
        let result = app.handle_mouse(scroll_down);

        assert_eq!(result, EventResult::Consumed);
        // offset powinien spaść o scroll_step (3)
        assert_eq!(app.panels[&1].scroll_offset, 7);
    }

    #[test]
    fn mouse_scroll_over_second_panel_only_scrolls_that_panel() {
        let mut app = make_app(2);
        set_worker_rect(&mut app, 1, ratatui::layout::Rect::new(0, 0, 50, 20));
        set_worker_rect(&mut app, 2, ratatui::layout::Rect::new(50, 0, 50, 20));

        // Scroll up nad worker 2 (kursor: 60, 5)
        let scroll_up = make_scroll(MouseEventKind::ScrollUp, 60, 5);
        let result = app.handle_mouse(scroll_up);

        assert_eq!(result, EventResult::Consumed);
        // Tylko worker 2 powinien być przewinięty
        assert_eq!(app.panels[&1].scroll_offset, 0);
        assert_eq!(app.panels[&2].scroll_offset, 3);
    }

    #[test]
    fn mouse_scroll_outside_panels_ignored() {
        let mut app = make_app(1);
        // Panel w obszarze 0-49, 0-19
        set_worker_rect(&mut app, 1, ratatui::layout::Rect::new(0, 0, 50, 20));

        // Kursor poza panelem (kursor: 60, 25)
        let scroll_up = make_scroll(MouseEventKind::ScrollUp, 60, 25);
        let result = app.handle_mouse(scroll_up);

        assert_eq!(result, EventResult::Ignored);
        assert_eq!(app.panels[&1].scroll_offset, 0);
    }

    #[test]
    fn mouse_scroll_with_no_panels_ignored() {
        let mut app = make_app(1);
        // grid_rects jest pusty (brak paneli)
        assert!(app.grid_rects.is_empty());

        let scroll_up = make_scroll(MouseEventKind::ScrollUp, 10, 5);
        let result = app.handle_mouse(scroll_up);

        assert_eq!(result, EventResult::Ignored);
    }

    #[test]
    fn mouse_scroll_over_panel_without_focus_works() {
        let mut app = make_app(2);
        set_worker_rect(&mut app, 1, ratatui::layout::Rect::new(0, 0, 50, 20));

        // Brak focusu — focused_worker = None
        app.focused_worker = None;

        // Scroll nad worker 1 powinien działać niezależnie od focusu
        let scroll_up = make_scroll(MouseEventKind::ScrollUp, 10, 5);
        let result = app.handle_mouse(scroll_up);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.panels[&1].scroll_offset, 3);
    }

    #[test]
    fn mouse_scroll_uses_scroll_step_from_config() {
        let mut app = make_app(1).with_scroll_step(5); // niedomyślny scroll_step
        set_worker_rect(&mut app, 1, ratatui::layout::Rect::new(0, 0, 50, 20));

        let scroll_up = make_scroll(MouseEventKind::ScrollUp, 10, 5);
        app.handle_mouse(scroll_up);

        // offset powinien wzrosnąć o scroll_step=5
        assert_eq!(app.panels[&1].scroll_offset, 5);
    }

    #[test]
    fn mouse_scroll_when_preview_active_scrolls_preview() {
        let mut app = make_app(1);
        set_worker_rect(&mut app, 1, ratatui::layout::Rect::new(0, 0, 50, 20));
        app.show_task_preview = true;
        app.preview_scroll_offset = 5;

        let scroll_up = make_scroll(MouseEventKind::ScrollUp, 10, 5);
        let result = app.handle_mouse(scroll_up);

        assert_eq!(result, EventResult::Consumed);
        // Preview powinno być przewinięte w górę (offset zmniejszony)
        assert_eq!(app.preview_scroll_offset, 2); // 5 - 3
        // Panel workera bez zmian
        assert_eq!(app.panels[&1].scroll_offset, 0);
    }

    #[test]
    fn mouse_scroll_preview_clamps_to_zero() {
        let mut app = make_app(1);
        app.show_task_preview = true;
        app.preview_scroll_offset = 1;

        let scroll_up = make_scroll(MouseEventKind::ScrollUp, 10, 5);
        app.handle_mouse(scroll_up);

        // Nie powinno zejść poniżej 0
        assert_eq!(app.preview_scroll_offset, 0);
    }

    #[test]
    fn mouse_scroll_down_from_tail_no_change() {
        let mut app = make_app(1);
        set_worker_rect(&mut app, 1, ratatui::layout::Rect::new(0, 0, 50, 20));
        app.panels.get_mut(&1).unwrap().scroll_offset = 0; // na ogonie

        let scroll_down = make_scroll(MouseEventKind::ScrollDown, 10, 5);
        app.handle_mouse(scroll_down);

        // offset=0 (tail) — scroll down nie zmienia
        assert_eq!(app.panels[&1].scroll_offset, 0);
    }

    #[test]
    fn mouse_scroll_up_from_tail_sets_offset() {
        let mut app = make_app(1);
        set_worker_rect(&mut app, 1, ratatui::layout::Rect::new(0, 0, 50, 20));
        app.panels.get_mut(&1).unwrap().scroll_offset = 0;

        let scroll_up = make_scroll(MouseEventKind::ScrollUp, 10, 5);
        app.handle_mouse(scroll_up);

        // Przejście z 0 → scroll_step
        assert_eq!(app.panels[&1].scroll_offset, 3);
    }

    #[test]
    fn mouse_right_click_ignored() {
        let mut app = make_app(1);
        set_worker_rect(&mut app, 1, ratatui::layout::Rect::new(0, 0, 50, 20));

        use crossterm::event::MouseButton;
        let click = make_scroll(MouseEventKind::Down(MouseButton::Right), 10, 5);
        let result = app.handle_mouse(click);

        assert_eq!(result, EventResult::Ignored);
        assert_eq!(app.panels[&1].scroll_offset, 0);
    }

    #[test]
    fn mouse_scroll_when_overlay_active_ignored() {
        use crate::tui::widgets::TextInputOverlay;
        let overlay = Arc::new(Mutex::new(Some(TextInputOverlay::new(1))));
        let mut app = OrchestrateApp::new(1, overlay);
        set_worker_rect(&mut app, 1, ratatui::layout::Rect::new(0, 0, 50, 20));

        let scroll_up = make_scroll(MouseEventKind::ScrollUp, 10, 5);
        let result = app.handle_mouse(scroll_up);

        // Overlay aktywny — scroll ignorowany
        assert_eq!(result, EventResult::Ignored);
        assert_eq!(app.panels[&1].scroll_offset, 0);
    }

    #[test]
    fn handle_event_mouse_scroll_routes_to_handle_mouse() {
        use crate::tui::app::AppState;
        use crate::tui::keybindings::KeybindingResolver;

        let mut app = make_app(1);
        set_worker_rect(&mut app, 1, ratatui::layout::Rect::new(0, 0, 50, 20));

        let mouse = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 10,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        let resolver = KeybindingResolver::with_defaults();
        let result = app.handle_event(AppEvent::Mouse(mouse), &resolver);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(app.panels[&1].scroll_offset, 3);
    }
}
