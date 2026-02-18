//! Plain-text session summary printer (after TUI exit).
//!
//! Converts OutputFormatter stats to styled console output using crossterm's Stylize.
//! Called after LeaveAlternateScreen to print final session statistics.

use super::output::OutputFormatter;
use crossterm::style::Stylize;

/// Final session result type
#[derive(Debug, Clone, Copy)]
pub enum SessionResult {
    /// Promise found and accepted
    PromiseFound,
    /// Max iterations reached
    MaxIterations,
    /// User interrupted (Ctrl+C)
    Interrupted,
}

impl SessionResult {
    fn as_str(&self) -> &'static str {
        match self {
            SessionResult::PromiseFound => "COMPLETED",
            SessionResult::MaxIterations => "MAX ITERATIONS",
            SessionResult::Interrupted => "INTERRUPTED",
        }
    }

    fn is_success(&self) -> bool {
        matches!(self, SessionResult::PromiseFound)
    }
}

/// Print final session summary to stdout after exiting TUI.
///
/// Format:
/// ```
/// === Session Summary ===
/// Status: COMPLETED / MAX ITERATIONS / INTERRUPTED
/// Iterations: N
/// Elapsed: XXs
/// Tokens: ...in / ...out
/// Cost: $X.XXXX
/// Speed: N.N/h
/// ```
pub fn print_final_summary(
    formatter: &OutputFormatter,
    iterations: u32,
    result: SessionResult,
    promise: &str,
) {
    println!();

    // Header
    let status_text = result.as_str();
    let status_formatted = if result.is_success() {
        format!("{}", status_text.green().bold())
    } else if matches!(result, SessionResult::Interrupted) {
        format!("{}", status_text.yellow().bold())
    } else {
        format!("{}", status_text.red())
    };

    println!("{}", "=== Session Summary ===".bold());
    println!("Status:     {}", status_formatted);

    // Status details
    if result.is_success() {
        println!("Promise:    {}", promise.cyan());
    }

    // Basic metrics
    let elapsed = formatter.get_elapsed_secs();
    println!("Iterations: {}", iterations.to_string().cyan());
    println!("Elapsed:    {}", format!("{:.2}s", elapsed).cyan());

    // Tokens
    let input = formatter.get_total_input_tokens();
    let output = formatter.get_total_output_tokens();
    if input > 0 || output > 0 {
        println!(
            "Tokens:     {} {} / {} {}",
            format_tokens(input).green(),
            "in".dark_grey(),
            format_tokens(output).magenta(),
            "out".dark_grey()
        );
    }

    // Cost
    let cost = formatter.get_total_cost();
    if cost > 0.0 {
        println!("Cost:       {}", format!("${:.4}", cost).yellow());
    }

    // Speed and ETA
    if let Some(speed_text) = formatter.get_speed_text() {
        println!("Speed:      {}", speed_text.green());
    }

    if let Some(eta_text) = formatter.get_eta_text() {
        println!("ETA:        {}", eta_text.cyan());
    }

    println!();
}

/// Helper: format tokens with K/M suffix
fn format_tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_result_as_str() {
        assert_eq!(SessionResult::PromiseFound.as_str(), "COMPLETED");
        assert_eq!(SessionResult::MaxIterations.as_str(), "MAX ITERATIONS");
        assert_eq!(SessionResult::Interrupted.as_str(), "INTERRUPTED");
    }

    #[test]
    fn test_session_result_is_success() {
        assert!(SessionResult::PromiseFound.is_success());
        assert!(!SessionResult::MaxIterations.is_success());
        assert!(!SessionResult::Interrupted.is_success());
    }

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn test_format_tokens_thousand() {
        assert_eq!(format_tokens(1_000), "1.0K");
        assert_eq!(format_tokens(12_345), "12.3K");
    }

    #[test]
    fn test_format_tokens_million() {
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(5_600_000), "5.6M");
    }
}
