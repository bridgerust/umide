//! In-process MCP server exposing UMIDE's emulator **device tools** to the
//! Claude Code CLI backend, so the in-panel session can drive the Android
//! device (screenshot → reason → tap) **on the machine's `claude` login — no
//! API key**. It mirrors [`super::permission_server`]: a `tiny_http`
//! Streamable-HTTP MCP server (`initialize`/`tools/list`/`tools/call`/`ping`)
//! bound to an ephemeral localhost port, added as a second entry in the CLI's
//! `--mcp-config` so Claude sees the tools as `mcp__umide-device__<tool>`.
//!
//! Each tool reuses the same device plumbing as the built-in LLM agent — the
//! `ai.rs` free functions (`android_screenshot`, `adb_input`,
//! `android_describe_ui`, `android_logs`), reached via `super::super::` since
//! `ai/cli/` is a descendant of the `ai` module — so behaviour is identical to
//! the built-in agent's device tools (and inherits the #41 argv-direct adb path
//! that works on Windows). Writes (`tap`/`swipe`/`type`/`key`) are still gated
//! by UMIDE's permission bridge (Claude routes every tool call through
//! `--permission-prompt-tool`); reads (`screenshot`/`describe_ui`/`logs`) are
//! auto-allowed there.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use umide_agent::ToolResultContent;
use umide_agent::tools::ToolOutput;
use umide_agent::types::ImageSource;

const SERVER_NAME: &str = "umide-device";
/// A protocol version in Claude 2.x's accepted set (matches the permission server).
const PROTOCOL_VERSION: &str = "2025-06-18";

/// A running device-tools MCP server. Dropping it shuts the server thread down.
pub struct DeviceServer {
    port: u16,
    shutdown: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl DeviceServer {
    /// Bind an ephemeral localhost port and start serving. `serial` pins the
    /// target device (the panel's viewed device, `emulator-<port>`); `None`
    /// falls back to the first running Android serial per call.
    pub fn start(serial: Option<String>) -> std::io::Result<Self> {
        let server = tiny_http::Server::http("127.0.0.1:0")
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
        let shutdown = Arc::new(AtomicBool::new(false));
        let sd = shutdown.clone();
        let handle = std::thread::Builder::new()
            .name("umide-mcp-device".into())
            .spawn(move || serve(server, serial, sd))?;
        Ok(Self {
            port,
            shutdown,
            handle: Some(handle),
        })
    }

    /// This server's entry for the `--mcp-config` `mcpServers` map, as
    /// `"umide-device": { … }` (no braces) so it can be merged with the
    /// permission server's entry into one JSON object.
    pub fn mcp_config_entry(&self) -> String {
        format!(
            "\"{SERVER_NAME}\":{{\"type\":\"http\",\
             \"url\":\"http://127.0.0.1:{}/mcp\"}}",
            self.port
        )
    }

    /// The MCP-qualified name of a device tool, e.g.
    /// `mcp__umide-device__device_tap`.
    pub fn tool_ref(tool: &str) -> String {
        format!("mcp__{SERVER_NAME}__{tool}")
    }
}

impl Drop for DeviceServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // The serve loop uses recv_timeout; poke it so it doesn't wait it out.
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn serve(
    server: tiny_http::Server,
    serial: Option<String>,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::Relaxed) {
        match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(req)) => {
                let serial = serial.clone();
                // Own thread per request so a slow device call (uiautomator can
                // take a second or two) doesn't stall the accept loop.
                std::thread::spawn(move || handle_request(req, serial));
            }
            Ok(None) => continue,
            Err(_) => break,
        }
    }
}

