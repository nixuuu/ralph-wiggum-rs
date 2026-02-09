use clap::Args;
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct RunArgs {
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

    /// Disable Nerd Font icons (use ASCII fallback)
    #[arg(long)]
    pub no_nf: bool,
}
