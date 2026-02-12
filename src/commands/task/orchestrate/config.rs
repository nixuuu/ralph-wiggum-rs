use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32};
use std::time::Duration;

use crate::shared::file_config::FileConfig;

// ── Input flags from TUI input thread ───────────────────────────────────

/// Atomic flags shared with the keyboard input thread.
pub(super) struct InputFlags {
    pub(super) shutdown: Arc<AtomicBool>,
    pub(super) graceful_shutdown: Arc<AtomicBool>,
    pub(super) resize_flag: Arc<AtomicBool>,
    pub(super) focused_worker: Arc<AtomicU32>,
    pub(super) scroll_delta: Arc<AtomicI32>,
    pub(super) render_notify: Arc<tokio::sync::Notify>,
    /// Toggle for task preview overlay (activated with 'p' key).
    /// Used by dashboard render to show task details instead of worker grid.
    pub(super) show_task_preview: Arc<AtomicBool>,
    pub(super) reload_requested: Arc<AtomicBool>,
    pub(super) quit_state: Arc<AtomicU8>,
    /// Flag indicating that all tasks have completed.
    /// When set, orchestrator enters idle state waiting for user quit confirmation.
    pub(super) completed: Arc<AtomicBool>,
}

// ── Resolved config ─────────────────────────────────────────────────────

/// Configuration resolved from CLI args + file config for orchestration.
pub struct ResolvedConfig {
    pub workers: u32,
    pub max_retries: u32,
    pub model: Option<String>,
    pub worktree_prefix: Option<String>,
    /// Verbosity flag — parsed from CLI but not actively used in orchestrator logic.
    #[allow(dead_code)] // CLI flag: parsed from args but not currently used in orchestrator
    pub verbose: bool,
    pub resume: bool,
    pub dry_run: bool,
    pub no_merge: bool,
    pub max_cost: Option<f64>,
    pub timeout: Option<Duration>,
    /// Task filter — parsed from CLI but not actively used in orchestrator logic.
    #[allow(dead_code)] // CLI flag: parsed from args but not currently used in orchestrator
    pub task_filter: Option<Vec<String>>,
    /// Model to use for merge conflict resolution (fallback: "opus")
    pub conflict_resolution_model: String,
    /// How often (seconds) the watchdog checks for panicked/stuck workers.
    pub watchdog_interval_secs: u32,
    /// Per-phase timeout for worker Claude processes. None = no timeout.
    pub phase_timeout: Option<Duration>,
    /// Timeout for git commands (add, commit, status, etc).
    pub git_timeout: Duration,
    /// Timeout for setup commands.
    pub setup_timeout: Duration,
    /// Merge task timeout (including AI conflict resolution). None = no timeout.
    pub merge_timeout: Option<Duration>,
}

impl ResolvedConfig {
    /// Build resolved config from CLI args + file config.
    ///
    /// Priority: CLI flags > .ralph.toml orchestrate section > hardcoded defaults.
    pub fn from_args(
        cli: &crate::commands::task::args::OrchestrateArgs,
        file_config: &FileConfig,
    ) -> Self {
        let orch_cfg = &file_config.task.orchestrate;

        let workers = cli.workers.unwrap_or(orch_cfg.workers);
        let max_retries = cli.max_retries.unwrap_or(orch_cfg.max_retries);
        let model = cli.model.clone().or_else(|| orch_cfg.default_model.clone());
        let worktree_prefix = cli
            .worktree_prefix
            .clone()
            .or_else(|| orch_cfg.worktree_prefix.clone());
        let timeout = cli.timeout.as_deref().and_then(parse_duration);
        let task_filter = cli
            .tasks
            .as_ref()
            .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
        let conflict_resolution_model = cli
            .conflict_model
            .clone()
            .or_else(|| orch_cfg.conflict_resolution_model.clone())
            .unwrap_or_else(|| "opus".to_string());
        let conflict_resolution_model =
            crate::shared::tasks::resolve_model_alias(&conflict_resolution_model);

        let phase_timeout_minutes = orch_cfg.phase_timeout_minutes;
        let phase_timeout = if phase_timeout_minutes == 0 {
            None
        } else {
            Some(Duration::from_secs(phase_timeout_minutes as u64 * 60))
        };

        let git_timeout = Duration::from_secs(orch_cfg.git_timeout_secs as u64);
        let setup_timeout = Duration::from_secs(orch_cfg.setup_timeout_secs as u64);

        let merge_timeout_minutes = orch_cfg.merge_timeout_minutes;
        let merge_timeout = if merge_timeout_minutes == 0 {
            None
        } else {
            Some(Duration::from_secs(merge_timeout_minutes as u64 * 60))
        };

        Self {
            workers,
            max_retries,
            model,
            worktree_prefix,
            verbose: cli.verbose,
            resume: cli.resume,
            dry_run: cli.dry_run,
            no_merge: cli.no_merge,
            max_cost: cli.max_cost,
            timeout,
            task_filter,
            conflict_resolution_model,
            watchdog_interval_secs: orch_cfg.watchdog_interval_secs,
            phase_timeout,
            git_timeout,
            setup_timeout,
            merge_timeout,
        }
    }
}

