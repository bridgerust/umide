//! Provider abstraction.
//!
//! The agent loop is provider-neutral: it works on the neutral model
//! ([`Message`]/[`ContentBlock`]/[`ToolDef`]) and lets a backend translate that
//! to a specific API's wire format. OpenAI, DeepSeek, and Gemini all speak the
//! OpenAI-compatible chat-completions protocol, so they share one backend.

use std::sync::atomic::AtomicBool;

use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::provider::{ProviderConfig, ProviderKind};
use crate::types::{ContentBlock, Message, ToolDef, Usage};

/// The assembled result of one streamed model turn, in the neutral model.
pub struct TurnResult {
    pub blocks: Vec<ContentBlock>,
    /// Normalized stop reason: `"tool_use"` when the model wants tools run,
    /// otherwise a terminal reason (e.g. `"end_turn"`).
    pub stop_reason: Option<String>,
    pub usage: Usage,
}

#[async_trait]
pub trait LlmBackend: Send + Sync {
    /// Stream one turn: translate `system`/`history`/`tools` to the wire format,
    /// parse the streamed response, forward deltas to `events`, and return the
    /// assembled neutral turn. `cancel` aborts mid-stream.
    async fn stream(
        &self,
        system: &str,
        history: &[Message],
        tools: &[ToolDef],
        cfg: &ProviderConfig,
        events: &UnboundedSender<AgentEvent>,
        cancel: &AtomicBool,
    ) -> Result<TurnResult, AgentError>;
}

/// Build the backend for a provider.
pub fn build_backend(
    cfg: &ProviderConfig,
) -> Result<Box<dyn LlmBackend>, AgentError> {
    let backend: Box<dyn LlmBackend> = match cfg.kind {
        ProviderKind::Anthropic => Box::new(crate::client::AnthropicBackend::new()?),
        ProviderKind::OpenAi | ProviderKind::DeepSeek | ProviderKind::Gemini => {
            Box::new(crate::openai::OpenAiBackend::new()?)
        }
    };
    Ok(backend)
}

/// Find the first occurrence of `needle` in `haystack` (shared SSE helper).
pub(crate) fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
