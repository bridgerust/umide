//! Parser for Gemini CLI's `-o stream-json` event stream.
//!
//! Event shapes are taken from the installed CLI's source (the
//! `JsonStreamEventType` enum + `nonInteractiveCli.js` emitter), so the schema
//! is ground-truth:
//!   * `{"type":"init","session_id":…,"model":…}` — capture `session_id`.
//!   * `{"type":"message","role":"assistant"|"user","content":…,"delta":true}` —
//!     assistant content is streamed as deltas → [`AgentEvent::TextDelta`]; the
//!     `user` echo is ignored.
//!   * `{"type":"tool_use","tool_name":…,"tool_id":…,"parameters":…}` →
//!     [`AgentEvent::ToolCallStarted`] + [`AgentEvent::ToolCallInput`].
//!   * `{"type":"tool_result","tool_id":…,"status":"success"|"error","output"?,
//!     "error"?:{message}}` → [`AgentEvent::ToolResult`] (name looked up by id).
//!   * `{"type":"result","status":…,"stats":{input_tokens,output_tokens,…}}` →
//!     [`AgentEvent::TurnComplete`]; the terminal `Done`/`Error` is the runner's
//!     (process exit). `error` events are non-fatal warnings and are skipped.

use std::collections::HashMap;

use serde_json::Value;
use umide_agent::{AgentEvent, Usage};

use super::runner::CliParser;
use crate::ai::Push;

#[derive(Default)]
pub struct GeminiParser {
    session_id: Option<String>,
    /// tool_id → tool_name, so a later `tool_result` can be labeled.
    tools: HashMap<String, String>,
}

impl GeminiParser {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CliParser for GeminiParser {
    fn on_record(&mut self, v: &Value, push: &Push) {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str());

        match v.get("type").and_then(|t| t.as_str()).unwrap_or_default() {
            "init" => {
                if let Some(sid) = s("session_id") {
                    self.session_id = Some(sid.to_string());
                }
            }
            "message" => {
                // Assistant content is streamed (delta:true); user is the echo.
                if s("role") == Some("assistant") {
                    if let Some(c) = s("content") {
                        if !c.is_empty() {
                            push.emit(AgentEvent::TextDelta(c.to_string()));
                        }
                    }
                }
            }
            "tool_use" => {
                let id = s("tool_id").unwrap_or_default().to_string();
                let name = s("tool_name").unwrap_or("tool").to_string();
                let params = v.get("parameters").cloned().unwrap_or(Value::Null);
                self.tools.insert(id.clone(), name.clone());
                push.emit(AgentEvent::ToolCallStarted {
                    id: id.clone(),
                    name: name.clone(),
                });
                push.emit(AgentEvent::ToolCallInput {
                    id,
                    name,
                    input: params,
                });
            }
            "tool_result" => {
                let id = s("tool_id").unwrap_or_default().to_string();
                let ok = s("status") != Some("error");
                let name =
                    self.tools.remove(&id).unwrap_or_else(|| "tool".to_string());
                let summary = s("output")
                    .map(str::to_string)
                    .or_else(|| {
                        v.pointer("/error/message")
                            .and_then(|m| m.as_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                push.emit(AgentEvent::ToolResult {
                    id,
                    name,
                    ok,
                    summary: truncate(&summary, 240),
                });
            }
            "result" => {
                if let Some(stats) = v.get("stats") {
                    push.emit(AgentEvent::TurnComplete {
                        usage: usage(stats),
                    });
                }
            }
            // `error` events are non-fatal warnings (loop/max-turns); a real
            // failure throws and the runner surfaces it from process exit.
            _ => {}
        }
    }

    fn take_session_id(&self) -> Option<String> {
        self.session_id.clone()
    }
}

fn usage(stats: &Value) -> Usage {
    let n = |k: &str| stats.get(k).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    Usage {
        input_tokens: n("input_tokens"),
        output_tokens: n("output_tokens"),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: n("cached_tokens"),
    }
}

fn truncate(s: &str, cap: usize) -> String {
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > cap {
        let mut t: String = one_line.chars().take(cap).collect();
        t.push('…');
        t
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
        (Push::new(move |ev| l.lock().unwrap().push(ev)), log)
    }

    #[test]
    fn init_message_tool_result_usage() {
        let (push, log) = capture();
        let mut p = GeminiParser::new();
        p.on_record(
            &serde_json::json!({"type":"init","session_id":"g-1","model":"auto"}),
            &push,
        );
        // user echo is ignored
        p.on_record(
            &serde_json::json!({"type":"message","role":"user","content":"hi"}),
            &push,
        );
        p.on_record(
            &serde_json::json!({"type":"tool_use","tool_name":"read_file","tool_id":"t1","parameters":{"path":"a.rs"}}),
            &push,
        );
        p.on_record(
            &serde_json::json!({"type":"tool_result","tool_id":"t1","status":"success","output":"contents"}),
            &push,
        );
        p.on_record(
            &serde_json::json!({"type":"message","role":"assistant","content":"Here is the answer.","delta":true}),
            &push,
        );
        p.on_record(
            &serde_json::json!({"type":"result","status":"success","stats":{"input_tokens":11,"output_tokens":22,"total_tokens":33}}),
            &push,
        );

        assert_eq!(p.take_session_id().as_deref(), Some("g-1"));
        let evs = log.lock().unwrap();
        assert!(
            matches!(&evs[0], AgentEvent::ToolCallStarted { name, .. } if name == "read_file")
        );
        assert!(matches!(&evs[1], AgentEvent::ToolCallInput { .. }));
        assert!(
            matches!(&evs[2], AgentEvent::ToolResult { ok: true, name, .. } if name == "read_file")
        );
        assert!(matches!(&evs[3], AgentEvent::TextDelta(t) if t.contains("answer")));
        assert!(
            matches!(&evs[4], AgentEvent::TurnComplete { usage } if usage.input_tokens == 11 && usage.output_tokens == 22)
        );
    }

    #[test]
    fn failed_tool_marks_not_ok() {
        let (push, log) = capture();
        let mut p = GeminiParser::new();
        p.on_record(
            &serde_json::json!({"type":"tool_use","tool_name":"write_file","tool_id":"w","parameters":{}}),
            &push,
        );
        p.on_record(
            &serde_json::json!({"type":"tool_result","tool_id":"w","status":"error","error":{"message":"denied"}}),
            &push,
        );
        let evs = log.lock().unwrap();
        assert!(
            matches!(&evs[2], AgentEvent::ToolResult { ok: false, summary, .. } if summary == "denied")
        );
    }
}
