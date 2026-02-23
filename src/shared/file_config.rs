use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::shared::error::{RalphError, Result};
use crate::tui::keybindings::KeybindingsConfig;

// ── Merge helpers ──────────────────────────────────────────────────────
// Semantyka: overlay nadpisuje base tylko gdy wartość jest inna niż default.

/// Scalar: użyj overlay jeśli różni się od default, inaczej base.
fn merge_scalar<T: PartialEq>(base: T, overlay: T, default: T) -> T {
    if overlay != default { overlay } else { base }
}

/// Option: overlay Some > base Some > None.
fn merge_option<T>(base: Option<T>, overlay: Option<T>) -> Option<T> {
    overlay.or(base)
}

/// Vec: overlay zastępuje base w całości jeśli niepusty.
fn merge_vec<T>(base: Vec<T>, overlay: Vec<T>) -> Vec<T> {
    if overlay.is_empty() { base } else { overlay }
}

// ── Includes support ────────────────────────────────────────────────────────

/// Wyciąga listę includes z sparsowanej wartości TOML.
fn extract_includes(value: &toml::Value) -> Vec<String> {
    value
        .get("includes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Sprawdza czy wartość TOML zawiera niepuste pole `includes`.
fn has_includes(value: &toml::Value) -> bool {
    value
        .get("includes")
        .and_then(|v| v.as_array())
        .map(|arr| !arr.is_empty())
        .unwrap_or(false)
}

/// Rozwiązuje ścieżkę do pliku include względem katalogu bazowego.
/// Ścieżki bezwzględne są zwracane bez zmian.
fn resolve_include_path(config_dir: &Path, include_name: &str) -> PathBuf {
    let p = Path::new(include_name);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        config_dir.join(p)
    }
}

/// Scala wartość `override_val` w `base` (last wins).
///
/// Dla tablic TOML: klucze z override są rekurencyjnie mergowane do base.
/// Dla pozostałych typów: override_val zastępuje base.
fn merge_toml(base: &mut toml::Value, override_val: toml::Value) {
    match override_val {
        toml::Value::Table(override_map) => {
            if let toml::Value::Table(base_map) = base {
                for (key, val) in override_map {
                    if let Some(base_val) = base_map.get_mut(&key) {
                        merge_toml(base_val, val);
                    } else {
                        base_map.insert(key, val);
                    }
                }
            } else {
                // base nie jest tabelą — zastąp całkowicie
                *base = toml::Value::Table(override_map);
            }
        }
        other => {
            *base = other;
        }
    }
}


/// A setup command to run after creating a worktree.
///
/// Can be a simple string or an object with `run` and optional `name`.
/// Supports template variables: `{ROOT_DIR}`, `{WORKTREE_DIR}`, `{TASK_ID}`, `{WORKER_ID}`.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
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
            VerifyCommand::Detailed { name: Some(n), .. } => n,
            VerifyCommand::Detailed { command, .. } => command,
        }
    }
}

/// Configuration loaded from .ralph.toml file
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct FileConfig {
    /// Lista plików konfiguracyjnych do zainkludowania.
    /// Ścieżki relatywne są rozwiązywane względem katalogu zawierającego config.toml.
    /// Pliki są mergowane w kolejności (last wins). Max depth = 1 (bez rekursji).
    #[serde(default)]
    #[allow(dead_code)] // Odczytywane przez load_from_path via TOML value, nie przez Rust code
    pub includes: Vec<String>,
    #[serde(default)]
    pub prompt: PromptConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    #[allow(dead_code)] // Will be consumed by TUI rendering modules
    pub tui: TuiConfig,
    #[serde(default)]
    pub task: TaskConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    /// Konfiguracja skrótów klawiszowych (`[keybindings]` w .ralph.toml)
    #[serde(default)]
    #[allow(dead_code)] // Will be consumed by TUI event handlers
    pub keybindings: KeybindingsConfig,
}

/// UI configuration
#[derive(Debug, Deserialize, Serialize)]
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

impl UiConfig {
    fn merge(base: Self, overlay: Self) -> Self {
        let d = Self::default();
        Self {
            nerd_font: merge_scalar(base.nerd_font, overlay.nerd_font, d.nerd_font),
        }
    }
}

/// TUI configuration for terminal user interface.
/// Fields will be consumed by TUI rendering modules in future tasks.
#[derive(Debug, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct TuiConfig {
    /// Output buffer size in characters (default: 5000)
    #[serde(default = "default_output_buffer_size")]
    pub output_buffer_size: u32,
    /// Sidebar width in characters (default: 25)
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: u16,
    /// Show sidebar (default: true)
    #[serde(default = "default_true")]
    pub sidebar_visible: bool,
    /// Theme name (default: "default")
    #[serde(default = "default_theme")]
    pub theme: String,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            output_buffer_size: default_output_buffer_size(),
            sidebar_width: default_sidebar_width(),
            sidebar_visible: default_true(),
            theme: default_theme(),
        }
    }
}

impl TuiConfig {
    fn merge(base: Self, overlay: Self) -> Self {
        let d = Self::default();
        Self {
            output_buffer_size: merge_scalar(
                base.output_buffer_size,
                overlay.output_buffer_size,
                d.output_buffer_size,
            ),
            sidebar_width: merge_scalar(base.sidebar_width, overlay.sidebar_width, d.sidebar_width),
            sidebar_visible: merge_scalar(
                base.sidebar_visible,
                overlay.sidebar_visible,
                d.sidebar_visible,
            ),
            theme: merge_scalar(base.theme, overlay.theme, d.theme),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_output_buffer_size() -> u32 {
    5000
}

fn default_sidebar_width() -> u16 {
    25
}

fn default_theme() -> String {
    "default".to_string()
}

/// Prompt configuration with optional prefix and suffix
#[derive(Debug, Default, Deserialize, Serialize)]
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

impl PromptConfig {
    fn merge(base: Self, overlay: Self) -> Self {
        Self {
            prefix: merge_option(base.prefix, overlay.prefix),
            suffix: merge_option(base.suffix, overlay.suffix),
            system: merge_option(base.system, overlay.system),
        }
    }
}

/// Logging configuration for diagnostic logs
#[derive(Debug, Deserialize, Serialize)]
pub struct LoggingConfig {
    /// Directory for diagnostic logs (default: .ralph/logs)
    #[serde(default = "default_log_dir")]
    pub log_dir: PathBuf,
    /// Max number of log files to keep (default: 10, 0 = unlimited)
    #[serde(default = "default_max_log_files")]
    pub max_log_files: u32,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            log_dir: default_log_dir(),
            max_log_files: default_max_log_files(),
        }
    }
}

impl LoggingConfig {
    fn merge(base: Self, overlay: Self) -> Self {
        let d = Self::default();
        Self {
            log_dir: merge_scalar(base.log_dir, overlay.log_dir, d.log_dir),
            max_log_files: merge_scalar(base.max_log_files, overlay.max_log_files, d.max_log_files),
        }
    }
}

/// Task management configuration
#[derive(Debug, Deserialize, Serialize)]
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

impl TaskConfig {
    fn merge(base: Self, overlay: Self) -> Self {
        let d = Self::default();
        Self {
            progress_file: merge_scalar(base.progress_file, overlay.progress_file, d.progress_file),
            tasks_file: merge_scalar(base.tasks_file, overlay.tasks_file, d.tasks_file),
            output_dir: merge_option(base.output_dir, overlay.output_dir),
            default_model: merge_option(base.default_model, overlay.default_model),
            orchestrate: OrchestrateConfig::merge(base.orchestrate, overlay.orchestrate),
        }
    }
}

/// A verification profile defines a set of paths to monitor with associated
/// verification and setup commands.
///
/// Used in orchestration to run targeted checks only when specific paths change.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VerifyProfile {
    /// Unique profile name
    pub name: String,
    /// Optional profile description
    #[serde(default)]
    #[allow(dead_code)]
    pub description: Option<String>,
    /// Path patterns to monitor (glob-compatible)
    #[serde(default)]
    pub paths: Vec<String>,
    /// Optional working directory for commands
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Verification commands to run when paths change
    #[serde(default)]
    pub verify_commands: Vec<VerifyCommand>,
    /// Setup commands to run before verification
    #[serde(default)]
    #[allow(dead_code)]
    pub setup_commands: Vec<SetupCommand>,
}

/// Configuration for task orchestration (`[task.orchestrate]` in .ralph.toml)
#[derive(Debug, Deserialize, Serialize)]
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
    /// Claude model to use for code review (default: "opus")
    #[serde(default)]
    pub review_model: Option<String>,
    /// Verification profiles - targeted checks for specific path patterns
    #[serde(default)]
    pub profiles: Vec<VerifyProfile>,
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
            review_model: None,
            profiles: Vec::new(),
        }
    }
}

