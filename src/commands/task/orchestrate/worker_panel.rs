/// Reusable worker panel widget for orchestrator dashboard.
///
/// Displays worker status, task info, model, cost, tokens, and output buffer.
/// Supports focus state, grace period dimming, and verify phase with profiles.
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap};

use super::shared_types::format_tokens;
use super::worker_status::WorkerState;
use crate::commands::task::orchestrate::app::WorkerPanel;
use crate::shared::tasks::shorten_model_name;
use crate::tui::theme::{DEFAULT_THEME, Theme};

// ── WorkerPanelWidget ────────────────────────────────────────────────

/// Stateful widget for rendering a worker panel.
///
/// Displays:
/// - Title: worker ID, task ID, component, model alias, focus marker
/// - Border: color/style based on state, focus, grace period
/// - Content: phase icon + name, verify profiles (if verify phase), output tail
/// - Footer: cost USD, input/output tokens
pub struct WorkerPanelWidget<'a> {
    /// Worker panel state (status, output, scroll offset, idle timestamp)
    pub panel: &'a WorkerPanel,
    /// Is this panel focused?
    pub is_focused: bool,
    /// Theme for colors
    pub theme: &'a Theme,
}

impl<'a> WorkerPanelWidget<'a> {
    /// Create a new WorkerPanelWidget with the default theme.
    pub fn new(panel: &'a WorkerPanel, is_focused: bool) -> Self {
        Self {
            panel,
            is_focused,
            theme: &DEFAULT_THEME,
        }
    }

    /// Create a new WorkerPanelWidget with a custom theme.
    #[allow(dead_code)] // Will be used when custom themes are needed
    pub fn with_theme(panel: &'a WorkerPanel, is_focused: bool, theme: &'a Theme) -> Self {
        Self {
            panel,
            is_focused,
            theme,
        }
    }

    /// Build the title line for the panel.
    fn build_title_line(&self) -> Line<'static> {
        let ws = &self.panel.status;
        let task_str = ws.task_id.as_deref().unwrap_or("---");
        let comp_str = ws.component.as_deref().unwrap_or("");
        let focus_marker = if self.is_focused { "▶ " } else { "" };

        // Build base title without model
        let base_title = if comp_str.is_empty() {
            format!(" {focus_marker}W{} [{}] ", self.panel.worker_id, task_str)
        } else {
            format!(
                " {focus_marker}W{} [{}: {}] ",
                self.panel.worker_id, task_str, comp_str
            )
        };

        let title_style = if self.is_focused {
            Style::default()
                .fg(self.theme.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        };

        // Append model suffix if present
        if let Some(model) = &ws.model {
            let alias = shorten_model_name(model);
            let model_color = if ws.state == WorkerState::ResolvingConflicts {
                self.theme.warning
            } else {
                self.theme.muted
            };

            Line::from(vec![
                Span::styled(base_title.trim_end().to_string(), title_style),
                Span::styled(format!(" ({})", alias), Style::default().fg(model_color)),
                Span::raw(" "),
            ])
        } else {
            Line::from(vec![Span::styled(base_title, title_style)])
        }
    }

    /// Build the footer line showing cost and tokens.
    fn build_footer_line(&self) -> String {
        let ws = &self.panel.status;
        format!(
            " ${:.4} │ ↓{} ↑{} ",
            ws.cost_usd.max(0.0),
            format_tokens(ws.input_tokens),
            format_tokens(ws.output_tokens)
        )
    }

    /// Build the panel background style.
    fn build_panel_style(&self) -> Style {
        Style::default().bg(self.theme.panel_bg)
    }

    /// Build the left border style based on worker state and focus.
    fn build_border_style(&self) -> Style {
        let state_color = self.panel.status.state.color();
        let mut style = Style::default().fg(state_color);
        if self.is_focused {
            style = style.add_modifier(Modifier::BOLD);
        }
        style
    }

    /// Build the content lines for the panel (status + output).
    fn build_panel_content(&self, area: Rect) -> Vec<Line<'a>> {
        let ws = &self.panel.status;

        // Inner area height (minus 2 for borders)
        let inner_height = area.height.saturating_sub(2) as usize;
        if inner_height == 0 {
            return Vec::new();
        }

        // First line: phase icon + name + profiles (if verify phase)
        let (icon, icon_color) = ws.state.icon();
        let phase_str = ws
            .phase
            .as_ref()
            .map(|p: &super::events::WorkerPhase| p.to_string())
            .unwrap_or_else(|| ws.state.to_string());

