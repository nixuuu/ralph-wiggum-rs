//! Configuration management commands: show, path, and init.

use crate::cli::ConfigCommands;
use crate::shared::error::Result;
use crate::shared::file_config;
use crate::shared::global_config;
use std::path::PathBuf;

/// Execute config subcommand.
pub fn execute(command: ConfigCommands) -> Result<()> {
    match command {
        ConfigCommands::Show => show(),
        ConfigCommands::Path => path(),
        ConfigCommands::Init => init(),
    }
}

/// Returns the local config path (.ralph.toml in current directory).
fn local_config_path() -> Result<PathBuf> {
    std::env::current_dir()
        .map(|cwd| cwd.join(".ralph.toml"))
        .map_err(|e| {
            crate::shared::error::RalphError::Config(format!(
                "Failed to get current directory: {}",
                e
            ))
        })
}

/// Show merged configuration in TOML format.
fn show() -> Result<()> {
    let local_path = local_config_path()?;
    let config = file_config::load_merged_config(&local_path)?;

    let toml_str = toml::to_string_pretty(&config).map_err(|e| {
        crate::shared::error::RalphError::Config(format!(
            "Failed to serialize config to TOML: {}",
            e
        ))
    })?;

    println!("{}", toml_str);
    Ok(())
}

/// Show configuration file paths (global and local).
fn path() -> Result<()> {
    let global_path = global_config::resolve_global_config_path();
    let local_path = local_config_path()?;

    println!("Global config: {}", global_path.display());
    println!("Local config:  {}", local_path.display());
    Ok(())
}

/// Initialize global configuration with defaults.
fn init() -> Result<()> {
    let config_dir = global_config::ensure_global_config_dir()?;
    let config_path = config_dir.join("config.toml");

    // Check if config already exists
    if config_path.exists() {
        println!("Config already exists: {}", config_path.display());
        println!("To update it, please edit the file manually.");
        return Ok(());
    }

    // Create default config content
    let default_config = r#"# Ralph-Wiggum Configuration File
# This file is automatically loaded from ~/.config/ralph/config.toml

[prompt]
# Optional prefix to prepend before each prompt
# prefix = "You are a coding expert. "

# Optional suffix to append after each prompt
# suffix = " Please be concise."

[ui]
# Use Nerd Font icons (default: true, set to false for ASCII)
nerd_font = true

[logging]
# Directory for diagnostic logs (default: .ralph/logs)
log_dir = ".ralph/logs"

# Max number of log files to keep (default: 10, 0 = unlimited)
max_log_files = 10

[task]
# Task progress tracking file (default: .ralph/progress.md)
progress_file = ".ralph/progress.md"

# Task definitions file (default: .ralph/tasks.yml)
tasks_file = ".ralph/tasks.yml"

# Optional output directory for task execution artifacts
# output_dir = ".ralph/output"

# Optional default Claude model for task execution
# default_model = "opus"

[task.orchestrate]
# Number of parallel workers (default: 2)
workers = 2

# Max retries per task before marking as blocked (default: 3)
max_retries = 3

# Optional default Claude model for workers
# default_model = "opus"

# Max duration (in minutes) for a single worker phase (default: 30, 0 = unlimited)
phase_timeout_minutes = 30

# Git command timeout in seconds (default: 120)
git_timeout_secs = 120

# Setup command timeout in seconds (default: 300)
setup_timeout_secs = 300

# Merge task timeout in minutes (default: 15, 0 = unlimited)
merge_timeout_minutes = 15
"#;

    std::fs::write(&config_path, default_config).map_err(|e| {
        crate::shared::error::RalphError::Config(format!(
            "Failed to write config file at {}: {}",
            config_path.display(),
            e
        ))
    })?;

    println!("Global config initialized: {}", config_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Mutex ensures test serialization when modifying environment variables.
    // env::set_var/remove_var are unsafe in Rust 2024 — wrap with unsafe blocks.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_path_command() {
        let result = path();
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_command_with_default_config() {
        // show() should work even without local .ralph.toml
        // load_merged_config returns default configuration
        let result = show();
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_init_creates_config_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();

        // Create a temporary directory and set XDG_CONFIG_HOME to point to it
        let tmp_dir = std::env::temp_dir().join("ralph-config-init-test");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir).expect("Failed to create temp dir");

        // SAFETY: test is serialized by ENV_LOCK + current_thread flavor, no concurrency
        unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp_dir.to_str().unwrap()) };

        let result = init();
        assert!(result.is_ok(), "init() should succeed");

        // Verify the config file was actually created
        let config_path = tmp_dir.join("ralph").join("config.toml");
        assert!(
            config_path.exists(),
            "Config file should exist at {}",
            config_path.display()
        );
        assert!(config_path.is_file(), "Config path should be a file");

        // Restore environment and cleanup
        unsafe {
            match original_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_init_does_not_overwrite_existing_config() {
        let _lock = ENV_LOCK.lock().unwrap();
        let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();

        // Create temporary directory with existing config
        let tmp_dir = std::env::temp_dir().join("ralph-config-no-overwrite-test");
        let ralph_dir = tmp_dir.join("ralph");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&ralph_dir).expect("Failed to create temp dir");

        let config_path = ralph_dir.join("config.toml");
        let original_content = "[test]\nvalue = \"original\"\n";
        std::fs::write(&config_path, original_content).expect("Failed to write original config");

        // Set XDG_CONFIG_HOME to tmp_dir
        // SAFETY: test is serialized by ENV_LOCK + current_thread flavor, no concurrency
        unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp_dir.to_str().unwrap()) };

        let result = init();
        assert!(result.is_ok(), "init() should succeed when config exists");

        // Verify the original content was not overwritten
        let current_content = std::fs::read_to_string(&config_path).expect("Failed to read config");
        assert_eq!(
            current_content, original_content,
            "Original config should not be overwritten"
        );

        // Restore environment and cleanup
        unsafe {
            match original_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
