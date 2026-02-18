//! Formatowanie eventów Claude CLI jako ratatui Line/Span.
//!
//! Zamiast `Vec<String>` z kodami ANSI (crossterm Stylize), buduje
//! `Vec<Line<'static>>` z ratatui Style/Span, wykorzystując kolory z Theme.

use std::collections::HashMap;
use std::fmt::Write;

use ansi_to_tui::IntoText;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::runner::{AssistantMessage, ClaudeEvent, ContentBlock, ModelUsageEntry, Usage};
use crate::shared::markdown;
use crate::tui::formatter::{border_span, label_line, prepend_border, separator_line, timing_line};
use crate::tui::theme::DEFAULT_THEME;
use crate::tui::tool_formatting::{format_tool_details, prettify_tool_name, shorten_path};

/// Typ ostatniego bloku treści — do grupowania wyjścia (separatory między blokami)
#[derive(Debug, PartialEq, Clone, Copy)]
pub(super) enum BlockType {
    None,
    Text,
    Tool,
    Thinking,
    User,
}

/// Stan śledzenia tokenów dla formatowania eventów.
///
/// Agreguje mutowalne referencje do liczników tokenów.
/// **Finalized tokens**: Z zakończonych iteracji (modelUsage w result events).
/// **Pending tokens**: Z bieżącej iteracji (live display).
pub(super) struct TokenState<'a> {
    pub finalized_input_tokens: &'a mut u64,
    pub finalized_output_tokens: &'a mut u64,
    pub pending_input_tokens: &'a mut u64,
    pub pending_output_tokens: &'a mut u64,
    pub total_cost_usd: &'a mut f64,
    pub model_costs: &'a mut HashMap<String, f64>,
}

// ---------------------------------------------------------------------------
// ANSI → ratatui conversion
// ---------------------------------------------------------------------------

/// Konwertuje tekst z kodami ANSI na `Vec<Line<'static>>` ratatui.
///
/// Markdown renderer (termimad) zwraca tekst z kodami ANSI — ta funkcja
/// parsuje escape sequences na styled spany ratatui.
/// Fallback: gdy parsowanie ANSI zawiedzie, traktuje tekst jako raw lines.
fn ansi_to_lines(text: &str) -> Vec<Line<'static>> {
    match text.into_text() {
        Ok(parsed) => parsed.lines.into_iter().collect(),
        Err(_) => text.lines().map(|l| Line::raw(l.to_string())).collect(),
    }
}

// ---------------------------------------------------------------------------
// Formatowanie assistant message
// ---------------------------------------------------------------------------

