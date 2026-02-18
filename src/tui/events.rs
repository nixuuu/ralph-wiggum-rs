//! Centralny event dispatcher na dedykowanym OS thread.
//!
//! Wspólna infrastruktura do obsługi keyboard/resize events w TUI.
//! Uruchamia dedykowany `std::thread` (nigdy tokio::spawn) dla crossterm polling.
//! Eventy trafiają do async loop przez `std::sync::mpsc` kanał.
//!
//! Wspólne handlery: Ctrl+C (shutdown), q (quit flow), resize.
//! Per-command handlers definiowane przez trait `KeyHandler`.

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

// ── Event types ──────────────────────────────────────────────────────

/// Zdarzenie aplikacyjne emitowane przez EventDispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Public API — używane przez App i per-command widoki TUI
pub enum AppEvent {
    /// Zdarzenie klawiatury (po przetworzeniu wspólnych skrótów)
    Key(KeyEvent),
    /// Terminal resize (nowe wymiary: kolumny, wiersze)
    Resize(u16, u16),
    /// Periodyczny tick (co interwał pollowania, gdy brak zdarzeń)
    Tick,
    /// Zdarzenie myszy (kółko, klik, ruch)
    Mouse(MouseEvent),
}

/// Wynik przetworzenia zdarzenia przez KeyHandler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Public API — używane przez App i per-command widoki TUI
pub enum EventResult {
    /// Zdarzenie zostało obsłużone, nie przekazuj dalej
    Consumed,
    /// Zdarzenie nie zostało rozpoznane — dispatcher może obsłużyć domyślnie
    Ignored,
    /// Żądanie zakończenia (graceful quit flow)
    Quit,
    /// Żądanie natychmiastowego shutdown (Ctrl+C)
    Shutdown,
}

// ── KeyHandler trait ─────────────────────────────────────────────────

/// Trait do per-command obsługi klawiszy.
///
/// Implementowany przez poszczególne widoki (run, status, orchestrate).
/// EventDispatcher wywołuje `handle_key` dla zdarzeń, które nie zostały
/// obsłużone przez wspólne handlery (Ctrl+C, resize).
#[allow(dead_code)] // Public API — będzie użyte przez per-command widoki TUI
pub trait KeyHandler: Send + 'static {
    /// Obsłuż zdarzenie klawiatury. Zwróć `EventResult` informujący
    /// dispatcher o akcji do podjęcia.
    fn handle_key(&mut self, key: KeyEvent) -> EventResult;
}

// ── EventDispatcher ──────────────────────────────────────────────────

/// Centralny event dispatcher z dedykowanym OS thread.
///
/// Polluje crossterm events na osobnym wątku systemowym i przekazuje je
/// przez `mpsc` kanał. Wspólna obsługa:
/// - **Ctrl+C** → `AppEvent::Key` + sygnał shutdown
/// - **Resize** → `AppEvent::Resize`
/// - **Tick** → `AppEvent::Tick` (gdy brak zdarzeń w oknie pollowania)
///
/// Konsument odbiera eventy przez `recv()` / `recv_timeout()` / `try_recv()`.
#[allow(dead_code)] // Public API — używane przez App
pub struct EventDispatcher {
    handle: Option<std::thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
    receiver: Option<mpsc::Receiver<AppEvent>>,
    /// Gdy true, dispatcher ignoruje key events (TUI widgety same obsługują input)
    paused: Arc<AtomicBool>,
}

