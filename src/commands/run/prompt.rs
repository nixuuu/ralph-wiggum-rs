const DEFAULT_SYSTEM_SUFFIX: &str = r#"When you determine that the task is fully completed and verified, end your
response with the tag:
<promise>{promise}</promise>

IMPORTANT: Only use the <promise> tag when you are certain the task is
complete. Do not lie to exit the loop!"#;

/// Build system prompt with optional custom prefix and placeholder support
///
/// Available placeholders in custom_prefix:
/// - {iteration} - current iteration number
/// - {promise} - completion promise text
/// - {min_iterations} - minimum iterations before accepting promise
/// - {max_iterations} - maximum iterations (0 = unlimited)
pub fn build_system_prompt(
    custom_prefix: Option<&str>,
    iteration: u32,
    completion_promise: &str,
    min_iterations: u32,
    max_iterations: u32,
) -> String {
    let mut result = String::new();

    if let Some(prefix) = custom_prefix {
        let processed = prefix
            .replace("{iteration}", &iteration.to_string())
            .replace("{promise}", completion_promise)
            .replace("{min_iterations}", &min_iterations.to_string())
            .replace("{max_iterations}", &max_iterations.to_string());
        result.push_str(&processed);
        result.push_str("\n\n");
    }

    result.push_str(&DEFAULT_SYSTEM_SUFFIX.replace("{promise}", completion_promise));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt_without_custom_prefix() {
        let result = build_system_prompt(None, 1, "done", 1, 10);
        assert!(result.contains("<promise>done</promise>"));
        assert!(result.contains("IMPORTANT"));
        // Should NOT contain custom prefix content
        assert!(!result.contains("Iteration #"));
    }

    #[test]
    fn test_build_system_prompt_with_custom_prefix() {
        let prefix = "## My Loop\n\nIteration #{iteration}";
        let result = build_system_prompt(Some(prefix), 2, "finished", 1, 10);
        assert!(result.contains("## My Loop"));
        assert!(result.contains("Iteration #2"));
        assert!(result.contains("<promise>finished</promise>"));
    }

    #[test]
    fn test_build_system_prompt_all_placeholders() {
        let prefix = "{iteration}/{min_iterations}/{max_iterations} - {promise}";
        let result = build_system_prompt(Some(prefix), 3, "done", 2, 5);
        assert!(result.contains("3/2/5 - done"));
    }

    #[test]
    fn test_build_system_prompt_always_includes_promise_instructions() {
        let result = build_system_prompt(None, 1, "test_promise", 1, 10);
        assert!(result.contains("<promise>test_promise</promise>"));
        assert!(result.contains("Only use the <promise> tag"));
    }
}