/// Formatuje wiadomość asystenta na linie ratatui z bordered blocks.
///
/// Styl opencode-inspired:
/// - Tekst: `█ ` border w Cyan + label "Claude" + markdown content
/// - Tool: `█ ` border w DarkGray + `Name: params`
/// - Thinking: `█ ` border w DarkGray + italic text
/// - Separator (pusta linia) między różnymi typami bloków
fn format_assistant_message(
    message: &AssistantMessage,
    last_block_type: &mut BlockType,
    tool_use_names: &mut HashMap<String, String>,
    _use_nerd_font: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let primary = DEFAULT_THEME.primary;
    let muted = DEFAULT_THEME.muted;

    for block in &message.content {
        match block {
            ContentBlock::Text { text } => {
                // Separator przed blokiem (jeśli nie pierwszy)
                if *last_block_type != BlockType::None {
                    lines.push(separator_line());
                }

                // Label "Claude" (tylko gdy poprzedni blok nie był Text)
                if *last_block_type != BlockType::Text {
                    lines.push(label_line("Claude", primary));
                }
                *last_block_type = BlockType::Text;

                // Shorten paths, process thinking tags, render markdown
                let text = shorten_path(text);
                let text = process_thinking_blocks(&text);
                let rendered = markdown::render_markdown(&text);
                // Markdown renderer zwraca ANSI-styled text — parsujemy na ratatui Spans
                let content_lines = ansi_to_lines(&rendered);
                lines.extend(prepend_border(content_lines, primary));
            }
            ContentBlock::ToolUse { name, id, input } => {
                // Śledzenie tool_use_id → name dla wyświetlania ask_user results
                if let Some(id) = id {
                    tool_use_names.insert(id.clone(), name.clone());
                }

                let is_ask_user = name == "ask_user" || name.ends_with("__ask_user");

                if is_ask_user {
                    // ask_user → wyświetl pytanie jako blok Claude
                    if let Some(question_text) = extract_ask_user_questions(input) {
                        if *last_block_type != BlockType::None {
                            lines.push(separator_line());
                        }
                        if *last_block_type != BlockType::Text {
                            lines.push(label_line("Claude", primary));
                        }
                        *last_block_type = BlockType::Text;
                        let rendered = markdown::render_markdown(&question_text);
                        lines.extend(prepend_border(ansi_to_lines(&rendered), primary));
                    }
                } else {
                    // Separator przed blokiem (jeśli nie pierwszy tool w sekwencji)
                    if *last_block_type != BlockType::None && *last_block_type != BlockType::Tool {
                        lines.push(separator_line());
                    }
                    *last_block_type = BlockType::Tool;

                    // Opencode style: `▍Name: params` (bez ikony, border w muted)
                    let detail_spans = format_tool_details(name, input);
                    let display_name = prettify_tool_name(name);

                    let mut spans = vec![
                        border_span(muted),
                        Span::styled(display_name, Style::default().add_modifier(Modifier::BOLD)),
                    ];

                    if !detail_spans.is_empty() {
                        spans.push(Span::raw(": "));
                        spans.extend(detail_spans);
                    }

                    lines.push(Line::from(spans));
                }
            }
            ContentBlock::Thinking { thinking } => {
                // Separator przed blokiem
                if *last_block_type != BlockType::None {
                    lines.push(separator_line());
                }
                *last_block_type = BlockType::Thinking;

                // Thinking → bordered block z italic (muted border + italic text)
                let italic_style = Style::default().fg(muted).add_modifier(Modifier::ITALIC);

                for line in thinking.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        lines.push(Line::from(border_span(muted)));
                    } else {
                        lines.push(Line::from(vec![
                            border_span(muted),
                            Span::styled(trimmed.to_string(), italic_style),
                        ]));
                    }
                }
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
            } => {
                // Wyświetlaj tylko odpowiedzi ask_user
                let is_ask_user = tool_use_id
                    .as_deref()
                    .and_then(|id| tool_use_names.get(id))
                    .is_some_and(|n: &String| n == "ask_user" || n.ends_with("__ask_user"));

                if is_ask_user && let Some(text) = extract_tool_result_text(content) {
                    let answer = extract_answer_only(&text);
                    if !answer.is_empty() {
                        // Separator + label "You"
                        if *last_block_type != BlockType::None {
                            lines.push(separator_line());
                        }
                        *last_block_type = BlockType::User;

                        let secondary = DEFAULT_THEME.secondary;
                        lines.push(label_line("You", secondary));

                        let rendered = markdown::render_markdown(&answer);
                        let content_lines = ansi_to_lines(&rendered);
                        lines.extend(prepend_border(content_lines, secondary));
                    }
                }
            }
            ContentBlock::Other => {}
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// Formatowanie user message
// ---------------------------------------------------------------------------

/// Formatuje wiadomość użytkownika — tylko ToolResult (ask_user).
///
/// User events zawierają też Text blocks (prompt), ale pomijamy je
/// żeby nie zaśmiecać outputu. Styl: bordered block z labelem "You" w secondary color.
fn format_user_message(
    message: &AssistantMessage,
    last_block_type: &mut BlockType,
    tool_use_names: &mut HashMap<String, String>,
    _use_nerd_font: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let secondary = DEFAULT_THEME.secondary;

    for block in &message.content {
        if let ContentBlock::ToolResult {
            tool_use_id,
            content,
        } = block
        {
            let is_ask_user = tool_use_id
                .as_deref()
                .and_then(|id| tool_use_names.get(id))
                .is_some_and(|n: &String| n == "ask_user" || n.ends_with("__ask_user"));

            if is_ask_user && let Some(text) = extract_tool_result_text(content) {
                let answer = extract_answer_only(&text);
                if !answer.is_empty() {
                    // Separator + label "You"
                    if *last_block_type != BlockType::None {
                        lines.push(separator_line());
                    }
                    *last_block_type = BlockType::User;

                    lines.push(label_line("You", secondary));

                    let rendered = markdown::render_markdown(&answer);
                    let content_lines = ansi_to_lines(&rendered);
                    lines.extend(prepend_border(content_lines, secondary));
                }
            }
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// Ekstrakcja tekstu z ToolResult
// ---------------------------------------------------------------------------

/// Wyciąga pytania z inputu ToolUse ask_user.
///
/// Obsługuje dwa formaty:
/// - simple: `{"question": "text"}`
/// - full: `{"questions": [{"question": "Q1"}, {"question": "Q2"}]}`
fn extract_ask_user_questions(input: &serde_json::Value) -> Option<String> {
    // Simple format: {"question": "text"}
    if let Some(q) = input.get("question").and_then(|v| v.as_str()) {
        return Some(q.to_string());
    }
    // Full format: {"questions": [{"question": "text"}, ...]}
    if let Some(arr) = input.get("questions").and_then(|v| v.as_array()) {
        let qs: Vec<&str> = arr
            .iter()
            .filter_map(|q| q.get("question").and_then(|v| v.as_str()))
            .collect();
        if !qs.is_empty() {
            return Some(qs.join("\n"));
        }
    }
    None
}

/// Wyciąga samą odpowiedź z markdown ToolResult (bez pytań).
///
/// Format z `build_answer_markdown()`: `"**question**\nanswer\n\n**question2**\nanswer2"`.
/// Pytanie może być multilinijkowe (np. choice z opcjami) — `**` otwiera się
/// w pierwszej linii a zamyka `**\n` w ostatniej linii pytania.
fn extract_answer_only(text: &str) -> String {
    let mut answers = Vec::new();
    let mut remaining = text;

    loop {
        remaining = remaining.trim_start_matches('\n');
        if remaining.is_empty() {
            break;
        }

        if remaining.starts_with("**") {
            // Blok pytania — szukaj zamknięcia **\n
            if let Some(pos) = remaining[2..].find("**\n") {
                remaining = &remaining[2 + pos + 3..];
            } else if remaining[2..].ends_with("**") {
                break; // pytanie na końcu, brak odpowiedzi
            } else {
                // Brak zamknięcia — traktuj jako zwykły tekst
                answers.push(remaining.trim().to_string());
                break;
            }
        } else {
            // Blok odpowiedzi — szukaj następnego pytania
            if let Some(pos) = remaining.find("\n\n**") {
                let answer = remaining[..pos].trim();
                if !answer.is_empty() {
                    answers.push(answer.to_string());
                }
                remaining = &remaining[pos + 2..]; // skip \n\n
            } else {
                let answer = remaining.trim();
                if !answer.is_empty() {
                    answers.push(answer.to_string());
                }
                break;
            }
        }
    }

    answers.join("\n\n")
}

/// Wyciąga tekst z ToolResult content.
///
/// Claude CLI wysyła content jako:
/// - plain string
/// - array `[{type: "text", text: "..."}]`
fn extract_tool_result_text(content: &serde_json::Value) -> Option<String> {
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }

    if let Some(arr) = content.as_array() {
        let texts: Vec<&str> = arr
            .iter()
            .filter_map(|block| {
                if block.get("type")?.as_str()? == "text" {
                    block.get("text")?.as_str()
                } else {
                    None
                }
            })
            .collect();

        if !texts.is_empty() {
            return Some(texts.join("\n"));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Token tracking (finalizacja usage)
// ---------------------------------------------------------------------------

/// Formatuje result event i finalizuje usage tracking
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

/// Finalizuje usage z modelUsage (zastępuje pending tokens)
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

/// Fallback finalizacji gdy brak modelUsage (backwards compat)
fn finalize_legacy(cost: Option<f64>, tokens: &mut TokenState<'_>) {
    if let Some(c) = cost {
        *tokens.total_cost_usd += c;
    }
    *tokens.finalized_input_tokens += *tokens.pending_input_tokens;
    *tokens.finalized_output_tokens += *tokens.pending_output_tokens;
    *tokens.pending_input_tokens = 0;
    *tokens.pending_output_tokens = 0;
}

/// Dodaje inkrementalne tokeny z assistant message (pending, dla live display)
pub(super) fn add_pending_usage(
    usage: &Usage,
    pending_input_tokens: &mut u64,
    pending_output_tokens: &mut u64,
) {
    *pending_input_tokens += usage.input_tokens;
    *pending_output_tokens += usage.output_tokens;
}

// ---------------------------------------------------------------------------
// Główna funkcja formatowania
// ---------------------------------------------------------------------------

/// Formatuje event Claude CLI i zwraca `Vec<Line<'static>>` (ratatui).
///
/// Zastępuje wcześniejsze `Vec<String>` z kodami ANSI (crossterm Stylize)
/// na natywne typy ratatui z użyciem Theme kolorów.
pub(super) fn format_event(
    event: &ClaudeEvent,
    last_block_type: &mut BlockType,
    tool_use_names: &mut HashMap<String, String>,
    use_nerd_font: bool,
    tokens: &mut TokenState<'_>,
) -> Vec<Line<'static>> {
    match event {
        ClaudeEvent::Assistant { message } => {
            // Dodaj do pending tokens dla live display
            if let Some(u) = &message.usage {
                add_pending_usage(u, tokens.pending_input_tokens, tokens.pending_output_tokens);
            }
            format_assistant_message(message, last_block_type, tool_use_names, use_nerd_font)
        }
        ClaudeEvent::Result {
            cost_usd,
            model_usage,
            ..
        } => {
            // Timing footer przed finalizacją tokenów
            let input = *tokens.pending_input_tokens + *tokens.finalized_input_tokens;
            let output = *tokens.pending_output_tokens + *tokens.finalized_output_tokens;

            format_result_event(cost_usd, model_usage, tokens);

            // Emit timing line jeśli mamy jakiekolwiek tokeny
            if input > 0 || output > 0 {
                vec![timing_line(DEFAULT_THEME.primary, input, output)]
            } else {
                Vec::new()
            }
        }
        ClaudeEvent::User { message } => {
            format_user_message(message, last_block_type, tool_use_names, use_nerd_font)
        }
        ClaudeEvent::System { .. } => Vec::new(),
        ClaudeEvent::Other => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Thinking blocks processing
// ---------------------------------------------------------------------------

/// Konwertuje `<thinking>...</thinking>` na markdown blockquotes z italic.
///
/// Tekst poza tagami thinking jest przekazywany bez zmian.
/// Przykład: `<thinking>\nfoo\nbar\n</thinking>` → `> *foo*\n> *bar*`
fn process_thinking_blocks(text: &str) -> String {
    let open_tag = "<thinking>";
    let close_tag = "</thinking>";

    let mut result = String::with_capacity(text.len());
    let mut search_from = 0;

    while let Some(start) = text[search_from..].find(open_tag) {
        let abs_start = search_from + start;
        result.push_str(&text[search_from..abs_start]);

        let content_start = abs_start + open_tag.len();
        if let Some(end) = text[content_start..].find(close_tag) {
            let inner = text[content_start..content_start + end].trim_matches('\n');
            for line in inner.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    result.push_str("> \n");
                } else {
                    writeln!(result, "> *{}*", trimmed).unwrap();
                }
            }
            search_from = content_start + end + close_tag.len();
        } else {
            // Brak zamykającego tagu — reszta as-is
            result.push_str(&text[abs_start..]);
            return result;
        }
    }

    result.push_str(&text[search_from..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Testy process_thinking_blocks --

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

    // -- Testy extract_tool_result_text --

    #[test]
    fn test_extract_tool_result_text_string() {
        let content = serde_json::json!("Hello");
        assert_eq!(
            extract_tool_result_text(&content),
            Some("Hello".to_string())
        );
    }

    #[test]
    fn test_extract_tool_result_text_array() {
        let content =
            serde_json::json!([{"type": "text", "text": "A"}, {"type": "text", "text": "B"}]);
        assert_eq!(extract_tool_result_text(&content), Some("A\nB".to_string()));
    }

    #[test]
    fn test_extract_tool_result_text_null() {
        assert_eq!(extract_tool_result_text(&serde_json::json!(null)), None);
    }

    #[test]
    fn test_extract_tool_result_text_empty_array() {
        assert_eq!(extract_tool_result_text(&serde_json::json!([])), None);
    }

    // -- Testy extract_ask_user_questions --

    #[test]
    fn test_extract_ask_user_questions_simple() {
        let input = serde_json::json!({"question": "How are you?"});
        assert_eq!(
            extract_ask_user_questions(&input),
            Some("How are you?".to_string())
        );
    }

    #[test]
    fn test_extract_ask_user_questions_full() {
        let input = serde_json::json!({
            "questions": [
                {"question": "Q1?", "type": "text"},
                {"question": "Q2?", "type": "choice"}
            ]
        });
        assert_eq!(
            extract_ask_user_questions(&input),
            Some("Q1?\nQ2?".to_string())
        );
    }

    #[test]
    fn test_extract_ask_user_questions_empty() {
        let input = serde_json::json!({"command": "run"});
        assert_eq!(extract_ask_user_questions(&input), None);
    }

    #[test]
    fn test_extract_ask_user_questions_empty_array() {
        let input = serde_json::json!({"questions": []});
        assert_eq!(extract_ask_user_questions(&input), None);
    }

    // -- Testy extract_answer_only --

    #[test]
    fn test_extract_answer_only_single() {
        assert_eq!(extract_answer_only("**How are you?**\nGood"), "Good");
    }

    #[test]
    fn test_extract_answer_only_multiple() {
        let text = "**Q1?**\nAnswer1\n\n**Q2?**\nAnswer2";
        assert_eq!(extract_answer_only(text), "Answer1\n\nAnswer2");
    }

    #[test]
    fn test_extract_answer_only_no_questions() {
        assert_eq!(extract_answer_only("Just an answer"), "Just an answer");
    }

    #[test]
    fn test_extract_answer_only_only_question() {
        assert_eq!(extract_answer_only("**Question?**"), "");
    }

    #[test]
    fn test_extract_answer_only_multiline_question() {
        let text = "**Q line1\nline2\nline3?**\nAnswer";
        assert_eq!(extract_answer_only(text), "Answer");
    }

    #[test]
    fn test_extract_answer_only_multiline_with_blank_lines() {
        let text =
            "**Cześć! Co chcesz?\n\n1. Opcja A\n2. Opcja B\n\nCo Cię interesuje?**\nOdpowiedź";
        assert_eq!(extract_answer_only(text), "Odpowiedź");
    }

    // -- Testy format_assistant_message --

    #[test]
    fn test_format_assistant_message_renders_text() {
        let message = AssistantMessage {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "# Heading\n\nSome **bold** text".to_string(),
            }],
            usage: None,
        };

        let mut block_type = BlockType::None;
        let mut tool_names = HashMap::new();
        let lines = format_assistant_message(&message, &mut block_type, &mut tool_names, false);

        assert!(!lines.is_empty());
        assert_eq!(block_type, BlockType::Text);
    }

    #[test]
    fn test_format_assistant_message_renders_tool_use() {
        let message = AssistantMessage {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                name: "Read".to_string(),
                id: Some("toolu_01".to_string()),
                input: serde_json::json!({"file_path": "/tmp/test.rs"}),
            }],
            usage: None,
        };

        let mut block_type = BlockType::None;
        let mut tool_names = HashMap::new();
        let lines = format_assistant_message(&message, &mut block_type, &mut tool_names, false);

        assert!(!lines.is_empty());
        assert_eq!(block_type, BlockType::Tool);
        // Sprawdź śledzenie tool_use_id
        assert_eq!(tool_names.get("toolu_01"), Some(&"Read".to_string()));

        // Sprawdź że linia zawiera border span + bold tool name
        let line = &lines[0];
        // First span is border "█ "
        assert_eq!(line.spans[0].content, "▍");
        // Second span is bold tool name "Read"
        let tool_span = line.spans.iter().find(|s| s.content.contains("Read"));
        assert!(tool_span.is_some(), "Powinna zawierać 'Read' span");
        assert!(
            tool_span
                .unwrap()
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn test_format_assistant_message_renders_ask_user_as_claude_block() {
        let message = AssistantMessage {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                name: "ask_user".to_string(),
                id: Some("toolu_ask".to_string()),
                input: serde_json::json!({"question": "How are you?"}),
            }],
            usage: None,
        };

        let mut block_type = BlockType::None;
        let mut tool_names = HashMap::new();
        let lines = format_assistant_message(&message, &mut block_type, &mut tool_names, false);

        assert!(!lines.is_empty());
        // ask_user ToolUse → renders as Text (Claude block), not Tool
        assert_eq!(block_type, BlockType::Text);
        // Powinno śledzić tool_use_id
        assert_eq!(tool_names.get("toolu_ask"), Some(&"ask_user".to_string()));
        // Powinna zawierać label "Claude"
        let has_claude = lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains("Claude")));
        assert!(has_claude, "ask_user ToolUse powinno mieć label Claude");
    }

    #[test]
    fn test_format_assistant_message_renders_mcp_ask_user_as_claude_block() {
        let message = AssistantMessage {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                name: "mcp__ralph__ask_user".to_string(),
                id: Some("toolu_mcp_ask".to_string()),
                input: serde_json::json!({
                    "questions": [{"question": "Pick one", "type": "choice"}]
                }),
            }],
            usage: None,
        };

        let mut block_type = BlockType::None;
        let mut tool_names = HashMap::new();
        let lines = format_assistant_message(&message, &mut block_type, &mut tool_names, false);

        assert!(!lines.is_empty());
        assert_eq!(block_type, BlockType::Text);
    }

    #[test]
    fn test_format_assistant_message_renders_thinking() {
        let message = AssistantMessage {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Thinking {
                thinking: "Let me think".to_string(),
            }],
            usage: None,
        };

        let mut block_type = BlockType::None;
        let mut tool_names = HashMap::new();
        let lines = format_assistant_message(&message, &mut block_type, &mut tool_names, false);

        assert!(!lines.is_empty());
        assert_eq!(block_type, BlockType::Thinking);

        // First span is border "▍", second is italic thinking text
        assert_eq!(lines[0].spans[0].content, "▍");
        let thinking_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("Let me think"));
        assert!(thinking_span.is_some());
        assert!(
            thinking_span
                .unwrap()
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
    }

    // -- Testy format_event --

    #[test]
    fn test_format_event_assistant_text() {
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

        let mut tool_names = HashMap::new();
        let lines = format_event(&event, &mut block_type, &mut tool_names, false, &mut tokens);

        assert!(!lines.is_empty());
        assert_eq!(block_type, BlockType::Text);
    }

    #[test]
    fn test_format_user_message_renders_ask_user_tool_result() {
        let mut tool_names = HashMap::new();
        tool_names.insert("toolu_01".to_string(), "ask_user".to_string());

        let message = AssistantMessage {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: Some("toolu_01".to_string()),
                content: serde_json::json!("User's answer"),
            }],
            usage: None,
        };

        let mut block_type = BlockType::None;
        let lines = format_user_message(&message, &mut block_type, &mut tool_names, false);

        assert!(!lines.is_empty(), "Powinno renderować ask_user ToolResult");
        assert_eq!(block_type, BlockType::User);

        // Sprawdź label "You" z bordered block
        let has_you = lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains("You")));
        assert!(has_you, "Powinno zawierać label 'You'");

        // Sprawdź border na content lines
        let has_border = lines
            .iter()
            .any(|l| l.spans.first().is_some_and(|s| s.content == "▍"));
        assert!(has_border, "Powinno mieć border '▍'");
    }

    #[test]
    fn test_format_user_message_skips_text_blocks() {
        let message = AssistantMessage {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "User prompt text".to_string(),
            }],
            usage: None,
        };

        let mut block_type = BlockType::None;
        let mut tool_names = HashMap::new();
        let lines = format_user_message(&message, &mut block_type, &mut tool_names, false);

        assert!(lines.is_empty(), "Text blocks użytkownika pomijane");
    }

    #[test]
    fn test_format_user_message_skips_non_ask_user_tool_result() {
        let mut tool_names = HashMap::new();
        tool_names.insert("toolu_01".to_string(), "Read".to_string());

        let message = AssistantMessage {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: Some("toolu_01".to_string()),
                content: serde_json::json!("file contents"),
            }],
            usage: None,
        };

        let mut block_type = BlockType::None;
        let lines = format_user_message(&message, &mut block_type, &mut tool_names, false);

        assert!(lines.is_empty(), "Non-ask_user ToolResults pomijane");
    }

    #[test]
    fn test_format_event_handles_user_event() {
        let mut tool_names = HashMap::new();
        tool_names.insert("toolu_99".to_string(), "mcp__ralph__ask_user".to_string());

        let event = ClaudeEvent::User {
            message: AssistantMessage {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: Some("toolu_99".to_string()),
                    content: serde_json::json!("**How are you?**\nI'm fine!"),
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

        let lines = format_event(&event, &mut block_type, &mut tool_names, false, &mut tokens);

        assert!(!lines.is_empty(), "User event z ask_user generuje output");
        assert_eq!(pending_input, 0);
        assert_eq!(pending_output, 0);
    }

    #[test]
    fn test_format_event_result_finalizes_tokens() {
        let mut block_type = BlockType::None;
        let mut finalized_input = 0u64;
        let mut finalized_output = 0u64;
        let mut pending_input = 100u64;
        let mut pending_output = 50u64;
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

        let event = ClaudeEvent::Result {
            subtype: None,
            cost_usd: Some(0.01),
            duration_ms: None,
            duration_api_ms: None,
            usage: None,
            model_usage: None,
        };

        let mut tool_names = HashMap::new();
        let lines = format_event(&event, &mut block_type, &mut tool_names, false, &mut tokens);

        // Result event now emits timing line when tokens > 0
        assert_eq!(lines.len(), 1, "Result event generuje timing line");
        assert_eq!(finalized_input, 100, "Pending → finalized");
        assert_eq!(finalized_output, 50);
        assert_eq!(pending_input, 0, "Pending wyzerowane");
        assert_eq!(pending_output, 0);
        assert!((total_cost - 0.01).abs() < f64::EPSILON);
    }

    #[test]
    fn test_format_event_result_with_model_usage() {
        let mut block_type = BlockType::None;
        let mut finalized_input = 0u64;
        let mut finalized_output = 0u64;
        let mut pending_input = 50u64;
        let mut pending_output = 25u64;
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

        let mut mu = HashMap::new();
        mu.insert(
            "claude-sonnet".to_string(),
            ModelUsageEntry {
                input_tokens: 200,
                output_tokens: 100,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
                cost_usd: 0.05,
            },
        );

        let event = ClaudeEvent::Result {
            subtype: None,
            cost_usd: None,
            duration_ms: None,
            duration_api_ms: None,
            usage: None,
            model_usage: Some(mu),
        };

        let mut tool_names = HashMap::new();
        format_event(&event, &mut block_type, &mut tool_names, false, &mut tokens);

        assert_eq!(finalized_input, 200);
        assert_eq!(finalized_output, 100);
        assert_eq!(pending_input, 0);
        assert_eq!(pending_output, 0);
        assert!((total_cost - 0.05).abs() < f64::EPSILON);
        assert_eq!(model_costs.get("claude-sonnet"), Some(&0.05));
    }

    #[test]
    fn test_block_type_transitions_add_separators() {
        let message = AssistantMessage {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::ToolUse {
                    name: "Read".to_string(),
                    id: None,
                    input: serde_json::json!({}),
                },
                ContentBlock::Text {
                    text: "Result".to_string(),
                },
            ],
            usage: None,
        };

        let mut block_type = BlockType::None;
        let mut tool_names = HashMap::new();
        let lines = format_assistant_message(&message, &mut block_type, &mut tool_names, false);

        // Powinna być pusta linia między tool a text
        let has_empty = lines
            .iter()
            .any(|l| l.spans.is_empty() || (l.spans.len() == 1 && l.spans[0].content.is_empty()));
        assert!(has_empty, "Separator między tool/text");
    }

    #[test]
    fn test_format_event_system_returns_empty() {
        let event = ClaudeEvent::System {
            subtype: None,
            session_id: None,
            model: None,
            tools: None,
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

        let mut tool_names = HashMap::new();
        let lines = format_event(&event, &mut block_type, &mut tool_names, false, &mut tokens);

        assert!(lines.is_empty());
    }

    #[test]
    fn test_format_event_other_returns_empty() {
        let event = ClaudeEvent::Other;

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

        let mut tool_names = HashMap::new();
        let lines = format_event(&event, &mut block_type, &mut tool_names, false, &mut tokens);

        assert!(lines.is_empty());
    }

    // ============ Snapshot tests ============

    /// Helper: konwertuje Vec<Line> na debug representation z kolorami i modyfikatorami.
    ///
    /// Format spanu: `[Color,MODIFIER]content` — np. `[DarkGray,ITALIC]*thinking*`
    /// Bez stylu: `content` (raw text).
    fn lines_to_debug(lines: &[Line<'_>]) -> String {
        let mut output = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if line.spans.is_empty() {
                output.push(format!("Line {}: [empty]", i));
            } else {
                let spans_repr: Vec<String> = line
                    .spans
                    .iter()
                    .map(|s| {
                        let mut tags = Vec::new();
                        if let Some(fg) = s.style.fg {
                            tags.push(format!("{:?}", fg));
                        }
                        if s.style
                            .add_modifier
                            .contains(ratatui::style::Modifier::BOLD)
                        {
                            tags.push("BOLD".to_string());
                        }
                        if s.style
                            .add_modifier
                            .contains(ratatui::style::Modifier::ITALIC)
                        {
                            tags.push("ITALIC".to_string());
                        }
                        if tags.is_empty() {
                            s.content.to_string()
                        } else {
                            format!("[{}]{}", tags.join(","), s.content)
                        }
                    })
                    .collect();
                output.push(format!("Line {}: {}", i, spans_repr.join("")));
            }
        }
        output.join("\n")
    }

    /// Helper: formatuje event i zwraca debug representation linii
    fn format_event_for_snapshot(event: &ClaudeEvent) -> String {
        format_event_for_snapshot_with(event, &mut HashMap::new())
    }

    /// Helper z pre-loaded tool_use_names (dla testów user events z ask_user)
    fn format_event_for_snapshot_with(
        event: &ClaudeEvent,
        tool_names: &mut HashMap<String, String>,
    ) -> String {
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

        let lines = format_event(event, &mut block_type, tool_names, false, &mut tokens);
        lines_to_debug(&lines)
    }

    #[test]
    fn test_snapshot_format_event_assistant_text_plain() {
        let event = ClaudeEvent::Assistant {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Hello, this is a plain text response.".to_string(),
                }],
                usage: None,
            },
        };

        let output = format_event_for_snapshot(&event);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_snapshot_format_event_assistant_text_markdown() {
        let event = ClaudeEvent::Assistant {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "# Heading\n\nSome **bold** and *italic* text.\n\n- List item 1\n- List item 2".to_string(),
                }],
                usage: None,
            },
        };

        let output = format_event_for_snapshot(&event);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_snapshot_format_event_assistant_thinking() {
        let event = ClaudeEvent::Assistant {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Thinking {
                    thinking: "Let me think about this.\nI need to consider multiple factors."
                        .to_string(),
                }],
                usage: None,
            },
        };

        let output = format_event_for_snapshot(&event);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_snapshot_format_event_tool_use_read() {
        let event = ClaudeEvent::Assistant {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    name: "Read".to_string(),
                    id: Some("toolu_01".to_string()),
                    input: serde_json::json!({
                        "file_path": "~/project/src/main.rs"
                    }),
                }],
                usage: None,
            },
        };

        let output = format_event_for_snapshot(&event);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_snapshot_format_event_tool_use_write() {
        let event = ClaudeEvent::Assistant {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    name: "Write".to_string(),
                    id: Some("toolu_02".to_string()),
                    input: serde_json::json!({
                        "file_path": "/tmp/output.txt"
                    }),
                }],
                usage: None,
            },
        };

        let output = format_event_for_snapshot(&event);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_snapshot_format_event_tool_use_bash() {
        let event = ClaudeEvent::Assistant {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    name: "Bash".to_string(),
                    id: Some("toolu_03".to_string()),
                    input: serde_json::json!({
                        "description": "Run tests",
                        "command": "cargo test --all"
                    }),
                }],
                usage: None,
            },
        };

        let output = format_event_for_snapshot(&event);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_snapshot_format_event_tool_use_grep() {
        let event = ClaudeEvent::Assistant {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    name: "Grep".to_string(),
                    id: Some("toolu_04".to_string()),
                    input: serde_json::json!({
                        "pattern": "TODO|FIXME",
                        "path": "~/project/src"
                    }),
                }],
                usage: None,
            },
        };

        let output = format_event_for_snapshot(&event);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_snapshot_format_event_mixed_content() {
        let event = ClaudeEvent::Assistant {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Text {
                        text: "Let me read the file first.".to_string(),
                    },
                    ContentBlock::ToolUse {
                        name: "Read".to_string(),
                        id: Some("toolu_05".to_string()),
                        input: serde_json::json!({
                            "file_path": "/tmp/test.rs"
                        }),
                    },
                    ContentBlock::Text {
                        text: "Now I'll analyze the contents.".to_string(),
                    },
                ],
                usage: None,
            },
        };

        let output = format_event_for_snapshot(&event);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_snapshot_format_event_user_with_ask_user_result() {
        let mut tool_names = HashMap::new();
        tool_names.insert("toolu_ask_01".to_string(), "ask_user".to_string());

        let event = ClaudeEvent::User {
            message: AssistantMessage {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: Some("toolu_ask_01".to_string()),
                    content: serde_json::json!("User answered: **Yes, proceed**"),
                }],
                usage: None,
            },
        };

        let output = format_event_for_snapshot_with(&event, &mut tool_names);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_snapshot_format_event_ask_user_tool_use_shows_question() {
        let event = ClaudeEvent::Assistant {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    name: "ask_user".to_string(),
                    id: Some("toolu_ask_02".to_string()),
                    input: serde_json::json!({"question": "How are you today?"}),
                }],
                usage: None,
            },
        };

        let output = format_event_for_snapshot(&event);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_snapshot_format_event_ask_user_question_and_answer() {
        // Symulacja pełnego flow: ToolUse pytanie + ToolResult odpowiedź (z bold question stripped)
        let mut tool_names = HashMap::new();

        // 1. Assistant wysyła ask_user ToolUse
        let ask_event = ClaudeEvent::Assistant {
            message: AssistantMessage {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    name: "ask_user".to_string(),
                    id: Some("toolu_qa".to_string()),
                    input: serde_json::json!({"question": "How are you?"}),
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

        let ask_lines = format_event(
            &ask_event,
            &mut block_type,
            &mut tool_names,
            false,
            &mut tokens,
        );

        // 2. User odpowiada — ToolResult z format build_answer_markdown
        let answer_event = ClaudeEvent::User {
            message: AssistantMessage {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: Some("toolu_qa".to_string()),
                    content: serde_json::json!("**How are you?**\nI'm doing great!"),
                }],
                usage: None,
            },
        };

        let answer_lines = format_event(
            &answer_event,
            &mut block_type,
            &mut tool_names,
            false,
            &mut tokens,
        );

        // Połącz obie części
        let mut all_lines = ask_lines;
        all_lines.extend(answer_lines);
        let output = lines_to_debug(&all_lines);
        insta::assert_snapshot!(output);
    }
}
