//! The agentic loop: drive the model, run the tools it asks for, repeat until
//! it produces a final answer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc::UnboundedSender;

use crate::client::AnthropicClient;
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::provider::ProviderConfig;
use crate::tools::{ToolExecutor, ToolInvocation};
use crate::types::*;

/// Hard cap on tool-use turns per user message, so a misbehaving loop can never
/// run away with the user's API credits.
const MAX_ITERATIONS: u32 = 24;

pub struct Agent {
    client: AnthropicClient,
    provider: ProviderConfig,
    system_prompt: String,
    tools: Arc<dyn ToolExecutor>,
    /// Full conversation history (stateless API → we resend it every turn).
    history: Vec<Message>,
}

impl Agent {
    pub fn new(
        provider: ProviderConfig,
        tools: Arc<dyn ToolExecutor>,
        system_prompt: impl Into<String>,
    ) -> Result<Self, AgentError> {
        let client = AnthropicClient::new(provider.api_key.clone(), provider.base_url.clone())?;
        Ok(Self {
            client,
            provider,
            system_prompt: system_prompt.into(),
            tools,
            history: Vec::new(),
        })
    }

    /// Like [`Agent::new`] but seeded with prior conversation history, so a UI
    /// can rebuild the agent each turn while keeping the multi-turn context.
    pub fn resume(
        provider: ProviderConfig,
        tools: Arc<dyn ToolExecutor>,
        system_prompt: impl Into<String>,
        history: Vec<Message>,
    ) -> Result<Self, AgentError> {
        let mut me = Self::new(provider, tools, system_prompt)?;
        me.history = history;
        Ok(me)
    }

    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// Send a user message (text and/or images) and run the loop to completion,
    /// streaming everything to `events`. Returns once the model stops asking for
    /// tools. Errors are also forwarded as `AgentEvent::Error` for the UI.
    pub async fn send(
        &mut self,
        content: Vec<ContentBlock>,
        events: UnboundedSender<AgentEvent>,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        self.history.push(Message::user(content));

        let result = self.run_loop(&events, cancel).await;
        match &result {
            Ok(()) => {
                let _ = events.send(AgentEvent::Done);
            }
            Err(e) => {
                let _ = events.send(AgentEvent::Error(e.to_string()));
            }
        }
        result
    }

    async fn run_loop(
        &mut self,
        events: &UnboundedSender<AgentEvent>,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        for _ in 0..MAX_ITERATIONS {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let req = self.build_request();
            let turn = self.client.stream(&req, events, cancel).await?;

            // Discard a cancelled turn: don't record the partial assistant
            // message (a dangling tool_use would corrupt the next request).
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }

            let _ = events.send(AgentEvent::TurnComplete { usage: turn.usage.clone() });
            self.history.push(Message::assistant(turn.blocks.clone()));

            // Collect any tool calls the model made this turn.
            let calls: Vec<ToolInvocation> = turn
                .blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => Some(ToolInvocation {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    }),
                    _ => None,
                })
                .collect();

            let stop = turn.stop_reason.as_deref();
            if calls.is_empty() || stop != Some("tool_use") {
                // end_turn / max_tokens / refusal / pause_turn with no tools → done.
                return Ok(());
            }

            // Execute each tool, then feed all results back in ONE user message
            // (the API requires every tool_use to have a matching tool_result).
            let mut results = Vec::with_capacity(calls.len());
            for call in calls {
                let _ = events.send(AgentEvent::ToolCallInput {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.input.clone(),
                });
                let out = self.tools.execute(call.clone()).await;
                let _ = events.send(AgentEvent::ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    ok: !out.is_error,
                    summary: out.summary,
                });
                results.push(ContentBlock::ToolResult {
                    tool_use_id: call.id,
                    content: out.content,
                    is_error: out.is_error.then_some(true),
                });
            }
            self.history.push(Message::user(results));
        }

        Err(AgentError::MaxIterations(MAX_ITERATIONS))
    }

    fn build_request(&self) -> MessagesRequest {
        MessagesRequest {
            model: self.provider.model.clone(),
            max_tokens: self.provider.max_tokens,
            // Single cached system block → tools+system prefix is cache-served.
            system: vec![SystemBlock::cached(self.system_prompt.clone())],
            messages: self.history.clone(),
            tools: self.tools.specs(),
            thinking: self.provider.thinking.then(Thinking::adaptive),
            output_config: self
                .provider
                .effort
                .clone()
                .map(|effort| OutputConfig { effort: Some(effort) }),
            stream: true,
        }
    }
}
