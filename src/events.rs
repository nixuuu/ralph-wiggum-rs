use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Handle for managing the dedicated keyboard input thread.
///
/// Spawns an OS thread (not a tokio task) to poll crossterm keyboard events.
/// This ensures keyboard handling never blocks the async runtime.
pub struct InputThread {
    handle: Option<std::thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

impl InputThread {
    /// Spawn a dedicated OS thread for keyboard input.
    ///
    /// The thread polls crossterm events with a 100ms timeout and sets
    /// the `shutdown` flag on 'q' or Ctrl+C.
    pub fn spawn(shutdown: Arc<AtomicBool>) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let handle = std::thread::Builder::new()
            .name("input-thread".into())
            .spawn(move || {
                while running_clone.load(Ordering::SeqCst) {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }

                    if event::poll(Duration::from_millis(100)).unwrap_or(false)
                        && let Ok(Event::Key(key_event)) = event::read()
                    {
                        let should_quit = matches!(
                            key_event,
                            KeyEvent {
                                code: KeyCode::Char('q'),
                                ..
                            } | KeyEvent {
                                code: KeyCode::Char('c'),
                                modifiers: KeyModifiers::CONTROL,
                                ..
                            }
                        );
                        if should_quit {
                            shutdown.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                }
            })
            .expect("Failed to spawn input thread");

        InputThread {
            handle: Some(handle),
            running,
        }
    }

    /// Signal the thread to stop and wait for it to finish.
    pub fn stop(mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for InputThread {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Don't join in Drop to avoid blocking on panic unwind.
        // The thread exits within 100ms (poll timeout).
    }
}