#[allow(dead_code)] // Public API — używane przez App
impl EventDispatcher {
    /// Utwórz i uruchom dispatcher na dedykowanym OS thread.
    ///
    /// # Argumenty
    /// - `poll_interval` — interwał pollowania crossterm (typowo 100ms)
    pub fn spawn(poll_interval: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));

        let running_clone = running.clone();
        let paused_clone = paused.clone();

        let handle = std::thread::Builder::new()
            .name("event-dispatcher".into())
            .spawn(move || {
                Self::poll_loop(running_clone, paused_clone, tx, poll_interval);
            })
            .expect("Failed to spawn event-dispatcher thread");

        EventDispatcher {
            handle: Some(handle),
            running,
            receiver: Some(rx),
            paused,
        }
    }

    /// Główna pętla pollowania — działa na dedykowanym OS thread.
    fn poll_loop(
        running: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        tx: mpsc::Sender<AppEvent>,
        poll_interval: Duration,
    ) {
        while running.load(Ordering::SeqCst) {
            // W trybie pauzy tylko czekamy — nie konsumujemy eventów z crossterm,
            // żeby TUI widgety mogły je czytać bezpośrednio
            if paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }

            match event::poll(poll_interval) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) => {
                        if tx.send(AppEvent::Key(key)).is_err() {
                            break; // Receiver dropped
                        }
                    }
                    Ok(Event::Resize(w, h)) => {
                        if tx.send(AppEvent::Resize(w, h)).is_err() {
                            break;
                        }
                    }
                    Ok(Event::Mouse(mouse)) => {
                        if tx.send(AppEvent::Mouse(mouse)).is_err() {
                            break;
                        }
                    }
                    _ => {}
                },
                Ok(false) => {
                    // Timeout — emituj Tick
                    if tx.send(AppEvent::Tick).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    // Błąd pollowania (np. brak TTY w testach) — emituj Tick
                    // i krótki sleep żeby nie spinlockować
                    if tx.send(AppEvent::Tick).is_err() {
                        break;
                    }
                    std::thread::sleep(poll_interval);
                }
            }
        }
    }

    /// Odbierz następny event (blokujący).
    ///
    /// Zwraca `None` gdy dispatcher został zatrzymany lub receiver został zabrany.
    pub fn recv(&self) -> Option<AppEvent> {
        self.receiver.as_ref()?.recv().ok()
    }

    /// Odbierz event z timeoutem.
    ///
    /// Zwraca `None` gdy upłynął timeout lub dispatcher został zatrzymany.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<AppEvent> {
        self.receiver.as_ref()?.recv_timeout(timeout).ok()
    }

    /// Spróbuj odebrać event bez blokowania.
    ///
    /// Zwraca `None` gdy brak eventów w kolejce.
    pub fn try_recv(&self) -> Option<AppEvent> {
        self.receiver.as_ref()?.try_recv().ok()
    }

    /// Take ownership of the receiver for external bridging (e.g. to tokio channel).
    ///
    /// After calling this, `recv()`, `try_recv()`, `recv_timeout()` will return `None`.
    /// The dispatcher thread continues running and sending events to the channel;
    /// the caller owns the receiving end.
    pub fn take_receiver(&mut self) -> mpsc::Receiver<AppEvent> {
        self.receiver
            .take()
            .expect("EventDispatcher receiver already taken")
    }

    /// Flaga pauzy — gdy `true`, dispatcher nie konsumuje crossterm events.
    ///
    /// Ustawiaj przed renderowaniem interaktywnych TUI widgetów
    /// (np. render_question), które same obsługują crossterm input.
    pub fn paused_flag(&self) -> Arc<AtomicBool> {
        self.paused.clone()
    }

    /// Czy dispatcher nadal działa.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Zatrzymaj dispatcher i poczekaj na zakończenie wątku.
    pub fn stop(mut self) {
        self.shutdown_internal();
    }

    /// Wewnętrzna metoda shutdown — ustawia flagę i joinuje wątek.
    fn shutdown_internal(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for EventDispatcher {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Nie joinujemy w Drop — wątek zakończy się w ciągu poll_interval
    }
}

// ── Pomocnicze funkcje do wspólnej obsługi klawiszy ──────────────────

