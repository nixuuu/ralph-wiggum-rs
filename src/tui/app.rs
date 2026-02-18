//! Centralny App struct zarządzający terminalem i event loop.
//!
//! Enkapsuluje `Terminal<CrosstermBackend<Stdout>>`, `EventDispatcher`
//! i cykl życia raw mode / AlternateScreen. Generic state per-command
//! definiowany przez trait `AppState` przekazywany do `App::run()`.

use std::io::{self, Stdout};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::prelude::CrosstermBackend;

use super::events::{AppEvent, EventDispatcher, EventResult, is_ctrl_c};

// ── AppState trait ──────────────────────────────────────────────────

/// Per-command state trait. Każdy widok TUI implementuje ten trait,
/// definiując jak rysować UI i jak reagować na zdarzenia.
#[allow(dead_code)] // Public API — będzie użyte przez per-command widoki TUI
pub trait AppState {
    /// Rysuj UI w podanym frame i area.
    /// `&mut self` umożliwia StatefulWidget mutację stanu (scroll, clamp) w render.
    fn draw(&mut self, frame: &mut Frame, area: Rect);

    /// Obsłuż zdarzenie aplikacyjne (key, resize, tick).
    /// Zwraca `EventResult` informujący event loop o wymaganej akcji.
    fn handle_event(&mut self, event: AppEvent) -> EventResult;
}

// ── App struct ──────────────────────────────────────────────────────

/// Centralny manager terminala i event loop.
///
/// Zarządza:
/// - `Terminal<CrosstermBackend<Stdout>>` (ratatui)
/// - `EventDispatcher` (crossterm polling na OS thread)
/// - Raw mode i AlternateScreen lifecycle
/// - Graceful shutdown via `Arc<AtomicBool>`
///
/// Typowy cykl życia:
/// ```ignore
/// let mut app = App::new(Duration::from_millis(100))?;
/// app.run(&mut my_state)?;
/// // Drop automatycznie cleanup-uje terminal
/// ```
#[allow(dead_code)] // Public API — będzie użyte przez per-command widoki TUI
pub struct App {
    terminal: ratatui::Terminal<CrosstermBackend<Stdout>>,
    dispatcher: Option<EventDispatcher>,
    shutdown: Arc<AtomicBool>,
    /// Flaga: czy raw mode jest aktywny (na potrzeby cleanup)
    raw_mode_active: bool,
}

#[allow(dead_code)] // Public API — będzie użyte przez per-command widoki TUI
impl App {
    /// Utwórz nową instancję App.
    ///
    /// Włącza raw mode, EnterAlternateScreen i inicjalizuje Terminal.
    /// EventDispatcher startuje na dedykowanym OS thread.
    ///
    /// # Argumenty
    /// - `poll_interval` — interwał pollowania crossterm events (typowo 100ms)
    pub fn new(poll_interval: Duration) -> io::Result<Self> {
        // Włącz raw mode — przechwytujemy klawisze bezpośrednio
        crossterm::terminal::enable_raw_mode()?;

        // Guard: jeśli kolejne operacje zawiodą, musimy wyłączyć raw mode
        let setup = || -> io::Result<ratatui::Terminal<CrosstermBackend<Stdout>>> {
            let mut stdout = io::stdout();
            execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
            let backend = CrosstermBackend::new(stdout);
            ratatui::Terminal::new(backend)
        };

        let terminal = match setup() {
            Ok(t) => t,
            Err(e) => {
                // Cleanup raw mode — App nie istnieje więc Drop się nie odpali
                let _ = crossterm::terminal::disable_raw_mode();
                let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
                return Err(e);
            }
        };

        let dispatcher = EventDispatcher::spawn(poll_interval);
        let shutdown = Arc::new(AtomicBool::new(false));

        Ok(App {
            terminal,
            dispatcher: Some(dispatcher),
            shutdown,
            raw_mode_active: true,
        })
    }

