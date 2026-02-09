use std::path::PathBuf;
use std::process::Stdio;

use crate::shared::error::{RalphError, Result};

pub struct ClaudeOnceOptions {
    pub prompt: String,
    pub model: Option<String>,
    pub output_dir: Option<PathBuf>,
}

/// Run Claude CLI once (non-streaming, inherit stdout/stderr).
/// Used by `task prd` and `task add` for one-shot Claude invocations.
pub async fn run_claude_once(options: ClaudeOnceOptions) -> Result<()> {
    let mut cmd = tokio::process::Command::new("claude");
    cmd.arg("-p");
    cmd.arg("--dangerously-skip-permissions");

    if let Some(ref model) = options.model {
        cmd.arg("--model").arg(model);
    }

    cmd.arg(&options.prompt);
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    cmd.kill_on_drop(true);

    if let Some(ref dir) = options.output_dir {
        cmd.current_dir(dir);
    }

    let status = cmd
        .status()
        .await
        .map_err(|e| RalphError::ClaudeProcess(format!("Failed to run claude: {}", e)))?;

    if !status.success() {
        return Err(RalphError::ClaudeProcess(format!(
            "Claude exited with status: {}",
            status
        )));
    }

    Ok(())
}
