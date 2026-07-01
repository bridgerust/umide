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
use floem::ext_event::{ExtSendTrigger, register_ext_trigger};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use umide_agent::AgentEvent;

use super::claude::ClaudeParser;
use super::codex::CodexParser;
use super::device_server::DeviceServer;
use super::framer::CliFramer;
use super::gemini::GeminiParser;
use super::permission_server::PermissionServer;
use super::{CliKind, proc_group};
use crate::ai::{AgentRunner, ApprovalQueue, CancelHandle, Push};

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

/// Tools denied in the read-only fallback (execution + mutation, plus `Task`
/// so a subagent can't run them either). Space-separated; `--disallowedTools`
/// takes precedence over any saved allow-grant.
const READ_ONLY_DENY: &str = "Bash Edit Write MultiEdit NotebookEdit Task";

/// Mobile-first UMIDE framing prepended to the CLI agent's system prompt, so it
/// behaves like a native part of a mobile IDE — not a generic code agent that
/// happens to be pointed at a folder. Mirrors the built-in `SYSTEM_PROMPT`.
const UMIDE_CONTEXT: &str = "You are an AI agent working inside UMIDE, a \
Rust-based IDE for cross-platform MOBILE development (React Native and Flutter). \
The open project is a mobile app — prefer mobile-dev idioms, tooling, and \
workflows (components/screens/navigation, native modules, RN/Flutter build & \
reload). UMIDE embeds a live Android emulator (and, on macOS, an iOS simulator) \
directly in the editor, so changes are meant to be seen and tested on a real \
running device without leaving the IDE. When you change UI or behavior, think in \
that see→act→verify loop the way a mobile developer would: after a change, reload \
the app, look at the running screen, and check device logs to confirm the result. ";

/// Read-only-fallback note (used only when the approval bridge can't start), so
/// the agent advises instead of attempting edits/commands the headless default
/// mode then denies. Prefixed with [`UMIDE_CONTEXT`] at call time.
const READ_ONLY_NOTE: &str = "You are in read-only mode: read and search the \
project to answer questions and explain code, but you must NOT edit files or run \
shell commands. If a change is needed, describe it (show the diff or commands) \
for the developer to apply.";

/// Normal (write) mode note, so the agent knows its edits/commands are surfaced
/// to the developer for approval. Prefixed with [`UMIDE_CONTEXT`] at call time.
const WRITE_NOTE: &str = "You can read the project, edit files, and run commands \
— but every edit and command is shown to the developer for approval before it \
takes effect, so act normally and they will confirm. Reads are automatic.";

/// Appended when the emulator device-MCP tools are wired, so the agent knows it
/// can actually drive the running device (not just talk about it).
const DEVICE_NOTE: &str = " You can also drive the running Android emulator: use \
the umide-device tools — `device_screenshot` and `describe_ui` to SEE the \
screen, `device_tap`/`device_swipe`/`device_type`/`device_key` to interact, and \
`device_logs` to read logcat — to test your changes on the device and verify the \
result yourself.";

/// Gemini read-only tool whitelist: these run without confirmation; mutating
/// tools (write_file/replace/run_shell_command) then need a confirmation that
/// headless can't give, so they're effectively blocked.
const GEMINI_READ_TOOLS: &[&str] = &[
    "read_file",
    "read_many_files",
    "glob",
    "search_file_content",
    "list_directory",
    "google_web_search",
    "web_fetch",
];

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
    /// Approval cards for the CLI permission bridge (mutating tools).
    approvals: ApprovalQueue,
    /// Wakes the UI when an approval card is pushed.
    trigger: ExtSendTrigger,
    /// adb serial of the device the user is viewing (`None` ⇒ first running).
    /// Pins the device-MCP tools (Claude Code only) to that emulator.
    serial: Option<String>,
    idle_timeout: Duration,
}

impl CliRunner {
    pub fn new(
        kind: CliKind,
        workspace: PathBuf,
        session: Arc<Mutex<Option<String>>>,
        approvals: ApprovalQueue,
        trigger: ExtSendTrigger,
        serial: Option<String>,
    ) -> Self {
        Self {
            kind,
            workspace,
            session,
            approvals,
            trigger,
            serial,
            idle_timeout: DEFAULT_IDLE,
        }
    }

    fn make_parser(&self) -> Box<dyn CliParser> {
        match self.kind {
            CliKind::ClaudeCode => Box::new(ClaudeParser::new()),
            CliKind::Codex => Box::new(CodexParser::new()),
            CliKind::GeminiCli => Box::new(GeminiParser::new()),
        }
    }

