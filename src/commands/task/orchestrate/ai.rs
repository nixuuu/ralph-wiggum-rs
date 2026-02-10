#![allow(dead_code)]
use std::collections::HashMap;
use std::path::Path;

use crate::commands::run::runner::ClaudeRunner;
use crate::shared::dag::TaskDag;
use crate::shared::error::{RalphError, Result};
use crate::shared::progress::ProgressFrontmatter;

/// Maximum attempts to generate a valid (acyclic) dependency DAG.
const MAX_CYCLE_RETRIES: u32 = 3;

/// Generate a dependency DAG from PROGRESS.md content using Claude AI.
///
/// Prompts Claude to analyze the task list and return a JSON deps map.
/// If the generated DAG has cycles, retries up to `MAX_CYCLE_RETRIES` times
/// with feedback about the detected cycle.
pub async fn generate_deps(
    progress_content: &str,
    model: Option<&str>,
    cwd: &Path,
) -> Result<ProgressFrontmatter> {
    let base_prompt = format!(
        "Analyze the following PROGRESS.md task list and determine the dependency graph.\n\
         Return ONLY valid JSON in the format: {{\"deps\": {{\"TASK_ID\": [\"DEP_ID\", ...], ...}}}}\n\
         \n\
         Rules:\n\
         - Every task must appear as a key in deps\n\
         - Tasks with no dependencies should have an empty array []\n\
         - Dependencies must reference existing task IDs\n\
         - The graph MUST be acyclic (no circular dependencies)\n\
         \n\
         PROGRESS.md content:\n\
         {progress_content}"
    );

    let mut last_cycle: Option<Vec<String>> = None;

    for attempt in 0..MAX_CYCLE_RETRIES {
        let prompt = if let Some(cycle) = &last_cycle {
            format!(
                "{base_prompt}\n\n\
                 WARNING: Your previous attempt (attempt {attempt}) produced a cycle: {cycle:?}\n\
                 Please fix the cycle and return a valid acyclic dependency graph."
            )
        } else {
            base_prompt.clone()
        };

        let runner = ClaudeRunner::oneshot(
            prompt,
            model.map(|s| s.to_string()),
            Some(cwd.to_path_buf()),
        );

        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let result = runner.run(shutdown, |_| {}, || {}).await;

        let output = match result {
            Ok(Some(text)) => text,
            Ok(None) => {
                return Err(RalphError::Orchestrate(
                    "Claude returned empty response for deps generation".to_string(),
                ));
            }
            Err(e) => return Err(e),
        };

        // Parse JSON from response
        let frontmatter = parse_deps_json(&output)?;

        // Check for cycles
        let dag = TaskDag::from_frontmatter(&frontmatter);
        if let Some(cycle) = dag.detect_cycles() {
            last_cycle = Some(cycle);
            continue;
        }

        return Ok(frontmatter);
    }

    Err(RalphError::DagCycle(last_cycle.unwrap_or_default()))
}

/// Generate a conflict resolution from Claude AI.
///
/// Provides the diff context and task description, and asks Claude
/// to produce the resolved file content.
pub async fn resolve_conflict(
    our_diff: &str,
    their_diff: &str,
    task_desc: &str,
    conflicting_files: &[String],
    model: Option<&str>,
    cwd: &Path,
) -> Result<String> {
    let files_list = conflicting_files.join(", ");
    let prompt = format!(
        "# Merge Conflict Resolution\n\n\
         Resolve the following merge conflict.\n\n\
         ## Task Context\n{task_desc}\n\n\
         ## Conflicting Files\n{files_list}\n\n\
         ## Our Changes (current branch)\n```diff\n{our_diff}\n```\n\n\
         ## Their Changes (worker branch)\n```diff\n{their_diff}\n```\n\n\
         Provide the resolved content for each conflicting file.\n\
         For each file, output the full resolved content between markers:\n\
         --- FILE: <path> ---\n\
         <resolved content>\n\
         --- END FILE ---"
    );

    let runner = ClaudeRunner::oneshot(
        prompt,
        model.map(|s| s.to_string()),
        Some(cwd.to_path_buf()),
    );

    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let result = runner.run(shutdown, |_| {}, || {}).await;

    match result {
        Ok(Some(text)) => Ok(text),
        Ok(None) => Err(RalphError::Orchestrate(
            "Claude returned empty response for conflict resolution".to_string(),
        )),
        Err(e) => Err(e),
    }
}

