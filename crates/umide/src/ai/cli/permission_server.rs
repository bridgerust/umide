//! In-process MCP server backing Claude Code's `--permission-prompt-tool`.
//!
//! When Claude Code runs in the default permission mode, every tool call it
//! makes is routed to a single MCP tool we expose here (`permission_prompt`).
//! We auto-allow read-only tools (so the user isn't pestered for every file
//! read) and, for mutating tools (Edit/Write/Bash/…), surface an Approve/Reject
//! card in UMIDE's [`ApprovalQueue`] and block on the user's decision — then
//! return Claude's allow/deny contract. Claude performs the action itself; UMIDE
//! only decides. This restores per-action approval for the agent-CLI path,
//! matching the built-in assistant's "additive & safe" posture.
//!
//! Transport is HTTP on `127.0.0.1:<ephemeral>` (no subprocess); Claude connects
//! via `--mcp-config {"mcpServers":{"umide":{"type":"http","url":…}}}`. We
//! implement the minimal Streamable-HTTP MCP surface: `initialize`,
//! `notifications/initialized`, `tools/list`, `tools/call`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::ai::{ApprovalKind, ApprovalOutcome, ApprovalQueue, ApprovalRequest};

/// Called after an approval card is pushed, to wake whoever drains the queue
/// (the UI thread in the app; an auto-approver in tests). Decouples this server
/// from floem so it can run headless.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// The single tool we advertise; the CLI references it as
/// `mcp__umide__permission_prompt`.
const TOOL_NAME: &str = "permission_prompt";
const SERVER_NAME: &str = "umide";
/// A protocol version in Claude 2.x's accepted set.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// A running permission server. Dropping it shuts the server thread down.
pub struct PermissionServer {
    port: u16,
    shutdown: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl PermissionServer {
    /// Bind an ephemeral localhost port and start serving. Approval cards are
    /// pushed onto `approvals`; `notify` is invoked to wake whoever drains them.
    pub fn start(approvals: ApprovalQueue, notify: Notify) -> std::io::Result<Self> {
        let server = tiny_http::Server::http("127.0.0.1:0")
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
        let shutdown = Arc::new(AtomicBool::new(false));
        let sd = shutdown.clone();
        let handle = std::thread::Builder::new()
            .name("umide-mcp-perm".into())
            .spawn(move || serve(server, approvals, notify, sd))?;
        Ok(Self {
            port,
            shutdown,
            handle: Some(handle),
        })
    }

    /// The `--mcp-config` JSON for Claude to reach this server.
    pub fn mcp_config_json(&self) -> String {
        format!("{{\"mcpServers\":{{{}}}}}", self.mcp_config_entry())
    }

    /// This server's entry for the `mcpServers` map, as `"umide": { … }` (no
    /// braces), so it can be merged with the device server's entry into one
    /// `--mcp-config` object (Claude runs `--strict-mcp-config`, so both servers
    /// must live in that single JSON).
    pub fn mcp_config_entry(&self) -> String {
        format!(
            "\"{SERVER_NAME}\":{{\"type\":\"http\",\
             \"url\":\"http://127.0.0.1:{}/mcp\"}}",
            self.port
        )
    }

    /// The `--permission-prompt-tool` value.
    pub fn tool_ref(&self) -> String {
        format!("mcp__{SERVER_NAME}__{TOOL_NAME}")
    }
}

impl Drop for PermissionServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // The serve loop uses recv_timeout, so it will notice the flag; poke it
        // with a throwaway connection so it doesn't wait out the full timeout.
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn serve(
    server: tiny_http::Server,
    approvals: ApprovalQueue,
    notify: Notify,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::Relaxed) {
        match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(req)) => {
                // Handle each request on its own thread, so a card the user is
                // still deciding on doesn't stall the accept loop / other calls.
                let approvals = approvals.clone();
                let notify = notify.clone();
                std::thread::spawn(move || handle_request(req, approvals, notify));
            }
            Ok(None) => continue, // timeout — re-check shutdown
            Err(_) => break,
        }
    }
}