        let mut status_spans = vec![
            Span::styled(icon, Style::default().fg(icon_color)),
            Span::raw(" "),
            Span::styled(phase_str, Style::default().fg(icon_color)),
        ];

        // Add profile info for Verify phase
        if ws.state == WorkerState::Verifying && !ws.verify_profiles.is_empty() {
            let profile_info: Vec<String> = ws
                .verify_profiles
                .iter()
                .map(|(name, success)| {
                    let icon = match success {
                        Some(true) => "✓",
                        Some(false) => "✗",
                        None => "⏳",
                    };
                    format!("{} {}", name, icon)
                })
                .collect();
            let profiles_str = format!(" [{}]", profile_info.join(", "));
            status_spans.push(Span::styled(
                profiles_str,
                Style::default().fg(self.theme.muted),
            ));
        }

        let status_line = Line::from(status_spans);

        // Remaining lines: output tail (visual-wrap aware)
        let output_height = inner_height.saturating_sub(1);
        let inner_width = area.width.saturating_sub(2); // subtract borders
        let mut lines = vec![status_line];

        if output_height > 0 {
            let tail = if self.panel.scroll_offset == 0 {
                // Auto-scroll: show last N lines fitting visual rows
                self.panel.output.tail_visual(output_height, inner_width)
            } else {
                // Manual scroll: offset counted in visual rows from bottom
                // Clamp offset to valid range: max = total_visual_rows - output_height
                let total_visual = self.panel.output.total_visual_rows(inner_width);
                let max_offset = total_visual.saturating_sub(output_height);
                let clamped_offset = self.panel.scroll_offset.min(max_offset);

                self.panel
                    .output
                    .slice_visual(clamped_offset, output_height, inner_width)
            };
            lines.extend(tail);
        }

        lines
    }
}

