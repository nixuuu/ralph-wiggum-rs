//! Testy integracyjne dla cascading config merge i XDG path resolution.
//!
//! Pokrywa scenariusze:
//! 1. Brak global i local → same defaults
//! 2. Tylko global → global nadpisuje defaults
//! 3. Tylko local → local nadpisuje defaults
//! 4. Cascading merge: defaults → global → local (każda warstwa nadpisuje poprzednią)
//! 5. Includes: kolejność mergowania, brakujący plik
//! 6. Brakujący global config → graceful fallback do defaults
//! 7. Istniejący .ralph.toml backward compat (bez nowych pól)
//! 8. XDG_CONFIG_HOME env override dla globalnego configa

use std::path::PathBuf;

use crate::shared::file_config::{FileConfig, load_merged_config};
// Wspólny process-wide lock — zdefiniowany w crate::shared::ENV_LOCK.
// Serializuje testy ze wszystkich modułów shared, które modyfikują XDG_CONFIG_HOME/HOME,
// eliminując race condition przy `cargo test` z domyślną wielowątkowością.
use super::ENV_LOCK;

/// Pomocnik: ustawia XDG_CONFIG_HOME i zwraca poprzednią wartość.
///
/// # Safety
/// Wywołujący musi trzymać `ENV_LOCK` przed wywołaniem.
unsafe fn set_xdg(val: &str) -> Option<String> {
    let prev = std::env::var("XDG_CONFIG_HOME").ok();
    // SAFETY: wywołujący gwarantuje serializację przez ENV_LOCK
    unsafe { std::env::set_var("XDG_CONFIG_HOME", val) };
    prev
}

