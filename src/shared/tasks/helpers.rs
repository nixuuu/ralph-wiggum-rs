use crate::shared::tasks::LeafTask;

/// Resolve short model aliases to full model IDs.
pub fn resolve_model_alias(alias: &str) -> String {
    match alias {
        "opus" => "claude-opus-4-6".to_string(),
        "sonnet" => "claude-sonnet-4-5-20250929".to_string(),
        "haiku" => "claude-haiku-4-5-20251001".to_string(),
        other => other.to_string(),
    }
}

/// Reverse-map full model IDs to human-friendly aliases.
pub fn reverse_model_alias(full_id: &str) -> String {
    match full_id {
        "claude-opus-4-6" => "opus".to_string(),
        "claude-sonnet-4-5-20250929" => "sonnet".to_string(),
        "claude-haiku-4-5-20251001" => "haiku".to_string(),
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

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::progress::TaskStatus;

    #[test]
    fn test_resolve_model_alias() {
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("sonnet"), "claude-sonnet-4-5-20250929");
        assert_eq!(resolve_model_alias("haiku"), "claude-haiku-4-5-20251001");
        assert_eq!(resolve_model_alias("claude-opus-4-6"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("custom-model"), "custom-model");
        assert_eq!(resolve_model_alias(""), "");
    }

    #[test]
    fn test_reverse_model_alias() {
        assert_eq!(reverse_model_alias("claude-opus-4-6"), "opus");
        assert_eq!(reverse_model_alias("claude-sonnet-4-5-20250929"), "sonnet");
        assert_eq!(reverse_model_alias("claude-haiku-4-5-20251001"), "haiku");
        assert_eq!(reverse_model_alias("opus"), "opus");
        assert_eq!(reverse_model_alias("custom-model"), "custom-model");
        assert_eq!(reverse_model_alias(""), "");
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
        };
        let prompt = format_task_prompt(&leaf);
        assert!(prompt.contains("**ID:** 1.1"));
        assert!(prompt.contains("**Component:** api"));
        assert!(prompt.contains("**Name:** Simple task"));
        assert!(!prompt.contains("**Description:**"));
        assert!(!prompt.contains("**Related files:**"));
        assert!(!prompt.contains("**Implementation steps:**"));
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
        };
        let prompt = format_task_prompt(&leaf);
        assert!(prompt.contains("**ID:** 2.3.1"));
        assert!(prompt.contains("**Description:**\nCreate REST endpoints"));
        assert!(prompt.contains("- src/routes/auth.rs"));
        assert!(prompt.contains("- docs/auth-spec.md"));
        assert!(prompt.contains("1. Create auth route handler module"));
        assert!(prompt.contains("2. Implement POST /login endpoint"));
    }
}
