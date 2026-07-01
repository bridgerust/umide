//! Headless smoke test for the Claude Code backend in READ-ONLY mode (P1a).
//!
//! Spawns the real `claude` with the read-only args and feeds each stream-json
//! line through `ClaudeParser`, printing the translated `AgentEvent`s. Confirms
//! the read pipeline (reads/answers) works and that mutation is refused.
//!
//! Run (uses your local `claude` auth; tiny token spend):
//!     cargo run -p umide-app --example cli_smoke -- "what does this crate do?"

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use umide_agent::AgentEvent;
use umide_app::ai::Push;
use umide_app::ai::cli::claude::ClaudeParser;
use umide_app::ai::cli::runner::CliParser;

fn main() {
    let workspace = std::env::current_dir().expect("cwd");
    let prompt = std::env::args().nth(1).unwrap_or_else(|| {
        "In one sentence, what does the umide_agent crate do? Read the code."
            .to_string()
    });
    println!("→ workspace: {}", workspace.display());
    println!("→ prompt:    {prompt}\n");

    // Mirrors CliRunner's read-only fallback args.
    let mut child = Command::new("claude")
        .args([
            "--print",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "default",
            "--strict-mcp-config",
            "--disallowedTools",
            "Bash Edit Write MultiEdit NotebookEdit Task",
        ])
        .current_dir(&workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn claude");

    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(prompt.as_bytes()).unwrap();
    drop(stdin);

    let push = Push::new(|ev: AgentEvent| print_event(&ev));
    let mut parser = ClaudeParser::new();
    let stdout = child.stdout.take().unwrap();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            parser.on_record(&v, &push);
        }
    }
    let _ = child.wait();
    println!("\n✓ session: {:?}", parser.take_session_id());
}

fn print_event(ev: &AgentEvent) {
    match ev {
        AgentEvent::TextDelta(t) => print!("{t}"),
        AgentEvent::ThinkingDelta(t) => eprintln!("[thinking] {t}"),
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
