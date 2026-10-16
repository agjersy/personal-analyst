//! Degraded tier for models without native tool calling: tool calls are fenced
//! ```` ```tool_call ```` blocks containing `{"name": ..., "arguments": {...}}`, and results go
//! back as user messages prefixed `tool_result <name>:`.

use analyst_protocol::model::ToolDef;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCall {
    pub name: String,
    pub arguments: Value,
}

/// Extract fenced tool calls from assistant text. Returns the text with the blocks removed
/// and the calls in order. Malformed blocks are left in the text untouched.
pub fn parse_tool_calls(text: &str) -> (String, Vec<ParsedCall>) {
    let mut calls = Vec::new();
    let mut clean = String::new();
    let mut rest = text;
    loop {
        let Some(start) = rest.find("```tool_call") else {
            clean.push_str(rest);
            break;
        };
        let after_fence = &rest[start + "```tool_call".len()..];
        let body_start = after_fence.find('\n').map(|i| i + 1).unwrap_or(0);
        let body = &after_fence[body_start..];
        let Some(end) = body.find("```") else {
            clean.push_str(rest);
            break;
        };
        let json_text = body[..end].trim();
        match serde_json::from_str::<Value>(json_text) {
            Ok(v) if v.get("name").and_then(Value::as_str).is_some() => {
                let name = v["name"].as_str().unwrap_or_default().to_string();
                let arguments = v
                    .get("arguments")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()));
                calls.push(ParsedCall { name, arguments });
                clean.push_str(&rest[..start]);
            }
            _ => {
                // leave the block in place
                let consumed = start + "```tool_call".len() + body_start + end + 3;
                clean.push_str(&rest[..consumed]);
            }
        }
        rest = &body[end + 3..];
    }
    (clean.trim().to_string(), calls)
}

/// System prompt addendum describing the protocol and the available tools.
pub fn instructions(tools: &[ToolDef]) -> String {
    let mut s = String::from(
        "TOOL CALLS: this interface has no native tool calling. To call a tool, emit a fenced block exactly like:\n\n```tool_call\n{\"name\": \"<tool name>\", \"arguments\": { ... }}\n```\n\nEmit one block per call, then stop and wait; results arrive as a user message starting with `tool_result <name>:`. Available tools:\n",
    );
    for t in tools {
        s.push_str(&format!(
            "- {}: {}\n  parameters: {}\n",
            t.name,
            t.description,
            serde_json::to_string(&t.parameters).unwrap_or_default()
        ));
    }
    s
}

/// Format a result for the user message.
pub fn format_result(name: &str, result: &Value) -> String {
    format!(
        "tool_result {name}:\n{}",
        serde_json::to_string_pretty(result).unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fenced_calls_and_keeps_text() {
        let text = "I will read it.\n```tool_call\n{\"name\": \"fs_read\", \"arguments\": {\"path\": \"a.py\"}}\n```\nThen done.";
        let (clean, calls) = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "fs_read");
        assert_eq!(calls[0].arguments, json!({"path":"a.py"}));
        assert_eq!(clean, "I will read it.\n\nThen done.");
    }

    #[test]
    fn malformed_block_is_left_in_text() {
        let text = "```tool_call\nnot json\n```";
        let (clean, calls) = parse_tool_calls(text);
        assert!(calls.is_empty());
        assert_eq!(clean, text);
    }
}
