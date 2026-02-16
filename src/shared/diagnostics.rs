//! Diagnostyczny file-logger dla ralph-wiggum.
//!
//! **Problem**: Wszystkie `eprintln!()` w kodzie piszą na stderr, co psuje TUI (ratatui).
//! Użytkownik nie widzi tych komunikatów w dashboardzie, a jednocześnie zaburzają one rendering.
//!
//! **Rozwiązanie**: Globalny file-logger z dwoma poziomami:
//! - `warn!()` — zawsze logowane (błędy parsowania, timeouty, cleanup failures)
//! - `debug!()` — logowane tylko gdy `--debug` flag jest aktywna (verbose info)
//!
//! # Przykład użycia
//!
//! ```no_run
//! use ralph_wiggum::shared::diagnostics;
//! use ralph_wiggum::{diag_warn, diag_debug};
//!
//! // Inicjalizacja loggera na początku programu
//! diagnostics::init(std::path::Path::new(".ralph/logs"), true)?;
//!
//! // Użycie makr
//! let filename = "test.rs";
//! diag_warn!("Parser error: unexpected token");
//! diag_debug!("Processing file: {}", "test.rs");
//!
//! // Pobranie ścieżki do aktywnego pliku logu
//! if let Some(path) = diagnostics::log_file_path() {
//!     println!("Log zapisany w: {}", path.display());
//! }
//! # Ok::<(), ralph_wiggum::shared::error::RalphError>(())
//! ```

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use crate::shared::error::{RalphError, Result};

/// Globalny singleton dla diagnostycznego file-loggera.
/// Jeśli `None`, logowanie jest no-op (graceful degradation).
#[cfg_attr(not(test), allow(dead_code))]
static DIAG_LOGGER: LazyLock<Mutex<Option<DiagFile>>> = LazyLock::new(|| Mutex::new(None));

/// Wewnętrzna struktura przechowująca buforowany writer do pliku logu.
#[cfg_attr(not(test), allow(dead_code))]
struct DiagFile {
    /// Buforowany writer do pliku logu
    writer: BufWriter<File>,
    /// Ścieżka do pliku logu
    path: PathBuf,
    /// Czy tryb debug jest włączony
    debug_enabled: bool,
    /// Licznik linii debug (flush co 100 linii)
    debug_line_count: usize,
}

impl DiagFile {
    /// Tworzy nowy DiagFile z otwartym plikiem.
    #[cfg_attr(not(test), allow(dead_code))]
    fn new(path: PathBuf, file: File, debug_enabled: bool) -> Self {
        Self {
            writer: BufWriter::new(file),
            path,
            debug_enabled,
            debug_line_count: 0,
        }
    }

    /// Zapisuje linię logu z timestampem i poziomem.
    #[cfg_attr(not(test), allow(dead_code))]
    fn write_line(&mut self, level: &str, msg: &str) -> std::io::Result<()> {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        writeln!(self.writer, "[{}] [{}] {}", timestamp, level, msg)?;
        Ok(())
    }

    /// Zapisuje warn (zawsze flush).
    #[cfg_attr(not(test), allow(dead_code))]
    fn log_warn(&mut self, msg: &str) {
        if let Err(e) = self.write_line("WARN", msg) {
            // Fallback na stderr jeśli zapis do pliku się nie powiedzie
            eprintln!("Failed to write warn log: {}", e);
            return;
        }
        // Flush po każdym warnie
        let _ = self.writer.flush();
    }

    /// Zapisuje debug (buforowany, flush co 100 linii).
    #[cfg_attr(not(test), allow(dead_code))]
    fn log_debug(&mut self, msg: &str) {
        if !self.debug_enabled {
            return;
        }

        if let Err(e) = self.write_line("DEBUG", msg) {
            eprintln!("Failed to write debug log: {}", e);
            return;
        }

        self.debug_line_count += 1;
        if self.debug_line_count >= 100 {
            let _ = self.writer.flush();
            self.debug_line_count = 0;
        }
    }
}

