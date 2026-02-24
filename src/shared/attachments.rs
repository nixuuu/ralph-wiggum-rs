//! Typy dołączników dla wiadomości Claude API.
//!
//! Obsługuje obrazy w formatach PNG, JPEG, GIF i WebP,
//! kodowane jako base64 i przesyłane inline w żądaniu API.
//!
//! Typy są publicznym API — będą używane przez przyszłe moduły (np. attachment loader).
// TODO: remove when attachment loader is implemented
#![allow(dead_code)]
use base64::{Engine as _, engine::general_purpose};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Obsługiwane rozszerzenia plików graficznych (małe litery).
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

use image::imageops::FilterType;

use super::error::{RalphError, Result};

/// Maksymalna liczba obrazów w jednej wiadomości.
pub const MAX_IMAGES: usize = 10;

/// Maksymalny wymiar obrazu (szerokość lub wysokość) w pikselach.
/// Claude API wymaga, aby obrazy nie przekraczały 1568px na dłuższym boku.
pub const MAX_DIMENSION: u32 = 1568;

/// Obsługiwane formaty obrazów.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaType {
    Png,
    Jpeg,
    Gif,
    WebP,
}

impl MediaType {
    /// Zwraca MIME type dla danego formatu obrazu.
    pub fn as_mime_str(&self) -> &str {
        match self {
            MediaType::Png => "image/png",
            MediaType::Jpeg => "image/jpeg",
            MediaType::Gif => "image/gif",
            MediaType::WebP => "image/webp",
        }
    }
}

/// Wykrywa format obrazu na podstawie magic bytes (pierwszych bajtów pliku).
///
/// Obsługiwane formaty:
/// - PNG: nagłówek `\x89PNG`
/// - JPEG: nagłówek `\xFF\xD8\xFF`
/// - GIF: nagłówek `GIF8` (GIF87a lub GIF89a)
/// - WebP: nagłówek `RIFF....WEBP` (RIFF container)
///
/// Zwraca `None` dla nieznanych formatów lub danych krótszych niż wymagany nagłówek.
pub fn detect_media_type(data: &[u8]) -> Option<MediaType> {
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Some(MediaType::Png);
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(MediaType::Jpeg);
    }
    if data.starts_with(b"GIF8") {
        return Some(MediaType::Gif);
    }
    // WebP: bajty 0-3 to "RIFF", bajty 8-11 to "WEBP"
    if data.len() >= 12 && data[..4] == *b"RIFF" && data[8..12] == *b"WEBP" {
        return Some(MediaType::WebP);
    }
    None
}

/// Obraz zakodowany w base64, gotowy do wysłania do Claude API.
#[derive(Debug, Clone)]
pub struct ImageAttachment {
    /// Ścieżka do oryginalnego pliku obrazu.
    pub path: PathBuf,
    /// Format obrazu (PNG, JPEG, GIF, WebP).
    pub media_type: MediaType,
    /// Dane obrazu zakodowane w base64.
    pub base64_data: String,
    /// Rozmiar oryginalnego pliku w bajtach.
    pub original_size_bytes: u64,
}

/// Dołącznik do wiadomości — może być obrazem lub innym typem w przyszłości.
#[derive(Debug, Clone)]
pub enum Attachment {
    Image(ImageAttachment),
}

/// Koduje bajty do stringa base64 (standard encoding, bez prefixu `data:`).
///
/// Używa standardowego alfabetu base64 z paddingiem `=`.
pub fn encode_base64(data: &[u8]) -> String {
    general_purpose::STANDARD.encode(data)
}

// ── Wykrywanie ścieżek graficznych w tekście ─────────────────────────────────

/// Sprawdza, czy rozszerzenie stringa jest obsługiwanym formatem graficznym.
///
/// Porównanie jest case-insensitive (`.PNG` == `.png`).
fn is_image_extension(s: &str) -> bool {
    if let Some(dot_pos) = s.rfind('.') {
        let ext = s[dot_pos + 1..].to_lowercase();
        IMAGE_EXTENSIONS.contains(&ext.as_str())
    } else {
        false
    }
}

/// Sprawdza, czy tekst wygląda jak ścieżka do pliku.
///
/// Kryteria: zaczyna się od `/`, `~/`, `./` lub zawiera `/`.
fn looks_like_path(s: &str) -> bool {
    s.starts_with('/') || s.starts_with("~/") || s.starts_with("./") || s.contains('/')
}

/// Usuwa cudzysłowy (pojedyncze i podwójne) z początku i końca stringa.
///
/// Cudzysłowy muszą być symetryczne — `"..."` lub `'...'`.
fn strip_quotes(s: &str) -> &str {
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        if (bytes[0] == b'"' && bytes[s.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[s.len() - 1] == b'\'')
        {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// Rozszerza `~` na katalog domowy użytkownika.
///
/// Jeśli `HOME` nie jest ustawione, ścieżka jest zwracana bez zmian.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    } else if path == "~"
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home);
    }
    PathBuf::from(path)
}

