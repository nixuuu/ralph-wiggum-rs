use crossterm::style::Stylize;
use std::collections::HashMap;

use super::runner::{ClaudeEvent, ContentBlock, ModelUsageEntry, Usage};
use super::tool_formatting::{colorize_tool_name, format_tool_details, shorten_path};
use crate::shared::icons;
use crate::shared::markdown;

/// Type of the last content block for grouping output
#[derive(Debug, PartialEq, Clone, Copy)]
pub(super) enum BlockType {
    None,
    Text,
    Tool,
}

/// Token tracking state for event formatting.
///
/// This struct aggregates mutable references to token counters to avoid
/// clippy's too_many_arguments warning (was 6+ params in format_event).
///
/// **Finalized tokens**: Accumulated from completed iterations (modelUsage in result events).
/// **Pending tokens**: From current iteration's assistant messages (for live display).
pub(super) struct TokenState<'a> {
    pub finalized_input_tokens: &'a mut u64,
    pub finalized_output_tokens: &'a mut u64,
    pub pending_input_tokens: &'a mut u64,
    pub pending_output_tokens: &'a mut u64,
    pub total_cost_usd: &'a mut f64,
    pub model_costs: &'a mut HashMap<String, f64>,
}

/// Format an assistant message event and return lines to print
fn format_assistant_message(
    message: &super::runner::AssistantMessage,
    last_block_type: &mut BlockType,
    use_nerd_font: bool,
) -> Vec<String> {
    let mut lines = Vec::new();

    for block in &message.content {
        match block {
            ContentBlock::Text { text } => {
                // Add empty line if switching from tool to text
                if *last_block_type == BlockType::Tool {
                    lines.push(String::new());
                }
                *last_block_type = BlockType::Text;

                // Shorten paths, style thinking blocks, render markdown
                let text = shorten_path(text);
                let text = process_thinking_blocks(&text);
                let rendered = markdown::render_markdown(&text);
                for line in rendered.lines() {
                    lines.push(line.to_string());
                }
            }
            ContentBlock::ToolUse { name, input, .. } => {
                // Add empty line if switching from text to tool
                if *last_block_type == BlockType::Text {
                    lines.push(String::new());
                }
                *last_block_type = BlockType::Tool;

                let details = format_tool_details(name, input);
                let tool_icon = icons::tool_icon(name, use_nerd_font);
                let colored_name = colorize_tool_name(name);
                if details.is_empty() {
                    lines.push(format!("  {} {}", tool_icon, colored_name));
                } else {
                    lines.push(format!(
                        "  {} {} {}",
                        tool_icon,
                        colored_name,
                        details.dark_grey()
                    ));
                }
            }
            ContentBlock::Thinking { thinking } => {
                if *last_block_type == BlockType::Tool {
                    lines.push(String::new());
                }
                *last_block_type = BlockType::Text;

                // Render thinking as blockquotes (same as <thinking> tags)
                for line in thinking.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        lines.push("> ".to_string());
                    } else {
                        lines.push(format!("> *{}*", trimmed));
                    }
                }
            }
            ContentBlock::ToolResult { .. } => {
                // Don't print tool results - too verbose
            }
            ContentBlock::Other => {}
        }
    }

    lines
}

/// Format a result event and finalize usage tracking
fn format_result_event(
    cost_usd: &Option<f64>,
    model_usage: &Option<HashMap<String, ModelUsageEntry>>,
    tokens: &mut TokenState<'_>,
) {
    if let Some(mu) = model_usage {
        finalize_model_usage(mu, tokens);
    } else {
        finalize_legacy(*cost_usd, tokens);
    }
}

/// Finalize an iteration's usage from modelUsage (replaces pending tokens)
fn finalize_model_usage(
    model_usage: &HashMap<String, ModelUsageEntry>,
    tokens: &mut TokenState<'_>,
) {
    for (model_name, entry) in model_usage {
        *tokens.finalized_input_tokens += entry.input_tokens;
        *tokens.finalized_output_tokens += entry.output_tokens;
        *tokens.total_cost_usd += entry.cost_usd;
        *tokens.model_costs.entry(model_name.clone()).or_insert(0.0) += entry.cost_usd;
    }
    *tokens.pending_input_tokens = 0;
    *tokens.pending_output_tokens = 0;
}

/// Fallback finalization when modelUsage is absent (backwards compat)
fn finalize_legacy(cost: Option<f64>, tokens: &mut TokenState<'_>) {
    if let Some(c) = cost {
        *tokens.total_cost_usd += c;
    }
    // Promote pending tokens to finalized (don't add result.usage — it would double-count)
    *tokens.finalized_input_tokens += *tokens.pending_input_tokens;
    *tokens.finalized_output_tokens += *tokens.pending_output_tokens;
    *tokens.pending_input_tokens = 0;
    *tokens.pending_output_tokens = 0;
}

/// Add incremental tokens from assistant message (pending, for live display)
pub(super) fn add_pending_usage(
    usage: &Usage,
    pending_input_tokens: &mut u64,
    pending_output_tokens: &mut u64,
) {
    *pending_input_tokens += usage.input_tokens;
    *pending_output_tokens += usage.output_tokens;
}

