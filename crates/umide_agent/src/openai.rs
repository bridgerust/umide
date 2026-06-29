//! OpenAI-compatible chat-completions backend.
//!
//! Covers OpenAI, DeepSeek, and Google Gemini (via its OpenAI-compatible
//! endpoint) — they differ only in base URL, model, and key. The neutral model
//! is translated to/from OpenAI's `messages` + `tool_calls` shape.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::backend::{find_subsequence, LlmBackend, TurnResult};
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::provider::ProviderConfig;
use crate::types::{
    ContentBlock, ImageSource, Message, Role, ToolDef, ToolResultContent, Usage,
};

pub struct OpenAiBackend {
    http: reqwest::Client,
}

impl OpenAiBackend {
    pub fn new() -> Result<Self, AgentError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
                .build()?,
        })
    }
}

fn image_part(source: &ImageSource) -> Value {
    let url = match source {
        ImageSource::Base64 { media_type, data } => {
            format!("data:{media_type};base64,{data}")
        }
        ImageSource::Url { url } => url.clone(),
    };
    json!({ "type": "image_url", "image_url": { "url": url } })
}

/// Translate the neutral history into OpenAI chat messages.
fn to_openai_messages(system: &str, history: &[Message]) -> Vec<Value> {
    let mut msgs = vec![json!({ "role": "system", "content": system })];
    for m in history {
        match m.role {
            Role::User => {
                let mut parts: Vec<Value> = Vec::new();
                let mut tool_msgs: Vec<Value> = Vec::new();
                let mut tool_images: Vec<Value> = Vec::new();
                for block in &m.content {
                    match block {
                        ContentBlock::Text { text } => {
                            parts.push(json!({ "type": "text", "text": text }));
                        }
                        ContentBlock::Image { source } => {
                            parts.push(image_part(source))
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            let mut text = String::new();
                            for c in content {
                                match c {
                                    ToolResultContent::Text { text: t } => {
                                        text.push_str(t)
                                    }
                                    ToolResultContent::Image { source } => {
                                        // tool messages are text-only; surface the
                                        // image as a following user message.
                                        tool_images.push(image_part(source));
                                    }
                                }
                            }
                            if is_error.unwrap_or(false) && !text.is_empty() {
                                text = format!("ERROR: {text}");
                            }
                            if text.is_empty() {
                                text = "(no textual output)".to_string();
                            }
                            tool_msgs.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": text
                            }));
                        }
                        _ => {}
                    }
                }
                // tool results answer the previous assistant's tool_calls, so
                // they must come immediately after it.
                msgs.append(&mut tool_msgs);
                parts.append(&mut tool_images);
                if !parts.is_empty() {
                    msgs.push(json!({ "role": "user", "content": parts }));
                }
            }
            Role::Assistant => {
                let mut text = String::new();
                let mut tool_calls: Vec<Value> = Vec::new();
                for block in &m.content {
                    match block {
                        ContentBlock::Text { text: t } => text.push_str(t),
                        ContentBlock::ToolUse { id, name, input } => {
                            tool_calls.push(json!({
                                "id": id,
                                "type": "function",
                                "function": { "name": name, "arguments": input.to_string() }
                            }));
                        }
                        _ => {} // thinking blocks are dropped for OpenAI-shaped APIs
                    }
                }
                let mut am = json!({ "role": "assistant" });
                am["content"] = if text.is_empty() {
                    Value::Null
                } else {
                    Value::String(text)
                };
                if !tool_calls.is_empty() {
                    am["tool_calls"] = Value::Array(tool_calls);
                }
                msgs.push(am);
            }
        }
    }
    msgs
}