fn handle_request(
    mut req: tiny_http::Request,
    approvals: ApprovalQueue,
    notify: Notify,
) {
    let mut body = String::new();
    let _ = req.as_reader().read_to_string(&mut body);
    let msg: Value = serde_json::from_str(&body).unwrap_or(Value::Null);

    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = msg.get("id").cloned();

    let result: Option<Value> = match method {
        "initialize" => Some(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "umide-approver", "version": env!("CARGO_PKG_VERSION") },
        })),
        "tools/list" => Some(json!({ "tools": [tool_descriptor()] })),
        "tools/call" => Some(json!({
            "content": [{
                "type": "text",
                "text": handle_tools_call(&msg, &approvals, &notify).to_string(),
            }],
        })),
        "ping" => Some(json!({})),
        // Notifications (no `id`) and anything else: no result body.
        _ => None,
    };

    // A request (has `id`) gets a JSON-RPC response; a notification gets 202.
    let response_body = match (id, result) {
        (Some(id), Some(result)) => {
            Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
        }
        (Some(id), None) => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "method not found" },
        })),
        (None, _) => None, // notification
    };

    match response_body {
        Some(v) => {
            let header = tiny_http::Header::from_bytes(
                &b"Content-Type"[..],
                &b"application/json"[..],
            )
            .expect("static header");
            let resp =
                tiny_http::Response::from_string(v.to_string()).with_header(header);
            let _ = req.respond(resp);
        }
        None => {
            let _ = req.respond(tiny_http::Response::empty(202));
        }
    }
}

fn tool_descriptor() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "UMIDE approval gate: decides whether a proposed tool use \
                        is allowed. Returns {\"behavior\":\"allow\",\"updatedInput\":…} \
                        or {\"behavior\":\"deny\",\"message\":…}.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "tool_name": { "type": "string" },
                "input": { "type": "object" },
                "tool_use_id": { "type": "string" },
            },
            "required": ["tool_name", "input"],
        },
    })
}

