pub mod attachments;
pub mod banner;
pub mod dag;
pub mod diagnostics;
pub mod error;
pub mod file_config;
pub mod global_config;
pub mod icons;
pub mod markdown;
pub mod mcp;
pub mod notifications;
pub mod progress;
pub mod tasks;

#[cfg(test)]
mod tests_backward_compat_profiles;

/// Process-wide lock dla testów modyfikujących zmienne środowiskowe (XDG_CONFIG_HOME, HOME).
///
/// Używany we wszystkich modułach testowych w `shared`, które manipulują env vars,
/// aby zapobiec race condition przy `cargo test` z domyślną wielowątkowością.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests_cascade_config;
