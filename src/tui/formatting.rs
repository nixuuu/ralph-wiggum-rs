//! Centralne formatowanie liczb (tokeny, czas) dla TUI i non-TUI kontekstów.
//!
//! Span warianty zwracają `Span::raw()` (bez stylu) — callsites same aplikują
//! kolory z Theme, bo różne konteksty wymagają różnych stylów
//! (np. input tokens = green, output = magenta).

use ratatui::text::Span;
use std::time::Duration;
use unicode_width::UnicodeWidthChar;

/// Format tokens as an unstyled Span for TUI composition.
///
/// Converts token count to human-readable format:
/// - 0-999: raw number (e.g., "123")
/// - 1,000-999,999: with 'k' suffix (e.g., "1.2k")
/// - 1,000,000+: with 'M' suffix (e.g., "3.5M")
///
/// # Examples
/// ```
/// use ralph_wiggum::tui::formatting::format_tokens_span;
/// use ratatui::style::Style;
///
/// let span = format_tokens_span(1234);
/// assert_eq!(span.content, "1.2k");
/// ```
#[must_use]
#[allow(dead_code)] // API przygotowane dla przyszłych implementacji
pub fn format_tokens_span(tokens: u64) -> Span<'static> {
    let text = format_tokens_string(tokens);
    Span::raw(text)
}

