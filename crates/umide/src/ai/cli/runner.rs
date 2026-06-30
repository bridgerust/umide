//! [`CliRunner`] — drives an external agent CLI as a child process and streams
//! its events into the panel's [`AgentEvent`] feed.
//!
//! The hard parts (which the simple "parse the JSON" framing hides) are all here:
//!   * spawn in the project dir, in its own process group, prompt fed on stdin;
//!   * drain stderr concurrently into a bounded tail (an undrained stderr pipe
//!     fills at ~64 KiB and deadlocks the child);
//!   * a `select!` over {stdout record, child exit, cancel, idle watchdog};
//!   * an idle watchdog that resets on *any* stdout/stderr byte (so a long
//!     `cargo build` the agent runs is not mistaken for a hang);
//!   * group-kill on cancel/idle (SIGTERM → grace → SIGKILL) so sub-shells don't
//!     orphan;
//!   * exactly one terminal `Done`/`Error`, sourced from how we stopped + the
//!     process exit — never from a `result` line (which only carries usage).

use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use umide_agent::AgentEvent;

use super::claude::ClaudeParser;
use super::framer::CliFramer;
use super::{CliKind, proc_group};
use crate::ai::{AgentRunner, CancelHandle, Push};

/// Per-CLI event parser: translate one JSON record into [`AgentEvent`]s and
/// expose the captured session id for multi-turn resume.
pub trait CliParser {
    fn on_record(&mut self, v: &Value, push: &Push);
    fn take_session_id(&self) -> Option<String>;
}

/// Max bytes for a single JSON record before the framer resyncs.
const FRAME_CAP: usize = 1 << 20;
/// Keep at most this much stderr for the error message.
const STDERR_TAIL_CAP: usize = 16 * 1024;
/// How often the watchdog wakes to check for idleness.
const WATCH_INTERVAL: Duration = Duration::from_secs(15);
/// Grace between SIGTERM and SIGKILL of the process group.
const GRACE: Duration = Duration::from_millis(1500);
/// Default idle ceiling: no stdout/stderr byte for this long ⇒ stop. Generous so
/// a legitimately long agent-run command (builds, test suites) is not killed.
const DEFAULT_IDLE: Duration = Duration::from_secs(360);

/// Tools denied in the read-only P1a phase (execution + mutation). Space-
/// separated; `--disallowedTools` takes precedence over any saved allow-grant.
const READ_ONLY_DENY: &str = "Bash Edit Write MultiEdit NotebookEdit";

/// Appended to Claude's system prompt in the read-only P1a phase, so it advises
/// instead of attempting edits/commands that the headless default mode denies.
const READ_ONLY_NOTE: &str = "You are running inside the UMIDE editor in \
read-only mode: you can read and search the project to answer questions and \
explain code, but you must NOT edit files or run shell commands. If a change is \
needed, describe it (show the diff or commands) for the developer to apply.";

/// Why the read loop stopped.
enum Stop {
    Exited(ExitStatus),
    Eof,
    Cancelled,
    Idle,
    ReadError,
}

enum Term {
    Normal,
    Cancelled,
    Idle,
}

pub struct CliRunner {
    kind: CliKind,
    workspace: PathBuf,
    /// Shared across turns so we can `--resume` the same conversation.
    session: Arc<Mutex<Option<String>>>,
    idle_timeout: Duration,
}

impl CliRunner {
    pub fn new(
        kind: CliKind,
        workspace: PathBuf,
        session: Arc<Mutex<Option<String>>>,
    ) -> Self {
        Self {
            kind,
            workspace,
            session,
            idle_timeout: DEFAULT_IDLE,
        }
    }

    fn make_parser(&self) -> Box<dyn CliParser> {
        match self.kind {
            CliKind::ClaudeCode => Box::new(ClaudeParser::new()),
            // Codex/Gemini parsers land in later phases; the panel only offers a
            // CLI once its backend is wired, so this fallback is never hit today.
            _ => Box::new(ClaudeParser::new()),
        }
    }

    /// argv (the prompt is fed on stdin, not as an arg).
    fn build_args(&self, resume: Option<&str>) -> Vec<String> {
        match self.kind {
            CliKind::ClaudeCode => {
                // P1a is read-only: the default headless permission mode (no
                // approver attached) denies Edit/Write/Bash, while Read/Grep/Glob
                // stay available — so the agent reads and advises but cannot
                // mutate. The system-prompt note makes that graceful (it won't
                // attempt edits only to be denied). Writes with per-action
                // approval arrive in P1b via the --permission-prompt-tool bridge
                // into UMIDE's ApprovalQueue. (Plan mode is avoided: it refuses
                // anything that isn't a planning task.)
                let mut a = vec![
                    "--print".into(),
                    "--output-format".into(),
                    "stream-json".into(),
                    "--verbose".into(),
                    // Hard read-only guarantee: deny the execution + mutation
                    // tools. `--disallowedTools` overrides any saved "always
                    // allow" grant in the project's settings, so behavior is the
                    // same on every machine (default mode alone is NOT read-only:
                    // a dev box with prior grants will auto-run Bash). Read/Grep/
                    // Glob/Web stay available.
                    "--disallowedTools".into(),
                    READ_ONLY_DENY.into(),
                    "--append-system-prompt".into(),
                    READ_ONLY_NOTE.into(),
                ];
                if let Some(id) = resume {
                    a.push("--resume".into());
                    a.push(id.into());
                }
                a
            }
            _ => Vec::new(),
        }
    }
}

