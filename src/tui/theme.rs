use ratatui::style::{Color, Modifier, Style};

use crate::shared::progress::TaskStatus;

/// Centralna paleta kolorów dla całej aplikacji TUI.
///
/// Zbiera wszystkie kolory UI w jednym miejscu zamiast hardcoded wartości
/// rozrzuconych po plikach. Dostęp przez singleton `DEFAULT_THEME`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)] // Public API — paleta kolorów dla TUI widoków
pub struct Theme {
    /// Kolor główny (accent) — header, focused UI, branding
    pub primary: Color,

    /// Kolor sukcesu — done, zielone strzałki, pozytywne komunikaty
    pub success: Color,

    /// Kolor ostrzeżenia — warningi, ładowanie, in-progress
    pub warning: Color,

    /// Kolor błędu — błędy, stany negatywne, blocked
    pub error: Color,

    /// Kolor drugorzędny (secondary accent) — user messages, ask_user border
    pub secondary: Color,

    /// Kolor wyciszony — tekst drugorzędny, disabled, separatory
    pub muted: Color,

    /// Kolor obramowania panelu focused (zaznaczonego)
    pub border_focused: Color,

    /// Kolor obramowania panelu nie-focused
    pub border_normal: Color,

    /// Kolor tła headeru (tytuł panelu)
    pub header_bg: Color,

    /// Kolor tła status bar
    pub status_bar_bg: Color,

    /// Kolor tła sidebara (borderless, ciemny)
    pub sidebar_bg: Color,

    /// Kolor tła paneli (unfocused)
    pub panel_bg: Color,

    /// Kolor tła paneli (focused) — jaśniejsze
    pub panel_bg_focused: Color,

    /// Kolor tekstu głównego — Catppuccin Text #cdd6f4 (na ciemnym tle)
    pub text: Color,

    /// Kolor obramowania panelu hovered (kursor myszy, bez focusa)
    /// Subtelny — wyraźniejszy niż border_normal, ale ciemniejszy niż border_focused
    pub border_hover: Color,
}

/// Domyślny motyw — Catppuccin Mocha
///
/// Paleta: https://github.com/catppuccin/catppuccin#-palette
#[allow(dead_code)] // Public API — singleton theme dla TUI widoków
pub const DEFAULT_THEME: Theme = Theme {
    // Blue #89b4fa — main accent, focus, branding
    primary: Color::Rgb(137, 180, 250),
    // Green #a6e3a1 — success, done states
    success: Color::Rgb(166, 227, 161),
    // Peach #fab387 — warnings, in-progress
    warning: Color::Rgb(250, 179, 135),
    // Red #f38ba8 — errors, blocked
    error: Color::Rgb(243, 139, 168),
    // Mauve #cba6f7 — secondary accent, user messages
    secondary: Color::Rgb(203, 166, 247),
    // Overlay0 #6c7086 — muted text, separators
    muted: Color::Rgb(108, 112, 134),
    // Blue #89b4fa — focused border (same as primary)
    border_focused: Color::Rgb(137, 180, 250),
    // Surface1 #45475a — unfocused border
    border_normal: Color::Rgb(69, 71, 90),
    // Surface0 #313244 — header background
    header_bg: Color::Rgb(49, 50, 68),
    // Mantle #181825 — status bar background
    status_bar_bg: Color::Rgb(24, 24, 37),
    // Mantle #181825 — sidebar background
    sidebar_bg: Color::Rgb(24, 24, 37),
    // Base #1e1e2e — panel background (unfocused)
    panel_bg: Color::Rgb(30, 30, 46),
    // Surface0 #313244 — panel background (focused)
    panel_bg_focused: Color::Rgb(49, 50, 68),
    // Text #cdd6f4 — main text color
    text: Color::Rgb(205, 214, 244),
    // Overlay1 #7f849c — hover border (between border_normal and border_focused)
    border_hover: Color::Rgb(127, 132, 156),
};

#[allow(dead_code)] // Public API — paleta kolorów dla TUI widoków
impl Theme {
    /// Zwraca domyślny theme (alias dla DEFAULT_THEME)
    pub const fn default() -> Self {
        DEFAULT_THEME
    }

