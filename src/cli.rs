use clap::{Parser, Subcommand};

use crate::commands::run::RunArgs;

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

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run claude in a loop until completion promise is found
    Run(RunArgs),

    /// Update to the latest version
    Update,
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
}
