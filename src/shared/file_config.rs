use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::shared::error::{RalphError, Result};

/// A setup command to run after creating a worktree.
///
/// Can be a simple string or an object with `run` and optional `name`.
/// Supports template variables: `{ROOT_DIR}`, `{WORKTREE_DIR}`, `{TASK_ID}`, `{WORKER_ID}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum SetupCommand {
    Simple(String),
    Detailed {
        run: String,
        #[serde(default)]
        name: Option<String>,
    },
}

impl SetupCommand {
    pub fn command(&self) -> &str {
        match self {
            SetupCommand::Simple(s) => s,
            SetupCommand::Detailed { run, .. } => run,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            SetupCommand::Simple(s) => s,
            SetupCommand::Detailed { name: Some(n), .. } => n,
            SetupCommand::Detailed { run, .. } => run,
        }
    }
}

/// A verification command to run during worker's verify phase.
///
/// Can be a simple string or an object with `command` and optional `name`/`description`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
#[allow(dead_code)] // Used in task 13.3
pub enum VerifyCommand {
    Simple(String),
    Detailed {
        command: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
    },
}

#[allow(dead_code)] // Used in task 13.3
impl VerifyCommand {
    /// Returns the command string to execute.
    pub fn command(&self) -> &str {
        match self {
            VerifyCommand::Simple(s) => s,
            VerifyCommand::Detailed { command, .. } => command,
        }
    }

    /// Returns the optional name for this command.
    pub fn name(&self) -> Option<&str> {
        match self {
            VerifyCommand::Simple(_) => None,
            VerifyCommand::Detailed { name, .. } => name.as_deref(),
        }
    }

    /// Returns the optional description for this command.
    pub fn description(&self) -> Option<&str> {
        match self {
            VerifyCommand::Simple(_) => None,
            VerifyCommand::Detailed { description, .. } => description.as_deref(),
        }
    }

    /// Returns a display label for this command (name if available, otherwise command).
    pub fn label(&self) -> &str {
        match self {
            VerifyCommand::Simple(s) => s,
            VerifyCommand::Detailed {
                name: Some(n), ..
            } => n,
            VerifyCommand::Detailed { command, .. } => command,
        }
    }
}

/// Configuration loaded from .ralph.toml file
#[derive(Debug, Default, Deserialize)]
pub struct FileConfig {
    #[serde(default)]
    pub prompt: PromptConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub task: TaskConfig,
}

/// UI configuration
#[derive(Debug, Deserialize)]
pub struct UiConfig {
    /// Use Nerd Font icons (default: true). Set to false for ASCII fallback.
    #[serde(default = "default_true")]
    pub nerd_font: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { nerd_font: true }
    }
}

fn default_true() -> bool {
    true
}

/// Prompt configuration with optional prefix and suffix
#[derive(Debug, Default, Deserialize)]
pub struct PromptConfig {
    /// Text to prepend before user's prompt
    #[serde(default)]
    pub prefix: Option<String>,

    /// Text to append after user's prompt
    #[serde(default)]
    pub suffix: Option<String>,

    /// Custom system prompt prefix with placeholder support
    /// Available placeholders: {iteration}, {promise}, {min_iterations}, {max_iterations}
    #[serde(default)]
    pub system: Option<String>,
}

/// Task management configuration
#[derive(Debug, Deserialize)]
pub struct TaskConfig {
    #[serde(default = "default_progress_file")]
    pub progress_file: PathBuf,
    #[serde(default = "default_tasks_file")]
    pub tasks_file: PathBuf,
    #[serde(default)]
    pub output_dir: Option<PathBuf>,
    #[serde(default)]
    pub default_model: Option<String>,
    /// Orchestration settings (`[task.orchestrate]` in .ralph.toml)
    #[serde(default)]
    pub orchestrate: OrchestrateConfig,
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            progress_file: default_progress_file(),
            tasks_file: default_tasks_file(),
            output_dir: None,
            default_model: None,
            orchestrate: OrchestrateConfig::default(),
        }
    }
}

