use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use tokio::signal;

use crate::shared::error::{RalphError, Result};

use super::events::InputThread;
use super::output::OutputFormatter;
use super::runner::ClaudeRunner;
use super::ui::StatusTerminal;

pub(crate) struct RunOnceOptions {
    pub prompt: String,
    pub model: Option<String>,
    pub output_dir: Option<PathBuf>,
    pub use_nerd_font: bool,
}

/// Run Claude CLI once with full streaming output (same UX as `run` command).
/// Sets up OutputFormatter + StatusTerminal + InputThread for rich display,
/// then runs a single ClaudeRunner invocation.
pub(crate) async fn run_once(options: RunOnceOptions) -> Result<()> {
    // Setup Ctrl+C handler
    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = signal::ctrl_c().await;
            shutdown.store(true, Ordering::SeqCst);
        });
    }

    // Initialize output formatter (single iteration)
    let formatter = Arc::new(Mutex::new(OutputFormatter::new(options.use_nerd_font)));
    {
        let mut fmt = formatter.lock().unwrap();
        fmt.set_iteration(1);
        fmt.start_iteration();
    }

    // Initialize status terminal (enables raw mode)
    let status_terminal = Arc::new(Mutex::new(StatusTerminal::new(options.use_nerd_font)?));

    // Flags required by InputThread (update-related ones are unused for one-shot)
    let resize_flag = Arc::new(AtomicBool::new(false));
    let update_state = Arc::new(AtomicU8::new(0));
    let update_trigger = Arc::new(AtomicBool::new(false));
    let refresh_flag = Arc::new(AtomicBool::new(false));

    // Spawn keyboard input thread (handles Ctrl+C/q in raw mode)
    let input_thread = InputThread::spawn(
        shutdown.clone(),
        resize_flag.clone(),
        update_state.clone(),
        update_trigger.clone(),
        refresh_flag.clone(),
    );

    // Print iteration header
    {
        let header_lines = formatter.lock().unwrap().format_iteration_header();
        let status = formatter.lock().unwrap().get_status();
        let mut term = status_terminal.lock().unwrap();
        term.print_lines(&header_lines)?;
        term.update(&status)?;
    }

    // Create and run ClaudeRunner
    let runner = ClaudeRunner::oneshot(options.prompt, options.model, options.output_dir);

    let formatter_event = formatter.clone();
    let status_terminal_event = status_terminal.clone();
    let shutdown_event = shutdown.clone();
    let resize_flag_event = resize_flag.clone();

    let formatter_idle = formatter.clone();
    let status_terminal_idle = status_terminal.clone();
    let resize_flag_idle = resize_flag.clone();

    let run_result = runner
        .run(
            shutdown.clone(),
            |event| {
                let (lines, status) = {
                    let mut fmt = formatter_event.lock().unwrap();
                    let lines = fmt.format_event(event);
                    let status = fmt.get_status();
                    (lines, status)
                };
                let mut term = status_terminal_event.lock().unwrap();
                if resize_flag_event.swap(false, Ordering::SeqCst) {
                    let _ = term.handle_resize(&status);
                }
                for line in &lines {
                    let _ = term.print_line(line);
                }
                if shutdown_event.load(Ordering::SeqCst) {
                    let _ = term.show_shutting_down();
                } else {
                    let _ = term.update(&status);
                }
            },
            || {
                let status = formatter_idle.lock().unwrap().get_status();
                let mut term = status_terminal_idle.lock().unwrap();
                if resize_flag_idle.swap(false, Ordering::SeqCst) {
                    let _ = term.handle_resize(&status);
                }
                let _ = term.update(&status);
            },
        )
        .await;

    // Cleanup terminal (disable raw mode, clear status bar)
    {
        let mut term = status_terminal.lock().unwrap();
        term.cleanup()?;
    }
    input_thread.stop();

    // Handle result
    match run_result {
        Ok(_) => Ok(()),
        Err(RalphError::Interrupted) => {
            let lines = formatter
                .lock()
                .unwrap()
                .format_interrupted(1);
            let mut term = status_terminal.lock().unwrap();
            term.print_lines(&lines)?;
            Err(RalphError::Interrupted)
        }
        Err(e) => Err(e),
    }
}
