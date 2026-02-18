mod cli;
mod commands;
mod shared;
mod templates;
mod tui;
mod updater;

#[cfg(test)]
mod test_helpers;

use clap::Parser;
use cli::{Cli, Commands};
use shared::error::RalphError;

#[tokio::main]
async fn main() {
    // Cleanup old executable backup on Windows
    updater::executable_manager::cleanup_old_exe();

    let cli = Cli::parse();

    // Load configuration to get logging settings
    let file_config =
        shared::file_config::FileConfig::load_from_path(&std::path::PathBuf::from(".ralph.toml"))
            .unwrap_or_default();

    // Initialize diagnostics logger with config
    let debug = cli.debug();
    let log_dir = &file_config.logging.log_dir;
    let max_log_files = file_config.logging.max_log_files;

    if let Err(e) = shared::diagnostics::init_with_config(log_dir, max_log_files, debug) {
        eprintln!("Warning: Failed to initialize diagnostics logger: {}", e);
    } else if debug && let Some(log_path) = shared::diagnostics::log_file_path() {
        eprintln!("Debug logging enabled: {}", log_path.display());
    }

    let result = match cli.command {
        Some(Commands::Update) => {
            commands::update::execute();
            return;
        }
        Some(Commands::Run(args)) => {
            shared::banner::print_banner();
            commands::run::execute(args).await
        }
        Some(Commands::Task { command, .. }) => {
            commands::task::execute(command, &file_config).await
        }
        None => {
            shared::banner::print_banner();
            commands::run::execute(cli.run_args).await
        }
    };

    if let Err(e) = result {
        match e {
            RalphError::Interrupted => {
                std::process::exit(130); // Standard exit code for Ctrl+C
            }
            RalphError::MaxIterations(n) => {
                let msg = format!("Max iterations ({}) reached without finding promise", n);
                println!("{}", msg);
                diag_warn!("{}", msg);
                if let Some(log_path) = shared::diagnostics::log_file_path() {
                    println!("Diagnostic log: {}", log_path.display());
                }
                std::process::exit(1);
            }
            _ => {
                let msg = format!("Error: {}", e);
                println!("{}", msg);
                diag_warn!("{}", msg);
                if let Some(log_path) = shared::diagnostics::log_file_path() {
                    println!("Diagnostic log: {}", log_path.display());
                }
                std::process::exit(1);
            }
        }
    }
}
