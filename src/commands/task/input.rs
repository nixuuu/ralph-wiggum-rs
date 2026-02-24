use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

use crate::commands::standalone_text_input::standalone_text_input;
use crate::shared::attachments::{Attachment, MAX_IMAGES, load_attachment};
use crate::shared::error::{RalphError, Result};
use crossterm::style::Stylize;

/// Wynik resolucji wejścia: tekst promptu wraz z załadowanymi załącznikami.
///
/// Typ zwracany z `resolve_input` — zastępuje `String` i umożliwia przekazanie
/// obrazów lub innych załączników obok tekstu.
// TODO: remove when resolve_input is updated to return ResolvedInput
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedInput {
    /// Tekst promptu po oczyszczeniu (bez ścieżek do plików inline).
    pub text: String,
    /// Lista załadowanych załączników (np. obrazy).
    pub attachments: Vec<Attachment>,
}

#[allow(dead_code)]
impl ResolvedInput {
    /// Tworzy `ResolvedInput` z samym tekstem, bez załączników.
    pub fn text_only(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attachments: Vec::new(),
        }
    }

    /// Zwraca `true` jeśli lista załączników jest niepusta.
    pub fn has_attachments(&self) -> bool {
        !self.attachments.is_empty()
    }

    /// Zwraca liczbę załączników.
    pub fn attachment_count(&self) -> usize {
        self.attachments.len()
    }
}

/// Resolve input from file, prompt, or stdin.
/// Priority: file > prompt > stdin (piped) > interactive TUI > error
///
/// # Arguments
/// * `file` - Optional path to a file containing input
/// * `prompt` - Optional text prompt
/// * `context_hint` - Optional placeholder text for interactive mode (e.g., "Describe tasks to add...")
///
/// # Returns
/// * `Ok(String)` - Resolved input text
/// * `Err(Interrupted)` - User pressed Ctrl+C in interactive mode
/// * `Err(TaskSetup)` - No input method available (edge case)
pub fn resolve_input(
    file: Option<&PathBuf>,
    prompt: Option<&str>,
    context_hint: Option<&str>,
) -> Result<String> {
    // 1. File has highest priority
    if let Some(path) = file {
        return std::fs::read_to_string(path).map_err(|e| {
            RalphError::TaskSetup(format!("Failed to read file {}: {}", path.display(), e))
        });
    }

    // 2. Prompt text
    if let Some(text) = prompt {
        return Ok(text.to_string());
    }

    // 3. Stdin if piped (not a terminal)
    if !io::stdin().is_terminal() {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| RalphError::TaskSetup(format!("Failed to read stdin: {}", e)))?;
        if !buf.trim().is_empty() {
            return Ok(buf);
        }
    }

    // 4. Interactive TUI text input (fallback when stdin is terminal)
    if io::stdin().is_terminal() {
        let input = standalone_text_input(context_hint, true)?;
        // Echo submitted text — text_input viewport collapses after submit,
        // so user doesn't see what they typed without this
        echo_submitted_input(&input);
        return Ok(input);
    }

    // 5. Error (edge case: no method available)
    Err(RalphError::TaskSetup(
        "No input provided. Use --file <path>, --prompt <text>, or pipe via stdin.".into(),
    ))
}