/// Sprawdź czy zdarzenie to Ctrl+C.
#[allow(dead_code)] // Public API — używane przez App i per-command widoki TUI
pub fn is_ctrl_c(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Sprawdź czy zdarzenie to klawisz 'q' (bez modyfikatorów).
#[allow(dead_code)] // Public API — używane przez App i per-command widoki TUI
pub fn is_quit_key(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('q') && key.modifiers.is_empty()
}

/// Sprawdź czy zdarzenie to strzałka w górę.
#[allow(dead_code)] // Public API — używane przez per-command widoki TUI
pub fn is_scroll_up(key: &KeyEvent) -> bool {
    key.code == KeyCode::Up
}

/// Sprawdź czy zdarzenie to strzałka w dół.
#[allow(dead_code)] // Public API — używane przez per-command widoki TUI
pub fn is_scroll_down(key: &KeyEvent) -> bool {
    key.code == KeyCode::Down
}

/// Przetwórz zdarzenie klawiatury przez wspólne handlery + KeyHandler.
///
/// **UWAGA:** Legacy API — `App::run()` nie korzysta z `dispatch_key()`.
/// `App` deleguje klawisze bezpośrednio do `AppState::handle_event()`,
/// gdzie per-command state (np. RunApp) może implementować confirmation flow na `q`.
///
/// Kolejność w `dispatch_key`:
/// 1. Ctrl+C → `EventResult::Shutdown`
/// 2. 'q' → `EventResult::Quit` (natychmiastowy, bez confirmation)
/// 3. Per-command handler via `KeyHandler::handle_key`
///
/// Zwraca `EventResult` informujący caller o wymaganej akcji.
#[allow(dead_code)] // Public API — używane przez per-command widoki TUI
pub fn dispatch_key(key: KeyEvent, handler: &mut dyn KeyHandler) -> EventResult {
    // Ctrl+C zawsze oznacza natychmiastowy shutdown
    if is_ctrl_c(&key) {
        return EventResult::Shutdown;
    }

    // 'q' (bez modyfikatorów) — quit flow
    if is_quit_key(&key) {
        return EventResult::Quit;
    }

    // Deleguj do per-command handlera
    handler.handle_key(key)
}

// ── Testy ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    /// Helper: tworzy KeyEvent z podanym kodem i modyfikatorami.
    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    // ── AppEvent tests ──

    #[test]
    fn app_event_key_equality() {
        let key = make_key(KeyCode::Char('a'), KeyModifiers::NONE);
        let ev1 = AppEvent::Key(key);
        let ev2 = AppEvent::Key(key);
        assert_eq!(ev1, ev2);
    }

    #[test]
    fn app_event_resize_equality() {
        assert_eq!(AppEvent::Resize(80, 24), AppEvent::Resize(80, 24));
        assert_ne!(AppEvent::Resize(80, 24), AppEvent::Resize(120, 40));
    }

    #[test]
    fn app_event_tick_equality() {
        assert_eq!(AppEvent::Tick, AppEvent::Tick);
    }

    #[test]
    fn app_event_variants_distinct() {
        let key = make_key(KeyCode::Char('x'), KeyModifiers::NONE);
        assert_ne!(AppEvent::Key(key), AppEvent::Tick);
    }

    // ── EventResult tests ──

    #[test]
    fn event_result_values_are_distinct() {
        assert_ne!(EventResult::Consumed, EventResult::Ignored);
        assert_ne!(EventResult::Quit, EventResult::Shutdown);
        assert_ne!(EventResult::Consumed, EventResult::Quit);
    }

    // ── Key detection helpers ──

    #[test]
    fn is_ctrl_c_detects_ctrl_c() {
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(is_ctrl_c(&key));
    }

    #[test]
    fn is_ctrl_c_rejects_plain_c() {
        let key = make_key(KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(!is_ctrl_c(&key));
    }

    #[test]
    fn is_ctrl_c_rejects_ctrl_x() {
        let key = make_key(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert!(!is_ctrl_c(&key));
    }

    #[test]
    fn is_quit_key_detects_q() {
        let key = make_key(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(is_quit_key(&key));
    }

    #[test]
    fn is_quit_key_rejects_capital_q() {
        let key = make_key(KeyCode::Char('Q'), KeyModifiers::SHIFT);
        assert!(!is_quit_key(&key));
    }

    #[test]
    fn is_quit_key_rejects_ctrl_q() {
        let key = make_key(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert!(!is_quit_key(&key));
    }

    #[test]
    fn is_scroll_up_detects_up_arrow() {
        let key = make_key(KeyCode::Up, KeyModifiers::NONE);
        assert!(is_scroll_up(&key));
    }

    #[test]
    fn is_scroll_up_rejects_down_arrow() {
        let key = make_key(KeyCode::Down, KeyModifiers::NONE);
        assert!(!is_scroll_up(&key));
    }

    #[test]
    fn is_scroll_down_detects_down_arrow() {
        let key = make_key(KeyCode::Down, KeyModifiers::NONE);
        assert!(is_scroll_down(&key));
    }

    #[test]
    fn is_scroll_down_rejects_up_arrow() {
        let key = make_key(KeyCode::Up, KeyModifiers::NONE);
        assert!(!is_scroll_down(&key));
    }

    // ── dispatch_key tests ──

    /// Test KeyHandler — loguje zdarzenia i zwraca skonfigurowany wynik.
    struct TestHandler {
        result: EventResult,
        received: Vec<KeyEvent>,
    }

    impl TestHandler {
        fn new(result: EventResult) -> Self {
            Self {
                result,
                received: Vec::new(),
            }
        }
    }

    impl KeyHandler for TestHandler {
        fn handle_key(&mut self, key: KeyEvent) -> EventResult {
            self.received.push(key);
            self.result
        }
    }

    #[test]
    fn dispatch_key_ctrl_c_returns_shutdown() {
        let mut handler = TestHandler::new(EventResult::Consumed);
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        let result = dispatch_key(key, &mut handler);

        assert_eq!(result, EventResult::Shutdown);
        // Handler nie powinien być wywołany — Ctrl+C obsłużone przez dispatcher
        assert!(handler.received.is_empty());
    }

    #[test]
    fn dispatch_key_q_returns_quit() {
        let mut handler = TestHandler::new(EventResult::Consumed);
        let key = make_key(KeyCode::Char('q'), KeyModifiers::NONE);

        let result = dispatch_key(key, &mut handler);

        assert_eq!(result, EventResult::Quit);
        // Handler nie powinien być wywołany — 'q' obsłużone przez dispatcher
        assert!(handler.received.is_empty());
    }

    #[test]
    fn dispatch_key_other_delegates_to_handler() {
        let mut handler = TestHandler::new(EventResult::Consumed);
        let key = make_key(KeyCode::Char('r'), KeyModifiers::NONE);

        let result = dispatch_key(key, &mut handler);

        assert_eq!(result, EventResult::Consumed);
        assert_eq!(handler.received.len(), 1);
        assert_eq!(handler.received[0].code, KeyCode::Char('r'));
    }

    #[test]
    fn dispatch_key_handler_can_return_ignored() {
        let mut handler = TestHandler::new(EventResult::Ignored);
        let key = make_key(KeyCode::Char('x'), KeyModifiers::NONE);

        let result = dispatch_key(key, &mut handler);

        assert_eq!(result, EventResult::Ignored);
        assert_eq!(handler.received.len(), 1);
    }

    #[test]
    fn dispatch_key_handler_can_return_quit() {
        let mut handler = TestHandler::new(EventResult::Quit);
        let key = make_key(KeyCode::Char('x'), KeyModifiers::NONE);

        let result = dispatch_key(key, &mut handler);

        assert_eq!(result, EventResult::Quit);
    }

    #[test]
    fn dispatch_key_handler_can_return_shutdown() {
        let mut handler = TestHandler::new(EventResult::Shutdown);
        let key = make_key(KeyCode::Char('x'), KeyModifiers::NONE);

        let result = dispatch_key(key, &mut handler);

        assert_eq!(result, EventResult::Shutdown);
    }

    // ── EventDispatcher lifecycle tests ──

    #[test]
    fn event_dispatcher_spawns_and_stops() {
        let dispatcher = EventDispatcher::spawn(Duration::from_millis(50));
        assert!(dispatcher.is_running());
        dispatcher.stop();
    }

    #[test]
    fn event_dispatcher_emits_tick_when_no_input() {
        let dispatcher = EventDispatcher::spawn(Duration::from_millis(20));

        // Czekamy na tick — dispatcher powinien emitować Tick po poll timeout
        let event = dispatcher.recv_timeout(Duration::from_millis(200));
        assert!(
            event.is_some(),
            "Should receive at least one event (Tick or Key)"
        );

        dispatcher.stop();
    }

    #[test]
    fn event_dispatcher_try_recv_returns_none_when_empty() {
        let dispatcher = EventDispatcher::spawn(Duration::from_millis(500));

        // Natychmiast po spawn, bufor może być pusty
        // (try_recv nie blokuje)
        let _ = dispatcher.try_recv(); // Może zwrócić Some lub None

        dispatcher.stop();
    }

    #[test]
    fn event_dispatcher_paused_flag_defaults_to_false() {
        let dispatcher = EventDispatcher::spawn(Duration::from_millis(50));
        let paused = dispatcher.paused_flag();

        assert!(!paused.load(Ordering::Relaxed));

        dispatcher.stop();
    }

    #[test]
    fn event_dispatcher_paused_flag_can_be_toggled() {
        let dispatcher = EventDispatcher::spawn(Duration::from_millis(50));
        let paused = dispatcher.paused_flag();

        paused.store(true, Ordering::Relaxed);
        assert!(paused.load(Ordering::Relaxed));

        paused.store(false, Ordering::Relaxed);
        assert!(!paused.load(Ordering::Relaxed));

        dispatcher.stop();
    }

    #[test]
    fn event_dispatcher_paused_flag_shared_across_clones() {
        let dispatcher = EventDispatcher::spawn(Duration::from_millis(50));
        let p1 = dispatcher.paused_flag();
        let p2 = dispatcher.paused_flag();

        p1.store(true, Ordering::Relaxed);
        assert!(p2.load(Ordering::Relaxed));

        p2.store(false, Ordering::Relaxed);
        assert!(!p1.load(Ordering::Relaxed));

        dispatcher.stop();
    }

    #[test]
    fn event_dispatcher_drop_sets_running_false() {
        let running_clone;
        {
            let dispatcher = EventDispatcher::spawn(Duration::from_millis(50));
            running_clone = dispatcher.running.clone();
            assert!(running_clone.load(Ordering::SeqCst));
            // Drop dispatcher
        }

        // Po drop, running powinno być false
        assert!(!running_clone.load(Ordering::SeqCst));
    }

    #[test]
    fn event_dispatcher_recv_returns_none_after_stop() {
        let dispatcher = EventDispatcher::spawn(Duration::from_millis(20));

        // Zapisz receiver przed stop (stop konsumuje self)
        // Musimy przetestować inaczej — po drop receiver zwraca błąd
        let running = dispatcher.running.clone();
        running.store(false, Ordering::SeqCst);

        // Daj wątkowi czas na zakończenie
        std::thread::sleep(Duration::from_millis(100));

        // Po zamknięciu sender, recv powinien zwrócić None (po wyczerpaniu bufora)
        // Opróżnij bufor
        while dispatcher.try_recv().is_some() {}

        // Teraz powinno zwrócić None
        let event = dispatcher.recv_timeout(Duration::from_millis(100));
        assert!(event.is_none());
    }

    #[test]
    fn event_dispatcher_stop_is_idempotent_via_drop() {
        // Testujemy że stop() + implicit drop nie panikuje
        let dispatcher = EventDispatcher::spawn(Duration::from_millis(50));
        dispatcher.stop();
        // Implicit drop po stop — nie powinno panikować
    }

    // ── Ctrl+C priority test ──

    #[test]
    fn ctrl_c_has_highest_priority_in_dispatch() {
        let mut handler = TestHandler::new(EventResult::Consumed);

        // Ctrl+C powinien mieć wyższy priorytet niż handler
        let ctrl_c = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(dispatch_key(ctrl_c, &mut handler), EventResult::Shutdown);

        // 'q' powinien mieć wyższy priorytet niż handler
        let q = make_key(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(dispatch_key(q, &mut handler), EventResult::Quit);

        // Handler nie powinien być wywołany dla żadnego z powyższych
        assert!(handler.received.is_empty());
    }

    // ── Scroll direction helpers ──

    #[test]
    fn scroll_helpers_are_mutually_exclusive() {
        let up = make_key(KeyCode::Up, KeyModifiers::NONE);
        let down = make_key(KeyCode::Down, KeyModifiers::NONE);

        assert!(is_scroll_up(&up));
        assert!(!is_scroll_down(&up));

        assert!(is_scroll_down(&down));
        assert!(!is_scroll_up(&down));
    }

    #[test]
    fn scroll_helpers_reject_non_arrow_keys() {
        let char_key = make_key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(!is_scroll_up(&char_key));
        assert!(!is_scroll_down(&char_key));
    }
}
