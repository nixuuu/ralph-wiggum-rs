use std::sync::LazyLock;

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

/// Colorize tool name based on tool type
pub fn colorize_tool_name(name: &str) -> String {
    use crossterm::style::Stylize;
    match name {
        "Read" | "Glob" | "Grep" => name.cyan().to_string(),
        "Write" | "Edit" => name.yellow().to_string(),
        "Bash" => name.magenta().to_string(),
        "Task" => name.blue().to_string(),
        "WebFetch" | "WebSearch" => name.green().to_string(),
        "TodoWrite" => name.white().to_string(),
        _ => name.white().to_string(),
    }
}

/// Format Edit tool as colored diff
fn format_edit_diff(path: &str, old: &str, new: &str) -> String {
    use crossterm::style::Stylize;

    let truncated_path = truncate_string(&shorten_path(path), MAX_DETAIL_WIDTH);
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // If diff is small (≤5 lines total), show inline diff
    if old_lines.len() + new_lines.len() <= 5 {
        let mut parts = vec![truncated_path];
        for line in &old_lines {
            parts.push(format!("\n    {} {}", "-".red(), truncate_string(line, 60)));
        }
        for line in &new_lines {
            parts.push(format!(
                "\n    {} {}",
                "+".green(),
                truncate_string(line, 60)
            ));
        }
        parts.join("")
    } else {
        // For larger diffs, show summary
        format!(
            "{} | {} {}",
            truncated_path,
            format!("-{}", old_lines.len()).red(),
            format!("+{}", new_lines.len()).green()
        )
    }
}

/// Format Read/Write tool - simple file path
fn format_file_path(input: &serde_json::Value) -> Option<String> {
    input
        .get("file_path")
        .and_then(|v| v.as_str())
        .map(|path| truncate_string(&shorten_path(path), MAX_DETAIL_WIDTH))
}

