use clap::Parser;
use std::path::PathBuf;

use crate::error::{RalphError, Result};
use crate::state::StateManager;

#[derive(Parser, Debug)]
#[command(name = "ralph-wiggum")]
#[command(version)]
#[command(about = "Run claude in a loop until completion promise is found")]
pub struct CliArgs {
    /// Prompt to send to claude
    #[arg(short, long)]
    pub prompt: Option<String>,

    /// Minimum iterations before accepting promise (default: 1)
    #[arg(short = 'm', long, default_value = "1")]
    pub min_iterations: u32,

    /// Maximum iterations (0 = unlimited)
    #[arg(short = 'n', long, default_value = "0")]
    pub max_iterations: u32,

    /// Completion promise text to look for
    #[arg(long, default_value = "done")]
    pub promise: String,

    /// Resume from state file
    #[arg(short, long)]
    pub resume: bool,

    /// Path to state file
    #[arg(long, default_value = ".claude/ralph-loop.local.md")]
    pub state_file: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub prompt: String,
    pub min_iterations: u32,
    pub max_iterations: u32,
    pub completion_promise: String,
    pub state_file: PathBuf,
    pub starting_iteration: u32,
}

impl Config {
    pub fn build(args: CliArgs) -> Result<Self> {
        // If resuming, load state from file
        if args.resume {
            if !args.state_file.exists() {
                return Err(RalphError::StateFile(format!(
                    "State file not found: {}. Cannot resume.",
                    args.state_file.display()
                )));
            }

            let (state, prompt) = StateManager::load_from_file(&args.state_file)?;

            // CLI args override state file values (except prompt which comes from file)
            let prompt = args.prompt.unwrap_or(prompt);
            let min_iterations = if args.min_iterations > 1 {
                args.min_iterations
            } else {
                state.min_iterations
            };
            let max_iterations = if args.max_iterations > 0 {
                args.max_iterations
            } else {
                state.max_iterations
            };
            let completion_promise = if args.promise != "done" {
                args.promise
            } else {
                state.completion_promise
            };

            return Ok(Self {
                prompt,
                min_iterations,
                max_iterations,
                completion_promise,
                state_file: args.state_file,
                starting_iteration: state.iteration,
            });
        }

        // Not resuming - require prompt
        let prompt = args.prompt.ok_or_else(|| {
            RalphError::Config(
                "Prompt is required. Use --prompt or --resume with state file".into(),
            )
        })?;

        Ok(Self {
            prompt,
            min_iterations: args.min_iterations,
            max_iterations: args.max_iterations,
            completion_promise: args.promise,
            state_file: args.state_file,
            starting_iteration: 0,
        })
    }
}
