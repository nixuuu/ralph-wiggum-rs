//! Tool formatting utilities for TUI — Span-based output dla Claude tool calls.
//!
//! Moduł odpowiada za formatowanie tool calls (Read, Write, Edit, Bash, itd.)
//! jako ratatui Span-y z odpowiednimi kolorami. Zastępuje String-based formatting
//! z crossterm ANSI codes na natywne typy ratatui.
//!
//! # Architektura
//!
//! - `colorize_tool_name()` → zwraca `Span<'static>` z kolorowaną nazwą toola
//! - `format_tool_details()` → zwraca `Vec<Span<'static>>` z parametrami toola
//! - `shorten_path()`, `truncate_string()` → pure String utilities (bez koloru)
//!
//! # Przykład użycia
//!
//! ```ignore
//! use ralph_wiggum::tui::tool_formatting::{colorize_tool_name, format_tool_details};
//! use ratatui::text::Line;
//!
//! let name_span = colorize_tool_name("Read");
//! let detail_spans = format_tool_details("Read", &tool_input_json);
//!
//! let line = Line::from(vec![
//!     name_span,
//!     Span::raw(" "),
//!     // ... detail_spans
//! ]);
//! ```

use ratatui::style::Color;
use ratatui::text::Span;
use serde_json::Value;
use std::sync::LazyLock;

use crate::tui::theme::DEFAULT_THEME;

/// Maximum width for tool detail strings (paths, descriptions, etc.)
const MAX_DETAIL_WIDTH: usize = 100;

/// Cached current working directory for path shortening
static CWD: LazyLock<String> = LazyLock::new(|| {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
});

/// Cached home directory for path shortening
static HOME_DIR: LazyLock<String> = LazyLock::new(|| std::env::var("HOME").unwrap_or_default());

// ============ Pure String utilities (bez koloru) ============

/// Shorten absolute paths for display: CWD → relative, HOME → ~
/// This is the internal implementation that takes explicit cwd and home values.
/// Only replaces the first occurrence to avoid issues with repeated path components.
fn shorten_path_with(s: &str, cwd: &str, home: &str) -> String {
    if !cwd.is_empty() {
        let with_slash = format!("{}/", cwd);
        if let Some(pos) = s.find(&with_slash) {
            // Check boundary: match must be at start OR preceded by '/'
            let prev_ok = pos == 0 || s.as_bytes().get(pos - 1) == Some(&b'/');

            if prev_ok {
                let after = &s[pos + with_slash.len()..];
                let mut result = String::with_capacity(s.len());
                result.push_str(&s[..pos]);
                // If nothing after CWD/, return "." to represent current directory
                if after.is_empty() {
                    result.push('.');
                } else {
                    result.push_str(after);
                }
                return result;
            }
        }
        if let Some(pos) = s.find(cwd) {
            // Check boundary: match must be at start OR preceded by '/'
            // AND next char must be '/' or end of string
            let prev_ok = pos == 0 || s.as_bytes().get(pos - 1) == Some(&b'/');
            let next_byte = s.as_bytes().get(pos + cwd.len());
            let next_ok = next_byte.is_none() || next_byte == Some(&b'/');

            if prev_ok && next_ok {
                let mut result = String::with_capacity(s.len());
                result.push_str(&s[..pos]);
                result.push('.');
                result.push_str(&s[pos + cwd.len()..]);
                return result;
            }
        }
    }
    if !home.is_empty()
        && let Some(pos) = s.find(home)
    {
        // Check boundary: match must be at start OR preceded by '/'
        // AND next char must be '/' or end of string
        let prev_ok = pos == 0 || s.as_bytes().get(pos - 1) == Some(&b'/');
        let next_byte = s.as_bytes().get(pos + home.len());
        let next_ok = next_byte.is_none() || next_byte == Some(&b'/');

        if prev_ok && next_ok {
            let mut result = String::with_capacity(s.len());
            result.push_str(&s[..pos]);
            result.push('~');
            result.push_str(&s[pos + home.len()..]);
            return result;
        }
    }
    s.to_string()
}

