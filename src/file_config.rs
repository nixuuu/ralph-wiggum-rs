use serde::Deserialize;
use std::path::Path;

use crate::error::{RalphError, Result};

/// Configuration loaded from .ralph.toml file
#[derive(Debug, Default, Deserialize)]
pub struct FileConfig {
    #[serde(default)]
    pub prompt: PromptConfig,
    #[serde(default)]
    pub ui: UiConfig,
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
}