/// Format tokens for non-TUI contexts (e.g., final summary, logging).
///
/// Same formatting logic as `format_tokens_span` but returns a String.
/// Used when styling is not needed (backward compatibility).
///
/// # Examples
/// ```
/// use ralph_wiggum::tui::formatting::format_tokens_string;
///
/// assert_eq!(format_tokens_string(1234), "1.2k");
/// assert_eq!(format_tokens_string(0), "0");
/// ```
#[must_use]
pub fn format_tokens_string(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 10_000 {
        // Use k suffix but without decimal for cleaner display at high k values
        format!("{:.0}k", tokens as f64 / 1_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

/// Format duration as a styled Span for TUI display.
///
/// Converts seconds to human-readable short duration:
/// - < 60 sec: "~{seconds}s" (e.g., "~45s")
/// - < 1 hour: "~{minutes}m" (e.g., "~23m")
/// - >= 1 hour: "~{hours}h{minutes}m" (e.g., "~1h05m")
///
/// # Examples
/// ```
/// use ralph_wiggum::tui::formatting::format_duration_span;
/// use std::time::Duration;
///
/// let span = format_duration_span(Duration::from_secs(90));
/// assert_eq!(span.content, "~1m");
/// ```
#[must_use]
#[allow(dead_code)] // API przygotowane dla przyszłych implementacji
pub fn format_duration_span(duration: Duration) -> Span<'static> {
    let text = format_duration_string(duration.as_secs());
    Span::raw(text)
}

/// Format duration in seconds for non-TUI contexts.
///
/// Same formatting logic as `format_duration_span` but accepts seconds as u64
/// and returns a String. Used when styling is not needed (backward compatibility).
///
/// # Examples
/// ```
/// use ralph_wiggum::tui::formatting::format_duration_string;
///
/// assert_eq!(format_duration_string(45), "~45s");
/// assert_eq!(format_duration_string(90), "~1m");
/// assert_eq!(format_duration_string(3900), "~1h05m");
/// ```
#[must_use]
pub fn format_duration_string(total_secs: u64) -> String {
    if total_secs < 60 {
        format!("~{}s", total_secs)
    } else if total_secs < 3600 {
        let mins = total_secs / 60;
        format!("~{}m", mins)
    } else {
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        if mins == 0 {
            format!("~{}h", hours)
        } else {
            format!("~{}h{:02}m", hours, mins)
        }
    }
}

/// Mapuje kolumnę terminala (wizualną) na indeks znaku (char index) w stringu.
///
/// Uwzględnia znaki o szerokości 2 kolumn (CJK, znaki pełnej szerokości),
/// gdzie jeden char zajmuje dwie kolumny terminala.
///
/// Przydatne przy obsłudze kliknięcia myszką w polu tekstowym — terminal
/// raportuje kolumnę piksela, a my potrzebujemy indeksu znaku w buforze.
///
/// # Parametry
/// - `text` — tekst źródłowy
/// - `target_col` — kolumna terminala (0-indexed)
///
/// # Zwraca
/// Indeks znaku (char index) odpowiadający kolumnie `target_col`.
/// Jeśli `target_col` przekracza szerokość tekstu, zwraca `text.chars().count()`.
///
/// # Przykłady
/// ```
/// use ralph_wiggum::tui::formatting::unicode_column_to_char_index;
///
/// // ASCII: każdy znak = 1 kolumna
/// assert_eq!(unicode_column_to_char_index("Hello", 0), 0);
/// assert_eq!(unicode_column_to_char_index("Hello", 5), 5);
///
/// // CJK: każdy znak = 2 kolumny → kolumna 4 = char index 2
/// assert_eq!(unicode_column_to_char_index("你好世界", 4), 2);
///
/// // Poza zakresem → zwraca chars().count()
/// assert_eq!(unicode_column_to_char_index("Hi", 100), 2);
/// ```
#[must_use]
pub fn unicode_column_to_char_index(text: &str, target_col: usize) -> usize {
    let mut col = 0;
    for (i, ch) in text.chars().enumerate() {
        if col >= target_col {
            return i;
        }
        col += UnicodeWidthChar::width(ch).unwrap_or(1);
    }
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============ format_tokens_string tests ============

    #[test]
    fn test_format_tokens_string_zero() {
        assert_eq!(format_tokens_string(0), "0");
    }

    #[test]
    fn test_format_tokens_string_small() {
        assert_eq!(format_tokens_string(42), "42");
        assert_eq!(format_tokens_string(123), "123");
        assert_eq!(format_tokens_string(999), "999");
    }

    #[test]
    fn test_format_tokens_string_thousands() {
        assert_eq!(format_tokens_string(1_000), "1.0k");
        assert_eq!(format_tokens_string(1_234), "1.2k");
        assert_eq!(format_tokens_string(9_876), "9.9k");
        assert_eq!(format_tokens_string(10_000), "10k");
        assert_eq!(format_tokens_string(99_999), "100k");
        assert_eq!(format_tokens_string(123_456), "123k");
        assert_eq!(format_tokens_string(999_999), "1000k");
    }

    #[test]
    fn test_format_tokens_string_millions() {
        assert_eq!(format_tokens_string(1_000_000), "1.0M");
        assert_eq!(format_tokens_string(3_456_789), "3.5M");
        assert_eq!(format_tokens_string(10_000_000), "10.0M");
    }

    // ============ format_tokens_span tests ============

    #[test]
    fn test_format_tokens_span_zero() {
        let span = format_tokens_span(0);
        assert_eq!(span.content, "0");
    }

    #[test]
    fn test_format_tokens_span_small() {
        let span = format_tokens_span(42);
        assert_eq!(span.content, "42");

        let span = format_tokens_span(999);
        assert_eq!(span.content, "999");
    }

    #[test]
    fn test_format_tokens_span_thousands() {
        let span = format_tokens_span(1_000);
        assert_eq!(span.content, "1.0k");

        let span = format_tokens_span(1_234);
        assert_eq!(span.content, "1.2k");

        let span = format_tokens_span(123_456);
        assert_eq!(span.content, "123k");
    }

    #[test]
    fn test_format_tokens_span_millions() {
        let span = format_tokens_span(1_000_000);
        assert_eq!(span.content, "1.0M");

        let span = format_tokens_span(3_456_789);
        assert_eq!(span.content, "3.5M");
    }

    // ============ format_duration_string tests ============

    #[test]
    fn test_format_duration_string_seconds() {
        assert_eq!(format_duration_string(0), "~0s");
        assert_eq!(format_duration_string(45), "~45s");
        assert_eq!(format_duration_string(59), "~59s");
    }

    #[test]
    fn test_format_duration_string_minutes() {
        assert_eq!(format_duration_string(60), "~1m");
        assert_eq!(format_duration_string(90), "~1m");
        assert_eq!(format_duration_string(720), "~12m");
        assert_eq!(format_duration_string(3599), "~59m");
    }

    #[test]
    fn test_format_duration_string_hours() {
        assert_eq!(format_duration_string(3600), "~1h");
        assert_eq!(format_duration_string(3900), "~1h05m");
        assert_eq!(format_duration_string(7200), "~2h");
        assert_eq!(format_duration_string(9000), "~2h30m");
    }

    // ============ format_duration_span tests ============

    #[test]
    fn test_format_duration_span_seconds() {
        let span = format_duration_span(Duration::from_secs(0));
        assert_eq!(span.content, "~0s");

        let span = format_duration_span(Duration::from_secs(45));
        assert_eq!(span.content, "~45s");

        let span = format_duration_span(Duration::from_secs(59));
        assert_eq!(span.content, "~59s");
    }

    #[test]
    fn test_format_duration_span_minutes() {
        let span = format_duration_span(Duration::from_secs(60));
        assert_eq!(span.content, "~1m");

        let span = format_duration_span(Duration::from_secs(90));
        assert_eq!(span.content, "~1m");

        let span = format_duration_span(Duration::from_secs(720));
        assert_eq!(span.content, "~12m");
    }

    #[test]
    fn test_format_duration_span_hours() {
        let span = format_duration_span(Duration::from_secs(3600));
        assert_eq!(span.content, "~1h");

        let span = format_duration_span(Duration::from_secs(3900));
        assert_eq!(span.content, "~1h05m");

        let span = format_duration_span(Duration::from_secs(9000));
        assert_eq!(span.content, "~2h30m");
    }

    // ============ unicode_column_to_char_index tests ============

    #[test]
    fn test_unicode_col_ascii_basic() {
        // ASCII: każdy znak = 1 kolumna → kolumna == char index
        assert_eq!(unicode_column_to_char_index("Hello", 0), 0);
        assert_eq!(unicode_column_to_char_index("Hello", 1), 1);
        assert_eq!(unicode_column_to_char_index("Hello", 3), 3);
        assert_eq!(unicode_column_to_char_index("Hello", 5), 5);
    }

    #[test]
    fn test_unicode_col_ascii_out_of_range() {
        // Poza zakresem → zwraca chars().count()
        assert_eq!(unicode_column_to_char_index("Hi", 100), 2);
        assert_eq!(unicode_column_to_char_index("", 0), 0);
        assert_eq!(unicode_column_to_char_index("", 5), 0);
    }

    #[test]
    fn test_unicode_col_cjk_double_width() {
        // CJK: każdy znak zajmuje 2 kolumny terminala
        // "你好世界" = 4 znaki × 2 kolumny = 8 kolumn
        assert_eq!(unicode_column_to_char_index("你好世界", 0), 0);
        assert_eq!(unicode_column_to_char_index("你好世界", 2), 1);
        assert_eq!(unicode_column_to_char_index("你好世界", 4), 2);
        assert_eq!(unicode_column_to_char_index("你好世界", 6), 3);
        // col=8 → poza zakresem → 4
        assert_eq!(unicode_column_to_char_index("你好世界", 8), 4);
    }

    #[test]
    fn test_unicode_col_cjk_mid_char() {
        // Kolumna w środku CJK znaku (nieparzysta) → zwraca następny char index
        // "你" ma szerokość 2; col=1 to środek znaku → powinien zwrócić char 1
        assert_eq!(unicode_column_to_char_index("你好", 1), 1);
        assert_eq!(unicode_column_to_char_index("你好", 3), 2);
    }

    #[test]
    fn test_unicode_col_mixed_ascii_cjk() {
        // Mieszane: "Hi你" = H(col0), i(col1), 你(col2-3)
        assert_eq!(unicode_column_to_char_index("Hi你", 0), 0); // H
        assert_eq!(unicode_column_to_char_index("Hi你", 1), 1); // i
        assert_eq!(unicode_column_to_char_index("Hi你", 2), 2); // 你 (start)
        assert_eq!(unicode_column_to_char_index("Hi你", 3), 3); // za 你 (mid → next)
        assert_eq!(unicode_column_to_char_index("Hi你", 4), 3); // poza zakresem
    }

    #[test]
    fn test_unicode_col_emoji() {
        // Emoji zajmują 2 kolumny terminala
        // "🔥AB" = 🔥(col0-1), A(col2), B(col3)
        assert_eq!(unicode_column_to_char_index("🔥AB", 0), 0); // 🔥
        assert_eq!(unicode_column_to_char_index("🔥AB", 2), 1); // A
        assert_eq!(unicode_column_to_char_index("🔥AB", 3), 2); // B
    }
}
