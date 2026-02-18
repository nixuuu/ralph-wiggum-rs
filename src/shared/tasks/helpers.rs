use crate::shared::tasks::LeafTask;

/// Shorten a model identifier for display in TUI panels.
///
/// Known full IDs map to their canonical short names.
/// Unknown `claude-*` IDs have the `claude-` prefix stripped.
/// Short names and other strings pass through unchanged.
pub fn shorten_model_name(model: &str) -> String {
    match model {
        "claude-opus-4-6" => "opus".to_string(),
        "claude-sonnet-4-5-20250929" => "sonnet".to_string(),
        "claude-haiku-4-5-20251001" => "haiku".to_string(),
        other if other.starts_with("claude-") => other["claude-".len()..].to_string(),
        other => other.to_string(),
    }
}

/// Build a rich task description from a LeafTask for use in prompts.
pub fn format_task_prompt(leaf: &LeafTask) -> String {
    use std::fmt::Write;

    let mut parts = Vec::new();
    parts.push(format!("**ID:** {}", leaf.id));
    parts.push(format!("**Component:** {}", leaf.component));
    parts.push(format!("**Name:** {}", leaf.name));

    if let Some(ref desc) = leaf.description {
        parts.push(String::new());
        parts.push(format!("**Description:**\n{desc}"));
    }

    if !leaf.related_files.is_empty() {
        parts.push(String::new());
        // Pre-allocate: "- " (2) + avg filename (30) + "\n" (1) per file
        let mut files = String::with_capacity(leaf.related_files.len() * 33);
        for f in &leaf.related_files {
            let _ = writeln!(files, "- {f}");
        }
        files.pop(); // Remove trailing newline
        parts.push(format!("**Related files:**\n{files}"));
    }

    if !leaf.implementation_steps.is_empty() {
        parts.push(String::new());
        // Pre-allocate: "N. " (3) + avg step text (50) + "\n" (1) per step
        let mut steps = String::with_capacity(leaf.implementation_steps.len() * 54);
        for (i, s) in leaf.implementation_steps.iter().enumerate() {
            let _ = writeln!(steps, "{}. {s}", i + 1);
        }
        steps.pop(); // Remove trailing newline
        parts.push(format!("**Implementation steps:**\n{steps}"));
    }

    if !leaf.profiles.is_empty() {
        parts.push(String::new());
        parts.push(format!("**Profiles:** {}", leaf.profiles.join(", ")));
    }

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::progress::TaskStatus;

    #[test]
    fn test_shorten_model_name() {
        // Known full IDs -> short names
        assert_eq!(shorten_model_name("claude-opus-4-6"), "opus");
        assert_eq!(shorten_model_name("claude-sonnet-4-5-20250929"), "sonnet");
        assert_eq!(shorten_model_name("claude-haiku-4-5-20251001"), "haiku");
        // Short names pass through
        assert_eq!(shorten_model_name("opus"), "opus");
        assert_eq!(shorten_model_name("sonnet"), "sonnet");
        assert_eq!(shorten_model_name("haiku"), "haiku");
        // Unknown claude- prefix gets stripped
        assert_eq!(
            shorten_model_name("claude-opus-5-20260101"),
            "opus-5-20260101"
        );
        // Custom model names pass through
        assert_eq!(shorten_model_name("custom-model"), "custom-model");
        assert_eq!(shorten_model_name(""), "");
    }

    #[test]
    fn test_format_task_prompt_minimal() {
        let leaf = LeafTask {
            id: "1.1".to_string(),
            name: "Simple task".to_string(),
            component: "api".to_string(),
            status: TaskStatus::Todo,
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            profiles: Vec::new(),
        };
        let prompt = format_task_prompt(&leaf);
        assert!(prompt.contains("**ID:** 1.1"));
        assert!(prompt.contains("**Component:** api"));
        assert!(prompt.contains("**Name:** Simple task"));
        // Empty optional sections must be omitted
        assert!(!prompt.contains("**Description:**"));
        assert!(!prompt.contains("**Related files:**"));
        assert!(!prompt.contains("**Implementation steps:**"));
        assert!(!prompt.contains("**Profiles:**"));
        // No trailing whitespace
        assert!(!prompt.ends_with('\n'));
        assert!(!prompt.ends_with(' '));
        // Exactly 3 lines: ID, Component, Name
        assert_eq!(prompt.lines().count(), 3);
    }

    #[test]
    fn test_format_task_prompt_full() {
        let leaf = LeafTask {
            id: "2.3.1".to_string(),
            name: "Implement auth endpoints".to_string(),
            component: "api".to_string(),
            status: TaskStatus::Todo,
            deps: Vec::new(),
            model: None,
            description: Some("Create REST endpoints for user authentication".to_string()),
            related_files: vec![
                "src/routes/auth.rs".to_string(),
                "docs/auth-spec.md".to_string(),
            ],
            implementation_steps: vec![
                "Create auth route handler module".to_string(),
                "Implement POST /login endpoint".to_string(),
            ],
            profiles: Vec::new(),
        };
        let prompt = format_task_prompt(&leaf);
        assert!(prompt.contains("**ID:** 2.3.1"));
        assert!(prompt.contains("**Description:**\nCreate REST endpoints"));
        assert!(prompt.contains("- src/routes/auth.rs"));
        assert!(prompt.contains("- docs/auth-spec.md"));
        assert!(prompt.contains("1. Create auth route handler module"));
        assert!(prompt.contains("2. Implement POST /login endpoint"));
        assert!(!prompt.contains("**Profiles:**"));
    }

    #[test]
    fn test_format_task_prompt_unicode_profiles() {
        // Test with Polish characters, emoji, and special characters in profile names
        let leaf = LeafTask {
            id: "3.1".to_string(),
            name: "Unicode profile test".to_string(),
            component: "tests".to_string(),
            status: TaskStatus::Todo,
            deps: Vec::new(),
            model: None,
            description: None,
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            profiles: vec![
                "produkcja".to_string(),      // Polish word: production
                "środowisko-dev".to_string(), // Polish word: environment-dev
                "🚀-deployment".to_string(),  // Rocket emoji + text
                "测试".to_string(),           // Chinese: test
                "café-français".to_string(),  // Accented characters
            ],
        };
        let prompt = format_task_prompt(&leaf);

        // Verify profiles section exists
        assert!(prompt.contains("**Profiles:**"));

        // Verify all profiles are present and properly joined with ", "
        assert!(prompt.contains("produkcja, środowisko-dev, 🚀-deployment, 测试, café-français"));

        // Verify the exact string to ensure join works correctly
        let profiles_line = prompt
            .lines()
            .find(|line| line.starts_with("**Profiles:**"))
            .expect("Should have profiles line");

        assert_eq!(
            profiles_line,
            "**Profiles:** produkcja, środowisko-dev, 🚀-deployment, 测试, café-français"
        );
    }

    #[test]
    fn test_format_task_prompt_very_long_description() {
        // Test with description of 10000+ characters to verify it's not truncated
        // and formatting doesn't break
        let long_description = "a".repeat(10000);
        let leaf = LeafTask {
            id: "4.1".to_string(),
            name: "Task with very long description".to_string(),
            component: "tests".to_string(),
            status: TaskStatus::Todo,
            deps: Vec::new(),
            model: None,
            description: Some(long_description.clone()),
            related_files: Vec::new(),
            implementation_steps: Vec::new(),
            profiles: Vec::new(),
        };
        let prompt = format_task_prompt(&leaf);

        // Verify the description section exists
        assert!(prompt.contains("**Description:**"));

        // Extract the description part from the prompt
        let description_start = prompt
            .find("**Description:**\n")
            .expect("Should have description section");
        let after_marker = description_start + "**Description:**\n".len();
        let description_text = &prompt[after_marker..];

        // Count the actual 'a' characters in the description
        let actual_count = description_text.chars().take_while(|&c| c == 'a').count();

        // Verify we have exactly 10000 characters (not truncated)
        assert_eq!(
            actual_count, 10000,
            "Description should contain all 10000 characters, but found {}",
            actual_count
        );

        // Verify the complete description is present
        assert!(
            description_text.starts_with(&long_description),
            "Description should contain the full 10000-character text"
        );
    }

    #[test]
    fn test_format_task_prompt_special_yaml_chars() {
        // YAML special characters (&, *, !, |, >) must pass through as-is
        // because format_task_prompt returns markdown, not YAML.
        let leaf = LeafTask {
            id: "4.2".to_string(),
            name: "Task & more *special* !chars".to_string(),
            component: "tests".to_string(),
            status: TaskStatus::Todo,
            deps: Vec::new(),
            model: None,
            description: Some(
                "This description contains | pipe and > greater-than symbols\n\
                 Also includes: & ampersand, * asterisk, ! exclamation\n\
                 And YAML anchors/aliases: &anchor *alias"
                    .to_string(),
            ),
            related_files: vec!["config.yml".to_string()],
            implementation_steps: vec![
                "Handle & and * in strings".to_string(),
                "Process | and > operators".to_string(),
            ],
            profiles: Vec::new(),
        };

        let prompt = format_task_prompt(&leaf);

        // All special chars are preserved literally in the output
        assert!(prompt.contains("**Name:** Task & more *special* !chars"));
        assert!(prompt.contains("This description contains | pipe and > greater-than symbols"));
        assert!(prompt.contains("& ampersand, * asterisk, ! exclamation"));
        assert!(prompt.contains("&anchor *alias"));
        assert!(prompt.contains("1. Handle & and * in strings"));
        assert!(prompt.contains("2. Process | and > operators"));

        // No YAML escaping should be applied (output is markdown)
        assert!(!prompt.contains("\\&"));
        assert!(!prompt.contains("\\*"));
        assert!(!prompt.contains("\\!"));

        // Round-trip: output embedded in a YAML literal block must survive parsing
        let indented: String = prompt.lines().map(|l| format!("  {l}\n")).collect();
        let yaml_doc = format!("prompt: |\n{indented}");
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&yaml_doc).expect("output should be embeddable in YAML");
        let recovered = parsed["prompt"].as_str().expect("should be a string");
        // YAML literal block scalar adds a trailing newline
        assert_eq!(recovered.trim_end(), prompt);
    }
}