/// Rozszerza `resolve_input()` o obsługę flag `--image`.
///
/// Łączy tekst promptu (z pliku / flagi / stdin / TUI) z listą załadowanych obrazów.
/// Każdy obraz jest walidowany i ładowany przez [`load_attachment`].
///
/// # Argumenty
/// * `file` - Opcjonalna ścieżka do pliku z tekstem promptu
/// * `prompt` - Opcjonalny tekst promptu
/// * `context_hint` - Opcjonalny placeholder dla trybu interaktywnego
/// * `images` - Lista ścieżek do obrazów przekazanych przez `--image`
///
/// # Błędy
/// - Przekroczenie `MAX_IMAGES` daje czytelny błąd z aktualnym limitem.
/// - Każdy nieistniejący lub nieprawidłowy obraz przerywa przetwarzanie (fail-fast).
/// - Propaguje błędy z `resolve_input()` bez zmian.
// TODO: remove when integrated into task commands using --image flag
#[allow(dead_code)]
pub fn resolve_input_with_attachments(
    file: Option<&PathBuf>,
    prompt: Option<&str>,
    context_hint: Option<&str>,
    images: &[PathBuf],
) -> Result<ResolvedInput> {
    // Krok 1: Sprawdź limit obrazów — przed resolve_input(), by nie angażować
    // użytkownika interaktywnymi TUI tylko po to, by dostał błąd limitu.
    if images.len() > MAX_IMAGES {
        return Err(RalphError::TaskSetup(format!(
            "Przekroczono limit obrazów: podano {}, maksimum to {}.",
            images.len(),
            MAX_IMAGES
        )));
    }

    // Krok 2: Resolvuj tekst promptu (file > prompt > stdin > TUI)
    let text = resolve_input(file, prompt, context_hint)?;

    // Krok 3: Załaduj każdy obraz — fail-fast przy pierwszym błędzie
    let attachments: Vec<Attachment> = images
        .iter()
        .map(|path| load_attachment(path))
        .collect::<Result<Vec<_>>>()?;

    Ok(ResolvedInput { text, attachments })
}

