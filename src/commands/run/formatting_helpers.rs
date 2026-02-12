/// Format seconds into a short human-readable duration string.
/// Examples: "~45s", "~3m", "~12m", "~1h05m", "~2h30m"
pub(super) fn format_duration_short(total_secs: u64) -> String {
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

/// Format tokens for display (e.g., 1234 -> "1.2k")
#[must_use]
pub(super) fn format_tokens(tokens: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_short_seconds() {
        assert_eq!(format_duration_short(0), "~0s");
        assert_eq!(format_duration_short(45), "~45s");
        assert_eq!(format_duration_short(59), "~59s");
    }

    #[test]
    fn test_format_duration_short_minutes() {
        assert_eq!(format_duration_short(60), "~1m");
        assert_eq!(format_duration_short(90), "~1m");
        assert_eq!(format_duration_short(720), "~12m");
        assert_eq!(format_duration_short(3599), "~59m");
    }

    #[test]
    fn test_format_duration_short_hours() {
        assert_eq!(format_duration_short(3600), "~1h");
        assert_eq!(format_duration_short(3900), "~1h05m");
        assert_eq!(format_duration_short(9000), "~2h30m");
        assert_eq!(format_duration_short(7200), "~2h");
    }

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(42), "42");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(format_tokens(1_000), "1.0k");
        assert_eq!(format_tokens(1_234), "1.2k");
        assert_eq!(format_tokens(9_876), "9.9k");
        assert_eq!(format_tokens(10_000), "10k");
        assert_eq!(format_tokens(123_456), "123k");
        assert_eq!(format_tokens(999_999), "1000k");
    }

    #[test]
    fn test_format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(3_456_789), "3.5M");
        assert_eq!(format_tokens(10_000_000), "10.0M");
    }
}