impl Drop for DiagFile {
    fn drop(&mut self) {
        // Flush remaining buffered debug logs
        let _ = self.writer.flush();
    }
}

/// Automatycznie usuwa stare pliki logów, utrzymując limit max_log_files.
///
/// Jeśli liczba plików logów w katalogu przekroczy `max_log_files`, usuwa
/// najstarsze pliki (po modification time) aż do pozostania dokładnie `max_log_files` plików.
///
/// Ignoruje 0 (nieograniczone).
///
/// # Parametry
/// - `log_dir`: katalog zawierający pliki logów
/// - `max_log_files`: maksymalna liczba plików (0 = nieograniczone)
///
/// # Nota
/// W przypadku błędu przy usuwaniu pliku (np. permission denied), funkcja
/// loguje ostrzeżenie na stderr ale nie zwraca błędu (best-effort cleanup).
fn cleanup_old_logs(log_dir: &Path, max_log_files: u32) -> Result<()> {
    if max_log_files == 0 {
        // 0 oznacza nieograniczone
        return Ok(());
    }

    let max_files = max_log_files as usize;

    // Przeszukaj katalog w poszukiwaniu .log plików
    let entries = fs::read_dir(log_dir)?;
    let mut log_files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        // Filtruj tylko .log pliki
        if path.extension().and_then(|s| s.to_str()) == Some("log")
            && let Ok(metadata) = fs::metadata(&path)
            && let Ok(modified) = metadata.modified()
        {
            log_files.push((path, modified));
        }
    }

    // Jeśli liczba plików nie przekracza limitu, nie rób nic
    if log_files.len() <= max_files {
        return Ok(());
    }

    // Posortuj malejąco po modification time (najnowsze na początku)
    log_files.sort_by(|a, b| b.1.cmp(&a.1));

    // Usuń najstarsze pliki (wszystkie poza pierwszymi max_files)
    for (path, _) in log_files.iter().skip(max_files) {
        if let Err(e) = fs::remove_file(path) {
            // Logowanie na stderr, bo logger może jeszcze nie być zainicjalizowany
            eprintln!(
                "Warning: Failed to remove old log file {}: {}",
                path.display(),
                e
            );
        }
    }

    Ok(())
}

/// Inicjalizuje diagnostyczny file-logger.
///
/// Tworzy katalog `log_dir` jeśli nie istnieje, a następnie otwiera plik logu
/// z timestampem w formacie: `ralph-YYYYMMDD-HHMMSS.log`.
///
/// Po otwarciu pliku wykonuje auto-cleanup starych logów jeśli `max_log_files` jest ustawiony.
///
/// # Parametry
/// - `log_dir`: katalog dla plików logu (np. `.ralph/logs`)
/// - `max_log_files`: maksymalna liczba plików (0 = nieograniczone)
/// - `debug`: czy włączyć logowanie poziomu DEBUG
///
/// # Błędy
/// Zwraca `RalphError::Io` jeśli nie udało się stworzyć katalogu lub otworzyć pliku.
#[cfg_attr(not(test), allow(dead_code))]
pub fn init_with_config(log_dir: &Path, max_log_files: u32, debug: bool) -> Result<()> {
    // Stwórz katalog jeśli nie istnieje
    fs::create_dir_all(log_dir)?;

    // Wygeneruj nazwę pliku z timestampem
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let filename = format!("ralph-{}.log", timestamp);
    let log_path = log_dir.join(filename);

    // Otwórz plik w trybie append
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    // Ustaw globalny singleton
    let mut logger = DIAG_LOGGER
        .lock()
        .map_err(|e| RalphError::Config(format!("Failed to lock logger: {}", e)))?;

    *logger = Some(DiagFile::new(log_path, file, debug));

    // Zwolnij lock przed cleanup (aby inne wątki mogły logować)
    drop(logger);

    // Auto-cleanup starych logów
    if let Err(e) = cleanup_old_logs(log_dir, max_log_files) {
        eprintln!("Warning: Auto-cleanup of old logs failed: {}", e);
    }

    Ok(())
}