/// Wykrywa ścieżki do plików graficznych w tekście i oddziela je od treści.
///
/// Drag'n'drop w terminalu wkleja pełną ścieżkę pliku jako osobną linię.
/// Ta funkcja identyfikuje takie linie i zwraca je osobno, oczyszczając tekst.
///
/// # Algorytm
///
/// Dla każdej linii tekstu:
/// 1. Usuń białe znaki i ewentualne cudzysłowy (`"..."` / `'...'`)
/// 2. Sprawdź czy wygląda jak ścieżka (starts_with `/`, `~/`, `./` lub zawiera `/`)
/// 3. Sprawdź, czy rozszerzenie jest graficzne (`.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`)
/// 4. Zweryfikuj istnienie pliku na dysku (z rozwinięciem `~`)
///
/// Linie spełniające wszystkie kryteria są usuwane z tekstu i dodawane do wektora.
///
/// # Returns
///
/// Krotka `(oczyszczony_tekst, wykryte_ścieżki)`.
pub fn extract_image_paths(text: &str) -> (String, Vec<PathBuf>) {
    let mut kept_lines: Vec<&str> = Vec::new();
    let mut image_paths: Vec<PathBuf> = Vec::new();

    for line in text.lines() {
        let candidate = strip_quotes(line.trim());

        if looks_like_path(candidate) && is_image_extension(candidate) {
            let path = expand_tilde(candidate);
            if path.exists() {
                image_paths.push(path);
                continue; // Linia z wykrytą ścieżką jest pomijana w tekście wyjściowym
            }
        }

        kept_lines.push(line);
    }

    let cleaned = kept_lines.join("\n");
    // Zachowaj trailing newline jeśli oryginalny tekst go miał
    let cleaned = if text.ends_with('\n') && !cleaned.is_empty() {
        format!("{cleaned}\n")
    } else {
        cleaned
    };

    (cleaned, image_paths)
}

// ── Walidacja pliku graficznego ──────────────────────────────────────────────

/// Lista nazw obsługiwanych formatów obrazów — używana w komunikatach błędu.
const SUPPORTED_FORMAT_NAMES: &[&str] = &["PNG", "JPEG", "GIF", "WebP"];

/// Waliduje plik graficzny i zwraca jego surowe bajty oraz wykryty typ MIME.
///
/// Implementuje fail-fast: pierwszy napotkany błąd przerywa walidację
/// i zwraca czytelny komunikat z pełną ścieżką pliku.
///
/// # Kroki walidacji
/// 1. Sprawdza istnienie pliku (`path.exists()`).
/// 2. Odczytuje bajty pliku (`fs::read`).
/// 3. Wykrywa format na podstawie magic bytes (`detect_media_type`).
///
/// # Błędy
/// - [`RalphError::MissingFile`] — plik nie istnieje (zawiera pełną ścieżkę).
/// - [`RalphError::Io`] — brak uprawnień do odczytu lub inny błąd I/O.
/// - [`RalphError::UnsupportedImageFormat`] — bajty nie pasują do żadnego
///   obsługiwanego formatu; komunikat zawiera listę dozwolonych formatów.
///
/// # Zwraca
/// `(bajty, MediaType)` dla poprawnych plików graficznych.
pub fn validate_image_file(path: &Path) -> Result<(Vec<u8>, MediaType)> {
    // Sprawdź istnienie — wcześniejszy błąd niż I/O, z pełną ścieżką.
    if !path.exists() {
        return Err(RalphError::MissingFile(path.display().to_string()));
    }

    // Odczytaj plik — propaguje błąd uprawnień / I/O przez RalphError::Io.
    let data = fs::read(path)?;

    // Wykryj format na podstawie magic bytes.
    let media_type =
        detect_media_type(&data).ok_or_else(|| RalphError::UnsupportedImageFormat {
            path: path.display().to_string(),
            supported: SUPPORTED_FORMAT_NAMES.join(", "),
        })?;

    Ok((data, media_type))
}

impl From<&MediaType> for image::ImageFormat {
    fn from(media_type: &MediaType) -> Self {
        match media_type {
            MediaType::Png => image::ImageFormat::Png,
            MediaType::Jpeg => image::ImageFormat::Jpeg,
            MediaType::Gif => image::ImageFormat::Gif,
            MediaType::WebP => image::ImageFormat::WebP,
        }
    }
}