/// Decide a single `tools/call`, blocking on the user for mutating tools.
fn handle_tools_call(
    msg: &Value,
    approvals: &ApprovalQueue,
    notify: &Notify,
) -> Value {
    let args = msg.pointer("/params/arguments");
    let tool_name = args
        .and_then(|a| a.get("tool_name"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    let input = args
        .and_then(|a| a.get("input"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    // Read-only tools never need a prompt.
    if tool_name.is_empty() || is_read_only(tool_name) {
        return allow(input);
    }

    let (title, detail) = describe(tool_name, &input);
    let (respond, rx) = oneshot::channel();
    approvals.lock().unwrap().push_back(ApprovalRequest {
        id: crate::ai::next_approval_id(),
        title,
        detail,
        kind: ApprovalKind::CliPermission {
            tool_name: tool_name.to_string(),
        },
        respond,
    });
    notify();

    // Block this request thread until the user decides (or the panel drops the
    // sender, which yields a deny).
    match rx.blocking_recv() {
        Ok(ApprovalOutcome::Allowed)
        | Ok(ApprovalOutcome::CommandApproved)
        | Ok(ApprovalOutcome::EditApplied) => allow(input),
        _ => deny("Rejected in UMIDE."),
    }
}

fn allow(input: Value) -> Value {
    // `updatedInput` must be an object; echo the input back unchanged.
    let updated = if input.is_object() { input } else { json!({}) };
    json!({ "behavior": "allow", "updatedInput": updated })
}

fn deny(message: &str) -> Value {
    json!({ "behavior": "deny", "message": message })
}

/// Tools that cannot mutate the workspace or run commands — auto-approved.
/// Includes the device-MCP *read* tools (observe the emulator); the device
/// *write* tools (tap/swipe/type/key) fall through to the approval prompt.
fn is_read_only(tool: &str) -> bool {
    matches!(
        tool,
        "Read"
            | "Grep"
            | "Glob"
            | "LS"
            | "WebFetch"
            | "WebSearch"
            | "TodoWrite"
            | "NotebookRead"
            | "BashOutput"
            | "ListMcpResources"
            | "ReadMcpResource"
            | "mcp__umide-device__device_screenshot"
            | "mcp__umide-device__describe_ui"
            | "mcp__umide-device__device_logs"
    )
}

/// A human title + detail for the approval card, by tool.
fn describe(tool_name: &str, input: &Value) -> (String, String) {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str()).map(str::to_string);
    match tool_name {
        "Bash" => {
            let cmd = s("command").unwrap_or_default();
            let desc = s("description");
            let detail = match desc {
                Some(d) if !d.is_empty() => format!("{cmd}\n\n{d}"),
                _ => cmd,
            };
            ("Claude Code wants to run a command".to_string(), detail)
        }
        "Edit" | "MultiEdit" => {
            let path = s("file_path").unwrap_or_default();
            (
                format!("Claude Code wants to edit {path}"),
                edit_detail(input),
            )
        }
        "Write" => {
            let path = s("file_path").unwrap_or_default();
            let body = s("content").unwrap_or_default();
            (
                format!("Claude Code wants to write {path}"),
                truncate(&body, 600),
            )
        }
        "NotebookEdit" => {
            let path = s("notebook_path").unwrap_or_default();
            (
                format!("Claude Code wants to edit notebook {path}"),
                String::new(),
            )
        }
        other => (
            format!("Claude Code wants to use {other}"),
            truncate(&input.to_string(), 600),
        ),
    }
}

fn edit_detail(input: &Value) -> String {
    let old = input.get("old_string").and_then(|v| v.as_str());
    let new = input.get("new_string").and_then(|v| v.as_str());
    match (old, new) {
        (Some(o), Some(n)) => {
            format!("- {}\n+ {}", truncate(o, 300), truncate(n, 300))
        }
        _ => truncate(&input.to_string(), 600),
    }
}

fn truncate(s: &str, cap: usize) -> String {
    if s.chars().count() > cap {
        let mut t: String = s.chars().take(cap).collect();
        t.push('…');
        t
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_tools_auto_allow() {
        // The full handler needs a Notify/queue (driven by the UI or the
        // example harness); here we exercise the read-only fast path + the
        // allow/deny shapes, which is the contract Claude depends on.
        let msg = json!({
            "method": "tools/call",
            "params": { "name": TOOL_NAME, "arguments": {
                "tool_name": "Read", "input": { "file_path": "a.rs" }
            }}
        });
        let args = msg.pointer("/params/arguments").unwrap();
        let tool = args.get("tool_name").unwrap().as_str().unwrap();
        assert!(is_read_only(tool));
        let decided = allow(json!({"file_path":"a.rs"}));
        assert_eq!(decided["behavior"], "allow");
        assert!(decided["updatedInput"].is_object());
    }

    #[test]
    fn deny_shape() {
        let d = deny("nope");
        assert_eq!(d["behavior"], "deny");
        assert_eq!(d["message"], "nope");
    }

    #[test]
    fn describe_bash_and_edit() {
        let (t, d) = describe("Bash", &json!({"command":"cargo test"}));
        assert!(t.contains("run a command"));
        assert!(d.contains("cargo test"));
        let (t2, _) = describe("Edit", &json!({"file_path":"src/x.rs"}));
        assert!(t2.contains("src/x.rs"));
    }

    #[test]
    fn mutating_tools_are_not_auto_allowed() {
        for m in ["Edit", "Write", "Bash", "MultiEdit", "NotebookEdit"] {
            assert!(!is_read_only(m), "{m} must require approval");
        }
    }
}