/// Inicjalizuje diagnostyczny file-logger (legacy).
///
/// Używa domyślnych wartości: max_log_files=10.
/// Dla pełnej kontroli użyj `init_with_config()`.
///
/// # Deprecated
/// Ta funkcja jest utrzymywana dla backward compatibility.
/// Nowy kod powinien używać `init_with_config()`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn init(log_dir: &Path, debug: bool) -> Result<()> {
    init_with_config(log_dir, 10, debug)
}

/// Loguje wiadomość na poziomie WARN (zawsze zapisywana).
///
/// Użyj makra `diag_warn!()` zamiast bezpośredniego wywołania tej funkcji.
///
/// Jeśli logger nie został zainicjalizowany, funkcja jest no-op.
#[cfg_attr(not(test), allow(dead_code))]
pub fn warn(msg: &str) {
    if let Ok(mut logger) = DIAG_LOGGER.lock()
        && let Some(ref mut diag) = *logger
    {
        diag.log_warn(msg);
    }
}

/// Loguje wiadomość na poziomie DEBUG (tylko jeśli debug=true).
///
/// Użyj makra `diag_debug!()` zamiast bezpośredniego wywołania tej funkcji.
///
/// Jeśli logger nie został zainicjalizowany lub debug nie jest włączony, funkcja jest no-op.
#[cfg_attr(not(test), allow(dead_code))]
pub fn debug(msg: &str) {
    if let Ok(mut logger) = DIAG_LOGGER.lock()
        && let Some(ref mut diag) = *logger
    {
        diag.log_debug(msg);
    }
}

/// Zwraca ścieżkę do aktywnego pliku logu.
///
/// Jeśli logger nie został zainicjalizowany, zwraca `None`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn log_file_path() -> Option<PathBuf> {
    DIAG_LOGGER
        .lock()
        .ok()?
        .as_ref()
        .map(|diag| diag.path.clone())
}

#[cfg(test)]
/// Resetuje globalny logger (tylko dla testów).
///
/// UWAGA: Ta funkcja jest thread-unsafe w kontekście testów równoległych.
/// Używaj `cargo test -- --test-threads=1` dla testów diagnostics.
fn reset_logger() {
    if let Ok(mut logger) = DIAG_LOGGER.lock() {
        *logger = None;
    }
}

/// Makro do logowania wiadomości WARN.
///
/// Wspiera formatowanie jak `format!()`.
///
/// # Przykład
/// ```ignore
/// diag_warn!("Parser error at line {}", line_num);
/// ```
#[macro_export]
macro_rules! diag_warn {
    ($($arg:tt)*) => {
        $crate::shared::diagnostics::warn(&format!($($arg)*))
    };
}

