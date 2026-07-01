//! Headless smoke test for the Gemini backend (P3, read-only).
//!
//! Spawns the real `gemini -o stream-json` in a throwaway dir and feeds each
//! event through `GeminiParser`, printing the translated `AgentEvent`s.
//!
//! Auth: inherits the environment — needs a Google login or an API key:
//!     GEMINI_API_KEY=... cargo run -p umide-app --example gemini_smoke

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use umide_agent::AgentEvent;
use umide_app::ai::Push;
use umide_app::ai::cli::gemini::GeminiParser;
use umide_app::ai::cli::runner::CliParser;

const READ_TOOLS: &[&str] = &[
    "read_file",
    "read_many_files",
    "glob",
    "search_file_content",
    "list_directory",
    "google_web_search",
    "web_fetch",
];

fn main() {
    let dir = std::env::temp_dir()
        .join(format!("umide_gemini_smoke_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("notes.txt"), "alpha\nbeta\n").unwrap();
    println!("→ workdir: {}", dir.display());

    let prompt = std::env::args().nth(1).unwrap_or_else(|| {
        "Read notes.txt and tell me the two words in it, then stop.".to_string()
    });

    let mut args: Vec<String> = vec![
        "--output-format".into(),
        "stream-json".into(),
        "--approval-mode".into(),
        "default".into(),
        "--allowed-tools".into(),
    ];
    args.extend(READ_TOOLS.iter().map(|t| t.to_string()));

    let mut child = Command::new("gemini")
        .args(&args)
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn gemini");

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
    let mut parser = GeminiParser::new();

    let stdout = child.stdout.take().unwrap();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            parser.on_record(&v, &push);
        }
    }
    let _ = child.wait();

    let evs = log.lock().unwrap();
    let has_text = evs.iter().any(|e| matches!(e, AgentEvent::TextDelta(_)));
    println!("\n=== verdict ===");
    println!(
        "events: {} | session: {:?}",
        evs.len(),
        parser.take_session_id()
    );
    if has_text {
        println!("✅ PASS — Gemini streamed events through the parser");
    } else {
        println!(
            "⚠ no assistant text — check auth (GEMINI_API_KEY / login) + output"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn print_event(ev: &AgentEvent) {
    match ev {
        AgentEvent::TextDelta(t) => print!("{t}"),
        AgentEvent::ThinkingDelta(t) => println!("\n[reasoning] {t}"),
        AgentEvent::ToolCallStarted { name, .. } => println!("\n[tool ▶ {name}]"),
        AgentEvent::ToolCallInput { name, input, .. } => {
            println!("[tool input {name}] {input}")
        }
        AgentEvent::ToolResult {
            name, ok, summary, ..
        } => {
            println!("[tool ✓ {name} ok={ok}] {summary}")
        }
        AgentEvent::TurnComplete { usage } => println!(
            "\n[usage in={} out={}]",
            usage.input_tokens, usage.output_tokens
        ),
        AgentEvent::Done => println!("\n[DONE]"),
        AgentEvent::Error(e) => println!("\n[ERROR] {e}"),
    }
}