    /// Style obramowania w zależności od stanu focus.
    /// Focused: bold + border_focused, unfocused: border_normal.
    pub fn border_style(&self, focused: bool) -> Style {
        if focused {
            Style::default()
                .fg(self.border_focused)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.border_normal)
        }
    }

    /// Style obramowania z uwzględnieniem hover i focus.
    ///
    /// Priorytety: focused > hovered > normalny.
    /// - focused: bold + border_focused
    /// - hovered (nie focused): border_hover (subtelna zmiana)
    /// - normalny: border_normal
    ///
    /// Generyczna wersja dla widgetów używających `border_focused` jako kolor focusa.
    /// Uwaga: WorkerPanelWidget używa własnej logiki (border = `state_color` workera),
    /// dlatego nie deleguje do tej metody.
    pub fn border_style_hover(&self, focused: bool, hovered: bool) -> Style {
        if focused {
            Style::default()
                .fg(self.border_focused)
                .add_modifier(Modifier::BOLD)
        } else if hovered {
            Style::default().fg(self.border_hover)
        } else {
            Style::default().fg(self.border_normal)
        }
    }

    /// Style dla headeru — primary + bold
    pub fn header_style(&self) -> Style {
        Style::default()
            .fg(self.primary)
            .add_modifier(Modifier::BOLD)
    }

    /// Style dla tekstu wyciszonego (drugorzędny, disabled)
    pub fn muted_style(&self) -> Style {
        Style::default().fg(self.muted)
    }

    /// Style dla tekstu sukcesu
    pub fn success_style(&self) -> Style {
        Style::default().fg(self.success)
    }

    /// Style dla tekstu ostrzeżenia
    pub fn warning_style(&self) -> Style {
        Style::default().fg(self.warning)
    }

    /// Style dla tekstu błędu
    pub fn error_style(&self) -> Style {
        Style::default().fg(self.error)
    }

    /// Style dla tekstu primary (accent color)
    pub fn primary_style(&self) -> Style {
        Style::default().fg(self.primary)
    }

    /// Style dla tekstu secondary (user messages, ask_user)
    pub fn secondary_style(&self) -> Style {
        Style::default().fg(self.secondary)
    }

    /// Style dla tekstu głównego (na ciemnym tle)
    pub fn text_style(&self) -> Style {
        Style::default().fg(self.text)
    }

    /// Kolor tła panelu w zależności od stanu focus.
    pub fn panel_bg(&self, focused: bool) -> Color {
        if focused {
            self.panel_bg_focused
        } else {
            self.panel_bg
        }
    }

    /// Mapuje status tasku na odpowiedni kolor z palety.
    /// Używane w sidebar, status bar i podsumowaniach.
    pub fn state_color(&self, status: &TaskStatus) -> Color {
        match status {
            TaskStatus::Done => self.success,
            TaskStatus::InProgress => self.warning,
            TaskStatus::Todo => self.muted,
            TaskStatus::Blocked => self.error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_colors_catppuccin_mocha() {
        // Catppuccin Mocha palette
        assert_eq!(DEFAULT_THEME.primary, Color::Rgb(137, 180, 250)); // Blue
        assert_eq!(DEFAULT_THEME.success, Color::Rgb(166, 227, 161)); // Green
        assert_eq!(DEFAULT_THEME.warning, Color::Rgb(250, 179, 135)); // Peach
        assert_eq!(DEFAULT_THEME.error, Color::Rgb(243, 139, 168)); // Red
        assert_eq!(DEFAULT_THEME.secondary, Color::Rgb(203, 166, 247)); // Mauve
        assert_eq!(DEFAULT_THEME.muted, Color::Rgb(108, 112, 134)); // Overlay0
        assert_eq!(DEFAULT_THEME.border_focused, Color::Rgb(137, 180, 250)); // Blue
        assert_eq!(DEFAULT_THEME.border_normal, Color::Rgb(69, 71, 90)); // Surface1
        assert_eq!(DEFAULT_THEME.header_bg, Color::Rgb(49, 50, 68)); // Surface0
        assert_eq!(DEFAULT_THEME.status_bar_bg, Color::Rgb(24, 24, 37)); // Mantle
        assert_eq!(DEFAULT_THEME.sidebar_bg, Color::Rgb(24, 24, 37)); // Mantle
        assert_eq!(DEFAULT_THEME.panel_bg, Color::Rgb(30, 30, 46)); // Base
        assert_eq!(DEFAULT_THEME.panel_bg_focused, Color::Rgb(49, 50, 68)); // Surface0
        assert_eq!(DEFAULT_THEME.text, Color::Rgb(205, 214, 244)); // Text
    }

    #[test]
    fn border_style_focused_is_bold_with_focused_color() {
        let style = DEFAULT_THEME.border_style(true);
        assert_eq!(style.fg, Some(Color::Rgb(137, 180, 250))); // Blue
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn border_style_unfocused_has_normal_color_no_bold() {
        let style = DEFAULT_THEME.border_style(false);
        assert_eq!(style.fg, Some(Color::Rgb(69, 71, 90))); // Surface1
        assert!(!style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn header_style_is_primary_bold() {
        let style = DEFAULT_THEME.header_style();
        assert_eq!(style.fg, Some(Color::Rgb(137, 180, 250))); // Blue
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn style_methods_return_correct_colors() {
        assert_eq!(
            DEFAULT_THEME.success_style().fg,
            Some(Color::Rgb(166, 227, 161))
        ); // Green
        assert_eq!(
            DEFAULT_THEME.warning_style().fg,
            Some(Color::Rgb(250, 179, 135))
        ); // Peach
        assert_eq!(
            DEFAULT_THEME.error_style().fg,
            Some(Color::Rgb(243, 139, 168))
        ); // Red
        assert_eq!(
            DEFAULT_THEME.muted_style().fg,
            Some(Color::Rgb(108, 112, 134))
        ); // Overlay0
    }

    #[test]
    fn state_color_maps_all_task_statuses() {
        assert_eq!(
            DEFAULT_THEME.state_color(&TaskStatus::Done),
            Color::Rgb(166, 227, 161)
        );
        assert_eq!(
            DEFAULT_THEME.state_color(&TaskStatus::InProgress),
            Color::Rgb(250, 179, 135)
        );
        assert_eq!(
            DEFAULT_THEME.state_color(&TaskStatus::Todo),
            Color::Rgb(108, 112, 134)
        );
        assert_eq!(
            DEFAULT_THEME.state_color(&TaskStatus::Blocked),
            Color::Rgb(243, 139, 168)
        );
    }

    #[test]
    fn theme_is_copy() {
        let theme1 = DEFAULT_THEME;
        let theme2 = theme1;
        assert_eq!(theme1, theme2);
    }

    #[test]
    fn theme_partial_eq_detects_differences() {
        let mut custom = DEFAULT_THEME;
        custom.primary = Color::Magenta;
        assert_ne!(custom, DEFAULT_THEME);
    }

    #[test]
    fn border_hover_is_overlay1_catppuccin_mocha() {
        assert_eq!(DEFAULT_THEME.border_hover, Color::Rgb(127, 132, 156)); // Overlay1
    }

    #[test]
    fn border_style_hover_focused_dominates_over_hover() {
        // focused=true, hovered=true → focused style (BOLD + border_focused)
        let style = DEFAULT_THEME.border_style_hover(true, true);
        assert_eq!(style.fg, Some(Color::Rgb(137, 180, 250)));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn border_style_hover_hovered_only_uses_hover_color() {
        // focused=false, hovered=true → hover color (no BOLD)
        let style = DEFAULT_THEME.border_style_hover(false, true);
        assert_eq!(style.fg, Some(Color::Rgb(127, 132, 156))); // Overlay1
        assert!(!style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn border_style_hover_neither_uses_normal_color() {
        // focused=false, hovered=false → normal color
        let style = DEFAULT_THEME.border_style_hover(false, false);
        assert_eq!(style.fg, Some(Color::Rgb(69, 71, 90))); // Surface1
        assert!(!style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn border_style_hover_focused_only_is_bold() {
        // focused=true, hovered=false → focused style
        let style = DEFAULT_THEME.border_style_hover(true, false);
        assert_eq!(style.fg, Some(Color::Rgb(137, 180, 250)));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }
}
