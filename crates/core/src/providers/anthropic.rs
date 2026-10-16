//! Anthropic Messages adapter (PROTOCOL 4).

use super::sse::SseParser;
use super::{channel_stream, ModelClient};
use analyst_protocol::model::{ChatRequest, ContentPart, ModelEvent, Role, StopReason};
use futures::stream::BoxStream;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Build the JSON request body.
pub fn build_body(req: &ChatRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    for m in &req.messages {
        let role = match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let blocks: Vec<Value> = m
            .content
            .iter()
            .map(|p| match p {
                ContentPart::Text { text } => json!({"type":"text","text":text}),
                ContentPart::Image { media_type, data } => json!({
                    "type":"image",
                    "source":{"type":"base64","media_type":media_type,"data":data}
                }),
                ContentPart::ToolCall {
                    id,
                    name,
                    arguments,
                } => json!({"type":"tool_use","id":id,"name":name,"input":arguments}),
                ContentPart::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                } => {
                    let mut b =
                        json!({"type":"tool_result","tool_use_id":tool_call_id,"content":content});
                    if *is_error {
                        b["is_error"] = json!(true);
                    }
                    b
                }
            })
            .filter(|b| !(b["type"] == "text" && b["text"].as_str() == Some("")))
            .collect();
        if blocks.is_empty() {
            continue;
        }
        messages.push(json!({"role": role, "content": blocks}));
    }
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "messages": messages,
        "stream": true,
    });
    if let Some(s) = &req.system {
        if !s.is_empty() {
            body["system"] = json!(s);
        }
    }
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| json!({"name":t.name,"description":t.description,"input_schema":t.parameters}))
                .collect(),
        );
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    body
}

#[derive(Debug, Default)]
struct Block {
    kind: String,
    id: String,
    name: String,
    json: String,
}

/// Incremental state for one streamed response.
#[derive(Debug, Default)]
pub struct StreamState {
    blocks: BTreeMap<u64, Block>,
    input_tokens: u64,
    output_tokens: u64,
    stop_reason: Option<String>,
    done: bool,
}

impl StreamState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one SSE event (`event` name and `data` payload).
    pub fn feed(&mut self, event: &str, data: &str) -> Vec<ModelEvent> {
        let mut out = Vec::new();
        if self.done {
            return out;
        }
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                self.done = true;
                out.push(ModelEvent::Stop {
                    reason: StopReason::Error,
                    message: Some(format!("malformed SSE JSON: {e}")),
                });
                return out;
            }
        };
        let kind = if event.is_empty() {
            v.get("type").and_then(Value::as_str).unwrap_or("")
        } else {
            event
        };
        match kind {
            "message_start" => {
                if let Some(u) = v.pointer("/message/usage") {
                    self.input_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                    self.output_tokens =
                        u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                }
            }
            "content_block_start" => {
                let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                let cb = v.get("content_block").cloned().unwrap_or(Value::Null);
                let b = Block {
                    kind: cb
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    id: cb
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    name: cb
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    json: String::new(),
                };
                if b.kind == "text" {
                    if let Some(t) = cb.get("text").and_then(Value::as_str) {
                        if !t.is_empty() {
                            out.push(ModelEvent::Text {
                                delta: t.to_string(),
                            });
                        }
                    }
                }
                self.blocks.insert(idx, b);
            }
            "content_block_delta" => {
                let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(d) = v.get("delta") {
                    match d.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            if let Some(t) = d.get("text").and_then(Value::as_str) {
                                if !t.is_empty() {
                                    out.push(ModelEvent::Text {
                                        delta: t.to_string(),
                                    });
                                }
                            }
                        }
                        Some("input_json_delta") => {
                            if let Some(p) = d.get("partial_json").and_then(Value::as_str) {
                                self.blocks.entry(idx).or_default().json.push_str(p);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "content_block_stop" => {
                let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(b) = self.blocks.remove(&idx) {
                    if b.kind == "tool_use" {
                        let arguments = if b.json.trim().is_empty() {
                            json!({})
                        } else {
                            serde_json::from_str(&b.json)
                                .unwrap_or_else(|_| json!({"_raw": b.json}))
                        };
                        out.push(ModelEvent::ToolCall {
                            id: b.id,
                            name: b.name,
                            arguments,
                        });
                    }
                }
            }
            "message_delta" => {
                if let Some(sr) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(sr.to_string());
                }
                if let Some(u) = v.get("usage") {
                    if let Some(o) = u.get("output_tokens").and_then(Value::as_u64) {
                        self.output_tokens = o;
                    }
                    if let Some(i) = u.get("input_tokens").and_then(Value::as_u64) {
                        self.input_tokens = i;
                    }
                }
            }
            "message_stop" => out.extend(self.finish()),
            "error" => {
                self.done = true;
                let msg = v
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("provider error")
                    .to_string();
                out.push(ModelEvent::Stop {
                    reason: StopReason::Error,
                    message: Some(msg),
                });
            }
            _ => {} // ping and unknown events
        }
        out
    }

    /// Emit usage and the stop event (idempotent).
    pub fn finish(&mut self) -> Vec<ModelEvent> {
        let mut out = Vec::new();
        if self.done {
            return out;
        }
        self.done = true;
        out.push(ModelEvent::Usage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
        });
        let reason = match self.stop_reason.as_deref() {
            Some("tool_use") => StopReason::ToolCalls,
            Some("max_tokens") => StopReason::MaxTokens,
            Some("end_turn") | Some("stop_sequence") | None => StopReason::End,
            Some(_) => StopReason::End,
        };
        out.push(ModelEvent::Stop {
            reason,
            message: None,
        });
        out
    }
}

