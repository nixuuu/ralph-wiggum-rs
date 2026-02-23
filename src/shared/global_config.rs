//! Global configuration management for ralph-wiggum.
//!
//! Obsługuje ładowanie konfiguracji z lokalizacji XDG:
//! - `$XDG_CONFIG_HOME/ralph/config.toml` jeśli `XDG_CONFIG_HOME` ustawiony
//! - `~/.config/ralph/config.toml` jako domyślna ścieżka

use std::path::PathBuf;

use crate::shared::{
    error::{RalphError, Result},
    file_config::FileConfig,
};

/// Zwraca katalog konfiguracji globalnej ralph.
///
/// Priorytet:
/// 1. `$XDG_CONFIG_HOME/ralph/` jeśli `XDG_CONFIG_HOME` jest ustawiony i niepusty
/// 2. `~/.config/ralph/` jako fallback
pub fn resolve_global_config_dir() -> PathBuf {
    // Sprawdź XDG_CONFIG_HOME (ignoruj pustą wartość)
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("ralph");
    }

    // Fallback: ~/.config/ralph
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("~"));

    home.join(".config").join("ralph")
}

/// Zwraca pełną ścieżkę do globalnego pliku konfiguracyjnego.
///
/// Wynik: `<config_dir>/config.toml`
pub fn resolve_global_config_path() -> PathBuf {
    resolve_global_config_dir().join("config.toml")
}

/// Tworzy katalog konfiguracji globalnej jeśli nie istnieje.
///
/// Idempotentna — nie zwraca błędu jeśli katalog już istnieje.
pub fn ensure_global_config_dir() -> Result<PathBuf> {
    let dir = resolve_global_config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| {
        RalphError::Config(format!(
            "Nie można utworzyć katalogu konfiguracji {}: {}",
            dir.display(),
            e
        ))
    })?;
    Ok(dir)
}

/// Ładuje globalną konfigurację z pliku `config.toml`.
///
/// Jeśli plik nie istnieje, zwraca `FileConfig::default()` bez błędu.
/// Jeśli plik istnieje ale jest niepoprawny TOML, zwraca błąd.
pub fn load_global_config() -> Result<FileConfig> {
    let path = resolve_global_config_path();
    FileConfig::load_from_path(&path)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Wspólny process-wide lock — zdefiniowany w crate::shared::ENV_LOCK.
    // Serializuje testy ze wszystkich modułów shared, które modyfikują XDG_CONFIG_HOME/HOME.
    use crate::shared::ENV_LOCK;

    #[test]
    fn test_resolve_global_config_path_with_xdg() {
        let _lock = ENV_LOCK.lock().unwrap();
        let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();

        // SAFETY: test jest serializowany przez ENV_LOCK, nie ma równoległości
        unsafe { std::env::set_var("XDG_CONFIG_HOME", "/tmp/test-xdg") };
        let path = resolve_global_config_path();

        // Przywróć oryginalne środowisko
        unsafe {
            match original_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }

        assert_eq!(path, PathBuf::from("/tmp/test-xdg/ralph/config.toml"));
    }

    #[test]
    fn test_resolve_global_config_dir_with_xdg() {
        let _lock = ENV_LOCK.lock().unwrap();
        let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();

        unsafe { std::env::set_var("XDG_CONFIG_HOME", "/custom/config") };
        let dir = resolve_global_config_dir();

        unsafe {
            match original_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }

        assert_eq!(dir, PathBuf::from("/custom/config/ralph"));
    }

    #[test]
    fn test_resolve_global_config_path_without_xdg_uses_home() {
        let _lock = ENV_LOCK.lock().unwrap();
        let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let original_home = std::env::var("HOME").ok();

        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::set_var("HOME", "/home/testuser");
        }

        let path = resolve_global_config_path();

        // Przywróć środowisko
        unsafe {
            match original_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match original_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }

        assert_eq!(
            path,
            PathBuf::from("/home/testuser/.config/ralph/config.toml")
        );
    }

    #[test]
    fn test_resolve_global_config_dir_without_xdg_uses_home() {
        let _lock = ENV_LOCK.lock().unwrap();
        let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let original_home = std::env::var("HOME").ok();

        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::set_var("HOME", "/home/testuser");
        }

        let dir = resolve_global_config_dir();

        unsafe {
            match original_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match original_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }

        assert_eq!(dir, PathBuf::from("/home/testuser/.config/ralph"));
    }

    #[test]
    fn test_resolve_global_config_dir_empty_xdg_falls_back_to_home() {
        let _lock = ENV_LOCK.lock().unwrap();
        let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let original_home = std::env::var("HOME").ok();

        // Pusty XDG_CONFIG_HOME traktujemy jak brak zmiennej
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", "");
            std::env::set_var("HOME", "/home/fallback");
        }

        let dir = resolve_global_config_dir();

        unsafe {
            match original_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match original_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }

        assert_eq!(dir, PathBuf::from("/home/fallback/.config/ralph"));
    }

    #[test]
    fn test_load_global_config_missing_file_returns_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();

        // Wskaż na katalog który na pewno nie zawiera config.toml
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", "/tmp/nonexistent-ralph-test-xyz");
        }

        let result = load_global_config();

        unsafe {
            match original_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }

        // Brakujący plik → domyślna konfiguracja bez błędu
        let config = result.expect("Brakujący config.toml powinien zwrócić Ok(default)");
        assert!(config.prompt.prefix.is_none());
        assert!(config.prompt.suffix.is_none());
        assert!(config.ui.nerd_font); // default = true
    }

    #[test]
    fn test_load_global_config_existing_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();

        // Utwórz tymczasowy katalog i plik config
        let tmp_dir = std::env::temp_dir().join("ralph-global-config-test");
        let ralph_dir = tmp_dir.join("ralph");
        std::fs::create_dir_all(&ralph_dir).unwrap();

        let config_path = ralph_dir.join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[prompt]
