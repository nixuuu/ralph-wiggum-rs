use std::path::PathBuf;
use std::process::Stdio;

use tokio::signal;

use crate::shared::error::{RalphError, Result};

pub struct ClaudeOnceOptions {
    pub prompt: String,
    pub model: Option<String>,
    pub output_dir: Option<PathBuf>,
}

/// Run Claude CLI once (non-streaming, inherit stdout/stderr).
/// Used by `task prd` and `task add` for one-shot Claude invocations.
/// Handles Ctrl+C gracefully — kills child process and returns `Interrupted`.
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

    let mut child = cmd
        .spawn()
        .map_err(|e| RalphError::ClaudeProcess(format!("Failed to run claude: {}", e)))?;

    // Race between child completion and Ctrl+C
    let status = tokio::select! {
        result = child.wait() => {
            result.map_err(|e| RalphError::ClaudeProcess(format!("Failed to wait for claude: {}", e)))?
        }
        _ = signal::ctrl_c() => {
            child.kill().await.ok();
            return Err(RalphError::Interrupted);
        }
    };

    if !status.success() {
        return Err(RalphError::ClaudeProcess(format!(
            "Claude exited with status: {}",
            status
        )));
    }

    Ok(())
}
