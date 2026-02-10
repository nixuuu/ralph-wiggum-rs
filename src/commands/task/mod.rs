mod add;
pub mod args;
mod continue_cmd;
mod edit;
mod input;
pub mod orchestrate;
mod prd;
mod status;

pub use args::TaskCommands;

use crate::shared::error::Result;
use crate::shared::file_config::FileConfig;

pub async fn execute(command: TaskCommands, file_config: &FileConfig) -> Result<()> {
    crate::shared::banner::print_banner();

    match command {
        TaskCommands::Prd(args) => prd::execute(args, file_config).await,
        TaskCommands::Continue => continue_cmd::execute(file_config).await,
        TaskCommands::Add(args) => add::execute(args, file_config).await,
        TaskCommands::Edit(args) => edit::execute(args, file_config).await,
        TaskCommands::Status => status::execute(file_config),
    }
}
