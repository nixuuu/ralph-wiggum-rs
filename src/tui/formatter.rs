#![allow(dead_code)] // API przygotowane dla przyszłych implementacji

//! RatuiFormatter trait - interfejs formatowania eventów Claude CLI jako ratatui Line/Span.
//!
//! Zastępuje obecne String-based formatowanie (z ANSI kodami crossterm) na natywne
//! typy ratatui, które mogą być bezpośrednio renderowane przez TUI widgets.
//!
//! Trait jest generyczny względem typu eventu, dzięki czemu może być używany
//! zarówno z ClaudeEvent (z commands::run), jak i innymi typami eventów.
//!
//! # Przykład użycia
//!
//! ```ignore
//! use ralph_wiggum::tui::{RatuiFormatter, colored_bold_span, muted_span};
//! use ratatui::text::Line;
//!
//! struct MyFormatter {
//!     iteration: u32,
//! }
//!
//! impl RatuiFormatter<MyEvent> for MyFormatter {
//!     fn format_event(&mut self, event: &MyEvent) -> Vec<Line<'static>> {
//!         let mut lines = Vec::new();
//!
//!         // Przykład formatowania z użyciem helper functions
//!         let line = Line::from(vec![
//!             colored_bold_span("Event:", Color::Cyan),
//!             Span::raw(" "),
//!             muted_span(event.name()),
//!         ]);
//!         lines.push(line);
//!
//!         lines
//!     }
//!
//!     fn format_iteration_header(&self) -> Vec<Line<'static>> {
//!         vec![Line::from(colored_bold_span(
//!             format!("Iteration {}", self.iteration),
//!             Color::Cyan
//!         ))]
//!     }
//!
//!     fn format_stats(&self) -> Vec<Line<'static>> {
//!         vec![Line::from(muted_span("Stats here"))]
//!     }
//! }
//! ```

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Trait definiujący interfejs formatowania eventów Claude CLI jako ratatui widgets.
///
/// Implementacje tego traita odpowiadają za konwersję eventów Claude CLI
/// (assistant messages, tool calls, results) na formatowane linie ratatui.
///
/// # Typ generyczny
///
/// - `Event`: Typ eventu do formatowania (np. ClaudeEvent z commands::run)
pub trait RatuiFormatter<Event> {
    /// Formatuje event Claude CLI jako wektor linii ratatui.
    ///
    /// Każda linia może zawierać wiele Span-ów z różnymi stylami.
    ///
    /// # Argumenty
    ///
    /// * `event` - Event do sformatowania
    fn format_event(&mut self, event: &Event) -> Vec<Line<'static>>;

    /// Formatuje nagłówek iteracji (separator, numer iteracji, elapsed time).
    ///
    /// # Zwraca
    ///
    /// Wektor linii reprezentujących nagłówek iteracji z separatorami i metadanymi.
    fn format_iteration_header(&self) -> Vec<Line<'static>>;

    /// Formatuje końcowe statystyki (tokeny, koszt, czas, speed).
    ///
    /// # Zwraca
    ///
    /// Wektor linii przedstawiających końcowe statystyki sesji.
    fn format_stats(&self) -> Vec<Line<'static>>;
}

/// Pomocnicza funkcja do tworzenia Span ze stylem.
///
/// # Przykłady
///
/// ```ignore
/// let span = styled_span("Hello", Style::default().fg(Color::Green));
/// let span = styled_span("World", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
/// ```
pub fn styled_span(text: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(text.into(), style)
}

/// Pomocnicza funkcja do tworzenia Span z ikoną i kolorem.
/// Semantyczny alias dla [`colored_span`] — czytelniejszy przy ikonach/emoji.
///
/// # Przykłady
///
/// ```ignore
/// let span = icon_span("✓", Color::Green);
/// let span = icon_span("✗", Color::Red);
/// ```
pub fn icon_span(icon: impl Into<String>, color: Color) -> Span<'static> {
    colored_span(icon, color)
}

/// Pomocnicza funkcja do tworzenia Span z tekstem pogrubionym.
pub fn bold_span(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().add_modifier(Modifier::BOLD))
}

/// Pomocnicza funkcja do tworzenia Span z tekstem wyciszonym (dark grey).
pub fn muted_span(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(Color::DarkGray))
}