fn handle_request(mut req: tiny_http::Request, serial: Option<String>) {
    let mut body = String::new();
    let _ = req.as_reader().read_to_string(&mut body);
    let msg: Value = serde_json::from_str(&body).unwrap_or(Value::Null);

    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = msg.get("id").cloned();

    let result: Option<Value> = match method {
        "initialize" => Some(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        })),
        "tools/list" => Some(json!({ "tools": tool_descriptors() })),
        "tools/call" => Some(handle_tools_call(&msg, serial.as_deref())),
        "ping" => Some(json!({})),
        _ => None,
    };

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

/// The tool catalogue advertised to Claude. `(cx,cy)` coordinates are device
/// pixels — the model gets them from `device_screenshot`/`describe_ui`.
fn tool_descriptors() -> Vec<Value> {
    let obj = |props: Value, required: Value| json!({ "type": "object", "properties": props, "required": required });
    let int = json!({ "type": "integer" });
    vec![
        json!({
            "name": "device_screenshot",
            "description": "Capture the current screen of the running Android device \
                            and return it as an image (the source of truth for what's \
                            on screen; coordinates are in device pixels).",
            "inputSchema": obj(json!({}), json!([])),
        }),
        json!({
            "name": "describe_ui",
            "description": "List the labelled/interactive on-screen elements with their \
                            tap centers (uiautomator). Use when pixels are ambiguous \
                            (custom-rendered UIs).",
            "inputSchema": obj(json!({}), json!([])),
        }),
        json!({
            "name": "device_tap",
            "description": "Tap the device at device-pixel (x, y).",
            "inputSchema": obj(json!({ "x": int, "y": int }), json!(["x", "y"])),
        }),
        json!({
            "name": "device_swipe",
            "description": "Swipe from (x1, y1) to (x2, y2) over duration_ms milliseconds \
                            (default 300). Use for scrolling and drags.",
            "inputSchema": obj(
                json!({ "x1": int, "y1": int, "x2": int, "y2": int, "duration_ms": int }),
                json!(["x1", "y1", "x2", "y2"]),
            ),
        }),
        json!({
            "name": "device_type",
            "description": "Type text into the focused field on the device.",
            "inputSchema": obj(
                json!({ "text": { "type": "string" } }),
                json!(["text"]),
            ),
        }),
        json!({
            "name": "device_key",
            "description": "Press a hardware/navigation key: home, back, recents, power, \
                            enter, tab, backspace, delete, up, down, left, right.",
            "inputSchema": obj(
                json!({ "key": { "type": "string" } }),
                json!(["key"]),
            ),
        }),
        json!({
            "name": "device_logs",
            "description": "Read the last N lines of logcat (default 100), optionally \
                            filtered (case-insensitive substring).",
            "inputSchema": obj(
                json!({ "lines": int, "filter": { "type": "string" } }),
                json!([]),
            ),
        }),
    ]
}

/// Execute one `tools/call` and return the MCP result body
/// (`{ "content": [...], "isError": bool }`).
fn handle_tools_call(msg: &Value, pinned: Option<&str>) -> Value {
    let name = msg
        .pointer("/params/name")
        .and_then(|n| n.as_str())
        .unwrap_or("");
    let args = msg
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // Resolve the target serial: the pinned (viewed) device, else first running.
    let serial = match pinned {
        Some(s) => s.to_string(),
        None => match super::super::android_serial() {
            Ok(s) => s,
            Err(e) => return error_result(&format!("no Android device: {e}")),
        },
    };

    let out = run_device_tool(name, &args, &serial);
    tooloutput_to_mcp(out)
}

