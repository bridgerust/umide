//! Anthropic Messages API backend (native `/v1/messages`, SSE streaming).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;

use crate::backend::{find_subsequence, LlmBackend, TurnResult};
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::provider::ProviderConfig;
use crate::types::*;

pub struct AnthropicBackend {
    http: reqwest::Client,
}

impl AnthropicBackend {
    pub fn new() -> Result<Self, AgentError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
                .build()?,
        })
    }
}

#[async_trait]
impl LlmBackend for AnthropicBackend {
    async fn stream(
        &self,
        system: &str,
        history: &[Message],
        tools: &[ToolDef],
        cfg: &ProviderConfig,
        events: &UnboundedSender<AgentEvent>,
        cancel: &AtomicBool,
    ) -> Result<TurnResult, AgentError> {
        let req = MessagesRequest {
            model: cfg.model.clone(),
            max_tokens: cfg.max_tokens,
            // Single cached system block → tools+system prefix is cache-served.
            system: vec![SystemBlock::cached(system.to_string())],
            messages: history.to_vec(),
            tools: tools.to_vec(),
            thinking: cfg.thinking.then(Thinking::adaptive),
            output_config: cfg.effort.clone().map(|effort| OutputConfig {
                effort: Some(effort),
            }),
            stream: true,
        };

        let url = format!("{}/v1/messages", cfg.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(url)
            .header("x-api-key", &cfg.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&req)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(AgentError::Api { status, body });
        }

        let mut acc = Accumulator::default();
        let mut buf: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();

        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            buf.extend_from_slice(&chunk?);
            while let Some(pos) = find_subsequence(&buf, b"\n\n") {
                let event: Vec<u8> = buf.drain(..pos + 2).collect();
                let text = String::from_utf8_lossy(&event);
                for line in text.lines() {
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }
                    match serde_json::from_str::<StreamEvent>(data) {
                        Ok(ev) => acc.handle(ev, events),
                        Err(e) => {
                            tracing::warn!(error = %e, "unparsable SSE event: {data}")
                        }
                    }
                }
            }
        }

        Ok(acc.finish())
    }
}

/// Reassembles streamed content blocks into a final assistant message.
#[derive(Default)]
struct Accumulator {
    blocks: Vec<BlockBuild>,
    stop_reason: Option<String>,
    usage: Usage,
}

enum BlockBuild {
    Text(String),
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    RedactedThinking(String),
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
}

impl Accumulator {
    fn handle(&mut self, ev: StreamEvent, events: &UnboundedSender<AgentEvent>) {
        match ev {
            StreamEvent::MessageStart { message } => {
                if let Some(u) = message.usage {
                    self.usage.merge(&u);
                }
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                let build = match content_block {
                    ContentBlock::Text { text } => BlockBuild::Text(text),
                    ContentBlock::Thinking {
                        thinking,
                        signature,
                    } => BlockBuild::Thinking {
                        thinking,
                        signature,
                    },
                    ContentBlock::RedactedThinking { data } => {
                        BlockBuild::RedactedThinking(data)
                    }
                    ContentBlock::ToolUse { id, name, .. } => {
                        let _ = events.send(AgentEvent::ToolCallStarted {
                            id: id.clone(),
                            name: name.clone(),
                        });
                        BlockBuild::ToolUse {
                            id,
                            name,
                            json: String::new(),
                        }
                    }
                    other => {
                        BlockBuild::Text(format!("[unexpected block: {other:?}]"))
                    }
                };
                self.set(index, build);
            }
            StreamEvent::ContentBlockDelta { index, delta } => match delta {
                ContentDelta::TextDelta { text } => {
                    let _ = events.send(AgentEvent::TextDelta(text.clone()));
                    if let Some(BlockBuild::Text(s)) = self.blocks.get_mut(index) {
                        s.push_str(&text);
                    }
                }
                ContentDelta::ThinkingDelta { thinking } => {
                    let _ = events.send(AgentEvent::ThinkingDelta(thinking.clone()));
                    if let Some(BlockBuild::Thinking { thinking: t, .. }) =
                        self.blocks.get_mut(index)
                    {
                        t.push_str(&thinking);
                    }
                }
                ContentDelta::SignatureDelta { signature } => {
                    if let Some(BlockBuild::Thinking { signature: sig, .. }) =
                        self.blocks.get_mut(index)
                    {
                        *sig = Some(signature);
                    }
                }
                ContentDelta::InputJsonDelta { partial_json } => {
                    if let Some(BlockBuild::ToolUse { json, .. }) =
                        self.blocks.get_mut(index)
                    {
                        json.push_str(&partial_json);
                    }
                }
            },
            StreamEvent::ContentBlockStop { .. } => {}
            StreamEvent::MessageDelta { delta, usage } => {
                if let Some(reason) = delta.stop_reason {
                    self.stop_reason = Some(reason);
                }
                if let Some(u) = usage {
                    self.usage.merge(&u);
                }
            }
            StreamEvent::MessageStop | StreamEvent::Ping => {}
            StreamEvent::Error { error } => {
                let _ = events.send(AgentEvent::Error(format!(
                    "{}: {}",
                    error.kind, error.message
                )));
            }
        }
    }

    fn set(&mut self, index: usize, build: BlockBuild) {
        while self.blocks.len() <= index {
            self.blocks.push(BlockBuild::Text(String::new()));
        }
        self.blocks[index] = build;
    }

    fn finish(self) -> TurnResult {
        let blocks = self
            .blocks
            .into_iter()
            .map(|b| match b {
                BlockBuild::Text(text) => ContentBlock::Text { text },
                BlockBuild::Thinking {
                    thinking,
                    signature,
                } => ContentBlock::Thinking {
                    thinking,
                    signature,
                },
                BlockBuild::RedactedThinking(data) => {
                    ContentBlock::RedactedThinking { data }
                }
                BlockBuild::ToolUse { id, name, json } => {
                    let input = if json.trim().is_empty() {
                        serde_json::json!({})
                    } else {
                        serde_json::from_str(&json)
                            .unwrap_or_else(|_| serde_json::json!({}))
                    };
                    ContentBlock::ToolUse { id, name, input }
                }
            })
            .collect();
        TurnResult {
            blocks,
            stop_reason: self.stop_reason,
            usage: self.usage,
        }
    }
}
