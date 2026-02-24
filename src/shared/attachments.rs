//! Typy dołączników dla wiadomości Claude API.
//!
//! Obsługuje obrazy w formatach PNG, JPEG, GIF i WebP,
//! kodowane jako base64 i przesyłane inline w żądaniu API.
//!
//! Typy są publicznym API — będą używane przez przyszłe moduły (np. attachment loader).
// TODO: remove when attachment loader is implemented
#![allow(dead_code)]
use base64::{Engine as _, engine::general_purpose};
use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
