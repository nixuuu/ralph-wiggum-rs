use std::path::PathBuf;

use super::args::RunArgs;
use super::state::StateManager;
use crate::shared::error::{RalphError, Result};
use crate::shared::file_config;

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
    /// Skip splash screen on startup (--no-splash)
    pub no_splash: bool,
    /// Command name for TUI header (default: "run")
    pub command_name: String,
}

impl Config {
    pub fn build(args: RunArgs) -> Result<Self> {
        // Load cascading config: defaults → global → local .ralph.toml
        let file_config = file_config::load_merged_config(&args.config)?;

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
                no_splash: args.no_splash,
                command_name: args.command_name.unwrap_or_else(|| "run".to_string()),
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
            no_splash: args.no_splash,
            command_name: args.command_name.unwrap_or_else(|| "run".to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create RunArgs with defaults, pointing config to a non-existent .ralph.toml
    /// so FileConfig::load_from_path returns defaults.
    fn make_args(tmp: &std::path::Path) -> RunArgs {
        RunArgs {
            prompt: Some("test prompt".to_string()),
            min_iterations: 1,
            max_iterations: 0,
            promise: "done".to_string(),
            resume: false,
            state_file: tmp.join("ralph-loop.local.md"),
            config: tmp.join(".ralph.toml"), // nie istnieje → FileConfig::default()
            continue_session: false,
            no_nf: false,
            debug: false,
            no_splash: false,
            progress_file: None,
            command_name: None,
        }
    }

    #[test]
    fn test_command_name_defaults_to_run() {
        let tmp = tempfile::tempdir().unwrap();
        let args = make_args(tmp.path());
        let config = Config::build(args).unwrap();
        assert_eq!(config.command_name, "run");
    }

    #[test]
    fn test_command_name_custom_value() {
        let tmp = tempfile::tempdir().unwrap();
        let mut args = make_args(tmp.path());
        args.command_name = Some("task continue".to_string());
        let config = Config::build(args).unwrap();
        assert_eq!(config.command_name, "task continue");
    }

    #[test]
    fn test_no_splash_false_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let args = make_args(tmp.path());
        let config = Config::build(args).unwrap();
        assert!(!config.no_splash);
    }

    #[test]
    fn test_no_splash_true_when_set() {
        let tmp = tempfile::tempdir().unwrap();
        let mut args = make_args(tmp.path());
        args.no_splash = true;
        let config = Config::build(args).unwrap();
        assert!(config.no_splash);
    }

    #[test]
    fn test_progress_file_none_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let args = make_args(tmp.path());
        let config = Config::build(args).unwrap();
        assert!(config.progress_file.is_none());
    }

    #[test]
    fn test_progress_file_passed_through() {
        let tmp = tempfile::tempdir().unwrap();
        let mut args = make_args(tmp.path());
        let tasks_path = tmp.path().join("tasks.yml");
        args.progress_file = Some(tasks_path.clone());
        let config = Config::build(args).unwrap();
        assert_eq!(config.progress_file, Some(tasks_path));
    }

    #[test]
    fn test_prompt_required_when_not_resuming() {
        let tmp = tempfile::tempdir().unwrap();
        let mut args = make_args(tmp.path());
        args.prompt = None;
        let result = Config::build(args);
        assert!(result.is_err());
    }
}
