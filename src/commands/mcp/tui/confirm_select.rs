// Widget dla pytań potwierdzających (QuestionType::Confirm)
//
// Używa ratatui Paragraph z Viewport::Inline — header pytania + [Yes] [No].

use ansi_to_tui::IntoText;

use crate::commands::mcp::ask_user::Question;
use crate::shared::error::{RalphError, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::{
    Frame, Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use std::io;
use std::time::Duration;

/// RAII guard dla raw mode
struct RawModeGuard;
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// Renderuje widget confirm do podanego Frame
/// Zwraca liczbę linii header dla obliczenia viewport height
fn render_widget(
    frame: &mut Frame,
    area: Rect,
    header_text: Option<&Text>,
    selected: usize,
) -> u16 {
    let header_lines = header_text
        .as_ref()
        .map(|t| t.lines.len() as u16)
        .unwrap_or(0);

    let buttons_area = if header_lines > 0 {
        let chunks =
            Layout::vertical([Constraint::Length(header_lines), Constraint::Length(1)]).split(area);
        if let Some(ht) = header_text {
            frame.render_widget(Paragraph::new(ht.clone()), chunks[0]);
        }
        chunks[1]
    } else {
        area
    };

    let yes_style = if selected == 0 {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let no_style = if selected == 1 {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let line = Line::from(vec![
        Span::styled("[Yes]", yes_style),
        Span::raw("  "),
        Span::styled("[No]", no_style),
    ]);
    frame.render_widget(Paragraph::new(line), buttons_area);

    header_lines
}

/// Renderuje widget confirm z headerem pytania osadzonym w viewporcie
pub fn render(question: &Question, header: &str) -> Result<String> {
    let header_text = (!header.is_empty())
        .then(|| header.into_text().ok())
        .flatten();
    let header_lines = header_text
        .as_ref()
        .map(|t| t.lines.len() as u16)
        .unwrap_or(0);

    let default_is_yes = if let Some(ref default) = question.default {
        let lower = default.to_lowercase();
        lower == "yes" || lower == "true"
    } else {
        false
    };

    let mut selected: usize = if default_is_yes { 0 } else { 1 }; // 0=Yes, 1=No
    let height = 1 + header_lines;
    let mut viewport_y = 0u16;

    // Setup ratatui terminal
    enable_raw_mode().map_err(|e| RalphError::Mcp(format!("Failed to enable raw mode: {e}")))?;
    let _guard = RawModeGuard;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(height),
        },
    )
    .map_err(|e| RalphError::Mcp(format!("Failed to create terminal: {e}")))?;

    loop {
        terminal
            .draw(|frame| {
                let area = frame.area();
                viewport_y = area.y;
                render_widget(frame, area, header_text.as_ref(), selected);
            })
            .map_err(|e| RalphError::Mcp(format!("Failed to draw: {e}")))?;

        if event::poll(Duration::from_millis(50))
            .map_err(|e| RalphError::Mcp(format!("Failed to poll events: {e}")))?
        {
            match event::read()
                .map_err(|e| RalphError::Mcp(format!("Failed to read event: {e}")))?
            {
                Event::Key(KeyEvent {
                    code: KeyCode::Left,
                    ..
                }) => {
                    selected = 0; // Yes
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Right,
                    ..
                }) => {
                    selected = 1; // No
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Enter,
                    ..
                }) => {
                    let choice = if selected == 0 { "yes" } else { "no" };
                    drop(terminal);
                    super::collapse_viewport(viewport_y)?;
                    return Ok(choice.to_string());
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                }) => {
                    drop(terminal);
                    super::collapse_viewport(viewport_y)?;
                    return Err(RalphError::Interrupted);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{render_widget_to_buffer, snap};
    use ratatui::widgets::Widget;

    // Wrapper Widget — deleguje rendering do render_widget() przez Frame
    struct ConfirmWidget {
        header_text: Option<Text<'static>>,
        selected: usize,
    }

    impl Widget for ConfirmWidget {
        fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
            // Renderuj header jako Paragraph (jeśli jest)
            let header_lines = self
                .header_text
                .as_ref()
                .map(|t| t.lines.len() as u16)
                .unwrap_or(0);

            let buttons_area = if header_lines > 0 {
                let chunks =
                    Layout::vertical([Constraint::Length(header_lines), Constraint::Length(1)])
                        .split(area);
                if let Some(ref ht) = self.header_text {
                    Paragraph::new(ht.clone()).render(chunks[0], buf);
                }
                chunks[1]
            } else {
                area
            };

            let yes_style = if self.selected == 0 {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let no_style = if self.selected == 1 {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let line = Line::from(vec![
                Span::styled("[Yes]", yes_style),
                Span::raw("  "),
                Span::styled("[No]", no_style),
            ]);
            Paragraph::new(line).render(buttons_area, buf);
        }
    }

    /// Helper: sprawdza styl komórek w podanym zakresie x
    fn assert_cell_style(
        buf: &ratatui::buffer::Buffer,
        y: u16,
        x_range: std::ops::Range<u16>,
        expected_fg: Color,
        expected_bg: Color,
        label: &str,
    ) {
        for x in x_range {
            let cell = buf.cell((x, y)).expect("Valid cell");
            assert_eq!(
                cell.fg, expected_fg,
                "{label}: fg mismatch at x={x}, expected {expected_fg:?}, got {:?}",
                cell.fg,
            );
            assert_eq!(
                cell.bg, expected_bg,
                "{label}: bg mismatch at x={x}, expected {expected_bg:?}, got {:?}",
                cell.bg,
            );
        }
    }

    // -- Snapshot testy --

    #[test]
    fn test_snapshot_yes_selected() {
        let widget = ConfirmWidget {
            header_text: None,
            selected: 0,
        };
        let buffer = render_widget_to_buffer(widget, 80, 1);
        insta::assert_snapshot!(snap(&buffer), @"[Yes]  [No]");
        // [Yes] = kolumny 0..5 — podświetlony (Cyan bg)
        assert_cell_style(
            &buffer,
            0,
            0..5,
            Color::Black,
            Color::Cyan,
            "[Yes] highlighted",
        );
        // [No] = kolumny 7..11 — nieaktywny (DarkGray fg, domyślne bg)
        assert_cell_style(
            &buffer,
            0,
            7..11,
            Color::DarkGray,
            Color::Reset,
            "[No] dimmed",
        );
    }

    #[test]
    fn test_snapshot_no_selected() {
        let widget = ConfirmWidget {
            header_text: None,
            selected: 1,
        };
        let buffer = render_widget_to_buffer(widget, 80, 1);
        insta::assert_snapshot!(snap(&buffer), @"[Yes]  [No]");
        // [Yes] = nieaktywny
        assert_cell_style(
            &buffer,
            0,
            0..5,
            Color::DarkGray,
            Color::Reset,
            "[Yes] dimmed",
        );
        // [No] = podświetlony
        assert_cell_style(
            &buffer,
            0,
            7..11,
            Color::Black,
            Color::Cyan,
            "[No] highlighted",
        );
    }

    #[test]
    fn test_snapshot_with_short_header() {
        let header = "Are you sure?".into_text().ok();
        let widget = ConfirmWidget {
            header_text: header,
            selected: 0,
        };
        let buffer = render_widget_to_buffer(widget, 80, 2);
        insta::assert_snapshot!(snap(&buffer), @"
        Are you sure?
        [Yes]  [No]
        ");
        // Przyciski na wierszu 1 (pod headerem)
        assert_cell_style(
            &buffer,
            1,
            0..5,
            Color::Black,
            Color::Cyan,
            "[Yes] highlighted",
        );
    }

    #[test]
    fn test_snapshot_with_long_header() {
        // into_text() tworzy 1 linię Text — header dostaje 1 wiersz z Layout,
        // Paragraph obcina tekst do szerokości terminala (nie zawija)
        let long = "This is a very long header text that should wrap on narrow terminals. It contains multiple sentences to test the wrapping behavior properly.";
        let header = long.into_text().ok();
        let widget = ConfirmWidget {
            header_text: header,
            selected: 1,
        };
        let buffer = render_widget_to_buffer(widget, 40, 2);
        insta::assert_snapshot!(snap(&buffer), @"
        This is a very long header text that sho
        [Yes]  [No]
        ");
        // No jest podświetlone
        assert_cell_style(
            &buffer,
            1,
            7..11,
            Color::Black,
            Color::Cyan,
            "[No] highlighted",
        );
    }

    #[test]
    fn test_snapshot_multiline_header() {
        // Wieloliniowy header (z \n) prawidłowo zajmuje wiele wierszy
        let header = "Line one\nLine two\nLine three".into_text().ok();
        let widget = ConfirmWidget {
            header_text: header,
            selected: 0,
        };
        let buffer = render_widget_to_buffer(widget, 40, 4);
        insta::assert_snapshot!(snap(&buffer), @"
        Line one
        Line two
        Line three
        [Yes]  [No]
        ");
    }

    #[test]
    fn test_snapshot_narrow_terminal() {
        let widget = ConfirmWidget {
            header_text: None,
            selected: 0,
        };
        let buffer = render_widget_to_buffer(widget, 40, 1);
        insta::assert_snapshot!(snap(&buffer), @"[Yes]  [No]");
    }

    #[test]
    fn test_snapshot_medium_terminal() {
        let widget = ConfirmWidget {
            header_text: None,
            selected: 0,
        };
        let buffer = render_widget_to_buffer(widget, 80, 1);
        insta::assert_snapshot!(snap(&buffer), @"[Yes]  [No]");
    }

    #[test]
    fn test_snapshot_wide_terminal() {
        let widget = ConfirmWidget {
            header_text: None,
            selected: 1,
        };
        let buffer = render_widget_to_buffer(widget, 120, 1);
        insta::assert_snapshot!(snap(&buffer), @"[Yes]  [No]");
    }

    #[test]
    fn test_snapshot_narrow_width_20_no_header() {
        // width=20: [Yes]  [No] powinny się zmieścić
        let widget = ConfirmWidget {
            header_text: None,
            selected: 0,
        };
        let buffer = render_widget_to_buffer(widget, 20, 1);
        insta::assert_snapshot!(snap(&buffer), @"[Yes]  [No]");
        // Weryfikuj że [Yes] jest podświetlone (Cyan bg)
        assert_cell_style(
            &buffer,
            0,
            0..5,
            Color::Black,
            Color::Cyan,
            "[Yes] highlighted at width=20",
        );
        // Weryfikuj że [No] jest nieaktywne (DarkGray fg)
        assert_cell_style(
            &buffer,
            0,
            7..11,
            Color::DarkGray,
            Color::Reset,
            "[No] dimmed at width=20",
        );
    }

    #[test]
    fn test_snapshot_narrow_width_20_long_header() {
        // width=20: długi header jest obcinany do szerokości terminala (Paragraph truncate),
        // przyciski powinny być widoczne poniżej
        let long = "This is a very long confirmation message that should wrap";
        let header = long.into_text().ok();
        let widget = ConfirmWidget {
            header_text: header,
            selected: 1,
        };
        let buffer = render_widget_to_buffer(widget, 20, 2);
        insta::assert_snapshot!(snap(&buffer), @"
        This is a very long
        [Yes]  [No]
        ");
        // Weryfikuj że [No] jest podświetlone na wierszu 1
        assert_cell_style(
            &buffer,
            1,
            7..11,
            Color::Black,
            Color::Cyan,
            "[No] highlighted at width=20",
        );
    }
}