/// Parse a complete SSE document into the event sequence.
pub fn parse_sse_document(doc: &str) -> Vec<ModelEvent> {
    let mut st = StreamState::new();
    let mut out = Vec::new();
    for ev in super::sse::parse_all(doc) {
        out.extend(st.feed(&ev.event, &ev.data));
    }
    out.extend(st.finish());
    out
}

pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl AnthropicClient {
    pub fn new(base_url: String, api_key: String, model: String) -> anyhow::Result<Self> {
        Ok(AnthropicClient {
            http: reqwest::Client::builder().build()?,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
        })
    }

    pub fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }

    /// `GET {base_url}/v1/models/{id}`, raw JSON (used for the capability probe).
    pub async fn model_info(&self) -> anyhow::Result<Value> {
        let url = format!("{}/v1/models/{}", self.base_url, self.model);
        Ok(self
            .http
            .get(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}

impl ModelClient for AnthropicClient {
    fn stream(&self, req: ChatRequest) -> BoxStream<'static, ModelEvent> {
        let body = build_body(&req);
        let http = self.http.clone();
        let url = self.messages_url();
        let api_key = self.api_key.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            let resp = http
                .post(&url)
                .header("x-api-key", &api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await;
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx
                        .send(ModelEvent::Stop {
                            reason: StopReason::Error,
                            message: Some(format!("request failed: {e}")),
                        })
                        .await;
                    return;
                }
            };
            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                let _ = tx
                    .send(ModelEvent::Stop {
                        reason: StopReason::Error,
                        message: Some(format!(
                            "HTTP {status}: {}",
                            crate::util::truncate_bytes(&text, 2000)
                        )),
                    })
                    .await;
                return;
            }
            use futures::StreamExt;
            let mut bytes = resp.bytes_stream();
            let mut parser = SseParser::new();
            let mut state = StreamState::new();
            while let Some(chunk) = bytes.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx
                            .send(ModelEvent::Stop {
                                reason: StopReason::Error,
                                message: Some(format!("stream error: {e}")),
                            })
                            .await;
                        return;
                    }
                };
                for ev in parser.push(&chunk) {
                    for me in state.feed(&ev.event, &ev.data) {
                        if tx.send(me).await.is_err() {
                            return;
                        }
                    }
                }
            }
            if let Some(ev) = parser.finish() {
                for me in state.feed(&ev.event, &ev.data) {
                    let _ = tx.send(me).await;
                }
            }
            for me in state.finish() {
                let _ = tx.send(me).await;
            }
        });
        channel_stream(rx)
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyst_protocol::model::{Message, ToolDef};

    const TOOL_FIXTURE: &str = include_str!("fixtures/anthropic_tool_use.sse");
    const TEXT_FIXTURE: &str = include_str!("fixtures/anthropic_text.sse");

    #[test]
    fn tool_use_fixture_yields_exact_sequence() {
        let events = parse_sse_document(TOOL_FIXTURE);
        assert_eq!(
            events,
            vec![
                ModelEvent::Text {
                    delta: "I'll check the script".into()
                },
                ModelEvent::Text {
                    delta: " index first.".into()
                },
                ModelEvent::ToolCall {
                    id: "toolu_01A".into(),
                    name: "scripts_index".into(),
                    arguments: json!({}),
                },
                ModelEvent::ToolCall {
                    id: "toolu_01B".into(),
                    name: "fs_read".into(),
                    arguments: json!({"path": "analysis/fitting.py", "end_line": 40}),
                },
                ModelEvent::Usage {
                    input_tokens: 250,
                    output_tokens: 61
                },
                ModelEvent::Stop {
                    reason: StopReason::ToolCalls,
                    message: None
                },
            ]
        );
    }

    #[test]
    fn text_fixture_yields_text_and_end() {
        let events = parse_sse_document(TEXT_FIXTURE);
        assert_eq!(
            events,
            vec![
                ModelEvent::Text {
                    delta: "The fit".into()
                },
                ModelEvent::Text {
                    delta: " converged.".into()
                },
                ModelEvent::Usage {
                    input_tokens: 40,
                    output_tokens: 8
                },
                ModelEvent::Stop {
                    reason: StopReason::End,
                    message: None
                },
            ]
        );
    }

    #[test]
    fn error_event_becomes_stop_error() {
        let doc = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
        assert_eq!(
            parse_sse_document(doc),
            vec![ModelEvent::Stop {
                reason: StopReason::Error,
                message: Some("Overloaded".into())
            }]
        );
    }

    #[test]
    fn request_body_construction() {
        let req = ChatRequest {
            model: "claude-x".into(),
            system: Some("sys".into()),
            messages: vec![
                Message::user_text("hi"),
                Message {
                    role: Role::Assistant,
                    content: vec![ContentPart::ToolCall {
                        id: "toolu_1".into(),
                        name: "fs_read".into(),
                        arguments: json!({"path":"a"}),
                    }],
                },
                Message {
                    role: Role::User,
                    content: vec![
                        ContentPart::ToolResult {
                            tool_call_id: "toolu_1".into(),
                            content: "ok".into(),
                            is_error: true,
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
                description: "read".into(),
                parameters: json!({"type":"object"}),
            }],
            max_tokens: None,
            temperature: Some(0.0),
        };
        let body = build_body(&req);
        assert_eq!(body["system"], "sys");
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(body["tools"][0]["name"], "fs_read");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(
            msgs[0],
            json!({"role":"user","content":[{"type":"text","text":"hi"}]})
        );
        assert_eq!(
            msgs[1],
            json!({"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"fs_read","input":{"path":"a"}}]})
        );
        assert_eq!(
            msgs[2]["content"][0],
            json!({"type":"tool_result","tool_use_id":"toolu_1","content":"ok","is_error":true})
        );
        assert_eq!(
            msgs[2]["content"][1],
            json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}})
        );
        assert!(body
            .get("messages")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m.get("role").is_some()));
    }
}
