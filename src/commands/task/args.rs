use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum TaskCommands {
    /// Generate project files from PRD
    Prd(PrdArgs),
    /// Continue working on tasks
    Continue,
    /// Add new tasks
    Add(AddArgs),
    /// Edit existing tasks in PROGRESS.md
    Edit(EditArgs),
    /// Show task status dashboard
    Status,
    /// Orchestrate parallel workers on tasks from PROGRESS.md
    Orchestrate(OrchestrateArgs),
    /// Clean up orphaned worktrees, branches, and state files
    Clean,
    /// Generate task dependencies using AI and update PROGRESS.md frontmatter
    GenerateDeps(GenerateDepsArgs),
}

#[derive(Args, Debug)]
pub struct PrdArgs {
    /// Path to PRD file
    #[arg(short, long)]
    pub file: Option<PathBuf>,

    /// PRD content as text
    #[arg(short, long)]
    pub prompt: Option<String>,

    /// Output directory (default: current directory)
    #[arg(short, long)]
    pub output_dir: Option<PathBuf>,

    /// Claude model to use
    #[arg(short, long)]
    pub model: Option<String>,
}

#[derive(Args, Debug)]
pub struct AddArgs {
    /// Path to requirements file
    #[arg(short, long)]
    pub file: Option<PathBuf>,

    /// Requirements as text
    #[arg(short, long)]
    pub prompt: Option<String>,

    /// Claude model to use
    #[arg(short, long)]
    pub model: Option<String>,
}

#[derive(Args, Debug)]
pub struct EditArgs {
    /// Path to file with edit instructions
    #[arg(short, long)]
    pub file: Option<PathBuf>,

    /// Edit instructions as text
    #[arg(short, long)]
    pub prompt: Option<String>,

    /// Claude model to use
    #[arg(short, long)]
    pub model: Option<String>,
}

#[derive(Args, Debug)]
pub struct OrchestrateArgs {
    /// Number of parallel workers
    #[arg(short, long)]
    pub workers: Option<u32>,

    /// Default Claude model for workers
    #[arg(short, long)]
    pub model: Option<String>,

    /// Max retries per task before marking as blocked
    #[arg(long)]
    pub max_retries: Option<u32>,

    /// Enable verbose event logging (JSONL)
    #[arg(short, long)]
    pub verbose: bool,

    /// Resume a previous orchestration session
    #[arg(long)]
    pub resume: bool,

    /// Show DAG plan without running workers
    #[arg(long)]
    pub dry_run: bool,

    /// Prefix for worktree directory names
    #[arg(long)]
    pub worktree_prefix: Option<String>,

    /// Skip merging — keep worktrees intact
    #[arg(long)]
    pub no_merge: bool,

    /// Maximum session cost in USD (e.g. 5.0)
    #[arg(long)]
    pub max_cost: Option<f64>,

    /// Maximum session duration (e.g. "2h", "30m")
    #[arg(long)]
    pub timeout: Option<String>,

    /// Filter specific tasks (comma-separated IDs, e.g. "T01,T03,T07")
    #[arg(long)]
    pub tasks: Option<String>,
}

#[derive(Args, Debug)]
pub struct GenerateDepsArgs {
    /// Claude model to use for dependency analysis
    #[arg(short, long)]
    pub model: Option<String>,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct TestCli {
        #[command(subcommand)]
        command: super::TaskCommands,
    }

    #[test]
    fn test_prd_with_file() {
        let cli = TestCli::parse_from(["test", "prd", "--file", "PRD.md"]);
        assert!(matches!(cli.command, super::TaskCommands::Prd(_)));
    }

    #[test]
    fn test_prd_with_prompt() {
        let cli = TestCli::parse_from(["test", "prd", "--prompt", "Build a REST API"]);
        if let super::TaskCommands::Prd(args) = cli.command {
            assert_eq!(args.prompt.as_deref(), Some("Build a REST API"));
        } else {
            panic!("Expected Prd command");
        }
    }

    #[test]
    fn test_continue() {
        let cli = TestCli::parse_from(["test", "continue"]);
        assert!(matches!(cli.command, super::TaskCommands::Continue));
    }

    #[test]
    fn test_add_with_prompt() {
        let cli = TestCli::parse_from(["test", "add", "--prompt", "Add auth"]);
        if let super::TaskCommands::Add(args) = cli.command {
            assert_eq!(args.prompt.as_deref(), Some("Add auth"));
        } else {
            panic!("Expected Add command");
        }
    }

