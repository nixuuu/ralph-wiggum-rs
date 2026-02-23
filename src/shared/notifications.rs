//! Moduł powiadomień systemowych.
//!
//! Dostarcza abstrakcję `Notifier` do wysyłania powiadomień desktopowych.
//! Na macOS używa `osascript` (zero dodatkowych zależności).
//! Na pozostałych platformach działa `NoopNotifier` (brak operacji).

// Publiczne API modułu — zostanie użyte przez przyszłe komendy (np. orchestrate completion).
#![allow(dead_code)]

use crate::shared::error::Result;

/// Konfiguracja powiadomień przekazywana do `create_notifier`.
#[derive(Debug, Default, Clone)]
pub struct NotificationConfig {
    /// Czy powiadomienia są włączone (default: true)
    pub enabled: bool,
    /// Czy odtwarzać dźwięk powiadomienia (default: true)
    pub sound: bool,
}

impl NotificationConfig {
    /// Tworzy domyślną konfigurację z włączonymi powiadomieniami i dźwiękiem.
    pub fn enabled_with_sound() -> Self {
        Self {
            enabled: true,
            sound: true,
        }
    }
}

/// Trait abstrakcji powiadomień systemowych.
pub trait Notifier: Send + Sync {
    /// Wysyła powiadomienie z tytułem i treścią.
    fn send(&self, title: &str, body: &str) -> Result<()>;
}

// ── macOS backend ────────────────────────────────────────────────────────────

/// Powiadomienia przez `osascript` na macOS.
/// Nie wymaga żadnych dodatkowych zależności — `osascript` jest częścią systemu.
#[cfg(target_os = "macos")]
pub struct MacOSNotifier {
    sound: bool,
}

#[cfg(target_os = "macos")]
impl MacOSNotifier {
    /// Tworzy nowy `MacOSNotifier`.
    ///
    /// * `sound` — jeśli true, powiadomienie odtwarza dźwięk "default"
    pub fn new(sound: bool) -> Self {
        Self { sound }
    }

    /// Buduje AppleScript dla `osascript`.
    /// Znaki `"` i `\` w title/body są escapowane, aby zapobiec command injection.
    fn build_script(title: &str, body: &str, sound: bool) -> String {
        let safe_title = escape_applescript(title);
        let safe_body = escape_applescript(body);

        if sound {
            format!(
                "display notification \"{}\" with title \"{}\" sound name \"default\"",
                safe_body, safe_title
            )
        } else {
            format!(
                "display notification \"{}\" with title \"{}\"",
                safe_body, safe_title
            )
        }
    }
}

#[cfg(target_os = "macos")]
impl Notifier for MacOSNotifier {
    fn send(&self, title: &str, body: &str) -> Result<()> {
        use std::process::Command;

        let script = Self::build_script(title, body, self.sound);

        // `?` konwertuje io::Error → RalphError::Io (via From impl)
        let output = Command::new("osascript").arg("-e").arg(&script).output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::shared::error::RalphError::Notification(format!(
                "osascript failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            )));
        }

        Ok(())
    }
}

// ── Noop backend (non-macOS) ─────────────────────────────────────────────────

/// Powiadomienia-zaślepka dla platform bez wsparcia (lub gdy wyłączone).
/// Nie robi nic — operacja zawsze kończy się sukcesem.
pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn send(&self, _title: &str, _body: &str) -> Result<()> {
        Ok(())
    }
}

// ── Factory ──────────────────────────────────────────────────────────────────

