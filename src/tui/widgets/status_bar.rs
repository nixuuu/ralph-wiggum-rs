use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Gauge, Widget},
};

use crate::tui::Theme;

/// Dane dotyczące opcjonalnego progress gauge w status bar.
///
/// Gdy obecne, powoduje renderowanie drugiej linii z paskiem postępu.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressData {
    /// Liczba ukończonych zadań
    pub done: usize,
    /// Całkowita liczba zadań
    pub total: usize,
    /// Opcjonalny tekst ETA (np. "~23m")
    pub eta_text: Option<String>,
}

/// Dane wejściowe dla widgetu StatusBar.
///
/// Zawiera wszystkie dane potrzebne do wyrenderowania 1-2 linijkowego
/// status bar z metrykami (tokeny, koszt, czas) i opcjonalnym gauge postępu.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusBarData {
    /// Liczba tokenów wejściowych (↓)
    pub input_tokens: u64,
    /// Liczba tokenów wyjściowych (↑)
    pub output_tokens: u64,
    /// Koszt w USD ($)
    pub cost_usd: f64,
    /// Czas trwania w sekundach (⏱)
    pub elapsed_secs: f64,
    /// Opcjonalne dane progress gauge (done/total + ETA)
    pub progress: Option<ProgressData>,
    /// Keybinding hints jako pary (klawisz, opis).
    /// Dynamiczne stringi pozwalają na wyświetlanie aktualnych keybindingów z resolvera.
    /// Np. vec![("q".to_string(), "Quit".to_string())]
    pub hints: Vec<(String, String)>,
}

impl Default for StatusBarData {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            elapsed_secs: 0.0,
            progress: None,
            hints: Vec::new(),
        }
    }
}

impl StatusBarData {
    /// Formatuj liczbę tokenów do czytelnej formy (np. 1234 -> "1.2k").
    fn format_tokens(tokens: u64) -> String {
        if tokens >= 1_000_000 {
            format!("{:.1}M", tokens as f64 / 1_000_000.0)
        } else if tokens >= 1_000 {
            format!("{:.1}k", tokens as f64 / 1_000.0)
        } else {
            tokens.to_string()
        }
    }

    /// Zbuduj główną linię statusu z metrykami (tokeny ↓↑, cost $, elapsed ⏱).
    fn build_main_line(&self, theme: &Theme, use_nerd_font: bool) -> Line<'static> {
        let tokens_in = Self::format_tokens(self.input_tokens);
        let tokens_out = Self::format_tokens(self.output_tokens);
        let cost_text = format!("{:.4}", self.cost_usd);
        let time_text = format!("{:.1}s", self.elapsed_secs);

        let time_icon = if use_nerd_font { "" } else { "[t]" };

        let mut spans = vec![
            // Elapsed time ⏱
            Span::styled(
                format!("{} ", time_icon),
                Style::default().fg(theme.warning),
            ),
            Span::raw(format!("{} ", time_text)),
            Span::raw("│ "),
            // Input tokens ↓
            Span::styled("↓", Style::default().fg(theme.success)),
            Span::raw(format!(" {} ", tokens_in)),
            // Output tokens ↑
            Span::styled("↑", Style::default().fg(theme.primary)),
            Span::raw(format!(" {} ", tokens_out)),
            Span::raw("│ "),
            // Cost $
            Span::styled("$", Style::default().fg(theme.warning)),
            Span::raw(cost_text),
        ];

