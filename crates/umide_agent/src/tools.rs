//! Tool interface.
//!
//! The engine defines *what a tool is* and drives the call loop; the editor
//! implements the concrete tools (read/edit files, run commands, screenshot the
//! emulator, tap/type on the device, read logs, hot-reload). Keeping the
//! executor behind a trait means this crate has no dependency on the editor,
//! the proxy, or the emulator — they're wired in at the call site.

use async_trait::async_trait;

use crate::types::{ContentBlock, ToolDef, ToolResultContent};

/// A single tool call requested by the model.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// The result of executing a tool, returned to the model as a `tool_result`.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub content: Vec<ToolResultContent>,
    pub is_error: bool,
    /// Short human-readable line for the UI tool card (not sent to the model).
    pub summary: String,
}

impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            summary: first_line(&text),
            content: vec![ToolResultContent::text(text)],
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            summary: format!("error: {}", first_line(&text)),
            content: vec![ToolResultContent::text(text)],
            is_error: true,
        }
    }

    /// A result that includes an image (e.g. a device screenshot) plus a caption.
    pub fn with_image(caption: impl Into<String>, png_bytes: &[u8]) -> Self {
        let caption = caption.into();
        Self {
            summary: first_line(&caption),
            content: vec![
                ToolResultContent::text(caption),
                ToolResultContent::image_png(png_bytes),
            ],
            is_error: false,
        }
    }
}

fn first_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.len() > 120 {
        format!("{}…", &line[..120])
    } else {
        line.to_string()
    }
}

/// Implemented by the editor to expose its capabilities to the agent.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Tool schemas advertised to the model. Keep the order stable across turns
    /// so the tools+system prompt cache stays valid.
    fn specs(&self) -> Vec<ToolDef>;

    /// Execute one tool call. Hard-to-reverse actions (file edits, taps, shell
    /// commands) should be gated behind user approval *inside* this method, so
    /// the human stays in control of the IDE.
    async fn execute(&self, call: ToolInvocation) -> ToolOutput;

    /// After a batch of tool calls runs, optionally return extra observation
    /// content to append to the tool-results message — e.g. a fresh device
    /// screenshot after a `tap`/`swipe`, so the agent always *sees* the result
    /// of its action without having to remember to ask. This is what closes the
    /// observe→act→observe loop. Default: nothing. Executors that add images
    /// here should keep them small (downscaled) to protect the token budget.
    async fn auto_observe(&self, _executed: &[ToolInvocation]) -> Vec<ContentBlock> {
        Vec::new()
    }
}
