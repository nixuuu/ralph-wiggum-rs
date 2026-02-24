//! Test helpers dla TUI — symulacja event loop i asercje stanu.
//!
//! Dostarcza `TestApp<S: AppState>` do testowania interakcji użytkownika
//! bez potrzeby prawdziwego terminala. Używa `TestBackend` z ratatui
//! i mock event channel dla symulacji sekwencji klawiszy.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use super::app::AppState;
use super::events::{AppEvent, EventResult};
use super::keybindings::KeybindingResolver;

// ── Shared test helpers ────────────────────────────────────────────────

/// Tworzy KeyEvent z KeyCode (Press, brak modyfikatorów).
/// Wspólny helper dla testów integracyjnych — unika duplikacji make_key w każdym pliku.
pub fn make_key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

/// Tworzy MouseEvent reprezentujący lewy klik myszy na podanej pozycji.
///
/// Wspólny helper dla testów myszy — unika duplikacji w każdym pliku testowym.
pub fn make_left_click(col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// Tworzy KeyEvent z KeyCode i modyfikatorem SHIFT.
/// Używane dla klawiszy wymagających Shift (np. 'R' = Shift+r, BackTab = Shift+Tab).
pub fn make_key_shift(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::SHIFT,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

// ── TestStep ─────────────────────────────────────────────────────────

/// Krok w multi-step test sequence.
///
/// Pozwala na definiowanie złożonych scenariuszy testowych jako sekwencji
/// kroków, które mogą zawierać eventy, asercje i opóźnienia.
pub enum TestStep<S: AppState> {
    /// Naciśnięcie klawisza — event zostanie wstrzyknięty i przetworzony
    KeyPress(KeyEvent),

    /// Asercja renderowanego bufora — funkcja otrzymuje Buffer i musi zwrócić true
    AssertRender(Box<dyn Fn(&Buffer) -> bool>),

    /// Asercja stanu aplikacji — funkcja otrzymuje &S i musi zwrócić true
    AssertState(Box<dyn Fn(&S) -> bool>),

    /// Opóźnienie przed następnym krokiem
    Wait(Duration),
}

// ── TestApp ──────────────────────────────────────────────────────────

/// Testowy wrapper dla App z TestBackend i mock event channel.
///
/// Pozwala na:
/// - Symulację sekwencji klawiszy przez `inject_key` / `inject_keys`
/// - Renderowanie UI i weryfikację bufora przez `render`
/// - Asercje stanu przez `assert_state`
///
/// # Przykład
/// ```ignore
/// use crate::tui::test_helpers::TestApp;
/// use crate::tui::AppState;
/// use crossterm::event::{KeyCode, KeyEvent};
///
/// let mut app = TestApp::new(MyState::default(), 80, 24);
/// app.inject_key(KeyEvent::from(KeyCode::Char('j')));
/// app.step();
/// app.assert_state(|s| s.cursor_position == 1);
/// ```
pub struct TestApp<S: AppState> {
    /// Terminal z TestBackend — używany do renderowania
    terminal: Terminal<TestBackend>,
    /// State aplikacji (generic AppState)
    state: S,
    /// Mock event channel — wysyłamy do niego symulowane eventy
    event_tx: mpsc::Sender<AppEvent>,
    /// Odbiornik eventów — przekazywany do event loop
    event_rx: mpsc::Receiver<AppEvent>,
    /// Resolver keybindingów — przekazywany do handle_event
    resolver: KeybindingResolver,
}

impl<S: AppState> TestApp<S> {
    /// Utwórz nową instancję TestApp z podanym state i wymiarami terminala.
    ///
    /// # Argumenty
    /// - `state` — początkowy state aplikacji (implementujący `AppState`)
    /// - `width` — szerokość terminala testowego (w kolumnach)
    /// - `height` — wysokość terminala testowego (w wierszach)
    pub fn new(state: S, width: u16, height: u16) -> Self {
        assert!(width > 0 && height > 0, "Terminal dimensions must be > 0");
        let backend = TestBackend::new(width, height);
        let terminal = Terminal::new(backend).expect("Failed to create test terminal");
        let (event_tx, event_rx) = mpsc::channel();

        TestApp {
            terminal,
            state,
            event_tx,
            event_rx,
            resolver: KeybindingResolver::with_defaults(),
        }
    }

    /// Ustaw resolver keybindingów (opcjonalnie — domyślnie używane są defaults).
    #[allow(dead_code)] // Public API — dostępne dla testów integracyjnych z custom keybindingami
    pub fn with_resolver(mut self, resolver: KeybindingResolver) -> Self {
        self.resolver = resolver;
        self
    }

    /// Wstrzyknij pojedyncze zdarzenie klawiatury do event queue.
    ///
    /// Event zostanie przetworzony podczas następnego wywołania `step()`.
    ///
    /// # Przykład
    /// ```ignore
    /// app.inject_key(KeyEvent::from(KeyCode::Char('q')));
    /// ```
    pub fn inject_key(&mut self, key: KeyEvent) {
        self.event_tx
            .send(AppEvent::Key(key))
            .expect("Failed to inject key event");
    }

    /// Wstrzyknij sekwencję zdarzeń klawiatury do event queue.
    ///
    /// Eventy zostaną przetworzone w kolejności podczas wywołań `step()`.
    ///
    /// # Przykład
    /// ```ignore
    /// app.inject_keys(vec![
    ///     KeyEvent::from(KeyCode::Char('j')),
    ///     KeyEvent::from(KeyCode::Char('k')),
    /// ]);
    /// ```
    pub fn inject_keys(&mut self, keys: Vec<KeyEvent>) {
        for key in keys {
            self.inject_key(key);
        }
    }

    /// Wstrzyknij zdarzenie myszy do event queue.
    ///
    /// Event zostanie przetworzony podczas następnego wywołania `step()`.
    /// Używaj razem z `make_left_click()` do symulowania kliknięć.
    ///
    /// # Przykład
    /// ```ignore
    /// app.inject_mouse(make_left_click(5, 3));
    /// app.step();
    /// ```
    pub fn inject_mouse(&mut self, mouse: MouseEvent) {
        self.event_tx
            .send(AppEvent::Mouse(mouse))
            .expect("Failed to inject mouse event");
    }

    /// Wstrzyknij event Tick do event queue.
    ///
    /// Przydatne do testowania obsługi periodycznych zdarzeń.
    pub fn inject_tick(&mut self) {
        self.event_tx
            .send(AppEvent::Tick)
            .expect("Failed to inject tick event");
    }

    /// Wstrzyknij event Resize do event queue.
    ///
    /// Przydatne do testowania responsywności layoutu.
    ///
    /// # Argumenty
    /// - `width` — nowa szerokość terminala
    /// - `height` — nowa wysokość terminala
    pub fn inject_resize(&mut self, width: u16, height: u16) {
        self.event_tx
            .send(AppEvent::Resize(width, height))
            .expect("Failed to inject resize event");
    }

    /// Zmień rozmiar terminala testowego (zmienia fizyczny rozmiar backendu).
    ///
    /// W przeciwieństwie do `inject_resize()`, ta metoda faktycznie zmienia rozmiar
    /// TestBackend, co wpływa na `frame.area()` podczas renderowania.
    /// Wstrzykuje również event Resize do kolejki.
    ///
    /// # Argumenty
    /// - `width` — nowa szerokość terminala
    /// - `height` — nowa wysokość terminala
    ///
    /// # Przykład
    /// ```ignore
    /// app.resize_terminal(100, 30); // Zmienia rozmiar backendu
    /// app.step(); // Przetwarza event Resize
    /// app.render(); // Renderuje z nowymi wymiarami
    /// ```
    pub fn resize_terminal(&mut self, width: u16, height: u16) {
        // Tworzymy nowy backend z nowymi wymiarami
        let new_backend = TestBackend::new(width, height);
        self.terminal = Terminal::new(new_backend).expect("Failed to create test terminal");

        // Wstrzykujemy event Resize do kolejki
        self.inject_resize(width, height);
    }

    /// Wykonaj jeden krok event loop: odbierz event i przetwórz przez state.
    ///
    /// Zwraca `Some(EventResult)` jeśli event został przetworzony,
    /// `None` jeśli brak eventów w kolejce.
    ///
    /// # Przykład
    /// ```ignore
    /// app.inject_key(KeyEvent::from(KeyCode::Char('j')));
    /// assert!(app.step().is_some()); // Event został przetworzony
    /// assert!(app.step().is_none()); // Brak eventów w kolejce
    /// ```
    pub fn step(&mut self) -> Option<EventResult> {
        self.step_with_timeout(Duration::from_millis(10))
    }

    /// Wykonaj jeden krok event loop z timeoutem.
    ///
    /// Zwraca `Some(EventResult)` jeśli event został przetworzony, `None` jeśli timeout.
    pub fn step_with_timeout(&mut self, timeout: Duration) -> Option<EventResult> {
        match self.event_rx.recv_timeout(timeout) {
            Ok(event) => Some(self.state.handle_event(event, &self.resolver)),
            Err(_) => None,
        }
    }

    /// Przetwórz wszystkie eventy w kolejce (do wyczerpania).
    ///
    /// Przydatne gdy wstrzykujesz sekwencję eventów i chcesz przetworzyć wszystkie.
    ///
    /// # Przykład
    /// ```ignore
    /// app.inject_keys(vec![
    ///     KeyEvent::from(KeyCode::Char('j')),
    ///     KeyEvent::from(KeyCode::Char('j')),
    ///     KeyEvent::from(KeyCode::Char('j')),
    /// ]);
    /// app.drain_events();
    /// app.assert_state(|s| s.cursor_position == 3);
    /// ```
    pub fn drain_events(&mut self) {
        while self.step_with_timeout(Duration::from_millis(1)).is_some() {}
    }

    /// Wyrenderuj UI do bufora i zwróć snapshot.
    ///
    /// Pozwala na weryfikację zawartości ekranu po przetworzeniu eventów.
    /// TestBackend nigdy nie zwraca błędów I/O — metoda zawsze się powiedzie.
    ///
    /// # Przykład
    /// ```ignore
    /// let buffer = app.render();
    /// let cell = buffer.cell((0, 0)).unwrap();
    /// assert_eq!(cell.symbol(), ">");
    /// ```
    pub fn render(&mut self) -> Buffer {
        self.terminal
            .draw(|frame| {
                let area = frame.area();
                self.state.draw(frame, area);
            })
            .expect("TestBackend::draw is infallible");
        self.terminal.backend().buffer().clone()
    }

    /// Wykonaj asercję na state aplikacji.
    ///
    /// Predykat przyjmuje referencję do state i zwraca `bool`.
    /// Panikuje jeśli predykat zwróci `false`.
    ///
    /// # Przykład
    /// ```ignore
    /// app.assert_state(|s| s.is_running);
    /// app.assert_state(|s| s.items.len() == 3);
    /// ```
    #[track_caller]
    pub fn assert_state(&self, predicate: impl FnOnce(&S) -> bool) {
        assert!(
            predicate(&self.state),
            "State assertion failed at {}",
            std::panic::Location::caller()
        );
    }

    /// Zwróć referencję do state (immutable).
    ///
    /// Przydatne gdy potrzebujesz bardziej szczegółowych asercji.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// Zwróć mutable referencję do state.
    ///
    /// Przydatne gdy chcesz bezpośrednio zmodyfikować state przed testem.
    pub fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }

    /// Zwróć wymiary terminala testowego.
    pub fn size(&mut self) -> (u16, u16) {
        let area = self.terminal.get_frame().area();
        (area.width, area.height)
    }

    /// Wykonaj sekwencję kroków testowych z timeoutem.
    ///
    /// Przetwarza każdy krok po kolei:
    /// - KeyPress: wstrzykuje klawisz i wykonuje step()
    /// - AssertRender: renderuje i sprawdza predykat na buforze
    /// - AssertState: sprawdza predykat na state
    /// - Wait: czeka przez podany czas
    ///
    /// Jeśli wykonanie przekroczy `max_duration`, funkcja panikuje.
    /// Domyślny timeout: 5 sekund.
    ///
    /// # Przykład
    /// ```ignore
    /// use crate::tui::test_helpers::{TestApp, TestStep};
    /// use crossterm::event::{KeyCode, KeyEvent};
    /// use std::time::Duration;
    ///
    /// let mut app = TestApp::new(MyState::default(), 80, 24);
    /// app.run_steps(vec![
    ///     TestStep::KeyPress(KeyEvent::from(KeyCode::Char('j'))),
    ///     TestStep::AssertState(Box::new(|s| s.cursor == 1)),
    ///     TestStep::Wait(Duration::from_millis(100)),
    ///     TestStep::AssertRender(Box::new(|buf| {
    ///         buf.cell((0, 1)).unwrap().symbol() == ">"
    ///     })),
    /// ]);
    /// ```
    ///
    /// # Panics
    /// - Jeśli wykonanie przekroczy `max_duration` (domyślnie 5s)
    /// - Jeśli któraś z asercji zwróci `false`
    #[track_caller]
    pub fn run_steps(&mut self, steps: Vec<TestStep<S>>) {
        self.run_steps_with_timeout(steps, Duration::from_secs(5))
    }

    /// Wykonaj sekwencję kroków testowych z własnym timeoutem.
    ///
    /// Podobnie jak `run_steps()`, ale pozwala na ustawienie własnego timeoutu.
    ///
    /// # Panics
    /// - Jeśli wykonanie przekroczy `max_duration`
    /// - Jeśli któraś z asercji zwróci `false`
    #[track_caller]
    pub fn run_steps_with_timeout(&mut self, steps: Vec<TestStep<S>>, max_duration: Duration) {
        let start = Instant::now();

        for (step_idx, step) in steps.into_iter().enumerate() {
            // Sprawdź timeout przed każdym krokiem
            if start.elapsed() > max_duration {
                panic!(
                    "Test step sequence timed out at step {} after {:?} (max: {:?})",
                    step_idx,
                    start.elapsed(),
                    max_duration
                );
            }

            match step {
                TestStep::KeyPress(key) => {
                    self.inject_key(key);
                    if self.step_with_timeout(Duration::from_millis(100)).is_none() {
                        panic!(
                            "Step {}: KeyPress event was not processed within 100ms",
                            step_idx
                        );
                    }
                }
                TestStep::AssertRender(predicate) => {
                    let buffer = self.render();
                    assert!(
                        predicate(&buffer),
                        "Step {}: Render assertion failed at {}",
                        step_idx,
                        std::panic::Location::caller()
                    );
                }
                TestStep::AssertState(predicate) => {
                    assert!(
                        predicate(&self.state),
                        "Step {}: State assertion failed at {}",
                        step_idx,
                        std::panic::Location::caller()
                    );
                }
                TestStep::Wait(duration) => {
                    // Sprawdź czy Wait nie przekroczy timeoutu
                    let remaining_time = max_duration.saturating_sub(start.elapsed());
                    if duration > remaining_time {
                        panic!(
                            "Test step sequence timed out at step {} (Wait({:?}) exceeds remaining time: {:?})",
                            step_idx, duration, remaining_time
                        );
                    }
                    std::thread::sleep(duration);
                }
            }
        }
    }
}

// ── Wolne funkcje — asercje tekstowe na buforze ─────────────────────

/// Wyciąga tekst z wiersza bufora ratatui.
fn buffer_line_text(buffer: &Buffer, y: u16) -> String {
    let mut line = String::new();
    for x in 0..buffer.area().width {
        if let Some(cell) = buffer.cell((x, y)) {
            line.push_str(cell.symbol());
        }
    }
    line
}

/// Sprawdza czy bufor zawiera podany tekst (skanuje wiersz po wierszu).
/// Tekst musi w całości mieścić się w jednym wierszu — nie wykrywa tekstu
/// rozciągającego się na kilka linii.
pub fn buffer_contains_text(buffer: &Buffer, text: &str) -> bool {
    (0..buffer.area().height).any(|y| buffer_line_text(buffer, y).contains(text))
}

/// Asercja: bufor MUSI zawierać podany tekst.
///
/// Panikuje z komunikatem jeśli tekst nie został znaleziony.
///
/// # Przykład
/// ```ignore
/// let buffer = app.render();
/// assert_contains_text(&buffer, "Hello");
/// ```
#[track_caller]
pub fn assert_contains_text(buffer: &Buffer, text: &str) {
    assert!(
        buffer_contains_text(buffer, text),
        "Expected buffer to contain {:?}, but it was not found.\nBuffer contents:\n{}",
        text,
        (0..buffer.area().height)
            .map(|y| buffer_line_text(buffer, y).trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Asercja: bufor NIE MOŻE zawierać podanego tekstu.
///
/// Panikuje z komunikatem jeśli tekst został znaleziony.
///
/// # Przykład
/// ```ignore
/// let buffer = app.render();
/// assert_not_contains_text(&buffer, "Error");
/// ```
#[track_caller]
pub fn assert_not_contains_text(buffer: &Buffer, text: &str) {
    assert!(
        !buffer_contains_text(buffer, text),
        "Expected buffer NOT to contain {:?}, but it was found.\nBuffer contents:\n{}",
        text,
        (0..buffer.area().height)
            .map(|y| buffer_line_text(buffer, y).trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

// ── Testy ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use ratatui::Frame;
    use ratatui::layout::Rect;

    // ── Test AppState implementations ──

    /// Prosty state do testowania — zlicza naciśnięcia klawiszy.
    #[derive(Default)]
    struct CountingState {
        key_count: usize,
        last_key: Option<KeyCode>,
    }

    impl AppState for CountingState {
        fn draw(&mut self, _frame: &mut Frame, _area: Rect) {}

        fn handle_event(
            &mut self,
            event: AppEvent,
            _resolver: &crate::tui::KeybindingResolver,
        ) -> EventResult {
            match event {
                AppEvent::Key(key) => {
                    self.key_count += 1;
                    self.last_key = Some(key.code);
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
            }
        }
    }

    /// State który renderuje zawartość do bufora.
    struct RenderingState {
        text: String,
    }

    impl RenderingState {
        fn new(text: String) -> Self {
            Self { text }
        }
    }

    impl AppState for RenderingState {
        fn draw(&mut self, frame: &mut Frame, area: Rect) {
            use ratatui::text::Text;
            use ratatui::widgets::Widget;

            Text::raw(&self.text).render(area, frame.buffer_mut());
        }

        fn handle_event(
            &mut self,
            event: AppEvent,
            _resolver: &crate::tui::KeybindingResolver,
        ) -> EventResult {
            match event {
                AppEvent::Key(key) if key.code == KeyCode::Char('a') => {
                    self.text.push('A');
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
            }
        }
    }

    // ── Helpers ──

    use super::make_key;

    // ── TestApp lifecycle tests ──

    #[test]
    fn test_app_new_creates_terminal() {
        let state = CountingState::default();
        let mut app = TestApp::new(state, 80, 24);
        assert_eq!(app.size(), (80, 24));
    }

    #[test]
    fn test_app_new_with_custom_dimensions() {
        let state = CountingState::default();
        let mut app = TestApp::new(state, 120, 40);
        assert_eq!(app.size(), (120, 40));
    }

    // ── inject_key tests ──

    #[test]
    fn test_inject_key_single() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);
        app.inject_key(make_key(KeyCode::Char('j')));

        assert!(app.step().is_some());
        assert_eq!(app.state().key_count, 1);
        assert_eq!(app.state().last_key, Some(KeyCode::Char('j')));
    }

    #[test]
    fn test_inject_key_multiple() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);
        app.inject_key(make_key(KeyCode::Char('j')));
        app.inject_key(make_key(KeyCode::Char('k')));
        app.inject_key(make_key(KeyCode::Char('l')));

        app.step();
        app.step();
        app.step();

        assert_eq!(app.state().key_count, 3);
        assert_eq!(app.state().last_key, Some(KeyCode::Char('l')));
    }

    // ── inject_keys tests ──

    #[test]
    fn test_inject_keys_sequence() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);
        app.inject_keys(vec![
            make_key(KeyCode::Char('a')),
            make_key(KeyCode::Char('b')),
            make_key(KeyCode::Char('c')),
        ]);

        app.drain_events();

        assert_eq!(app.state().key_count, 3);
        assert_eq!(app.state().last_key, Some(KeyCode::Char('c')));
    }

    #[test]
    fn test_inject_keys_empty() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);
        app.inject_keys(vec![]);

        assert!(app.step().is_none());
        assert_eq!(app.state().key_count, 0);
    }

    // ── inject_tick tests ──

    #[test]
    fn test_inject_tick() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);
        app.inject_tick();

        assert!(app.step().is_some());
        // Tick ignorowany przez CountingState → key_count == 0
        assert_eq!(app.state().key_count, 0);
    }

    // ── inject_resize tests ──

    #[test]
    fn test_inject_resize() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);
        app.inject_resize(100, 30);

        assert!(app.step().is_some());
        // Resize ignorowany przez CountingState
        assert_eq!(app.state().key_count, 0);
    }

    // ── step tests ──

    #[test]
    fn test_step_returns_false_when_no_events() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);
        assert!(app.step().is_none());
    }

    #[test]
    fn test_step_returns_true_when_event_available() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);
        app.inject_key(make_key(KeyCode::Char('x')));
        assert!(app.step().is_some());
    }

    // ── drain_events tests ──

    #[test]
    fn test_drain_events_processes_all() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);
        app.inject_keys(vec![
            make_key(KeyCode::Char('1')),
            make_key(KeyCode::Char('2')),
            make_key(KeyCode::Char('3')),
            make_key(KeyCode::Char('4')),
            make_key(KeyCode::Char('5')),
        ]);

        app.drain_events();

        assert_eq!(app.state().key_count, 5);
        assert_eq!(app.state().last_key, Some(KeyCode::Char('5')));
    }

    #[test]
    fn test_drain_events_empty_queue() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);
        app.drain_events();
        assert_eq!(app.state().key_count, 0);
    }

    // ── render tests ──

    #[test]
    fn test_render_returns_buffer() {
        let mut app = TestApp::new(RenderingState::new("Hello".into()), 80, 24);
        let buffer = app.render();

        assert_eq!(buffer.area.width, 80);
        assert_eq!(buffer.area.height, 24);
    }

    #[test]
    fn test_render_buffer_content() {
        let mut app = TestApp::new(RenderingState::new("Test".into()), 80, 24);
        let buffer = app.render();

        // Sprawdź pierwsze 4 znaki
        let content: String = (0..4)
            .map(|x| buffer.cell((x, 0)).unwrap().symbol())
            .collect();
        assert_eq!(content, "Test");
    }

    #[test]
    fn test_render_after_state_change() {
        let mut app = TestApp::new(RenderingState::new("".into()), 80, 24);

        // Wstrzyknij klawisz 'a' → dodaje 'A' do tekstu
        app.inject_key(make_key(KeyCode::Char('a')));
        app.step();

        let buffer = app.render();
        let symbol = buffer.cell((0, 0)).unwrap().symbol();
        assert_eq!(symbol, "A");
    }

    // ── assert_state tests ──

    #[test]
    fn test_assert_state_passes() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);
        app.inject_key(make_key(KeyCode::Char('x')));
        app.step();

        app.assert_state(|s| s.key_count == 1);
        app.assert_state(|s| s.last_key == Some(KeyCode::Char('x')));
    }

    #[test]
    #[should_panic(expected = "State assertion failed")]
    fn test_assert_state_fails() {
        let app = TestApp::new(CountingState::default(), 80, 24);
        app.assert_state(|s| s.key_count == 99);
    }

    // ── state accessors tests ──

    #[test]
    fn test_state_returns_immutable_ref() {
        let app = TestApp::new(CountingState::default(), 80, 24);
        let state = app.state();
        assert_eq!(state.key_count, 0);
    }

    #[test]
    fn test_state_mut_allows_modification() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);
        app.state_mut().key_count = 42;
        assert_eq!(app.state().key_count, 42);
    }

    // ── Integration tests: key injection → state change ──

    #[test]
    fn test_key_injection_updates_state() {
        let mut app = TestApp::new(CountingState::default(), 80, 24);

        // Sekwencja klawiszy
        app.inject_keys(vec![
            make_key(KeyCode::Char('h')),
            make_key(KeyCode::Char('j')),
            make_key(KeyCode::Char('k')),
            make_key(KeyCode::Char('l')),
        ]);

        // Przetwórz po kolei
        assert!(app.step().is_some());
        assert_eq!(app.state().key_count, 1);

        assert!(app.step().is_some());
        assert_eq!(app.state().key_count, 2);

        assert!(app.step().is_some());
        assert_eq!(app.state().key_count, 3);

        assert!(app.step().is_some());
        assert_eq!(app.state().key_count, 4);
        assert_eq!(app.state().last_key, Some(KeyCode::Char('l')));

        // Brak kolejnych eventów
        assert!(app.step().is_none());
    }

    #[test]
    fn test_full_workflow() {
        let mut app = TestApp::new(RenderingState::new("Init".into()), 80, 24);

        // Stan początkowy
        let buffer = app.render();
        let content: String = (0..4)
            .map(|x| buffer.cell((x, 0)).unwrap().symbol())
            .collect();
        assert_eq!(content, "Init");

        // Wstrzyknij klawisz 'a' 3 razy
        app.inject_keys(vec![
            make_key(KeyCode::Char('a')),
            make_key(KeyCode::Char('a')),
            make_key(KeyCode::Char('a')),
        ]);
        app.drain_events();

        // Sprawdź że state się zmienił
        assert_eq!(app.state().text, "InitAAA");

        // Renderuj i sprawdź buffer
        let buffer = app.render();
        let content: String = (0..7)
            .map(|x| buffer.cell((x, 0)).unwrap().symbol())
            .collect();
        assert_eq!(content, "InitAAA");
    }

    // ── run_steps tests ──

    #[test]
    fn test_run_steps_single_keypress() {
        use super::TestStep;

        let mut app = TestApp::new(CountingState::default(), 80, 24);

        app.run_steps(vec![TestStep::KeyPress(make_key(KeyCode::Char('x')))]);

        assert_eq!(app.state().key_count, 1);
        assert_eq!(app.state().last_key, Some(KeyCode::Char('x')));
    }

    #[test]
    fn test_run_steps_multiple_keypresses() {
        use super::TestStep;

        let mut app = TestApp::new(CountingState::default(), 80, 24);

        app.run_steps(vec![
            TestStep::KeyPress(make_key(KeyCode::Char('a'))),
            TestStep::KeyPress(make_key(KeyCode::Char('b'))),
            TestStep::KeyPress(make_key(KeyCode::Char('c'))),
        ]);

        assert_eq!(app.state().key_count, 3);
        assert_eq!(app.state().last_key, Some(KeyCode::Char('c')));
    }

    #[test]
    fn test_run_steps_with_state_assertion() {
        use super::TestStep;

        let mut app = TestApp::new(CountingState::default(), 80, 24);

        app.run_steps(vec![
            TestStep::KeyPress(make_key(KeyCode::Char('x'))),
            TestStep::AssertState(Box::new(|s| s.key_count == 1)),
            TestStep::KeyPress(make_key(KeyCode::Char('y'))),
            TestStep::AssertState(Box::new(|s| s.key_count == 2)),
        ]);

        assert_eq!(app.state().key_count, 2);
    }

    #[test]
    #[should_panic(expected = "State assertion failed")]
    fn test_run_steps_state_assertion_fails() {
        use super::TestStep;

        let mut app = TestApp::new(CountingState::default(), 80, 24);

        app.run_steps(vec![
            TestStep::KeyPress(make_key(KeyCode::Char('x'))),
            TestStep::AssertState(Box::new(|s| s.key_count == 99)),
        ]);
    }

    #[test]
    fn test_run_steps_with_render_assertion() {
        use super::TestStep;

        let mut app = TestApp::new(RenderingState::new("Test".into()), 80, 24);

        app.run_steps(vec![TestStep::AssertRender(Box::new(|buffer| {
            let symbol = buffer.cell((0, 0)).unwrap().symbol();
            symbol == "T"
        }))]);
    }

    #[test]
    #[should_panic(expected = "Render assertion failed")]
    fn test_run_steps_render_assertion_fails() {
        use super::TestStep;

        let mut app = TestApp::new(RenderingState::new("Test".into()), 80, 24);

        app.run_steps(vec![TestStep::AssertRender(Box::new(|buffer| {
            let symbol = buffer.cell((0, 0)).unwrap().symbol();
            symbol == "X"
        }))]);
    }

    #[test]
    fn test_run_steps_with_wait() {
        use super::TestStep;

        let mut app = TestApp::new(CountingState::default(), 80, 24);
        let start = Instant::now();

        app.run_steps(vec![
            TestStep::KeyPress(make_key(KeyCode::Char('x'))),
            TestStep::Wait(Duration::from_millis(50)),
            TestStep::AssertState(Box::new(|s| s.key_count == 1)),
        ]);

        // Sprawdź że Wait faktycznie zaczekał
        assert!(start.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    #[should_panic(expected = "timed out")]
    fn test_run_steps_timeout() {
        use super::TestStep;

        let mut app = TestApp::new(CountingState::default(), 80, 24);

        // Wait dłuższy niż timeout → panic
        app.run_steps_with_timeout(
            vec![TestStep::Wait(Duration::from_millis(200))],
            Duration::from_millis(50),
        );
    }

    #[test]
    fn test_run_steps_complex_sequence() {
        use super::TestStep;

        let mut app = TestApp::new(RenderingState::new("".into()), 80, 24);

        app.run_steps(vec![
            // Sprawdź początkowy stan
            TestStep::AssertState(Box::new(|s| s.text.is_empty())),
            // Dodaj 'A'
            TestStep::KeyPress(make_key(KeyCode::Char('a'))),
            TestStep::AssertState(Box::new(|s| s.text == "A")),
            // Dodaj kolejne 'A'
            TestStep::KeyPress(make_key(KeyCode::Char('a'))),
            TestStep::AssertState(Box::new(|s| s.text == "AA")),
            // Sprawdź renderowanie
            TestStep::AssertRender(Box::new(|buffer| {
                let c0 = buffer.cell((0, 0)).unwrap().symbol();
                let c1 = buffer.cell((1, 0)).unwrap().symbol();
                c0 == "A" && c1 == "A"
            })),
            // Krótkie opóźnienie
            TestStep::Wait(Duration::from_millis(10)),
            // Dodaj jeszcze jedno 'A'
            TestStep::KeyPress(make_key(KeyCode::Char('a'))),
            TestStep::AssertState(Box::new(|s| s.text == "AAA")),
        ]);

        assert_eq!(app.state().text, "AAA");
    }

    // ── buffer_contains_text tests ──

    #[test]
    fn test_buffer_contains_text_found() {
        let mut app = TestApp::new(RenderingState::new("Hello World".into()), 80, 24);
        let buffer = app.render();

        assert!(buffer_contains_text(&buffer, "Hello"));
        assert!(buffer_contains_text(&buffer, "World"));
        assert!(buffer_contains_text(&buffer, "Hello World"));
        assert!(buffer_contains_text(&buffer, "o W"));
    }

    #[test]
    fn test_buffer_contains_text_not_found() {
        let mut app = TestApp::new(RenderingState::new("Hello World".into()), 80, 24);
        let buffer = app.render();

        assert!(!buffer_contains_text(&buffer, "Goodbye"));
        assert!(!buffer_contains_text(&buffer, "xyz"));
        assert!(!buffer_contains_text(&buffer, "hello")); // case sensitive
    }

    #[test]
    fn test_buffer_contains_text_empty_buffer() {
        let mut app = TestApp::new(RenderingState::new("".into()), 80, 24);
        let buffer = app.render();

        assert!(!buffer_contains_text(&buffer, "anything"));
        assert!(buffer_contains_text(&buffer, "")); // empty string zawsze się znajdzie
    }

    // ── assert_contains_text / assert_not_contains_text tests ──

    #[test]
    fn test_assert_contains_text_passes() {
        let mut app = TestApp::new(RenderingState::new("Hello World".into()), 80, 24);
        let buffer = app.render();

        assert_contains_text(&buffer, "Hello");
        assert_contains_text(&buffer, "World");
    }

    #[test]
    #[should_panic(expected = "Expected buffer to contain")]
    fn test_assert_contains_text_panics_on_missing() {
        let mut app = TestApp::new(RenderingState::new("Hello".into()), 80, 24);
        let buffer = app.render();

        assert_contains_text(&buffer, "Goodbye");
    }

    #[test]
    fn test_assert_not_contains_text_passes() {
        let mut app = TestApp::new(RenderingState::new("Hello World".into()), 80, 24);
        let buffer = app.render();

        assert_not_contains_text(&buffer, "Goodbye");
        assert_not_contains_text(&buffer, "Error");
    }

    #[test]
    #[should_panic(expected = "Expected buffer NOT to contain")]
    fn test_assert_not_contains_text_panics_on_found() {
        let mut app = TestApp::new(RenderingState::new("Hello World".into()), 80, 24);
        let buffer = app.render();

        assert_not_contains_text(&buffer, "Hello");
    }

    // ── Integration test: run_steps with text assertions ──

    #[test]
    fn test_run_steps_with_text_assertions() {
        use super::TestStep;

        let mut app = TestApp::new(RenderingState::new("Init".into()), 80, 24);

        app.run_steps(vec![
            // Sprawdź początkowy tekst
            TestStep::AssertRender(Box::new(|buffer| {
                let mut line = String::new();
                for x in 0..4 {
                    line.push_str(buffer.cell((x, 0)).unwrap().symbol());
                }
                line == "Init"
            })),
            // Dodaj 'A'
            TestStep::KeyPress(make_key(KeyCode::Char('a'))),
            // Sprawdź że zawiera 'A'
            TestStep::AssertRender(Box::new(|buffer| {
                let mut line = String::new();
                for x in 0..5 {
                    line.push_str(buffer.cell((x, 0)).unwrap().symbol());
                }
                line.contains('A')
            })),
        ]);
    }
}