        // Dodaj keybinding hints jeśli istnieją
        if !self.hints.is_empty() {
            spans.push(Span::raw(" │ "));
            for (i, (key, desc)) in self.hints.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.push(Span::styled(key.clone(), theme.muted_style()));
                spans.push(Span::raw(format!(" {}", desc)));
            }
        }

        Line::from(spans)
    }

    /// Zbuduj opcjonalną linię progress gauge (jeśli progress jest Some).
    fn build_progress_gauge(&self, theme: &Theme) -> Option<Gauge<'static>> {
        let progress = self.progress.as_ref()?;

        let ratio = if progress.total > 0 {
            (progress.done as f64 / progress.total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let label = if let Some(ref eta) = progress.eta_text {
            format!(
                "{}/{} ({}%) | ETA {}",
                progress.done,
                progress.total,
                (ratio * 100.0).round() as u32,
                eta
            )
        } else {
            format!(
                "{}/{} ({}%)",
                progress.done,
                progress.total,
                (ratio * 100.0).round() as u32
            )
        };

        let gauge = Gauge::default()
            .ratio(ratio)
            .label(label)
            .gauge_style(Style::default().fg(theme.success).bg(theme.status_bar_bg))
            .style(Style::default().fg(theme.muted));

        Some(gauge)
    }
}

/// Reużywalny widget status bar: 1-2 linie z metrykami i opcjonalnym gauge.
///
/// # Layout
/// - **1 linia** (bez progress): tokeny ↓↑, cost $, elapsed ⏱, keybinding hints
/// - **2 linie** (z progress): pierwsza linia jak wyżej + druga linia z gauge
///
/// # Przykład użycia
/// ```rust,ignore
/// use ralph_wiggum::tui::{StatusBar, StatusBarData, DEFAULT_THEME};
///
/// let data = StatusBarData {
///     input_tokens: 15234,
///     output_tokens: 8976,
///     cost_usd: 0.4231,
///     elapsed_secs: 42.7,
///     progress: None,
///     hints: vec![("q".to_string(), "Quit".to_string()), ("r".to_string(), "Restart".to_string())],
/// };
///
/// let widget = StatusBar::new(data, &DEFAULT_THEME, false);
/// frame.render_widget(widget, area);
/// ```
pub struct StatusBar<'a> {
    data: StatusBarData,
    theme: &'a Theme,
    use_nerd_font: bool,
}

impl<'a> StatusBar<'a> {
    /// Utwórz nowy widget status bar.
    ///
    /// # Parametry
    /// - `data`: Dane do wyświetlenia (tokeny, koszt, czas, progress, hints)
    /// - `theme`: Paleta kolorów (z `crate::tui::Theme`)
    /// - `use_nerd_font`: Czy używać Nerd Font ikon (true) czy ASCII fallback (false)
    pub fn new(data: StatusBarData, theme: &'a Theme, use_nerd_font: bool) -> Self {
        Self {
            data,
            theme,
            use_nerd_font,
        }
    }