/// Format a claude event and return lines to print
pub(super) fn format_event(
    event: &ClaudeEvent,
    last_block_type: &mut BlockType,
    use_nerd_font: bool,
    tokens: &mut TokenState<'_>,
) -> Vec<String> {
    match event {
        ClaudeEvent::Assistant { message } => {
            // Add to pending tokens for live display during iteration
            if let Some(u) = &message.usage {
                add_pending_usage(u, tokens.pending_input_tokens, tokens.pending_output_tokens);
            }
            format_assistant_message(message, last_block_type, use_nerd_font)
        }
        ClaudeEvent::Result {
            cost_usd,
            model_usage,
            ..
        } => {
            format_result_event(cost_usd, model_usage, tokens);
            Vec::new()
        }
        ClaudeEvent::System { .. } => {
            // System init messages are informational — no output needed
            Vec::new()
        }
        ClaudeEvent::Other => Vec::new(),
    }
}

/// Convert `<thinking>...</thinking>` blocks to markdown blockquotes with italic.
///
/// Text outside thinking tags is passed through unchanged.
/// Example: `<thinking>\nfoo\nbar\n</thinking>` → `> *foo*\n> *bar*`
fn process_thinking_blocks(text: &str) -> String {
    let open_tag = "<thinking>";
    let close_tag = "</thinking>";

    let mut result = String::with_capacity(text.len());
    let mut search_from = 0;

    while let Some(start) = text[search_from..].find(open_tag) {
        let abs_start = search_from + start;
        // Append text before the tag
        result.push_str(&text[search_from..abs_start]);

        let content_start = abs_start + open_tag.len();
        if let Some(end) = text[content_start..].find(close_tag) {
            let inner = text[content_start..content_start + end].trim_matches('\n');
            // Convert each line to blockquote with italic
            for line in inner.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    result.push_str("> \n");
                } else {
                    result.push_str(&format!("> *{}*\n", trimmed));
                }
            }
            search_from = content_start + end + close_tag.len();
        } else {
            // No closing tag — output remainder as-is
            result.push_str(&text[abs_start..]);
            return result;
        }
    }

    // Append remaining text after last tag
    result.push_str(&text[search_from..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_thinking_no_tags() {
        assert_eq!(process_thinking_blocks("hello world"), "hello world");
    }

    #[test]
    fn test_process_thinking_simple() {
        let input = "<thinking>\nfoo\nbar\n</thinking>";
        let result = process_thinking_blocks(input);
        assert_eq!(result, "> *foo*\n> *bar*\n");
    }

    #[test]
    fn test_process_thinking_with_surrounding_text() {
        let input = "Before\n<thinking>\nthought\n</thinking>\nAfter";
        let result = process_thinking_blocks(input);
        assert_eq!(result, "Before\n> *thought*\n\nAfter");
    }

    #[test]
    fn test_process_thinking_empty_lines() {
        let input = "<thinking>\nfoo\n\nbar\n</thinking>";
        let result = process_thinking_blocks(input);
        assert_eq!(result, "> *foo*\n> \n> *bar*\n");
    }

    #[test]
    fn test_process_thinking_no_closing_tag() {
        let input = "text <thinking>unclosed";
        let result = process_thinking_blocks(input);
        assert_eq!(result, "text <thinking>unclosed");
    }

    #[test]
    fn test_format_assistant_message_renders_markdown() {
        use super::super::runner::{AssistantMessage, ContentBlock};

        let message = AssistantMessage {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "# Heading\n\nSome **bold** text".to_string(),
            }],
            usage: None,
        };

        let mut block_type = BlockType::None;
        let lines = format_assistant_message(&message, &mut block_type, false);

        // Should have multiple lines (markdown formatted)
        assert!(!lines.is_empty());
        // Block type should be Text after processing text content
        assert_eq!(block_type, BlockType::Text);
        // Verify that markdown rendering occurred by checking for ANSI escape codes
        // (termimad adds ANSI codes for styling)
        let output = lines.join("\n");
        assert!(
            output.contains("\x1b[") || output.len() > "# Heading\n\nSome **bold** text".len(),
            "Output should contain ANSI codes or be formatted (got: {:?})",
            output
        );
    }

    #[test]
    fn test_format_event_renders_markdown_for_text_blocks() {
        use super::super::runner::{AssistantMessage, ClaudeEvent, ContentBlock};
        use std::collections::HashMap;

        let event = ClaudeEvent::Assistant {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Plain text with **markdown**".to_string(),
                }],
                usage: None,
            },
        };

        let mut block_type = BlockType::None;
        let mut finalized_input = 0u64;
        let mut finalized_output = 0u64;
        let mut pending_input = 0u64;
        let mut pending_output = 0u64;
        let mut total_cost = 0.0;
        let mut model_costs = HashMap::new();

        let mut tokens = TokenState {
            finalized_input_tokens: &mut finalized_input,
            finalized_output_tokens: &mut finalized_output,
            pending_input_tokens: &mut pending_input,
            pending_output_tokens: &mut pending_output,
            total_cost_usd: &mut total_cost,
            model_costs: &mut model_costs,
        };

        let lines = format_event(&event, &mut block_type, false, &mut tokens);

        // Verify markdown was processed (non-empty output)
        assert!(!lines.is_empty());
        // Verify block type was set
        assert_eq!(block_type, BlockType::Text);
        // Verify markdown rendering by checking output contains ANSI codes or is formatted
        let output = lines.join("\n");
        assert!(
            output.contains("\x1b[") || output != "Plain text with **markdown**",
            "Markdown should be rendered with ANSI formatting"
        );
    }
}