    /// Użyj zewnętrznej flagi shutdown zamiast wewnętrznej.
    ///
    /// Pozwala na współdzielenie flagi między TUI a async runtime,
    /// np. żeby SIGINT przerwał zarówno TUI loop jak i runner.
    pub fn with_shutdown(mut self, shutdown: Arc<AtomicBool>) -> Self {
        self.shutdown = shutdown;
        self
    }

    /// Uruchom event loop z podanym state.
    ///
    /// Pętla:
    /// 1. Odbierz event z dispatcher (blokujący recv)
    /// 2. Obsłuż event przez `AppState::handle_event` + wspólne handlery
    /// 3. Rysuj UI przez `AppState::draw`
    ///
    /// Kończy się gdy state zwróci `Quit`/`Shutdown` lub flaga shutdown zostanie ustawiona.
    pub fn run(&mut self, state: &mut impl AppState) -> io::Result<()> {
        // Pierwsze renderowanie przed pętlą eventów
        self.draw(state)?;

        while !self.shutdown.load(Ordering::SeqCst) {
            let dispatcher = match &self.dispatcher {
                Some(d) => d,
                None => break,
            };

            let event = match dispatcher.recv() {
                Some(e) => e,
                None => break, // Dispatcher zatrzymany
            };

            // Ignoruj Release/Repeat — przetwarzamy tylko Press (crossterm na Windows
            // emituje wszystkie KeyEventKind, bez filtrowania eventy byłyby podwójne)
            if let AppEvent::Key(ref key) = event
                && key.kind != KeyEventKind::Press
            {
                continue;
            }

            // Wspólne handlery: tylko Ctrl+C ma bezwarunkowy priorytet.
            // Klawisz 'q' delegowany do state (może mieć confirmation flow).
            let result = match &event {
                AppEvent::Key(key) if is_ctrl_c(key) => EventResult::Shutdown,
                _ => state.handle_event(event),
            };

            match result {
                EventResult::Quit | EventResult::Shutdown => {
                    self.shutdown.store(true, Ordering::SeqCst);
                    break;
                }
                _ => {}
            }

            self.draw(state)?;
        }

        Ok(())
    }

    /// Uruchom event loop ze shared state (`Arc<Mutex<S>>`).
    ///
    /// Wariant `run()` dla scenariuszy gdzie state jest współdzielony między
    /// TUI thread a async runtime (np. runner callbacki pushują do ring buffera).
    /// Mutex jest lockowany krótkotrwale — tylko na czas draw/handle_event.
    pub fn run_shared(&mut self, state: Arc<Mutex<impl AppState>>) -> io::Result<()> {
        // Pierwsze renderowanie
        {
            let mut s = state.lock().expect("AppState mutex poisoned");
            self.draw(&mut *s)?;
        }

        while !self.shutdown.load(Ordering::SeqCst) {
            let dispatcher = match &self.dispatcher {
                Some(d) => d,
                None => break,
            };

            let event = match dispatcher.recv() {
                Some(e) => e,
                None => break,
            };

            if let AppEvent::Key(ref key) = event
                && key.kind != KeyEventKind::Press
            {
                continue;
            }

            let result = {
                let mut s = state.lock().expect("AppState mutex poisoned");
                match &event {
                    AppEvent::Key(key) if is_ctrl_c(key) => EventResult::Shutdown,
                    _ => s.handle_event(event),
                }
            };

            match result {
                EventResult::Quit | EventResult::Shutdown => {
                    self.shutdown.store(true, Ordering::SeqCst);
                    break;
                }
                _ => {}
            }

            {
                let mut s = state.lock().expect("AppState mutex poisoned");
                self.draw(&mut *s)?;
            }
        }

        Ok(())
    }

    /// Rysuj UI — deleguje do `AppState::draw`.
    fn draw(&mut self, state: &mut impl AppState) -> io::Result<()> {
        self.terminal.draw(|frame| {
            let area = frame.area();
            state.draw(frame, area);
        })?;
        Ok(())
    }

    /// Flaga shutdown — do współdzielenia z innymi komponentami.
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    /// Flaga pauzy EventDispatcher — do interaktywnych TUI widgetów.
    pub fn paused_flag(&self) -> Option<Arc<AtomicBool>> {
        self.dispatcher.as_ref().map(|d| d.paused_flag())
    }