fn run_device_tool(name: &str, args: &Value, serial: &str) -> ToolOutput {
    let i = |k: &str| args.get(k).and_then(|v| v.as_i64());
    match name {
        "device_screenshot" => super::super::android_screenshot(serial),
        "describe_ui" => super::super::android_describe_ui(serial),
        "device_logs" => {
            let lines = i("lines").unwrap_or(100).clamp(1, 1000);
            let filter = args.get("filter").and_then(|v| v.as_str()).unwrap_or("");
            super::super::android_logs(serial, lines, filter)
        }
        "device_tap" => match (i("x"), i("y")) {
            (Some(x), Some(y)) => super::super::adb_input(
                serial,
                &format!("input tap {x} {y}"),
                format!("tapped ({x},{y})"),
            ),
            _ => ToolOutput::error("device_tap needs integer x and y"),
        },
        "device_swipe" => match (i("x1"), i("y1"), i("x2"), i("y2")) {
            (Some(x1), Some(y1), Some(x2), Some(y2)) => {
                let dur = i("duration_ms").unwrap_or(300).clamp(50, 5000);
                super::super::adb_input(
                    serial,
                    &format!("input swipe {x1} {y1} {x2} {y2} {dur}"),
                    format!("swiped ({x1},{y1})→({x2},{y2})"),
                )
            }
            _ => ToolOutput::error("device_swipe needs x1,y1,x2,y2"),
        },
        "device_type" => match args.get("text").and_then(|v| v.as_str()) {
            Some(text) => {
                // adb `input text` uses %s for spaces; single-quote the rest.
                let escaped = text.replace('\'', "'\\''").replace(' ', "%s");
                super::super::adb_input(
                    serial,
                    &format!("input text '{escaped}'"),
                    format!("typed {} chars", text.chars().count()),
                )
            }
            None => ToolOutput::error("device_type needs a `text` string"),
        },
        "device_key" => match args.get("key").and_then(|v| v.as_str()) {
            Some(key) => match keycode(key) {
                Some(code) => super::super::adb_input(
                    serial,
                    &format!("input keyevent {code}"),
                    format!("pressed {key}"),
                ),
                None => ToolOutput::error(format!("unsupported key '{key}'")),
            },
            None => ToolOutput::error("device_key needs a `key` string"),
        },
        other => ToolOutput::error(format!("unknown device tool '{other}'")),
    }
}

/// Map a friendly key name to an Android `KEYCODE_*` (mirrors the built-in
/// agent's `press_key`, extended with the panel's hardware buttons).
fn keycode(key: &str) -> Option<&'static str> {
    Some(match key.to_ascii_lowercase().as_str() {
        "home" => "KEYCODE_HOME",
        "back" => "KEYCODE_BACK",
        "recents" | "apps" | "appswitch" => "KEYCODE_APP_SWITCH",
        "power" => "KEYCODE_POWER",
        "enter" | "return" => "KEYCODE_ENTER",
        "tab" => "KEYCODE_TAB",
        "backspace" | "delete" | "del" => "KEYCODE_DEL",
        "up" => "KEYCODE_DPAD_UP",
        "down" => "KEYCODE_DPAD_DOWN",
        "left" => "KEYCODE_DPAD_LEFT",
        "right" => "KEYCODE_DPAD_RIGHT",
        _ => return None,
    })
}

/// Convert a [`ToolOutput`] into an MCP `tools/call` result body.
fn tooloutput_to_mcp(out: ToolOutput) -> Value {
    let content: Vec<Value> = out
        .content
        .iter()
        .map(|c| match c {
            ToolResultContent::Text { text } => {
                json!({ "type": "text", "text": text })
            }
            ToolResultContent::Image {
                source: ImageSource::Base64 { media_type, data },
            } => json!({ "type": "image", "data": data, "mimeType": media_type }),
            // Device screenshots are always inline base64; a URL source (never
            // produced here) degrades to a text pointer rather than dropping it.
            ToolResultContent::Image {
                source: ImageSource::Url { url },
            } => json!({ "type": "text", "text": format!("[image: {url}]") }),
        })
        .collect();
    json!({ "content": content, "isError": out.is_error })
}