#[async_trait(?Send)]
impl AgentRunner for CliRunner {
    async fn run(&mut self, user_text: String, push: Push, cancel: CancelHandle) {
        let resume = self.session.lock().unwrap().clone();
        let args = self.build_args(resume.as_deref());

        // Resolve the absolute path when possible (a GUI app's PATH may be
        // minimal); fall back to the bare name.
        let bin = which::which(self.kind.binary_name())
            .unwrap_or_else(|_| PathBuf::from(self.kind.binary_name()));

        // Configure on a std Command so we can set the process group, then move
        // to tokio.
        let mut std_cmd = std::process::Command::new(bin);
        std_cmd
            .args(&args)
            .current_dir(&self.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        proc_group::lead_new_group(&mut std_cmd);

        let mut cmd = tokio::process::Command::from(std_cmd);
        cmd.kill_on_drop(true);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                push.emit(AgentEvent::Error(format!(
                    "Could not start {}: {e}",
                    self.kind.label()
                )));
                return;
            }
        };
        let pid = child.id();

        // Feed the prompt, then close stdin (EOF) so the CLI starts working.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(user_text.as_bytes()).await;
            let _ = stdin.write_all(b"\n").await;
            drop(stdin);
        }

        let mut stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                push.emit(AgentEvent::Error(format!(
                    "{} produced no output stream.",
                    self.kind.label()
                )));
                return;
            }
        };

        // Concurrent stderr drain into a bounded tail; also feeds the watchdog.
        let stderr_tail = Arc::new(Mutex::new(Vec::<u8>::new()));
        let last_activity = Arc::new(Mutex::new(Instant::now()));
        let stderr_task = child.stderr.take().map(|mut stderr| {
            let tail = stderr_tail.clone();
            let act = last_activity.clone();
            tokio::spawn(async move {
                let mut b = [0u8; 4096];
                loop {
                    match stderr.read(&mut b).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            *act.lock().unwrap() = Instant::now();
                            let mut t = tail.lock().unwrap();
                            t.extend_from_slice(&b[..n]);
                            let over = t.len().saturating_sub(STDERR_TAIL_CAP);
                            if over > 0 {
                                t.drain(0..over);
                            }
                        }
                    }
                }
            })
        });

        let mut framer = CliFramer::new(FRAME_CAP);
        let mut parser = self.make_parser();
        let mut buf = [0u8; 8192];

        let stop = loop {
            tokio::select! {
                r = stdout.read(&mut buf) => match r {
                    Ok(0) => break Stop::Eof,
                    Ok(n) => {
                        *last_activity.lock().unwrap() = Instant::now();
                        let mut records = Vec::new();
                        framer.push(&buf[..n], &mut records);
                        for v in &records {
                            parser.on_record(v, &push);
                        }
                    }
                    Err(_) => break Stop::ReadError,
                },
                status = child.wait() => match status {
                    Ok(s) => break Stop::Exited(s),
                    Err(_) => break Stop::ReadError,
                },
                _ = cancel.cancelled() => break Stop::Cancelled,
                _ = tokio::time::sleep(WATCH_INTERVAL) => {
                    if last_activity.lock().unwrap().elapsed() > self.idle_timeout {
                        break Stop::Idle;
                    }
                }
            }
        };

        // Persist the session id (a partial/cancelled turn is still resumable).
        if let Some(sid) = parser.take_session_id() {
            *self.session.lock().unwrap() = Some(sid);
        }

        // Resolve the child + classify how we stopped.
        let (exit, term): (Option<ExitStatus>, Term) = match stop {
            Stop::Exited(s) => (Some(s), Term::Normal),
            Stop::Eof => (child.wait().await.ok(), Term::Normal),
            Stop::ReadError => {
                kill_and_reap(pid, &mut child).await;
                (None, Term::Normal)
            }
            Stop::Cancelled => {
                kill_and_reap(pid, &mut child).await;
                (None, Term::Cancelled)
            }
            Stop::Idle => {
                kill_and_reap(pid, &mut child).await;
                (None, Term::Idle)
            }
        };

        if let Some(t) = stderr_task {
            t.abort();
        }

        let stderr_msg = {
            let t = stderr_tail.lock().unwrap();
            String::from_utf8_lossy(&t).trim().to_string()
        };

        // Exactly one terminal event.
        match term {
            Term::Cancelled => push.emit(AgentEvent::Done), // user stop = clean
            Term::Idle => push.emit(AgentEvent::Error(format!(
                "{} stopped: no output for {}s.",
                self.kind.label(),
                self.idle_timeout.as_secs()
            ))),
            Term::Normal => {
                let ok = exit.map(|s| s.success()).unwrap_or(false);
                if ok {
                    push.emit(AgentEvent::Done);
                } else if !stderr_msg.is_empty() {
                    push.emit(AgentEvent::Error(stderr_msg));
                } else {
                    let why = match exit.and_then(|s| s.code()) {
                        Some(c) => {
                            format!("{} exited with code {c}.", self.kind.label())
                        }
                        None => format!("{} terminated.", self.kind.label()),
                    };
                    push.emit(AgentEvent::Error(why));
                }
            }
        }
    }
}

/// Terminate the child's process group (SIGTERM → grace → SIGKILL) and reap it,
/// so neither the child nor its sub-shells orphan.
async fn kill_and_reap(pid: Option<u32>, child: &mut tokio::process::Child) {
    if let Some(pid) = pid {
        proc_group::kill_group(pid, false);
        tokio::time::sleep(GRACE).await;
        proc_group::kill_group(pid, true);
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}