    /// argv (the prompt is fed on stdin, not as an arg). `perm` is the running
    /// permission bridge; when present, Claude runs in normal mode and routes
    /// every mutating tool through UMIDE's approval queue. When it's `None`
    /// (the server couldn't bind), we fall back to a hard read-only mode so the
    /// agent can still read and advise but cannot mutate.
    fn build_args(
        &self,
        resume: Option<&str>,
        perm: Option<&PermissionServer>,
        dev: Option<&DeviceServer>,
    ) -> Vec<String> {
        match self.kind {
            CliKind::ClaudeCode => {
                let mut a = vec![
                    "--print".into(),
                    "--output-format".into(),
                    "stream-json".into(),
                    "--verbose".into(),
                ];
                match perm {
                    Some(p) => {
                        // Force default mode so every tool falls through to our
                        // prompt tool (acceptEdits/bypass in a user's config
                        // would otherwise skip the gate); --strict-mcp-config so
                        // only UMIDE's server is loaded.
                        a.push("--permission-mode".into());
                        a.push("default".into());
                        a.push("--mcp-config".into());
                        // ONE --mcp-config object (--strict-mcp-config loads only
                        // what's here): the permission bridge, plus the device
                        // tools when an emulator server is running.
                        a.push(match dev {
                            Some(d) => format!(
                                "{{\"mcpServers\":{{{},{}}}}}",
                                p.mcp_config_entry(),
                                d.mcp_config_entry()
                            ),
                            None => p.mcp_config_json(),
                        });
                        a.push("--strict-mcp-config".into());
                        a.push("--permission-prompt-tool".into());
                        a.push(p.tool_ref());
                        a.push("--append-system-prompt".into());
                        a.push(format!(
                            "{UMIDE_CONTEXT}{WRITE_NOTE}{}",
                            if dev.is_some() { DEVICE_NOTE } else { "" }
                        ));
                    }
                    None => {
                        // Read-only fallback (bridge couldn't bind). Deny the
                        // execution + mutation tools AND force default mode +
                        // strict MCP config, so a user's acceptEdits/bypass
                        // permissionMode or configured MCP tools can't mutate
                        // unprompted just because the approval bridge is down.
                        a.push("--permission-mode".into());
                        a.push("default".into());
                        a.push("--strict-mcp-config".into());
                        a.push("--disallowedTools".into());
                        a.push(READ_ONLY_DENY.into());
                        a.push("--append-system-prompt".into());
                        a.push(format!("{UMIDE_CONTEXT}{READ_ONLY_NOTE}"));
                    }
                }
                if let Some(id) = resume {
                    a.push("--resume".into());
                    a.push(id.into());
                }
                a
            }
            CliKind::Codex => {
                // Codex edits files and runs commands confined by the OS sandbox
                // (Apple Seatbelt on macOS, Landlock/seccomp on Linux):
                // `workspace-write` permits writes within the project dir (and
                // system temp) but blocks the rest of the filesystem and the
                // network — verified. `codex exec` has NO per-action approval
                // hook (the `-a` flag is rejected there), so the sandbox is the
                // boundary and the panel gates this behind an explicit
                // session-consent click. The prompt is fed on stdin.
                let mut a = vec!["exec".to_string()];
                if let Some(id) = resume {
                    a.push("resume".into());
                    a.push(id.into());
                }
                a.push("--json".into());
                a.push("--skip-git-repo-check".into());
                a.push("-C".into());
                a.push(self.workspace.to_string_lossy().into_owned());
                a.push("--sandbox".into());
                a.push("workspace-write".into());
                a
            }
            CliKind::GeminiCli => {
                // Read-only first cut: default approval mode + a read-tool
                // whitelist (those run without confirmation; mutating tools need
                // a confirmation headless can't give, so they're blocked). Prompt
                // is fed on stdin. Write mode (yolo + Docker sandbox) and
                // multi-turn `--resume` are follow-ups pending live verification.
                let mut a = vec![
                    "--output-format".to_string(),
                    "stream-json".into(),
                    "--approval-mode".into(),
                    "default".into(),
                    "--allowed-tools".into(),
                ];
                a.extend(GEMINI_READ_TOOLS.iter().map(|t| t.to_string()));
                a
            }
        }
    }
}

