mod claude;
mod config;
mod error;
mod markdown;
mod output;
mod prompt;
mod state;
mod ui;

use clap::Parser;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::signal;

use claude::ClaudeRunner;
use config::{CliArgs, Config};
use error::{RalphError, Result};
use output::{OutputFormatter, find_promise};
use prompt::build_wrapped_prompt;
use state::StateManager;
use ui::StatusTerminal;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        match e {
            RalphError::Interrupted => {
                // Already handled, just exit
                std::process::exit(130); // Standard exit code for Ctrl+C
            }
            RalphError::MaxIterations(n) => {
                eprintln!("Max iterations ({}) reached without finding promise", n);
                std::process::exit(1);
            }
            _ => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    }
}

async fn run() -> Result<()> {
    // Parse CLI args
    let args = CliArgs::parse();

    // Build config
    let config = Config::build(args)?;

    // Initialize state manager
    let mut state_manager = StateManager::new(&config);

    // Setup signal handler for graceful shutdown
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    tokio::spawn(async move {
        let _ = signal::ctrl_c().await;
        shutdown_clone.store(true, Ordering::SeqCst);
    });

    // Initialize output formatter
    let formatter = Arc::new(Mutex::new(OutputFormatter::new()));
    formatter
        .lock()
        .unwrap()
        .set_max_iterations(config.max_iterations);

    // Initialize status terminal
    let status_terminal = Arc::new(Mutex::new(StatusTerminal::new()?));

    // Save initial state
    state_manager.save().await?;

    // Main loop
    loop {
        // Check shutdown signal
        if shutdown.load(Ordering::SeqCst) {
            state_manager.save().await?;
            status_terminal.lock().unwrap().cleanup()?;
            let lines = formatter
                .lock()
                .unwrap()
                .format_interrupted(state_manager.iteration());
            status_terminal.lock().unwrap().print_lines(&lines)?;
            return Err(RalphError::Interrupted);
        }

        // Check max iterations (before incrementing)
        if state_manager.is_max_reached() {
            status_terminal.lock().unwrap().cleanup()?;
            let lines = formatter.lock().unwrap().format_stats(
                state_manager.iteration(),
                false,
                state_manager.completion_promise(),
            );
            status_terminal.lock().unwrap().print_lines(&lines)?;
            // Cleanup state file on max iterations
            state_manager.cleanup().await?;
            return Err(RalphError::MaxIterations(state_manager.iteration()));
        }

        // Increment iteration
        state_manager.increment_iteration().await?;
        formatter
            .lock()
            .unwrap()
            .set_iteration(state_manager.iteration());
        formatter.lock().unwrap().start_iteration();

        // Print iteration header
        let header_lines = formatter.lock().unwrap().format_iteration_header();
        status_terminal.lock().unwrap().print_lines(&header_lines)?;

        // Update status bar at iteration start
        status_terminal
            .lock()
            .unwrap()
            .update(&formatter.lock().unwrap().get_status())?;

        // Build prompt with current iteration number
        let prompt = build_wrapped_prompt(
            &config.prompt,
            &config.completion_promise,
            state_manager.iteration(),
        );

        // Create runner - first iteration starts new session, others continue
        let runner = if state_manager.iteration() == 1 {
            ClaudeRunner::with_prompt(prompt)
        } else {
            ClaudeRunner::for_continuation(prompt)
        };

        // Clone Arc for use in closure and background task
        let formatter_clone = formatter.clone();
        let status_terminal_clone = status_terminal.clone();

        // Start background task to refresh status bar and handle keyboard input
        let refresh_running = Arc::new(AtomicBool::new(true));
        let refresh_running_clone = refresh_running.clone();
        let formatter_refresh = formatter.clone();
        let status_terminal_refresh = status_terminal.clone();
        let shutdown_for_keys = shutdown.clone();

        let refresh_handle = tokio::spawn(async move {
            use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                if !refresh_running_clone.load(Ordering::SeqCst) {
                    break;
                }

                // Check for keyboard input (non-blocking)
                if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                    if let Ok(Event::Key(key_event)) = event::read() {
                        match key_event {
                            // 'q' to quit
                            KeyEvent {
                                code: KeyCode::Char('q'),
                                ..
                            } => {
                                shutdown_for_keys.store(true, Ordering::SeqCst);
                                break;
                            }
                            // Ctrl+C to quit
                            KeyEvent {
                                code: KeyCode::Char('c'),
                                modifiers: KeyModifiers::CONTROL,
                                ..
                            } => {
                                shutdown_for_keys.store(true, Ordering::SeqCst);
                                break;
                            }
                            _ => {}
                        }
                    }
                }

                // Update status bar
                tokio::select! {
                    _ = interval.tick() => {
                        if let (Ok(fmt), Ok(mut term)) = (
                            formatter_refresh.lock(),
                            status_terminal_refresh.lock(),
                        ) {
                            let _ = term.update(&fmt.get_status());
                        }
                    }
                }
            }
        });

        // Run claude
        let run_result = runner
            .run(shutdown.clone(), |event| {
                let lines = formatter_clone.lock().unwrap().format_event(event);
                for line in lines {
                    let _ = status_terminal_clone.lock().unwrap().print_line(&line);
                }
                let _ = status_terminal_clone
                    .lock()
                    .unwrap()
                    .update(&formatter_clone.lock().unwrap().get_status());
            })
            .await;

        // Stop background refresh task
        refresh_running.store(false, Ordering::SeqCst);
        let _ = refresh_handle.await;

        // Handle interrupted - do cleanup before returning error
        let last_message = match run_result {
            Ok(msg) => msg,
            Err(RalphError::Interrupted) => {
                state_manager.save().await?;
                status_terminal.lock().unwrap().cleanup()?;
                let lines = formatter
                    .lock()
                    .unwrap()
                    .format_interrupted(state_manager.iteration());
                status_terminal.lock().unwrap().print_lines(&lines)?;
                return Err(RalphError::Interrupted);
            }
            Err(e) => return Err(e),
        };

        // Update status bar after iteration completes
        status_terminal
            .lock()
            .unwrap()
            .update(&formatter.lock().unwrap().get_status())?;

        // Check for completion promise in last message
        if let Some(text) = last_message {
            if find_promise(&text, state_manager.completion_promise()) {
                if state_manager.can_accept_promise() {
                    status_terminal.lock().unwrap().cleanup()?;
                    let lines = formatter.lock().unwrap().format_stats(
                        state_manager.iteration(),
                        true,
                        state_manager.completion_promise(),
                    );
                    status_terminal.lock().unwrap().print_lines(&lines)?;
                    // Cleanup state file on successful completion
                    state_manager.cleanup().await?;
                    return Ok(());
                } else {
                    // Promise found but min iterations not reached - continue
                    let msg = format!(
                        "\n[Promise found but ignoring - need {} more iteration(s) to reach minimum of {}]",
                        state_manager.min_iterations() - state_manager.iteration(),
                        state_manager.min_iterations()
                    );
                    status_terminal.lock().unwrap().print_line(&msg)?;
                }
            }
        }

        // Check shutdown after iteration
        if shutdown.load(Ordering::SeqCst) {
            state_manager.save().await?;
            status_terminal.lock().unwrap().cleanup()?;
            let lines = formatter
                .lock()
                .unwrap()
                .format_interrupted(state_manager.iteration());
            status_terminal.lock().unwrap().print_lines(&lines)?;
            return Err(RalphError::Interrupted);
        }
    }
}
