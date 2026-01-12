const SYSTEM_WRAPPER_TEMPLATE: &str = r#"## Ralph Wiggum Loop - System Instructions

This is iteration #{iteration} of the ralph-wiggum loop.

You are working iteratively in a loop. This means you receive the same task
multiple times, so that in subsequent iterations you can verify whether the
previous iteration actually completed what needed to be done.

**How to end the loop:**
When you determine that the task is fully completed and verified, end your
response with the tag:
<promise>{promise}</promise>

IMPORTANT: Only use the <promise> tag when you are certain the task is
complete. Do not lie to exit the loop!"#;

/// Build system prompt with loop instructions
pub fn build_system_prompt(completion_promise: &str, iteration: u32) -> String {
    SYSTEM_WRAPPER_TEMPLATE
        .replace("{iteration}", &iteration.to_string())
        .replace("{promise}", completion_promise)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt() {
        let result = build_system_prompt("done", 1);
        assert!(result.contains("<promise>done</promise>"));
        assert!(result.contains("Ralph Wiggum Loop"));
        assert!(result.contains("iteration #1"));
    }

    #[test]
    fn test_build_system_prompt_iteration_2() {
        let result = build_system_prompt("done", 2);
        assert!(result.contains("iteration #2"));
    }

    #[test]
    fn test_system_prompt_is_in_english() {
        let result = build_system_prompt("done", 1);
        assert!(result.contains("System Instructions"));
        assert!(result.contains("working iteratively"));
        assert!(result.contains("How to end the loop"));
    }
}