impl OrchestrateConfig {
    fn merge(base: Self, overlay: Self) -> Self {
        let d = Self::default();
        Self {
            workers: merge_scalar(base.workers, overlay.workers, d.workers),
            max_retries: merge_scalar(base.max_retries, overlay.max_retries, d.max_retries),
            worktree_prefix: merge_option(base.worktree_prefix, overlay.worktree_prefix),
            default_model: merge_option(base.default_model, overlay.default_model),
            verify_commands: merge_vec(base.verify_commands, overlay.verify_commands),
            setup_commands: merge_vec(base.setup_commands, overlay.setup_commands),
            conflict_resolution_model: merge_option(
                base.conflict_resolution_model,
                overlay.conflict_resolution_model,
            ),
            watchdog_interval_secs: merge_scalar(
                base.watchdog_interval_secs,
                overlay.watchdog_interval_secs,
                d.watchdog_interval_secs,
            ),
            phase_timeout_minutes: merge_scalar(
                base.phase_timeout_minutes,
                overlay.phase_timeout_minutes,
                d.phase_timeout_minutes,
            ),
            git_timeout_secs: merge_scalar(
                base.git_timeout_secs,
                overlay.git_timeout_secs,
                d.git_timeout_secs,
            ),
            setup_timeout_secs: merge_scalar(
                base.setup_timeout_secs,
                overlay.setup_timeout_secs,
                d.setup_timeout_secs,
            ),
            merge_timeout_minutes: merge_scalar(
                base.merge_timeout_minutes,
                overlay.merge_timeout_minutes,
                d.merge_timeout_minutes,
            ),
            review_model: merge_option(base.review_model, overlay.review_model),
            profiles: merge_vec(base.profiles, overlay.profiles),
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

fn default_log_dir() -> PathBuf {
    PathBuf::from(".ralph/logs")
}

fn default_max_log_files() -> u32 {
    10
}

impl FileConfig {
    /// Load configuration from a specific path.
    /// Returns default config if file doesn't exist.
    ///
    /// Obsługuje pole `includes`: lista plików TOML do zainkludowania.
    /// - Ścieżki relatywne rozwiązywane względem katalogu `path`.
    /// - Mergowanie: base → include1 → include2 → ... (last wins).
    /// - Max depth = 1: includes w include files są ignorowane (z ostrzeżeniem).
    /// - Brakujący plik include → ostrzeżenie na stderr, nie błąd.
    pub fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| RalphError::Config(format!("Failed to read {}: {}", path.display(), e)))?;

        let mut base_value: toml::Value = toml::from_str(&content).map_err(|e| {
            RalphError::Config(format!("Failed to parse {}: {}", path.display(), e))
        })?;

        // Wyciągnij listę includes zanim przystąpimy do mergowania
        let includes = extract_includes(&base_value);

        if !includes.is_empty() {
            let config_dir = path.parent().unwrap_or(Path::new("."));
            Self::apply_includes(&mut base_value, config_dir, &includes);
        }

        // Serializuj z powrotem do TOML string i parsuj do FileConfig
        // (round-trip gwarantuje poprawną deserializację po merge)
        let merged_str = toml::to_string(&base_value)
            .map_err(|e| RalphError::Config(format!("Failed to serialize merged config: {}", e)))?;

        toml::from_str(&merged_str).map_err(|e| {
            RalphError::Config(format!(
                "Failed to parse merged config for {}: {}",
                path.display(),
                e
            ))
        })
    }

    /// Aplikuje include files do base wartości TOML (depth = 1).
    fn apply_includes(base: &mut toml::Value, config_dir: &Path, includes: &[String]) {
        for include_name in includes {
            let include_path = resolve_include_path(config_dir, include_name);

            if !include_path.exists() {
                eprintln!(
                    "[ralph] Warning: include file not found: {}",
                    include_path.display()
                );
                continue;
            }

            let content = match std::fs::read_to_string(&include_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!(
                        "[ralph] Warning: failed to read include {}: {}",
                        include_path.display(),
                        e
                    );
                    continue;
                }
            };

            let mut include_value: toml::Value = match toml::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "[ralph] Warning: failed to parse include {}: {}",
                        include_path.display(),
                        e
                    );
                    continue;
                }
            };

            // Ostrzeżenie o przekroczeniu max depth = 1
            if has_includes(&include_value) {
                eprintln!(
                    "[ralph] Warning: includes in {} are ignored (max depth = 1)",
                    include_path.display()
                );
            }

            // Usuń klucz `includes` z include_value przed merge,
            // żeby nie nadpisać oryginalnej listy includes z base config
            if let toml::Value::Table(ref mut map) = include_value {
                map.remove("includes");
            }

            // Merge: include wygrywa nad base
            merge_toml(base, include_value);
        }
    }

    /// Merge dwóch warstw konfiguracji: overlay nadpisuje base (non-default only).
    ///
    /// Semantyka:
    /// - Scalar fields: overlay zastępuje base jeśli overlay ≠ default
    /// - Option fields: overlay Some > base Some > None
    /// - Vec fields: overlay zastępuje base w całości jeśli niepusty
    pub fn merge(base: Self, overlay: Self) -> Self {
        Self {
            includes: merge_vec(base.includes, overlay.includes),
            prompt: PromptConfig::merge(base.prompt, overlay.prompt),
            ui: UiConfig::merge(base.ui, overlay.ui),
            tui: TuiConfig::merge(base.tui, overlay.tui),
            task: TaskConfig::merge(base.task, overlay.task),
            logging: LoggingConfig::merge(base.logging, overlay.logging),
        }
    }

    /// Merge dwóch warstw konfiguracji: overlay nadpisuje base (non-default only).
    ///
    /// Semantyka:
    /// - Scalar fields: overlay zastępuje base jeśli overlay ≠ default
    /// - Option fields: overlay Some > base Some > None
    /// - Vec fields: overlay zastępuje base w całości jeśli niepusty
    pub fn merge(base: Self, overlay: Self) -> Self {
        Self {
            prompt: PromptConfig::merge(base.prompt, overlay.prompt),
            ui: UiConfig::merge(base.ui, overlay.ui),
            tui: TuiConfig::merge(base.tui, overlay.tui),
            task: TaskConfig::merge(base.task, overlay.task),
            logging: LoggingConfig::merge(base.logging, overlay.logging),
            keybindings: KeybindingsConfig::merge(base.keybindings, overlay.keybindings),
        }
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

/// Ładuje konfigurację z trzech warstw: defaults → global → local.
///
/// Kolejność merge:
/// 1. `FileConfig::default()` — wartości domyślne
/// 2. Global config (`~/.config/ralph/config.toml`) — nadpisuje defaults
/// 3. Local config (`local_path`, zwykle `.ralph.toml`) — nadpisuje global
///
/// Brakujący plik (global lub local) nie powoduje błędu — używane są wartości
/// z niższej warstwy. Błąd zwracany jest tylko gdy plik istnieje ale ma
/// niepoprawny format TOML.
pub fn load_merged_config(local_path: &Path) -> Result<FileConfig> {
    let defaults = FileConfig::default();

    let global = match crate::shared::global_config::load_global_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            crate::diag_warn!("Global config error (using defaults): {}", e);
            FileConfig::default()
        }
    };
    let local = FileConfig::load_from_path(local_path)?;

    // defaults → global → local
    let merged = FileConfig::merge(defaults, global);
    Ok(FileConfig::merge(merged, local))
}