/// Pomocnicza funkcja do tworzenia Span z kolorowym tekstem.
pub fn colored_span(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(color))
}

/// Pomocnicza funkcja do tworzenia Span z kolorowym, pogrubionym tekstem.
pub fn colored_bold_span(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

// ============ Border helpers (opencode-inspired message blocks) ============

/// Tworzy border Span: `"▍"` (left three-eighths block + space) w danym kolorze.
/// Używany jako prefix każdej linii w bordered message blocks.
pub fn border_span(color: Color) -> Span<'static> {
    Span::styled("▍".to_string(), Style::default().fg(color))
}

/// Tworzy labeled header line: `"▍Label"` z bold label w kolorze bordera.
/// Np. `"▍Claude"` (Cyan+Bold) lub `"▍You"` (Blue+Bold).
pub fn label_line(label: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        border_span(color),
        Span::styled(
            label.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Tworzy pustą linię separatora między blokami wiadomości.
pub fn separator_line() -> Line<'static> {
    Line::default()
}

/// Dodaje border span na początek każdej linii.
/// Konwertuje `Vec<Line>` → `Vec<Line>` z `"█ "` prefix w danym kolorze.
pub fn prepend_border(lines: Vec<Line<'static>>, color: Color) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let mut spans = vec![border_span(color)];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

/// Tworzy timing footer line z tokenami: `"█  ↑1.2K ↓450"`.
/// Renderuje input/output tokens w muted style po borderze.
pub fn timing_line(color: Color, input_tokens: u64, output_tokens: u64) -> Line<'static> {
    use crate::tui::formatting::format_tokens_string;

    Line::from(vec![
        border_span(color),
        Span::styled(
            format!(
                " {}↑{} ↓{}",
                "",
                format_tokens_string(input_tokens),
                format_tokens_string(output_tokens)
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

// ============ Line → ANSI String conversion ============

/// Konwertuje ratatui `Line` na String z ANSI escape codes.
///
/// Używane w orchestrate pipeline, gdzie protocol wymaga `Vec<String>` (serde),
/// a ring buffer parsuje ANSI z powrotem na styled `Line` via `ansi_to_tui`.
pub fn line_to_ansi(line: &Line<'_>) -> String {
    use std::fmt::Write;

    let mut result = String::new();
    for span in &line.spans {
        let style = &span.style;
        let mut codes = Vec::new();

        // Modifiers
        if style.add_modifier.contains(Modifier::BOLD) {
            codes.push("1".to_string());
        }
        if style.add_modifier.contains(Modifier::DIM) {
            codes.push("2".to_string());
        }
        if style.add_modifier.contains(Modifier::ITALIC) {
            codes.push("3".to_string());
        }
        if style.add_modifier.contains(Modifier::UNDERLINED) {
            codes.push("4".to_string());
        }

        // Foreground color
        if let Some(color) = style.fg {
            codes.push(color_to_fg_ansi(color));
        }

        // Background color
        if let Some(color) = style.bg {
            codes.push(color_to_bg_ansi(color));
        }

        if codes.is_empty() {
            result.push_str(&span.content);
        } else {
            let _ = write!(result, "\x1b[{}m{}\x1b[0m", codes.join(";"), span.content);
        }
    }
    result
}

fn color_to_fg_ansi(color: Color) -> String {
    match color {
        Color::Black => "30".to_string(),
        Color::Red => "31".to_string(),
        Color::Green => "32".to_string(),
        Color::Yellow => "33".to_string(),
        Color::Blue => "34".to_string(),
        Color::Magenta => "35".to_string(),
        Color::Cyan => "36".to_string(),
        Color::Gray => "37".to_string(),
        Color::DarkGray => "90".to_string(),
        Color::LightRed => "91".to_string(),
        Color::LightGreen => "92".to_string(),
        Color::LightYellow => "93".to_string(),
        Color::LightBlue => "94".to_string(),
        Color::LightMagenta => "95".to_string(),
        Color::LightCyan => "96".to_string(),
        Color::White => "97".to_string(),
        Color::Indexed(n) => format!("38;5;{n}"),
        Color::Rgb(r, g, b) => format!("38;2;{r};{g};{b}"),
        _ => String::new(),
    }
}

fn color_to_bg_ansi(color: Color) -> String {
    match color {
        Color::Black => "40".to_string(),
        Color::Red => "41".to_string(),
        Color::Green => "42".to_string(),
        Color::Yellow => "43".to_string(),
        Color::Blue => "44".to_string(),
        Color::Magenta => "45".to_string(),
        Color::Cyan => "46".to_string(),
        Color::Gray => "47".to_string(),
        Color::DarkGray => "100".to_string(),
        Color::LightRed => "101".to_string(),
        Color::LightGreen => "102".to_string(),
        Color::LightYellow => "103".to_string(),
        Color::LightBlue => "104".to_string(),
        Color::LightMagenta => "105".to_string(),
        Color::LightCyan => "106".to_string(),
        Color::White => "107".to_string(),
        Color::Indexed(n) => format!("48;5;{n}"),
        Color::Rgb(r, g, b) => format!("48;2;{r};{g};{b}"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_styled_span_creates_span_with_style() {
        let style = Style::default().fg(Color::Green);
        let span = styled_span("test", style);
        assert_eq!(span.content, "test");
        assert_eq!(span.style.fg, Some(Color::Green));
    }

    #[test]
    fn test_icon_span_creates_colored_icon() {
        let span = icon_span("✓", Color::Green);
        assert_eq!(span.content, "✓");
        assert_eq!(span.style.fg, Some(Color::Green));
    }

    #[test]
    fn test_bold_span_creates_bold_text() {
        let span = bold_span("bold text");
        assert_eq!(span.content, "bold text");
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_muted_span_creates_dark_grey_text() {
        let span = muted_span("muted");
        assert_eq!(span.content, "muted");
        assert_eq!(span.style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn test_colored_span_creates_colored_text() {
        let span = colored_span("colored", Color::Cyan);
        assert_eq!(span.content, "colored");
        assert_eq!(span.style.fg, Some(Color::Cyan));
    }

    #[test]
    fn test_colored_bold_span_creates_colored_bold_text() {
        let span = colored_bold_span("important", Color::Red);
        assert_eq!(span.content, "important");
        assert_eq!(span.style.fg, Some(Color::Red));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_helper_functions_return_static_lifetime() {
        let _: Span<'static> = styled_span("test", Style::default());
        let _: Span<'static> = icon_span("✓", Color::Green);
        let _: Span<'static> = bold_span("bold");
        let _: Span<'static> = muted_span("muted");
        let _: Span<'static> = colored_span("colored", Color::Blue);
        let _: Span<'static> = colored_bold_span("important", Color::Yellow);
    }

    // -- Testy border helpers --

    #[test]
    fn test_border_span_creates_full_block() {
        let span = border_span(Color::Cyan);
        assert_eq!(span.content, "▍");
        assert_eq!(span.style.fg, Some(Color::Cyan));
    }

    #[test]
    fn test_label_line_creates_labeled_header() {
        let line = label_line("Claude", Color::Cyan);
        assert_eq!(line.spans.len(), 2);
        // Border span
        assert_eq!(line.spans[0].content, "▍");
        assert_eq!(line.spans[0].style.fg, Some(Color::Cyan));
        // Label span
        assert_eq!(line.spans[1].content, "Claude");
        assert_eq!(line.spans[1].style.fg, Some(Color::Cyan));
        assert!(line.spans[1].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_separator_line_is_empty() {
        let line = separator_line();
        assert!(line.spans.is_empty());
    }

    #[test]
    fn test_prepend_border_adds_prefix_to_all_lines() {
        let lines = vec![
            Line::raw("First line".to_string()),
            Line::raw("Second line".to_string()),
        ];
        let bordered = prepend_border(lines, Color::Blue);
        assert_eq!(bordered.len(), 2);
        for line in &bordered {
            assert_eq!(line.spans[0].content, "▍");
            assert_eq!(line.spans[0].style.fg, Some(Color::Blue));
        }
    }

    #[test]
    fn test_timing_line_shows_tokens() {
        let line = timing_line(Color::Cyan, 1234, 567);
        assert_eq!(line.spans.len(), 2);
        // Border
        assert_eq!(line.spans[0].content, "▍");
        // Token info
        assert!(line.spans[1].content.contains("1.2k"));
        assert!(line.spans[1].content.contains("567"));
        assert_eq!(line.spans[1].style.fg, Some(Color::DarkGray));
    }

    // -- Test implementacji traita --

    /// Prosty event do testów traita
    #[derive(Debug)]
    struct TestEvent {
        message: String,
    }

    /// Mock implementacja RatuiFormatter weryfikująca poprawność interfejsu
    struct TestFormatter {
        iteration: u32,
        event_count: u32,
    }

    impl RatuiFormatter<TestEvent> for TestFormatter {
        fn format_event(&mut self, event: &TestEvent) -> Vec<Line<'static>> {
            self.event_count += 1;
            vec![Line::from(vec![
                colored_bold_span(format!("[{}]", self.event_count), Color::Cyan),
                Span::raw(" "),
                Span::raw(event.message.clone()),
            ])]
        }

        fn format_iteration_header(&self) -> Vec<Line<'static>> {
            vec![
                Line::from(muted_span("━".repeat(40))),
                Line::from(vec![
                    icon_span("▶", Color::Cyan),
                    Span::raw(" "),
                    bold_span(format!("Iteration {}", self.iteration)),
                ]),
            ]
        }

        fn format_stats(&self) -> Vec<Line<'static>> {
            vec![Line::from(vec![
                muted_span("Events: "),
                colored_span(self.event_count.to_string(), Color::Green),
            ])]
        }
    }

    #[test]
    fn test_trait_format_event_mutates_state() {
        let mut fmt = TestFormatter {
            iteration: 1,
            event_count: 0,
        };
        let event = TestEvent {
            message: "hello".into(),
        };

        let lines = fmt.format_event(&event);
        assert_eq!(lines.len(), 1);
        assert_eq!(fmt.event_count, 1);

        // Drugi event powinien zinkrementować licznik
        let _ = fmt.format_event(&event);
        assert_eq!(fmt.event_count, 2);
    }

    #[test]
    fn test_trait_format_iteration_header() {
        let fmt = TestFormatter {
            iteration: 3,
            event_count: 0,
        };
        let lines = fmt.format_iteration_header();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_trait_format_stats() {
        let fmt = TestFormatter {
            iteration: 1,
            event_count: 5,
        };
        let lines = fmt.format_stats();
        assert_eq!(lines.len(), 1);
    }

    // -- Testy line_to_ansi --

    #[test]
    fn test_line_to_ansi_plain_text() {
        let line = Line::raw("Hello world");
        assert_eq!(line_to_ansi(&line), "Hello world");
    }

    #[test]
    fn test_line_to_ansi_colored_text() {
        let line = Line::from(colored_span("test", Color::Cyan));
        assert_eq!(line_to_ansi(&line), "\x1b[36mtest\x1b[0m");
    }

    #[test]
    fn test_line_to_ansi_bold_colored() {
        let line = Line::from(colored_bold_span("Claude", Color::Cyan));
        assert_eq!(line_to_ansi(&line), "\x1b[1;36mClaude\x1b[0m");
    }

    #[test]
    fn test_line_to_ansi_mixed_spans() {
        let line = Line::from(vec![
            border_span(Color::Cyan),
            Span::raw(" "),
            colored_bold_span("Title", Color::Green),
        ]);
        let ansi = line_to_ansi(&line);
        assert!(ansi.contains("\x1b[36m▍\x1b[0m"));
        assert!(ansi.contains(" "));
        assert!(ansi.contains("\x1b[1;32mTitle\x1b[0m"));
    }

    #[test]
    fn test_line_to_ansi_roundtrip() {
        // Line → ANSI → ansi_to_tui → Line: style powinien przetrwać
        use ansi_to_tui::IntoText;

        let original = Line::from(vec![
            colored_span("▍", Color::Cyan),
            colored_bold_span("Claude", Color::Cyan),
        ]);

        let ansi_str = line_to_ansi(&original);
        let parsed = ansi_str.into_text().expect("ANSI parse should succeed");
        let restored = &parsed.lines[0];

        // Sprawdź że style przetrwały roundtrip
        assert_eq!(restored.spans.len(), original.spans.len());
        for (orig, rest) in original.spans.iter().zip(restored.spans.iter()) {
            assert_eq!(orig.content, rest.content);
            assert_eq!(orig.style.fg, rest.style.fg);
        }
    }
}