/// Makro do logowania wiadomości DEBUG.
///
/// Wspiera formatowanie jak `format!()`.
///
/// # Przykład
/// ```ignore
/// diag_debug!("Processing file: {}", filename);
/// ```
#[macro_export]
macro_rules! diag_debug {
    ($($arg:tt)*) => {
        $crate::shared::diagnostics::debug(&format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Read;

    /// Pomocnicza funkcja do stworzenia tymczasowego katalogu testowego
    fn temp_log_dir() -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};

        let temp = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let unique = format!("ralph-diag-test-{}-{}", std::process::id(), nanos);
        temp.join(unique)
    }

    /// Pomocnicza funkcja do odczytania zawartości pliku logu
    fn read_log_file(path: &Path) -> std::io::Result<String> {
        let mut file = File::open(path)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        Ok(contents)
    }

    /// Force flush loggera i synchronizuj do dysku przed odczytem
    /// Używane w testach aby uniknąć race conditions
    fn flush_and_sync() {
        let mut logger = DIAG_LOGGER.lock().unwrap();
        if let Some(ref mut diag) = *logger {
            // Flush bufora
            let _ = diag.writer.flush();
            // Wymuś sync do dysku przez OS
            if let Err(e) = diag.writer.get_ref().sync_all() {
                eprintln!("Warning: flush_and_sync failed to sync: {}", e);
            }
        }
        drop(logger);
        // Krótki sleep aby zapewnić że wszystkie operacje IO się zakończyły
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    #[test]
    #[serial]
    fn test_init_creates_log_file() {
        reset_logger();
        let log_dir = temp_log_dir();

        // Inicjalizacja loggera
        let result = init(&log_dir, false);
        assert!(result.is_ok(), "init() should succeed");

        // Sprawdź czy katalog został stworzony
        assert!(log_dir.exists(), "Log directory should exist");

        // Sprawdź czy plik logu istnieje
        let log_path = log_file_path();
        assert!(log_path.is_some(), "Log file path should be set");
        assert!(
            log_path.unwrap().exists(),
            "Log file should exist after init"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    #[serial]
    fn test_warn_writes_to_file() {
        reset_logger();
        let log_dir = temp_log_dir();
        init(&log_dir, false).expect("init should succeed");

        // Użyj funkcji warn bezpośrednio
        warn("Test warning message");

        // Force flush i sync do dysku
        flush_and_sync();

        // Odczytaj plik logu
        let log_path = log_file_path().expect("Log path should be set");
        let contents = read_log_file(&log_path).expect("Should read log file");

        // Sprawdź zawartość
        assert!(contents.contains("[WARN]"), "Log should contain WARN level");
        assert!(
            contents.contains("Test warning message"),
            "Log should contain the message"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    #[serial]
    fn test_debug_writes_when_enabled() {
        reset_logger();
        let log_dir = temp_log_dir();
        init(&log_dir, true).expect("init should succeed");

        let log_path = log_file_path().expect("Log path should be set").clone();

        // Użyj funkcji debug bezpośrednio
        debug("Test debug message");

        // Force flush i sync do dysku
        flush_and_sync();

        let contents = read_log_file(&log_path).expect("Should read log file");

        // Sprawdź zawartość
        assert!(
            contents.contains("[DEBUG]"),
            "Log should contain DEBUG level"
        );
        assert!(
            contents.contains("Test debug message"),
            "Log should contain the message"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    #[serial]
    fn test_debug_silent_when_disabled() {
        reset_logger();
        let log_dir = temp_log_dir();
        init(&log_dir, false).expect("init should succeed");

        // Użyj funkcji debug bezpośrednio (debug disabled)
        debug("This should not appear");

        // Dodaj warn żeby plik nie był pusty
        warn("Marker");

        // Force flush i sync do dysku
        flush_and_sync();

        let log_path = log_file_path().expect("Log path should be set").clone();

        // Odczytaj plik logu
        let contents = read_log_file(&log_path).expect("Should read log file");

        // Sprawdź że debug nie został zapisany
        // Note: Log może zawierać [DEBUG] z poprzednich testów jeśli dzielą singleton,
        // ale nie powinien zawierać naszej konkretnej wiadomości
        assert!(
            !contents.contains("This should not appear"),
            "Log should NOT contain debug message when disabled"
        );
        assert!(
            contents.contains("Marker"),
            "Log should contain warn marker"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    #[serial]
    fn test_no_init_graceful_degradation() {
        reset_logger();
        // Nie wywołujemy init() — logger nie jest zainicjalizowany
        // Te wywołania nie powinny spowodować paniki
        warn("This is a no-op");
        debug("This is also a no-op");

        // log_file_path powinno zwrócić None
        assert!(
            log_file_path().is_none(),
            "Log path should be None before init"
        );
    }

    #[test]
    #[serial]
    fn test_log_file_path_returns_correct_path() {
        reset_logger();
        let log_dir = temp_log_dir();
        init(&log_dir, false).expect("init should succeed");

        let path = log_file_path().expect("Log path should be set");

        // Sprawdź że ścieżka zawiera timestamp w formacie ralph-YYYYMMDD-HHMMSS.log
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(
            filename.starts_with("ralph-"),
            "Filename should start with 'ralph-'"
        );
        assert!(
            filename.ends_with(".log"),
            "Filename should end with '.log'"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    #[serial]
    fn test_multiple_warns_all_flushed() {
        reset_logger();
        let log_dir = temp_log_dir();
        init(&log_dir, false).expect("init should succeed");

        let log_path = log_file_path().expect("Log path should be set").clone();

        // Zapisz kilka warnów
        warn("Warning 1");
        warn("Warning 2");
        warn("Warning 3");

        // Force flush i sync do dysku
        flush_and_sync();

        // Odczytaj plik logu
        let contents = read_log_file(&log_path).expect("Should read log file");

        // Wszystkie powinny być zapisane (warn flush po każdym)
        assert!(contents.contains("Warning 1"));
        assert!(contents.contains("Warning 2"));
        assert!(contents.contains("Warning 3"));

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    #[serial]
    fn test_init_creates_log_dir() {
        reset_logger();
        let log_dir = temp_log_dir();

        // Upewnij się że katalog nie istnieje przed testem
        if log_dir.exists() {
            let _ = fs::remove_dir_all(&log_dir);
        }

        // Inicjalizacja loggera powinna stworzyć katalog
        let result = init(&log_dir, false);
        assert!(result.is_ok(), "init() should succeed");

        // Sprawdź czy katalog został stworzony
        assert!(
            log_dir.exists(),
            "init() should create log directory if it doesn't exist"
        );
        assert!(
            log_dir.is_dir(),
            "Log directory should be a directory, not a file"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    #[serial]
    fn test_log_file_path_none_without_init() {
        reset_logger();
        // Nie wywołujemy init() — logger nie jest zainicjalizowany

        // log_file_path powinno zwrócić None przed init
        let path = log_file_path();
        assert!(
            path.is_none(),
            "log_file_path() should return None when logger is not initialized"
        );
    }

    #[test]
    #[serial]
    fn test_log_format() {
        reset_logger();
        let log_dir = temp_log_dir();
        init(&log_dir, true).expect("init should succeed");

        let log_path = log_file_path().expect("Log path should be set").clone();

        // Zapisz wiadomości różnych poziomów
        warn("Test warning");
        debug("Test debug");

        // Force flush i sync do dysku
        flush_and_sync();

        // Odczytaj plik logu
        let contents = read_log_file(&log_path).expect("Should read log file");

        // Sprawdź format: [YYYY-MM-DD HH:MM:SS] [LEVEL] message
        // Format timestampu: [2026-02-14 12:34:56]
        let lines: Vec<&str> = contents.lines().collect();

        for line in &lines {
            // Każda linia powinna zaczynać się od [
            assert!(line.starts_with('['), "Line should start with '['");

            // Powinna zawierać zamknięcie pierwszego nawiasu dla timestampu
            assert!(line.contains("] ["), "Line should contain '] ['");

            // Powinna zawierać poziom logu w nawiasach
            let has_level = line.contains("[WARN]") || line.contains("[DEBUG]");
            assert!(
                has_level,
                "Line should contain log level [WARN] or [DEBUG]: {}",
                line
            );

            // Powinna zawierać treść wiadomości po poziomie
            assert!(
                line.contains("Test warning") || line.contains("Test debug"),
                "Line should contain message content: {}",
                line
            );
        }

        // Sprawdź że mamy przynajmniej 2 linie (warn + debug)
        assert!(
            lines.len() >= 2,
            "Should have at least 2 log lines (warn + debug)"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    #[serial]
    fn test_thread_safety() {
        reset_logger();
        let log_dir = temp_log_dir();
        init(&log_dir, true).expect("init should succeed");

        let log_path = log_file_path().expect("Log path should be set").clone();

        // Utwórz 10 wątków, każdy pisze 10 wiadomości
        let handles: Vec<_> = (0..10)
            .map(|thread_id| {
                std::thread::spawn(move || {
                    for msg_id in 0..10 {
                        warn(&format!("Thread {} warn {}", thread_id, msg_id));
                        debug(&format!("Thread {} debug {}", thread_id, msg_id));
                    }
                })
            })
            .collect();

        // Czekaj na zakończenie wszystkich wątków
        for handle in handles {
            handle.join().expect("Thread should not panic");
        }

        // Force flush i sync do dysku
        flush_and_sync();

        // Odczytaj plik logu
        let contents = read_log_file(&log_path).expect("Should read log file");

        // Sprawdź że wszystkie wiadomości zostały zapisane
        // 10 wątków * 10 warnów = 100 warnów
        // 10 wątków * 10 debugów = 100 debugów
        let warn_count = contents.matches("[WARN]").count();
        let debug_count = contents.matches("[DEBUG]").count();

        assert_eq!(warn_count, 100, "Should have exactly 100 WARN messages");
        assert_eq!(debug_count, 100, "Should have exactly 100 DEBUG messages");

        // Sprawdź że wiadomości od wszystkich wątków są obecne
        for thread_id in 0..10 {
            let thread_marker = format!("Thread {}", thread_id);
            assert!(
                contents.contains(&thread_marker),
                "Should contain messages from thread {}",
                thread_id
            );
        }

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    // --- Auto-cleanup tests ---

    #[test]
    #[serial]
    fn test_cleanup_old_logs_unlimited() {
        let log_dir = temp_log_dir();
        fs::create_dir_all(&log_dir).expect("Create log dir");

        // Utwórz kilka plików logów
        for i in 0..15 {
            let filename = format!("ralph-test-{:02}.log", i);
            let path = log_dir.join(filename);
            File::create(path).expect("Create test log file");
        }

        // Sprawdź że jest 15 plików
        let count_before = fs::read_dir(&log_dir)
            .expect("Read dir")
            .filter_map(|e| e.ok())
            .count();
        assert_eq!(count_before, 15, "Should have 15 files before cleanup");

        // Cleanup z max_log_files=0 (unlimited) — powinien zostawić wszystkie
        let result = super::cleanup_old_logs(&log_dir, 0);
        assert!(
            result.is_ok(),
            "cleanup_old_logs with unlimited should succeed"
        );

        let count_after = fs::read_dir(&log_dir)
            .expect("Read dir")
            .filter_map(|e| e.ok())
            .count();
        assert_eq!(count_after, 15, "Should keep all files when unlimited");

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    #[serial]
    fn test_cleanup_old_logs_removes_oldest() {
        let log_dir = temp_log_dir();
        fs::create_dir_all(&log_dir).expect("Create log dir");

        // Utwórz kilka plików logów z opóźnieniem (aby miały różne modification times)
        let mut created_files = Vec::new();
        for i in 0..5 {
            let filename = format!("ralph-test-{:02}.log", i);
            let path = log_dir.join(filename.clone());
            File::create(&path).expect("Create test log file");
            created_files.push(path);
            // Mały opór aby system plików miał czas na zanotowanie różnych czasów
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Sprawdź że jest 5 plików
        let count_before = fs::read_dir(&log_dir)
            .expect("Read dir")
            .filter_map(|e| e.ok())
            .count();
        assert_eq!(count_before, 5, "Should have 5 files before cleanup");

        // Cleanup z max_log_files=3 — powinien usunąć 2 najstarsze
        let result = super::cleanup_old_logs(&log_dir, 3);
        assert!(result.is_ok(), "cleanup_old_logs should succeed");

        let count_after = fs::read_dir(&log_dir)
            .expect("Read dir")
            .filter_map(|e| e.ok())
            .count();
        assert_eq!(count_after, 3, "Should have 3 files after cleanup");

        // Sprawdź że został najnowszy plik
        assert!(created_files[4].exists(), "Newest file should still exist");

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    #[serial]
    fn test_cleanup_old_logs_within_limit() {
        let log_dir = temp_log_dir();
        fs::create_dir_all(&log_dir).expect("Create log dir");

        // Utwórz 3 pliki logów
        for i in 0..3 {
            let filename = format!("ralph-test-{:02}.log", i);
            let path = log_dir.join(filename);
            File::create(path).expect("Create test log file");
        }

        // Sprawdź że jest 3 pliki
        let count_before = fs::read_dir(&log_dir)
            .expect("Read dir")
            .filter_map(|e| e.ok())
            .count();
        assert_eq!(count_before, 3);

        // Cleanup z max_log_files=5 — powinien zostawić wszystkie (bo 3 <= 5)
        let result = super::cleanup_old_logs(&log_dir, 5);
        assert!(result.is_ok(), "cleanup_old_logs should succeed");

        let count_after = fs::read_dir(&log_dir)
            .expect("Read dir")
            .filter_map(|e| e.ok())
            .count();
        assert_eq!(count_after, 3, "Should keep all files when within limit");

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    #[serial]
    fn test_cleanup_old_logs_ignores_non_log_files() {
        let log_dir = temp_log_dir();
        fs::create_dir_all(&log_dir).expect("Create log dir");

        // Utwórz pliki logów
        for i in 0..3 {
            let filename = format!("ralph-test-{:02}.log", i);
            let path = log_dir.join(filename);
            File::create(path).expect("Create test log file");
        }

        // Utwórz plik .txt (nie powinien być brany pod uwagę)
        let txt_path = log_dir.join("README.txt");
        File::create(txt_path).expect("Create test txt file");

        // Sprawdź że są 4 pliki
        let count_before = fs::read_dir(&log_dir)
            .expect("Read dir")
            .filter_map(|e| e.ok())
            .count();
        assert_eq!(count_before, 4);

        // Cleanup z max_log_files=2 — powinien zostawić 2 .log i 1 .txt
        let result = super::cleanup_old_logs(&log_dir, 2);
        assert!(result.is_ok(), "cleanup_old_logs should succeed");

        let count_after = fs::read_dir(&log_dir)
            .expect("Read dir")
            .filter_map(|e| e.ok())
            .count();
        assert_eq!(count_after, 3, "Should have 2 .log files + 1 .txt file");

        // Sprawdź że .txt został zachowany
        let txt_path = log_dir.join("README.txt");
        assert!(txt_path.exists(), ".txt file should still exist");

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    #[serial]
    fn test_init_with_config_calls_cleanup() {
        reset_logger();
        let log_dir = temp_log_dir();

        // Utwórz katalog i kilka starych plików logów
        fs::create_dir_all(&log_dir).expect("Create log dir");
        for i in 0..8 {
            let filename = format!("ralph-old-{:02}.log", i);
            let path = log_dir.join(filename);
            File::create(path).expect("Create old log file");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        // Sprawdź że jest 8 plików
        let count_before = fs::read_dir(&log_dir)
            .expect("Read dir")
            .filter_map(|e| e.ok())
            .count();
        assert_eq!(count_before, 8);

        // Inicjalizuj logger z max_log_files=4
        // Po init() powinno być 4 stare pliki + 1 nowy = 5 plików
        // Ale cleanup powinien to zmienić na 4 (najnowsze zostają)
        let result = super::init_with_config(&log_dir, 4, false);
        assert!(result.is_ok(), "init_with_config should succeed");

        // Sprawdzić liczbę plików (powinno być max 4 starych + ewentualnie nowy)
        // w zależności od timing cleanup'u
        let count_after = fs::read_dir(&log_dir)
            .expect("Read dir")
            .filter_map(|e| e.ok())
            .count();

        // Powinno być co najwyżej 5 (4 limite + 1 nowy plik)
        // ale cleanup powinien usunąć zbędne
        assert!(
            count_after <= 5,
            "Should have at most 5 files after init with cleanup"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&log_dir);
    }
}