/// Parse JSON deps response from Claude output.
///
/// Looks for a JSON block containing `{"deps": {...}}` in the output,
/// handling both pure JSON and JSON embedded in markdown code blocks.
pub fn parse_deps_json(output: &str) -> Result<ProgressFrontmatter> {
    // Try to find JSON in the output
    let json_str = extract_json_block(output).unwrap_or(output);

    // Try to parse as full response with "deps" key
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str)
        && let Some(deps_val) = parsed.get("deps")
    {
        let deps: HashMap<String, Vec<String>> = serde_json::from_value(deps_val.clone())
            .map_err(|e| RalphError::Orchestrate(format!("Failed to parse deps JSON: {e}")))?;
        return Ok(ProgressFrontmatter {
            deps,
            models: HashMap::new(),
            default_model: None,
        });
    }

    // Try direct parse as HashMap
    if let Ok(deps) = serde_json::from_str::<HashMap<String, Vec<String>>>(json_str) {
        return Ok(ProgressFrontmatter {
            deps,
            models: HashMap::new(),
            default_model: None,
        });
    }

    Err(RalphError::Orchestrate(format!(
        "Could not parse deps JSON from Claude response: {output}"
    )))
}

/// Extract a JSON block from text that may contain markdown code fences.
fn extract_json_block(text: &str) -> Option<&str> {
    // Look for ```json ... ``` block
    if let Some(start) = text.find("```json") {
        let content_start = start + "```json".len();
        if let Some(end) = text[content_start..].find("```") {
            return Some(text[content_start..content_start + end].trim());
        }
    }
    // Look for ``` ... ``` block (generic code block)
    if let Some(start) = text.find("```\n") {
        let content_start = start + "```\n".len();
        if let Some(end) = text[content_start..].find("```") {
            let block = text[content_start..content_start + end].trim();
            if block.starts_with('{') {
                return Some(block);
            }
        }
    }
    // Look for bare JSON object
    let trimmed = text.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_deps_json_simple() {
        let json = r#"{"deps": {"T01": [], "T02": ["T01"], "T03": ["T01", "T02"]}}"#;
        let fm = parse_deps_json(json).unwrap();
        assert_eq!(fm.deps["T01"], Vec::<String>::new());
        assert_eq!(fm.deps["T02"], vec!["T01"]);
        assert_eq!(fm.deps["T03"], vec!["T01", "T02"]);
    }

    #[test]
    fn test_parse_deps_json_in_code_block() {
        let response = "Here's the dependency graph:\n\n```json\n{\"deps\": {\"T01\": [], \"T02\": [\"T01\"]}}\n```\n\nDone.";
        let fm = parse_deps_json(response).unwrap();
        assert!(fm.deps.contains_key("T01"));
        assert_eq!(fm.deps["T02"], vec!["T01"]);
    }

    #[test]
    fn test_parse_deps_json_bare_object() {
        let json = r#"{"deps": {"A": ["B"], "B": []}}"#;
        let fm = parse_deps_json(json).unwrap();
        assert_eq!(fm.deps["A"], vec!["B"]);
    }

    #[test]
    fn test_parse_deps_json_invalid() {
        let result = parse_deps_json("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_json_block_code_fence() {
        let text = "text\n```json\n{\"a\": 1}\n```\nmore";
        assert_eq!(extract_json_block(text), Some("{\"a\": 1}"));
    }

    #[test]
    fn test_extract_json_block_generic_fence() {
        let text = "text\n```\n{\"a\": 1}\n```\nmore";
        assert_eq!(extract_json_block(text), Some("{\"a\": 1}"));
    }

    #[test]
    fn test_extract_json_block_bare() {
        let text = r#"  {"deps": {}}  "#;
        assert_eq!(extract_json_block(text), Some(r#"{"deps": {}}"#));
    }

    #[test]
    fn test_extract_json_block_none() {
        assert_eq!(extract_json_block("just plain text"), None);
    }

    #[test]
    fn test_parse_deps_preserves_empty_arrays() {
        let json = r#"{"deps": {"T01": [], "T02": [], "T03": ["T01"]}}"#;
        let fm = parse_deps_json(json).unwrap();
        assert!(fm.deps["T01"].is_empty());
        assert!(fm.deps["T02"].is_empty());
        assert_eq!(fm.deps.len(), 3);
    }
}