/// Shorten absolute paths for display: CWD → relative, HOME → ~
/// Uses the current working directory and home directory from the environment.
pub fn shorten_path(s: &str) -> String {
    shorten_path_with(s, &CWD, &HOME_DIR)
}

/// Truncate string and add ellipsis if too long
/// Uses character count instead of byte count to properly handle Unicode (including emoji)
pub fn truncate_string(s: &str, max_len: usize) -> String {
    let s = s.replace('\n', "\\n").replace('\r', "");
    let char_count = s.chars().count();
    if char_count <= max_len {
        s
    } else {
        // Collect first max_len characters to avoid byte boundary issues
        let truncated: String = s.chars().take(max_len).collect();
        format!("{}...", truncated)
    }
}

// ============ Span-based tool formatting (z kolorami) ============

/// Colorize tool name based on tool type — zwraca Span<'static> z odpowiednim kolorem.
///
/// Mapuje nazwy toolów na kolory zgodnie z konwencją:
/// - Read/Glob/Grep → cyan (odczyt)
/// - Write/Edit → yellow (modyfikacja)
/// - Bash → magenta (wykonanie)
/// - Task → blue (delegacja)
/// - WebFetch/WebSearch → green (sieć)
/// - TodoWrite → white (organizacja)
pub fn colorize_tool_name(name: &str) -> Span<'static> {
    let color = match name {
        "Read" | "Glob" | "Grep" => Color::Cyan,
        "Write" | "Edit" => Color::Yellow,
        "Bash" => Color::Magenta,
        "Task" => Color::Blue,
        "WebFetch" | "WebSearch" => Color::Green,
        "TodoWrite" => Color::White,
        _ => Color::White,
    };
    Span::styled(name.to_string(), ratatui::style::Style::default().fg(color))
}

/// Format Edit tool as colored diff — zwraca Vec<Span> z czerwonymi minus i zielonymi plus.
fn format_edit_diff(path: &str, old: &str, new: &str) -> Vec<Span<'static>> {
    let truncated_path = truncate_string(&shorten_path(path), MAX_DETAIL_WIDTH);
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut spans = vec![Span::raw(truncated_path)];

    // If diff is small (≤5 lines total), show inline diff
    if old_lines.len() + new_lines.len() <= 5 {
        for line in &old_lines {
            spans.push(Span::raw("\n    "));
            spans.push(Span::styled(
                "-",
                ratatui::style::Style::default().fg(DEFAULT_THEME.error),
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::raw(truncate_string(line, 60)));
        }
        for line in &new_lines {
            spans.push(Span::raw("\n    "));
            spans.push(Span::styled(
                "+",
                ratatui::style::Style::default().fg(DEFAULT_THEME.success),
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::raw(truncate_string(line, 60)));
        }
    } else {
        // For larger diffs, show summary
        spans.push(Span::raw(" | "));
        spans.push(Span::styled(
            format!("-{}", old_lines.len()),
            ratatui::style::Style::default().fg(DEFAULT_THEME.error),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("+{}", new_lines.len()),
            ratatui::style::Style::default().fg(DEFAULT_THEME.success),
        ));
    }

    spans
}

/// Format Read/Write tool - simple file path
fn format_file_path(input: &Value) -> Option<Vec<Span<'static>>> {
    input.get("file_path").and_then(|v| v.as_str()).map(|path| {
        vec![Span::raw(truncate_string(
            &shorten_path(path),
            MAX_DETAIL_WIDTH,
        ))]
    })
}

/// Format Edit tool - show diff
fn format_edit_tool(input: &Value) -> Option<Vec<Span<'static>>> {
    let path = input.get("file_path").and_then(|v| v.as_str())?;
    let old = input
        .get("old_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let new = input
        .get("new_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Some(format_edit_diff(path, old, new))
}

/// Format Bash tool - description and/or command
fn format_bash_tool(input: &Value) -> Option<Vec<Span<'static>>> {
    let desc = input.get("description").and_then(|v| v.as_str());
    let cmd = input.get("command").and_then(|v| v.as_str());
    match (desc, cmd) {
        (Some(d), Some(c)) => Some(vec![Span::raw(truncate_string(
            &shorten_path(&format!("{}: {}", d, c)),
            MAX_DETAIL_WIDTH,
        ))]),
        (Some(d), None) => Some(vec![Span::raw(truncate_string(
            &shorten_path(d),
            MAX_DETAIL_WIDTH,
        ))]),
        (None, Some(c)) => Some(vec![Span::raw(truncate_string(
            &shorten_path(c),
            MAX_DETAIL_WIDTH,
        ))]),
        _ => None,
    }
}

