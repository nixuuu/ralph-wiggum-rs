use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
    // Install Ctrl+C handler before spawning child (same pattern as run loop)
    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = signal::ctrl_c().await;
            shutdown.store(true, Ordering::SeqCst);
        });
    }

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

    // Put child in its own process group so terminal Ctrl+C (SIGINT)
    // goes only to ralph-wiggum, not to claude. Without this, claude's
    // Node.js SIGINT handler absorbs the signal and ralph-wiggum never sees it.
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd
        .spawn()
        .map_err(|e| RalphError::ClaudeProcess(format!("Failed to run claude: {}", e)))?;

    // Poll child with periodic shutdown checks
    let status = loop {
        tokio::select! {
            result = child.wait() => {
                break result.map_err(|e| RalphError::ClaudeProcess(
                    format!("Failed to wait for claude: {}", e)
                ))?;
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                if shutdown.load(Ordering::SeqCst) {
                    child.kill().await.ok();
                    return Err(RalphError::Interrupted);
                }
            }
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
