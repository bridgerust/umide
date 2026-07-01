//! Parser for Claude Code's `--output-format stream-json` feed.
//!
//! `claude --print --output-format stream-json --verbose` emits newline-delimited
//! JSON, one object per record:
//!   * `{"type":"system","subtype":"init","session_id":…,"tools":[…]}` — once, at
//!     start; we capture `session_id` for multi-turn resume.
//!   * `{"type":"assistant","message":{"content":[ … ]}}` — content blocks: `text`
//!     → [`AgentEvent::TextDelta`], `thinking` → [`AgentEvent::ThinkingDelta`],
//!     `tool_use` → [`AgentEvent::ToolCallStarted`] + [`AgentEvent::ToolCallInput`].
//!   * `{"type":"user","message":{"content":[{"type":"tool_result", … }]}}` →
//!     [`AgentEvent::ToolResult`] (the name is looked up from the tool_use id).
//!   * `{"type":"result", … ,"usage":{…}}` — final; maps to
//!     [`AgentEvent::TurnComplete`]. The terminal `Done`/`Error` is NOT emitted
//!     here — process exit is the single source of truth for that (see the runner).

use std::collections::HashMap;

use serde_json::Value;
use umide_agent::{AgentEvent, Usage};

use super::runner::CliParser;
use crate::ai::Push;

#[derive(Default)]
pub struct ClaudeParser {
    session_id: Option<String>,
    /// tool_use id → tool name, so a later `tool_result` can be labeled.
    tool_names: HashMap<String, String>,
}

impl ClaudeParser {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CliParser for ClaudeParser {
    fn on_record(&mut self, v: &Value, push: &Push) {
        // Capture the session id from any record that carries it.
        if let Some(sid) = v.get("session_id").and_then(|s| s.as_str()) {
            self.session_id = Some(sid.to_string());
        }

        match v.get("type").and_then(|t| t.as_str()).unwrap_or_default() {
            "assistant" => {
                if let Some(content) =
                    v.pointer("/message/content").and_then(|c| c.as_array())
                {
                    for block in content {
                        self.on_assistant_block(block, push);
                    }
                }
            }
            "user" => {
                if let Some(content) =
                    v.pointer("/message/content").and_then(|c| c.as_array())
                {
                    for block in content {
                        if block.get("type").and_then(|t| t.as_str())
                            == Some("tool_result")
                        {
                            self.on_tool_result(block, push);
                        }
                    }
                }
            }
            "result" => {
                if let Some(usage) = v
                    .get("usage")
                    .and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok())
                {
                    push.emit(AgentEvent::TurnComplete { usage });
                }
            }
            _ => {} // system/init handled by the session_id capture above
        }
    }

    fn take_session_id(&self) -> Option<String> {
        self.session_id.clone()
    }
}

impl ClaudeParser {
    fn on_assistant_block(&mut self, block: &Value, push: &Push) {
        match block
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
        {
            "text" => {
                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                    if !t.is_empty() {
                        push.emit(AgentEvent::TextDelta(t.to_string()));
                    }
                }
            }
            "thinking" => {
                if let Some(t) = block.get("thinking").and_then(|t| t.as_str()) {
                    if !t.is_empty() {
                        push.emit(AgentEvent::ThinkingDelta(t.to_string()));
                    }
                }
            }
            "tool_use" => {
                let id = block
                    .get("id")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|s| s.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let input = block.get("input").cloned().unwrap_or(Value::Null);
                self.tool_names.insert(id.clone(), name.clone());
                push.emit(AgentEvent::ToolCallStarted {
                    id: id.clone(),
                    name: name.clone(),
                });
                push.emit(AgentEvent::ToolCallInput { id, name, input });
            }
            _ => {}
        }
    }

    fn on_tool_result(&self, block: &Value, push: &Push) {
        let id = block
            .get("tool_use_id")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        let is_error = block
            .get("is_error")
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        let name = self
            .tool_names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "tool".into());
        let summary = summarize_content(block.get("content"));
        push.emit(AgentEvent::ToolResult {
            id,
            name,
            ok: !is_error,
            summary,
        });
    }
}

/// A short, single-line summary of a tool_result `content` (string, or an array
/// of `{type:"text",text:…}` blocks), capped so a huge result doesn't flood the UI.
fn summarize_content(content: Option<&Value>) -> String {
    const CAP: usize = 200;
    let raw = match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > CAP {
        let mut s: String = one_line.chars().take(CAP).collect();
        s.push('…');
        s
    } else {
        one_line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn capture() -> (Push, Arc<Mutex<Vec<AgentEvent>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        let push = Push::new(move |ev| l.lock().unwrap().push(ev));
        (push, log)
    }

    #[test]
    fn text_and_session_capture() {
        let (push, log) = capture();
        let mut p = ClaudeParser::new();
        p.on_record(
            &serde_json::json!({"type":"system","subtype":"init","session_id":"sess-1"}),
            &push,
        );
        p.on_record(
            &serde_json::json!({
                "type":"assistant",
                "message":{"content":[{"type":"text","text":"Hello"}]}
            }),
            &push,
        );
        assert_eq!(p.take_session_id().as_deref(), Some("sess-1"));
        let evs = log.lock().unwrap();
        assert!(matches!(&evs[0], AgentEvent::TextDelta(t) if t == "Hello"));
    }

    #[test]
    fn tool_use_then_result_labels_by_id() {
        let (push, log) = capture();
        let mut p = ClaudeParser::new();
        p.on_record(
            &serde_json::json!({
                "type":"assistant",
                "message":{"content":[
                    {"type":"tool_use","id":"t1","name":"Read","input":{"path":"a.rs"}}
                ]}
            }),
            &push,
        );
        p.on_record(
            &serde_json::json!({
                "type":"user",
                "message":{"content":[
                    {"type":"tool_result","tool_use_id":"t1","is_error":false,
                     "content":[{"type":"text","text":"file contents here"}]}
                ]}
            }),
            &push,
        );
        let evs = log.lock().unwrap();
        assert!(
            matches!(&evs[0], AgentEvent::ToolCallStarted { name, .. } if name == "Read")
        );
        assert!(
            matches!(&evs[1], AgentEvent::ToolCallInput { name, .. } if name == "Read")
        );
        assert!(matches!(
            &evs[2],
            AgentEvent::ToolResult { name, ok: true, .. } if name == "Read"
        ));
    }

    #[test]
    fn result_maps_usage() {
        let (push, log) = capture();
        let mut p = ClaudeParser::new();
        p.on_record(
            &serde_json::json!({
                "type":"result","subtype":"success","session_id":"s",
                "usage":{"input_tokens":10,"output_tokens":20}
            }),
            &push,
        );
        let evs = log.lock().unwrap();
        assert!(matches!(
            &evs[0],
            AgentEvent::TurnComplete { usage } if usage.input_tokens == 10 && usage.output_tokens == 20
        ));
    }
}
