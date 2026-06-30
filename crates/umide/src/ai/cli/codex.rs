//! Parser for Codex CLI's `codex exec --json` event stream.
//!
//! Codex emits JSON Lines with a top-level `type`:
//!   * `{"type":"thread.started","thread_id":…}` — capture for `exec resume`.
//!   * `{"type":"turn.started"}` / `{"type":"turn.completed","usage":{…}}` — the
//!     latter carries token usage → [`AgentEvent::TurnComplete`].
//!   * `{"type":"item.started"|"item.completed","item":{…}}` — the work items.
//!     Item types we map: `command_execution` (a shell command → tool card +
//!     result), `agent_message` (assistant text → [`AgentEvent::TextDelta`]),
//!     `reasoning` (→ [`AgentEvent::ThinkingDelta`]), `file_change` (an edit).
//!   * `{"type":"error"|"turn.failed", …}` — left to the runner (process exit is
//!     the single source of truth for the terminal `Done`/`Error`).

use std::collections::HashMap;

use serde_json::Value;
use umide_agent::{AgentEvent, Usage};

use super::runner::CliParser;
use crate::ai::Push;

#[derive(Default)]
pub struct CodexParser {
    thread_id: Option<String>,
    /// item id → tool label, so `item.completed` can label its result.
    tools: HashMap<String, String>,
}

impl CodexParser {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CliParser for CodexParser {
    fn on_record(&mut self, v: &Value, push: &Push) {
        if let Some(tid) = v.get("thread_id").and_then(|t| t.as_str()) {
            self.thread_id = Some(tid.to_string());
        }

        match v.get("type").and_then(|t| t.as_str()).unwrap_or_default() {
            "item.started" => {
                if let Some(item) = v.get("item") {
                    self.on_item(item, false, push);
                }
            }
            "item.completed" => {
                if let Some(item) = v.get("item") {
                    self.on_item(item, true, push);
                }
            }
            "turn.completed" => {
                if let Some(u) = v.get("usage") {
                    push.emit(AgentEvent::TurnComplete { usage: usage(u) });
                }
            }
            // thread.started / turn.started: nothing to render.
            // error / turn.failed: the runner surfaces failure from process exit.
            _ => {}
        }
    }

