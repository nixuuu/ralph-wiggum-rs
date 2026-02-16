//! Shared utility functions for the orchestrator subsystem.

use std::time::Duration;

// ── Helper functions ────────────────────────────────────────────────

/// Render a Unicode progress bar.
///
/// Handles edge cases:
/// - `total == 0`: empty bar
/// - `done > total`: clamped to 100%
/// - Overflow-safe: uses `f64` division instead of integer multiplication
pub fn render_progress_bar(done: usize, total: usize, width: usize) -> String {
    if total == 0 {
        return format!("[{}]", " ".repeat(width));
    }

    // Calculate progress percentage safely without overflow:
    // Use (done as f64 / total as f64) to avoid integer multiplication overflow
    let progress = (done as f64) / (total as f64);
    // Clamp to [0.0, 1.0] to handle done > total gracefully
    let progress = progress.clamp(0.0, 1.0);

    let filled = ((width as f64) * progress) as usize;
    // Ensure filled doesn't exceed width (defensive against rounding edge cases)
    let filled = filled.min(width);
    let empty = width - filled;

    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

/// Format a duration as human-readable string.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Format token count for display (e.g., 1234 -> "1.2k").
#[must_use]
pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── render_progress_bar tests ─────────────────────────────────────────

    /// Snapshot test: progress bar 0/10 (0%)
    #[test]
    fn snapshot_render_progress_bar_0_percent() {
        let bar = render_progress_bar(0, 10, 10);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar 5/10 (50%)
    #[test]
    fn snapshot_render_progress_bar_50_percent() {
        let bar = render_progress_bar(5, 10, 10);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar 10/10 (100%)
    #[test]
    fn snapshot_render_progress_bar_100_percent() {
        let bar = render_progress_bar(10, 10, 10);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar 1/100 (1%)
    #[test]
    fn snapshot_render_progress_bar_1_percent() {
        let bar = render_progress_bar(1, 100, 10);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar 99/100 (99%)
    #[test]
    fn snapshot_render_progress_bar_99_percent() {
        let bar = render_progress_bar(99, 100, 10);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar 0/0 (edge case — dzielenie przez zero)
    #[test]
    fn snapshot_render_progress_bar_zero_total() {
        let bar = render_progress_bar(0, 0, 10);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar z szerokością 10 (30% — zaokrąglanie w dół)
    #[test]
    fn snapshot_render_progress_bar_width_10() {
        let bar = render_progress_bar(3, 10, 10);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar z szerokością 20
    #[test]
    fn snapshot_render_progress_bar_width_20() {
        let bar = render_progress_bar(10, 20, 20);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar z szerokością 40
    #[test]
    fn snapshot_render_progress_bar_width_40() {
        let bar = render_progress_bar(20, 40, 40);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar z szerokością 80
    #[test]
    fn snapshot_render_progress_bar_width_80() {
        let bar = render_progress_bar(40, 80, 80);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar done > total (15/10 — niespójny stan)
    #[test]
    fn snapshot_render_progress_bar_done_exceeds_total() {
        let bar = render_progress_bar(15, 10, 10);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar done=usize::MAX, total=usize::MAX (100% ekstremalnie duże)
    #[test]
    fn snapshot_render_progress_bar_max_max() {
        let bar = render_progress_bar(usize::MAX, usize::MAX, 10);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar done=1, total=usize::MAX (prawie 0%)
    #[test]
    fn snapshot_render_progress_bar_1_max() {
        let bar = render_progress_bar(1, usize::MAX, 10);
        insta::assert_snapshot!(bar);
    }

    /// Snapshot test: progress bar done=usize::MAX, total=1 (bardzo duża wartość done, total=1)
    #[test]
    fn snapshot_render_progress_bar_max_1() {
        let bar = render_progress_bar(usize::MAX, 1, 10);
        insta::assert_snapshot!(bar);
    }

    // ── format_duration tests ────────────────────────────────────────────

    /// Snapshot test: format_duration with 0 seconds
    #[test]
    fn snapshot_format_duration_0s() {
        let result = format_duration(Duration::from_secs(0));
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_duration with 1 second
    #[test]
    fn snapshot_format_duration_1s() {
        let result = format_duration(Duration::from_secs(1));
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_duration with 59 seconds (boundary before minutes)
    #[test]
    fn snapshot_format_duration_59s() {
        let result = format_duration(Duration::from_secs(59));
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_duration with 60 seconds (exactly 1 minute)
    #[test]
    fn snapshot_format_duration_60s() {
        let result = format_duration(Duration::from_secs(60));
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_duration with 61 seconds
    #[test]
    fn snapshot_format_duration_61s() {
        let result = format_duration(Duration::from_secs(61));
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_duration with 3599 seconds (boundary before hours)
    #[test]
    fn snapshot_format_duration_3599s() {
        let result = format_duration(Duration::from_secs(3599));
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_duration with 3600 seconds (exactly 1 hour)
    #[test]
    fn snapshot_format_duration_3600s() {
        let result = format_duration(Duration::from_secs(3600));
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_duration with 7261 seconds (2h 1m 1s)
    #[test]
    fn snapshot_format_duration_7261s() {
        let result = format_duration(Duration::from_secs(7261));
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_duration with Duration::ZERO
    #[test]
    fn snapshot_format_duration_zero() {
        let result = format_duration(Duration::ZERO);
        insta::assert_snapshot!(result);
    }

    // ── Boundary value tests with milliseconds ────────────────────────────

    /// Snapshot test: format_duration boundary value 59.9s (format check: still seconds)
    #[test]
    fn snapshot_format_duration_boundary_59_9s() {
        let result = format_duration(Duration::from_millis(59_999));
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_duration boundary value 60.0s (transition: seconds→minutes)
    #[test]
    fn snapshot_format_duration_boundary_60_0s() {
        let result = format_duration(Duration::from_millis(60_001));
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_duration boundary value 3599.9s (format check: still minutes)
    #[test]
    fn snapshot_format_duration_boundary_3599_9s() {
        let result = format_duration(Duration::from_millis(3_599_999));
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_duration boundary value 3600.0s (transition: minutes→hours)
    #[test]
    fn snapshot_format_duration_boundary_3600_0s() {
        let result = format_duration(Duration::from_millis(3_600_001));
        insta::assert_snapshot!(result);
    }

    // ── format_tokens tests ──────────────────────────────────────────────

    /// Snapshot test: format_tokens with 0 tokens
    #[test]
    fn snapshot_format_tokens_0() {
        let result = format_tokens(0);
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_tokens with 1 token
    #[test]
    fn snapshot_format_tokens_1() {
        let result = format_tokens(1);
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_tokens with 999 (boundary before 'k')
    #[test]
    fn snapshot_format_tokens_999() {
        let result = format_tokens(999);
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_tokens with 1000 (exactly 1.0k)
    #[test]
    fn snapshot_format_tokens_1000() {
        let result = format_tokens(1000);
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_tokens with 1500 (1.5k)
    #[test]
    fn snapshot_format_tokens_1500() {
        let result = format_tokens(1500);
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_tokens with 999999 (boundary before 'M')
    #[test]
    fn snapshot_format_tokens_999999() {
        let result = format_tokens(999_999);
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_tokens with 1000000 (exactly 1.0M)
    #[test]
    fn snapshot_format_tokens_1000000() {
        let result = format_tokens(1_000_000);
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_tokens with 1500000 (1.5M)
    #[test]
    fn snapshot_format_tokens_1500000() {
        let result = format_tokens(1_500_000);
        insta::assert_snapshot!(result);
    }

    /// Snapshot test: format_tokens with u64::MAX
    #[test]
    fn snapshot_format_tokens_max() {
        let result = format_tokens(u64::MAX);
        insta::assert_snapshot!(result);
    }
}
