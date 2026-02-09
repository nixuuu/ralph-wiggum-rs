use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use crate::updater::version_checker::UpdateState;

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
    /// the `shutdown` flag on 'q' or Ctrl+C, and the `resize_flag` on terminal resize.
    pub fn spawn(
        shutdown: Arc<AtomicBool>,
        resize_flag: Arc<AtomicBool>,
        update_state: Arc<AtomicU8>,
        update_trigger: Arc<AtomicBool>,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let handle = std::thread::Builder::new()
            .name("input-thread".into())
            .spawn(move || {
                while running_clone.load(Ordering::SeqCst) {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }

                    if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                        match event::read() {
                            Ok(Event::Key(key_event)) => {
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

                                // Ctrl+U: trigger background update
                                if matches!(
                                    key_event,
                                    KeyEvent {
                                        code: KeyCode::Char('u'),
                                        modifiers: KeyModifiers::CONTROL,
                                        ..
                                    }
                                ) {
                                    // Try Available → Downloading
                                    let triggered = update_state
                                        .compare_exchange(
                                            UpdateState::Available as u8,
                                            UpdateState::Downloading as u8,
                                            Ordering::SeqCst,
                                            Ordering::SeqCst,
                                        )
                                        .is_ok();
                                    // Or retry: Failed → Downloading
                                    let retried = !triggered
                                        && update_state
                                            .compare_exchange(
                                                UpdateState::Failed as u8,
                                                UpdateState::Downloading as u8,
                                                Ordering::SeqCst,
                                                Ordering::SeqCst,
                                            )
                                            .is_ok();
                                    if triggered || retried {
                                        update_trigger.store(true, Ordering::SeqCst);
                                    }
                                }
                            }
                            Ok(Event::Resize(_, _)) => {
                                resize_flag.store(true, Ordering::SeqCst);
                            }
                            _ => {}
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