fn error_result(msg: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": msg }], "isError": true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertises_the_device_tools() {
        let names: Vec<String> = tool_descriptors()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        for expected in [
            "device_screenshot",
            "describe_ui",
            "device_tap",
            "device_swipe",
            "device_type",
            "device_key",
            "device_logs",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn keycode_maps_hardware_and_nav_keys() {
        assert_eq!(keycode("home"), Some("KEYCODE_HOME"));
        assert_eq!(keycode("Back"), Some("KEYCODE_BACK"));
        assert_eq!(keycode("recents"), Some("KEYCODE_APP_SWITCH"));
        assert_eq!(keycode("enter"), Some("KEYCODE_ENTER"));
        assert_eq!(keycode("nope"), None);
    }

    #[test]
    fn tool_ref_is_mcp_qualified() {
        assert_eq!(
            DeviceServer::tool_ref("device_tap"),
            "mcp__umide-device__device_tap"
        );
    }

    #[test]
    fn text_output_becomes_mcp_text_content() {
        let v = tooloutput_to_mcp(ToolOutput::ok("hello"));
        assert_eq!(v["isError"], false);
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][0]["text"], "hello");
    }

    /// End-to-end against a REAL running Android emulator: exercises the MCP
    /// surface over HTTP exactly as Claude Code would (`tools/list` +
    /// `tools/call`), and confirms the tools actually drive the device.
    /// Ignored in CI (needs a device); run with:
    ///   cargo test -p umide-app --lib -- --ignored live_device_mcp --nocapture
    #[test]
    #[ignore = "needs a running Android emulator"]
    fn live_device_mcp_drives_the_device() {
        use std::io::{Read, Write};

        let server = DeviceServer::start(None).expect("start device server");
        let port = server.port;
        // Minimal HTTP/1.1 client that handles the chunked bodies tiny_http emits
        // for large responses (e.g. a base64 screenshot). Byte-level so it's safe
        // for non-ASCII UI text.
        let call = |body: &str| -> Value {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            let req = format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: \
                 application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            s.write_all(req.as_bytes()).unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).unwrap();
            let pos = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
            let mut rest = &buf[pos + 4..];
            let json_bytes: Vec<u8> = if head.contains("transfer-encoding: chunked")
            {
                let mut out = Vec::new();
                while let Some(nl) = rest.windows(2).position(|w| w == b"\r\n") {
                    let n = usize::from_str_radix(
                        String::from_utf8_lossy(&rest[..nl]).trim(),
                        16,
                    )
                    .unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    let start = nl + 2;
                    let end = (start + n).min(rest.len());
                    out.extend_from_slice(&rest[start..end]);
                    rest = &rest[(end + 2).min(rest.len())..];
                }
                out
            } else {
                rest.to_vec()
            };
            serde_json::from_slice(&json_bytes).unwrap()
        };

        let list = call(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let n = list["result"]["tools"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        eprintln!("tools/list -> {n} device tools");
        assert!(n >= 7);

        let shot = call(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"device_screenshot","arguments":{}}}"#,
        );
        // with_image yields [text caption, image]; find the image item.
        let content = shot["result"]["content"].as_array().expect("content array");
        let img = content.iter().find(|c| c["type"] == "image");
        eprintln!(
            "device_screenshot -> {} items, image_b64_bytes={}, isError={}",
            content.len(),
            img.and_then(|c| c["data"].as_str())
                .map(|d| d.len())
                .unwrap_or(0),
            shot["result"]["isError"],
        );
        assert!(img.is_some(), "screenshot should return an image");
        assert_eq!(shot["result"]["isError"], false);

        let ui = call(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"describe_ui","arguments":{}}}"#,
        );
        let ui_txt = ui["result"]["content"][0]["text"].as_str().unwrap_or("");
        eprintln!(
            "describe_ui -> first line: {}",
            ui_txt.lines().next().unwrap_or("")
        );
        assert_eq!(ui["result"]["isError"], false);

        let tap = call(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"device_tap","arguments":{"x":540,"y":1200}}}"#,
        );
        eprintln!("device_tap -> {}", tap["result"]["content"][0]["text"]);
        assert_eq!(tap["result"]["isError"], false);
    }
}
