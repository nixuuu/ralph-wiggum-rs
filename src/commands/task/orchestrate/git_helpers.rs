use tokio::process::Command;

/// Create a `git` command pre-configured to never block on interactive prompts.
///
/// Sets environment variables that prevent git from:
/// - Prompting for credentials (`GIT_TERMINAL_PROMPT=0`)
/// - Opening an editor (`GIT_EDITOR=true`)
/// - Using a pager (`GIT_PAGER=cat`)
///
/// This is critical for orchestrator workers and merge operations where
/// git commands run without a terminal attached.
pub(crate) fn git_command() -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_EDITOR", "true");
    cmd.env("GIT_PAGER", "cat");
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_git_command_runs_successfully() {
        let output = git_command()
            .args(["--version"])
            .output()
            .await
            .expect("git --version should succeed");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("git version"));
    }
}
