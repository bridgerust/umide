use std::{collections::HashSet, io::Write};

use serde_json::{Value, json};

use crate::{Event, ProtocolIo, Turn};

pub(super) fn thread_request(turn: &Turn) -> Value {
    let mut config = json!({"cwd":turn.cwd,"sandbox":"workspace-write","approvalPolicy":"on-request","approvalsReviewer":"user"});
    if let Some(server) = &turn.verify_server {
        // A dotted key overrides only Umide for this thread. Other
        // configured servers keep their own settings.
        let mut server = server.clone();
        for &name in crate::READ_ONLY_VERIFY_TOOLS {
            server["tools"][name] = json!({"approval_mode":"approve"});
        }
        config["config"] = json!({"mcp_servers.umide": server});
    }
    let method = if let Some(thread) = &turn.session {
        config["threadId"] = json!(thread);
        config["excludeTurns"] = json!(true);
        "thread/resume"
    } else {
        "thread/start"
    };
    json!({"id":2,"method":method,"params":config})
}

#[derive(Default)]
pub(super) struct State {
    thread: Option<String>,
    streamed: HashSet<String>,
}

impl State {
    pub(super) fn handle<W: Write>(
        &mut self,
        io: &mut ProtocolIo<'_, W>,
        msg: &Value,
        turn: &Turn,
    ) -> Result<Option<Event>, String> {
        let p = &msg["params"];
        let method = msg["method"].as_str();
        if let Some(id) = msg.get("id") {
            if let Some(method) = method {
                self.request(io, id, method, p, turn.verify_server.is_some())?;
            } else {
                if let Some(error) = msg.get("error") {
                    return Ok(Some(match error["message"].as_str() {
                        Some(message) => Event::ProviderError(message.into()),
                        None => Event::Error("Codex rejected the request.".into()),
                    }));
                }
                match id.as_u64() {
                    Some(1) => {
                        io.send(json!({"method":"initialized"}))?;
                        io.send(thread_request(turn))?;
                    }
                    Some(2) => {
                        let id = msg
                            .pointer("/result/thread/id")
                            .and_then(Value::as_str)
                            .ok_or("Codex returned no session ID.")?;
                        self.thread = Some(id.to_owned());
                        io.event(Event::Session(id.to_owned()));
                        io.send(json!({"id":3,"method":"turn/start","params":{"threadId":id,"input":turn.input}}))?;
                    }
                    _ => {}
                }
            }
            return Ok(None);
        }
        if self.thread.is_none() || p["threadId"].as_str() != self.thread.as_deref()
        {
            return Ok(None);
        }
        let item = &p["item"];
        match method.unwrap_or_default() {
            "item/agentMessage/delta" => {
                if let Some(delta) = p["delta"].as_str() {
                    self.streamed
                        .insert(p["itemId"].as_str().unwrap_or_default().into());
                    io.event(Event::Text(delta.into()));
                }
            }
            "item/completed" if item["type"] == "agentMessage" => {
                let already_streamed = self
                    .streamed
                    .remove(item["id"].as_str().unwrap_or_default());
                if let Some(text) =
                    item["text"].as_str().filter(|_| !already_streamed)
                {
                    io.event(Event::Text(text.into()));
                }
            }
            "item/started" if item["type"] == "commandExecution" => {
                if let Some(cmd) = item["command"].as_str() {
                    io.event(Event::Command(cmd.into()));
                }
            }
            "item/completed" if item["type"] == "fileChange" => {
                io.event(Event::Activity(format!(
                    "File changes: {}",
                    item["status"].as_str().unwrap_or("complete")
                )));
            }
            "turn/completed" => {
                let end = match p.pointer("/turn/status").and_then(Value::as_str) {
                    Some("completed") => Event::Done,
                    Some("interrupted") => Event::Cancelled,
                    _ => match p
                        .pointer("/turn/error/message")
                        .and_then(Value::as_str)
                    {
                        Some(message) => Event::ProviderError(message.into()),
                        None => {
                            Event::Error("Codex failed to complete the turn.".into())
                        }
                    },
                };
                return Ok(Some(end));
            }
            _ => {}
        }
        Ok(None)
    }

    fn request<W: Write>(
        &self,
        io: &mut ProtocolIo<'_, W>,
        id: &Value,
        method: &str,
        p: &Value,
        verification: bool,
    ) -> Result<(), String> {
        match method {
            "mcpServer/elicitation/request" => {
                let decline=json!({"id":id,"result":{"action":"decline","content":null}});
                let schema=&p["requestedSchema"];
                let is_confirmation = p["mode"]=="form" && schema["type"]=="object"
                    && schema["required"].as_array().is_none_or(Vec::is_empty)
                    && schema["properties"].as_object().is_none_or(serde_json::Map::is_empty);
                if !verification || p["serverName"]!="umide" || self.thread.is_none() || p["threadId"].as_str()!=self.thread.as_deref() || !is_confirmation {
                    return io.send(decline);
                }
                let Some(message)=p["message"].as_str() else { return io.send(decline); };
                let description = format!("Umide · {message}");
                io.approve(id.to_string(),description,json!({"id":id,"result":{"action":"accept","content":{}}}),decline)
            }
            "item/permissions/requestApproval" => io.send(json!({"id":id,"result":{"permissions":{},"scope":"turn"}})),
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                let deny = json!({"id":id,"result":{"decision":"decline"}});
                if self.thread.is_none() || p["threadId"].as_str() != self.thread.as_deref()
                    || p["availableDecisions"].as_array().is_some_and(|allowed| !allowed.iter().any(|a| *a == "accept")) {
                    return io.send(deny);
                }
                let title = if method.contains("fileChange") { "Allow file changes" } else { "Allow command" };
                let mut details = vec![title.to_string()];
                for key in ["command","cwd","grantRoot","reason"] {
                    if let Some(value) = p[key].as_str() { details.push(value.into()); }
                }
                for key in ["networkApprovalContext", "additionalPermissions", "commandActions"] {
                    if let Some(value) = p.get(key).filter(|v| !v.is_null()) { details.push(value.to_string()); }
                }
                io.approve(id.to_string(), details.join("\n"), json!({"id":id,"result":{"decision":"accept"}}), deny)
            }
            _ => io.send(json!({"id":id,"error":{"code":-32601,"message":"This request is unsupported by Umide."}})),
        }
    }
}