/// Zmniejsza obraz do `max_dim` pikseli na dłuższym boku, zachowując proporcje.
///
/// Jeśli obraz mieści się w limicie (oba wymiary ≤ `max_dim`), zwraca oryginalne bajty
/// bez dodatkowego kodowania. Dzięki temu unikamy stratnych re-enkodowań dla JPEG/WebP.
///
/// Format wyjściowy odpowiada formatowi wejściowemu (PNG→PNG, JPEG→JPEG, itd.).
///
/// # Ograniczenia
///
/// - **GIF animowany**: image crate podczas resize traci animację — wynikowy GIF będzie
///   statyczny (tylko pierwsza klatka). Jeśli konieczna jest obsługa animowanych GIFów,
///   należy użyć dedykowanej biblioteki (np. `gifsicle` przez subprocess).
/// - **WebP animowany**: analogiczne ograniczenie jak GIF.
pub fn resize_if_needed(data: &[u8], media_type: &MediaType, max_dim: u32) -> Result<Vec<u8>> {
    let img =
        image::load_from_memory(data).map_err(|e| RalphError::ImageProcessing(e.to_string()))?;

    let (w, h) = (img.width(), img.height());

    // Jeśli obraz mieści się w limicie, zwróć oryginalne bajty bez re-enkodowania
    if w <= max_dim && h <= max_dim {
        return Ok(data.to_vec());
    }

    // Oblicz nowe wymiary zachowując aspect ratio (dłuższy bok → max_dim)
    let (new_w, new_h) = if w >= h {
        let new_h = (h as f64 * max_dim as f64 / w as f64).round() as u32;
        (max_dim, new_h.max(1))
    } else {
        let new_w = (w as f64 * max_dim as f64 / h as f64).round() as u32;
        (new_w.max(1), max_dim)
    };

    // resize_exact zamiast resize — proporcje już obliczone ręcznie powyżej,
    // więc nie chcemy, żeby image crate przeliczało je ponownie.
    let resized = img.resize_exact(new_w, new_h, FilterType::Lanczos3);

    let format = image::ImageFormat::from(media_type);
    let mut output = Vec::new();
    resized
        .write_to(&mut Cursor::new(&mut output), format)
        .map_err(|e| RalphError::ImageProcessing(e.to_string()))?;

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // ── Testy funkcji pomocniczych ────────────────────────────────────────────

    #[test]
    fn test_is_image_extension() {
        assert!(is_image_extension("/path/to/img.png"));
        assert!(is_image_extension("/path/to/img.PNG")); // case-insensitive
        assert!(is_image_extension("/path/to/img.jpg"));
        assert!(is_image_extension("/path/to/img.JPEG"));
        assert!(is_image_extension("/path/to/img.gif"));
        assert!(is_image_extension("/path/to/img.webp"));
        assert!(!is_image_extension("/path/to/file.txt"));
        assert!(!is_image_extension("/path/to/noextension"));
        assert!(!is_image_extension("/path/to/file.mp4"));
    }

    #[test]
    fn test_looks_like_path() {
        assert!(looks_like_path("/absolute/path"));
        assert!(looks_like_path("~/home/path"));
        assert!(looks_like_path("./relative/path"));
        assert!(looks_like_path("some/nested/path"));
        assert!(!looks_like_path("justword"));
        assert!(!looks_like_path("hello world"));
    }

    #[test]
    fn test_strip_quotes_double() {
        assert_eq!(strip_quotes("\"/path/to/file.png\""), "/path/to/file.png");
    }

    #[test]
    fn test_strip_quotes_single() {
        assert_eq!(strip_quotes("'/path/to/file.png'"), "/path/to/file.png");
    }

    #[test]
    fn test_strip_quotes_no_quotes() {
        assert_eq!(strip_quotes("/path/to/file.png"), "/path/to/file.png");
    }

    #[test]
    fn test_strip_quotes_mismatched() {
        // Niesymetryczne cudzysłowy — zostają bez zmian
        assert_eq!(strip_quotes("\"file.png'"), "\"file.png'");
    }

    #[test]
    fn test_strip_quotes_single_char() {
        // Jeden znak w cudzysłowach — nie jest prawidłową ścieżką, ale nie może panikować
        assert_eq!(strip_quotes("\""), "\"");
        assert_eq!(strip_quotes("'"), "'");
    }

    #[test]
    fn test_expand_tilde_with_home() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            let expanded = expand_tilde("~/Desktop/img.png");
            assert_eq!(expanded, PathBuf::from(&home).join("Desktop/img.png"));
        }
    }

    #[test]
    fn test_expand_tilde_absolute_unchanged() {
        let path = expand_tilde("/usr/local/bin/something");
        assert_eq!(path, PathBuf::from("/usr/local/bin/something"));
    }

    // ── Testy extract_image_paths ─────────────────────────────────────────────

    /// Tworzy tymczasowy plik obrazu i zwraca jego ścieżkę.
    fn create_temp_image(filename: &str) -> PathBuf {
        let path = std::env::temp_dir().join(filename);
        fs::write(&path, b"fake image data").expect("Failed to create temp image");
        path
    }

    #[test]
    fn test_extract_absolute_path() {
        let img = create_temp_image("test_extract_abs.png");
        let img_str = img.to_str().unwrap().to_string();
        let text = format!("Oto mój obraz:\n{img_str}\nCo o nim myślisz?");

        let (cleaned, paths) = extract_image_paths(&text);

        // Sprzątamy przed asercjami — plik nie zostaje na dysku przy błędzie
        fs::remove_file(&img).ok();

        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], img);
        assert!(!cleaned.contains(&img_str));
        assert!(cleaned.contains("Oto mój obraz:"));
        assert!(cleaned.contains("Co o nim myślisz?"));
    }

    #[test]
    fn test_extract_path_with_double_quotes() {
        let img = create_temp_image("test_extract_quoted_double.jpg");
        let img_str = img.to_str().unwrap().to_string();
        let text = format!("Sprawdź ten plik:\n\"{img_str}\"");

        let (cleaned, paths) = extract_image_paths(&text);

        // Sprzątamy przed asercjami — plik nie zostaje na dysku przy błędzie
        fs::remove_file(&img).ok();

        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], img);
        assert!(!cleaned.contains(&img_str));
    }

    #[test]
    fn test_extract_path_with_single_quotes() {
        let img = create_temp_image("test_extract_quoted_single.gif");
        let img_str = img.to_str().unwrap().to_string();
        let text = format!("'{img_str}'");

        let (_cleaned, paths) = extract_image_paths(&text);

        // Sprzątamy przed asercjami — plik nie zostaje na dysku przy błędzie
        fs::remove_file(&img).ok();

        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], img);
    }

    #[test]
    fn test_ignore_nonexistent_path() {
        let nonexistent = "/tmp/this_file_does_not_exist_ralph_12345.png";
        let text = format!("Tekst\n{nonexistent}\nKoniec");

        let (cleaned, paths) = extract_image_paths(text.as_str());

        // Ścieżka nieistniejąca — zostawiona w tekście, nie dodana do paths
        assert!(paths.is_empty());
        assert!(cleaned.contains(nonexistent));
    }

    #[test]
    fn test_ignore_non_image_extension() {
        let path = "/tmp/document.pdf";
        let text = format!("Tekst\n{path}\nKoniec");

        let (cleaned, paths) = extract_image_paths(&text);

        assert!(paths.is_empty());
        assert!(cleaned.contains(path));
    }

    #[test]
    fn test_ignore_plain_words() {
        let text = "Zwykłe słowa bez ścieżek.\nNie ma tu żadnych plików.\n";

        let (cleaned, paths) = extract_image_paths(text);

        assert!(paths.is_empty());
        assert_eq!(cleaned, text);
    }

    #[test]
    fn test_multiple_images() {
        let img1 = create_temp_image("test_multi_1.png");
        let img2 = create_temp_image("test_multi_2.webp");
        let img1_str = img1.to_str().unwrap().to_string();
        let img2_str = img2.to_str().unwrap().to_string();
        let text = format!("Opis:\n{img1_str}\n{img2_str}\nKoniec");

        let (cleaned, paths) = extract_image_paths(&text);

        // Sprzątamy przed asercjami — pliki nie zostają na dysku przy błędzie
        fs::remove_file(&img1).ok();
        fs::remove_file(&img2).ok();

        assert_eq!(paths.len(), 2);
        assert!(!cleaned.contains(&img1_str));
        assert!(!cleaned.contains(&img2_str));
        assert!(cleaned.contains("Opis:"));
        assert!(cleaned.contains("Koniec"));
    }

    #[test]
    fn test_empty_text() {
        let (cleaned, paths) = extract_image_paths("");
        assert!(paths.is_empty());
        assert_eq!(cleaned, "");
    }

    #[test]
    fn test_case_insensitive_extension() {
        let img = create_temp_image("test_case_ext.PNG");
        let img_str = img.to_str().unwrap().to_string();
        let text = img_str.clone();

        let (cleaned, paths) = extract_image_paths(&text);

        // Sprzątamy przed asercjami — plik nie zostaje na dysku przy błędzie
        fs::remove_file(&img).ok();

        assert_eq!(paths.len(), 1);
        assert!(cleaned.is_empty() || !cleaned.contains(&img_str));
    }

    #[test]
    fn test_extract_relative_path() {
        // Tworzymy plik w CWD, bo ścieżka `./foo.jpg` jest rozwiązywana względem CWD.
        // W `cargo test` CWD to katalog główny workspace'u.
        let filename = "test_ralph_relative_extract.jpg";
        let cwd = std::env::current_dir().unwrap();
        let abs_path = cwd.join(filename);
        fs::write(&abs_path, b"fake image data").expect("Failed to create temp relative image");

        let rel_path_str = format!("./{filename}");
        let text = format!("Oto obraz z drag-and-drop:\n{rel_path_str}\nCo o nim myślisz?");

        let (cleaned, paths) = extract_image_paths(&text);

        // Sprzątamy przed asercjami, żeby nie zostawić pliku przy błędzie testu
        fs::remove_file(&abs_path).ok();

        assert_eq!(paths.len(), 1, "Ścieżka relatywna powinna zostać wykryta");
        assert_eq!(paths[0], PathBuf::from(&rel_path_str));
        assert!(!cleaned.contains(&rel_path_str));
        assert!(cleaned.contains("Oto obraz z drag-and-drop:"));
        assert!(cleaned.contains("Co o nim myślisz?"));
    }

    #[test]
    fn test_ignore_txt_extension() {
        // Plik z rozszerzeniem .txt nie powinien być wykrywany jako obraz,
        // nawet jeśli wygląda jak ścieżka. Rozszerzenie sprawdzane jest przed
        // weryfikacją istnienia pliku.
        let path = "/tmp/document.txt";
        let text = format!("Wiadomość:\n{path}\nKoniec");

        let (cleaned, paths) = extract_image_paths(&text);

        assert!(paths.is_empty(), ".txt nie jest formatem graficznym");
        assert!(
            cleaned.contains(path),
            "Ścieżka .txt powinna pozostać w tekście"
        );
    }

    // ── Istniejące testy ──────────────────────────────────────────────────────

    /// Tworzy minimalny obraz PNG o podanych wymiarach (1px solid color) jako Vec<u8>.
    fn make_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::DynamicImage::new_rgb8(width, height);
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    /// Tworzy minimalny obraz JPEG o podanych wymiarach jako Vec<u8>.
    fn make_jpeg(width: u32, height: u32) -> Vec<u8> {
        let img = image::DynamicImage::new_rgb8(width, height);
        let mut buf = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut buf),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
        buf
    }

    #[test]
    fn test_media_type_mime_strings() {
        assert_eq!(MediaType::Png.as_mime_str(), "image/png");
        assert_eq!(MediaType::Jpeg.as_mime_str(), "image/jpeg");
        assert_eq!(MediaType::Gif.as_mime_str(), "image/gif");
        assert_eq!(MediaType::WebP.as_mime_str(), "image/webp");
    }

    #[test]
    fn test_constants() {
        assert_eq!(MAX_IMAGES, 10);
        assert_eq!(MAX_DIMENSION, 1568u32);
    }

    #[test]
    fn test_image_attachment_fields() {
        let attachment = ImageAttachment {
            path: PathBuf::from("/tmp/test.png"),
            media_type: MediaType::Png,
            base64_data: "dGVzdA==".to_string(),
            original_size_bytes: 1024,
        };

        assert_eq!(attachment.path, PathBuf::from("/tmp/test.png"));
        assert_eq!(attachment.media_type, MediaType::Png);
        assert_eq!(attachment.base64_data, "dGVzdA==");
        assert_eq!(attachment.original_size_bytes, 1024);
    }

    #[test]
    fn test_attachment_enum_image_variant() {
        let img = ImageAttachment {
            path: PathBuf::from("/tmp/photo.jpg"),
            media_type: MediaType::Jpeg,
            base64_data: "abc123".to_string(),
            original_size_bytes: 2048,
        };
        let attachment = Attachment::Image(img);

        // Sprawdź że można dopasować wariant
        let Attachment::Image(inner) = attachment;
        assert_eq!(inner.media_type.as_mime_str(), "image/jpeg");
    }

    #[test]
    fn test_media_type_equality() {
        assert_eq!(MediaType::Png, MediaType::Png);
        assert_ne!(MediaType::Png, MediaType::Jpeg);
        assert_ne!(MediaType::Gif, MediaType::WebP);
    }

    #[test]
    fn test_detect_media_type_png() {
        let data = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect_media_type(&data), Some(MediaType::Png));
    }

    #[test]
    fn test_detect_media_type_jpeg() {
        let data = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        assert_eq!(detect_media_type(&data), Some(MediaType::Jpeg));
    }

    #[test]
    fn test_detect_media_type_gif87a() {
        let data = b"GIF87a\x01\x00\x01\x00";
        assert_eq!(detect_media_type(data), Some(MediaType::Gif));
    }

    #[test]
    fn test_detect_media_type_gif89a() {
        let data = b"GIF89a\x01\x00\x01\x00";
        assert_eq!(detect_media_type(data), Some(MediaType::Gif));
    }

    #[test]
    fn test_detect_media_type_webp() {
        let mut data = [0u8; 12];
        data[..4].copy_from_slice(b"RIFF");
        data[4..8].copy_from_slice(&[0x00, 0x00, 0x00, 0x00]); // rozmiar pliku
        data[8..12].copy_from_slice(b"WEBP");
        assert_eq!(detect_media_type(&data), Some(MediaType::WebP));
    }

    #[test]
    fn test_detect_media_type_riff_not_webp() {
        let mut data = [0u8; 12];
        data[..4].copy_from_slice(b"RIFF");
        data[8..12].copy_from_slice(b"WAVE"); // AVI/WAV — nie WebP
        assert_eq!(detect_media_type(&data), None);
    }

    #[test]
    fn test_detect_media_type_unknown() {
        let data = [0x00, 0x01, 0x02, 0x03];
        assert_eq!(detect_media_type(&data), None);
    }

    #[test]
    fn test_detect_media_type_empty() {
        assert_eq!(detect_media_type(&[]), None);
    }

    #[test]
    fn test_detect_media_type_short_data() {
        // Mniej niż 12 bajtów — nie powinno panikować
        let data = [0x89, 0x50]; // za krótkie nawet na PNG (4 bajty)
        assert_eq!(detect_media_type(&data), None);

        // Dokładnie 3 bajty — rozpoznaje JPEG
        let jpeg_short = [0xFF, 0xD8, 0xFF];
        assert_eq!(detect_media_type(&jpeg_short), Some(MediaType::Jpeg));

        // 11 bajtów RIFF — za krótkie na WebP (potrzeba 12)
        let riff_short = b"RIFF\x00\x00\x00WEB";
        assert_eq!(detect_media_type(riff_short), None);
    }

    #[test]
    fn test_encode_base64_known_value() {
        // "test" → base64 STANDARD to "dGVzdA=="
        assert_eq!(encode_base64(b"test"), "dGVzdA==");
    }

    #[test]
    fn test_encode_base64_empty() {
        assert_eq!(encode_base64(b""), "");
    }

    #[test]
    fn test_encode_base64_roundtrip() {
        use base64::{Engine as _, engine::general_purpose};
        let original = b"\x89PNG\r\n\x1a\nsome_image_bytes";
        let encoded = encode_base64(original);
        // Brak prefixu data:
        assert!(!encoded.starts_with("data:"));
        // Dekodowanie zwraca identyczne bajty
        let decoded = general_purpose::STANDARD.decode(&encoded).unwrap();
        assert_eq!(decoded.as_slice(), original.as_slice());
    }

    #[test]
    fn test_encode_base64_no_data_prefix() {
        let encoded = encode_base64(b"hello world");
        assert!(!encoded.starts_with("data:"));
    }

    /// Roundtrip z pseudolosowymi danymi (z szerokim spektrum bajt-wartości).
    /// Weryfikuje że encode→decode daje identyczne dane dla arbitralnych bajt-wartości.
    #[test]
    fn test_encode_base64_roundtrip_random_data() {
        use base64::{Engine as _, engine::general_purpose};

        // Prosty LCG (Linear Congruential Generator) — brak zależności od crate `rand`
        let mut state: u64 = 0xDEAD_BEEF_CAFE_1337;
        let data: Vec<u8> = (0..1024)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 56) as u8
            })
            .collect();

        let encoded = encode_base64(&data);

        // Brak prefixu data:
        assert!(!encoded.starts_with("data:"));

        // Decode roundtrip
        let decoded = general_purpose::STANDARD.decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    /// Roundtrip dla dużych danych (1 MB) — sprawdza wydajność i poprawność dla dużych plików.
    #[test]
    fn test_encode_base64_large_data_roundtrip() {
        use base64::{Engine as _, engine::general_purpose};

        const ONE_MB: usize = 1024 * 1024;

        // Generuj 1MB pseudolosowych danych przez LCG
        let mut state: u64 = 0xABCD_EF01_2345_6789;
        let data: Vec<u8> = (0..ONE_MB)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 56) as u8
            })
            .collect();

        assert_eq!(data.len(), ONE_MB);

        let encoded = encode_base64(&data);

        // Brak prefixu data:
        assert!(!encoded.starts_with("data:"));

        // Base64 zwiększa rozmiar o ~4/3
        assert!(encoded.len() > ONE_MB);

        // Decode roundtrip
        let decoded = general_purpose::STANDARD.decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    // --- validate_image_file ---

    /// Pomocnik: tworzy tymczasowy plik z podanymi bajtami.
    fn tmp_file_with(bytes: &[u8]) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("tempfile");
        f.write_all(bytes).expect("write");
        f
    }

    #[test]
    fn test_validate_image_file_png_ok() {
        let header: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let f = tmp_file_with(header);
        let (data, mt) = validate_image_file(f.path()).expect("valid PNG");
        assert_eq!(mt, MediaType::Png);
        assert_eq!(data, header);
    }

    #[test]
    fn test_validate_image_file_jpeg_ok() {
        let header: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let f = tmp_file_with(header);
        let (_, mt) = validate_image_file(f.path()).expect("valid JPEG");
        assert_eq!(mt, MediaType::Jpeg);
    }

    #[test]
    fn test_validate_image_file_gif_ok() {
        let f = tmp_file_with(b"GIF89a\x01\x00\x01\x00");
        let (_, mt) = validate_image_file(f.path()).expect("valid GIF");
        assert_eq!(mt, MediaType::Gif);
    }

    #[test]
    fn test_validate_image_file_webp_ok() {
        let mut data = [0u8; 12];
        data[..4].copy_from_slice(b"RIFF");
        data[8..12].copy_from_slice(b"WEBP");
        let f = tmp_file_with(&data);
        let (_, mt) = validate_image_file(f.path()).expect("valid WebP");
        assert_eq!(mt, MediaType::WebP);
    }

    #[test]
    fn test_validate_image_file_missing_path() {
        let path = std::path::Path::new("/tmp/__ralph_nonexistent_image_xyz.png");
        let err = validate_image_file(path).expect_err("should fail");
        match err {
            RalphError::MissingFile(msg) => {
                // Komunikat zawiera pełną ścieżkę
                assert!(
                    msg.contains("__ralph_nonexistent_image_xyz"),
                    "path in error: {msg}"
                );
            }
            other => panic!("expected MissingFile, got: {other:?}"),
        }
    }

    #[test]
    fn test_validate_image_file_unsupported_format() {
        // Dane losowe — żaden format nie pasuje
        let f = tmp_file_with(&[0x00, 0x01, 0x02, 0x03, 0x04]);
        let err = validate_image_file(f.path()).expect_err("should fail");
        match err {
            RalphError::UnsupportedImageFormat { path, supported } => {
                // Ścieżka jest w komunikacie
                assert!(!path.is_empty(), "path should not be empty");
                // Lista formatów jest w komunikacie
                assert!(
                    supported.contains("PNG"),
                    "supported should list PNG: {supported}"
                );
                assert!(
                    supported.contains("JPEG"),
                    "supported should list JPEG: {supported}"
                );
                assert!(
                    supported.contains("GIF"),
                    "supported should list GIF: {supported}"
                );
                assert!(
                    supported.contains("WebP"),
                    "supported should list WebP: {supported}"
                );
            }
            other => panic!("expected UnsupportedImageFormat, got: {other:?}"),
        }
    }

    #[test]
    fn test_validate_image_file_empty_file() {
        // Pusty plik — detect_media_type(&[]) zwraca None → UnsupportedImageFormat
        let f = tmp_file_with(&[]);
        let err = validate_image_file(f.path()).expect_err("empty file should fail");
        assert!(
            matches!(err, RalphError::UnsupportedImageFormat { .. }),
            "expected UnsupportedImageFormat, got: {err:?}"
        );
    }

    #[test]
    fn test_validate_image_file_error_message_readable() {
        // Sprawdź że Display komunikatu błędu jest czytelny
        let path = std::path::Path::new("/tmp/__ralph_test.xyz");
        if let Err(e) = validate_image_file(path) {
            let msg = e.to_string();
            assert!(!msg.is_empty());
        }

        let f = tmp_file_with(b"\x00\x00\x00\x00");
        if let Err(e) = validate_image_file(f.path()) {
            let msg = e.to_string();
            // Komunikat zawiera informację o obsługiwanych formatach
            assert!(
                msg.contains("PNG") || msg.contains("Obsługiwane"),
                "Error should mention supported formats: {msg}"
            );
        }
    }

    // ─── resize_if_needed ────────────────────────────────────────────────────

    #[test]
    fn test_resize_small_image_unchanged() {
        // Obraz poniżej limitu — oryginalne bajty powinny wrócić bez zmian
        let data = make_png(100, 100);
        let result = resize_if_needed(&data, &MediaType::Png, MAX_DIMENSION).unwrap();
        assert_eq!(result, data, "Mały obraz nie powinien być modyfikowany");
    }

    #[test]
    fn test_resize_exact_limit_unchanged() {
        // Obraz dokładnie na granicy limitu — bez zmian
        let data = make_png(MAX_DIMENSION, MAX_DIMENSION);
        let result = resize_if_needed(&data, &MediaType::Png, MAX_DIMENSION).unwrap();
        assert_eq!(
            result, data,
            "Obraz na granicy limitu nie powinien być modyfikowany"
        );
    }

    #[test]
    fn test_resize_wide_image_reduced() {
        // Szeroki obraz — szerokość zostaje zmniejszona do max_dim, wysokość proporcjonalnie
        let data = make_png(3000, 1000);
        let result = resize_if_needed(&data, &MediaType::Png, MAX_DIMENSION).unwrap();

        let resized = image::load_from_memory(&result).unwrap();
        assert_eq!(
            resized.width(),
            MAX_DIMENSION,
            "Szerokość powinna wynosić max_dim"
        );
        assert!(
            resized.height() <= MAX_DIMENSION,
            "Wysokość nie może przekraczać max_dim"
        );
        // Proporcje: 3000:1000 = 3:1 → oczekiwana wysokość ≈ 1568/3 ≈ 523
        let expected_h = (1000.0 * MAX_DIMENSION as f64 / 3000.0).round() as u32;
        assert_eq!(
            resized.height(),
            expected_h,
            "Wysokość powinna być proporcjonalna"
        );
    }

    #[test]
    fn test_resize_tall_image_reduced() {
        // Wysoki obraz — wysokość zostaje zmniejszona do max_dim, szerokość proporcjonalnie
        let data = make_png(800, 4000);
        let result = resize_if_needed(&data, &MediaType::Png, MAX_DIMENSION).unwrap();

        let resized = image::load_from_memory(&result).unwrap();
        assert_eq!(
            resized.height(),
            MAX_DIMENSION,
            "Wysokość powinna wynosić max_dim"
        );
        assert!(
            resized.width() <= MAX_DIMENSION,
            "Szerokość nie może przekraczać max_dim"
        );
    }

    #[test]
    fn test_resize_preserves_jpeg_format() {
        // Wynik dla JPEG powinien być wykrywalny jako JPEG
        let data = make_jpeg(2000, 1500);
        let result = resize_if_needed(&data, &MediaType::Jpeg, MAX_DIMENSION).unwrap();

        // Dane wynikowe nie mogą być puste
        assert!(!result.is_empty());
        // Wynik powinien dekodować się jako prawidłowy obraz
        let resized = image::load_from_memory(&result).unwrap();
        assert_eq!(resized.width(), MAX_DIMENSION);
    }

    #[test]
    fn test_resize_square_image() {
        // Obraz kwadratowy — oba wymiary skalowane do max_dim
        let data = make_png(2000, 2000);
        let result = resize_if_needed(&data, &MediaType::Png, MAX_DIMENSION).unwrap();

        let resized = image::load_from_memory(&result).unwrap();
        assert_eq!(resized.width(), MAX_DIMENSION);
        assert_eq!(resized.height(), MAX_DIMENSION);
    }

    #[test]
    fn test_resize_one_dimension_just_above_limit() {
        // Szerokość minimalnie przekracza limit, wysokość poniżej — typowy landscape foto
        // 1600x1000: tylko szerokość > 1568, wysokość OK
        let data = make_png(1600, 1000);
        let result = resize_if_needed(&data, &MediaType::Png, MAX_DIMENSION).unwrap();

        let resized = image::load_from_memory(&result).unwrap();
        assert_eq!(resized.width(), MAX_DIMENSION, "Szerokość powinna być obcięta do max_dim");
        assert!(resized.height() < MAX_DIMENSION, "Wysokość powinna być < max_dim");
        // Proporcje: 1600:1000 → 1568:980
        let expected_h = (1000.0 * MAX_DIMENSION as f64 / 1600.0).round() as u32;
        assert_eq!(resized.height(), expected_h, "Wysokość powinna być proporcjonalna");
    }

    #[test]
    fn test_resize_invalid_data_returns_error() {
        let garbage = b"not an image at all";
        let result = resize_if_needed(garbage, &MediaType::Png, MAX_DIMENSION);
        assert!(result.is_err(), "Nieprawidłowe dane powinny zwrócić błąd");
        // Sprawdź że to ImageProcessing error
        let err = result.unwrap_err();
        assert!(
            matches!(err, RalphError::ImageProcessing(_)),
            "Błąd powinien być ImageProcessing, got: {:?}",
            err
        );
    }

    #[test]
    fn test_media_type_to_image_format() {
        // Sprawdź konwersje MediaType → image::ImageFormat
        assert_eq!(
            image::ImageFormat::from(&MediaType::Png),
            image::ImageFormat::Png
        );
        assert_eq!(
            image::ImageFormat::from(&MediaType::Jpeg),
            image::ImageFormat::Jpeg
        );
        assert_eq!(
            image::ImageFormat::from(&MediaType::Gif),
            image::ImageFormat::Gif
        );
        assert_eq!(
            image::ImageFormat::from(&MediaType::WebP),
            image::ImageFormat::WebP
        );
    }
}
