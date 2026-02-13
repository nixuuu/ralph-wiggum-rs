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
    /// When true, the thread skips processing key events (used during TUI question rendering)
    paused: Arc<AtomicBool>,
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
        refresh_flag: Arc<AtomicBool>,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));
        let running_clone = running.clone();
        let paused_clone = paused.clone();

        let handle = std::thread::Builder::new()
            .name("input-thread".into())
            .spawn(move || {
                while running_clone.load(Ordering::SeqCst) {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }

                    // When paused, yield CPU without consuming events.
                    // TUI widgets (render_question) manage their own crossterm input
                    // via event::poll/read — we must NOT race them for events.
                    if paused_clone.load(Ordering::Relaxed) {
                        if shutdown.load(Ordering::SeqCst) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
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

                                // r: trigger progress refresh
                                if matches!(
                                    key_event,
                                    KeyEvent {
                                        code: KeyCode::Char('r'),
                                        ..
                                    }
                                ) {
                                    refresh_flag.store(true, Ordering::SeqCst);
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
            paused,
        }
    }

    /// Returns a shared handle to the paused flag for external control.
    ///
    /// When the paused flag is set to `true`, the input thread will continue running
    /// but will drain keyboard events without processing them. This allows TUI widgets
    /// (like question rendering) to handle keyboard input directly without conflicts.
    ///
    /// The paused flag should be set before rendering interactive TUI components and
    /// cleared afterwards. Consider using a RAII guard (like `PauseGuard`) to ensure
    /// the flag is always cleared, even if rendering panics.
    ///
    /// # Example
    /// ```ignore
    /// let input_thread = InputThread::spawn(/* ... */);
    /// let paused = input_thread.paused_flag();
    ///
    /// // Pause before rendering TUI widget
    /// paused.store(true, Ordering::Relaxed);
    /// render_interactive_widget();
    /// paused.store(false, Ordering::Relaxed);
    /// ```
    pub fn paused_flag(&self) -> Arc<AtomicBool> {
        self.paused.clone()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_thread_spawns_successfully() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let resize = Arc::new(AtomicBool::new(false));
        let update_state = Arc::new(AtomicU8::new(0));
        let update_trigger = Arc::new(AtomicBool::new(false));
        let refresh = Arc::new(AtomicBool::new(false));

        let thread = InputThread::spawn(
            shutdown.clone(),
            resize,
            update_state,
            update_trigger,
            refresh,
        );

        // Thread should be running
        assert!(thread.running.load(Ordering::SeqCst));

        // Cleanup
        shutdown.store(true, Ordering::SeqCst);
        thread.stop();
    }

    #[test]
    fn test_input_thread_stops_on_shutdown_flag() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let resize = Arc::new(AtomicBool::new(false));
        let update_state = Arc::new(AtomicU8::new(0));
        let update_trigger = Arc::new(AtomicBool::new(false));
        let refresh = Arc::new(AtomicBool::new(false));

        let thread = InputThread::spawn(
            shutdown.clone(),
            resize,
            update_state,
            update_trigger,
            refresh,
        );

        // Set shutdown flag
        shutdown.store(true, Ordering::SeqCst);

        // Give thread time to exit (max 200ms: 100ms poll + 100ms margin)
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Thread should have exited (we can't directly test but stop() should complete quickly)
        thread.stop();
    }

    #[test]
    fn test_input_thread_paused_flag() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let resize = Arc::new(AtomicBool::new(false));
        let update_state = Arc::new(AtomicU8::new(0));
        let update_trigger = Arc::new(AtomicBool::new(false));
        let refresh = Arc::new(AtomicBool::new(false));

        let thread = InputThread::spawn(
            shutdown.clone(),
            resize,
            update_state,
            update_trigger,
            refresh,
        );

        // Get paused flag
        let paused = thread.paused_flag();
        assert!(!paused.load(Ordering::Relaxed), "Should start unpaused");

        // Set paused
        paused.store(true, Ordering::Relaxed);
        assert!(paused.load(Ordering::Relaxed), "Should be paused");

        // Thread should still be running (just draining events)
        assert!(thread.running.load(Ordering::SeqCst));

        // Resume
        paused.store(false, Ordering::Relaxed);
        assert!(!paused.load(Ordering::Relaxed), "Should be resumed");

        // Cleanup
        shutdown.store(true, Ordering::SeqCst);
        thread.stop();
    }

    #[test]
    fn test_input_thread_stop_method() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let resize = Arc::new(AtomicBool::new(false));
        let update_state = Arc::new(AtomicU8::new(0));
        let update_trigger = Arc::new(AtomicBool::new(false));
        let refresh = Arc::new(AtomicBool::new(false));

        let thread = InputThread::spawn(shutdown, resize, update_state, update_trigger, refresh);

        // stop() should set running to false and join the thread
        thread.stop();
        // If we reach here, thread has stopped successfully
    }

    #[test]
    fn test_input_thread_drop_sets_running_false() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let resize = Arc::new(AtomicBool::new(false));
        let update_state = Arc::new(AtomicU8::new(0));
        let update_trigger = Arc::new(AtomicBool::new(false));
        let refresh = Arc::new(AtomicBool::new(false));

        let running_clone = {
            let thread = InputThread::spawn(
                shutdown.clone(),
                resize,
                update_state,
                update_trigger,
                refresh,
            );
            let running = thread.running.clone();

            assert!(running.load(Ordering::SeqCst), "Should be running");

            // Drop thread explicitly
            drop(thread);

            running
        };

        // After drop, running should be false
        assert!(
            !running_clone.load(Ordering::SeqCst),
            "Drop should set running to false"
        );
    }

    #[test]
    fn test_input_thread_paused_still_checks_shutdown() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let resize = Arc::new(AtomicBool::new(false));
        let update_state = Arc::new(AtomicU8::new(0));
        let update_trigger = Arc::new(AtomicBool::new(false));
        let refresh = Arc::new(AtomicBool::new(false));

        let thread = InputThread::spawn(
            shutdown.clone(),
            resize,
            update_state,
            update_trigger,
            refresh,
        );

        // Pause thread
        let paused = thread.paused_flag();
        paused.store(true, Ordering::Relaxed);

        // Set shutdown while paused
        shutdown.store(true, Ordering::SeqCst);

        // Thread should exit even when paused (checks shutdown in pause loop)
        std::thread::sleep(std::time::Duration::from_millis(200));

        // stop() should complete quickly since thread already exited
        thread.stop();
    }

    #[test]
    fn test_paused_flag_shared_across_clones() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let resize = Arc::new(AtomicBool::new(false));
        let update_state = Arc::new(AtomicU8::new(0));
        let update_trigger = Arc::new(AtomicBool::new(false));
        let refresh = Arc::new(AtomicBool::new(false));

        let thread = InputThread::spawn(
            shutdown.clone(),
            resize,
            update_state,
            update_trigger,
            refresh,
        );

        let paused1 = thread.paused_flag();
        let paused2 = thread.paused_flag();

        // Both should point to same Arc
        paused1.store(true, Ordering::Relaxed);
        assert!(
            paused2.load(Ordering::Relaxed),
            "Paused flag should be shared"
        );

        paused2.store(false, Ordering::Relaxed);
        assert!(
            !paused1.load(Ordering::Relaxed),
            "Changes should be visible to all clones"
        );

        // Cleanup
        shutdown.store(true, Ordering::SeqCst);
        thread.stop();
    }
}