/// Format Glob tool - pattern in path
fn format_glob_tool(input: &Value) -> Option<Vec<Span<'static>>> {
    let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    Some(vec![Span::raw(truncate_string(
        &format!("{} in {}", pattern, shorten_path(path)),
        MAX_DETAIL_WIDTH,
    ))])
}

/// Format Grep tool - quoted pattern in path
fn format_grep_tool(input: &Value) -> Option<Vec<Span<'static>>> {
    let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    Some(vec![Span::raw(truncate_string(
        &format!(
            "\"{}\" in {}",
            truncate_string(pattern, 30),
            shorten_path(path)
        ),
        MAX_DETAIL_WIDTH,
    ))])
}

/// Format Task tool - [agent] description
fn format_task_tool(input: &Value) -> Option<Vec<Span<'static>>> {
    let desc = input.get("description").and_then(|v| v.as_str())?;
    let agent = input
        .get("subagent_type")
        .and_then(|v| v.as_str())
        .unwrap_or("agent");
    Some(vec![Span::raw(truncate_string(
        &format!("[{}] {}", agent, desc),
        MAX_DETAIL_WIDTH,
    ))])
}

/// Format WebFetch tool - description (url)
fn format_web_fetch_tool(input: &Value) -> Option<Vec<Span<'static>>> {
    let desc = input.get("prompt").and_then(|v| v.as_str());
    let url = input.get("url").and_then(|v| v.as_str());
    match (desc, url) {
        (Some(d), Some(u)) => Some(vec![Span::raw(truncate_string(
            &format!("{} ({})", d, truncate_string(u, 40)),
            MAX_DETAIL_WIDTH,
        ))]),
        (None, Some(u)) => Some(vec![Span::raw(truncate_string(u, MAX_DETAIL_WIDTH))]),
        (Some(d), None) => Some(vec![Span::raw(truncate_string(d, MAX_DETAIL_WIDTH))]),
        _ => None,
    }
}

/// Format WebSearch tool - quoted query
fn format_web_search_tool(input: &Value) -> Option<Vec<Span<'static>>> {
    input.get("query").and_then(|v| v.as_str()).map(|query| {
        vec![Span::raw(truncate_string(
            &format!("\"{}\"", query),
            MAX_DETAIL_WIDTH,
        ))]
    })
}

/// Format TodoWrite tool - task count summary
fn format_todo_write_tool(input: &Value) -> Option<Vec<Span<'static>>> {
    let todos = input.get("todos").and_then(|v| v.as_array())?;
    let in_progress: Vec<_> = todos
        .iter()
        .filter(|t| t.get("status").and_then(|s| s.as_str()) == Some("in_progress"))
        .filter_map(|t| t.get("content").and_then(|c| c.as_str()))
        .collect();

    if !in_progress.is_empty() {
        Some(vec![Span::raw(format!(
            "{} task(s) in progress",
            in_progress.len()
        ))])
    } else {
        Some(vec![Span::raw(format!("{} task(s)", todos.len()))])
    }
}

/// Prettify tool name for display.
///
/// MCP tool names follow pattern `mcp__<server>__<tool>` — strip prefix and
/// show just the tool name with underscores replaced by spaces.
/// Built-in tools (Read, Write, etc.) are returned unchanged.
///
/// Examples:
/// - `mcp__ralph-tasks__tasks_summary` → `tasks summary`
/// - `mcp__context7__resolve-library-id` → `resolve-library-id`
/// - `Read` → `Read`
pub fn prettify_tool_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("mcp__") {
        // Strip server name: take everything after the second `__`
        if let Some(tool_part) = rest.split_once("__").map(|(_, tool)| tool) {
            return tool_part.replace('_', " ");
        }
    }
    name.to_string()
}

