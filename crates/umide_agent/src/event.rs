//! Events streamed from the agent to the UI.
//!
//! The agent never touches Floem or the editor directly — it emits these over a
//! channel, and the AI Assistant panel renders them into signals. This keeps the
//! engine UI-agnostic and the editor responsive: the agent runs on its own
//! thread and the IDE stays usable while it works.

use crate::types::Usage;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// A chunk of the assistant's visible answer (render incrementally).
    TextDelta(String),
    /// A chunk of summarized reasoning (shown in a collapsible "thinking" area).
    ThinkingDelta(String),
    /// The model decided to call a tool; inputs are still streaming.
    ToolCallStarted { id: String, name: String },
    /// Full, parsed tool input is ready — render a tool/diff card here.
    ToolCallInput {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// A tool finished executing (e.g. an edit applied, a screenshot taken).
    ToolResult {
        id: String,
        name: String,
        ok: bool,
        summary: String,
    },
    /// One model turn completed; carries token usage for the cost meter.
    TurnComplete { usage: Usage },
    /// The whole request (possibly many tool-use turns) is done.
    Done,
    /// Something failed; the IDE keeps running, the panel shows this.
    Error(String),
}
