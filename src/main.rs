mod claude;
mod config;
mod error;
mod output;
mod prompt;
mod state;
mod ui;

use clap::Parser;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::signal;

use claude::ClaudeRunner;
use config::{CliArgs, Config};
use error::{RalphError, Result};
use output::{find_promise, OutputFormatter};
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
    let formatter = Rc::new(RefCell::new(OutputFormatter::new()));
    formatter.borrow_mut().set_max_iterations(config.max_iterations);

    // Initialize status terminal
    let status_terminal = Rc::new(RefCell::new(StatusTerminal::new()?));

    // Save initial state
    state_manager.save().await?;

    // Main loop
    loop {
        // Check shutdown signal
        if shutdown.load(Ordering::SeqCst) {
            state_manager.save().await?;
            status_terminal.borrow_mut().cleanup()?;
            let lines = formatter.borrow().format_interrupted(state_manager.iteration());
            status_terminal.borrow_mut().print_lines(&lines)?;
            return Err(RalphError::Interrupted);
        }

        // Check max iterations (before incrementing)
        if state_manager.is_max_reached() {
            status_terminal.borrow_mut().cleanup()?;
            let lines = formatter.borrow().format_stats(
                state_manager.iteration(),
                false,
                state_manager.completion_promise(),
            );
            status_terminal.borrow_mut().print_lines(&lines)?;
            // Cleanup state file on max iterations
            state_manager.cleanup().await?;
            return Err(RalphError::MaxIterations(state_manager.iteration()));
        }

        // Increment iteration
        state_manager.increment_iteration().await?;
        formatter.borrow_mut().set_iteration(state_manager.iteration());
        formatter.borrow_mut().start_iteration();

        // Print iteration header
        let header_lines = formatter.borrow().format_iteration_header();
        status_terminal.borrow_mut().print_lines(&header_lines)?;

        // Update status bar at iteration start
        status_terminal.borrow_mut().update(&formatter.borrow().get_status())?;

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

        // Clone Rc for use in closure
        let formatter_clone = formatter.clone();
        let status_terminal_clone = status_terminal.clone();

        // Run claude
        let last_message = runner
            .run(|event| {
                let lines = formatter_clone.borrow_mut().format_event(event);
                for line in lines {
                    let _ = status_terminal_clone.borrow_mut().print_line(&line);
                }
                let _ = status_terminal_clone.borrow_mut().update(&formatter_clone.borrow().get_status());
            })
            .await?;

        // Update status bar after iteration completes
        status_terminal.borrow_mut().update(&formatter.borrow().get_status())?;

        // Check for completion promise in last message
        if let Some(text) = last_message {
            if find_promise(&text, state_manager.completion_promise()) {
                if state_manager.can_accept_promise() {
                    status_terminal.borrow_mut().cleanup()?;
                    let lines = formatter.borrow().format_stats(
                        state_manager.iteration(),
                        true,
                        state_manager.completion_promise(),
                    );
                    status_terminal.borrow_mut().print_lines(&lines)?;
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
                    status_terminal.borrow_mut().print_line(&msg)?;
                }
            }
        }

        // Check shutdown after iteration
        if shutdown.load(Ordering::SeqCst) {
            state_manager.save().await?;
            status_terminal.borrow_mut().cleanup()?;
            let lines = formatter.borrow().format_interrupted(state_manager.iteration());
            status_terminal.borrow_mut().print_lines(&lines)?;
            return Err(RalphError::Interrupted);
        }
    }
}