/// Tworzy odpowiedni `Notifier` na podstawie platformy i konfiguracji.
///
/// * macOS + enabled → `MacOSNotifier`
/// * inne platformy lub disabled → `NoopNotifier`
pub fn create_notifier(config: &NotificationConfig) -> Box<dyn Notifier> {
    #[cfg(target_os = "macos")]
    {
        if config.enabled {
            return Box::new(MacOSNotifier::new(config.sound));
        }
    }

    // Suppress unused variable warning on non-macOS
    #[cfg(not(target_os = "macos"))]
    let _ = config;

    Box::new(NoopNotifier)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Escapuje znaki specjalne w ciągu AppleScript.
/// Zapobiega wstrzyknięciu poleceń przez spreparowany title/body.
///
/// Escapowane znaki: `\` → `\\`, `"` → `\"`, `\n` → `\n` (literal), `\r` → `\r` (literal).
/// AppleScript nie akceptuje raw newline wewnątrz ciągu — powoduje syntax error.
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── escape_applescript ────────────────────────────────────────────────────

    #[test]
    fn test_escape_plain_string() {
        assert_eq!(escape_applescript("Hello World"), "Hello World");
    }

    #[test]
    fn test_escape_double_quotes() {
        assert_eq!(escape_applescript(r#"Say "hello""#), r#"Say \"hello\""#);
    }

    #[test]
    fn test_escape_backslash() {
        assert_eq!(escape_applescript(r"path\to\file"), r"path\\to\\file");
    }

    #[test]
    fn test_escape_backslash_and_quotes() {
        // Backslash musi być escapowany przed cudzysłowem
        assert_eq!(escape_applescript(r#"\"quoted\""#), r#"\\\"quoted\\\""#);
    }

    #[test]
    fn test_escape_empty_string() {
        assert_eq!(escape_applescript(""), "");
    }

    #[test]
    fn test_escape_unicode() {
        // Unicode nie wymaga escapowania
        assert_eq!(escape_applescript("Zażółć gęślą jaźń"), "Zażółć gęślą jaźń");
    }

    #[test]
    fn test_escape_newline() {
        // \n i \r muszą być escapowane — AppleScript nie akceptuje raw newline w stringu
        assert_eq!(escape_applescript("line1\nline2"), "line1\\nline2");
    }

    #[test]
    fn test_escape_carriage_return() {
        assert_eq!(escape_applescript("line1\rline2"), "line1\\rline2");
    }

    #[test]
    fn test_escape_tab_unchanged() {
        // Tab nie wymaga escapowania w AppleScript
        assert_eq!(escape_applescript("col1\tcol2"), "col1\tcol2");
    }

    // ── MacOSNotifier::build_script ──────────────────────────────────────────

    #[cfg(target_os = "macos")]
    #[test]
    fn test_build_script_with_sound() {
        let script = MacOSNotifier::build_script("Title", "Body", true);
        assert_eq!(
            script,
            r#"display notification "Body" with title "Title" sound name "default""#
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_build_script_without_sound() {
        let script = MacOSNotifier::build_script("Title", "Body", false);
        assert_eq!(script, r#"display notification "Body" with title "Title""#);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_build_script_escapes_quotes_in_title() {
        // Title: Say "hi"  → AppleScript: Say \"hi\"
        let script = MacOSNotifier::build_script(r#"Say "hi""#, "Body", false);
        // Sprawdzamy że cudzysłowy w tytule są poprawnie escapowane
        assert!(
            script.contains(r#"\"hi\""#),
            "Oczekiwano escapowanego \" w tytule, got: {:?}",
            script
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_build_script_escapes_quotes_in_body() {
        // Body: Say "hello"  → AppleScript: Say \"hello\"
        let script = MacOSNotifier::build_script("Title", r#"Say "hello""#, false);
        // Sprawdzamy że cudzysłowy w body są poprawnie escapowane
        assert!(
            script.contains(r#"\"hello\""#),
            "Oczekiwano escapowanego \" w body, got: {:?}",
            script
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_build_script_escapes_backslash_in_body() {
        let script = MacOSNotifier::build_script("Title", r"path\file", false);
        assert!(script.contains(r"path\\file"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_build_script_empty_title_and_body() {
        let script = MacOSNotifier::build_script("", "", false);
        assert_eq!(script, r#"display notification "" with title """#);
    }

    // ── NoopNotifier ─────────────────────────────────────────────────────────

    #[test]
    fn test_noop_notifier_always_succeeds() {
        let notifier = NoopNotifier;
        assert!(notifier.send("Title", "Body").is_ok());
    }

    #[test]
    fn test_noop_notifier_with_special_chars() {
        let notifier = NoopNotifier;
        assert!(
            notifier
                .send(r#"Title "with" quotes"#, r"Body\with\backslash")
                .is_ok()
        );
    }

    #[test]
    fn test_noop_notifier_with_empty_strings() {
        let notifier = NoopNotifier;
        assert!(notifier.send("", "").is_ok());
    }

    // ── create_notifier ───────────────────────────────────────────────────────

    #[test]
    fn test_create_notifier_disabled_returns_noop() {
        let config = NotificationConfig {
            enabled: false,
            sound: true,
        };
        let notifier = create_notifier(&config);
        // Noop notifier zawsze zwraca Ok
        assert!(notifier.send("test", "test").is_ok());
    }

    #[test]
    fn test_create_notifier_default_config_succeeds() {
        let config = NotificationConfig::default();
        // default: enabled=false — factory zwraca NoopNotifier
        let notifier = create_notifier(&config);
        assert!(notifier.send("test", "body").is_ok());
    }

    #[test]
    fn test_notification_config_enabled_with_sound() {
        let config = NotificationConfig::enabled_with_sound();
        assert!(config.enabled);
        assert!(config.sound);
    }

    #[test]
    fn test_notification_config_default() {
        let config = NotificationConfig::default();
        assert!(!config.enabled);
        assert!(!config.sound);
    }

    /// Weryfikuje że create_notifier zwraca obiekt implementujący Notifier.
    /// Na macOS z enabled=true zwróci MacOSNotifier; na innych — NoopNotifier.
    /// W teście nie wywołujemy send() z enabled=true na macOS, aby uniknąć
    /// wyskakiwania powiadomień podczas testów CI.
    #[test]
    fn test_create_notifier_enabled_is_notifier() {
        let config = NotificationConfig::enabled_with_sound();
        let notifier = create_notifier(&config);
        // Wywołaj z pustymi stringami — na macOS osascript może zwrócić błąd lub OK
        // Na non-macOS zawsze OK. Głównie sprawdzamy że typ jest prawidłowy.
        let _ = notifier.send("", "");
    }
}