/// Formats a list of verification profiles into a human-readable string.
///
/// Generates a formatted text description of each profile including:
/// - Profile name and description
/// - Path patterns monitored
/// - Verification commands (using labels)
/// - Setup commands (using labels)
/// - Working directory (if specified)
///
/// # Arguments
/// * `profiles` - Slice of VerifyProfile to format
///
/// # Returns
/// Formatted string with profile information, or empty string if profiles is empty
pub fn format_profiles_info(profiles: &[VerifyProfile]) -> String {
    if profiles.is_empty() {
        return String::new();
    }

    let mut result = String::new();

    for (idx, profile) in profiles.iter().enumerate() {
        if idx > 0 {
            result.push('\n');
        }

        // Profile header with name
        result.push_str(&format!("- **{}**", profile.name));

        // Add description if present
        if let Some(desc) = &profile.description {
            result.push_str(&format!(": {}", desc));
        }
        result.push('\n');

        // Paths
        if !profile.paths.is_empty() {
            result.push_str("  paths: [");
            result.push_str(&profile.paths.join(", "));
            result.push_str("]\n");
        }

        // Verify commands (using labels)
        if !profile.verify_commands.is_empty() {
            result.push_str("  verify: [");
            let verify_labels: Vec<&str> = profile
                .verify_commands
                .iter()
                .map(|cmd| cmd.label())
                .collect();
            result.push_str(&verify_labels.join(", "));
            result.push_str("]\n");
        }

        // Setup commands (using labels)
        if !profile.setup_commands.is_empty() {
            result.push_str("  setup: [");
            let setup_labels: Vec<&str> = profile
                .setup_commands
                .iter()
                .map(|cmd| cmd.label())
                .collect();
            result.push_str(&setup_labels.join(", "));
            result.push_str("]\n");
        }

        // Working directory
        if let Some(wd) = &profile.working_dir {
            result.push_str(&format!("  working_dir: {}\n", wd));
        }
    }

    result
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
        assert!(
            config.task.orchestrate.verify_commands[0]
                .description()
                .is_none()
        );
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
        assert!(
            config.task.orchestrate.verify_commands[0]
                .description()
                .is_none()
        );
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
        assert!(
            config.task.orchestrate.verify_commands[0]
                .description()
                .is_none()
        );
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
        assert_eq!(config.task.orchestrate.verify_commands[0].command(), "");
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
        assert_eq!(config.task.orchestrate.verify_commands[0].command(), "   ");
        assert_eq!(config.task.orchestrate.verify_commands[1].command(), "\t\n");
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

    // --- Review model tests ---

    #[test]
    fn test_orchestrate_config_review_model() {
        let toml_content = r#"
[task.orchestrate]
review_model = "opus"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.task.orchestrate.review_model.as_deref(),
            Some("opus")
        );
    }

    #[test]
    fn test_orchestrate_config_review_model_default_none() {
        let config = FileConfig::default();
        assert!(config.task.orchestrate.review_model.is_none());
    }

    #[test]
    fn test_orchestrate_config_with_all_models() {
        let toml_content = r#"
[task.orchestrate]
workers = 4
max_retries = 5
worktree_prefix = "my-project"
default_model = "claude-sonnet-4-5-20250929"
conflict_resolution_model = "claude-opus-4-6"
review_model = "claude-opus-4-6"
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
        assert_eq!(
            config.task.orchestrate.review_model.as_deref(),
            Some("claude-opus-4-6")
        );
    }

    // --- Logging config tests ---

    #[test]
    fn test_logging_config_defaults() {
        let config = FileConfig::default();
        assert_eq!(
            config.logging.log_dir,
            std::path::PathBuf::from(".ralph/logs")
        );
        assert_eq!(config.logging.max_log_files, 10);
    }

    #[test]
    fn test_logging_config_backward_compat() {
        // Existing .ralph.toml without [logging] section should still work
        let toml_content = r#"
[prompt]
prefix = "test"

[ui]
nerd_font = false
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        // Logging config should have all defaults
        assert_eq!(
            config.logging.log_dir,
            std::path::PathBuf::from(".ralph/logs")
        );
        assert_eq!(config.logging.max_log_files, 10);
    }

    #[test]
    fn test_logging_config_custom() {
        let toml_content = r#"
[logging]
log_dir = "custom/logs"
max_log_files = 20
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.logging.log_dir,
            std::path::PathBuf::from("custom/logs")
        );
        assert_eq!(config.logging.max_log_files, 20);
    }

    #[test]
    fn test_logging_config_partial() {
        let toml_content = r#"
[logging]
log_dir = ".logs"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.logging.log_dir, std::path::PathBuf::from(".logs"));
        assert_eq!(config.logging.max_log_files, 10); // default
    }

    #[test]
    fn test_logging_config_unlimited_files() {
        let toml_content = r#"
[logging]
max_log_files = 0
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.logging.log_dir,
            std::path::PathBuf::from(".ralph/logs")
        ); // default
        assert_eq!(config.logging.max_log_files, 0);
    }

    #[test]
    fn test_logging_config_full() {
        let toml_content = r#"
[logging]
log_dir = "logs/diagnostics"
max_log_files = 15
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.logging.log_dir,
            std::path::PathBuf::from("logs/diagnostics")
        );
        assert_eq!(config.logging.max_log_files, 15);
    }

    // ── Task 53.2: Test max_retries=0 parsing and scheduler integration ──

    #[test]
    fn test_max_retries_zero_parsing() {
        // Test that max_retries=0 is parsed correctly from TOML
        let toml_content = r#"
[task.orchestrate]
max_retries = 0
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.task.orchestrate.max_retries, 0,
            "max_retries=0 should be parsed correctly"
        );
    }

    #[test]
    fn test_max_retries_zero_with_other_fields() {
        // Test max_retries=0 alongside other config fields
        let toml_content = r#"
[task.orchestrate]
workers = 4
max_retries = 0
worktree_prefix = "test"
verify_commands = ["cargo test"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.workers, 4);
        assert_eq!(
            config.task.orchestrate.max_retries, 0,
            "max_retries=0 should coexist with other fields"
        );
        assert_eq!(
            config.task.orchestrate.worktree_prefix.as_deref(),
            Some("test")
        );
        assert_eq!(config.task.orchestrate.verify_commands.len(), 1);
    }

    #[test]
    fn test_max_retries_zero_vs_default() {
        // Verify that max_retries=0 differs from the default value
        let default_config = FileConfig::default();
        assert_eq!(
            default_config.task.orchestrate.max_retries, 3,
            "Default max_retries should be 3"
        );

        let toml_content = r#"
[task.orchestrate]
max_retries = 0
"#;
        let zero_config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            zero_config.task.orchestrate.max_retries, 0,
            "Explicit max_retries=0 should override default"
        );
    }

    // --- VerifyProfile tests ---

    #[test]
    fn test_profiles_default_empty() {
        let config = FileConfig::default();
        assert!(config.task.orchestrate.profiles.is_empty());
    }

    #[test]
    fn test_profiles_backward_compat() {
        // Existing .ralph.toml without profiles should still work
        let toml_content = r#"
[task.orchestrate]
workers = 4
verify_commands = ["cargo test"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert!(config.task.orchestrate.profiles.is_empty());
        assert_eq!(config.task.orchestrate.verify_commands.len(), 1);
    }

    #[test]
    fn test_profiles_single_profile() {
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "frontend"
description = "Frontend tests"
paths = ["src/ui/**/*.rs", "assets/**/*"]
working_dir = "frontend"
verify_commands = ["npm test", "npm run lint"]
setup_commands = ["npm install"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.profiles.len(), 1);

        let profile = &config.task.orchestrate.profiles[0];
        assert_eq!(profile.name, "frontend");
        assert_eq!(profile.description.as_deref(), Some("Frontend tests"));
        assert_eq!(profile.paths, vec!["src/ui/**/*.rs", "assets/**/*"]);
        assert_eq!(profile.working_dir.as_deref(), Some("frontend"));
        assert_eq!(profile.verify_commands.len(), 2);
        assert_eq!(profile.setup_commands.len(), 1);
    }

    #[test]
    fn test_profiles_multiple_profiles() {
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "backend"
paths = ["src/api/**/*.rs"]
verify_commands = ["cargo test --package api"]

[[task.orchestrate.profiles]]
name = "database"
description = "Database migrations"
paths = ["migrations/**/*.sql"]
verify_commands = ["sqlx migrate run"]
setup_commands = ["sqlx database create"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.profiles.len(), 2);

        let backend = &config.task.orchestrate.profiles[0];
        assert_eq!(backend.name, "backend");
        assert!(backend.description.is_none());
        assert_eq!(backend.paths, vec!["src/api/**/*.rs"]);
        assert!(backend.working_dir.is_none());
        assert_eq!(backend.verify_commands.len(), 1);
        assert!(backend.setup_commands.is_empty());

        let database = &config.task.orchestrate.profiles[1];
        assert_eq!(database.name, "database");
        assert_eq!(database.description.as_deref(), Some("Database migrations"));
        assert_eq!(database.paths, vec!["migrations/**/*.sql"]);
        assert_eq!(database.verify_commands.len(), 1);
        assert_eq!(database.setup_commands.len(), 1);
    }

    #[test]
    fn test_profiles_mixed_command_formats() {
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "mixed"
paths = ["**/*.rs"]
verify_commands = [
    "cargo fmt --check",
    { command = "cargo test", name = "Tests", description = "Run test suite" }
]
setup_commands = [
    "cargo build",
    { run = "cp .env.example .env", name = "Setup env" }
]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.profiles.len(), 1);

        let profile = &config.task.orchestrate.profiles[0];
        assert_eq!(profile.name, "mixed");
        assert_eq!(profile.verify_commands.len(), 2);
        assert_eq!(profile.setup_commands.len(), 2);

        // Verify first command is simple
        assert_eq!(profile.verify_commands[0].command(), "cargo fmt --check");
        assert!(profile.verify_commands[0].name().is_none());

        // Verify second command is detailed
        assert_eq!(profile.verify_commands[1].command(), "cargo test");
        assert_eq!(profile.verify_commands[1].name(), Some("Tests"));
        assert_eq!(
            profile.verify_commands[1].description(),
            Some("Run test suite")
        );

        // Setup commands
        assert_eq!(profile.setup_commands[0].command(), "cargo build");
        assert_eq!(profile.setup_commands[1].command(), "cp .env.example .env");
        assert_eq!(profile.setup_commands[1].label(), "Setup env");
    }

    #[test]
    fn test_profiles_minimal_profile() {
        // Profile with only required field (name)
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "minimal"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.profiles.len(), 1);

        let profile = &config.task.orchestrate.profiles[0];
        assert_eq!(profile.name, "minimal");
        assert!(profile.description.is_none());
        assert!(profile.paths.is_empty());
        assert!(profile.working_dir.is_none());
        assert!(profile.verify_commands.is_empty());
        assert!(profile.setup_commands.is_empty());
    }

    #[test]
    fn test_profiles_with_existing_verify_commands() {
        // Profiles can coexist with global verify_commands
        let toml_content = r#"
[task.orchestrate]
verify_commands = ["cargo test"]

[[task.orchestrate.profiles]]
name = "docs"
paths = ["docs/**/*.md"]
verify_commands = ["mdbook test"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        // Global verify_commands still work
        assert_eq!(config.task.orchestrate.verify_commands.len(), 1);
        assert_eq!(
            config.task.orchestrate.verify_commands[0].command(),
            "cargo test"
        );
        // Profile-specific verify_commands also work
        assert_eq!(config.task.orchestrate.profiles.len(), 1);
        assert_eq!(config.task.orchestrate.profiles[0].name, "docs");
        assert_eq!(config.task.orchestrate.profiles[0].verify_commands.len(), 1);
        assert_eq!(
            config.task.orchestrate.profiles[0].verify_commands[0].command(),
            "mdbook test"
        );
    }

    // --- Task 40.4: Comprehensive profile parsing tests ---

    #[test]
    fn test_profile_empty_paths_allowed() {
        // Profile with empty paths array is allowed
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "global-checks"
description = "Global checks that run always"
paths = []
verify_commands = ["cargo fmt --check", "cargo clippy"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.profiles.len(), 1);

        let profile = &config.task.orchestrate.profiles[0];
        assert_eq!(profile.name, "global-checks");
        assert_eq!(
            profile.description.as_deref(),
            Some("Global checks that run always")
        );
        assert!(profile.paths.is_empty());
        assert_eq!(profile.verify_commands.len(), 2);
    }

    #[test]
    fn test_profile_with_working_dir() {
        // Profile with working_dir for running commands in specific directory
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "api"
paths = ["api/**/*.rs"]
working_dir = "api"
verify_commands = ["cargo test"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.profiles.len(), 1);

        let profile = &config.task.orchestrate.profiles[0];
        assert_eq!(profile.name, "api");
        assert_eq!(profile.working_dir.as_deref(), Some("api"));
        assert_eq!(profile.paths, vec!["api/**/*.rs"]);
        assert_eq!(profile.verify_commands.len(), 1);
    }

    #[test]
    fn test_profile_setup_commands_detailed_format() {
        // Profile with setup commands using detailed format with name
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "frontend"
paths = ["frontend/**/*.ts"]

[[task.orchestrate.profiles.setup_commands]]
run = "npm install"
name = "Install dependencies"

[[task.orchestrate.profiles.setup_commands]]
run = "npm run build:dev"
name = "Build for development"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.profiles.len(), 1);

        let profile = &config.task.orchestrate.profiles[0];
        assert_eq!(profile.name, "frontend");
        assert_eq!(profile.setup_commands.len(), 2);

        // First command
        assert_eq!(profile.setup_commands[0].command(), "npm install");
        assert_eq!(profile.setup_commands[0].label(), "Install dependencies");

        // Second command
        assert_eq!(profile.setup_commands[1].command(), "npm run build:dev");
        assert_eq!(profile.setup_commands[1].label(), "Build for development");
    }

    #[test]
    fn test_profile_verify_commands_detailed_format() {
        // Profile with verify commands using detailed format
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "backend"
paths = ["src/api/**/*.rs"]

[[task.orchestrate.profiles.verify_commands]]
command = "cargo test --package api"
name = "API tests"
description = "Run all API unit tests"

[[task.orchestrate.profiles.verify_commands]]
command = "cargo clippy --package api -- -D warnings"
name = "API lint"
description = "Check API code with clippy"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.profiles.len(), 1);

        let profile = &config.task.orchestrate.profiles[0];
        assert_eq!(profile.name, "backend");
        assert_eq!(profile.verify_commands.len(), 2);

        // First command
        assert_eq!(
            profile.verify_commands[0].command(),
            "cargo test --package api"
        );
        assert_eq!(profile.verify_commands[0].name(), Some("API tests"));
        assert_eq!(
            profile.verify_commands[0].description(),
            Some("Run all API unit tests")
        );

        // Second command
        assert_eq!(
            profile.verify_commands[1].command(),
            "cargo clippy --package api -- -D warnings"
        );
        assert_eq!(profile.verify_commands[1].name(), Some("API lint"));
        assert_eq!(
            profile.verify_commands[1].description(),
            Some("Check API code with clippy")
        );
    }

    #[test]
    fn test_profile_with_all_fields() {
        // Profile with all possible fields populated
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "comprehensive"
description = "Comprehensive test profile"
paths = ["src/**/*.rs", "tests/**/*.rs"]
working_dir = "workspace"
verify_commands = [
    { command = "cargo test", name = "Tests", description = "Run tests" },
    "cargo fmt --check"
]
setup_commands = [
    { run = "cargo build", name = "Build" },
    "cp .env.example .env"
]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.profiles.len(), 1);

        let profile = &config.task.orchestrate.profiles[0];
        assert_eq!(profile.name, "comprehensive");
        assert_eq!(
            profile.description.as_deref(),
            Some("Comprehensive test profile")
        );
        assert_eq!(profile.paths, vec!["src/**/*.rs", "tests/**/*.rs"]);
        assert_eq!(profile.working_dir.as_deref(), Some("workspace"));
        assert_eq!(profile.verify_commands.len(), 2);
        assert_eq!(profile.setup_commands.len(), 2);

        // Verify mixed command formats work
        assert_eq!(profile.verify_commands[0].name(), Some("Tests"));
        assert!(profile.verify_commands[1].name().is_none());
        assert_eq!(profile.setup_commands[0].label(), "Build");
        assert_eq!(profile.setup_commands[1].label(), "cp .env.example .env");
    }

    #[test]
    fn test_multiple_profiles_different_configurations() {
        // Multiple profiles with varying configurations
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "basic"
paths = ["src/**/*.rs"]
verify_commands = ["cargo test"]

[[task.orchestrate.profiles]]
name = "detailed"
description = "Detailed profile"
paths = ["api/**/*.rs"]
working_dir = "api"
verify_commands = [
    { command = "cargo test --package api", name = "API Tests" }
]
setup_commands = ["npm install"]

[[task.orchestrate.profiles]]
name = "minimal"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.profiles.len(), 3);

        // Basic profile
        let basic = &config.task.orchestrate.profiles[0];
        assert_eq!(basic.name, "basic");
        assert!(basic.description.is_none());
        assert_eq!(basic.paths, vec!["src/**/*.rs"]);
        assert!(basic.working_dir.is_none());
        assert_eq!(basic.verify_commands.len(), 1);
        assert!(basic.setup_commands.is_empty());

        // Detailed profile
        let detailed = &config.task.orchestrate.profiles[1];
        assert_eq!(detailed.name, "detailed");
        assert_eq!(detailed.description.as_deref(), Some("Detailed profile"));
        assert_eq!(detailed.paths, vec!["api/**/*.rs"]);
        assert_eq!(detailed.working_dir.as_deref(), Some("api"));
        assert_eq!(detailed.verify_commands.len(), 1);
        assert_eq!(detailed.setup_commands.len(), 1);

        // Minimal profile
        let minimal = &config.task.orchestrate.profiles[2];
        assert_eq!(minimal.name, "minimal");
        assert!(minimal.description.is_none());
        assert!(minimal.paths.is_empty());
        assert!(minimal.working_dir.is_none());
        assert!(minimal.verify_commands.is_empty());
        assert!(minimal.setup_commands.is_empty());
    }

    #[test]
    fn test_profile_without_paths_field() {
        // Profile where paths field is omitted entirely (not just empty array)
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "no-paths"
description = "Profile without paths field"
verify_commands = ["cargo test --all"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.profiles.len(), 1);

        let profile = &config.task.orchestrate.profiles[0];
        assert_eq!(profile.name, "no-paths");
        assert_eq!(
            profile.description.as_deref(),
            Some("Profile without paths field")
        );
        assert!(profile.paths.is_empty()); // Should default to empty vec
        assert_eq!(profile.verify_commands.len(), 1);
    }

    // ── Task 53.4: Test duplikaty nazw profili w .ralph.toml ──

    #[test]
    fn test_profiles_duplicate_names_parsing() {
        // Dwa profile z tą samą nazwą "frontend" — Vec<VerifyProfile> dopuszcza duplikaty
        let toml_content = r#"
[[task.orchestrate.profiles]]
name = "frontend"
description = "First frontend profile"
paths = ["src/ui/**/*.ts"]
verify_commands = ["npm test"]

[[task.orchestrate.profiles]]
name = "frontend"
description = "Second frontend profile"
paths = ["src/components/**/*.tsx"]
verify_commands = ["npm run lint"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();

        // Vec dopuszcza duplikaty — oba profile są sparsowane
        assert_eq!(
            config.task.orchestrate.profiles.len(),
            2,
            "Vec<VerifyProfile> powinien zawierać oba profile (duplikaty nazw są dozwolone)"
        );

        // Pierwszy profil
        let profile1 = &config.task.orchestrate.profiles[0];
        assert_eq!(profile1.name, "frontend");
        assert_eq!(
            profile1.description.as_deref(),
            Some("First frontend profile")
        );
        assert_eq!(profile1.paths, vec!["src/ui/**/*.ts"]);
        assert_eq!(profile1.verify_commands.len(), 1);

        // Drugi profil (duplikat nazwy)
        let profile2 = &config.task.orchestrate.profiles[1];
        assert_eq!(profile2.name, "frontend");
        assert_eq!(
            profile2.description.as_deref(),
            Some("Second frontend profile")
        );
        assert_eq!(profile2.paths, vec!["src/components/**/*.tsx"]);
        assert_eq!(profile2.verify_commands.len(), 1);
    }

    // --- format_profiles_info tests ---

    #[test]
    fn test_format_profiles_info_empty() {
        // Empty profiles slice should return empty string
        let result = format_profiles_info(&[]);
        assert_eq!(result, "");
    }

    #[test]
    fn test_format_profiles_info_single_profile() {
        // Single profile with all fields
        let profile = VerifyProfile {
            name: "frontend".to_string(),
            description: Some("Frontend tests".to_string()),
            paths: vec!["src/ui/**/*.rs".to_string(), "assets/**/*".to_string()],
            working_dir: Some("frontend".to_string()),
            verify_commands: vec![
                VerifyCommand::Simple("npm test".to_string()),
                VerifyCommand::Detailed {
                    command: "npm run lint".to_string(),
                    name: Some("Lint".to_string()),
                    description: None,
                },
            ],
            setup_commands: vec![SetupCommand::Simple("npm install".to_string())],
        };

        let result = format_profiles_info(&[profile]);
        let expected = "- **frontend**: Frontend tests
  paths: [src/ui/**/*.rs, assets/**/*]
  verify: [npm test, Lint]
  setup: [npm install]
  working_dir: frontend
";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_format_profiles_info_multiple_profiles() {
        // Multiple profiles with varying configurations
        let profiles = vec![
            VerifyProfile {
                name: "backend".to_string(),
                description: None,
                paths: vec!["src/api/**/*.rs".to_string()],
                working_dir: None,
                verify_commands: vec![VerifyCommand::Simple(
                    "cargo test --package api".to_string(),
                )],
                setup_commands: vec![],
            },
            VerifyProfile {
                name: "database".to_string(),
                description: Some("Database migrations".to_string()),
                paths: vec!["migrations/**/*.sql".to_string()],
                working_dir: Some("db".to_string()),
                verify_commands: vec![VerifyCommand::Simple("sqlx migrate run".to_string())],
                setup_commands: vec![SetupCommand::Detailed {
                    run: "sqlx database create".to_string(),
                    name: Some("Create DB".to_string()),
                }],
            },
        ];

        let result = format_profiles_info(&profiles);
        let expected = "- **backend**
  paths: [src/api/**/*.rs]
  verify: [cargo test --package api]

- **database**: Database migrations
  paths: [migrations/**/*.sql]
  verify: [sqlx migrate run]
  setup: [Create DB]
  working_dir: db
";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_format_profiles_info_profile_without_optional_fields() {
        // Profile with only required field (name)
        let profile = VerifyProfile {
            name: "minimal".to_string(),
            description: None,
            paths: vec![],
            working_dir: None,
            verify_commands: vec![],
            setup_commands: vec![],
        };

        let result = format_profiles_info(&[profile]);
        let expected = "- **minimal**\n";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_format_profiles_info_mixed_command_formats() {
        // Profile with mixed simple and detailed commands
        let profile = VerifyProfile {
            name: "mixed".to_string(),
            description: Some("Mixed command formats".to_string()),
            paths: vec!["**/*.rs".to_string()],
            working_dir: None,
            verify_commands: vec![
                VerifyCommand::Simple("cargo fmt --check".to_string()),
                VerifyCommand::Detailed {
                    command: "cargo test".to_string(),
                    name: Some("Tests".to_string()),
                    description: Some("Run test suite".to_string()),
                },
                VerifyCommand::Detailed {
                    command: "cargo clippy".to_string(),
                    name: None, // Falls back to command as label
                    description: None,
                },
            ],
            setup_commands: vec![
                SetupCommand::Simple("cargo build".to_string()),
                SetupCommand::Detailed {
                    run: "cp .env.example .env".to_string(),
                    name: Some("Setup env".to_string()),
                },
            ],
        };

        let result = format_profiles_info(&[profile]);
        let expected = "- **mixed**: Mixed command formats
  paths: [**/*.rs]
  verify: [cargo fmt --check, Tests, cargo clippy]
  setup: [cargo build, Setup env]
";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_format_profiles_info_no_paths_no_commands() {
        // Profile with description but no paths or commands
        let profile = VerifyProfile {
            name: "empty-profile".to_string(),
            description: Some("Profile with only description".to_string()),
            paths: vec![],
            working_dir: None,
            verify_commands: vec![],
            setup_commands: vec![],
        };

        let result = format_profiles_info(&[profile]);
        let expected = "- **empty-profile**: Profile with only description\n";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_format_profiles_info_single_path_and_command() {
        // Profile with single path and single command
        let profile = VerifyProfile {
            name: "simple".to_string(),
            description: None,
            paths: vec!["src/main.rs".to_string()],
            working_dir: None,
            verify_commands: vec![VerifyCommand::Simple("cargo build".to_string())],
            setup_commands: vec![],
        };

        let result = format_profiles_info(&[profile]);
        let expected = "- **simple**
  paths: [src/main.rs]
  verify: [cargo build]
";
        assert_eq!(result, expected);
    }

    // ── Task 48.2: Backward compatibility tests (config without profiles) ──

    /// Test parsing .ralph.toml without [task.orchestrate.profiles] section.
    /// Should default to empty profiles vec, preserving pre-profiles behavior.
    #[test]
    fn test_orchestrate_config_backward_compat_no_profiles() {
        let toml_content = r#"
[task.orchestrate]
workers = 4
verify_commands = ["cargo test", "cargo clippy"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.workers, 4);
        assert_eq!(config.task.orchestrate.verify_commands.len(), 2);
        // Profiles should default to empty vec
        assert!(
            config.task.orchestrate.profiles.is_empty(),
            "Config without profiles section should have empty profiles vec"
        );
    }

    /// Test that global verify_commands still work without profiles.
    #[test]
    fn test_orchestrate_config_global_verify_commands_without_profiles() {
        let toml_content = r#"
[task.orchestrate]
verify_commands = ["cargo test", "cargo fmt --check"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        // Global verify_commands should work as before
        assert_eq!(config.task.orchestrate.verify_commands.len(), 2);
        assert_eq!(
            config.task.orchestrate.verify_commands[0].command(),
            "cargo test"
        );
        assert_eq!(
            config.task.orchestrate.verify_commands[1].command(),
            "cargo fmt --check"
        );
        // Profiles should be empty
        assert!(config.task.orchestrate.profiles.is_empty());
    }

    /// Test minimal .ralph.toml without any orchestrate settings.
    #[test]
    fn test_orchestrate_config_minimal_toml() {
        let toml_content = r#"
[prompt]
prefix = "test"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        // Should have all defaults
        assert_eq!(config.task.orchestrate.workers, 2);
        assert!(config.task.orchestrate.verify_commands.is_empty());
        assert!(config.task.orchestrate.profiles.is_empty());
    }

    /// Test empty .ralph.toml (completely empty file).
    #[test]
    fn test_orchestrate_config_empty_toml() {
        let config: FileConfig = toml::from_str("").unwrap();
        // Should have all defaults
        assert_eq!(config.task.orchestrate.workers, 2);
        assert_eq!(config.task.orchestrate.max_retries, 3);
        assert!(config.task.orchestrate.verify_commands.is_empty());
        assert!(config.task.orchestrate.profiles.is_empty());
    }

    /// Test that profiles field is truly optional (omitted vs empty array).
    #[test]
    fn test_orchestrate_config_profiles_field_omitted_vs_empty() {
        // Omitted profiles field
        let toml_omitted = r#"
[task.orchestrate]
workers = 2
"#;
        let config_omitted: FileConfig = toml::from_str(toml_omitted).unwrap();
        assert!(config_omitted.task.orchestrate.profiles.is_empty());

        // Explicit empty profiles array
        let toml_empty = r#"
[task.orchestrate]
workers = 2
profiles = []
"#;
        let config_empty: FileConfig = toml::from_str(toml_empty).unwrap();
        assert!(config_empty.task.orchestrate.profiles.is_empty());

        // Both should behave identically
        assert_eq!(
            config_omitted.task.orchestrate.profiles.len(),
            config_empty.task.orchestrate.profiles.len()
        );
    }

    /// Test: .ralph.toml with unknown sections and fields — should be ignored gracefully.
    /// Serde without deny_unknown_fields silently ignores unknown top-level sections
    /// and unknown fields in known sections.
    #[test]
    fn test_toml_with_unknown_section_and_fields_ignored() {
        let toml_content = r#"
[prompt]
prefix = "Test prefix"
suffix = "Test suffix"

[unknown_section]
unknown_key = "This should be ignored"
another_unknown = 42

[task]
progress_file = "CUSTOM_PROGRESS.md"
tasks_file = "CUSTOM_TASKS.yml"
unknown_field_in_task = "This should also be ignored"

[task.orchestrate]
workers = 5
max_retries = 7
unknown_orchestrate_field = "Ignored too"
"#;
        // Should parse successfully without error
        let config: FileConfig = toml::from_str(toml_content).unwrap();

        // Verify known fields were parsed correctly
        assert_eq!(
            config.prompt.prefix.as_deref(),
            Some("Test prefix"),
            "Known prompt.prefix should be parsed"
        );
        assert_eq!(
            config.prompt.suffix.as_deref(),
            Some("Test suffix"),
            "Known prompt.suffix should be parsed"
        );
        assert_eq!(
            config.task.progress_file,
            std::path::PathBuf::from("CUSTOM_PROGRESS.md"),
            "Known task.progress_file should be parsed"
        );
        assert_eq!(
            config.task.tasks_file,
            std::path::PathBuf::from("CUSTOM_TASKS.yml"),
            "Known task.tasks_file should be parsed"
        );
        assert_eq!(
            config.task.orchestrate.workers, 5,
            "Known task.orchestrate.workers should be parsed"
        );
        assert_eq!(
            config.task.orchestrate.max_retries, 7,
            "Known task.orchestrate.max_retries should be parsed"
        );

        // Unknown sections and fields should simply be ignored
        // (their absence in the config struct proves they were not deserialized)
    }

    // ── Task 53.1: workers=0 validation tests ──────────────────────────

    #[test]
    fn test_workers_zero_parsing_succeeds() {
        // workers=0 parses as u32 without error — no validation at config level
        let toml_content = r#"
[task.orchestrate]
workers = 0
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.task.orchestrate.workers, 0);
    }

    #[test]
    fn test_workers_zero_vs_omitted_field() {
        // Explicit workers=0 vs omitted field (default=2) — different behavior
        let config_zero: FileConfig = toml::from_str(
            r#"
[task.orchestrate]
workers = 0
"#,
        )
        .unwrap();
        assert_eq!(config_zero.task.orchestrate.workers, 0);

        let config_omitted: FileConfig = toml::from_str(
            r#"
[task.orchestrate]
max_retries = 3
"#,
        )
        .unwrap();
        assert_eq!(config_omitted.task.orchestrate.workers, 2);
    }

    /// Test: Empty [task.orchestrate] section without any fields.
    /// Serde with #[serde(default)] should fill all fields with default values.
    #[test]
    fn test_empty_orchestrate_section_uses_defaults() {
        let toml_content = r#"
[task.orchestrate]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();

        // All orchestrate fields should have their default values
        assert_eq!(
            config.task.orchestrate.workers, 2,
            "Empty [task.orchestrate] should use default workers=2"
        );
        assert_eq!(
            config.task.orchestrate.max_retries, 3,
            "Empty [task.orchestrate] should use default max_retries=3"
        );
        assert!(
            config.task.orchestrate.worktree_prefix.is_none(),
            "Empty [task.orchestrate] should have default worktree_prefix=None"
        );
        assert!(
            config.task.orchestrate.default_model.is_none(),
            "Empty [task.orchestrate] should have default default_model=None"
        );
        assert!(
            config.task.orchestrate.verify_commands.is_empty(),
            "Empty [task.orchestrate] should have default verify_commands=[]"
        );
        assert!(
            config.task.orchestrate.setup_commands.is_empty(),
            "Empty [task.orchestrate] should have default setup_commands=[]"
        );
        assert!(
            config.task.orchestrate.conflict_resolution_model.is_none(),
            "Empty [task.orchestrate] should have default conflict_resolution_model=None"
        );
        assert_eq!(
            config.task.orchestrate.watchdog_interval_secs, 10,
            "Empty [task.orchestrate] should use default watchdog_interval_secs=10"
        );
        assert_eq!(
            config.task.orchestrate.phase_timeout_minutes, 30,
            "Empty [task.orchestrate] should use default phase_timeout_minutes=30"
        );
        assert_eq!(
            config.task.orchestrate.git_timeout_secs, 120,
            "Empty [task.orchestrate] should use default git_timeout_secs=120"
        );
        assert_eq!(
            config.task.orchestrate.setup_timeout_secs, 300,
            "Empty [task.orchestrate] should use default setup_timeout_secs=300"
        );
        assert_eq!(
            config.task.orchestrate.merge_timeout_minutes, 15,
            "Empty [task.orchestrate] should use default merge_timeout_minutes=15"
        );
        assert!(
            config.task.orchestrate.review_model.is_none(),
            "Empty [task.orchestrate] should have default review_model=None"
        );
        assert!(
            config.task.orchestrate.profiles.is_empty(),
            "Empty [task.orchestrate] should have default profiles=[]"
        );
    }

    // ── Task 53.5: Negative timeout values — parse error ────────────────

    /// Test: .ralph.toml z ujemnym phase_timeout_minutes — error
    /// Typ u32 odrzuci ujemną wartość na etapie parsowania.
    /// Sprawdzamy czytelność error message.
    #[test]
    fn test_negative_phase_timeout_minutes_parse_error() {
        let toml_content = r#"
[task.orchestrate]
phase_timeout_minutes = -1
"#;
        let result = toml::from_str::<FileConfig>(toml_content);

        assert!(
            result.is_err(),
            "Expected parsing error for negative phase_timeout_minutes"
        );

        // Error message powinien jasno wskazywać problem z typem
        // np. "invalid value: integer `-1`, expected u32"
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("invalid value") && err_msg.contains("expected u32"),
            "Error message should mention both invalid value and expected type. Got: {}",
            err_msg
        );
    }

    // --- TUI config tests ---

    #[test]
    fn test_tui_config_defaults() {
        let config = FileConfig::default();
        assert_eq!(config.tui.output_buffer_size, 5000);
        assert_eq!(config.tui.sidebar_width, 25);
        assert!(config.tui.sidebar_visible);
        assert_eq!(config.tui.theme, "default");
    }

    #[test]
    fn test_tui_config_without_tui_section() {
        // Existing .ralph.toml without [tui] section should use defaults
        let toml_content = r#"
[prompt]
prefix = "test"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.tui.output_buffer_size, 5000);
        assert_eq!(config.tui.sidebar_width, 25);
        assert!(config.tui.sidebar_visible);
        assert_eq!(config.tui.theme, "default");
    }

    #[test]
    fn test_tui_config_full() {
        let toml_content = r#"
[tui]
output_buffer_size = 10000
sidebar_width = 35
sidebar_visible = false
theme = "dark"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.tui.output_buffer_size, 10000);
        assert_eq!(config.tui.sidebar_width, 35);
        assert!(!config.tui.sidebar_visible);
        assert_eq!(config.tui.theme, "dark");
    }

    #[test]
    fn test_tui_config_partial() {
        let toml_content = r#"
[tui]
output_buffer_size = 8000
theme = "light"
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.tui.output_buffer_size, 8000);
        assert_eq!(config.tui.sidebar_width, 25); // default
        assert!(config.tui.sidebar_visible); // default
        assert_eq!(config.tui.theme, "light");
    }

    #[test]
    fn test_tui_config_with_other_sections() {
        // TUI config should work alongside other sections
        let toml_content = r#"
[prompt]
prefix = "Custom prompt"

[tui]
output_buffer_size = 7500
theme = "solarized"

[ui]
nerd_font = false
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.prompt.prefix.as_deref(), Some("Custom prompt"));
        assert_eq!(config.tui.output_buffer_size, 7500);
        assert_eq!(config.tui.theme, "solarized");
        assert!(!config.ui.nerd_font);
    }

    // ── Task 10.2: Cascading config merge tests ─────────────────────────

    #[test]
    fn test_merge_overlay_overrides_base_option_fields() {
        let base = FileConfig {
            prompt: PromptConfig {
                prefix: Some("base-prefix".into()),
                suffix: None,
                system: Some("base-system".into()),
            },
            ..Default::default()
        };
        let overlay = FileConfig {
            prompt: PromptConfig {
                prefix: Some("overlay-prefix".into()),
                suffix: Some("overlay-suffix".into()),
                system: None,
            },
            ..Default::default()
        };

        let merged = FileConfig::merge(base, overlay);
        // overlay Some nadpisuje base Some
        assert_eq!(merged.prompt.prefix.as_deref(), Some("overlay-prefix"));
        // overlay Some nadpisuje base None
        assert_eq!(merged.prompt.suffix.as_deref(), Some("overlay-suffix"));
        // overlay None → base Some zachowane
        assert_eq!(merged.prompt.system.as_deref(), Some("base-system"));
    }

    #[test]
    fn test_merge_overlay_overrides_base_scalar_fields() {
        let base: FileConfig = toml::from_str(
            r#"
[task.orchestrate]
workers = 8
max_retries = 5
phase_timeout_minutes = 60
"#,
        )
        .unwrap();

        let overlay: FileConfig = toml::from_str(
            r#"
[task.orchestrate]
workers = 4
"#,
        )
        .unwrap();

        let merged = FileConfig::merge(base, overlay);
        // overlay workers=4 (non-default) nadpisuje base workers=8
        assert_eq!(merged.task.orchestrate.workers, 4);
        // overlay max_retries=3 (default) → base max_retries=5 zachowane
        assert_eq!(merged.task.orchestrate.max_retries, 5);
        // overlay phase_timeout_minutes=30 (default) → base 60 zachowane
        assert_eq!(merged.task.orchestrate.phase_timeout_minutes, 60);
    }

    #[test]
    fn test_merge_vec_fields_replace_entirely() {
        let base = FileConfig {
            task: TaskConfig {
                orchestrate: OrchestrateConfig {
                    verify_commands: vec![
                        VerifyCommand::Simple("cargo test".into()),
                        VerifyCommand::Simple("cargo clippy".into()),
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let overlay = FileConfig {
            task: TaskConfig {
                orchestrate: OrchestrateConfig {
                    verify_commands: vec![VerifyCommand::Simple("npm test".into())],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = FileConfig::merge(base, overlay);
        // Overlay Vec niepusty → zastępuje base w całości (nie appenduje)
        assert_eq!(merged.task.orchestrate.verify_commands.len(), 1);
        assert_eq!(
            merged.task.orchestrate.verify_commands[0].command(),
            "npm test"
        );
    }

    // ── Task 10.3: Includes support tests ───────────────────────────────────

    /// Helper: tworzy tymczasowy katalog z plikami TOML dla testów includes.
    fn write_temp_toml(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_includes_field_parses_as_empty_by_default() {
        // Plik bez `includes` — pole domyślnie puste
        let config: FileConfig = toml::from_str("").unwrap();
        assert!(config.includes.is_empty());
    }

    #[test]
    fn test_includes_field_parses_vec_of_strings() {
        // `includes` parsuje się jako Vec<String>
        let toml_content = r#"
includes = ["keybindings.toml", "notifications.toml"]
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(
            config.includes,
            vec!["keybindings.toml", "notifications.toml"]
        );
    }

    #[test]
    fn test_merge_empty_vec_overlay_keeps_base() {
        let base = FileConfig {
            task: TaskConfig {
                orchestrate: OrchestrateConfig {
                    verify_commands: vec![VerifyCommand::Simple("cargo test".into())],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        // overlay z pustym Vec — zachowaj base
        let overlay = FileConfig::default();

        let merged = FileConfig::merge(base, overlay);
        assert_eq!(merged.task.orchestrate.verify_commands.len(), 1);
        assert_eq!(
            merged.task.orchestrate.verify_commands[0].command(),
            "cargo test"
        );
    }

    #[test]
    fn test_merge_defaults_into_defaults_is_default() {
        let base = FileConfig::default();
        let overlay = FileConfig::default();
        let merged = FileConfig::merge(base, overlay);

        // Merge dwóch defaults → wynik jest identyczny z default
        assert!(merged.prompt.prefix.is_none());
        assert!(merged.prompt.suffix.is_none());
        assert!(merged.ui.nerd_font);
        assert_eq!(merged.task.orchestrate.workers, 2);
        assert_eq!(merged.task.orchestrate.max_retries, 3);
        assert!(merged.task.orchestrate.verify_commands.is_empty());
    }

    #[test]
    fn test_merge_three_layers_cascading() {
        // Symulacja: defaults → global → local
        let defaults = FileConfig::default();

        let global: FileConfig = toml::from_str(
            r#"
[prompt]
prefix = "global-prefix"

[task.orchestrate]
workers = 6
default_model = "sonnet"
verify_commands = ["cargo test"]
"#,
        )
        .unwrap();

        let local: FileConfig = toml::from_str(
            r#"
[prompt]
suffix = "local-suffix"

[task.orchestrate]
workers = 3
verify_commands = ["npm test", "npm lint"]
"#,
        )
        .unwrap();

        let step1 = FileConfig::merge(defaults, global);
        let merged = FileConfig::merge(step1, local);

        // prefix z global (local nie ma)
        assert_eq!(merged.prompt.prefix.as_deref(), Some("global-prefix"));
        // suffix z local
        assert_eq!(merged.prompt.suffix.as_deref(), Some("local-suffix"));
        // workers: local=3 nadpisuje global=6
        assert_eq!(merged.task.orchestrate.workers, 3);
        // default_model z global (local nie ustawia)
        assert_eq!(
            merged.task.orchestrate.default_model.as_deref(),
            Some("sonnet")
        );
        // verify_commands: local niepusty → zastępuje global
        assert_eq!(merged.task.orchestrate.verify_commands.len(), 2);
        assert_eq!(
            merged.task.orchestrate.verify_commands[0].command(),
            "npm test"
        );
    }

    #[test]
    fn test_merge_partial_overrides_preserve_base() {
        let base: FileConfig = toml::from_str(
            r#"
[ui]
nerd_font = false

[tui]
output_buffer_size = 10000
sidebar_width = 40
theme = "dark"

[logging]
log_dir = "custom/logs"
max_log_files = 20
"#,
        )
        .unwrap();

        // overlay zmienia tylko theme i log_dir
        let overlay: FileConfig = toml::from_str(
            r#"
[tui]
theme = "light"

[logging]
log_dir = "other/logs"
"#,
        )
        .unwrap();

        let merged = FileConfig::merge(base, overlay);

        // ui.nerd_font: overlay=true (default) → base=false zachowane
        assert!(!merged.ui.nerd_font);
        // tui: overlay theme="light" nadpisuje, reszta z base
        assert_eq!(merged.tui.theme, "light");
        assert_eq!(merged.tui.output_buffer_size, 10000);
        assert_eq!(merged.tui.sidebar_width, 40);
        // logging: overlay log_dir nadpisuje, max_log_files z base
        assert_eq!(merged.logging.log_dir, PathBuf::from("other/logs"));
        assert_eq!(merged.logging.max_log_files, 20);
    }

    #[test]
    fn test_load_merged_config_missing_local_uses_global() {
        // load_merged_config z nieistniejącym local_path → używa global + defaults
        let result = load_merged_config(&PathBuf::from("/tmp/nonexistent/.ralph.toml"));
        assert!(
            result.is_ok(),
            "Brak local .ralph.toml nie powinien powodować błędu"
        );
    }

    #[test]
    fn test_includes_empty_array_parses() {
        // Jawna pusta tablica includes = []
        let toml_content = r#"
includes = []
"#;
        let config: FileConfig = toml::from_str(toml_content).unwrap();
        assert!(config.includes.is_empty());
    }

    #[test]
    fn test_load_from_path_no_includes() {
        // load_from_path bez includes — działa jak poprzednio
        let tmp = tempfile::TempDir::new().unwrap();

        let config_path = write_temp_toml(
            tmp.path(),
            "config.toml",
            r#"
[prompt]
prefix = "BasePrefix"
"#,
        );

        let config = FileConfig::load_from_path(&config_path).unwrap();
        assert_eq!(config.prompt.prefix.as_deref(), Some("BasePrefix"));
    }

    #[test]
    fn test_load_from_path_single_include_merges() {
        // Jeden plik include — jego wartości są mergowane (include wygrywa)
        let tmp = tempfile::TempDir::new().unwrap();

        write_temp_toml(
            tmp.path(),
            "extra.toml",
            r#"
[prompt]
suffix = "IncludeSuffix"

[ui]
nerd_font = false
"#,
        );

        let config_path = write_temp_toml(
            tmp.path(),
            "config.toml",
            r#"
includes = ["extra.toml"]

[prompt]
prefix = "BasePrefix"
"#,
        );

        let config = FileConfig::load_from_path(&config_path).unwrap();
        // prefix z base, suffix z include
        assert_eq!(config.prompt.prefix.as_deref(), Some("BasePrefix"));
        assert_eq!(config.prompt.suffix.as_deref(), Some("IncludeSuffix"));
        // ui.nerd_font z include
        assert!(!config.ui.nerd_font);
    }

    #[test]
    fn test_load_from_path_multiple_includes_last_wins() {
        // Wiele includes — ostatni wygrywa
        let tmp = tempfile::TempDir::new().unwrap();

        write_temp_toml(
            tmp.path(),
            "first.toml",
            r#"
[prompt]
prefix = "FirstPrefix"
suffix = "FirstSuffix"
"#,
        );

        write_temp_toml(
            tmp.path(),
            "second.toml",
            r#"
[prompt]
suffix = "SecondSuffix"
"#,
        );

        let config_path = write_temp_toml(
            tmp.path(),
            "config.toml",
            r#"
includes = ["first.toml", "second.toml"]

[prompt]
prefix = "BasePrefix"
"#,
        );

        let config = FileConfig::load_from_path(&config_path).unwrap();
        // prefix: base → first.toml → second.toml (second nie ma prefix, więc first wygrywa)
        assert_eq!(config.prompt.prefix.as_deref(), Some("FirstPrefix"));
        // suffix: base nie ma → first → second wins
        assert_eq!(config.prompt.suffix.as_deref(), Some("SecondSuffix"));
    }

    #[test]
    fn test_load_from_path_missing_include_is_warning_not_error() {
        // Brakujący plik include — OK (ostrzeżenie na stderr), reszta działa
        let tmp = tempfile::TempDir::new().unwrap();

        let config_path = write_temp_toml(
            tmp.path(),
            "config.toml",
            r#"
includes = ["nonexistent.toml"]

[prompt]
prefix = "BasePrefix"
"#,
        );

        // Powinno zwrócić Ok, nie Err
        let result = FileConfig::load_from_path(&config_path);
        assert!(
            result.is_ok(),
            "Brakujący include nie powinien zwracać błędu"
        );

        let config = result.unwrap();
        assert_eq!(config.prompt.prefix.as_deref(), Some("BasePrefix"));
    }

    #[test]
    fn test_load_from_path_include_overrides_base() {
        // Include wygrywa nad bazowym configiem (last wins)
        let tmp = tempfile::TempDir::new().unwrap();

        write_temp_toml(
            tmp.path(),
            "override.toml",
            r#"
[prompt]
prefix = "OverridePrefix"
"#,
        );

        let config_path = write_temp_toml(
            tmp.path(),
            "config.toml",
            r#"
includes = ["override.toml"]

[prompt]
prefix = "BasePrefix"
suffix = "BaseSuffix"
"#,
        );

        let config = FileConfig::load_from_path(&config_path).unwrap();
        // prefix z override (wygrywa nad base)
        assert_eq!(config.prompt.prefix.as_deref(), Some("OverridePrefix"));
        // suffix tylko w base — zachowany
        assert_eq!(config.prompt.suffix.as_deref(), Some("BaseSuffix"));
    }

    #[test]
    fn test_load_from_path_include_depth_limit() {
        // Includes w plikach include są ignorowane (depth = 1)
        let tmp = tempfile::TempDir::new().unwrap();

        // nested.toml sam ma includes — powinny być ignorowane
        write_temp_toml(
            tmp.path(),
            "nested.toml",
            r#"
includes = ["another.toml"]

[prompt]
suffix = "NestedSuffix"
"#,
        );

        // another.toml — NIE powinien być załadowany (depth limit)
        write_temp_toml(
            tmp.path(),
            "another.toml",
            r#"
[prompt]
suffix = "AnotherSuffix"
"#,
        );

        let config_path = write_temp_toml(
            tmp.path(),
            "config.toml",
            r#"
includes = ["nested.toml"]

[prompt]
prefix = "BasePrefix"
"#,
        );

        let config = FileConfig::load_from_path(&config_path).unwrap();
        assert_eq!(config.prompt.prefix.as_deref(), Some("BasePrefix"));
        // suffix z nested.toml (bezpośredni include)
        assert_eq!(config.prompt.suffix.as_deref(), Some("NestedSuffix"));
        // another.toml NIE jest zainkludowany — AnotherSuffix nie powinien nadpisać
    }

    #[test]
    fn test_load_from_path_relative_include_path() {
        // Ścieżki relatywne rozwiązywane względem katalogu config.toml
        let tmp = tempfile::TempDir::new().unwrap();
        let sub_dir = tmp.path().join("sub");
        std::fs::create_dir_all(&sub_dir).unwrap();

        // extra.toml w tym samym katalogu co config.toml (sub/)
        write_temp_toml(
            &sub_dir,
            "extra.toml",
            r#"
[ui]
nerd_font = false
"#,
        );

        let config_path = write_temp_toml(
            &sub_dir,
            "config.toml",
            r#"
includes = ["extra.toml"]
"#,
        );

        let config = FileConfig::load_from_path(&config_path).unwrap();
        assert!(
            !config.ui.nerd_font,
            "nerd_font powinno być false z relatywnego include"
        );
    }

    #[test]
    fn test_load_merged_config_malformed_local_returns_error() {
        let tmp = std::env::temp_dir().join("ralph-test-malformed.toml");
        std::fs::write(&tmp, "[broken\ngarbage!!!").unwrap();
        let result = load_merged_config(&tmp);
        let _ = std::fs::remove_file(&tmp);
        assert!(
            result.is_err(),
            "Malformed local .ralph.toml powinien zwrócić Err"
        );
    }

    #[test]
    fn test_load_from_path_empty_includes_array() {
        // includes = [] — brak includes, domyślna konfiguracja
        let tmp = tempfile::TempDir::new().unwrap();

        let config_path = write_temp_toml(
            tmp.path(),
            "config.toml",
            r#"
includes = []

[prompt]
prefix = "OnlyBase"
"#,
        );

        let config = FileConfig::load_from_path(&config_path).unwrap();
        assert_eq!(config.prompt.prefix.as_deref(), Some("OnlyBase"));
        assert!(config.prompt.suffix.is_none());
    }

    #[test]
    fn test_includes_merge_preserves_nested_config() {
        // Include merguje zagnieżdżone sekcje (np. task.orchestrate)
        let tmp = tempfile::TempDir::new().unwrap();

        write_temp_toml(
            tmp.path(),
            "orch.toml",
            r#"
[task.orchestrate]
workers = 8
"#,
        );

        let config_path = write_temp_toml(
            tmp.path(),
            "config.toml",
            r#"
includes = ["orch.toml"]

[task]
default_model = "claude-sonnet-4-6"
"#,
        );

        let config = FileConfig::load_from_path(&config_path).unwrap();
        // workers z include
        assert_eq!(config.task.orchestrate.workers, 8);
        // default_model z base
        assert_eq!(
            config.task.default_model.as_deref(),
            Some("claude-sonnet-4-6")
        );
    }

    #[test]
    fn test_load_from_path_absolute_include_path() {
        // Ścieżki bezwzględne w includes są obsługiwane bez zmian
        let tmp_base = tempfile::TempDir::new().unwrap();
        let tmp_extra = tempfile::TempDir::new().unwrap();

        // extra.toml w oddzielnym (absolutnym) katalogu
        let extra_path = write_temp_toml(
            tmp_extra.path(),
            "extra.toml",
            r#"
[prompt]
suffix = "AbsoluteSuffix"
"#,
        );

        // config.toml z bezwzględną ścieżką do extra.toml
        let includes_line = format!(r#"includes = ["{}"]"#, extra_path.display());
        let config_path = write_temp_toml(
            tmp_base.path(),
            "config.toml",
            &format!(
                r#"
{includes_line}

[prompt]
prefix = "BasePrefix"
"#
            ),
        );

        let config = FileConfig::load_from_path(&config_path).unwrap();
        assert_eq!(config.prompt.prefix.as_deref(), Some("BasePrefix"));
        assert_eq!(config.prompt.suffix.as_deref(), Some("AbsoluteSuffix"));
    }

    #[test]
    fn test_merge_toml_array_overwrite() {
        // merge_toml dla tablic: override zastępuje (nie appenduje) base
        let mut base: toml::Value =
            toml::from_str(r#"verify = ["cargo test", "cargo build"]"#).unwrap();
        let override_val: toml::Value = toml::from_str(r#"verify = ["cargo clippy"]"#).unwrap();

        merge_toml(&mut base, override_val);

        let result_arr = base.get("verify").and_then(|v| v.as_array()).unwrap();
        assert_eq!(
            result_arr.len(),
            1,
            "Tablica powinna być zastąpiona (nie appendowana)"
        );
        assert_eq!(result_arr[0].as_str(), Some("cargo clippy"));
    }
}