    #[test]
    fn test_status() {
        let cli = TestCli::parse_from(["test", "status"]);
        assert!(matches!(cli.command, super::TaskCommands::Status));
    }

    #[test]
    fn test_edit_with_prompt() {
        let cli = TestCli::parse_from(["test", "edit", "--prompt", "Remove task 2.3"]);
        if let super::TaskCommands::Edit(args) = cli.command {
            assert_eq!(args.prompt.as_deref(), Some("Remove task 2.3"));
        } else {
            panic!("Expected Edit command");
        }
    }

    #[test]
    fn test_edit_with_file() {
        let cli = TestCli::parse_from(["test", "edit", "--file", "edits.md"]);
        if let super::TaskCommands::Edit(args) = cli.command {
            assert!(args.file.is_some());
        } else {
            panic!("Expected Edit command");
        }
    }

    #[test]
    fn test_prd_with_all_flags() {
        let cli = TestCli::parse_from([
            "test",
            "prd",
            "--file",
            "PRD.md",
            "--output-dir",
            "/tmp/out",
            "--model",
            "claude-sonnet-4-5-20250929",
        ]);
        if let super::TaskCommands::Prd(args) = cli.command {
            assert!(args.file.is_some());
            assert_eq!(
                args.output_dir.as_ref().unwrap().to_str().unwrap(),
                "/tmp/out"
            );
            assert_eq!(args.model.as_deref(), Some("claude-sonnet-4-5-20250929"));
        } else {
            panic!("Expected Prd command");
        }
    }

    #[test]
    fn test_orchestrate_defaults() {
        let cli = TestCli::parse_from(["test", "orchestrate"]);
        if let super::TaskCommands::Orchestrate(args) = cli.command {
            assert!(args.workers.is_none());
            assert!(args.model.is_none());
            assert!(args.max_retries.is_none());
            assert!(!args.verbose);
            assert!(!args.resume);
            assert!(!args.dry_run);
            assert!(!args.no_merge);
            assert!(args.max_cost.is_none());
            assert!(args.timeout.is_none());
            assert!(args.tasks.is_none());
        } else {
            panic!("Expected Orchestrate command");
        }
    }

    #[test]
    fn test_orchestrate_all_flags() {
        let cli = TestCli::parse_from([
            "test",
            "orchestrate",
            "--workers",
            "4",
            "--model",
            "claude-opus-4-6",
            "--max-retries",
            "5",
            "--verbose",
            "--resume",
            "--no-merge",
            "--max-cost",
            "10.0",
            "--timeout",
            "2h",
            "--tasks",
            "T01,T03,T07",
        ]);
        if let super::TaskCommands::Orchestrate(args) = cli.command {
            assert_eq!(args.workers, Some(4));
            assert_eq!(args.model.as_deref(), Some("claude-opus-4-6"));
            assert_eq!(args.max_retries, Some(5));
            assert!(args.verbose);
            assert!(args.resume);
            assert!(args.no_merge);
            assert_eq!(args.max_cost, Some(10.0));
            assert_eq!(args.timeout.as_deref(), Some("2h"));
            assert_eq!(args.tasks.as_deref(), Some("T01,T03,T07"));
        } else {
            panic!("Expected Orchestrate command");
        }
    }

    #[test]
    fn test_orchestrate_dry_run() {
        let cli = TestCli::parse_from(["test", "orchestrate", "--dry-run"]);
        if let super::TaskCommands::Orchestrate(args) = cli.command {
            assert!(args.dry_run);
        } else {
            panic!("Expected Orchestrate command");
        }
    }

    #[test]
    fn test_clean_command() {
        let cli = TestCli::parse_from(["test", "clean"]);
        assert!(matches!(cli.command, super::TaskCommands::Clean));
    }

    #[test]
    fn test_generate_deps_defaults() {
        let cli = TestCli::parse_from(["test", "generate-deps"]);
        if let super::TaskCommands::GenerateDeps(args) = cli.command {
            assert!(args.model.is_none());
        } else {
            panic!("Expected GenerateDeps command");
        }
    }

    #[test]
    fn test_generate_deps_with_model() {
        let cli = TestCli::parse_from(["test", "generate-deps", "--model", "claude-opus-4-6"]);
        if let super::TaskCommands::GenerateDeps(args) = cli.command {
            assert_eq!(args.model.as_deref(), Some("claude-opus-4-6"));
        } else {
            panic!("Expected GenerateDeps command");
        }
    }
}
