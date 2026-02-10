use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::shared::error::{RalphError, Result};

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
#[allow(dead_code)]
pub struct TaskConfig {
    #[serde(default = "default_progress_file")]
    pub progress_file: PathBuf,
    #[serde(default = "default_system_prompt_file")]
    pub system_prompt_file: PathBuf,
    #[serde(default = "default_current_task_file")]
    pub current_task_file: PathBuf,
    #[serde(default)]
    pub output_dir: Option<PathBuf>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub auto_continue: bool,
    #[serde(default = "default_true")]
    pub adaptive_iterations: bool,
    #[serde(default)]
    pub files: TaskFilesConfig,
    /// Orchestration settings (`[task.orchestrate]` in .ralph.toml)
    #[serde(default)]
    pub orchestrate: OrchestrateConfig,
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            progress_file: default_progress_file(),
            system_prompt_file: default_system_prompt_file(),
            current_task_file: default_current_task_file(),
            output_dir: None,
            default_model: None,
            auto_continue: false,
            adaptive_iterations: true,
            files: TaskFilesConfig::default(),
            orchestrate: OrchestrateConfig::default(),
        }
    }
}

/// Configuration for task orchestration (`[task.orchestrate]` in .ralph.toml)
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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
    /// Shell commands to run for verification phase (e.g. "cargo test && cargo clippy")
    #[serde(default)]
    pub verify_commands: Option<String>,
}

impl Default for OrchestrateConfig {
    fn default() -> Self {
        Self {
            workers: default_workers(),
            max_retries: default_max_retries(),
            worktree_prefix: None,
            default_model: None,
            verify_commands: None,
        }
    }
}

fn default_workers() -> u32 {
    2
}

fn default_max_retries() -> u32 {
    3
}

fn default_progress_file() -> PathBuf {
    PathBuf::from("PROGRESS.md")
}

fn default_system_prompt_file() -> PathBuf {
    PathBuf::from("SYSTEM_PROMPT.md")
}

fn default_current_task_file() -> PathBuf {
    PathBuf::from("CURRENT_TASK.md")
}

/// Paths for auxiliary tracking files
#[derive(Debug, Deserialize)]
pub struct TaskFilesConfig {
    #[serde(default = "default_changenotes")]
    pub changenotes: PathBuf,
    #[serde(default = "default_issues")]
    pub issues: PathBuf,
    #[serde(default = "default_questions")]
    pub questions: PathBuf,
}

impl Default for TaskFilesConfig {
    fn default() -> Self {
        Self {
            changenotes: default_changenotes(),
            issues: default_issues(),
            questions: default_questions(),
        }
    }
}

fn default_changenotes() -> PathBuf {
    PathBuf::from("CHANGENOTES.md")
}

fn default_issues() -> PathBuf {
    PathBuf::from("IMPLEMENTATION_ISSUES.md")
}

fn default_questions() -> PathBuf {
    PathBuf::from("OPEN_QUESTIONS.md")
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
            config.task.system_prompt_file,
            std::path::PathBuf::from("SYSTEM_PROMPT.md")
        );
        assert_eq!(
            config.task.current_task_file,
            std::path::PathBuf::from("CURRENT_TASK.md")
        );
        assert!(config.task.output_dir.is_none());
        assert!(config.task.default_model.is_none());
        assert!(!config.task.auto_continue);
        assert!(config.task.adaptive_iterations);
        assert_eq!(
            config.task.files.changenotes,
            std::path::PathBuf::from("CHANGENOTES.md")
        );
        assert_eq!(
            config.task.files.issues,
            std::path::PathBuf::from("IMPLEMENTATION_ISSUES.md")
        );
        assert_eq!(
            config.task.files.questions,
            std::path::PathBuf::from("OPEN_QUESTIONS.md")
        );
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
        assert!(config.task.adaptive_iterations);
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
            config.task.system_prompt_file,
            std::path::PathBuf::from("SYSTEM_PROMPT.md")
        );
    }

    #[test]
    fn test_task_config_full() {
        let toml_content = r#"
[task]
progress_file = "PROG.md"
system_prompt_file = "SP.md"
current_task_file = "CT.md"
output_dir = "output"
default_model = "claude-opus-4-6"
auto_continue = true
adaptive_iterations = false

[task.files]
changenotes = "CHANGES.md"
issues = "ISSUES.md"
questions = "QS.md"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.task.progress_file,
            std::path::PathBuf::from("PROG.md")
        );
        assert_eq!(
            config.task.system_prompt_file,
            std::path::PathBuf::from("SP.md")
        );
        assert_eq!(
            config.task.current_task_file,
            std::path::PathBuf::from("CT.md")
        );
        assert_eq!(
            config.task.output_dir,
            Some(std::path::PathBuf::from("output"))
        );
        assert_eq!(
            config.task.default_model.as_deref(),
            Some("claude-opus-4-6")
        );
        assert!(config.task.auto_continue);
        assert!(!config.task.adaptive_iterations);
        assert_eq!(
            config.task.files.changenotes,
            std::path::PathBuf::from("CHANGES.md")
        );
        assert_eq!(
            config.task.files.issues,
            std::path::PathBuf::from("ISSUES.md")
        );
        assert_eq!(
            config.task.files.questions,
            std::path::PathBuf::from("QS.md")
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
        assert!(config.task.orchestrate.verify_commands.is_none());
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
    fn test_orchestrate_config_verify_commands() {
        let toml_content = r#"
[task.orchestrate]
verify_commands = "cargo test && cargo clippy --all-targets -- -D warnings"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.task.orchestrate.verify_commands.as_deref(),
            Some("cargo test && cargo clippy --all-targets -- -D warnings")
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
}