#[async_trait]
impl LlmBackend for OpenAiBackend {
    async fn stream(
        &self,
        system: &str,
        history: &[Message],
        tools: &[ToolDef],
        cfg: &ProviderConfig,
        events: &UnboundedSender<AgentEvent>,
        cancel: &AtomicBool,
    ) -> Result<TurnResult, AgentError> {
        let messages = to_openai_messages(system, history);
        let tools_json: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema
                    }
                })
            })
            .collect();

        let mut body = json!({
            "model": cfg.model,
            "max_tokens": cfg.max_tokens,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if !tools_json.is_empty() {
            body["tools"] = Value::Array(tools_json);
        }

        let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(url)
            .header("authorization", format!("Bearer {}", cfg.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(AgentError::Api { status, body });
        }

        let mut acc = OpenAiAcc::default();
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
                    if data.is_empty() {
                        continue;
                    }
                    if data == "[DONE]" {
                        return Ok(acc.finish());
                    }
                    match serde_json::from_str::<Value>(data) {
                        Ok(v) => acc.handle(&v, events),
                        Err(e) => {
                            tracing::warn!(error = %e, "unparsable OpenAI SSE: {data}")
                        }
                    }
                }
            }
        }

        Ok(acc.finish())
    }
}

use futures_util::StreamExt;

#[derive(Default)]
struct ToolCallAcc {
    id: String,
    name: String,
    args: String,
    announced: bool,
}

#[derive(Default)]
struct OpenAiAcc {
    text: String,
    tools: Vec<ToolCallAcc>,
    finish_reason: Option<String>,
    usage: Usage,
}

impl OpenAiAcc {
    fn handle(&mut self, v: &Value, events: &UnboundedSender<AgentEvent>) {
        // Usage arrives on the final chunk (choices may be empty there).
        if let Some(usage) = v.get("usage").filter(|u| u.is_object()) {
            let input = usage
                .get("prompt_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0) as u32;
            let output = usage
                .get("completion_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0) as u32;
            self.usage.input_tokens = self.usage.input_tokens.max(input);
            self.usage.output_tokens = self.usage.output_tokens.max(output);
        }

        let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
            return;
        };
        if let Some(fr) = choice.get("finish_reason").and_then(|x| x.as_str()) {
            self.finish_reason = Some(fr.to_string());
        }
        let Some(delta) = choice.get("delta") else {
            return;
        };

        if let Some(content) = delta.get("content").and_then(|x| x.as_str()) {
            if !content.is_empty() {
                let _ = events.send(AgentEvent::TextDelta(content.to_string()));
                self.text.push_str(content);
            }
        }

        if let Some(tcs) = delta.get("tool_calls").and_then(|x| x.as_array()) {
            for tc in tcs {
                let idx =
                    tc.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                while self.tools.len() <= idx {
                    self.tools.push(ToolCallAcc::default());
                }
                let slot = &mut self.tools[idx];
                if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                    if !id.is_empty() {
                        slot.id = id.to_string();
                    }
                }
                if let Some(func) = tc.get("function") {
                    if let Some(name) = func.get("name").and_then(|x| x.as_str()) {
                        if !name.is_empty() {
                            slot.name = name.to_string();
                        }
                    }
                    if let Some(args) =
                        func.get("arguments").and_then(|x| x.as_str())
                    {
                        slot.args.push_str(args);
                    }
                }
                if !slot.announced && !slot.name.is_empty() {
                    slot.announced = true;
                    let _ = events.send(AgentEvent::ToolCallStarted {
                        id: slot.id.clone(),
                        name: slot.name.clone(),
                    });
                }
            }
        }
    }

    fn finish(self) -> TurnResult {
        let mut blocks: Vec<ContentBlock> = Vec::new();
        if !self.text.is_empty() {
            blocks.push(ContentBlock::Text { text: self.text });
        }
        let has_tools = !self.tools.is_empty();
        for (i, t) in self.tools.into_iter().enumerate() {
            let input = if t.args.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&t.args).unwrap_or_else(|_| json!({}))
            };
            let id = if t.id.is_empty() {
                format!("call_{i}")
            } else {
                t.id
            };
            blocks.push(ContentBlock::ToolUse {
                id,
                name: t.name,
                input,
            });
        }
        let stop_reason = if has_tools {
            Some("tool_use".to_string())
        } else {
            Some(self.finish_reason.unwrap_or_else(|| "end_turn".to_string()))
        };
        TurnResult {
            blocks,
            stop_reason,
            usage: self.usage,
        }
    }
}
