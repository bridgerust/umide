use std::{collections::HashMap, io::Write, process::Command};

use serde_json::{Value, json};

use crate::{Event, ProtocolIo};

pub(super) fn configure(
    cmd: &mut Command,
    session: Option<&str>,
    verify_server: Option<&Value>,
) {
    // Bare mode skips the CLI's keychain. Keep it for configured API/cloud
    // clients, and allow the normal CLI sign-in for desktop users.
    if [
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ]
    .iter()
    .any(|key| {
        std::env::var_os(key).is_some_and(|value| !value.is_empty() && value != "0")
    }) {
        cmd.arg("--bare");
    } else {
        cmd.args(["--setting-sources", ""]);
    }
    cmd.args([
        "--print",
        "--verbose",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
        "--permission-mode",
        "default",
        "--strict-mcp-config",
        "--tools",
        if verify_server.is_some() {
            "Read,Glob,Grep,Edit,Write,Bash,PowerShell,ToolSearch"
        } else {
            "Read,Glob,Grep,Edit,Write,Bash,PowerShell"
        },
        "--settings",
        r#"{"disableAllHooks":true,"permissions":{"ask":["Edit","Write","Bash","PowerShell"]}}"#,
    ]);
    if let Some(server) = verify_server {
        cmd.arg("--mcp-config")
            .arg(json!({"mcpServers": {"umide": server}}).to_string());
        let reads = crate::READ_ONLY_VERIFY_TOOLS
            .iter()
            .map(|name| format!("mcp__umide__{name}"))
            .collect::<Vec<_>>()
            .join(",");
        cmd.arg("--allowedTools").arg(reads);
        if session.is_none() {
            cmd.args(["--append-system-prompt", crate::VERIFY_INSTRUCTIONS]);
        }
    }
    if let Some(id) = session {
        cmd.args(["--resume", id]);
    }
}

#[derive(Default)]
pub(super) struct State {
    // Claude sends completed blocks as well as deltas. Retain the streamed prefix
    // by message ID to handle tool turns and multiple blocks without duplicates.
    prefixes: HashMap<String, HashMap<u64, String>>,
    streaming: String,
    emitted: bool,
}

impl State {
    fn text<W: Write>(&mut self, io: &ProtocolIo<'_, W>, text: &str) {
        if !text.is_empty() {
            self.emitted = true;
            io.event(Event::Text(text.into()));
        }
    }

    pub(super) fn handle<W: Write>(
        &mut self,
        io: &mut ProtocolIo<'_, W>,
        msg: &Value,
    ) -> Result<Option<Event>, String> {
        match msg["type"].as_str().unwrap_or_default() {
            "control_request" => {
                let id = msg["request_id"]
                    .as_str()
                    .ok_or("Missing Claude permission request ID.")?;
                let request = &msg["request"];
                let response = |r: Value| json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":r}});
                if request["subtype"] == "can_use_tool" {
                    let tool = request["tool_name"].as_str().unwrap_or("tool");
                    let details = format!(
                        "Claude Code · {tool}\n{}\n{}",
                        request["decision_reason"].as_str().unwrap_or_default(),
                        serde_json::to_string_pretty(&request["input"])
                            .unwrap_or_default()
                    );
                    io.approve(id.into(), details, response(json!({"behavior":"allow","updatedInput":request["input"]})),
                        response(json!({"behavior":"deny","message":"The user declined this action."})))?;
                } else {
                    io.send(json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"Unsupported host request"}}))?;
                }
            }
            "control_cancel_request" => {
                if let Some(id) = msg["request_id"].as_str() {
                    io.approvals.remove(id);
                }
            }
            "system" if msg["subtype"] == "init" => {
                if let Some(id) = msg["session_id"].as_str() {
                    io.event(Event::Session(id.into()));
                }
            }
            "stream_event" if msg["parent_tool_use_id"].is_null() => {
                let e = &msg["event"];
                let index = e["index"].as_u64().unwrap_or(0);
                match e["type"].as_str() {
                    Some("message_start") => {
                        self.streaming =
                            e["message"]["id"].as_str().unwrap_or_default().into()
                    }
                    Some("content_block_delta" | "content_block_start") => {
                        if let Some(s) = e
                            .pointer("/delta/text")
                            .or_else(|| e.pointer("/content_block/text"))
                            .and_then(Value::as_str)
                        {
                            self.prefixes
                                .entry(self.streaming.clone())
                                .or_default()
                                .entry(index)
                                .or_default()
                                .push_str(s);
                            self.text(io, s);
                        }
                    }
                    _ => {}
                }
            }
            "assistant" if msg["parent_tool_use_id"].is_null() => {
                let id = msg["message"]["id"].as_str().unwrap_or_default();
                if let Some(blocks) =
                    msg.pointer("/message/content").and_then(Value::as_array)
                {
                    // Claude may send one assistant event per finished block.
                    for block in blocks {
                        if let Some(s) = block["text"]
                            .as_str()
                            .filter(|_| block["type"] == "text")
                        {
                            let partial = self
                                .prefixes
                                .get_mut(id)
                                .and_then(|by_index| {
                                    let key = by_index
                                        .iter()
                                        .filter(|(_, p)| s.starts_with(p.as_str()))
                                        .map(|(idx, _)| *idx)
                                        .min()?;
                                    by_index.remove(&key)
                                })
                                .unwrap_or_default();
                            self.text(io, s.strip_prefix(&partial).unwrap_or(s));
                        } else if block["type"] == "tool_use" {
                            io.event(Event::Activity(format!(
                                "Claude Code · {}",
                                block["name"].as_str().unwrap_or("tool")
                            )));
                        }
                    }
                }
            }
            "result" => {
                let text = msg["result"].as_str().unwrap_or_default();
                if msg["is_error"] == true || msg["subtype"] != "success" {
                    let reason = msg["errors"]
                        .as_array()
                        .map(|v| {
                            v.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    let detail = [
                        text,
                        reason.as_str(),
                        msg["subtype"].as_str().unwrap_or_default(),
                    ]
                    .into_iter()
                    .find(|s| !s.is_empty());
                    return Ok(Some(match detail {
                        Some(text) => Event::ProviderError(text.into()),
                        None => Event::Error("Unknown error".into()),
                    }));
                }
                if !self.emitted {
                    self.text(io, text);
                }
                return Ok(Some(Event::Done));
            }
            _ => {}
        }
        Ok(None)
    }
}