prefix = "GlobalPrefix"
suffix = "GlobalSuffix"

[ui]
nerd_font = false
"#,
        )
        .unwrap();

        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", tmp_dir.to_str().unwrap());
        }

        let result = load_global_config();

        // Cleanup
        unsafe {
            match original_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
        let _ = std::fs::remove_file(&config_path);
        let _ = std::fs::remove_dir_all(&tmp_dir);

        let config = result.expect("Istniejący config.toml powinien być załadowany");
        assert_eq!(config.prompt.prefix.as_deref(), Some("GlobalPrefix"));
        assert_eq!(config.prompt.suffix.as_deref(), Some("GlobalSuffix"));
        assert!(!config.ui.nerd_font);
    }

    #[test]
    fn test_ensure_global_config_dir_creates_directory() {
        let _lock = ENV_LOCK.lock().unwrap();
        let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();

        let tmp_dir = std::env::temp_dir().join("ralph-ensure-dir-test");
        // Usuń jeśli istnieje z poprzedniego uruchomienia
        let _ = std::fs::remove_dir_all(&tmp_dir);

        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", tmp_dir.to_str().unwrap());
        }

        let result = ensure_global_config_dir();

        // Cleanup
        unsafe {
            match original_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }

        let dir = result.expect("ensure_global_config_dir powinien utworzyć katalog");
        assert!(
            dir.exists(),
            "Katalog powinien istnieć po ensure_global_config_dir()"
        );
        assert!(dir.is_dir(), "Ścieżka powinna być katalogiem");
        assert_eq!(dir, tmp_dir.join("ralph"));

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_load_global_config_invalid_toml_returns_error() {
        let _lock = ENV_LOCK.lock().unwrap();
        let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();

        // Utwórz tymczasowy katalog z niepoprawnym TOML
        let tmp_dir = std::env::temp_dir().join("ralph-invalid-toml-test");
        let ralph_dir = tmp_dir.join("ralph");
        std::fs::create_dir_all(&ralph_dir).unwrap();

        let config_path = ralph_dir.join("config.toml");
        std::fs::write(&config_path, "[broken\ngarbage = !!!").unwrap();

        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", tmp_dir.to_str().unwrap());
        }

        let result = load_global_config();

        // Cleanup
        unsafe {
            match original_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
        let _ = std::fs::remove_file(&config_path);
        let _ = std::fs::remove_dir_all(&tmp_dir);

        // Niepoprawny TOML → błąd
        assert!(
            result.is_err(),
            "Niepoprawny TOML powinien zwrócić Err, a zwrócił Ok"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Failed to parse"),
            "Komunikat błędu powinien zawierać 'Failed to parse', a zawiera: {err}"
        );
    }

    #[test]
    fn test_ensure_global_config_dir_idempotent() {
        let _lock = ENV_LOCK.lock().unwrap();
        let original_xdg = std::env::var("XDG_CONFIG_HOME").ok();

        let tmp_dir = std::env::temp_dir().join("ralph-idempotent-test");
        let ralph_dir = tmp_dir.join("ralph");
        std::fs::create_dir_all(&ralph_dir).unwrap();

        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", tmp_dir.to_str().unwrap());
        }

        // Wywołaj dwa razy — oba powinny się powieść
        let result1 = ensure_global_config_dir();
        let result2 = ensure_global_config_dir();

        unsafe {
            match original_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);

        assert!(result1.is_ok(), "Pierwsze wywołanie powinno się powieść");
        assert!(result2.is_ok(), "Drugie wywołanie powinno być idempotentne");
    }
}