/// Format Edit tool - show diff
fn format_edit_tool(input: &serde_json::Value) -> Option<String> {
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
fn format_bash_tool(input: &serde_json::Value) -> Option<String> {
    let desc = input.get("description").and_then(|v| v.as_str());
    let cmd = input.get("command").and_then(|v| v.as_str());
    match (desc, cmd) {
        (Some(d), Some(c)) => Some(truncate_string(
            &shorten_path(&format!("{}: {}", d, c)),
            MAX_DETAIL_WIDTH,
        )),
        (Some(d), None) => Some(truncate_string(&shorten_path(d), MAX_DETAIL_WIDTH)),
        (None, Some(c)) => Some(truncate_string(&shorten_path(c), MAX_DETAIL_WIDTH)),
        _ => None,
    }
}

/// Format Glob tool - pattern in path
fn format_glob_tool(input: &serde_json::Value) -> Option<String> {
    let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    Some(truncate_string(
        &format!("{} in {}", pattern, shorten_path(path)),
        MAX_DETAIL_WIDTH,
    ))
}

/// Format Grep tool - quoted pattern in path
fn format_grep_tool(input: &serde_json::Value) -> Option<String> {
    let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    Some(truncate_string(
        &format!(
            "\"{}\" in {}",
            truncate_string(pattern, 30),
            shorten_path(path)
        ),
        MAX_DETAIL_WIDTH,
    ))
}

/// Format Task tool - [agent] description
fn format_task_tool(input: &serde_json::Value) -> Option<String> {
    let desc = input.get("description").and_then(|v| v.as_str())?;
    let agent = input
        .get("subagent_type")
        .and_then(|v| v.as_str())
        .unwrap_or("agent");
    Some(truncate_string(
        &format!("[{}] {}", agent, desc),
        MAX_DETAIL_WIDTH,
    ))
}

/// Format WebFetch tool - description (url)
fn format_web_fetch_tool(input: &serde_json::Value) -> Option<String> {
    let desc = input.get("prompt").and_then(|v| v.as_str());
    let url = input.get("url").and_then(|v| v.as_str());
    match (desc, url) {
        (Some(d), Some(u)) => Some(truncate_string(
            &format!("{} ({})", d, truncate_string(u, 40)),
            MAX_DETAIL_WIDTH,
        )),
        (None, Some(u)) => Some(truncate_string(u, MAX_DETAIL_WIDTH)),
        (Some(d), None) => Some(truncate_string(d, MAX_DETAIL_WIDTH)),
        _ => None,
    }
}

/// Format WebSearch tool - quoted query
fn format_web_search_tool(input: &serde_json::Value) -> Option<String> {
    input
        .get("query")
        .and_then(|v| v.as_str())
        .map(|query| truncate_string(&format!("\"{}\"", query), MAX_DETAIL_WIDTH))
}

/// Format TodoWrite tool - task count summary
fn format_todo_write_tool(input: &serde_json::Value) -> Option<String> {
    let todos = input.get("todos").and_then(|v| v.as_array())?;
    let in_progress: Vec<_> = todos
        .iter()
        .filter(|t| t.get("status").and_then(|s| s.as_str()) == Some("in_progress"))
        .filter_map(|t| t.get("content").and_then(|c| c.as_str()))
        .collect();

    if !in_progress.is_empty() {
        Some(format!("{} task(s) in progress", in_progress.len()))
    } else {
        Some(format!("{} task(s)", todos.len()))
    }
}

/// Format tool details for display - main entry point
pub fn format_tool_details(name: &str, input: &serde_json::Value) -> String {
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
                .map(|desc| truncate_string(desc, MAX_DETAIL_WIDTH))
        }
    };

    result.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_colorize_tool_name() {
        // Just verify it doesn't panic
        let _ = colorize_tool_name("Read");
        let _ = colorize_tool_name("Write");
        let _ = colorize_tool_name("Bash");
        let _ = colorize_tool_name("Unknown");
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
    fn test_shorten_path_boundary_matching() {
        // Test boundary matching for CWD
        // Mock scenario: if CWD were "/a/b/c"
        // Path "/a/b/c-extra/file" should NOT match (no boundary)
        // Path "/a/b/c/file" should match (has / after CWD)
        // Path "/a/b/c" should match (end of string)

        // We can't easily mock CWD, but we can test the logic doesn't break
        // on paths that could cause false positives
        let test_cases = vec![
            // These should pass through without panicking
            "/usr/local-bin/test", // Would be false positive if not checking boundary
            "/home/user-extra/file.txt", // Would be false positive
            "/Users/nix/file.txt", // Normal case
            "/Users/nixer/file.txt", // Should not match HOME if HOME=/Users/nix
        ];

        for path in test_cases {
            let result = shorten_path(path);
            assert!(!result.is_empty());
        }
    }

    #[test]
    fn test_shorten_path_exact_boundary() {
        // Test that exact CWD match (no trailing slash) is handled correctly
        // This is a regression test for the boundary matching bug

        // Get actual CWD for testing
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        if !cwd.is_empty() {
            // Test 1: CWD itself should be replaced with "."
            let result = shorten_path(&cwd);
            assert_eq!(result, ".");

            // Test 2: CWD with trailing content after "/" should be shortened
            let path_with_file = format!("{}/test.txt", cwd);
            let result = shorten_path(&path_with_file);
            assert_eq!(result, "test.txt");

            // Test 3: Path that starts with CWD but has extra chars (no /) should NOT match
            // Use a path that doesn't contain HOME to avoid HOME replacement taking precedence
            let false_positive = format!("{}-extra/file.txt", cwd);
            let result = shorten_path(&false_positive);

            // Note: if CWD is under HOME, the result will have HOME replaced with ~
            // So we need to check if the result still contains the "-extra" part
            // which proves the boundary check prevented CWD replacement
            assert!(
                result.contains("-extra"),
                "Expected path to contain '-extra', got: {}",
                result
            );

            // Test 4: CWD in middle of path (with trailing slash) should NOT match
            let cwd_in_middle = format!("/some/prefix{}/file.txt", cwd);
            let result = shorten_path(&cwd_in_middle);

            // Should NOT be shortened - CWD is not at start or after /
            assert!(
                !result.starts_with('/') || result.contains(&cwd) || result.contains('~'),
                "Expected CWD in middle to not be replaced, got: {}",
                result
            );
        }
    }

    #[test]
    fn test_shorten_path_home_boundary() {
        // Test HOME directory boundary matching
        let home = std::env::var("HOME").unwrap_or_default();

        if !home.is_empty() {
            // Test 1: HOME itself should be replaced with "~"
            let result = shorten_path(&home);
            assert_eq!(result, "~");

            // Test 2: HOME with subdirectory should be shortened
            let path_with_subdir = format!("{}/Documents/test.txt", home);
            let result = shorten_path(&path_with_subdir);
            assert_eq!(result, "~/Documents/test.txt");

            // Test 3: Path similar to HOME but with extra chars (no /) should NOT match
            let false_positive = format!("{}er/file.txt", home);
            let result = shorten_path(&false_positive);
            // Should return original path since it's not a real HOME subdirectory
            assert_eq!(result, false_positive);
        }
    }

    #[test]
    fn test_shorten_path_match_only_at_start() {
        // Edge case: HOME/CWD in middle of path should NOT be replaced
        let home = std::env::var("HOME").unwrap_or_default();

        if !home.is_empty() {
            // Test HOME: path with HOME in the middle should NOT be shortened
            let path_with_home_in_middle = format!("/some/prefix{}/file.txt", home);
            let result = shorten_path(&path_with_home_in_middle);

            // Should NOT contain "~" because HOME is not at the start
            assert!(
                !result.contains('~'),
                "Expected no ~ replacement when HOME is in middle of path, got: {}",
                result
            );
        }

        // Test CWD: Use a simple synthetic path that won't contain HOME
        // Mock scenario: imagine CWD is "/a/b/c"
        // Path "/x/y/a/b/c/file.txt" should NOT have CWD replaced
        // We can't easily test this without mocking, but we can verify the function
        // doesn't break on complex paths
        let complex_paths = vec![
            "/var/tmp/project/nested/path/file.txt",
            "/opt/local/bin/executable",
        ];

        for path in complex_paths {
            let result = shorten_path(path);
            // Should not panic and should return a valid string
            assert!(!result.is_empty());
        }
    }

    // Tests for shorten_path_with() with controlled CWD and HOME values
    // These tests verify boundary matching with predictable inputs

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
    fn test_shorten_path_with_cwd_embedded_after_slash() {
        // Test that CWD replacement works when CWD appears after '/' in string
        // The pattern "/a/b/c/" gets matched and remaining content follows
        let result = shorten_path_with("/prefix//a/b/c/file", "/a/b/c", "");
        assert_eq!(result, "/prefix/file", "CWD/ pattern should be replaced");
    }

    #[test]
    fn test_shorten_path_with_cwd_embedded_after_slash_no_content() {
        // Test that CWD/ at end returns "." when after a prefix
        let result = shorten_path_with("/prefix//a/b/c/", "/a/b/c", "");
        assert_eq!(
            result, "/prefix/.",
            "CWD/ at end after prefix should append '.'"
        );
    }

    #[test]
    fn test_shorten_path_with_cwd_not_after_space() {
        // Test that CWD does NOT match when preceded by space (not '/')
        let result = shorten_path_with("text /a/b/c/file end", "/a/b/c", "");
        assert_eq!(
            result, "text /a/b/c/file end",
            "CWD preceded by space should not match"
        );
    }

    #[test]
    fn test_shorten_path_with_cwd_no_boundary_in_middle() {
        // Test that CWD does NOT match when it's in the middle without proper boundary
        let result = shorten_path_with("/prefix-a/b/c/file", "/a/b/c", "");
        assert_eq!(
            result, "/prefix-a/b/c/file",
            "CWD without proper boundary should not match"
        );
    }

    #[test]
    fn test_shorten_path_with_home_after_slash() {
        // Test that HOME matches when preceded by '/'
        // HOME gets replaced with ~, the preceding / stays
        let result = shorten_path_with("/prefix//Users/nix/file", "", "/Users/nix");
        assert_eq!(
            result, "/prefix/~/file",
            "HOME after '/' gets replaced with ~"
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
    fn test_shorten_path_with_multiple_cwd_occurrences() {
        // Test that only the first occurrence of CWD is replaced
        let result = shorten_path_with("/a/b/c/subdir/a/b/c/file", "/a/b/c", "");
        // First /a/b/c/ gets replaced, second remains
        assert_eq!(
            result, "subdir/a/b/c/file",
            "Only first CWD occurrence should be replaced"
        );
    }

    #[test]
    fn test_shorten_path_with_empty_cwd_and_home() {
        // Test that nothing happens with empty CWD and HOME
        let result = shorten_path_with("/some/path/file", "", "");
        assert_eq!(
            result, "/some/path/file",
            "No replacement with empty CWD and HOME"
        );
    }

    #[test]
    fn test_shorten_path_with_relative_path() {
        // Test that relative paths pass through unchanged
        let result = shorten_path_with("relative/path/file", "/a/b/c", "/Users/nix");
        assert_eq!(
            result, "relative/path/file",
            "Relative paths should not be modified"
        );
    }

    #[test]
    fn test_shorten_path_with_cwd_trailing_slash_variant() {
        // Test the CWD/ pattern (with trailing slash in path)
        // Should return "." to represent current directory
        let result = shorten_path_with("/a/b/c/", "/a/b/c", "");
        assert_eq!(result, ".", "CWD with trailing slash should be '.'");
    }

    #[test]
    fn test_shorten_path_with_cwd_with_content_after_slash() {
        // Test CWD with content after trailing slash
        let result = shorten_path_with("/a/b/c/file.txt", "/a/b/c", "");
        assert_eq!(
            result, "file.txt",
            "CWD/ pattern should leave only the file"
        );
    }

    #[test]
    fn test_shorten_path_with_home_deep_nested() {
        // Test HOME replacement with deeply nested path
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
}
