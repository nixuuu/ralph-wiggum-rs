use clap::Parser;
use std::path::PathBuf;

use crate::error::{RalphError, Result};
use crate::file_config::FileConfig;
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

    /// Path to config file (default: .ralph.toml)
    #[arg(short, long, default_value = ".ralph.toml")]
    pub config: PathBuf,

    /// Continue conversation from previous iteration
    /// (by default each iteration starts a fresh conversation)
    #[arg(long)]
    pub continue_session: bool,

    /// Update to the latest version
    #[arg(long)]
    pub update: bool,

    /// Disable Nerd Font icons (use ASCII fallback)
    #[arg(long)]
    pub no_nf: bool,
}

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
}

impl Config {
    pub fn build(args: CliArgs) -> Result<Self> {
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_cli_args_continue_session_default() {
        // Without --continue-session flag, it should be false (default: no continuation)
        let args = CliArgs::parse_from(["ralph-wiggum", "--prompt", "test"]);
        assert!(!args.continue_session);
    }

    #[test]
    fn test_cli_args_continue_session_enabled() {
        // With --continue-session flag, it should be true
        let args = CliArgs::parse_from(["ralph-wiggum", "--prompt", "test", "--continue-session"]);
        assert!(args.continue_session);
    }

    #[test]
    fn test_cli_args_all_flags_together() {
        // Test that --continue-session works with other flags
        let args = CliArgs::parse_from([
            "ralph-wiggum",
            "--prompt",
            "test prompt",
            "--min-iterations",
            "3",
            "--max-iterations",
            "10",
            "--continue-session",
        ]);
        assert!(args.continue_session);
        assert_eq!(args.min_iterations, 3);
        assert_eq!(args.max_iterations, 10);
        assert_eq!(args.prompt, Some("test prompt".to_string()));
    }
}