/// Format tool details for display - main entry point
///
/// Zwraca `Vec<Span<'static>>` dla parametrów toola. Pusty vec = brak szczegółów.
pub fn format_tool_details(name: &str, input: &Value) -> Vec<Span<'static>> {
    let result = match name {
        "Read" | "Write" => format_file_path(input),
        "Edit" => format_edit_tool(input),
        "Bash" => format_bash_tool(input),
        "Glob" => format_glob_tool(input),
        "Grep" => format_grep_tool(input),
        "Task" => format_task_tool(input),
        "WebFetch" => format_web_fetch_tool(input),
        "WebSearch" => format_web_search_tool(input),
        "TodoWrite" => format_todo_write_tool(input),
        _ => {
            // Fallback: check for common description field
            input
                .get("description")
                .and_then(|v| v.as_str())
                .map(|desc| vec![Span::raw(truncate_string(desc, MAX_DETAIL_WIDTH))])
        }
    };

    result.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============ prettify_tool_name tests ============

    #[test]
    fn test_prettify_tool_name_builtin() {
        assert_eq!(prettify_tool_name("Read"), "Read");
        assert_eq!(prettify_tool_name("Write"), "Write");
        assert_eq!(prettify_tool_name("Bash"), "Bash");
    }

    #[test]
    fn test_prettify_tool_name_mcp() {
        assert_eq!(
            prettify_tool_name("mcp__ralph-tasks__tasks_summary"),
            "tasks summary"
        );
        assert_eq!(
            prettify_tool_name("mcp__context7__resolve-library-id"),
            "resolve-library-id"
        );
        assert_eq!(
            prettify_tool_name("mcp__ralph-tasks__ask_user"),
            "ask user"
        );
    }

    #[test]
    fn test_prettify_tool_name_mcp_no_double_underscore() {
        // Edge case: mcp__ but no second __ separator
        assert_eq!(prettify_tool_name("mcp__something"), "mcp__something");
    }

    // ============ Pure String utilities tests ============

    #[test]
    fn test_truncate_string_ascii() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world", 5), "hello...");
        assert_eq!(truncate_string("abc", 3), "abc");
    }

    #[test]
    fn test_truncate_string_with_emoji() {
        // Emoji like 🔍 takes 4 bytes but is 1 character
        let s = "| 🔍 Do dodania |";
        // Should not panic - this was the original bug
        let result = truncate_string(s, 10);
        assert_eq!(result.chars().count(), 13); // 10 chars + "..."

        // Test with multiple emoji
        let emoji_str = "✅ ⚠️ 🔍 test";
        let truncated = truncate_string(emoji_str, 5);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_truncate_string_unicode_boundary() {
        // This is the exact case from the bug report
        let s = "| popover, calendar, radio-group, sheet | ✅ | ⚠️ | 🔍 Do dodania |";
        // Should not panic even with small max_len
        let result = truncate_string(s, 60);
        assert!(!result.is_empty());

        // Test truncation at various points
        for max_len in 1..=s.chars().count() {
            let _ = truncate_string(s, max_len); // Should not panic
        }
    }

    #[test]
    fn test_truncate_string_newlines() {
        assert_eq!(truncate_string("hello\nworld", 20), "hello\\nworld");
        assert_eq!(truncate_string("a\rb\nc", 10), "ab\\nc");
    }

    #[test]
    fn test_shorten_path_basic() {
        // Test with simple paths - these should pass through unchanged
        assert_eq!(shorten_path("simple.txt"), "simple.txt");
        assert_eq!(shorten_path("relative/path.txt"), "relative/path.txt");
    }

    #[test]
    fn test_shorten_path_single_occurrence() {
        // These tests verify that we only replace the first occurrence
        // We can't test actual CWD/HOME values as they vary per system,
        // but we can verify the function doesn't panic on various inputs
        let test_cases = vec![
            "/usr/local/bin/test",
            "/home/user/project/file.txt",
            "~/Documents/test.txt",
            "/tmp/test/tmp/file.txt", // repeated component
        ];
        for path in test_cases {
            let result = shorten_path(path);
            // Should not panic and should return non-empty string
            assert!(!result.is_empty());
        }
    }

    #[test]
    fn test_shorten_path_empty() {
        assert_eq!(shorten_path(""), "");
    }

    #[test]
    fn test_shorten_path_with_cwd_prefix_collision() {
        // Test that CWD="/a/b/c" does NOT match path="/a/b/c-extra/file"
        let result = shorten_path_with("/a/b/c-extra/file", "/a/b/c", "");
        assert_eq!(
            result, "/a/b/c-extra/file",
            "CWD should not match when followed by non-slash character"
        );
    }

    #[test]
    fn test_shorten_path_with_cwd_exact_match() {
        // Test that CWD="/a/b/c" matches exactly path="/a/b/c"
        let result = shorten_path_with("/a/b/c", "/a/b/c", "");
        assert_eq!(result, ".", "CWD exact match should be replaced with '.'");
    }

    #[test]
    fn test_shorten_path_with_cwd_subdirectory() {
        // Test that CWD="/a/b/c" matches path="/a/b/c/file"
        let result = shorten_path_with("/a/b/c/file", "/a/b/c", "");
        assert_eq!(result, "file", "CWD with subdirectory should be shortened");
    }

    #[test]
    fn test_shorten_path_with_home_prefix_collision() {
        // Test that HOME="/Users/nix" does NOT match path="/Users/nixer/file"
        let result = shorten_path_with("/Users/nixer/file", "", "/Users/nix");
        assert_eq!(
            result, "/Users/nixer/file",
            "HOME should not match when followed by non-slash character"
        );
    }

    #[test]
    fn test_shorten_path_with_home_exact_match() {
        // Test that HOME="/Users/nix" matches exactly path="/Users/nix"
        let result = shorten_path_with("/Users/nix", "", "/Users/nix");
        assert_eq!(result, "~", "HOME exact match should be replaced with '~'");
    }

    #[test]
    fn test_shorten_path_with_home_subdirectory() {
        // Test that HOME="/Users/nix" matches path="/Users/nix/file"
        let result = shorten_path_with("/Users/nix/file", "", "/Users/nix");
        assert_eq!(
            result, "~/file",
            "HOME with subdirectory should be shortened"
        );
    }

    #[test]
    fn test_shorten_path_with_cwd_priority_over_home() {
        // Test that CWD replacement happens before HOME
        // If CWD is under HOME, CWD should be replaced first
        let result = shorten_path_with(
            "/Users/nix/project/file",
            "/Users/nix/project",
            "/Users/nix",
        );
        assert_eq!(result, "file", "CWD should take priority over HOME");
    }

    #[test]
    fn test_shorten_path_with_cwd_embedded_after_slash() {
        // CWD replacement works when CWD appears after '/' in string
        let result = shorten_path_with("/prefix//a/b/c/file", "/a/b/c", "");
        assert_eq!(result, "/prefix/file", "CWD/ pattern should be replaced");
    }

    #[test]
    fn test_shorten_path_with_cwd_embedded_after_slash_no_content() {
        // CWD/ at end returns "." when after a prefix
        let result = shorten_path_with("/prefix//a/b/c/", "/a/b/c", "");
        assert_eq!(
            result, "/prefix/.",
            "CWD/ at end after prefix should append '.'"
        );
    }

    #[test]
    fn test_shorten_path_with_cwd_not_after_space() {
        // CWD does NOT match when preceded by space (not '/')
        let result = shorten_path_with("text /a/b/c/file end", "/a/b/c", "");
        assert_eq!(
            result, "text /a/b/c/file end",
            "CWD preceded by space should not match"
        );
    }

    #[test]
    fn test_shorten_path_with_cwd_no_boundary_in_middle() {
        // CWD does NOT match when it's in the middle without proper boundary
        let result = shorten_path_with("/prefix-a/b/c/file", "/a/b/c", "");
        assert_eq!(
            result, "/prefix-a/b/c/file",
            "CWD without proper boundary should not match"
        );
    }

    #[test]
    fn test_shorten_path_with_home_after_slash() {
        // HOME matches when preceded by '/'
        let result = shorten_path_with("/prefix//Users/nix/file", "", "/Users/nix");
        assert_eq!(
            result, "/prefix/~/file",
            "HOME after '/' gets replaced with ~"
        );
    }

    #[test]
    fn test_shorten_path_with_multiple_cwd_occurrences() {
        // Only the first occurrence of CWD is replaced
        let result = shorten_path_with("/a/b/c/subdir/a/b/c/file", "/a/b/c", "");
        assert_eq!(
            result, "subdir/a/b/c/file",
            "Only first CWD occurrence should be replaced"
        );
    }

    #[test]
    fn test_shorten_path_with_empty_cwd_and_home() {
        let result = shorten_path_with("/some/path/file", "", "");
        assert_eq!(
            result, "/some/path/file",
            "No replacement with empty CWD and HOME"
        );
    }

    #[test]
    fn test_shorten_path_with_relative_path() {
        let result = shorten_path_with("relative/path/file", "/a/b/c", "/Users/nix");
        assert_eq!(
            result, "relative/path/file",
            "Relative paths should not be modified"
        );
    }

    #[test]
    fn test_shorten_path_with_cwd_trailing_slash_variant() {
        // CWD/ pattern (with trailing slash in path) → "."
        let result = shorten_path_with("/a/b/c/", "/a/b/c", "");
        assert_eq!(result, ".", "CWD with trailing slash should be '.'");
    }

    #[test]
    fn test_shorten_path_with_cwd_with_content_after_slash() {
        let result = shorten_path_with("/a/b/c/file.txt", "/a/b/c", "");
        assert_eq!(
            result, "file.txt",
            "CWD/ pattern should leave only the file"
        );
    }

    #[test]
    fn test_shorten_path_with_home_deep_nested() {
        let result = shorten_path_with(
            "/Users/nix/Documents/Projects/rust/file.rs",
            "",
            "/Users/nix",
        );
        assert_eq!(
            result, "~/Documents/Projects/rust/file.rs",
            "Deep nested HOME path should be shortened"
        );
    }

    #[test]
    fn test_shorten_path_boundary_matching() {
        let test_cases = vec![
            "/usr/local-bin/test",
            "/home/user-extra/file.txt",
            "/Users/nix/file.txt",
            "/Users/nixer/file.txt",
        ];
        for path in test_cases {
            let result = shorten_path(path);
            assert!(!result.is_empty());
        }
    }

    #[test]
    fn test_shorten_path_exact_boundary_with_real_cwd() {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        if !cwd.is_empty() {
            assert_eq!(shorten_path(&cwd), ".");

            let path_with_file = format!("{}/test.txt", cwd);
            assert_eq!(shorten_path(&path_with_file), "test.txt");

            // CWD prefix collision: CWD + "-extra" should NOT match
            let false_positive = format!("{}-extra/file.txt", cwd);
            let result = shorten_path(&false_positive);
            assert!(
                result.contains("-extra"),
                "Expected path to contain '-extra', got: {}",
                result
            );
        }
    }

    #[test]
    fn test_shorten_path_home_boundary_with_real_home() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            // HOME+subdirectory should shorten (CWD may take priority though)
            let path_with_subdir = format!("{}/Documents/test.txt", home);
            let result = shorten_path(&path_with_subdir);
            // Either CWD replaces it or HOME replaces it with ~
            assert!(
                result.contains("Documents/test.txt"),
                "Should keep relative part, got: {}",
                result
            );

            // HOME prefix collision: HOME + "er" should NOT match
            let false_positive = format!("{}er/file.txt", home);
            let result = shorten_path(&false_positive);
            assert_eq!(result, false_positive);
        }
    }

    // ============ Span-based formatting tests ============

    #[test]
    fn test_colorize_tool_name_returns_span() {
        let span = colorize_tool_name("Read");
        assert_eq!(span.content, "Read");
        assert_eq!(span.style.fg, Some(Color::Cyan));
    }

    #[test]
    fn test_colorize_tool_name_colors() {
        // Read/Glob/Grep → cyan
        assert_eq!(colorize_tool_name("Read").style.fg, Some(Color::Cyan));
        assert_eq!(colorize_tool_name("Glob").style.fg, Some(Color::Cyan));
        assert_eq!(colorize_tool_name("Grep").style.fg, Some(Color::Cyan));

        // Write/Edit → yellow
        assert_eq!(colorize_tool_name("Write").style.fg, Some(Color::Yellow));
        assert_eq!(colorize_tool_name("Edit").style.fg, Some(Color::Yellow));

        // Bash → magenta
        assert_eq!(colorize_tool_name("Bash").style.fg, Some(Color::Magenta));

        // Task → blue
        assert_eq!(colorize_tool_name("Task").style.fg, Some(Color::Blue));

        // WebFetch/WebSearch → green
        assert_eq!(colorize_tool_name("WebFetch").style.fg, Some(Color::Green));
        assert_eq!(colorize_tool_name("WebSearch").style.fg, Some(Color::Green));

        // TodoWrite → white
        assert_eq!(colorize_tool_name("TodoWrite").style.fg, Some(Color::White));

        // Unknown → white
        assert_eq!(colorize_tool_name("Unknown").style.fg, Some(Color::White));
    }

    #[test]
    fn test_format_tool_details_read() {
        let input = serde_json::json!({
            "file_path": "/home/user/test.txt"
        });
        let spans = format_tool_details("Read", &input);
        assert_eq!(spans.len(), 1);
        // Should contain shortened path
        assert!(spans[0].content.contains("test.txt"));
    }

    #[test]
    fn test_format_tool_details_bash() {
        let input = serde_json::json!({
            "description": "List files",
            "command": "ls -la"
        });
        let spans = format_tool_details("Bash", &input);
        assert_eq!(spans.len(), 1);
        assert!(spans[0].content.contains("List files"));
        assert!(spans[0].content.contains("ls -la"));
    }

    #[test]
    fn test_format_tool_details_grep() {
        let input = serde_json::json!({
            "pattern": "TODO",
            "path": "/home/user/project"
        });
        let spans = format_tool_details("Grep", &input);
        assert_eq!(spans.len(), 1);
        assert!(spans[0].content.contains("TODO"));
    }

    #[test]
    fn test_format_tool_details_task() {
        let input = serde_json::json!({
            "description": "Run tests",
            "subagent_type": "test-runner"
        });
        let spans = format_tool_details("Task", &input);
        assert_eq!(spans.len(), 1);
        assert!(spans[0].content.contains("[test-runner]"));
        assert!(spans[0].content.contains("Run tests"));
    }

    #[test]
    fn test_format_tool_details_edit_small_diff() {
        let input = serde_json::json!({
            "file_path": "/home/user/test.txt",
            "old_string": "old line",
            "new_string": "new line"
        });
        let spans = format_tool_details("Edit", &input);
        // Should have path + newlines + diff markers + lines
        assert!(!spans.is_empty());
        // First span should be the path
        assert!(spans[0].content.contains("test.txt"));
    }

    #[test]
    fn test_format_tool_details_edit_large_diff() {
        let old_lines = (0..10)
            .map(|i| format!("old line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let new_lines = (0..10)
            .map(|i| format!("new line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let input = serde_json::json!({
            "file_path": "/home/user/test.txt",
            "old_string": old_lines,
            "new_string": new_lines
        });
        let spans = format_tool_details("Edit", &input);
        // Should show summary format (path | -N +M)
        assert!(!spans.is_empty());
    }

    #[test]
    fn test_format_tool_details_todo_write() {
        let input = serde_json::json!({
            "todos": [
                {"status": "in_progress", "content": "Task 1"},
                {"status": "pending", "content": "Task 2"}
            ]
        });
        let spans = format_tool_details("TodoWrite", &input);
        assert_eq!(spans.len(), 1);
        assert!(spans[0].content.contains("1 task(s) in progress"));
    }

    #[test]
    fn test_format_tool_details_unknown_with_description() {
        let input = serde_json::json!({
            "description": "Some custom action"
        });
        let spans = format_tool_details("CustomTool", &input);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "Some custom action");
    }

    #[test]
    fn test_format_tool_details_empty_input() {
        let input = serde_json::json!({});
        let spans = format_tool_details("Read", &input);
        // Should return empty vec when no file_path
        assert!(spans.is_empty());
    }

    // ============ Snapshot tests ============

    #[test]
    fn test_snapshot_colorize_tool_name_all_types() {
        let mut output = Vec::new();
        let tools = vec![
            "Read",
            "Glob",
            "Grep",
            "Write",
            "Edit",
            "Bash",
            "Task",
            "WebFetch",
            "WebSearch",
            "TodoWrite",
            "Unknown",
        ];
        for tool in tools {
            let span = colorize_tool_name(tool);
            output.push(format!(
                "{}: color={:?}",
                tool,
                span.style
                    .fg
                    .map(|c| format!("{:?}", c))
                    .unwrap_or("None".to_string())
            ));
        }
        insta::assert_snapshot!(output.join("\n"));
    }

    #[test]
    fn test_snapshot_format_tool_details_read() {
        let input = serde_json::json!({
            "file_path": "/Users/nix/project/src/main.rs"
        });
        let spans = format_tool_details("Read", &input);
        let output: Vec<String> = spans.iter().map(|s| s.content.to_string()).collect();
        insta::assert_snapshot!(output.join(""));
    }

    #[test]
    fn test_snapshot_format_tool_details_edit_small_diff() {
        let input = serde_json::json!({
            "file_path": "/Users/nix/project/test.txt",
            "old_string": "old line 1\nold line 2",
            "new_string": "new line 1\nnew line 2"
        });
        let spans = format_tool_details("Edit", &input);
        let output: Vec<String> = spans
            .iter()
            .map(|s| {
                if let Some(fg) = s.style.fg {
                    format!("[{:?}]{}", fg, s.content)
                } else {
                    s.content.to_string()
                }
            })
            .collect();
        insta::assert_snapshot!(output.join(""));
    }

    #[test]
    fn test_snapshot_format_tool_details_bash() {
        let input = serde_json::json!({
            "description": "Run tests",
            "command": "cargo test --all"
        });
        let spans = format_tool_details("Bash", &input);
        let output: Vec<String> = spans.iter().map(|s| s.content.to_string()).collect();
        insta::assert_snapshot!(output.join(""));
    }

    #[test]
    fn test_snapshot_format_tool_details_grep() {
        let input = serde_json::json!({
            "pattern": "TODO|FIXME",
            "path": "/Users/nix/project/src"
        });
        let spans = format_tool_details("Grep", &input);
        let output: Vec<String> = spans.iter().map(|s| s.content.to_string()).collect();
        insta::assert_snapshot!(output.join(""));
    }

    #[test]
    fn test_snapshot_format_tool_details_task() {
        let input = serde_json::json!({
            "description": "Explore codebase architecture",
            "subagent_type": "Explore"
        });
        let spans = format_tool_details("Task", &input);
        let output: Vec<String> = spans.iter().map(|s| s.content.to_string()).collect();
        insta::assert_snapshot!(output.join(""));
    }

    #[test]
    fn test_snapshot_format_tool_details_web_search() {
        let input = serde_json::json!({
            "query": "rust ratatui span styling examples"
        });
        let spans = format_tool_details("WebSearch", &input);
        let output: Vec<String> = spans.iter().map(|s| s.content.to_string()).collect();
        insta::assert_snapshot!(output.join(""));
    }

    #[test]
    fn test_snapshot_truncate_long_path() {
        let long_path = "/Users/nix/very/deep/nested/directory/structure/with/many/levels/that/exceeds/max/width/file.rs";
        let result = truncate_string(long_path, MAX_DETAIL_WIDTH);
        insta::assert_snapshot!(result);
    }
}
