//! The agentic loop: drive the model, run the tools it asks for, repeat until
//! it produces a final answer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::backend::{build_backend, LlmBackend};
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::provider::ProviderConfig;
use crate::tools::{ToolExecutor, ToolInvocation};
use crate::types::{ContentBlock, Message};

/// Hard cap on tool-use turns per user message, so a misbehaving loop can never
/// run away with the user's API credits.
const MAX_ITERATIONS: u32 = 24;

pub struct Agent {
    backend: Box<dyn LlmBackend>,
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
        let backend = build_backend(&provider)?;
        Ok(Self {
            backend,
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
            let tools = self.tools.specs();
            let turn = self
                .backend
                .stream(
                    &self.system_prompt,
                    &self.history,
                    &tools,
                    &self.provider,
                    events,
                    cancel,
                )
                .await?;

            // Discard a cancelled turn: don't record the partial assistant
            // message (a dangling tool_use would corrupt the next request).
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }

            let _ = events.send(AgentEvent::TurnComplete {
                usage: turn.usage.clone(),
            });
            self.history.push(Message::assistant(turn.blocks.clone()));

            // Collect any tool calls the model made this turn.
            let calls: Vec<ToolInvocation> = turn
                .blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => {
                        Some(ToolInvocation {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        })
                    }
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
            for call in &calls {
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
                    tool_use_id: call.id.clone(),
                    content: out.content,
                    is_error: out.is_error.then_some(true),
                });
            }
            // A2 — close the observe→act→observe loop: after the tools run, let
            // the executor append an automatic observation (a fresh device
            // screenshot after a tap/swipe/type/key), so the model always sees
            // the post-action state next turn instead of acting blind.
            results.extend(self.tools.auto_observe(&calls).await);
            self.history.push(Message::user(results));
        }

        Err(AgentError::MaxIterations(MAX_ITERATIONS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::TurnResult;
    use crate::provider::ProviderKind;
    use crate::tools::ToolOutput;
    use crate::types::{ToolDef, Usage};
    use async_trait::async_trait;
    use std::sync::atomic::AtomicUsize;

    /// Turn 1 asks for a `tap`; turn 2 ends. Exercises the observe→act→observe
    /// path without a network.
    struct MockBackend {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LlmBackend for MockBackend {
        async fn stream(
            &self,
            _system: &str,
            _history: &[Message],
            _tools: &[ToolDef],
            _cfg: &ProviderConfig,
            _events: &UnboundedSender<AgentEvent>,
            _cancel: &AtomicBool,
        ) -> Result<TurnResult, AgentError> {
            let usage = Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            };
            if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
                Ok(TurnResult {
                    blocks: vec![ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "tap".into(),
                        input: serde_json::json!({ "x": 1, "y": 2 }),
                    }],
                    stop_reason: Some("tool_use".into()),
                    usage,
                })
            } else {
                Ok(TurnResult {
                    blocks: vec![ContentBlock::text("done")],
                    stop_reason: Some("end_turn".into()),
                    usage,
                })
            }
        }
    }

    struct MockExec;

    #[async_trait]
    impl ToolExecutor for MockExec {
        fn specs(&self) -> Vec<ToolDef> {
            vec![]
        }
        async fn execute(&self, _call: ToolInvocation) -> ToolOutput {
            ToolOutput::ok("tapped")
        }
        async fn auto_observe(
            &self,
            executed: &[ToolInvocation],
        ) -> Vec<ContentBlock> {
            if executed.iter().any(|c| c.name == "tap") {
                vec![ContentBlock::text("AUTO_OBSERVED")]
            } else {
                vec![]
            }
        }
    }

    #[tokio::test]
    async fn loop_auto_observes_after_a_device_action() {
        let mut agent = Agent {
            backend: Box::new(MockBackend {
                calls: AtomicUsize::new(0),
            }),
            provider: ProviderConfig::new(ProviderKind::Anthropic, "test-key"),
            system_prompt: "sys".into(),
            tools: Arc::new(MockExec),
            history: Vec::new(),
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = AtomicBool::new(false);
        agent
            .send(vec![ContentBlock::text("go")], tx, &cancel)
            .await
            .unwrap();

        // The tool-results message must also carry the auto-observation, i.e. the
        // loop re-observed after the tap without the model asking.
        let observed = agent.history().iter().any(|m| {
            m.content.iter().any(
                |b| matches!(b, ContentBlock::Text { text } if text == "AUTO_OBSERVED"),
            )
        });
        assert!(observed, "auto_observe result was not appended to history");
    }
}
