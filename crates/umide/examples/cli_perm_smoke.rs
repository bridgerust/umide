//! Headless verification of the P1b permission bridge (Claude Code writes).
//!
//! Starts the in-process MCP permission server with an AUTO-APPROVER standing in
//! for the UI, spawns the real `claude` in write mode (--permission-prompt-tool
//! pointed at the server) inside a throwaway dir, and asks it to create a file.
//! Proves end-to-end that: Claude connects to the MCP server, calls the
//! permission tool for its mutating tool, the bridge routes + the decision flows
//! back, and the file actually gets written.
//!
//! Run (uses your local `claude` auth; tiny token spend):
//!     cargo run -p umide-app --example cli_perm_smoke

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use umide_app::ai::cli::permission_server::{Notify, PermissionServer};
use umide_app::ai::{ApprovalOutcome, ApprovalQueue};

fn main() {
    // Throwaway working dir.
    let dir = std::env::temp_dir()
        .join(format!("umide_perm_smoke_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let target = dir.join("hello.txt");
    println!("→ workdir: {}", dir.display());

    let approvals: ApprovalQueue = Arc::new(Mutex::new(VecDeque::new()));
    let approved = Arc::new(AtomicUsize::new(0));

    // Auto-approver: stands in for the UI. When the server pushes a card, drain
    // it and allow it (printing what was gated).
    let notify: Notify = {
        let approvals = approvals.clone();
        let approved = approved.clone();
        Arc::new(move || {
            let reqs: Vec<_> = approvals.lock().unwrap().drain(..).collect();
            for req in reqs {
                let first = req.detail.lines().next().unwrap_or("");
                println!("  [GATE] {} :: {first}", req.title);
                approved.fetch_add(1, Ordering::Relaxed);
                let _ = req.respond.send(ApprovalOutcome::Allowed);
            }
        })
    };

    let server = PermissionServer::start(approvals, notify).expect("server");
    let mcp_config = server.mcp_config_json();
    let tool_ref = server.tool_ref();
    println!("→ mcp:    {mcp_config}");
    println!("→ tool:   {tool_ref}\n");

    let mut child = Command::new("claude")
        .args([
            "--print",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "default",
            "--mcp-config",
            &mcp_config,
            "--strict-mcp-config",
            "--permission-prompt-tool",
            &tool_ref,
        ])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn claude");

    // Prompt on stdin, then close it.
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(
            b"Create a new file named hello.txt in the current directory whose \
              entire contents are exactly: hello from umide",
        )
        .unwrap();
    drop(stdin);

    // Echo a compact view of the event stream.
    let stdout = child.stdout.take().unwrap();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            match v.get("type").and_then(|t| t.as_str()) {
                Some("assistant") => {
                    if let Some(content) =
                        v.pointer("/message/content").and_then(|c| c.as_array())
                    {
                        for b in content {
                            match b.get("type").and_then(|t| t.as_str()) {
                                Some("text") => {
                                    if let Some(t) =
                                        b.get("text").and_then(|x| x.as_str())
                                    {
                                        if !t.trim().is_empty() {
                                            println!("  [text] {t}");
                                        }
                                    }
                                }
                                Some("tool_use") => {
                                    let n = b
                                        .get("name")
                                        .and_then(|x| x.as_str())
                                        .unwrap_or("?");
                                    println!("  [tool] {n}");
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Some("result") => {
                    let sub =
                        v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
                    println!("  [result] {sub}");
                }
                _ => {}
            }
        }
    }
    let _ = child.wait();

    // Verdict.
    println!("\n=== verdict ===");
    println!("approvals gated: {}", approved.load(Ordering::Relaxed));
    match std::fs::read_to_string(&target) {
        Ok(c) => {
            println!("hello.txt contents: {:?}", c.trim());
            if c.contains("hello from umide") {
                println!(
                    "✅ PASS — Claude wrote the file through the approval bridge"
                );
            } else {
                println!("⚠ file written but content unexpected");
            }
        }
        Err(_) => println!("❌ FAIL — hello.txt was not created"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}