    /// Oblicz wymaganą wysokość widgetu (1 lub 2 linie).
    pub fn required_height(&self) -> u16 {
        if self.data.progress.is_some() { 2 } else { 1 }
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Główna linia statusu
        let main_line = self.data.build_main_line(self.theme, self.use_nerd_font);

        if self.data.progress.is_none() || area.height < 2 {
            // 1-liniowy layout: tylko metryki
            main_line.render(area, buf);
            return;
        }

        // 2-liniowy layout: metryki + gauge
        // Linia 1: metryki
        let line1_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
        main_line.render(line1_area, buf);

        // Linia 2: gauge
        if let Some(gauge) = self.data.build_progress_gauge(self.theme) {
            let line2_area = Rect {
                x: area.x,
                y: area.y + 1,
                width: area.width,
                height: 1,
            };
            gauge.render(line2_area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::snap;
    use crate::tui::DEFAULT_THEME;
    use ratatui::{Terminal, backend::TestBackend};

    /// Helper do renderowania status bar do bufora testowego.
    fn render_status_bar(data: StatusBarData, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("Failed to create test terminal");

        terminal
            .draw(|frame| {
                let widget = StatusBar::new(data, &DEFAULT_THEME, false);
                frame.render_widget(widget, frame.area());
            })
            .expect("Failed to draw widget");

        terminal.backend().buffer().clone()
    }

    #[test]
    fn test_snapshot_default_metrics() {
        let data = StatusBarData::default();
        let buffer = render_status_bar(data, 80, 1);
        insta::assert_snapshot!(snap(&buffer), @"[t] 0.0s │ ↓ 0 ↑ 0 │ $0.0000");
    }

    #[test]
    fn test_snapshot_active_metrics() {
        let data = StatusBarData {
            input_tokens: 15234,
            output_tokens: 8976,
            cost_usd: 0.4231,
            elapsed_secs: 42.7,
            ..Default::default()
        };
        let buffer = render_status_bar(data, 80, 1);
        insta::assert_snapshot!(snap(&buffer), @"[t] 42.7s │ ↓ 15.2k ↑ 9.0k │ $0.4231");
    }

    #[test]
    fn test_snapshot_with_hints() {
        let data = StatusBarData {
            input_tokens: 5000,
            output_tokens: 3000,
            cost_usd: 0.15,
            elapsed_secs: 12.3,
            hints: vec![
                ("q".to_string(), "Quit".to_string()),
                ("r".to_string(), "Restart".to_string()),
            ],
            ..Default::default()
        };
        let buffer = render_status_bar(data, 80, 1);
        insta::assert_snapshot!(snap(&buffer), @"[t] 12.3s │ ↓ 5.0k ↑ 3.0k │ $0.1500 │ q Quit r Restart");
    }

    #[test]
    fn test_snapshot_progress_gauge_50_percent() {
        let data = StatusBarData {
            input_tokens: 10000,
            output_tokens: 6000,
            cost_usd: 0.25,
            elapsed_secs: 25.6,
            progress: Some(ProgressData {
                done: 5,
                total: 10,
                eta_text: Some("~23m".to_string()),
            }),
            ..Default::default()
        };
        let buffer = render_status_bar(data, 80, 2);
        insta::assert_snapshot!(snap(&buffer), @"
        [t] 25.6s │ ↓ 10.0k ↑ 6.0k │ $0.2500
        █████████████████████████████5/10 (50%) | ETA ~23m
        ");
    }

    #[test]
    fn test_snapshot_progress_gauge_0_percent() {
        let data = StatusBarData {
            input_tokens: 2000,
            output_tokens: 1000,
            cost_usd: 0.05,
            elapsed_secs: 5.2,
            progress: Some(ProgressData {
                done: 0,
                total: 10,
                eta_text: None,
            }),
            ..Default::default()
        };
        let buffer = render_status_bar(data, 80, 2);
        insta::assert_snapshot!(snap(&buffer), @"
        [t] 5.2s │ ↓ 2.0k ↑ 1.0k │ $0.0500
                                           0/10 (0%)
        ");
    }

    #[test]
    fn test_snapshot_progress_gauge_100_percent() {
        let data = StatusBarData {
            input_tokens: 50000,
            output_tokens: 30000,
            cost_usd: 1.25,
            elapsed_secs: 120.5,
            progress: Some(ProgressData {
                done: 10,
                total: 10,
                eta_text: None,
            }),
            ..Default::default()
        };
        let buffer = render_status_bar(data, 80, 2);
        insta::assert_snapshot!(snap(&buffer), @"
        [t] 120.5s │ ↓ 50.0k ↑ 30.0k │ $1.2500
        ██████████████████████████████████10/10 (100%) █████████████████████████████████
        ");
    }

    #[test]
    fn test_snapshot_progress_without_eta() {
        let data = StatusBarData {
            input_tokens: 8000,
            output_tokens: 4500,
            cost_usd: 0.18,
            elapsed_secs: 18.3,
            progress: Some(ProgressData {
                done: 3,
                total: 7,
                eta_text: None,
            }),
            ..Default::default()
        };
        let buffer = render_status_bar(data, 80, 2);
        insta::assert_snapshot!(snap(&buffer), @"
        [t] 18.3s │ ↓ 8.0k ↑ 4.5k │ $0.1800
        ██████████████████████████████████ 3/7 (43%)
        ");
    }

    #[test]
    fn test_snapshot_progress_total_zero() {
        let data = StatusBarData {
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.01,
            elapsed_secs: 0.5,
            progress: Some(ProgressData {
                done: 0,
                total: 0,
                eta_text: None,
            }),
            ..Default::default()
        };
        let buffer = render_status_bar(data, 80, 2);
        insta::assert_snapshot!(snap(&buffer), @"
        [t] 0.5s │ ↓ 100 ↑ 50 │ $0.0100
                                            0/0 (0%)
        ");
    }

    #[test]
    fn test_snapshot_all_fields_combined() {
        let data = StatusBarData {
            input_tokens: 25000,
            output_tokens: 15000,
            cost_usd: 0.55,
            elapsed_secs: 65.4,
            progress: Some(ProgressData {
                done: 7,
                total: 12,
                eta_text: Some("~1h 15m".to_string()),
            }),
            hints: vec![
                ("q".to_string(), "Quit".to_string()),
                ("r".to_string(), "Restart".to_string()),
            ],
        };
        let buffer = render_status_bar(data, 100, 2);
        insta::assert_snapshot!(snap(&buffer), @"
        [t] 65.4s │ ↓ 25.0k ↑ 15.0k │ $0.5500 │ q Quit r Restart
        ██████████████████████████████████████7/12 (58%) | ETA ~1h 15m
        ");
    }

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(StatusBarData::format_tokens(0), "0");
        assert_eq!(StatusBarData::format_tokens(123), "123");
        assert_eq!(StatusBarData::format_tokens(999), "999");
    }

    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(StatusBarData::format_tokens(1_000), "1.0k");
        assert_eq!(StatusBarData::format_tokens(1_234), "1.2k");
        assert_eq!(StatusBarData::format_tokens(9_876), "9.9k");
        assert_eq!(StatusBarData::format_tokens(15_234), "15.2k");
    }

    #[test]
    fn test_format_tokens_millions() {
        assert_eq!(StatusBarData::format_tokens(1_000_000), "1.0M");
        assert_eq!(StatusBarData::format_tokens(1_234_567), "1.2M");
        assert_eq!(StatusBarData::format_tokens(9_876_543), "9.9M");
        assert_eq!(StatusBarData::format_tokens(15_234_567), "15.2M");
    }

    #[test]
    fn test_required_height_without_progress() {
        let data = StatusBarData::default();
        let widget = StatusBar::new(data, &DEFAULT_THEME, false);
        assert_eq!(widget.required_height(), 1);
    }

    #[test]
    fn test_required_height_with_progress() {
        let data = StatusBarData {
            progress: Some(ProgressData {
                done: 5,
                total: 10,
                eta_text: None,
            }),
            ..Default::default()
        };
        let widget = StatusBar::new(data, &DEFAULT_THEME, false);
        assert_eq!(widget.required_height(), 2);
    }

    #[test]
    fn test_progress_data_partial_eq() {
        let p1 = ProgressData {
            done: 5,
            total: 10,
            eta_text: Some("~23m".to_string()),
        };
        let p2 = ProgressData {
            done: 5,
            total: 10,
            eta_text: Some("~23m".to_string()),
        };
        let p3 = ProgressData {
            done: 6,
            total: 10,
            eta_text: Some("~23m".to_string()),
        };
        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }

    #[test]
    fn test_status_bar_data_partial_eq() {
        let d1 = StatusBarData {
            input_tokens: 1000,
            output_tokens: 500,
            cost_usd: 0.05,
            elapsed_secs: 10.0,
            progress: None,
            hints: vec![("q".to_string(), "Quit".to_string())],
        };
        let d2 = StatusBarData {
            input_tokens: 1000,
            output_tokens: 500,
            cost_usd: 0.05,
            elapsed_secs: 10.0,
            progress: None,
            hints: vec![("q".to_string(), "Quit".to_string())],
        };
        let d3 = StatusBarData {
            input_tokens: 2000,
            output_tokens: 500,
            cost_usd: 0.05,
            elapsed_secs: 10.0,
            progress: None,
            hints: vec![("q".to_string(), "Quit".to_string())],
        };
        assert_eq!(d1, d2);
        assert_ne!(d1, d3);
    }
}
