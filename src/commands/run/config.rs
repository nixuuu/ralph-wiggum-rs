use std::path::PathBuf;

use crate::shared::error::{RalphError, Result};
use crate::shared::file_config::FileConfig;
use super::args::RunArgs;
use super::state::StateManager;

#[derive(Debug, Clone)]
pub struct Config {
    pub prompt: String,
    pub min_iterations: u32,
    pub max_iterations: u32,
    pub completion_promise: String,
    pub state_file: PathBuf,
    pub starting_iteration: u32,
    /// When true, enables --continue flag for subsequent iterations
    pub continue_session: bool,
    /// Custom system prompt prefix from .ralph.toml
    pub system_prompt_template: Option<String>,
    /// Use Nerd Font icons (false = ASCII fallback)
    pub use_nerd_font: bool,
    /// Path to PROGRESS.md for adaptive iterations (set by task continue)
    pub progress_file: Option<std::path::PathBuf>,
}

impl Config {
    pub fn build(args: RunArgs) -> Result<Self> {
        // Load file config (.ralph.toml)
        let file_config = FileConfig::load_from_path(&args.config)?;

        // CLI --no-nf has priority, then .ralph.toml, default = true
        let use_nerd_font = if args.no_nf {
            false
        } else {
            file_config.ui.nerd_font
        };

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
            // Note: on resume, the prompt already has prefix/suffix applied from initial run
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
                continue_session: args.continue_session,
                system_prompt_template: file_config.prompt.system.clone(),
                use_nerd_font,
                progress_file: args.progress_file,
            });
        }

        // Not resuming - require prompt
        let raw_prompt = args.prompt.ok_or_else(|| {
            RalphError::Config(
                "Prompt is required. Use --prompt or --resume with state file".into(),
            )
        })?;

        // Apply prefix and suffix from file config
        let prompt = file_config.wrap_user_prompt(&raw_prompt);

        Ok(Self {
            prompt,
            min_iterations: args.min_iterations,
            max_iterations: args.max_iterations,
            completion_promise: args.promise,
            state_file: args.state_file,
            starting_iteration: 0,
            continue_session: args.continue_session,
            system_prompt_template: file_config.prompt.system.clone(),
            use_nerd_font,
            progress_file: args.progress_file,
        })
    }
}