    fn take_session_id(&self) -> Option<String> {
        self.thread_id.clone()
    }
}

impl CodexParser {
    fn on_item(&mut self, item: &Value, completed: bool, push: &Push) {
        let id = item
            .get("id")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        let str_field = |k: &str| item.get(k).and_then(|v| v.as_str());

        match item
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
        {
            "agent_message" => {
                // Emit once, on completion (avoids dupes if updates stream).
                if completed {
                    if let Some(t) = str_field("text") {
                        if !t.is_empty() {
                            push.emit(AgentEvent::TextDelta(t.to_string()));
                        }
                    }
                }
            }
            "reasoning" => {
                if completed {
                    if let Some(t) = str_field("text") {
                        if !t.is_empty() {
                            push.emit(AgentEvent::ThinkingDelta(t.to_string()));
                        }
                    }
                }
            }
            "command_execution" => {
                let cmd = str_field("command").unwrap_or_default().to_string();
                if !completed {
                    self.tools.insert(id.clone(), "command".to_string());
                    push.emit(AgentEvent::ToolCallStarted {
                        id: id.clone(),
                        name: "command".to_string(),
                    });
                    push.emit(AgentEvent::ToolCallInput {
                        id,
                        name: "command".to_string(),
                        input: serde_json::json!({ "command": cmd }),
                    });
                } else {
                    let exit =
                        item.get("exit_code").and_then(|c| c.as_i64()).unwrap_or(-1);
                    let out = str_field("aggregated_output").unwrap_or_default();
                    let name =
                        self.tools.remove(&id).unwrap_or_else(|| "command".into());
                    push.emit(AgentEvent::ToolResult {
                        id,
                        name,
                        ok: exit == 0,
                        summary: truncate(out, 240),
                    });
                }
            }
            "file_change" => {
                if !completed {
                    push.emit(AgentEvent::ToolCallStarted {
                        id: id.clone(),
                        name: "file_change".to_string(),
                    });
                    push.emit(AgentEvent::ToolCallInput {
                        id,
                        name: "file_change".to_string(),
                        input: item.clone(),
                    });
                } else {
                    push.emit(AgentEvent::ToolResult {
                        id,
                        name: "file_change".to_string(),
                        ok: item.get("status").and_then(|s| s.as_str())
                            != Some("failed"),
                        summary: file_change_summary(item),
                    });
                }
            }
            other => {
                // Generic: surface unknown item kinds as a tool card so the user
                // still sees activity (e.g. mcp_tool_call, web_search, todo_list).
                if !completed && !other.is_empty() {
                    push.emit(AgentEvent::ToolCallStarted {
                        id,
                        name: other.to_string(),
                    });
                }
            }
        }
    }
}

fn usage(u: &Value) -> Usage {
    let n = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    Usage {
        input_tokens: n("input_tokens"),
        output_tokens: n("output_tokens"),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: n("cached_input_tokens"),
    }
}

fn file_change_summary(item: &Value) -> String {
    // Codex reports the touched paths; show them if present.
    if let Some(changes) = item.get("changes").and_then(|c| c.as_array()) {
        let paths: Vec<&str> = changes
            .iter()
            .filter_map(|c| c.get("path").and_then(|p| p.as_str()))
            .collect();
        if !paths.is_empty() {
            return truncate(&paths.join(", "), 240);
        }
    }
    "applied".to_string()
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
    fn command_then_message_and_session() {
        let (push, log) = capture();
        let mut p = CodexParser::new();
        p.on_record(
            &serde_json::json!({"type":"thread.started","thread_id":"th-1"}),
            &push,
        );
        p.on_record(
            &serde_json::json!({"type":"item.started","item":{
                "id":"item_0","type":"command_execution",
                "command":"bash -lc 'cat notes.txt'","status":"in_progress"
            }}),
            &push,
        );
        p.on_record(
            &serde_json::json!({"type":"item.completed","item":{
                "id":"item_0","type":"command_execution",
                "command":"bash -lc 'cat notes.txt'","aggregated_output":"alpha\nbeta\n",
                "exit_code":0,"status":"completed"
            }}),
            &push,
        );
        p.on_record(
            &serde_json::json!({"type":"item.completed","item":{
                "id":"item_1","type":"agent_message","text":"The words are alpha and beta."
            }}),
            &push,
        );
        p.on_record(
            &serde_json::json!({"type":"turn.completed","usage":{
                "input_tokens":6215,"cached_input_tokens":3072,"output_tokens":257
            }}),
            &push,
        );

        assert_eq!(p.take_session_id().as_deref(), Some("th-1"));
        let evs = log.lock().unwrap();
        assert!(
            matches!(&evs[0], AgentEvent::ToolCallStarted { name, .. } if name == "command")
        );
        assert!(matches!(&evs[1], AgentEvent::ToolCallInput { .. }));
        assert!(matches!(
            &evs[2],
            AgentEvent::ToolResult { ok: true, summary, .. } if summary.contains("alpha")
        ));
        assert!(matches!(&evs[3], AgentEvent::TextDelta(t) if t.contains("alpha")));
        assert!(matches!(
            &evs[4],
            AgentEvent::TurnComplete { usage }
                if usage.input_tokens == 6215 && usage.cache_read_input_tokens == 3072
        ));
    }

    #[test]
    fn failed_command_marks_not_ok() {
        let (push, log) = capture();
        let mut p = CodexParser::new();
        p.on_record(
            &serde_json::json!({"type":"item.completed","item":{
                "id":"x","type":"command_execution","command":"false",
                "aggregated_output":"","exit_code":1,"status":"completed"
            }}),
            &push,
        );
        let evs = log.lock().unwrap();
        assert!(matches!(&evs[0], AgentEvent::ToolResult { ok: false, .. }));
    }
}
