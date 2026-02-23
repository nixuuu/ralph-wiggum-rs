pub mod args;
mod clean;
#[allow(dead_code)] // Public API — shared state for task commands with TUI
pub mod command_app;
mod continue_cmd;
#[allow(dead_code)] // Public API — będzie użyte przez task explorer command
pub mod explorer;
mod generate_deps_cmd;
mod input;
mod migrate;
pub mod orchestrate;
mod prd;
mod shared_runner;
mod state_helper;
mod status;
mod task_runner;

pub use args::TaskCommands;

use crate::shared::error::Result;
use crate::shared::file_config::FileConfig;
use shared_runner::{TaskCommandMode, execute_task_command};

pub async fn execute(command: TaskCommands, file_config: &FileConfig) -> Result<()> {
    crate::shared::banner::print_banner();

    match command {
        TaskCommands::Prd(args) => prd::execute(args, file_config).await,
        TaskCommands::Continue => continue_cmd::execute(file_config).await,
        TaskCommands::Add(args) => {
            execute_task_command(
                TaskCommandMode::Add,
                args.file,
                args.prompt,
                args.model,
                file_config,
            )
            .await
        }
        TaskCommands::Edit(args) => {
            execute_task_command(
                TaskCommandMode::Edit,
                args.file,
                args.prompt,
                args.model,
                file_config,
            )
            .await
        }
        TaskCommands::Plan(args) => {
            execute_task_command(
                TaskCommandMode::Plan,
                args.file,
                args.prompt,
                args.model,
                file_config,
            )
            .await
        }
        TaskCommands::Status => status::execute(file_config),
        TaskCommands::Orchestrate(args) => {
            let project_root = std::env::current_dir()?;
            let config = orchestrate::ResolvedConfig::from_args(&args, file_config);
            let orch =
                orchestrate::orchestrator::Orchestrator::new(config, file_config, project_root)?;
            orch.execute().await
        }
        TaskCommands::Clean => clean::execute(file_config).await,
        TaskCommands::GenerateDeps(args) => generate_deps_cmd::execute(args, file_config).await,
        TaskCommands::Migrate => migrate::execute(file_config).await,
    }
}
