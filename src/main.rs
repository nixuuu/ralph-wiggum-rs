mod cli;
mod commands;
mod shared;
mod templates;
mod updater;

use clap::Parser;
use cli::{Cli, Commands};
use shared::error::RalphError;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Commands::Update) => {
            commands::update::execute();
            return;
        }
        Some(Commands::Run(args)) => commands::run::execute(args).await,
        Some(Commands::Task { command }) => {
            match shared::file_config::FileConfig::load_from_path(&std::path::PathBuf::from(
                ".ralph.toml",
            )) {
                Ok(file_config) => commands::task::execute(command, &file_config).await,
                Err(e) => Err(e),
            }
        }
        None => commands::run::execute(cli.run_args).await,
    };

    if let Err(e) = result {
        match e {
            RalphError::Interrupted => {
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