/// Configuration for task orchestration (`[task.orchestrate]` in .ralph.toml)
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // verify_commands field used in task 13.3
pub struct OrchestrateConfig {
    /// Number of parallel workers (default: 2)
    #[serde(default = "default_workers")]
    pub workers: u32,
    /// Max retries per task before marking as blocked (default: 3)
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Prefix for worktree directory names
    #[serde(default)]
    pub worktree_prefix: Option<String>,
    /// Default Claude model for workers
    #[serde(default)]
    pub default_model: Option<String>,
    /// Shell commands to run for verification phase.
    /// Can be simple strings or detailed objects with name/description.
    #[serde(default)]
    pub verify_commands: Vec<VerifyCommand>,
    /// Shell commands to run after creating worktree, before Claude starts.
    /// Supports template variables: {ROOT_DIR}, {WORKTREE_DIR}, {TASK_ID}, {WORKER_ID}
    #[serde(default)]
    pub setup_commands: Vec<SetupCommand>,
    /// Claude model to use for merge conflict resolution (default: "opus")
    #[serde(default)]
    pub conflict_resolution_model: Option<String>,
    /// How often (seconds) the watchdog checks for panicked/stuck workers (default: 10)
    #[serde(default = "default_watchdog_interval")]
    pub watchdog_interval_secs: u32,
    /// Max duration (in minutes) for a single worker phase before timeout.
    /// Default: 30 minutes. Set to 0 to disable phase timeout.
    #[serde(default = "default_phase_timeout")]
    pub phase_timeout_minutes: u32,
    /// Timeout (in seconds) for git commands (add, commit, status, etc).
    /// Default: 120 seconds (2 minutes).
    #[serde(default = "default_git_timeout")]
    pub git_timeout_secs: u32,
    /// Timeout (in seconds) for setup commands.
    /// Default: 300 seconds (5 minutes).
    #[serde(default = "default_setup_timeout")]
    pub setup_timeout_secs: u32,
    /// Max duration (in minutes) for a merge task (including AI conflict resolution).
    /// Default: 15 minutes. Set to 0 to disable merge timeout.
    #[serde(default = "default_merge_timeout")]
    pub merge_timeout_minutes: u32,
}

impl Default for OrchestrateConfig {
    fn default() -> Self {
        Self {
            workers: default_workers(),
            max_retries: default_max_retries(),
            worktree_prefix: None,
            default_model: None,
            verify_commands: Vec::new(),
            setup_commands: Vec::new(),
            conflict_resolution_model: None,
            watchdog_interval_secs: default_watchdog_interval(),
            phase_timeout_minutes: default_phase_timeout(),
            git_timeout_secs: default_git_timeout(),
            setup_timeout_secs: default_setup_timeout(),
            merge_timeout_minutes: default_merge_timeout(),
        }
    }
}

fn default_workers() -> u32 {
    2
}

fn default_max_retries() -> u32 {
    3
}

fn default_watchdog_interval() -> u32 {
    10
}

fn default_phase_timeout() -> u32 {
    30
}

fn default_git_timeout() -> u32 {
    120
}

fn default_setup_timeout() -> u32 {
    300
}

fn default_merge_timeout() -> u32 {
    15
}

fn default_progress_file() -> PathBuf {
    PathBuf::from("PROGRESS.md")
}

fn default_tasks_file() -> PathBuf {
    PathBuf::from(".ralph/tasks.yml")
}