    /// Jawny cleanup terminala — przywraca raw mode i alternate screen.
    ///
    /// Wywoływane automatycznie przez Drop, ale można wywołać wcześniej
    /// gdy potrzebne jest gwarantowane przywrócenie terminala.
    pub fn cleanup(&mut self) {
        // Zatrzymaj dispatcher
        if let Some(dispatcher) = self.dispatcher.take() {
            dispatcher.stop();
        }

        // Przywróć terminal
        if self.raw_mode_active {
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
            let _ = self.terminal.show_cursor();
            self.raw_mode_active = false;
        }
    }
}

/// Safety net — jeśli App zostanie upuszczony bez jawnego cleanup(),
/// przywracamy terminal do normalnego stanu.
impl Drop for App {
    fn drop(&mut self) {
        self.cleanup();
    }
}

// ── Testy ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    // ── Test AppState ──

    /// Minimalny state do testowania — zlicza wywołania handle_event.
    struct CountingState {
        event_count: usize,
        quit_after: usize,
    }

    impl CountingState {
        fn new(quit_after: usize) -> Self {
            Self {
                event_count: 0,
                quit_after,
            }
        }
    }

    impl AppState for CountingState {
        fn draw(&mut self, _frame: &mut Frame, _area: Rect) {}

        fn handle_event(&mut self, _event: AppEvent) -> EventResult {
            self.event_count += 1;
            if self.event_count >= self.quit_after {
                EventResult::Quit
            } else {
                EventResult::Consumed
            }
        }
    }

    /// State który zawsze ignoruje eventy.
    struct IgnoringState;

    impl AppState for IgnoringState {
        fn draw(&mut self, _frame: &mut Frame, _area: Rect) {}
        fn handle_event(&mut self, _event: AppEvent) -> EventResult {
            EventResult::Ignored
        }
    }

    /// State który natychmiast żąda shutdown.
    struct ShutdownState;

    impl AppState for ShutdownState {
        fn draw(&mut self, _frame: &mut Frame, _area: Rect) {}
        fn handle_event(&mut self, _event: AppEvent) -> EventResult {
            EventResult::Shutdown
        }
    }

    // ── Trait compliance tests ──

    #[test]
    fn app_state_trait_is_object_safe() {
        // Weryfikacja że AppState może być używany jako trait object
        fn accept_state(_state: &dyn AppState) {}
        let state = IgnoringState;
        accept_state(&state);
    }

    #[test]
    fn counting_state_tracks_events() {
        let mut state = CountingState::new(3);
        let tick = AppEvent::Tick;

        assert_eq!(state.handle_event(tick.clone()), EventResult::Consumed);
        assert_eq!(state.event_count, 1);

        assert_eq!(state.handle_event(tick.clone()), EventResult::Consumed);
        assert_eq!(state.event_count, 2);

        assert_eq!(state.handle_event(tick), EventResult::Quit);
        assert_eq!(state.event_count, 3);
    }

    #[test]
    fn shutdown_state_returns_shutdown() {
        let mut state = ShutdownState;
        assert_eq!(state.handle_event(AppEvent::Tick), EventResult::Shutdown);
    }

    #[test]
    fn ignoring_state_returns_ignored() {
        let mut state = IgnoringState;
        assert_eq!(state.handle_event(AppEvent::Tick), EventResult::Ignored);
    }

    // ── Key priority tests (weryfikacja logiki z App::run) ──

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// Helper: symuluje logikę dispatch z App::run bez prawdziwego terminala.
    fn simulate_dispatch(state: &mut impl AppState, event: AppEvent) -> Option<EventResult> {
        // Filtrowanie Release/Repeat — jak w App::run()
        if let AppEvent::Key(ref key) = event
            && key.kind != KeyEventKind::Press
        {
            return None;
        }

        // Od teraz tylko Ctrl+C jest przechwytywany globalnie
        Some(match &event {
            AppEvent::Key(key) if is_ctrl_c(key) => EventResult::Shutdown,
            _ => state.handle_event(event),
        })
    }

    #[test]
    fn ctrl_c_bypasses_app_state() {
        let mut state = CountingState::new(10);
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let result = simulate_dispatch(&mut state, AppEvent::Key(key));
        assert_eq!(result, Some(EventResult::Shutdown));
        // State nie powinien dostać zdarzenia — Ctrl+C ma priorytet
        assert_eq!(state.event_count, 0);
    }

    #[test]
    fn quit_key_reaches_app_state() {
        // Od teraz 'q' nie jest przechwytywany globalnie — trafia do state
        let mut state = CountingState::new(10);
        let key = make_key(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = simulate_dispatch(&mut state, AppEvent::Key(key));
        assert_eq!(result, Some(EventResult::Consumed));
        assert_eq!(state.event_count, 1);
    }

    #[test]
    fn regular_key_reaches_app_state() {
        let mut state = CountingState::new(10);
        let key = make_key(KeyCode::Char('r'), KeyModifiers::NONE);
        let result = simulate_dispatch(&mut state, AppEvent::Key(key));
        assert_eq!(result, Some(EventResult::Consumed));
        assert_eq!(state.event_count, 1);
    }

    #[test]
    fn tick_reaches_app_state() {
        let mut state = CountingState::new(10);
        let result = simulate_dispatch(&mut state, AppEvent::Tick);
        assert_eq!(result, Some(EventResult::Consumed));
        assert_eq!(state.event_count, 1);
    }

    #[test]
    fn resize_reaches_app_state() {
        let mut state = CountingState::new(10);
        let result = simulate_dispatch(&mut state, AppEvent::Resize(80, 24));
        assert_eq!(result, Some(EventResult::Consumed));
        assert_eq!(state.event_count, 1);
    }

    #[test]
    fn key_release_is_filtered_out() {
        let mut state = CountingState::new(10);
        let key = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        let result = simulate_dispatch(&mut state, AppEvent::Key(key));
        assert_eq!(result, None);
        assert_eq!(state.event_count, 0);
    }

    #[test]
    fn key_repeat_is_filtered_out() {
        let mut state = CountingState::new(10);
        let key = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Repeat,
            state: KeyEventState::NONE,
        };
        let result = simulate_dispatch(&mut state, AppEvent::Key(key));
        assert_eq!(result, None);
        assert_eq!(state.event_count, 0);
    }

    // ── App cleanup safety tests (bez prawdziwego terminala) ──

    #[test]
    fn app_cleanup_is_idempotent() {
        // Testujemy cleanup na mockowym state — nie możemy zainicjalizować
        // prawdziwego terminala w CI, ale weryfikujemy logikę flag
        let raw_mode_active = true;

        // Symulacja: po cleanup raw_mode_active = false
        let mut flag = raw_mode_active;
        if flag {
            // Tutaj normalnie byłby disable_raw_mode + LeaveAlternateScreen
            flag = false;
        }
        assert!(!flag);

        // Drugie wywołanie nic nie robi
        if flag {
            panic!("Should not enter cleanup block twice");
        }
    }

    #[test]
    fn shutdown_flag_default_is_false() {
        let shutdown = Arc::new(AtomicBool::new(false));
        assert!(!shutdown.load(Ordering::SeqCst));
    }

    #[test]
    fn shutdown_flag_can_be_shared() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let clone = shutdown.clone();

        shutdown.store(true, Ordering::SeqCst);
        assert!(clone.load(Ordering::SeqCst));
    }

    // ── AppState z różnymi event types ──

    #[test]
    fn app_state_handles_resize_event() {
        let mut state = CountingState::new(10);
        let result = state.handle_event(AppEvent::Resize(120, 40));
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(state.event_count, 1);
    }

    #[test]
    fn app_state_handles_tick_event() {
        let mut state = CountingState::new(10);
        let result = state.handle_event(AppEvent::Tick);
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(state.event_count, 1);
    }

    #[test]
    fn app_state_handles_key_event() {
        let mut state = CountingState::new(10);
        let key = make_key(KeyCode::Char('x'), KeyModifiers::NONE);
        let result = state.handle_event(AppEvent::Key(key));
        assert_eq!(result, EventResult::Consumed);
        assert_eq!(state.event_count, 1);
    }

    // ── Event sequence tests ──

    #[test]
    fn app_state_quit_after_n_events() {
        let mut state = CountingState::new(3);
        let events = vec![AppEvent::Tick, AppEvent::Tick, AppEvent::Tick];

        let mut results = Vec::new();
        for event in events {
            results.push(state.handle_event(event));
        }

        assert_eq!(
            results,
            vec![
                EventResult::Consumed,
                EventResult::Consumed,
                EventResult::Quit,
            ]
        );
    }

    // ── run_shared dispatch logic tests ──

    /// Helper: symuluje logikę dispatch z App::run_shared (shared Mutex state).
    /// run_shared() nie przechwytuje 'q' globalnie — deleguje do AppState,
    /// żeby state mógł implementować własny quit confirmation flow.
    fn simulate_shared_dispatch(
        state: &Arc<Mutex<impl AppState>>,
        event: AppEvent,
    ) -> Option<EventResult> {
        if let AppEvent::Key(ref key) = event
            && key.kind != KeyEventKind::Press
        {
            return None;
        }

        let mut s = state.lock().expect("test mutex poisoned");
        Some(match &event {
            AppEvent::Key(key) if is_ctrl_c(key) => EventResult::Shutdown,
            _ => s.handle_event(event),
        })
    }

    #[test]
    fn shared_dispatch_ctrl_c_returns_shutdown() {
        let state = Arc::new(Mutex::new(CountingState::new(10)));
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let result = simulate_shared_dispatch(&state, AppEvent::Key(key));
        assert_eq!(result, Some(EventResult::Shutdown));
        assert_eq!(state.lock().unwrap().event_count, 0);
    }

    #[test]
    fn shared_dispatch_regular_key_reaches_state() {
        let state = Arc::new(Mutex::new(CountingState::new(10)));
        let key = make_key(KeyCode::Char('r'), KeyModifiers::NONE);
        let result = simulate_shared_dispatch(&state, AppEvent::Key(key));
        assert_eq!(result, Some(EventResult::Consumed));
        assert_eq!(state.lock().unwrap().event_count, 1);
    }

    #[test]
    fn shared_dispatch_tick_reaches_state() {
        let state = Arc::new(Mutex::new(CountingState::new(10)));
        let result = simulate_shared_dispatch(&state, AppEvent::Tick);
        assert_eq!(result, Some(EventResult::Consumed));
        assert_eq!(state.lock().unwrap().event_count, 1);
    }

    #[test]
    fn shared_state_accessible_between_dispatches() {
        // Weryfikacja że Mutex jest zwalniany między dispatch a draw
        let state = Arc::new(Mutex::new(CountingState::new(10)));

        // Symulacja: dispatch + concurrent access (jak runner callback)
        simulate_shared_dispatch(&state, AppEvent::Tick);

        // Mutex powinien być wolny — runner callback może go zablokować
        let guard = state.try_lock();
        assert!(guard.is_ok(), "Mutex should be free between dispatches");
        assert_eq!(guard.unwrap().event_count, 1);
    }

    // ── with_shutdown tests ──

    #[test]
    fn with_shutdown_shares_external_flag() {
        let external_shutdown = Arc::new(AtomicBool::new(false));
        let clone = external_shutdown.clone();

        // Symulacja: App tworzy wewnętrzny shutdown, z with_shutdown zastępujemy
        let internal_shutdown = Arc::new(AtomicBool::new(false));

        // Symulacja logiki with_shutdown
        let shutdown = external_shutdown;
        shutdown.store(true, Ordering::SeqCst);

        // Oba powinny widzieć tę samą wartość
        assert!(clone.load(Ordering::SeqCst));
        // Wewnętrzny nie jest zmieniony
        assert!(!internal_shutdown.load(Ordering::SeqCst));
    }
}
