//! Headless smoke test for the Codex backend (P2, read-only).
//!
//! Spawns the real `codex exec --json` in a throwaway dir and feeds each event
//! line through `CodexParser`, printing the translated `AgentEvent`s. Validates
//! the read-only flow end-to-end: command_execution tool cards, agent_message
//! text, usage, and thread_id capture.
//!
//! Auth: inherits the environment, so point CODEX_HOME at a dir whose `codex
//! login` has model access, e.g.:
//!     CODEX_HOME=/path/to/keyed-home cargo run -p umide-app --example codex_smoke

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use umide_agent::AgentEvent;
use umide_app::ai::Push;
use umide_app::ai::cli::codex::CodexParser;
use umide_app::ai::cli::runner::CliParser;

fn main() {
    let dir = std::env::temp_dir()
        .join(format!("umide_codex_smoke_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("notes.txt"), "alpha\nbeta\n").unwrap();
    println!("→ workdir: {}", dir.display());

    let prompt = std::env::args().nth(1).unwrap_or_else(|| {
        "Read notes.txt and tell me the two words in it, then stop.".to_string()
    });

    let mut child = Command::new("codex")
        .args([
            "exec",
            "--json",
            "--skip-git-repo-check",
            "-C",
            &dir.to_string_lossy(),
            "--sandbox",
            "read-only",
        ])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn codex");

    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(prompt.as_bytes()).unwrap();
    drop(stdin);

    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let push = {
        let log = log.clone();
        Push::new(move |ev| {
            print_event(&ev);
            log.lock().unwrap().push(ev);
        })
    };
    let mut parser = CodexParser::new();

    let stdout = child.stdout.take().unwrap();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            parser.on_record(&v, &push);
        }
    }
    let _ = child.wait();

    let evs = log.lock().unwrap();
    let has_text = evs.iter().any(|e| matches!(e, AgentEvent::TextDelta(_)));
    let session = parser.take_session_id();
    println!("\n=== verdict ===");
    println!("events: {} | session: {:?}", evs.len(), session);
    if has_text && session.is_some() {
        println!("✅ PASS — Codex streamed events through the parser");
    } else {
        println!("⚠ no assistant text / session captured — check auth/output");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn print_event(ev: &AgentEvent) {
    match ev {
        AgentEvent::TextDelta(t) => println!("  [text] {t}"),
        AgentEvent::ThinkingDelta(t) => println!("  [reasoning] {t}"),
        AgentEvent::ToolCallStarted { name, .. } => println!("  [tool ▶ {name}]"),
        AgentEvent::ToolCallInput { name, input, .. } => {
            println!("  [tool input {name}] {input}")
        }
        AgentEvent::ToolResult {
            name, ok, summary, ..
        } => {
            println!("  [tool ✓ {name} ok={ok}] {summary}")
        }
        AgentEvent::TurnComplete { usage } => println!(
            "  [usage in={} out={} cache-read={}]",
            usage.input_tokens, usage.output_tokens, usage.cache_read_input_tokens
        ),
        AgentEvent::Done => println!("  [DONE]"),
        AgentEvent::Error(e) => println!("  [ERROR] {e}"),
    }
}
