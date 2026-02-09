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
}