impl<'a> Widget for WorkerPanelWidget<'a> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        // Build title line
        let title_line = self.build_title_line();

        // Build footer line
        let footer = self.build_footer_line();

        // Build panel style
        let panel_style = self.build_panel_style();

        // Build border style based on worker state + focus
        let border_style = self.build_border_style();

        // Construct block with left border, title and footer
        let block = Block::default()
            .borders(Borders::LEFT | Borders::TOP)
            .border_style(border_style)
            .padding(Padding::new(1, 1, 0, 1))
            .style(panel_style)
            .title(title_line)
            .title_bottom(Span::styled(footer, Style::default().fg(self.theme.muted)));

        // Build content lines
        let lines = self.build_panel_content(area);

        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::super::events::WorkerPhase;
    use super::super::worker_status::WorkerStatus;
    use super::*;
    use crate::test_helpers::snap;
    use crate::tui::ring_buffer::OutputRingBuffer;
    use ratatui::{Terminal, backend::TestBackend};

    /// Helper: builds a WorkerPanel and renders it via WorkerPanelWidget + snap.
    /// Delegates to `render_panel_with_output` with empty output.
    fn render_panel_snapshot(
        status: WorkerStatus,
        worker_id: u32,
        is_focused: bool,
        idle_since: Option<std::time::Instant>,
        width: u16,
        height: u16,
    ) -> String {
        render_panel_with_output(
            status,
            worker_id,
            is_focused,
            idle_since,
            &[],
            width,
            height,
        )
    }

    /// Helper: builds a WorkerPanel with output lines and renders it.
    fn render_panel_with_output(
        status: WorkerStatus,
        worker_id: u32,
        is_focused: bool,
        idle_since: Option<std::time::Instant>,
        output_lines: &[&str],
        width: u16,
        height: u16,
    ) -> String {
        let mut output = OutputRingBuffer::with_capacity(100);
        for line in output_lines {
            output.push_str(line);
        }

        let panel = WorkerPanel {
            worker_id,
            status,
            output,
            scroll_offset: 0,
            idle_since,
        };

        let area = Rect::new(0, 0, width, height);
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let widget = WorkerPanelWidget::new(&panel, is_focused);
                frame.render_widget(widget, area);
            })
            .expect("draw");

        snap(terminal.backend().buffer())
    }

    /// Helper: creates a default WorkerStatus with common fields.
    fn make_status(state: WorkerState) -> WorkerStatus {
        WorkerStatus {
            state,
            phase: None,
            task_id: None,
            component: None,
            model: None,
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            verify_profiles: Vec::new(),
        }
    }

    // ── 1. Border type: focused (Double) vs unfocused (Rounded) ──────

    #[test]
    fn test_snapshot_panel_focused_border() {
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("2.1".into());
        status.component = Some("api".into());
        status.cost_usd = 0.042;
        status.input_tokens = 1500;
        status.output_tokens = 2300;

        let snapshot = render_panel_snapshot(status, 1, true, None, 50, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_panel_unfocused_border() {
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("2.1".into());
        status.component = Some("api".into());
        status.cost_usd = 0.042;
        status.input_tokens = 1500;
        status.output_tokens = 2300;

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 6);
        insta::assert_snapshot!(snapshot);
    }

    // ── 2–3. Title line variants ─────────────────────────────────────

    #[test]
    fn test_snapshot_title_with_component_and_model() {
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("3.2".into());
        status.component = Some("frontend".into());
        status.model = Some("claude-sonnet-4-5-20250929".into());
        status.cost_usd = 0.015;
        status.input_tokens = 800;
        status.output_tokens = 400;

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_title_no_component_no_model() {
        let mut status = make_status(WorkerState::Idle);
        status.task_id = None;
        status.component = None;
        status.model = None;

        let snapshot = render_panel_snapshot(status, 3, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── 4. Footer line ───────────────────────────────────────────────

    #[test]
    fn test_snapshot_footer_cost_tokens() {
        let mut status = make_status(WorkerState::Reviewing);
        status.task_id = Some("1.1".into());
        status.cost_usd = 0.042;
        status.input_tokens = 1500;
        status.output_tokens = 2300;

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── 5–8. Panel content with different WorkerStates ───────────────

    #[test]
    fn test_snapshot_state_implementing() {
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("5.1".into());
        status.phase = Some(WorkerPhase::Implement);

        let snapshot = render_panel_snapshot(status, 2, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_state_resolving_conflicts() {
        let mut status = make_status(WorkerState::ResolvingConflicts);
        status.task_id = Some("6.1".into());
        status.model = Some("claude-opus-4-6".into());

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_state_idle() {
        let status = make_status(WorkerState::Idle);

        let snapshot = render_panel_snapshot(status, 4, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_state_verifying() {
        let mut status = make_status(WorkerState::Verifying);
        status.task_id = Some("7.1".into());
        status.phase = Some(WorkerPhase::Verify);

        let snapshot = render_panel_snapshot(status, 3, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── 9. Grace period dimming ──────────────────────────────────────

    #[test]
    fn test_snapshot_grace_period_dimming() {
        let status = make_status(WorkerState::Idle);
        // idle_since = Some(Instant::now()) simulates grace period
        let idle_since = Some(std::time::Instant::now());

        let snapshot = render_panel_snapshot(status, 2, false, idle_since, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── 10. Profile status rows (verify phase) ──────────────────────

    #[test]
    fn test_snapshot_verify_profiles() {
        let mut status = make_status(WorkerState::Verifying);
        status.task_id = Some("8.1".into());
        status.phase = Some(WorkerPhase::Verify);
        status.verify_profiles = vec![
            ("lint".into(), Some(true)),
            ("test".into(), Some(false)),
            ("build".into(), None),
        ];

        let snapshot = render_panel_snapshot(status, 1, false, None, 60, 6);
        insta::assert_snapshot!(snapshot);
    }

    // ── 11. Panel with output buffer tail ────────────────────────────

    #[test]
    fn test_snapshot_panel_with_output_lines() {
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("9.1".into());
        status.component = Some("core".into());
        status.phase = Some(WorkerPhase::Implement);

        let output_lines = &[
            "Compiling ralph-wiggum v0.1.0",
            "  Running `target/debug/ralph`",
            "warning: unused variable `x`",
            "  --> src/main.rs:42:9",
            "Build completed successfully",
        ];

        let snapshot = render_panel_with_output(status, 1, false, None, output_lines, 55, 10);
        insta::assert_snapshot!(snapshot);
    }

    // ── Missing state snapshots: SettingUp, Reviewing, Merging ─────────

    #[test]
    fn test_snapshot_state_setting_up() {
        let mut status = make_status(WorkerState::SettingUp);
        status.task_id = Some("10.1".into());
        status.phase = Some(WorkerPhase::Setup);

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_state_reviewing() {
        let mut status = make_status(WorkerState::Reviewing);
        status.task_id = Some("11.1".into());
        status.phase = Some(WorkerPhase::ReviewFix);

        let snapshot = render_panel_snapshot(status, 2, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_state_merging() {
        let mut status = make_status(WorkerState::Merging);
        status.task_id = Some("12.1".into());

        let snapshot = render_panel_snapshot(status, 3, false, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── Focused panel with model ────────────────────────────────────────

    #[test]
    fn test_snapshot_focused_with_model() {
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("4.1".into());
        status.component = Some("cli".into());
        status.model = Some("claude-sonnet-4-5-20250929".into());
        status.cost_usd = 0.025;
        status.input_tokens = 3000;
        status.output_tokens = 1200;

        let snapshot = render_panel_snapshot(status, 1, true, None, 50, 5);
        insta::assert_snapshot!(snapshot);
    }

    // ── Narrow terminal tests ────────────────────────────────────────────

    #[test]
    fn test_snapshot_narrow_width_20() {
        // Test minimal width (20) — title/footer truncation, bordering preserved
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("13.1".into());
        status.component = Some("backend".into());
        status.model = Some("claude-sonnet-4-5-20250929".into());
        status.cost_usd = 0.0123;
        status.input_tokens = 1234;
        status.output_tokens = 567;

        let output_lines = &[
            "Short line",
            "This is a much longer line that should wrap",
            "OK",
        ];

        let snapshot = render_panel_with_output(status, 1, false, None, output_lines, 20, 8);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_narrow_width_30() {
        // Test narrow but usable width (30) — better layout
        let mut status = make_status(WorkerState::Verifying);
        status.task_id = Some("14.2".into());
        status.component = Some("api".into());
        status.model = Some("claude-opus-4-6".into());
        status.cost_usd = 0.456;
        status.input_tokens = 5000;
        status.output_tokens = 3000;
        status.phase = Some(WorkerPhase::Verify);
        status.verify_profiles = vec![("lint".into(), Some(true)), ("test".into(), None)];

        let output_lines = &[
            "Running verification...",
            "Profile: lint — passed",
            "Profile: test — in progress",
        ];

        let snapshot = render_panel_with_output(status, 2, false, None, output_lines, 30, 10);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_minimal_height_3() {
        // Test minimal height (3) — border + 1 line content
        let mut status = make_status(WorkerState::Idle);
        status.task_id = None;
        status.cost_usd = 0.0;
        status.input_tokens = 0;
        status.output_tokens = 0;

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 3);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_narrow_width_20_focused() {
        // Test focused panel at minimal width
        let mut status = make_status(WorkerState::Reviewing);
        status.task_id = Some("15".into());
        status.component = Some("ui".into());
        status.cost_usd = 0.001;
        status.input_tokens = 100;
        status.output_tokens = 50;

        let snapshot = render_panel_snapshot(status, 3, true, None, 20, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_narrow_width_30_with_long_output() {
        // Test narrow panel with wrapping output lines
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("16.1".into());
        status.component = Some("core".into());

        let output_lines = &[
            "Line 1 normal",
            "This is a very long line that will definitely wrap in a 30-char wide terminal",
            "Short",
            "Another long line with many words that should cause wrapping behavior",
        ];

        let snapshot = render_panel_with_output(status, 1, false, None, output_lines, 30, 12);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_minimal_height_4() {
        // Test height=4 — border + status + 1 output line
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("17".into());
        status.phase = Some(WorkerPhase::Implement);

        let output_lines = &["First output line", "Second line (hidden)"];

        let snapshot = render_panel_with_output(status, 2, false, None, output_lines, 50, 4);
        insta::assert_snapshot!(snapshot);
    }

    // ── Unicode tests ─────────────────────────────────────────────────────

    #[test]
    fn test_snapshot_unicode_polish_task_id_component() {
        // Test Polish characters in task_id and component
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("zażółć".into());
        status.component = Some("środowisko".into());
        status.model = Some("claude-sonnet-4-5-20250929".into());
        status.cost_usd = 0.042;
        status.input_tokens = 1500;
        status.output_tokens = 2300;

        let snapshot = render_panel_snapshot(status, 1, false, None, 60, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_unicode_emoji_output() {
        // Test emoji in output lines
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("18.1".into());
        status.component = Some("core".into());

        let output_lines = &[
            "🚀 Starting build process",
            "✅ Tests passed successfully",
            "❌ Linter found issues",
            "⚠️  Warning: deprecated API",
        ];

        let snapshot = render_panel_with_output(status, 1, false, None, output_lines, 60, 10);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_unicode_cjk_output() {
        // Test CJK double-width characters in output
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("19.1".into());
        status.component = Some("i18n".into());

        let output_lines = &[
            "中文测试 Chinese test",
            "日本語テスト Japanese test",
            "한글 테스트 Korean test",
            "Mixed: 中文 and English",
        ];

        let snapshot = render_panel_with_output(status, 1, false, None, output_lines, 60, 10);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_unicode_mixed_polish_emoji_cjk() {
        // Test mixed Unicode: Polish + emoji + CJK
        let mut status = make_status(WorkerState::Verifying);
        status.task_id = Some("zażółć".into());
        status.component = Some("środowisko".into());
        status.phase = Some(WorkerPhase::Verify);

        let output_lines = &[
            "🚀 Uruchamianie testów",
            "✅ Test został zakończony pomyślnie",
            "中文: 测试通过",
            "❌ Błąd: nie znaleziono pliku",
        ];

        let snapshot = render_panel_with_output(status, 1, false, None, output_lines, 70, 10);
        insta::assert_snapshot!(snapshot);
    }

    // ── Extremely long task_id and component tests ────────────────────────

    #[test]
    fn test_snapshot_extremely_long_task_id_width_60() {
        // Test task_id with 100 characters on width=60
        // Title line should be truncated to fit panel width
        let mut status = make_status(WorkerState::Implementing);
        // 100-char task_id
        status.task_id = Some(
            "1.2.3.4.5.6.7.8.9.10.11.12.13.14.15.16.17.18.19.20.21.22.23.24.25.26.27.28.29.30.31.32.33.34.35.36.37.38.39.40".into()
        );
        status.component = Some("api".into());
        status.model = Some("claude-sonnet-4-5-20250929".into());
        status.cost_usd = 0.042;
        status.input_tokens = 1500;
        status.output_tokens = 2300;

        let snapshot = render_panel_snapshot(status, 1, false, None, 60, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_extremely_long_component_width_40() {
        // Test component with 50 characters on width=40
        // Title line should be truncated to fit panel width
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("2.1".into());
        // 50-char component
        status.component = Some("authentication-and-authorization-backend-service".into());
        status.model = Some("claude-opus-4-6".into());
        status.cost_usd = 0.123;
        status.input_tokens = 2000;
        status.output_tokens = 1000;

        let snapshot = render_panel_snapshot(status, 1, false, None, 40, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_extremely_high_cost_footer() {
        // Test footer with extremely high cost ($999999.99)
        // Footer should display large cost without panic
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("3.1".into());
        status.component = Some("core".into());
        status.cost_usd = 999999.99;
        status.input_tokens = 999_999_999;
        status.output_tokens = 999_999_999;

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_extremely_long_both_task_id_and_component() {
        // Test both task_id and component extremely long
        // Should verify truncation behavior with both long values
        let mut status = make_status(WorkerState::Reviewing);
        // 100-char task_id
        status.task_id = Some(
            "feature.authentication.oauth2.implementation.backend.api.endpoints.v2.user.profile.settings.update.handler".into()
        );
        // 50-char component
        status.component = Some("authentication-backend-microservice-orchestrator".into());
        status.model = Some("claude-sonnet-4-5-20250929".into());
        status.cost_usd = 12.3456;
        status.input_tokens = 50_000;
        status.output_tokens = 25_000;

        let snapshot = render_panel_snapshot(status, 1, false, None, 60, 6);
        insta::assert_snapshot!(snapshot);
    }

    // ── Empty/minimal data tests ──────────────────────────────────────────

    #[test]
    fn test_snapshot_empty_worker_with_task_id_no_component_no_model() {
        // Test worker with task_id but without component and model
        // Should render task_id without component or model suffix
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("42.1".into());
        status.component = None;
        status.model = None;
        status.cost_usd = 0.0;
        status.input_tokens = 0;
        status.output_tokens = 0;

        let snapshot = render_panel_snapshot(status, 2, false, None, 50, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_empty_worker_zero_cost_zero_tokens() {
        // Test worker with cost=0.0 and tokens=0/0
        // Footer should display $0.0000 and ↓0 ↑0 without errors
        let mut status = make_status(WorkerState::Idle);
        status.task_id = Some("1.1".into());
        status.component = Some("test".into());

        let snapshot = render_panel_snapshot(status, 3, false, None, 50, 6);
        insta::assert_snapshot!(snapshot);
    }

    #[test]
    fn test_snapshot_empty_worker_empty_output_buffer() {
        // Test worker with empty output buffer
        // Should render only status line, no output lines
        let mut status = make_status(WorkerState::Implementing);
        status.task_id = Some("5.2".into());
        status.component = Some("core".into());
        status.phase = Some(WorkerPhase::Implement);

        let snapshot = render_panel_snapshot(status, 1, false, None, 50, 8);
        insta::assert_snapshot!(snapshot);
    }
}