#[async_trait(?Send)]
impl AgentRunner for CliRunner {
    async fn run(&mut self, user_text: String, push: Push, cancel: CancelHandle) {
        let resume = self.session.lock().unwrap().clone();

        // Start the in-process approval bridge for Claude. Held for the whole
        // turn (Claude calls back into it); dropped at the end, which shuts it
        // down. If it can't bind, we fall back to read-only mode.
        let perm = if matches!(self.kind, CliKind::ClaudeCode) {
            let trigger = self.trigger;
            let notify: super::permission_server::Notify =
                Arc::new(move || register_ext_trigger(trigger));
            match PermissionServer::start(self.approvals.clone(), notify) {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::warn!(
                        "permission bridge unavailable ({e}); read-only fallback"
                    );
                    None
                }
            }
        } else {
            None
        };

        // Device-MCP tools (Claude only): let the agent drive the viewed emulator
        // (screenshot/tap/logs). Held for the whole turn, dropped at the end.
        // Best-effort — if it can't bind, the agent just runs without them.
        let dev = if matches!(self.kind, CliKind::ClaudeCode) {
            match DeviceServer::start(self.serial.clone()) {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::warn!("device MCP unavailable ({e}); no device tools");
                    None
                }
            }
        } else {
            None
        };
        let args = self.build_args(resume.as_deref(), perm.as_ref(), dev.as_ref());

        // Resolve the absolute path when possible (a GUI app's PATH may be
        // minimal); fall back to the bare name.
        let bin = which::which(self.kind.binary_name())
            .unwrap_or_else(|_| PathBuf::from(self.kind.binary_name()));

        // Configure on a std Command so we can set the process group, then move
        // to tokio. `build_std_command` runs npm `.cmd`/`.bat` shims through
        // `cmd /C` on Windows (a bare CreateProcess on a `.cmd` fails, os 193).
        let mut std_cmd = build_std_command(&bin, &args);
        std_cmd
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
                        if framer.overflowed() {
                            tracing::warn!(
                                "{}: dropped an oversized/torn record",
                                self.kind.label()
                            );
                        }
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
            Stop::Exited(s) => {
                // The exit can win the select! race before the final `result`
                // record is read — drain the rest of stdout so it isn't lost.
                drain_stdout(&mut stdout, &mut framer, parser.as_mut(), &push).await;
                (Some(s), Term::Normal)
            }
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

        // Bounded join (not abort) so the last stderr reads flush into the tail
        // before we build the error message. The child is gone by now, so its
        // stderr closes and the task ends promptly.
        if let Some(t) = stderr_task {
            let _ = tokio::time::timeout(Duration::from_millis(200), t).await;
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
/// so neither the child nor its sub-shells orphan. The grace is polled, not a
/// flat sleep, so a child that stops promptly returns without the full delay.
async fn kill_and_reap(pid: Option<u32>, child: &mut tokio::process::Child) {
    if let Some(pid) = pid {
        proc_group::kill_group(pid, false); // SIGTERM
        let step = Duration::from_millis(50);
        let mut waited = Duration::ZERO;
        while waited < GRACE {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            tokio::time::sleep(step).await;
            waited += step;
        }
        if matches!(child.try_wait(), Ok(None)) {
            proc_group::kill_group(pid, true); // SIGKILL
        }
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

/// Build the child command, routing npm `.cmd`/`.bat` shims through `cmd /C` on
/// Windows (a bare `CreateProcess` on a batch shim fails with os error 193, so
/// `claude`/`codex`/`gemini` installed via npm can't otherwise start there).
/// Rust ≥1.77 applies the correct batch-argument escaping for the trailing args.
fn build_std_command(
    bin: &std::path::Path,
    args: &[String],
) -> std::process::Command {
    #[cfg(windows)]
    {
        let is_batch = bin
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
            .unwrap_or(false);
        if is_batch {
            let mut c = std::process::Command::new("cmd");
            c.arg("/C").arg(bin).args(args);
            return c;
        }
    }
    let mut c = std::process::Command::new(bin);
    c.args(args);
    c
}

/// Read whatever stdout the process left buffered (up to EOF or a short bound)
/// and feed it through the framer/parser, so a `result` record that arrived
/// right as the process exited is not dropped.
async fn drain_stdout(
    stdout: &mut tokio::process::ChildStdout,
    framer: &mut CliFramer,
    parser: &mut dyn CliParser,
    push: &Push,
) {
    let mut buf = [0u8; 8192];
    loop {
        match tokio::time::timeout(Duration::from_millis(200), stdout.read(&mut buf))
            .await
        {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
            Ok(Ok(n)) => {
                let mut records = Vec::new();
                framer.push(&buf[..n], &mut records);
                for v in &records {
                    parser.on_record(v, push);
                }
            }
        }
    }
}
