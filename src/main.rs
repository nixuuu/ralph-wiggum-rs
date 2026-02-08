mod claude;
mod config;
mod error;
mod events;
mod file_config;
mod icons;
mod markdown;
mod output;
mod prompt;
mod state;
mod ui;
mod updater;

use clap::Parser;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::signal;

use claude::ClaudeRunner;
use config::{CliArgs, Config};
use error::{RalphError, Result};
use events::InputThread;
use output::{OutputFormatter, find_promise};
use prompt::build_system_prompt;
use state::StateManager;
use ui::StatusTerminal;

#[tokio::main]
async fn main() {
    let args = CliArgs::parse();

    // Handle --update before TUI setup
    if args.update {
        if let Err(e) = updater::update_self() {
            eprintln!("Update failed: {e}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    if let Err(e) = run_with_args(args).await {
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

async fn run_with_args(args: CliArgs) -> Result<()> {
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

    // Initialize background version checker (non-blocking)
    let version_checker = updater::VersionChecker::new();
    let update_info = version_checker.update_info();
    version_checker.spawn_checker(shutdown.clone());

    // Initialize output formatter
    let formatter = Arc::new(Mutex::new(OutputFormatter::new(config.use_nerd_font)));
    formatter
        .lock()
        .unwrap()
        .set_max_iterations(config.max_iterations);

    // Initialize status terminal (enables raw mode)
    let status_terminal = Arc::new(Mutex::new(StatusTerminal::new(config.use_nerd_font)?));

    // Resize flag for terminal resize detection
    let resize_flag = Arc::new(AtomicBool::new(false));

    // Spawn dedicated OS thread for keyboard input (after raw mode is enabled)
    let input_thread = InputThread::spawn(shutdown.clone(), resize_flag.clone());

    // Save initial state
    state_manager.save().await?;

    // Main loop
    loop {
        // Check shutdown signal
        if shutdown.load(Ordering::SeqCst) {
            status_terminal.lock().unwrap().show_shutting_down()?;
            state_manager.save().await?;
            {
                let mut term = status_terminal.lock().unwrap();
                term.cleanup()?;
                let lines = formatter
                    .lock()
                    .unwrap()
                    .format_interrupted(state_manager.iteration());
                term.print_lines(&lines)?;
            }
            drop(input_thread);
            return Err(RalphError::Interrupted);
        }

        // Check max iterations (before incrementing)
        if state_manager.is_max_reached() {
            {
                let mut term = status_terminal.lock().unwrap();
                term.cleanup()?;
                let lines = formatter.lock().unwrap().format_stats(
                    state_manager.iteration(),
                    false,
                    state_manager.completion_promise(),
                );
                term.print_lines(&lines)?;
            }
            // Cleanup state file on max iterations
            state_manager.cleanup().await?;
            drop(input_thread);
            return Err(RalphError::MaxIterations(state_manager.iteration()));
        }

        // Increment iteration
        state_manager.increment_iteration().await?;
        {
            let mut fmt = formatter.lock().unwrap();
            fmt.set_iteration(state_manager.iteration());
            fmt.start_iteration();
        }

        // Print iteration header and update status bar
        {
            let header_lines = formatter.lock().unwrap().format_iteration_header();
            let mut status = formatter.lock().unwrap().get_status();
            status.update_info = update_info.lock().unwrap().clone();
            let mut term = status_terminal.lock().unwrap();
            if resize_flag.swap(false, Ordering::SeqCst) {
                let _ = term.handle_resize(&status);
            }
            term.print_lines(&header_lines)?;
            term.update(&status)?;
        }

        // Build system prompt with optional custom prefix
        let system_prompt = build_system_prompt(
            config.system_prompt_template.as_deref(),
            state_manager.iteration(),
            &config.completion_promise,
            config.min_iterations,
            config.max_iterations,
        );

        // Create runner - first iteration starts new session, others may continue
        // When continue_session is set, subsequent iterations continue the conversation
        // User prompt is passed as positional argument, system prompt via --append-system-prompt
        let runner = if state_manager.iteration() == 1 || !config.continue_session {
            ClaudeRunner::new(config.prompt.clone(), system_prompt)
        } else {
            ClaudeRunner::for_continuation(config.prompt.clone(), system_prompt)
        };

        // Check shutdown before spawning claude process
        if shutdown.load(Ordering::SeqCst) {
            continue; // Will be caught by shutdown check at top of loop
        }

        // Clone Arc for use in event closure
        let formatter_clone = formatter.clone();
        let status_terminal_clone = status_terminal.clone();
        let shutdown_for_callback = shutdown.clone();
        let update_info_clone = update_info.clone();
        let resize_flag_clone = resize_flag.clone();

        // Clone Arc for use in idle closure
        let resize_flag_idle = resize_flag.clone();
        let formatter_idle = formatter.clone();
        let status_terminal_idle = status_terminal.clone();
        let update_info_idle = update_info.clone();

        // Run claude with consolidated lock acquisitions in callback
        let run_result = runner
            .run(
                shutdown.clone(),
                |event| {
                    // Lock formatter once: format event + get status
                    let (lines, mut status) = {
                        let mut fmt = formatter_clone.lock().unwrap();
                        let lines = fmt.format_event(event);
                        let status = fmt.get_status();
                        (lines, status)
                    };
                    status.update_info = update_info_clone.lock().unwrap().clone();
                    // Lock terminal once: print all lines + update status bar
                    {
                        let mut term = status_terminal_clone.lock().unwrap();
                        // Handle resize: clear viewport and redraw status bar
                        if resize_flag_clone.swap(false, Ordering::SeqCst) {
                            let _ = term.handle_resize(&status);
                        }
                        for line in &lines {
                            let _ = term.print_line(line);
                        }
                        if shutdown_for_callback.load(Ordering::SeqCst) {
                            let _ = term.show_shutting_down();
                        } else {
                            let _ = term.update(&status);
                        }
                    }
                },
                || {
                    // Idle tick: handle resize between Claude events
                    if resize_flag_idle.swap(false, Ordering::SeqCst) {
                        let mut status = formatter_idle.lock().unwrap().get_status();
                        status.update_info = update_info_idle.lock().unwrap().clone();
                        let mut term = status_terminal_idle.lock().unwrap();
                        let _ = term.handle_resize(&status);
                    }
                },
            )
            .await;

        // Handle interrupted - do cleanup before returning error
        let last_message = match run_result {
            Ok(msg) => msg,
            Err(RalphError::Interrupted) => {
                status_terminal.lock().unwrap().show_shutting_down()?;
                state_manager.save().await?;
                {
                    let mut term = status_terminal.lock().unwrap();
                    term.cleanup()?;
                    let lines = formatter
                        .lock()
                        .unwrap()
                        .format_interrupted(state_manager.iteration());
                    term.print_lines(&lines)?;
                }
                drop(input_thread);
                return Err(RalphError::Interrupted);
            }
            Err(e) => return Err(e),
        };

        // Update status bar after iteration completes
        {
            let mut status = formatter.lock().unwrap().get_status();
            status.update_info = update_info.lock().unwrap().clone();
            let mut term = status_terminal.lock().unwrap();
            if resize_flag.swap(false, Ordering::SeqCst) {
                let _ = term.handle_resize(&status);
            }
            term.update(&status)?;
        }

        // Check for completion promise in last message
        if let Some(text) = last_message
            && find_promise(&text, state_manager.completion_promise())
        {
            if state_manager.can_accept_promise() {
                {
                    let mut term = status_terminal.lock().unwrap();
                    term.cleanup()?;
                    let lines = formatter.lock().unwrap().format_stats(
                        state_manager.iteration(),
                        true,
                        state_manager.completion_promise(),
                    );
                    term.print_lines(&lines)?;
                }
                // Cleanup state file on successful completion
                state_manager.cleanup().await?;
                input_thread.stop();
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

        // Check shutdown after iteration
        if shutdown.load(Ordering::SeqCst) {
            status_terminal.lock().unwrap().show_shutting_down()?;
            state_manager.save().await?;
            {
                let mut term = status_terminal.lock().unwrap();
                term.cleanup()?;
                let lines = formatter
                    .lock()
                    .unwrap()
                    .format_interrupted(state_manager.iteration());
                term.print_lines(&lines)?;
            }
            drop(input_thread);
            return Err(RalphError::Interrupted);
        }
    }
}
