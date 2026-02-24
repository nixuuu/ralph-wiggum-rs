//! Typy dołączników dla wiadomości Claude API.
//!
//! Obsługuje obrazy w formatach PNG, JPEG, GIF i WebP,
//! kodowane jako base64 i przesyłane inline w żądaniu API.
//!
//! Typy są publicznym API — będą używane przez przyszłe moduły (np. attachment loader).
// TODO: remove when attachment loader is implemented
#![allow(dead_code)]
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
}