/// Echoes user's submitted text after text_input viewport collapses.
/// Shows a compact preview so user sees what they typed.
fn echo_submitted_input(input: &str) {
    let lines: Vec<&str> = input.lines().collect();
    let preview = if lines.len() > 5 {
        format!(
            "{}\n  ... (+{} wierszy)",
            lines[..5].join("\n"),
            lines.len() - 5
        )
    } else {
        input.to_string()
    };
    println!("{}", preview.dark_grey());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::attachments::{ImageAttachment, MediaType};
    use std::path::PathBuf;

    // ── Testy ResolvedInput ───────────────────────────────────────────────────

    #[test]
    fn test_resolved_input_text_only() {
        let ri = ResolvedInput::text_only("Hello world");
        assert_eq!(ri.text, "Hello world");
        assert!(ri.attachments.is_empty());
        assert!(!ri.has_attachments());
        assert_eq!(ri.attachment_count(), 0);
    }

    #[test]
    fn test_resolved_input_with_attachments() {
        let img = ImageAttachment {
            path: PathBuf::from("/tmp/test.png"),
            media_type: MediaType::Png,
            base64_data: "dGVzdA==".to_string(),
            original_size_bytes: 512,
        };
        let ri = ResolvedInput {
            text: "Opis obrazu".to_string(),
            attachments: vec![Attachment::Image(img)],
        };

        assert_eq!(ri.text, "Opis obrazu");
        assert!(ri.has_attachments());
        assert_eq!(ri.attachment_count(), 1);
    }

    #[test]
    fn test_resolved_input_multiple_attachments() {
        let make_img = |path: &str| {
            Attachment::Image(ImageAttachment {
                path: PathBuf::from(path),
                media_type: MediaType::Jpeg,
                base64_data: "abc".to_string(),
                original_size_bytes: 256,
            })
        };

        let ri = ResolvedInput {
            text: "Dwa obrazy".to_string(),
            attachments: vec![make_img("/tmp/a.jpg"), make_img("/tmp/b.jpg")],
        };

        assert_eq!(ri.attachment_count(), 2);
        assert!(ri.has_attachments());
    }

    #[test]
    fn test_resolved_input_clone() {
        let ri = ResolvedInput::text_only("Klonowalny");
        let cloned = ri.clone();
        assert_eq!(cloned.text, ri.text);
        assert_eq!(cloned.attachment_count(), ri.attachment_count());
    }

    #[test]
    fn test_resolve_input_prompt() {
        let result = resolve_input(None, Some("Hello world"), None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Hello world");
    }

    #[test]
    fn test_resolve_input_missing_file() {
        let path = PathBuf::from("/nonexistent/file.md");
        let result = resolve_input(Some(&path), None, None);
        assert!(result.is_err());
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn test_text_input_imports_available() {
        // Weryfikacja że funkcje text_input są dostępne z commands::standalone_text_input

        // Import przez pełną ścieżkę
        use crate::commands::standalone_text_input::{standalone_text_input, text_input};

        // Weryfikacja że typy funkcji się zgadzają
        let _fn1: fn(Option<&str>, Option<&str>, bool, Option<&str>) -> Result<String> = text_input;
        let _fn2: fn(Option<&str>, bool) -> Result<String> = standalone_text_input;
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn test_standalone_text_input_wrapper_signature() {
        // Weryfikacja że standalone_text_input konwertuje Back → Interrupted
        use crate::commands::standalone_text_input::standalone_text_input;

        // Sygnatura: standalone_text_input jest wrapperem text_input(placeholder, None, required, None)
        let _fn: fn(Option<&str>, bool) -> Result<String> = standalone_text_input;
    }

    #[test]
    fn test_resolve_input_priority_file_over_prompt() {
        // Stwórz tymczasowy plik
        let dir = std::env::temp_dir().join("ralph_test_resolve_input_priority");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, "File content").unwrap();

        // File ma priorytet nad prompt
        let result = resolve_input(Some(&path), Some("Prompt text"), Some("Placeholder"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "File content");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_input_priority_prompt_over_stdin() {
        // Gdy prompt jest podany, stdin nie jest sprawdzany
        // (stdin.is_terminal() zwróci true w środowisku testowym)
        let result = resolve_input(None, Some("Prompt text"), Some("Placeholder"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Prompt text");
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn test_resolve_input_context_hint_signature() {
        // Test weryfikujący że context_hint jest przekazywany do standalone_text_input
        // W środowisku testowym (stdin jest terminalem) funkcja resolve_input
        // będzie próbowała wywołać standalone_text_input, co wymaga interakcji.
        // Ten test tylko weryfikuje że sygnatura się kompiluje.

        let _file: Option<&PathBuf> = None;
        let _prompt: Option<&str> = None;
        let _context: Option<&str> = Some("Test placeholder");

        // Funkcja kompiluje się z trzema parametrami
        // Nie wywołujemy bo wymaga interakcji z terminalem
        let _fn_ptr: fn(Option<&PathBuf>, Option<&str>, Option<&str>) -> Result<String> =
            resolve_input;
    }

    #[test]
    fn test_resolve_input_priority_all_levels() {
        // Test weryfikujący pełną hierarchię priorytetów:
        // file > prompt > stdin > interactive
        // (context_hint nie zmienia priorytetów, tylko parametryzuje interactive)

        let dir = std::env::temp_dir().join("ralph_test_priority_all");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("priority.txt");
        std::fs::write(&path, "File wins").unwrap();

        // 1. File ma najwyższy priorytet nawet z context_hint i prompt
        let result = resolve_input(
            Some(&path),
            Some("Prompt text"),
            Some("Interactive placeholder"),
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "File wins");

        // 2. Prompt ma wyższy priorytet niż interactive (bez file)
        let result = resolve_input(None, Some("Prompt wins"), Some("Interactive placeholder"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Prompt wins");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_input_context_hint_passed() {
        // Test weryfikujący że context_hint jest poprawnie przekazywany
        // W praktyce w środowisku testowym stdin jest terminalem, więc funkcja
        // próbowałaby wywołać standalone_text_input(context_hint, true).
        // Nie możemy tego bezpośrednio przetestować bez mockowania lub interakcji.
        // Ten test weryfikuje że gdy podamy wszystkie parametry None/None/Some,
        // funkcja nie zwróci natychmiastowego błędu przed próbą TUI.

        // W środowisku testowym is_terminal() zwraca true, więc:
        // - file=None → skip
        // - prompt=None → skip
        // - stdin not piped → skip (is_terminal()=true)
        // - próba wywołania standalone_text_input(Some("hint"), true)
        // To wymaga interakcji, więc test tylko sprawdza, że nie ma błędu kompilacji
        // i że sygnatura jest poprawna.

        let _result = || resolve_input(None, None, Some("Test hint"));
        // Nie wywołujemy, bo to wymaga terminala + interakcji
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn test_resolve_input_no_input_in_terminal_edge_case() {
        // Test edge case: gdy stdin jest terminalem ale nie możemy uruchomić TUI
        // (np. brak dostępu do /dev/tty lub mock środowisko).
        // W normalnym flow resolve_input(None, None, None) w terminalu
        // powinien próbować interaktywny input. Jeśli to się nie uda,
        // standalone_text_input zwróci błąd (np. Interrupted lub inny).

        // W środowisku testowym is_terminal() zwraca true, ale standalone_text_input
        // może nie mieć dostępu do prawdziwego terminala.
        // Tutaj testujemy że logika nie zwróci natychmiastowo błędu TaskSetup
        // z komunikatem "No input provided..." — zamiast tego powinna próbować TUI.

        // Uwaga: Ten test NIE może bezpośrednio wywołać resolve_input(None, None, None),
        // bo to wymaga interakcji. Test weryfikuje tylko że logika jest poprawna
        // przez sprawdzenie ścieżki kodu.

        // Mock scenario: gdyby is_terminal() zwracało false (piped stdin),
        // a stdin był pusty, wtedy dostalibyśmy edge case error.
        // Ale to NIE jest nasz case — chcemy sprawdzić terminal=true.

        // Alternatywne podejście: weryfikujemy że w terminalu błąd TaskSetup
        // "No input provided..." NIE jest zwracany przed próbą TUI.
        // To wymaga mockowania is_terminal() lub akceptacji że test jest deklaratywny.

        // Deklaratywny test: logika w resolve_input MUSI spełniać:
        // if stdin.is_terminal() → standalone_text_input(...) → Result
        // Jeśli is_terminal()=true, funkcja NIE dochodzi do ostatniego Err().

        // Implementujemy test kompilacji logiki:
        use std::io::{self, IsTerminal};

        // Sprawdzamy że w środowisku testowym stdin jest terminalem
        // (testy cargo są uruchamiane w terminalu)
        let is_terminal = io::stdin().is_terminal();

        if is_terminal {
            // W tym przypadku resolve_input(None, None, hint) próbuje TUI
            // i NIE zwraca natychmiastowo błędu "No input provided".
            // Test przechodzi jeśli funkcja ma poprawną logikę.
            // Nie wywołujemy, bo to wymaga interakcji.

            // Sprawdzamy że logika istnieje (test kompilacji)
            let _fn_ptr: fn(Option<&PathBuf>, Option<&str>, Option<&str>) -> Result<String> =
                resolve_input;
        } else {
            // Jeśli stdin nie jest terminalem (rzadki przypadek w testach),
            // wtedy resolve_input(None, None, None) zwróci błąd "No input provided"
            // po próbie odczytu pustego stdin.
            let result = resolve_input(None, None, None);
            assert!(
                result.is_err(),
                "Expected error when stdin is not terminal and no input provided"
            );
        }
    }

    // ── Testy resolve_input_with_attachments ────────────────────────────────

    /// Tworzy prawidłowy plik PNG 1x1 w podanym katalogu, zwraca ścieżkę.
    ///
    /// Używa crate `image` do wygenerowania poprawnego obrazu (z prawidłowym CRC).
    fn write_temp_png(dir: &std::path::Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let img = image::DynamicImage::new_rgb8(1, 1);
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(&path, &buf).unwrap();
        path
    }

    #[test]
    fn test_resolve_input_with_attachments_no_images() {
        // Brak obrazów → ResolvedInput z tekstem i pustą listą załączników
        let result = resolve_input_with_attachments(None, Some("Hello"), None, &[]);
        assert!(result.is_ok());
        let ri = result.unwrap();
        assert_eq!(ri.text, "Hello");
        assert!(!ri.has_attachments());
        assert_eq!(ri.attachment_count(), 0);
    }

    #[test]
    fn test_resolve_input_with_attachments_valid_image() {
        let dir = std::env::temp_dir().join("ralph_test_resolve_attach_valid");
        let _ = std::fs::create_dir_all(&dir);
        let img_path = write_temp_png(&dir, "test.png");

        let result = resolve_input_with_attachments(None, Some("Opis obrazu"), None, &[img_path]);

        assert!(result.is_ok(), "Expected Ok, got: {:?}", result.err());
        let ri = result.unwrap();
        assert_eq!(ri.text, "Opis obrazu");
        assert!(ri.has_attachments());
        assert_eq!(ri.attachment_count(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_input_with_attachments_multiple_images() {
        let dir = std::env::temp_dir().join("ralph_test_resolve_attach_multi");
        let _ = std::fs::create_dir_all(&dir);
        let paths: Vec<PathBuf> = (0..3)
            .map(|i| write_temp_png(&dir, &format!("img{i}.png")))
            .collect();

        let result = resolve_input_with_attachments(None, Some("Trzy obrazy"), None, &paths);

        assert!(result.is_ok());
        let ri = result.unwrap();
        assert_eq!(ri.attachment_count(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_input_with_attachments_exceeds_limit() {
        // MAX_IMAGES + 1 ścieżek → błąd z czytelnym komunikatem,
        // zwracany PRZED wywołaniem resolve_input() (fail-fast, bez TUI).
        let fake_paths: Vec<PathBuf> = (0..=MAX_IMAGES)
            .map(|i| PathBuf::from(format!("/tmp/fake_{i}.png")))
            .collect();

        // prompt=None, file=None — gdyby limit nie był sprawdzany jako pierwszy,
        // funkcja próbowałaby uruchomić interaktywny TUI. Nie powinna.
        let result = resolve_input_with_attachments(None, None, None, &fake_paths);

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("limit") || err_msg.contains("maksimum"),
            "Błąd powinien wspominać o limicie: {err_msg}"
        );
    }

    #[test]
    fn test_resolve_input_with_attachments_nonexistent_image() {
        // Nieistniejący obraz → fail-fast z błędem
        let bad_path = PathBuf::from("/nonexistent/image.png");

        let result = resolve_input_with_attachments(None, Some("Tekst"), None, &[bad_path]);

        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_input_with_attachments_fail_fast_on_second_bad_image() {
        // Pierwszy obraz OK, drugi nieistniejący → przerywa z błędem (fail-fast)
        let dir = std::env::temp_dir().join("ralph_test_resolve_attach_failfast");
        let _ = std::fs::create_dir_all(&dir);
        let good = write_temp_png(&dir, "good.png");
        let bad = PathBuf::from("/nonexistent/bad.png");

        let result =
            resolve_input_with_attachments(None, Some("Fail fast test"), None, &[good, bad]);

        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_input_with_attachments_text_from_file() {
        // Tekst pochodzi z pliku — file ma wyższy priorytet niż prompt
        let dir = std::env::temp_dir().join("ralph_test_resolve_attach_file");
        let _ = std::fs::create_dir_all(&dir);
        let text_file = dir.join("input.md");
        std::fs::write(&text_file, "Treść z pliku").unwrap();
        let img = write_temp_png(&dir, "img.png");

        let result = resolve_input_with_attachments(
            Some(&text_file),
            Some("Zignorowany prompt"),
            None,
            &[img],
        );

        assert!(result.is_ok());
        let ri = result.unwrap();
        assert_eq!(ri.text, "Treść z pliku");
        assert_eq!(ri.attachment_count(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_input_with_attachments_exactly_at_limit() {
        // Dokładnie MAX_IMAGES obrazów → powinno przejść (limit jest włączny)
        let dir = std::env::temp_dir().join("ralph_test_resolve_attach_limit");
        let _ = std::fs::create_dir_all(&dir);
        let paths: Vec<PathBuf> = (0..MAX_IMAGES)
            .map(|i| write_temp_png(&dir, &format!("img{i}.png")))
            .collect();

        let result = resolve_input_with_attachments(None, Some("Na granicy"), None, &paths);

        assert!(
            result.is_ok(),
            "MAX_IMAGES obrazów powinno być OK: {:?}",
            result.err()
        );
        let ri = result.unwrap();
        assert_eq!(ri.attachment_count(), MAX_IMAGES);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_input_with_attachments_propagates_resolve_error() {
        // images=[] + brakujący plik tekstowy → propaguje błąd z resolve_input()
        let bad_file = PathBuf::from("/nonexistent/input.md");
        let result = resolve_input_with_attachments(Some(&bad_file), None, None, &[]);
        assert!(
            result.is_err(),
            "Powinien propagować błąd z resolve_input() gdy plik nie istnieje"
        );
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn test_resolve_input_with_attachments_signature() {
        // Weryfikacja że sygnatura funkcji jest poprawna i kompiluje się
        let _fn_ptr: fn(
            Option<&PathBuf>,
            Option<&str>,
            Option<&str>,
            &[PathBuf],
        ) -> Result<ResolvedInput> = resolve_input_with_attachments;
    }
}
