//! Model provider layer types (PROTOCOL 4, ARCHITECTURE 5.10).

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::config::ToolSupport;

/// Internal event stream produced by both wire adapters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    Text {
        delta: String,
    },
    /// Emitted once the call is complete.
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Stop {
        reason: StopReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    End,
    ToolCalls,
    MaxTokens,
    Error,
}

/// Message roles in a chat request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A part of a message's content. Providers map these to their wire formats.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    /// Base64-encoded image.
    Image {
        media_type: String,
        data: String,
    },
    /// An assistant tool call (model-facing `name`).
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
    },
    /// The result for an earlier tool call, in a user message.
    ToolResult {
        tool_call_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentPart>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Message {
            role: Role::User,
            content: vec![ContentPart::Text { text: text.into() }],
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Message {
            role: Role::Assistant,
            content: vec![ContentPart::Text { text: text.into() }],
        }
    }

    /// Concatenated text parts.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for p in &self.content {
            if let ContentPart::Text { text } = p {
                out.push_str(text);
            }
        }
        out
    }
}

/// A tool definition as shown to the model. `name` is model-facing (PROTOCOL 4 mapping).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON schema object.
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ChatRequest {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<ToolDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

/// Probed capability record, cached in `~/.analyst/capabilities.json` keyed by `provider/model`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelCapabilities {
    pub context_window: u32,
    pub tools: ToolSupport,
    pub vision: bool,
    pub probed_at: String,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        ModelCapabilities {
            context_window: 32768,
            tools: ToolSupport::Native,
            vision: false,
            probed_at: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn model_event_round_trip() {
        let evs = vec![
            ModelEvent::Text { delta: "a".into() },
            ModelEvent::ToolCall {
                id: "1".into(),
                name: "fs_read".into(),
                arguments: json!({"path":"x"}),
            },
            ModelEvent::Usage {
                input_tokens: 3,
                output_tokens: 4,
            },
            ModelEvent::Stop {
                reason: StopReason::ToolCalls,
                message: None,
            },
        ];
        let s = serde_json::to_string(&evs).unwrap();
        assert!(s.contains("\"type\":\"tool_call\""));
        assert!(s.contains("\"reason\":\"tool_calls\""));
        let back: Vec<ModelEvent> = serde_json::from_str(&s).unwrap();
        assert_eq!(back, evs);
    }

    #[test]
    fn chat_request_round_trip() {
        let req = ChatRequest {
            model: "m".into(),
            system: Some("sys".into()),
            messages: vec![
                Message::user_text("hi"),
                Message {
                    role: Role::Assistant,
                    content: vec![ContentPart::ToolCall {
                        id: "c".into(),
                        name: "fs_read".into(),
                        arguments: json!({}),
                    }],
                },
                Message {
                    role: Role::User,
                    content: vec![
                        ContentPart::ToolResult {
                            tool_call_id: "c".into(),
                            content: "ok".into(),
                            is_error: false,
                        },
                        ContentPart::Image {
                            media_type: "image/png".into(),
                            data: "AAAA".into(),
                        },
                    ],
                },
            ],
            tools: vec![ToolDef {
                name: "fs_read".into(),
                description: "d".into(),
                parameters: json!({"type":"object"}),
            }],
            max_tokens: Some(10),
            temperature: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: ChatRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back, req);
    }
}
