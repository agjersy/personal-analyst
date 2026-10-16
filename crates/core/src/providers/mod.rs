//! Model provider layer (PROTOCOL 4, ARCHITECTURE 5.10): one `ModelClient` trait, two wire
//! adapters, SSE parsing, capability probing, and the text-protocol fallback.

pub mod anthropic;
pub mod capabilities;
pub mod names;
pub mod openai_compat;
pub mod sse;
pub mod text_protocol;

use crate::config::ResolvedModel;
use analyst_protocol::config::ProviderKind;
use analyst_protocol::model::{ChatRequest, ModelEvent, StopReason};
use futures::stream::BoxStream;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A streaming chat client. Every adapter produces exactly the `ModelEvent` sequence of
/// PROTOCOL 4 and always terminates the stream with a `Stop` event.
pub trait ModelClient: Send + Sync {
    fn stream(&self, req: ChatRequest) -> BoxStream<'static, ModelEvent>;
    /// Model id to put in requests and usage events.
    fn model_id(&self) -> &str;
}

/// Build the client for a resolved model.
pub fn client_for(resolved: &ResolvedModel) -> anyhow::Result<Arc<dyn ModelClient>> {
    let api_key = crate::config::api_key(&resolved.provider)?;
    let base_url = resolved.provider.effective_base_url();
    Ok(match resolved.provider.kind {
        ProviderKind::OpenAiCompatible => Arc::new(openai_compat::OpenAiCompatClient::new(
            base_url,
            api_key,
            resolved.spec.id.clone(),
        )?),
        ProviderKind::Anthropic => Arc::new(anthropic::AnthropicClient::new(
            base_url,
            api_key.ok_or_else(|| {
                anyhow::anyhow!(
                    "provider `{}` (anthropic) needs `api_key_env` in config.toml",
                    resolved.provider.name
                )
            })?,
            resolved.spec.id.clone(),
        )?),
    })
}

/// Turn a receiver of events into a boxed stream (adapters push from a spawned task).
pub(crate) fn channel_stream(
    rx: tokio::sync::mpsc::Receiver<ModelEvent>,
) -> BoxStream<'static, ModelEvent> {
    Box::pin(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|e| (e, rx))
    }))
}

/// A client that replays scripted event sequences; records every request it receives.
/// Used by agent, eval, and probe tests.
pub struct ScriptedModelClient {
    scripts: Mutex<VecDeque<Vec<ModelEvent>>>,
    pub requests: Mutex<Vec<ChatRequest>>,
    model: String,
}

impl ScriptedModelClient {
    pub fn new(scripts: Vec<Vec<ModelEvent>>) -> Self {
        ScriptedModelClient {
            scripts: Mutex::new(scripts.into()),
            requests: Mutex::new(vec![]),
            model: "scripted/model".to_string(),
        }
    }

    /// Number of requests received so far.
    pub fn calls(&self) -> usize {
        self.requests.lock().map(|r| r.len()).unwrap_or(0)
    }

    /// Snapshot of the recorded requests.
    pub fn recorded(&self) -> Vec<ChatRequest> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

impl ModelClient for ScriptedModelClient {
    fn stream(&self, req: ChatRequest) -> BoxStream<'static, ModelEvent> {
        if let Ok(mut r) = self.requests.lock() {
            r.push(req);
        }
        let mut events = self
            .scripts
            .lock()
            .ok()
            .and_then(|mut s| s.pop_front())
            .unwrap_or_default();
        if !matches!(events.last(), Some(ModelEvent::Stop { .. })) {
            events.push(ModelEvent::Stop {
                reason: StopReason::End,
                message: None,
            });
        }
        Box::pin(futures::stream::iter(events))
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

/// Collect a full response from a stream: text, completed tool calls, usage, stop reason.
#[derive(Debug, Default, Clone)]
pub struct Collected {
    pub text: String,
    pub tool_calls: Vec<(String, String, serde_json::Value)>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub stop: Option<StopReason>,
    pub stop_message: Option<String>,
}

pub async fn collect(mut stream: BoxStream<'static, ModelEvent>) -> Collected {
    use futures::StreamExt;
    let mut c = Collected::default();
    while let Some(ev) = stream.next().await {
        match ev {
            ModelEvent::Text { delta } => c.text.push_str(&delta),
            ModelEvent::ToolCall {
                id,
                name,
                arguments,
            } => c.tool_calls.push((id, name, arguments)),
            ModelEvent::Usage {
                input_tokens,
                output_tokens,
            } => {
                c.input_tokens += input_tokens;
                c.output_tokens += output_tokens;
            }
            ModelEvent::Stop { reason, message } => {
                c.stop = Some(reason);
                c.stop_message = message;
                break;
            }
        }
    }
    c
}