/// Parse a human-readable duration string like "2h", "30m", "45s", "1h30m".
fn parse_duration(s: &str) -> Option<Duration> {
    let mut total_secs: u64 = 0;
    let mut current_num = String::new();

    for ch in s.chars() {
        if ch.is_ascii_digit() {
            current_num.push(ch);
        } else {
            let n: u64 = current_num.parse().ok()?;
            current_num.clear();
            match ch {
                'h' => total_secs += n * 3600,
                'm' => total_secs += n * 60,
                's' => total_secs += n,
                _ => return None,
            }
        }
    }

    if total_secs > 0 {
        Some(Duration::from_secs(total_secs))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create default OrchestrateArgs for testing.
    fn default_orchestrate_args() -> crate::commands::task::args::OrchestrateArgs {
        crate::commands::task::args::OrchestrateArgs {
            workers: None,
            model: None,
            max_retries: None,
            verbose: false,
            resume: false,
            dry_run: false,
            worktree_prefix: None,
            no_merge: false,
            max_cost: None,
            timeout: None,
            tasks: None,
            conflict_model: None,
        }
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("30m"), Some(Duration::from_secs(1800)));
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("45s"), Some(Duration::from_secs(45)));
    }

    #[test]
    fn test_parse_duration_combined() {
        assert_eq!(
            parse_duration("1h30m"),
            Some(Duration::from_secs(3600 + 1800))
        );
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("abc"), None);
    }

    #[test]
    fn test_resolved_config_from_args() {
        let cli_args = crate::commands::task::args::OrchestrateArgs {
            workers: Some(4),
            model: Some("cli-model".to_string()),
            verbose: true,
            max_cost: Some(5.0),
            timeout: Some("1h".to_string()),
            tasks: Some("T01,T03".to_string()),
            ..default_orchestrate_args()
        };

        let file_config = FileConfig::default();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        assert_eq!(config.workers, 4);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.model.as_deref(), Some("cli-model"));
        assert!(config.verbose);
        assert_eq!(config.max_cost, Some(5.0));
        assert_eq!(config.timeout, Some(Duration::from_secs(3600)));
        assert_eq!(
            config.task_filter,
            Some(vec!["T01".to_string(), "T03".to_string()])
        );
        // Default "opus" should be resolved to full ID
        assert_eq!(config.conflict_resolution_model, "claude-opus-4-6");
    }

    #[test]
    fn test_conflict_resolution_model_cli_override() {
        let cli_args = crate::commands::task::args::OrchestrateArgs {
            conflict_model: Some("claude-opus-4-6".to_string()),
            ..default_orchestrate_args()
        };

        let file_config = FileConfig::default();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        assert_eq!(config.conflict_resolution_model, "claude-opus-4-6");
    }

    #[test]
    fn test_conflict_resolution_model_config_fallback() {
        let cli_args = default_orchestrate_args();

        let toml_content = r#"
[task.orchestrate]
conflict_resolution_model = "sonnet"
"#;
        let file_config: FileConfig = toml::from_str(toml_content).unwrap();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        // Config file value should be resolved to full ID
        assert_eq!(
            config.conflict_resolution_model,
            "claude-sonnet-4-5-20250929"
        );
    }

    #[test]
    fn test_conflict_resolution_model_hardcoded_fallback() {
        let cli_args = default_orchestrate_args();

        let file_config = FileConfig::default();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        // Default "opus" should be resolved to full ID
        assert_eq!(config.conflict_resolution_model, "claude-opus-4-6");
    }

    #[test]
    fn test_conflict_resolution_model_cli_alias_resolution() {
        let cli_args = crate::commands::task::args::OrchestrateArgs {
            conflict_model: Some("sonnet".to_string()),
            ..default_orchestrate_args()
        };

        let file_config = FileConfig::default();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        assert_eq!(
            config.conflict_resolution_model,
            "claude-sonnet-4-5-20250929"
        );
    }

    #[test]
    fn test_conflict_resolution_model_config_alias_resolution() {
        let cli_args = default_orchestrate_args();

        let toml_content = r#"
[task.orchestrate]
conflict_resolution_model = "haiku"
"#;
        let file_config: FileConfig = toml::from_str(toml_content).unwrap();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        assert_eq!(
            config.conflict_resolution_model,
            "claude-haiku-4-5-20251001"
        );
    }

    #[test]
    fn test_conflict_resolution_model_default_opus_alias_resolution() {
        let cli_args = default_orchestrate_args();

        let file_config = FileConfig::default();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        // Default "opus" should be resolved to full ID
        assert_eq!(config.conflict_resolution_model, "claude-opus-4-6");
    }

    #[test]
    fn test_conflict_resolution_model_full_id_passthrough() {
        let cli_args = crate::commands::task::args::OrchestrateArgs {
            conflict_model: Some("claude-opus-4-6".to_string()),
            ..default_orchestrate_args()
        };

        let file_config = FileConfig::default();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        // Full model ID should pass through unchanged
        assert_eq!(config.conflict_resolution_model, "claude-opus-4-6");
    }

    #[test]
    fn test_conflict_resolution_model_cli_overrides_config() {
        let cli_args = crate::commands::task::args::OrchestrateArgs {
            conflict_model: Some("sonnet".to_string()),
            ..default_orchestrate_args()
        };

        let toml_content = r#"
[task.orchestrate]
conflict_resolution_model = "haiku"
"#;
        let file_config: FileConfig = toml::from_str(toml_content).unwrap();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        // CLI flag should take precedence over config file (sonnet not haiku)
        assert_eq!(
            config.conflict_resolution_model,
            "claude-sonnet-4-5-20250929"
        );
    }

    // --- Phase timeout tests ---

    #[test]
    fn test_phase_timeout_default_30_minutes() {
        let cli_args = default_orchestrate_args();
        let file_config = FileConfig::default();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        assert_eq!(config.phase_timeout, Some(Duration::from_secs(30 * 60)));
    }

    #[test]
    fn test_phase_timeout_custom_value() {
        let cli_args = default_orchestrate_args();
        let toml_content = r#"
[task.orchestrate]
phase_timeout_minutes = 60
"#;
        let file_config: FileConfig = toml::from_str(toml_content).unwrap();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        assert_eq!(config.phase_timeout, Some(Duration::from_secs(60 * 60)));
    }

    #[test]
    fn test_phase_timeout_zero_disables() {
        let cli_args = default_orchestrate_args();
        let toml_content = r#"
[task.orchestrate]
phase_timeout_minutes = 0
"#;
        let file_config: FileConfig = toml::from_str(toml_content).unwrap();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        assert_eq!(config.phase_timeout, None);
    }

    // --- Merge timeout tests ---

    #[test]
    fn test_merge_timeout_default_15_minutes() {
        let cli_args = default_orchestrate_args();
        let file_config = FileConfig::default();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        assert_eq!(config.merge_timeout, Some(Duration::from_secs(15 * 60)));
    }

    #[test]
    fn test_merge_timeout_custom_value() {
        let cli_args = default_orchestrate_args();
        let toml_content = r#"
[task.orchestrate]
merge_timeout_minutes = 30
"#;
        let file_config: FileConfig = toml::from_str(toml_content).unwrap();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        assert_eq!(config.merge_timeout, Some(Duration::from_secs(30 * 60)));
    }

    #[test]
    fn test_merge_timeout_zero_disables() {
        let cli_args = default_orchestrate_args();
        let toml_content = r#"
[task.orchestrate]
merge_timeout_minutes = 0
"#;
        let file_config: FileConfig = toml::from_str(toml_content).unwrap();
        let config = ResolvedConfig::from_args(&cli_args, &file_config);

        assert_eq!(config.merge_timeout, None);
    }
}