/// Pomocnik: przywraca XDG_CONFIG_HOME do poprzedniej wartości.
///
/// # Safety
/// Wywołujący musi trzymać `ENV_LOCK` przed wywołaniem.
unsafe fn restore_xdg(prev: Option<String>) {
    // SAFETY: wywołujący gwarantuje serializację przez ENV_LOCK
    unsafe {
        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
}

/// Tworzy strukturę katalogów dla globalnego config ralph:
/// `<xdg_root>/ralph/config.toml`.
///
/// Zwraca ścieżkę do `<xdg_root>`.
fn create_global_config(xdg_root: &std::path::Path, toml_content: &str) -> PathBuf {
    let ralph_dir = xdg_root.join("ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    std::fs::write(ralph_dir.join("config.toml"), toml_content).unwrap();
    xdg_root.to_path_buf()
}

/// Tworzy plik lokalnego configa `.ralph.toml` w podanym katalogu.
///
/// Zwraca pełną ścieżkę do pliku.
fn create_local_config(dir: &std::path::Path, toml_content: &str) -> PathBuf {
    let path = dir.join(".ralph.toml");
    std::fs::write(&path, toml_content).unwrap();
    path
}

// ── 1. Defaults only (no global, no local) ──────────────────────────────────

#[test]
fn test_cascade_defaults_only() {
    let _lock = ENV_LOCK.lock().unwrap();
    // Wskaż XDG na katalog, który na pewno nie ma ralph/config.toml
    let prev = unsafe { set_xdg("/tmp/ralph-nonexistent-global-xyz") };

    let result = load_merged_config(&PathBuf::from("/tmp/ralph-nonexistent-local-xyz.toml"));

    unsafe { restore_xdg(prev) };

    let config = result.expect("Brak obu konfiguracji powinien zwrócić Ok(default)");

    // Wszystkie wartości muszą być domyślne
    assert!(
        config.prompt.prefix.is_none(),
        "prompt.prefix powinien być None (default)"
    );
    assert!(
        config.prompt.suffix.is_none(),
        "prompt.suffix powinien być None (default)"
    );
    assert!(
        config.ui.nerd_font,
        "ui.nerd_font powinien być true (default)"
    );
    assert_eq!(
        config.task.orchestrate.workers, 2,
        "workers powinien być 2 (default)"
    );
    assert_eq!(
        config.task.orchestrate.max_retries, 3,
        "max_retries powinien być 3 (default)"
    );
    assert!(
        config.task.orchestrate.verify_commands.is_empty(),
        "verify_commands powinien być pusty (default)"
    );
    assert!(
        config.task.orchestrate.default_model.is_none(),
        "default_model powinien być None (default)"
    );
}

// ── 2. Global overrides defaults ────────────────────────────────────────────

#[test]
fn test_cascade_global_overrides_defaults() {
    let _lock = ENV_LOCK.lock().unwrap();

    let tmp_dir = std::env::temp_dir().join("ralph-cascade-global-only");
    let xdg_root = create_global_config(
        &tmp_dir,
        r#"
[prompt]
prefix = "GlobalPrefix"
suffix = "GlobalSuffix"

[ui]
nerd_font = false

[task.orchestrate]
workers = 8
default_model = "opus"
verify_commands = ["cargo test"]
"#,
    );

    let prev = unsafe { set_xdg(xdg_root.to_str().unwrap()) };
    let result = load_merged_config(&PathBuf::from("/tmp/ralph-nonexistent-local-xyz.toml"));
    unsafe { restore_xdg(prev) };
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let config = result.expect("Global config powinien się załadować bez błędu");

    // Global nadpisuje defaults
    assert_eq!(
        config.prompt.prefix.as_deref(),
        Some("GlobalPrefix"),
        "prefix powinien być z globalnego configa"
    );
    assert_eq!(
        config.prompt.suffix.as_deref(),
        Some("GlobalSuffix"),
        "suffix powinien być z globalnego configa"
    );
    assert!(
        !config.ui.nerd_font,
        "nerd_font powinien być false (global)"
    );
    assert_eq!(config.task.orchestrate.workers, 8, "workers z global");
    assert_eq!(
        config.task.orchestrate.default_model.as_deref(),
        Some("opus"),
        "default_model z global"
    );
    assert_eq!(
        config.task.orchestrate.verify_commands.len(),
        1,
        "verify_commands z global"
    );
}

// ── 3. Local overrides global ────────────────────────────────────────────────

#[test]
fn test_cascade_local_overrides_global() {
    let _lock = ENV_LOCK.lock().unwrap();

    let tmp_dir = std::env::temp_dir().join("ralph-cascade-local-global");
    let xdg_root = create_global_config(
        &tmp_dir,
        r#"
[prompt]
prefix = "GlobalPrefix"
suffix = "GlobalSuffix"

[task.orchestrate]
workers = 6
default_model = "opus"
"#,
    );

    let local_dir = tmp_dir.join("project");
    std::fs::create_dir_all(&local_dir).unwrap();
    let local_path = create_local_config(
        &local_dir,
        r#"
[prompt]
prefix = "LocalPrefix"

[task.orchestrate]
workers = 3
"#,
    );

    let prev = unsafe { set_xdg(xdg_root.to_str().unwrap()) };
    let result = load_merged_config(&local_path);
    unsafe { restore_xdg(prev) };
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let config = result.expect("Cascading merge powinien się powieść");

    // Local nadpisuje global
    assert_eq!(
        config.prompt.prefix.as_deref(),
        Some("LocalPrefix"),
        "prefix: local nadpisuje global"
    );
    // Global zachowane gdy local nie ustawia
    assert_eq!(
        config.prompt.suffix.as_deref(),
        Some("GlobalSuffix"),
        "suffix: z global (local nie ustawia)"
    );
    assert_eq!(
        config.task.orchestrate.workers, 3,
        "workers: local=3 nadpisuje global=6"
    );
    assert_eq!(
        config.task.orchestrate.default_model.as_deref(),
        Some("opus"),
        "default_model: z global (local nie ustawia)"
    );
}

// ── 4. Cascading merge: wszystkie 3 warstwy ──────────────────────────────────

#[test]
fn test_cascade_all_three_layers() {
    let _lock = ENV_LOCK.lock().unwrap();

    let tmp_dir = std::env::temp_dir().join("ralph-cascade-all-three");

    // Global: nadpisuje defaults
    let xdg_root = create_global_config(
        &tmp_dir,
        r#"
[prompt]
prefix = "global-prefix"

[ui]
nerd_font = false

[task.orchestrate]
workers = 8
max_retries = 5
default_model = "opus"
verify_commands = ["cargo test"]
"#,
    );

    // Local: nadpisuje global (tylko część pól)
    let local_dir = tmp_dir.join("project");
    std::fs::create_dir_all(&local_dir).unwrap();
    let local_path = create_local_config(
        &local_dir,
        r#"
[prompt]
suffix = "local-suffix"

[task.orchestrate]
workers = 3
verify_commands = ["npm test", "npm lint"]
"#,
    );

    let prev = unsafe { set_xdg(xdg_root.to_str().unwrap()) };
    let result = load_merged_config(&local_path);
    unsafe { restore_xdg(prev) };
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let config = result.expect("3-warstwowy cascade powinien się powieść");

    // Warstwa 1 → 2 → 3:
    // prefix: defaults=None → global="global-prefix" → local nie zmienia → "global-prefix"
    assert_eq!(
        config.prompt.prefix.as_deref(),
        Some("global-prefix"),
        "prefix z global (local nie nadpisuje)"
    );
    // suffix: defaults=None → global nie ustawia → local="local-suffix"
    assert_eq!(
        config.prompt.suffix.as_deref(),
        Some("local-suffix"),
        "suffix z local"
    );
    // nerd_font: defaults=true → global=false → local nie zmienia → false
    assert!(!config.ui.nerd_font, "nerd_font z global (false)");
    // workers: defaults=2 → global=8 → local=3 → 3
    assert_eq!(
        config.task.orchestrate.workers, 3,
        "workers: local=3 wygrywa"
    );
    // max_retries: defaults=3 → global=5 → local nie zmienia → 5
    assert_eq!(
        config.task.orchestrate.max_retries, 5,
        "max_retries z global (local nie nadpisuje)"
    );
    // default_model: defaults=None → global=Some("opus") → local nie zmienia → "opus"
    assert_eq!(
        config.task.orchestrate.default_model.as_deref(),
        Some("opus"),
        "default_model z global"
    );
    // verify_commands: local niepusty → zastępuje global w całości
    assert_eq!(
        config.task.orchestrate.verify_commands.len(),
        2,
        "verify_commands z local (zastępuje global)"
    );
    assert_eq!(
        config.task.orchestrate.verify_commands[0].command(),
        "npm test"
    );
}

// ── 5. Includes: kolejność mergowania ────────────────────────────────────────

#[test]
fn test_cascade_with_includes_merge_order() {
    let _lock = ENV_LOCK.lock().unwrap();

    // Brak global — testujemy tylko includes w local
    let prev = unsafe { set_xdg("/tmp/ralph-nonexistent-global-includes") };

    let tmp_dir = std::env::temp_dir().join("ralph-cascade-includes-order");
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // first.toml: ustala workers i prefix
    std::fs::write(
        tmp_dir.join("first.toml"),
        r#"
[task.orchestrate]
workers = 4

[prompt]
prefix = "first-prefix"
"#,
    )
    .unwrap();

    // second.toml: nadpisuje workers (last wins)
    std::fs::write(
        tmp_dir.join("second.toml"),
        r#"
[task.orchestrate]
workers = 6
"#,
    )
    .unwrap();

    let local_path = create_local_config(
        &tmp_dir,
        r#"
includes = ["first.toml", "second.toml"]

[prompt]
suffix = "base-suffix"
"#,
    );

    let result = load_merged_config(&local_path);
    unsafe { restore_xdg(prev) };
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let config = result.expect("Config z includes powinien się załadować");

    // second.toml wygrywa nad first.toml (last wins w includes)
    assert_eq!(
        config.task.orchestrate.workers, 6,
        "workers: second.toml wygrywa (last wins)"
    );
    // prefix z first.toml (second nie ustawia)
    assert_eq!(
        config.prompt.prefix.as_deref(),
        Some("first-prefix"),
        "prefix z first.toml"
    );
    // suffix z bazy (includes nie ustawiają)
    assert_eq!(
        config.prompt.suffix.as_deref(),
        Some("base-suffix"),
        "suffix z bazy config"
    );
}

// ── 5b. Includes: brakujący plik nie powoduje błędu ─────────────────────────

#[test]
fn test_cascade_with_missing_include_is_ok() {
    let _lock = ENV_LOCK.lock().unwrap();
    let prev = unsafe { set_xdg("/tmp/ralph-nonexistent-global-miss") };

    let tmp_dir = std::env::temp_dir().join("ralph-cascade-missing-include");
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let local_path = create_local_config(
        &tmp_dir,
        r#"
includes = ["nonexistent.toml"]

[prompt]
prefix = "BasePrefix"
"#,
    );

    let result = load_merged_config(&local_path);
    unsafe { restore_xdg(prev) };
    let _ = std::fs::remove_dir_all(&tmp_dir);

    // Brakujący include → ostrzeżenie na stderr, nie błąd
    assert!(
        result.is_ok(),
        "Brakujący include nie powinien powodować Err"
    );
    let config = result.unwrap();
    assert_eq!(
        config.prompt.prefix.as_deref(),
        Some("BasePrefix"),
        "Base config zachowany mimo brakującego include"
    );
}

// ── 6. Missing global config → defaults ─────────────────────────────────────

#[test]
fn test_cascade_missing_global_uses_defaults() {
    let _lock = ENV_LOCK.lock().unwrap();
    // Wskaż XDG na katalog bez ralph/config.toml
    let prev = unsafe { set_xdg("/tmp/ralph-no-global-config") };

    let tmp_dir = std::env::temp_dir().join("ralph-cascade-no-global");
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let local_path = create_local_config(
        &tmp_dir,
        r#"
[prompt]
prefix = "LocalOnly"
"#,
    );

    let result = load_merged_config(&local_path);
    unsafe { restore_xdg(prev) };
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let config = result.expect("Brak global config nie powinien powodować błędu");

    // Local nadpisuje defaults (global = brak → defaults)
    assert_eq!(
        config.prompt.prefix.as_deref(),
        Some("LocalOnly"),
        "prefix z local"
    );
    // Reszta z defaults
    assert!(
        config.ui.nerd_font,
        "nerd_font z defaults (true), gdy brak global"
    );
    assert_eq!(
        config.task.orchestrate.workers, 2,
        "workers z defaults (2), gdy brak global"
    );
}

// ── 7. Backward compatibility: istniejący .ralph.toml bez nowych pól ─────────

#[test]
fn test_cascade_backward_compat_existing_ralph_toml() {
    let _lock = ENV_LOCK.lock().unwrap();
    // Brak globalnego config
    let prev = unsafe { set_xdg("/tmp/ralph-compat-no-global") };

    let tmp_dir = std::env::temp_dir().join("ralph-cascade-compat");
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Stary format .ralph.toml bez nowych pól (includes, tui, profiles)
    let local_path = create_local_config(
        &tmp_dir,
        r#"
[prompt]
prefix = "My prefix"
suffix = "My suffix"

[task.orchestrate]
workers = 4
max_retries = 2
verify_commands = ["cargo test", "cargo clippy"]
"#,
    );

    let result = load_merged_config(&local_path);
    unsafe { restore_xdg(prev) };
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let config = result.expect("Stary format .ralph.toml powinien działać bez błędu");

    // Istniejące pola nadal działają
    assert_eq!(config.prompt.prefix.as_deref(), Some("My prefix"));
    assert_eq!(config.prompt.suffix.as_deref(), Some("My suffix"));
    assert_eq!(config.task.orchestrate.workers, 4);
    assert_eq!(config.task.orchestrate.max_retries, 2);
    assert_eq!(config.task.orchestrate.verify_commands.len(), 2);

    // Nowe pola mają wartości domyślne (backward compat)
    assert!(
        config.includes.is_empty(),
        "includes powinno być puste (backward compat)"
    );
    assert!(
        config.task.orchestrate.profiles.is_empty(),
        "profiles powinno być puste (backward compat)"
    );
    assert!(
        config.ui.nerd_font,
        "nerd_font powinno być true (default, backward compat)"
    );
    assert!(
        config.task.orchestrate.default_model.is_none(),
        "default_model powinno być None (default, backward compat)"
    );
}

#[test]
fn test_cascade_backward_compat_empty_ralph_toml() {
    let _lock = ENV_LOCK.lock().unwrap();
    let prev = unsafe { set_xdg("/tmp/ralph-compat-empty-no-global") };

    let tmp_dir = std::env::temp_dir().join("ralph-cascade-compat-empty");
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Całkowicie pusty .ralph.toml
    let local_path = create_local_config(&tmp_dir, "");

    let result = load_merged_config(&local_path);
    unsafe { restore_xdg(prev) };
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let config = result.expect("Pusty .ralph.toml powinien działać bez błędu");

    // Wszystkie wartości domyślne
    assert!(config.prompt.prefix.is_none());
    assert!(config.prompt.suffix.is_none());
    assert!(config.ui.nerd_font);
    assert_eq!(config.task.orchestrate.workers, 2);
}

// ── 8. XDG_CONFIG_HOME env override ─────────────────────────────────────────

#[test]
fn test_cascade_xdg_config_home_override() {
    let _lock = ENV_LOCK.lock().unwrap();

    // Utwórz globalny config w katalogu wskazanym przez XDG_CONFIG_HOME
    let tmp_dir = std::env::temp_dir().join("ralph-cascade-xdg");
    let xdg_root = create_global_config(
        &tmp_dir,
        r#"
[prompt]
prefix = "XdgGlobalPrefix"

[task.orchestrate]
workers = 7
"#,
    );

    let prev = unsafe { set_xdg(xdg_root.to_str().unwrap()) };
    let result = load_merged_config(&PathBuf::from("/tmp/ralph-nonexistent-local.toml"));
    unsafe { restore_xdg(prev) };
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let config = result.expect("XDG override powinien załadować global config");

    // Globalny config załadowany z katalogu XDG_CONFIG_HOME
    assert_eq!(
        config.prompt.prefix.as_deref(),
        Some("XdgGlobalPrefix"),
        "prefix z global config załadowanego przez XDG_CONFIG_HOME"
    );
    assert_eq!(
        config.task.orchestrate.workers, 7,
        "workers z global config załadowanego przez XDG_CONFIG_HOME"
    );
}

#[test]
fn test_cascade_xdg_config_home_empty_falls_back() {
    let _lock = ENV_LOCK.lock().unwrap();

    // Pusty XDG_CONFIG_HOME → fallback do ~/.config/ralph (który pewnie nie ma configa)
    // Test weryfikuje, że pusty XDG nie powoduje błędu
    let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();
    let original_home = std::env::var("HOME").ok();

    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", "");
        std::env::set_var("HOME", "/tmp/ralph-fake-home-xyz");
    }

    // ~/.config/ralph/config.toml nie istnieje dla fake HOME → graceful fallback
    let result = load_merged_config(&PathBuf::from("/tmp/ralph-nonexistent-local.toml"));

    unsafe {
        restore_xdg(original_xdg);
        match original_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    let config = result.expect("Pusty XDG + brak local → Ok(default)");

    // Wszystkie wartości domyślne
    assert!(config.prompt.prefix.is_none());
    assert!(config.ui.nerd_font);
    assert_eq!(config.task.orchestrate.workers, 2);
}

// ── Dodatkowe testy FileConfig::merge (unit-level) ───────────────────────────

#[test]
fn test_file_config_merge_defaults_only_is_default() {
    // Merge dwóch defaults → wynik identyczny z FileConfig::default()
    let merged = FileConfig::merge(FileConfig::default(), FileConfig::default());

    assert!(merged.prompt.prefix.is_none());
    assert!(merged.prompt.suffix.is_none());
    assert!(merged.prompt.system.is_none());
    assert!(merged.ui.nerd_font);
    assert_eq!(merged.task.orchestrate.workers, 2);
    assert_eq!(merged.task.orchestrate.max_retries, 3);
    assert!(merged.task.orchestrate.verify_commands.is_empty());
    assert!(merged.task.orchestrate.default_model.is_none());
    assert!(merged.includes.is_empty());
}

#[test]
fn test_file_config_merge_global_into_defaults() {
    // Merge: defaults → global (tylko global ustawia wartości)
    let defaults = FileConfig::default();
    let global: FileConfig = toml::from_str(
        r#"
[prompt]
prefix = "G-prefix"
system = "G-system"

[task.orchestrate]
workers = 5
default_model = "sonnet"
"#,
    )
    .unwrap();

    let merged = FileConfig::merge(defaults, global);

    assert_eq!(merged.prompt.prefix.as_deref(), Some("G-prefix"));
    assert_eq!(merged.prompt.system.as_deref(), Some("G-system"));
    assert!(merged.prompt.suffix.is_none()); // global nie ustawia
    assert_eq!(merged.task.orchestrate.workers, 5);
    assert_eq!(
        merged.task.orchestrate.default_model.as_deref(),
        Some("sonnet")
    );
    assert_eq!(merged.task.orchestrate.max_retries, 3); // default
}

#[test]
fn test_file_config_merge_local_into_global() {
    // Merge: global → local (local nadpisuje tylko część pól)
    let global: FileConfig = toml::from_str(
        r#"
[prompt]
prefix = "G-prefix"
suffix = "G-suffix"

[task.orchestrate]
workers = 6
max_retries = 5
default_model = "opus"
"#,
    )
    .unwrap();

    let local: FileConfig = toml::from_str(
        r#"
[prompt]
prefix = "L-prefix"

[task.orchestrate]
workers = 2
"#,
    )
    .unwrap();

    let merged = FileConfig::merge(global, local);

    // Local nadpisuje
    assert_eq!(merged.prompt.prefix.as_deref(), Some("L-prefix"));
    // Global zachowane gdy local nie zmienia
    assert_eq!(merged.prompt.suffix.as_deref(), Some("G-suffix"));
    // workers: local=2 (default) → NIE nadpisuje global=6, bo merge_scalar zachowuje base gdy overlay=default
    // To jest prawidłowe zachowanie merge_scalar: overlay=2 (default) → base=6 zachowane
    assert_eq!(merged.task.orchestrate.workers, 6);
    assert_eq!(merged.task.orchestrate.max_retries, 5);
    assert_eq!(
        merged.task.orchestrate.default_model.as_deref(),
        Some("opus")
    );
}

#[test]
fn test_file_config_merge_three_layers_all_values() {
    // Pełny 3-warstwowy merge: defaults → global → local
    let defaults = FileConfig::default();

    let global: FileConfig = toml::from_str(
        r#"
[prompt]
prefix = "G-prefix"

[ui]
nerd_font = false

[task.orchestrate]
workers = 8
max_retries = 5
default_model = "opus"
verify_commands = ["cargo test"]
"#,
    )
    .unwrap();

    let local: FileConfig = toml::from_str(
        r#"
[prompt]
suffix = "L-suffix"

[task.orchestrate]
workers = 3
verify_commands = ["npm test", "npm lint"]
"#,
    )
    .unwrap();

    let after_global = FileConfig::merge(defaults, global);
    let merged = FileConfig::merge(after_global, local);

    // defaults → global → local
    assert_eq!(
        merged.prompt.prefix.as_deref(),
        Some("G-prefix"),
        "prefix z global (local nie zmienia)"
    );
    assert_eq!(
        merged.prompt.suffix.as_deref(),
        Some("L-suffix"),
        "suffix z local"
    );
    assert!(merged.prompt.system.is_none(), "system z defaults (None)");
    assert!(!merged.ui.nerd_font, "nerd_font z global (false)");
    assert_eq!(merged.task.orchestrate.workers, 3, "workers z local");
    assert_eq!(
        merged.task.orchestrate.max_retries, 5,
        "max_retries z global"
    );
    assert_eq!(
        merged.task.orchestrate.default_model.as_deref(),
        Some("opus"),
        "default_model z global"
    );
    assert_eq!(
        merged.task.orchestrate.verify_commands.len(),
        2,
        "verify_commands z local (zastępuje global)"
    );
}