impl FileConfig {
    /// Load configuration from a specific path
    /// Returns default config if file doesn't exist
    pub fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| RalphError::Config(format!("Failed to read {}: {}", path.display(), e)))?;

        toml::from_str(&content)
            .map_err(|e| RalphError::Config(format!("Failed to parse {}: {}", path.display(), e)))
    }

    /// Apply prefix and suffix to user prompt
    pub fn wrap_user_prompt(&self, user_prompt: &str) -> String {
        let mut result = String::new();

        if let Some(prefix) = &self.prompt.prefix {
            result.push_str(prefix);
            result.push_str("\n\n");
        }

        result.push_str(user_prompt);

        if let Some(suffix) = &self.prompt.suffix {
            result.push_str("\n\n");
            result.push_str(suffix);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = FileConfig::default();
        assert!(config.prompt.prefix.is_none());
        assert!(config.prompt.suffix.is_none());
    }

    #[test]
    fn test_wrap_user_prompt_no_modifications() {
        let config = FileConfig::default();
        let result = config.wrap_user_prompt("My prompt");
        assert_eq!(result, "My prompt");
    }

    #[test]
    fn test_wrap_user_prompt_with_prefix() {
        let config = FileConfig {
            prompt: PromptConfig {
                prefix: Some("PREFIX:".to_string()),
                suffix: None,
                system: None,
            },
            ..Default::default()
        };
        let result = config.wrap_user_prompt("My prompt");
        assert_eq!(result, "PREFIX:\n\nMy prompt");
    }

    #[test]
    fn test_wrap_user_prompt_with_suffix() {
        let config = FileConfig {
            prompt: PromptConfig {
                prefix: None,
                suffix: Some("SUFFIX".to_string()),
                system: None,
            },
            ..Default::default()
        };
        let result = config.wrap_user_prompt("My prompt");
        assert_eq!(result, "My prompt\n\nSUFFIX");
    }

    #[test]
    fn test_wrap_user_prompt_with_both() {
        let config = FileConfig {
            prompt: PromptConfig {
                prefix: Some("Kryteria akceptacji:".to_string()),
                suffix: Some("Śledź progres zmian w pliku TODO.md".to_string()),
                system: None,
            },
            ..Default::default()
        };
        let result = config.wrap_user_prompt("Napisz testy");
        assert_eq!(
            result,
            "Kryteria akceptacji:\n\nNapisz testy\n\nŚledź progres zmian w pliku TODO.md"
        );
    }

    #[test]
    fn test_parse_toml() {
        let toml_content = r#"
[prompt]
prefix = "Kryteria akceptacji:"
suffix = "Śledź progres"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.prompt.prefix.as_deref(),
            Some("Kryteria akceptacji:")
        );
        assert_eq!(config.prompt.suffix.as_deref(), Some("Śledź progres"));
    }

    #[test]
    fn test_parse_toml_partial() {
        let toml_content = r#"
[prompt]
prefix = "Only prefix"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.prompt.prefix.as_deref(), Some("Only prefix"));
        assert!(config.prompt.suffix.is_none());
    }

    #[test]
    fn test_parse_empty_toml() {
        let config: FileConfig = toml::from_str("").unwrap();
        assert!(config.prompt.prefix.is_none());
        assert!(config.prompt.suffix.is_none());
        assert!(config.prompt.system.is_none());
    }

    #[test]
    fn test_parse_toml_with_system_prompt() {
        let toml_content = r#"
[prompt]
system = "Iteration {iteration} of {max_iterations}"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.prompt.system.as_deref(),
            Some("Iteration {iteration} of {max_iterations}")
        );
    }

    #[test]
    fn test_default_system_is_none() {
        let config = FileConfig::default();
        assert!(config.prompt.system.is_none());
    }

    #[test]
    fn test_default_nerd_font_is_true() {
        let config = FileConfig::default();
        assert!(config.ui.nerd_font);
    }

    #[test]
    fn test_parse_toml_nerd_font_disabled() {
        let toml_content = r#"
[ui]
nerd_font = false
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert!(!config.ui.nerd_font);
    }

    #[test]
    fn test_parse_toml_without_ui_section() {
        let toml_content = r#"
[prompt]
prefix = "test"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert!(config.ui.nerd_font); // default = true
    }

    #[test]
    fn test_task_config_defaults() {
        let config = FileConfig::default();
        assert_eq!(
            config.task.progress_file,
            std::path::PathBuf::from("PROGRESS.md")
        );
        assert_eq!(
            config.task.tasks_file,
            std::path::PathBuf::from(".ralph/tasks.yml")
        );
        assert!(config.task.output_dir.is_none());
        assert!(config.task.default_model.is_none());
    }

    #[test]
    fn test_task_config_backward_compat() {
        // Existing .ralph.toml without [task] section should still work
        let toml_content = r#"
[prompt]
prefix = "test"

[ui]
nerd_font = false
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        // Task config should have all defaults
        assert_eq!(
            config.task.progress_file,
            std::path::PathBuf::from("PROGRESS.md")
        );
    }

    #[test]
    fn test_task_config_partial() {
        let toml_content = r#"
[task]
progress_file = "custom/PROGRESS.md"
default_model = "claude-sonnet-4-5-20250929"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.task.progress_file,
            std::path::PathBuf::from("custom/PROGRESS.md")
        );
        assert_eq!(
            config.task.default_model.as_deref(),
            Some("claude-sonnet-4-5-20250929")
        );
        // Other fields should be defaults
        assert_eq!(
            config.task.tasks_file,
            std::path::PathBuf::from(".ralph/tasks.yml")
        );
    }

    #[test]
    fn test_task_config_full() {
        let toml_content = r#"
[task]
progress_file = "PROG.md"
output_dir = "output"
default_model = "claude-opus-4-6"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.task.progress_file,
            std::path::PathBuf::from("PROG.md")
        );
        assert_eq!(
            config.task.output_dir,
            Some(std::path::PathBuf::from("output"))
        );
        assert_eq!(
            config.task.default_model.as_deref(),
            Some("claude-opus-4-6")
        );
    }

    // --- Orchestrate config tests ---

    #[test]
    fn test_orchestrate_config_defaults() {
        let config = FileConfig::default();
        assert_eq!(config.task.orchestrate.workers, 2);
        assert_eq!(config.task.orchestrate.max_retries, 3);
        assert!(config.task.orchestrate.worktree_prefix.is_none());
        assert!(config.task.orchestrate.default_model.is_none());
        assert!(config.task.orchestrate.verify_commands.is_empty());
    }

    #[test]
    fn test_orchestrate_config_backward_compat() {
        // Existing .ralph.toml without [task.orchestrate] should still work
        let toml_content = r#"
[task]
progress_file = "PROGRESS.md"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.workers, 2);
        assert_eq!(config.task.orchestrate.max_retries, 3);
    }

    #[test]
    fn test_orchestrate_config_full() {
        let toml_content = r#"
[task.orchestrate]
workers = 4
max_retries = 5
worktree_prefix = "my-project-ralph-w"
default_model = "claude-sonnet-4-5-20250929"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.workers, 4);
        assert_eq!(config.task.orchestrate.max_retries, 5);
        assert_eq!(
            config.task.orchestrate.worktree_prefix.as_deref(),
            Some("my-project-ralph-w")
        );
        assert_eq!(
            config.task.orchestrate.default_model.as_deref(),
            Some("claude-sonnet-4-5-20250929")
        );
    }

    #[test]
    fn test_orchestrate_config_partial() {
        let toml_content = r#"
[task.orchestrate]
workers = 6
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.workers, 6);
        assert_eq!(config.task.orchestrate.max_retries, 3); // default
        assert!(config.task.orchestrate.worktree_prefix.is_none());
    }

    // --- Setup commands tests ---

    #[test]
    fn test_setup_commands_default_empty() {
        let config = FileConfig::default();
        assert!(config.task.orchestrate.setup_commands.is_empty());
    }

    #[test]
    fn test_setup_commands_simple_strings() {
        let toml_content = r#"
[task.orchestrate]
setup_commands = ["npm install", "cp .env.example .env"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.setup_commands.len(), 2);
        assert_eq!(
            config.task.orchestrate.setup_commands[0].command(),
            "npm install"
        );
        assert_eq!(
            config.task.orchestrate.setup_commands[0].label(),
            "npm install"
        );
        assert_eq!(
            config.task.orchestrate.setup_commands[1].command(),
            "cp .env.example .env"
        );
    }

    #[test]
    fn test_setup_commands_detailed_objects() {
        let toml_content = r#"
[[task.orchestrate.setup_commands]]
run = "cp {ROOT_DIR}/.env {WORKTREE_DIR}/.env"
name = "Copy env file"

[[task.orchestrate.setup_commands]]
run = "npm install"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.setup_commands.len(), 2);
        assert_eq!(
            config.task.orchestrate.setup_commands[0].command(),
            "cp {ROOT_DIR}/.env {WORKTREE_DIR}/.env"
        );
        assert_eq!(
            config.task.orchestrate.setup_commands[0].label(),
            "Copy env file"
        );
        assert_eq!(
            config.task.orchestrate.setup_commands[1].command(),
            "npm install"
        );
        assert_eq!(
            config.task.orchestrate.setup_commands[1].label(),
            "npm install"
        );
    }

    #[test]
    fn test_setup_commands_backward_compat() {
        let toml_content = r#"
[task.orchestrate]
workers = 4
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert!(config.task.orchestrate.setup_commands.is_empty());
    }

    #[test]
    fn test_setup_command_label_fallback() {
        let cmd = SetupCommand::Detailed {
            run: "echo hello".to_string(),
            name: None,
        };
        assert_eq!(cmd.label(), "echo hello");
        assert_eq!(cmd.command(), "echo hello");

        let cmd = SetupCommand::Detailed {
            run: "echo hello".to_string(),
            name: Some("Greeting".to_string()),
        };
        assert_eq!(cmd.label(), "Greeting");
        assert_eq!(cmd.command(), "echo hello");
    }

    // --- Verify commands tests ---

    #[test]
    fn test_verify_commands_default_empty() {
        let config = FileConfig::default();
        assert!(config.task.orchestrate.verify_commands.is_empty());
    }

    #[test]
    fn test_verify_commands_simple_strings() {
        let toml_content = r#"
[task.orchestrate]
verify_commands = ["cargo test", "cargo clippy --all-targets -- -D warnings"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.verify_commands.len(), 2);
        assert_eq!(
            config.task.orchestrate.verify_commands[0].command(),
            "cargo test"
        );
        assert!(config.task.orchestrate.verify_commands[0].name().is_none());
        assert!(config.task.orchestrate.verify_commands[0].description().is_none());
        assert_eq!(
            config.task.orchestrate.verify_commands[1].command(),
            "cargo clippy --all-targets -- -D warnings"
        );
    }

    #[test]
    fn test_verify_commands_detailed_objects() {
        let toml_content = r#"
[[task.orchestrate.verify_commands]]
command = "cargo test"
name = "Unit tests"
description = "Run all unit and integration tests"

[[task.orchestrate.verify_commands]]
command = "cargo clippy --all-targets -- -D warnings"
name = "Lint"
description = "Check for clippy warnings"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.verify_commands.len(), 2);
        assert_eq!(
            config.task.orchestrate.verify_commands[0].command(),
            "cargo test"
        );
        assert_eq!(
            config.task.orchestrate.verify_commands[0].name(),
            Some("Unit tests")
        );
        assert_eq!(
            config.task.orchestrate.verify_commands[0].description(),
            Some("Run all unit and integration tests")
        );
        assert_eq!(
            config.task.orchestrate.verify_commands[1].command(),
            "cargo clippy --all-targets -- -D warnings"
        );
        assert_eq!(
            config.task.orchestrate.verify_commands[1].name(),
            Some("Lint")
        );
        assert_eq!(
            config.task.orchestrate.verify_commands[1].description(),
            Some("Check for clippy warnings")
        );
    }

    #[test]
    fn test_verify_commands_mixed_format() {
        let toml_content = r#"
[task.orchestrate]
verify_commands = [
    "cargo fmt --check",
    { command = "cargo test", name = "Tests", description = "Run test suite" }
]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.verify_commands.len(), 2);
        // First is simple
        assert_eq!(
            config.task.orchestrate.verify_commands[0].command(),
            "cargo fmt --check"
        );
        assert!(config.task.orchestrate.verify_commands[0].name().is_none());
        assert!(config.task.orchestrate.verify_commands[0].description().is_none());
        // Second is detailed
        assert_eq!(
            config.task.orchestrate.verify_commands[1].command(),
            "cargo test"
        );
        assert_eq!(
            config.task.orchestrate.verify_commands[1].name(),
            Some("Tests")
        );
        assert_eq!(
            config.task.orchestrate.verify_commands[1].description(),
            Some("Run test suite")
        );
    }

    #[test]
    fn test_verify_commands_backward_compat_empty() {
        let toml_content = r#"
[task.orchestrate]
workers = 4
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert!(config.task.orchestrate.verify_commands.is_empty());
    }

    #[test]
    fn test_verify_command_methods() {
        let simple = VerifyCommand::Simple("cargo test".to_string());
        assert_eq!(simple.command(), "cargo test");
        assert!(simple.name().is_none());
        assert!(simple.description().is_none());

        let detailed = VerifyCommand::Detailed {
            command: "cargo clippy".to_string(),
            name: Some("Lint".to_string()),
            description: Some("Run linter".to_string()),
        };
        assert_eq!(detailed.command(), "cargo clippy");
        assert_eq!(detailed.name(), Some("Lint"));
        assert_eq!(detailed.description(), Some("Run linter"));

        let detailed_partial = VerifyCommand::Detailed {
            command: "cargo build".to_string(),
            name: Some("Build".to_string()),
            description: None,
        };
        assert_eq!(detailed_partial.command(), "cargo build");
        assert_eq!(detailed_partial.name(), Some("Build"));
        assert!(detailed_partial.description().is_none());
    }

    #[test]
    fn test_verify_commands_object_without_optional_fields() {
        // Test parsing TOML with only 'command' field (no name/description)
        let toml_content = r#"
[[task.orchestrate.verify_commands]]
command = "cargo test"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.verify_commands.len(), 1);
        assert_eq!(
            config.task.orchestrate.verify_commands[0].command(),
            "cargo test"
        );
        assert!(config.task.orchestrate.verify_commands[0].name().is_none());
        assert!(config.task.orchestrate.verify_commands[0]
            .description()
            .is_none());
    }

    #[test]
    fn test_verify_commands_empty_strings() {
        // Test that empty command strings are preserved (handled at runtime)
        let toml_content = r#"
[task.orchestrate]
verify_commands = [""]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.verify_commands.len(), 1);
        assert_eq!(
            config.task.orchestrate.verify_commands[0].command(),
            ""
        );
    }

    #[test]
    fn test_verify_commands_whitespace_only() {
        // Test commands with only whitespace
        let toml_content = r#"
[task.orchestrate]
verify_commands = ["   ", "\t\n"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.verify_commands.len(), 2);
        assert_eq!(
            config.task.orchestrate.verify_commands[0].command(),
            "   "
        );
        assert_eq!(
            config.task.orchestrate.verify_commands[1].command(),
            "\t\n"
        );
    }

    #[test]
    fn test_verify_command_label() {
        // Simple variant - label is the command
        let simple = VerifyCommand::Simple("cargo test".to_string());
        assert_eq!(simple.label(), "cargo test");

        // Detailed with name - label is the name
        let detailed_with_name = VerifyCommand::Detailed {
            command: "cargo clippy --all".to_string(),
            name: Some("Lint".to_string()),
            description: Some("Run linter".to_string()),
        };
        assert_eq!(detailed_with_name.label(), "Lint");

        // Detailed without name - label falls back to command
        let detailed_without_name = VerifyCommand::Detailed {
            command: "cargo build".to_string(),
            name: None,
            description: Some("Build project".to_string()),
        };
        assert_eq!(detailed_without_name.label(), "cargo build");
    }

    #[test]
    fn test_orchestrate_config_conflict_resolution_model() {
        let toml_content = r#"
[task.orchestrate]
conflict_resolution_model = "opus"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.task.orchestrate.conflict_resolution_model.as_deref(),
            Some("opus")
        );
    }

    #[test]
    fn test_orchestrate_config_conflict_resolution_model_default_none() {
        let config = FileConfig::default();
        assert!(config.task.orchestrate.conflict_resolution_model.is_none());
    }

    #[test]
    fn test_orchestrate_config_with_all_fields() {
        let toml_content = r#"
[task.orchestrate]
workers = 4
max_retries = 5
worktree_prefix = "my-project"
default_model = "claude-sonnet-4-5-20250929"
conflict_resolution_model = "claude-opus-4-6"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.workers, 4);
        assert_eq!(config.task.orchestrate.max_retries, 5);
        assert_eq!(
            config.task.orchestrate.worktree_prefix.as_deref(),
            Some("my-project")
        );
        assert_eq!(
            config.task.orchestrate.default_model.as_deref(),
            Some("claude-sonnet-4-5-20250929")
        );
        assert_eq!(
            config.task.orchestrate.conflict_resolution_model.as_deref(),
            Some("claude-opus-4-6")
        );
    }

    // --- Watchdog interval tests ---

    #[test]
    fn test_watchdog_interval_default() {
        let config = FileConfig::default();
        assert_eq!(config.task.orchestrate.watchdog_interval_secs, 10);
    }

    #[test]
    fn test_watchdog_interval_custom() {
        let toml_content = r#"
[task.orchestrate]
watchdog_interval_secs = 30
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.watchdog_interval_secs, 30);
    }

    #[test]
    fn test_watchdog_interval_backward_compat() {
        // Existing .ralph.toml without watchdog_interval_secs should use default
        let toml_content = r#"
[task.orchestrate]
workers = 4
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.watchdog_interval_secs, 10);
    }

    // --- Phase timeout tests ---

    #[test]
    fn test_phase_timeout_default() {
        let config = FileConfig::default();
        assert_eq!(config.task.orchestrate.phase_timeout_minutes, 30);
    }

    #[test]
    fn test_phase_timeout_custom() {
        let toml_content = r#"
[task.orchestrate]
phase_timeout_minutes = 60
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.phase_timeout_minutes, 60);
    }

    #[test]
    fn test_phase_timeout_backward_compat() {
        // Existing config without phase_timeout_minutes should use default
        let toml_content = r#"
[task.orchestrate]
workers = 4
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.phase_timeout_minutes, 30);
    }

    // --- Git timeout tests ---

    #[test]
    fn test_git_timeout_default() {
        let config = FileConfig::default();
        assert_eq!(config.task.orchestrate.git_timeout_secs, 120);
    }

    #[test]
    fn test_git_timeout_custom() {
        let toml_content = r#"
[task.orchestrate]
git_timeout_secs = 60
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.git_timeout_secs, 60);
    }

    #[test]
    fn test_git_timeout_backward_compat() {
        let toml_content = r#"
[task.orchestrate]
workers = 4
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.git_timeout_secs, 120);
    }

    // --- Setup timeout tests ---

    #[test]
    fn test_setup_timeout_default() {
        let config = FileConfig::default();
        assert_eq!(config.task.orchestrate.setup_timeout_secs, 300);
    }

    #[test]
    fn test_setup_timeout_custom() {
        let toml_content = r#"
[task.orchestrate]
setup_timeout_secs = 600
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.setup_timeout_secs, 600);
    }

    #[test]
    fn test_setup_timeout_backward_compat() {
        let toml_content = r#"
[task.orchestrate]
workers = 4
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.setup_timeout_secs, 300);
    }

    #[test]
    fn test_orchestrate_config_with_all_timeouts() {
        let toml_content = r#"
[task.orchestrate]
workers = 4
phase_timeout_minutes = 60
git_timeout_secs = 90
setup_timeout_secs = 450
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.workers, 4);
        assert_eq!(config.task.orchestrate.phase_timeout_minutes, 60);
        assert_eq!(config.task.orchestrate.git_timeout_secs, 90);
        assert_eq!(config.task.orchestrate.setup_timeout_secs, 450);
    }

    // ── Task 15.6: Tests for per-phase timeout, watchdog, heartbeat ────

    #[test]
    fn test_default_config_values_task_15_6() {
        // Verify all default timeout values
        let config = FileConfig::default();
        assert_eq!(
            config.task.orchestrate.phase_timeout_minutes, 30,
            "Default phase_timeout should be 30 minutes"
        );
        assert_eq!(
            config.task.orchestrate.git_timeout_secs, 120,
            "Default git_timeout should be 120 seconds"
        );
        assert_eq!(
            config.task.orchestrate.setup_timeout_secs, 300,
            "Default setup_timeout should be 300 seconds"
        );
        assert_eq!(
            config.task.orchestrate.watchdog_interval_secs, 10,
            "Default watchdog_interval should be 10 seconds"
        );
    }

    #[test]
    fn test_phase_timeout_config_parsing() {
        let toml_content = r#"
[task.orchestrate]
phase_timeout_minutes = 45
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.phase_timeout_minutes, 45);
    }

    #[test]
    fn test_phase_timeout_zero_disables() {
        // Verify that phase_timeout_minutes = 0 is parsed correctly
        // (downstream code interprets 0 as disabled)
        let toml_content = r#"
[task.orchestrate]
phase_timeout_minutes = 0
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.task.orchestrate.phase_timeout_minutes, 0,
            "phase_timeout_minutes=0 should be parsed as 0 (disabled)"
        );
    }

    #[test]
    fn test_git_timeout_config_parsing() {
        let toml_content = r#"
[task.orchestrate]
git_timeout_secs = 180
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.git_timeout_secs, 180);
    }

    #[test]
    fn test_setup_timeout_config_parsing() {
        let toml_content = r#"
[task.orchestrate]
setup_timeout_secs = 600
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.setup_timeout_secs, 600);
    }
}
