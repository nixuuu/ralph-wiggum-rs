mod claude;
mod config;
mod error;
mod output;
mod prompt;
mod state;

use clap::Parser;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::signal;

use claude::ClaudeRunner;
use config::{CliArgs, Config};
use error::{RalphError, Result};
use output::{find_promise, OutputFormatter};
use prompt::build_wrapped_prompt;
use state::StateManager;

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
    let mut formatter = OutputFormatter::new();

    // Save initial state
    state_manager.save().await?;

    // Main loop
    loop {
        // Check shutdown signal
        if shutdown.load(Ordering::SeqCst) {
            state_manager.save().await?;
            formatter.print_interrupted(state_manager.iteration());
            return Err(RalphError::Interrupted);
        }

        // Check max iterations (before incrementing)
        if state_manager.is_max_reached() {
            formatter.print_stats(
                state_manager.iteration(),
                false,
                state_manager.completion_promise(),
            );
            // Cleanup state file on max iterations
            state_manager.cleanup().await?;
            return Err(RalphError::MaxIterations(state_manager.iteration()));
        }

        // Increment iteration
        state_manager.increment_iteration().await?;
        formatter.set_iteration(state_manager.iteration());
        formatter.print_iteration_header();

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

        // Run claude
        let last_message = runner
            .run(|event| {
                formatter.print_event(event);
            })
            .await?;

        // Check for completion promise in last message
        if let Some(text) = last_message {
            if find_promise(&text, state_manager.completion_promise()) {
                if state_manager.can_accept_promise() {
                    formatter.print_stats(
                        state_manager.iteration(),
                        true,
                        state_manager.completion_promise(),
                    );
                    // Cleanup state file on successful completion
                    state_manager.cleanup().await?;
                    return Ok(());
                } else {
                    // Promise found but min iterations not reached - continue
                    println!(
                        "\n[Promise found but ignoring - need {} more iteration(s) to reach minimum of {}]",
                        state_manager.min_iterations() - state_manager.iteration(),
                        state_manager.min_iterations()
                    );
                }
            }
        }

        // Check shutdown after iteration
        if shutdown.load(Ordering::SeqCst) {
            state_manager.save().await?;
            formatter.print_interrupted(state_manager.iteration());
            return Err(RalphError::Interrupted);
        }
    }
}
