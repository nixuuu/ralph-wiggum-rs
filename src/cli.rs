use clap::{Parser, Subcommand};

use crate::commands::run::RunArgs;
use crate::commands::task::TaskCommands;

#[derive(Parser, Debug)]
#[command(name = "ralph-wiggum")]
#[command(version)]
#[command(about = "Run claude in a loop until completion promise is found")]
#[command(before_help = crate::shared::banner::COLORED.as_str())]
#[command(args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// When no subcommand given, treat all args as RunArgs (backward compat)
    #[command(flatten)]
    pub run_args: RunArgs,
}

impl Cli {
    /// Zwraca czy flaga --debug jest aktywna dla tego CLI wywołania
    pub fn debug(&self) -> bool {
        match &self.command {
            Some(Commands::Run(args)) => args.debug,
            Some(Commands::Task { debug, .. }) => *debug,
            Some(Commands::Config { .. }) => false,
            Some(Commands::Update) => false,
            None => self.run_args.debug,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run claude in a loop until completion promise is found
    Run(RunArgs),

    /// Update to the latest version
    Update,

    /// Task management commands
    Task {
        /// Enable debug logging to .ralph/logs/ diagnostic file
        #[arg(long)]
        debug: bool,

        #[command(subcommand)]
        command: TaskCommands,
    },

    /// Configuration management commands
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Show merged configuration in TOML format
    Show,

    /// Show configuration file paths
    Path,

    /// Initialize global configuration with defaults
    Init,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_cli_args_continue_session_default() {
        // Without --continue-session flag, it should be false (default: no continuation)
        let cli = Cli::parse_from(["ralph-wiggum", "--prompt", "test"]);
        assert!(!cli.run_args.continue_session);
    }

    #[test]
    fn test_cli_args_continue_session_enabled() {
        // With --continue-session flag, it should be true
        let cli = Cli::parse_from(["ralph-wiggum", "--prompt", "test", "--continue-session"]);
        assert!(cli.run_args.continue_session);
    }

    #[test]
    fn test_cli_args_all_flags_together() {
        // Test that --continue-session works with other flags
        let cli = Cli::parse_from([
            "ralph-wiggum",
            "--prompt",
            "test prompt",
            "--min-iterations",
            "3",
            "--max-iterations",
            "10",
            "--continue-session",
        ]);
        assert!(cli.run_args.continue_session);
        assert_eq!(cli.run_args.min_iterations, 3);
        assert_eq!(cli.run_args.max_iterations, 10);
        assert_eq!(cli.run_args.prompt, Some("test prompt".to_string()));
    }

    #[test]
    fn test_subcommand_run() {
        let cli = Cli::parse_from(["ralph-wiggum", "run", "--prompt", "test"]);
        assert!(matches!(cli.command, Some(Commands::Run(_))));
    }

    #[test]
    fn test_subcommand_update() {
        let cli = Cli::parse_from(["ralph-wiggum", "update"]);
        assert!(matches!(cli.command, Some(Commands::Update)));
    }

    #[test]
    fn test_no_subcommand_backward_compat() {
        let cli = Cli::parse_from(["ralph-wiggum", "--prompt", "test"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.run_args.prompt, Some("test".to_string()));
    }

    #[test]
    fn test_debug_flag_default() {
        let cli = Cli::parse_from(["ralph-wiggum", "--prompt", "test"]);
        assert!(!cli.debug());
    }

    #[test]
    fn test_debug_flag_enabled() {
        let cli = Cli::parse_from(["ralph-wiggum", "--debug", "--prompt", "test"]);
        assert!(cli.debug());
    }

    #[test]
    fn test_debug_flag_with_run_subcommand() {
        let cli = Cli::parse_from(["ralph-wiggum", "run", "--debug", "--prompt", "test"]);
        assert!(cli.debug());
        assert!(matches!(cli.command, Some(Commands::Run(_))));
    }

    #[test]
    fn test_debug_flag_with_task_subcommand() {
        let cli = Cli::parse_from(["ralph-wiggum", "task", "--debug", "status"]);
        assert!(cli.debug());
        assert!(matches!(cli.command, Some(Commands::Task { .. })));
    }

    #[test]
    fn test_debug_flag_with_update_subcommand() {
        let cli = Cli::parse_from(["ralph-wiggum", "update"]);
        assert!(!cli.debug()); // update nie ma flagi debug
        assert!(matches!(cli.command, Some(Commands::Update)));
    }

    #[test]
    fn test_debug_flag_position() {
        // Flaga --debug musi być po nazwach subkomend, przed pozostałymi flagami
        let cli = Cli::parse_from(["ralph-wiggum", "--debug", "--prompt", "test"]);
        assert!(cli.debug());
        assert_eq!(cli.run_args.prompt, Some("test".to_string()));
    }

    #[test]
    fn test_subcommand_config_show() {
        let cli = Cli::parse_from(["ralph-wiggum", "config", "show"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Config {
                command: ConfigCommands::Show
            })
        ));
    }

    #[test]
    fn test_subcommand_config_path() {
        let cli = Cli::parse_from(["ralph-wiggum", "config", "path"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Config {
                command: ConfigCommands::Path
            })
        ));
    }

    #[test]
    fn test_subcommand_config_init() {
        let cli = Cli::parse_from(["ralph-wiggum", "config", "init"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Config {
                command: ConfigCommands::Init
            })
        ));
    }
}
