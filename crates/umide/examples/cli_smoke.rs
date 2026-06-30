//! Headless smoke test for the agent-CLI runner (P1, Claude Code).
//!
//! Drives `CliRunner` against the real `claude` CLI in the current directory,
//! printing each translated `AgentEvent`. Validates spawn → stdin prompt →
//! stream-json framing → parse → terminal event, with no UI.
//!
//! Run (uses your local `claude` auth; keep the prompt tiny — it spends tokens):
//!     cargo run -p umide-app --example cli_smoke -- "say hello in 3 words"

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use umide_agent::AgentEvent;
use umide_app::ai::cli::CliKind;
use umide_app::ai::cli::runner::CliRunner;
use umide_app::ai::{AgentRunner, CancelHandle, Push};

fn main() {
    let workspace = std::env::current_dir().expect("cwd");
    let prompt = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Say hello in exactly three words.".to_string());

    println!("→ workspace: {}", workspace.display());
    println!("→ prompt:    {prompt}\n");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let push = Push::new(|ev: AgentEvent| print_event(&ev));
    let cancel = CancelHandle::new(Arc::new(AtomicBool::new(false)));
    let session = Arc::new(Mutex::new(None));

    rt.block_on(async move {
        let mut runner = CliRunner::new(CliKind::ClaudeCode, workspace, session);
        runner.run(prompt, push, cancel).await;
    });

    println!("\n✓ runner returned");
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
