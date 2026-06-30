//! Editor-side glue for the AI assistant.
//!
//! This is where [`umide_agent`] (the UI-agnostic engine) meets the IDE. It
//! provides:
//!   * [`ReadOnlyTools`] — the first, safe tool surface (read/list/grep), bounded
//!     to the open workspace. No edits, no shell, no device control yet, so the
//!     assistant cannot change your code or seize the emulator — it can only
//!     read to answer questions.
//!   * [`spawn_turn`] — runs one agent turn on a dedicated worker thread (its own
//!     tokio runtime) and streams [`AgentEvent`]s back to the UI. The editor
//!     thread is never blocked, so you keep coding while the agent works.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use floem::ext_event::{ExtSendTrigger, register_ext_trigger};
use floem::prelude::SignalWith;
use lsp_types::{Position, Range, TextEdit, Url, WorkspaceEdit};
use tokio::sync::oneshot;
use umide_agent::tools::{ToolExecutor, ToolInvocation, ToolOutput};
use umide_agent::{
    Agent, AgentEvent, ContentBlock, Message, ProviderConfig, ProviderKind, ToolDef,
};

pub mod cli;

use crate::window_tab::WindowTabData;

/// Frozen system prompt (kept byte-stable so the prompt cache stays valid).
pub const SYSTEM_PROMPT: &str = "\
You are the UMIDE coding assistant, embedded in a Rust + Floem IDE for \
cross-platform mobile development (React Native and Flutter) that embeds the \
Android emulator and iOS simulator directly in the editor.

What you can do:
- Explore the workspace: `read_file`, `list_dir`, `grep`.
- Change code: `edit_file` (replace an exact, unique snippet) and `run_command` \
(build, test, lint, etc.). Both require the user to approve a diff or command \
before they take effect.
- Drive the running emulator/simulator to test your changes: `screenshot_device` \
to SEE the screen, `tap`, `swipe`, `type_text`, `press_key` to interact, and \
`read_logs` to read the device log. These work on both Android (adb) and iOS \
(simctl + idb); they auto-detect the running device, or pass `platform` \
(\"android\"/\"ios\") to choose.

Your superpower is the closed loop: after changing code and reloading the app, \
take a screenshot to see the result, interact with the UI to reproduce or \
verify the behavior, read logs to diagnose errors, then fix and re-verify — \
autonomously, the way a developer would.

Guidance:
- The emulator tools target the running device. If none is running, ask the \
user to start an Android emulator or iOS simulator in the Emulator panel. iOS \
input needs `idb` installed.
- Take a screenshot before interacting so you know the current screen and the \
element coordinates (device pixels, top-left origin).
- To reload a React Native or Flutter app, open the dev menu (press_key \
\"menu\", then screenshot and tap Reload), or use `run_command` with the \
project's reload command.
- Cite code as `path:line`. Lead with the answer, then the detail. Prefer \
reading and looking over guessing. The user can watch and take over the \
emulator at any time — keep your interactions purposeful.";

// ---------------------------------------------------------------------------
// API key storage (OS keychain)
// ---------------------------------------------------------------------------

const KEYCHAIN_SERVICE: &str = "dev.umide.app";

/// Per-provider keychain account, so each provider's key is stored separately.
fn keychain_user(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "anthropic-api-key",
        ProviderKind::OpenAi => "openai-api-key",
        ProviderKind::DeepSeek => "deepseek-api-key",
        ProviderKind::Gemini => "gemini-api-key",
    }
}

/// Load a stored API key for `kind` from the OS keychain, if present and
/// non-empty. The panel resolves keychain → provider env var.
pub fn load_api_key(kind: ProviderKind) -> Option<String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, keychain_user(kind))
        .ok()?
        .get_password()
        .ok()
        .filter(|k| !k.trim().is_empty())
}

/// Store an API key for `kind` in the OS keychain.
pub fn store_api_key(kind: ProviderKind, key: &str) -> Result<(), String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, keychain_user(kind))
        .map_err(|e| e.to_string())?
        .set_password(key.trim())
        .map_err(|e| e.to_string())
}

/// Shared queue of pending events. The worker pushes events and pulses the
/// Floem trigger; the UI drains the queue on its own thread.
pub type EventQueue = Arc<Mutex<VecDeque<AgentEvent>>>;

/// What a pending approval will do once the user accepts it.
#[derive(Clone)]
pub enum ApprovalKind {
    /// Run a shell command on the worker thread.
    Command,
    /// Apply a buffer-aware edit on the UI thread (so undo/cursor are kept).
    Edit {
        path: PathBuf,
        old_str: String,
        new_str: String,
    },
}

/// The result the UI sends back to the worker after the user decides.
pub enum ApprovalOutcome {
    Rejected,
    /// The user approved a command; the worker should run it.
    CommandApproved,
    /// The UI already applied (and saved) the edit.
    EditApplied,
    /// The UI tried to apply the edit but failed (e.g. the file changed).
    EditFailed(String),
}

/// A mutating action the agent wants to take, awaiting the user's decision.
/// `respond` is fulfilled when the user clicks Approve/Reject; the worker blocks
/// on it before any file is changed or any command runs.
pub struct ApprovalRequest {
    pub id: u64,
    pub title: String,
    pub detail: String,
    pub kind: ApprovalKind,
    pub respond: oneshot::Sender<ApprovalOutcome>,
}

pub type ApprovalQueue = Arc<Mutex<VecDeque<ApprovalRequest>>>;

/// A cheap, cloneable sink the runner uses to push events to the UI: it locks
/// the shared queue, appends the event, and pulses the Floem trigger. Boxed so
/// the LLM and CLI runners share one type and can clone it across tasks.
#[derive(Clone)]
pub struct Push(Arc<dyn Fn(AgentEvent) + Send + Sync>);

impl Push {
    pub fn new(f: impl Fn(AgentEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// Emit one event to the UI.
    pub fn emit(&self, ev: AgentEvent) {
        let f: &(dyn Fn(AgentEvent) + Send + Sync) = &*self.0;
        f(ev);
    }
}

/// A cancellation token shared with the UI's Stop button. Wraps the existing
/// `Arc<AtomicBool>` (which the panel sets directly) and adds an awaitable
/// [`CancelHandle::cancelled`], so a CLI runner's `select!` can react to a stop
/// while parked on child I/O — not only by polling between events.
#[derive(Clone)]
pub struct CancelHandle {
    flag: Arc<AtomicBool>,
}

impl CancelHandle {
    pub fn new(flag: Arc<AtomicBool>) -> Self {
        Self { flag }
    }

    /// The raw flag, for the LLM path (`Agent::send` takes `&AtomicBool`).
    pub fn flag(&self) -> &AtomicBool {
        &self.flag
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    /// Resolves once cancellation is observed. Polls the shared flag on a short
    /// interval so it composes in a `select!` and works with the panel's direct
    /// `flag.store(true)` without the UI signaling a separate primitive.
    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
    }
}

/// One assistant turn, abstracted over how it is fulfilled: UMIDE's own agentic
/// loop ([`LlmRunner`]) or an external agent CLI. Both stream the same
/// [`AgentEvent`]s through `push` and honor `cancel`, so the panel is identical.
///
/// `?Send`: runners are driven on a dedicated worker thread via a current-thread
/// runtime (see [`spawn_turn`]), so their futures need not be `Send`.
#[async_trait(?Send)]
pub trait AgentRunner {
    async fn run(&mut self, user_text: String, push: Push, cancel: CancelHandle);
}

/// The built-in path: drive [`umide_agent::Agent`] over the workspace tools,
/// gating every mutation through the [`ApprovalQueue`]. This is the original
/// `spawn_turn` body, unchanged in behavior — just lifted behind [`AgentRunner`].
pub struct LlmRunner {
    pub workspace: Option<PathBuf>,
    pub provider: ProviderConfig,
    pub history: Arc<Mutex<Vec<Message>>>,
    pub approvals: ApprovalQueue,
    pub trigger: ExtSendTrigger,
}

#[async_trait(?Send)]
impl AgentRunner for LlmRunner {
    async fn run(&mut self, user_text: String, push: Push, cancel: CancelHandle) {
        let tools: Arc<dyn ToolExecutor> = Arc::new(EditorTools::new(
            self.workspace.clone(),
            self.approvals.clone(),
            self.trigger,
        ));
        let seed = self.history.lock().unwrap().clone();
        let mut agent =
            match Agent::resume(self.provider.clone(), tools, SYSTEM_PROMPT, seed) {
                Ok(a) => a,
                Err(e) => {
                    push.emit(AgentEvent::Error(e.to_string()));
                    return;
                }
            };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        let content = vec![ContentBlock::text(user_text)];

        let send_fut = agent.send(content, tx, cancel.flag());
        let drain_fut = async {
            while let Some(ev) = rx.recv().await {
                push.emit(ev);
            }
        };
        let _ = tokio::join!(send_fut, drain_fut);

        // Persist updated history for the next turn.
        *self.history.lock().unwrap() = agent.history().to_vec();
    }
}

/// Run one agent turn off-thread, streaming events into `queue` and surfacing
/// approval requests into `approvals`; `trigger` wakes the UI for both. Setting
/// `cancel` aborts the turn (the in-flight request and the tool loop).
#[allow(clippy::too_many_arguments)]
pub fn spawn_turn(
    workspace: Option<PathBuf>,
    provider: ProviderConfig,
    history: Arc<Mutex<Vec<Message>>>,
    user_text: String,
    queue: EventQueue,
    approvals: ApprovalQueue,
    trigger: ExtSendTrigger,
    cancel: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name("umide-agent".into())
        .spawn(move || {
            let push = Push::new(move |ev: AgentEvent| {
                queue.lock().unwrap().push_back(ev);
                register_ext_trigger(trigger);
            });

            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    push.emit(AgentEvent::Error(format!("runtime: {e}")));
                    return;
                }
            };

            let cancel = CancelHandle::new(cancel);
            rt.block_on(async move {
                let mut runner = LlmRunner {
                    workspace,
                    provider,
                    history,
                    approvals,
                    trigger,
                };
                runner.run(user_text, push, cancel).await;
            });
        })
        .expect("spawn agent thread");
}

// ---------------------------------------------------------------------------
// Read-only tools, bounded to the open workspace
// ---------------------------------------------------------------------------

/// Max bytes returned by `read_file` (truncated past this).
const MAX_READ_BYTES: usize = 100 * 1024;
/// Max matches returned by `grep`.
const MAX_GREP_MATCHES: usize = 80;
/// Directories never traversed.
const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", "build", ".gradle"];

pub struct ReadOnlyTools {
    root: Option<PathBuf>,
}

impl ReadOnlyTools {
    pub fn new(root: Option<PathBuf>) -> Self {
        let root = root.and_then(|p| p.canonicalize().ok());
        Self { root }
    }

    /// Resolve a model-supplied path against the workspace root and refuse to
    /// escape it (no `..`, no absolute paths outside the tree, no symlink-out).
    fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| "no workspace is open".to_string())?;
        let candidate = if rel.is_empty() || rel == "." {
            root.clone()
        } else {
            root.join(rel)
        };
        let canonical = candidate
            .canonicalize()
            .map_err(|e| format!("{rel}: {e}"))?;
        if canonical.starts_with(root) {
            Ok(canonical)
        } else {
            Err(format!("{rel}: outside the workspace"))
        }
    }

    fn read_file(&self, input: &serde_json::Value) -> ToolOutput {
        let Some(rel) = input.get("path").and_then(|v| v.as_str()) else {
            return ToolOutput::error("read_file needs a `path` string");
        };
        let path = match self.resolve(rel) {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(e),
        };
        match std::fs::read_to_string(&path) {
            Ok(s) => ToolOutput {
                summary: format!("read {rel}"),
                content: vec![umide_agent::ToolResultContent::text(clip(
                    &s,
                    MAX_READ_BYTES,
                ))],
                is_error: false,
            },
            Err(e) => ToolOutput::error(format!("{rel}: {e}")),
        }
    }

    fn list_dir(&self, input: &serde_json::Value) -> ToolOutput {
        let rel = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let path = match self.resolve(rel) {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(e),
        };
        let read = match std::fs::read_dir(&path) {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(format!("{rel}: {e}")),
        };
        let mut entries: Vec<String> = read
            .filter_map(|e| e.ok())
            .map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if e.path().is_dir() {
                    format!("{name}/")
                } else {
                    name
                }
            })
            .collect();
        entries.sort();
        ToolOutput::ok(format!(
            "{} entries in {rel}\n{}",
            entries.len(),
            entries.join("\n")
        ))
    }

    fn grep(&self, input: &serde_json::Value) -> ToolOutput {
        let Some(pattern) = input.get("pattern").and_then(|v| v.as_str()) else {
            return ToolOutput::error("grep needs a `pattern` string");
        };
        let rel = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let base = match self.resolve(rel) {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(e),
        };
        let re = match regex::Regex::new(pattern) {
            Ok(re) => re,
            Err(e) => return ToolOutput::error(format!("bad regex: {e}")),
        };
        let root = self.root.clone().unwrap_or_else(|| base.clone());

        let mut out = Vec::new();
        let mut hit_limit = false;
        let mut stack = vec![base];
        while let Some(dir) = stack.pop() {
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in read.filter_map(|e| e.ok()) {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if path.is_dir() {
                    if !SKIP_DIRS.contains(&name.as_ref()) {
                        stack.push(path);
                    }
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let rel_path = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                for (i, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        out.push(format!("{rel_path}:{}: {}", i + 1, line.trim()));
                        if out.len() >= MAX_GREP_MATCHES {
                            hit_limit = true;
                            break;
                        }
                    }
                }
                if hit_limit {
                    break;
                }
            }
            if hit_limit {
                break;
            }
        }

        let mut report = format!("{} match(es) for /{pattern}/", out.len());
        if hit_limit {
            report.push_str(" (capped)");
        }
        report.push('\n');
        report.push_str(&out.join("\n"));
        ToolOutput::ok(report)
    }
}

#[async_trait]
impl ToolExecutor for ReadOnlyTools {
    fn specs(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "read_file".into(),
                description: "Read a UTF-8 text file from the open workspace. \
                    Call this when you need a file's exact contents."
                    .into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path relative to the workspace root." }
                    },
                    "required": ["path"]
                }),
                cache_control: None,
            },
            ToolDef {
                name: "list_dir".into(),
                description: "List the entries in a workspace directory \
                    (directories end with `/`)."
                    .into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory relative to the workspace root; defaults to the root." }
                    }
                }),
                cache_control: None,
            },
            ToolDef {
                name: "grep".into(),
                description: "Search workspace files for a Rust regex. Call this \
                    to locate where something is defined or used."
                    .into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Regular expression to search for." },
                        "path": { "type": "string", "description": "Subdirectory to search; defaults to the whole workspace." }
                    },
                    "required": ["pattern"]
                }),
                cache_control: None,
            },
        ]
    }

    async fn execute(&self, call: ToolInvocation) -> ToolOutput {
        match call.name.as_str() {
            "read_file" => self.read_file(&call.input),
            "list_dir" => self.list_dir(&call.input),
            "grep" => self.grep(&call.input),
            other => ToolOutput::error(format!("unknown tool: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Editor tools: read-only tools + approval-gated edit_file / run_command
// ---------------------------------------------------------------------------

/// Process-global so approval ids stay unique across turns (each turn builds a
/// fresh `EditorTools`).
static NEXT_APPROVAL_ID: AtomicU64 = AtomicU64::new(1);

const MAX_CMD_OUTPUT: usize = 16 * 1024;

/// The read-only tools plus `edit_file` and `run_command`. Every mutating
/// action pauses for an explicit Approve/Reject: the agent can never change a
/// file or run a command without the user clicking yes. This is the boundary
/// that keeps the IDE the developer's — not the agent's.
pub struct EditorTools {
    reader: ReadOnlyTools,
    approvals: ApprovalQueue,
    trigger: ExtSendTrigger,
}

impl EditorTools {
    pub fn new(
        root: Option<PathBuf>,
        approvals: ApprovalQueue,
        trigger: ExtSendTrigger,
    ) -> Self {
        Self {
            reader: ReadOnlyTools::new(root),
            approvals,
            trigger,
        }
    }

    /// Surface an approval card to the UI and block until the user decides.
    async fn request_approval(
        &self,
        title: String,
        detail: String,
        kind: ApprovalKind,
    ) -> ApprovalOutcome {
        let id = NEXT_APPROVAL_ID.fetch_add(1, Ordering::Relaxed);
        let (respond, rx) = oneshot::channel();
        self.approvals.lock().unwrap().push_back(ApprovalRequest {
            id,
            title,
            detail,
            kind,
            respond,
        });
        register_ext_trigger(self.trigger);
        // If the panel closes and drops the sender, default to "rejected".
        rx.await.unwrap_or(ApprovalOutcome::Rejected)
    }

    async fn edit_file(&self, input: &serde_json::Value) -> ToolOutput {
        let (Some(rel), Some(old), Some(new)) = (
            input.get("path").and_then(|v| v.as_str()),
            input.get("old_str").and_then(|v| v.as_str()),
            input.get("new_str").and_then(|v| v.as_str()),
        ) else {
            return ToolOutput::error(
                "edit_file needs `path`, `old_str`, and `new_str`",
            );
        };
        let path = match self.reader.resolve(rel) {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(e),
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("{rel}: {e}")),
        };
        match content.matches(old).count() {
            0 => return ToolOutput::error(format!("`old_str` not found in {rel}")),
            1 => {}
            n => {
                return ToolOutput::error(format!(
                    "`old_str` matches {n}× in {rel}; include more surrounding \
                     context so it is unique"
                ));
            }
        }
        let offset = content.find(old).unwrap();
        let line = content[..offset].matches('\n').count() + 1;

        // The edit is applied by the UI (buffer-aware) on approval, not here.
        let outcome = self
            .request_approval(
                format!("Edit {rel}:{line}"),
                diff_preview(old, new),
                ApprovalKind::Edit {
                    path,
                    old_str: old.to_string(),
                    new_str: new.to_string(),
                },
            )
            .await;
        match outcome {
            ApprovalOutcome::EditApplied => {
                ToolOutput::ok(format!("Applied edit to {rel}."))
            }
            ApprovalOutcome::EditFailed(e) => ToolOutput::error(e),
            ApprovalOutcome::Rejected => ToolOutput::ok(
                "The user rejected this edit. Consider a different approach.",
            ),
            ApprovalOutcome::CommandApproved => {
                ToolOutput::error("unexpected approval outcome for an edit")
            }
        }
    }

    async fn run_command(&self, input: &serde_json::Value) -> ToolOutput {
        let Some(cmd) = input.get("command").and_then(|v| v.as_str()) else {
            return ToolOutput::error("run_command needs a `command` string");
        };
        let outcome = self
            .request_approval(
                format!("Run: {cmd}"),
                cmd.to_string(),
                ApprovalKind::Command,
            )
            .await;
        match outcome {
            ApprovalOutcome::CommandApproved => {
                run_shell(cmd, self.reader.root.as_deref())
            }
            ApprovalOutcome::Rejected => {
                ToolOutput::ok("The user rejected this command.")
            }
            _ => ToolOutput::error("unexpected approval outcome for a command"),
        }
    }

    // --- Emulator/simulator driving (ungated, surfaced as tool cards) -------
    // Each tool auto-detects a running Android emulator or booted iOS simulator,
    // overridable with an optional "platform" arg. Android uses adb; iOS uses
    // simctl (screenshot/logs) and idb (input).

    fn screenshot_device(&self, input: &serde_json::Value) -> ToolOutput {
        match resolve_target(input) {
            Ok(Target::Android(serial)) => android_screenshot(&serial),
            Ok(Target::Ios(udid)) => ios_screenshot(&udid),
            Err(e) => ToolOutput::error(e),
        }
    }

    fn tap(&self, input: &serde_json::Value) -> ToolOutput {
        let (Some(x), Some(y)) = (
            input.get("x").and_then(|v| v.as_i64()),
            input.get("y").and_then(|v| v.as_i64()),
        ) else {
            return ToolOutput::error("tap needs integer `x` and `y`");
        };
        let summary = format!("tap ({x}, {y})");
        match resolve_target(input) {
            Ok(Target::Android(serial)) => {
                adb_input(&serial, &format!("input tap {x} {y}"), summary)
            }
            Ok(Target::Ios(udid)) => idb_run(
                &udid,
                &["ui", "tap", &x.to_string(), &y.to_string()],
                summary,
            ),
            Err(e) => ToolOutput::error(e),
        }
    }

    fn swipe(&self, input: &serde_json::Value) -> ToolOutput {
        let coords: Option<Vec<i64>> = ["x1", "y1", "x2", "y2"]
            .iter()
            .map(|k| input.get(*k).and_then(|v| v.as_i64()))
            .collect();
        let Some(c) = coords else {
            return ToolOutput::error("swipe needs integer `x1`,`y1`,`x2`,`y2`");
        };
        let dur = input
            .get("duration_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or(300)
            .clamp(50, 5000);
        let summary = format!("swipe ({},{})→({},{})", c[0], c[1], c[2], c[3]);
        match resolve_target(input) {
            Ok(Target::Android(serial)) => adb_input(
                &serial,
                &format!("input swipe {} {} {} {} {dur}", c[0], c[1], c[2], c[3]),
                summary,
            ),
            Ok(Target::Ios(udid)) => {
                let secs = format!("{:.2}", dur as f64 / 1000.0);
                idb_run(
                    &udid,
                    &[
                        "ui",
                        "swipe",
                        &c[0].to_string(),
                        &c[1].to_string(),
                        &c[2].to_string(),
                        &c[3].to_string(),
                        "--duration",
                        &secs,
                    ],
                    summary,
                )
            }
            Err(e) => ToolOutput::error(e),
        }
    }

    fn type_text(&self, input: &serde_json::Value) -> ToolOutput {
        let Some(text) = input.get("text").and_then(|v| v.as_str()) else {
            return ToolOutput::error("type_text needs a `text` string");
        };
        let summary = format!("typed {} chars", text.chars().count());
        match resolve_target(input) {
            Ok(Target::Android(serial)) => {
                // adb `input text` uses %s for spaces; single-quote the rest.
                let escaped = text.replace('\'', "'\\''").replace(' ', "%s");
                adb_input(&serial, &format!("input text '{escaped}'"), summary)
            }
            Ok(Target::Ios(udid)) => idb_run(&udid, &["ui", "text", text], summary),
            Err(e) => ToolOutput::error(e),
        }
    }

    fn press_key(&self, input: &serde_json::Value) -> ToolOutput {
        let Some(key) = input.get("key").and_then(|v| v.as_str()) else {
            return ToolOutput::error("press_key needs a `key` string");
        };
        match resolve_target(input) {
            Ok(Target::Android(serial)) => {
                let code = match key.to_ascii_lowercase().as_str() {
                    "back" => "KEYCODE_BACK",
                    "home" => "KEYCODE_HOME",
                    "enter" => "KEYCODE_ENTER",
                    "menu" => "KEYCODE_MENU",
                    "recents" | "app_switch" => "KEYCODE_APP_SWITCH",
                    "tab" => "KEYCODE_TAB",
                    "del" | "backspace" => "KEYCODE_DEL",
                    "power" => "KEYCODE_POWER",
                    _ => key, // raw KEYCODE_* name or numeric code
                };
                adb_input(
                    &serial,
                    &format!("input keyevent {code}"),
                    format!("key {key}"),
                )
            }
            Ok(Target::Ios(udid)) => ios_press_key(&udid, key),
            Err(e) => ToolOutput::error(e),
        }
    }

    fn read_logs(&self, input: &serde_json::Value) -> ToolOutput {
        let lines = input
            .get("lines")
            .and_then(|v| v.as_i64())
            .unwrap_or(120)
            .clamp(1, 1000);
        let filter = input.get("filter").and_then(|v| v.as_str()).unwrap_or("");
        match resolve_target(input) {
            Ok(Target::Android(serial)) => android_logs(&serial, lines, filter),
            Ok(Target::Ios(udid)) => ios_logs(&udid, lines, filter),
            Err(e) => ToolOutput::error(e),
        }
    }
}

#[async_trait]
impl ToolExecutor for EditorTools {
    fn specs(&self) -> Vec<ToolDef> {
        let mut specs = self.reader.specs();
        specs.push(ToolDef {
            name: "edit_file".into(),
            description: "Edit a workspace file by replacing an exact, unique \
                snippet (`old_str` must occur exactly once). The user must \
                approve the diff before anything is written to disk."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to edit, relative to the workspace root." },
                    "old_str": { "type": "string", "description": "Exact text to replace; must occur exactly once in the file." },
                    "new_str": { "type": "string", "description": "Replacement text." }
                },
                "required": ["path", "old_str", "new_str"]
            }),
            cache_control: None,
        });
        specs.push(ToolDef {
            name: "run_command".into(),
            description: "Run a shell command in the workspace root (e.g. a build \
                or test command). The user must approve before it runs; stdout, \
                stderr, and the exit code are returned."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to run." }
                },
                "required": ["command"]
            }),
            cache_control: None,
        });
        specs.extend(device_specs());
        specs
    }

    async fn execute(&self, call: ToolInvocation) -> ToolOutput {
        match call.name.as_str() {
            "read_file" | "list_dir" | "grep" => self.reader.execute(call).await,
            "edit_file" => self.edit_file(&call.input).await,
            "run_command" => self.run_command(&call.input).await,
            "screenshot_device" => self.screenshot_device(&call.input),
            "tap" => self.tap(&call.input),
            "swipe" => self.swipe(&call.input),
            "type_text" => self.type_text(&call.input),
            "press_key" => self.press_key(&call.input),
            "read_logs" => self.read_logs(&call.input),
            other => ToolOutput::error(format!("unknown tool: {other}")),
        }
    }
}

/// Tool schemas for the emulator/simulator driving tools (Android + iOS).
fn device_specs() -> Vec<ToolDef> {
    let platform = || {
        serde_json::json!({
            "type": "string",
            "enum": ["android", "ios"],
            "description": "Target device; defaults to whichever is running."
        })
    };
    vec![
        ToolDef {
            name: "screenshot_device".into(),
            description: "Capture the running emulator/simulator screen and \
                return it as an image so you can SEE the app's current state."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "platform": platform() }
            }),
            cache_control: None,
        },
        ToolDef {
            name: "tap".into(),
            description: "Tap the device at device-pixel coordinates (top-left \
                origin). Screenshot first to find coordinates."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "platform": platform()
                },
                "required": ["x", "y"]
            }),
            cache_control: None,
        },
        ToolDef {
            name: "swipe".into(),
            description: "Swipe/scroll on the device from (x1,y1) to (x2,y2) \
                over `duration_ms`."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "x1": { "type": "integer" },
                    "y1": { "type": "integer" },
                    "x2": { "type": "integer" },
                    "y2": { "type": "integer" },
                    "duration_ms": { "type": "integer", "description": "Defaults to 300." },
                    "platform": platform()
                },
                "required": ["x1", "y1", "x2", "y2"]
            }),
            cache_control: None,
        },
        ToolDef {
            name: "type_text".into(),
            description: "Type text into the currently focused field on the \
                device."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "platform": platform()
                },
                "required": ["text"]
            }),
            cache_control: None,
        },
        ToolDef {
            name: "press_key".into(),
            description: "Press a key/button. Android: back, home, enter, menu, \
                recents, tab, backspace, power, or a raw KEYCODE_* name. iOS: \
                home, enter, backspace, tab, lock."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string" },
                    "platform": platform()
                },
                "required": ["key"]
            }),
            cache_control: None,
        },
        ToolDef {
            name: "read_logs".into(),
            description: "Read recent device logs (Android logcat or iOS system \
                log) to diagnose crashes or errors. Optionally filter \
                (case-insensitive substring)."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "lines": { "type": "integer", "description": "How many recent lines (default 120, max 1000)." },
                    "filter": { "type": "string", "description": "Only lines containing this text." },
                    "platform": platform()
                }
            }),
            cache_control: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// Device helpers: Android (adb) and iOS (simctl + idb)
// ---------------------------------------------------------------------------

/// Which device a tool call targets.
enum Target {
    Android(String), // adb serial, e.g. emulator-5554
    Ios(String),     // simulator UDID
}

/// Resolve the target from an optional `platform` arg, else auto-detect a
/// running Android emulator first, then a booted iOS simulator.
fn resolve_target(input: &serde_json::Value) -> Result<Target, String> {
    match input.get("platform").and_then(|v| v.as_str()) {
        Some("android") => android_serial().map(Target::Android),
        Some("ios") => ios_udid().map(Target::Ios),
        Some(other) => Err(format!(
            "unknown platform '{other}' (use 'android' or 'ios')"
        )),
        None => {
            if let Ok(s) = android_serial() {
                return Ok(Target::Android(s));
            }
            if let Ok(u) = ios_udid() {
                return Ok(Target::Ios(u));
            }
            Err(
                "no running Android emulator or booted iOS simulator — start \
                 one in the Emulator panel"
                    .to_string(),
            )
        }
    }
}

/// PATH augmented with Homebrew, the Android SDK platform-tools, and the usual
/// idb install locations so `adb`/`idb`/`xcrun` resolve regardless of the shell.
fn tool_path_env() -> String {
    let base = std::env::var("PATH").unwrap_or_default();
    let home = std::env::var("HOME").unwrap_or_default();
    format!(
        "/opt/homebrew/bin:/usr/local/bin:{home}/.local/bin:\
         {home}/Library/Android/sdk/platform-tools:\
         {home}/Android/Sdk/platform-tools:{base}"
    )
}

/// Build a Command that runs `cmd` through the platform shell
/// (`cmd /C` on Windows, `sh -c` elsewhere), with tool paths on PATH.
fn shell_command(cmd: &str) -> std::process::Command {
    #[cfg(windows)]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(cmd);
        c
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    };
    command.env("PATH", tool_path_env());
    command
}

fn adb_sh(cmd: &str) -> std::io::Result<std::process::Output> {
    shell_command(cmd).output()
}

/// Single-quote a value for safe inclusion in a `sh -c` command.
fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Serial of the first running Android device (e.g. `emulator-5554`).
fn android_serial() -> Result<String, String> {
    let out = adb_sh("adb devices").map_err(|e| {
        format!("could not run adb ({e}); is the Android SDK installed?")
    })?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines().skip(1) {
        let mut parts = line.split_whitespace();
        if let (Some(serial), Some(state)) = (parts.next(), parts.next()) {
            if state == "device" {
                return Ok(serial.to_string());
            }
        }
    }
    Err("no running Android device".to_string())
}

/// UDID of the first booted iOS simulator.
fn ios_udid() -> Result<String, String> {
    let out = adb_sh("xcrun simctl list devices booted")
        .map_err(|e| format!("could not run simctl ({e}); is Xcode installed?"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    // Lines look like: "    iPhone 15 (UDID) (Booted)"
    for line in text.lines() {
        if line.contains("Booted") {
            if let (Some(open), Some(close)) = (line.find('('), line.find(')')) {
                if open < close {
                    return Ok(line[open + 1..close].trim().to_string());
                }
            }
        }
    }
    Err("no booted iOS simulator".to_string())
}

/// Run `adb -s <serial> shell <cmd>` and report success/failure for a card.
fn adb_input(serial: &str, shell_cmd: &str, summary: String) -> ToolOutput {
    match adb_sh(&format!("adb -s {serial} shell {shell_cmd}")) {
        Ok(out) if out.status.success() => ToolOutput::ok(summary),
        Ok(out) => ToolOutput::error(format!(
            "{summary} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )),
        Err(e) => ToolOutput::error(format!("{summary}: {e}")),
    }
}

/// Run `idb <args...> --udid <udid>` for iOS input.
fn idb_run(udid: &str, args: &[&str], summary: String) -> ToolOutput {
    let joined: String = args.iter().map(|a| shq(a)).collect::<Vec<_>>().join(" ");
    match adb_sh(&format!("idb {joined} --udid {udid}")) {
        Ok(out) if out.status.success() => ToolOutput::ok(summary),
        Ok(out) => ToolOutput::error(format!(
            "{summary} failed: {} (is idb installed? `brew install idb-companion` + `pipx install fb-idb`)",
            String::from_utf8_lossy(&out.stderr)
        )),
        Err(e) => ToolOutput::error(format!("{summary}: {e}")),
    }
}

fn android_screenshot(serial: &str) -> ToolOutput {
    match adb_sh(&format!("adb -s {serial} exec-out screencap -p")) {
        Ok(out) if out.status.success() && !out.stdout.is_empty() => {
            ToolOutput::with_image(format!("screenshot of {serial}"), &out.stdout)
        }
        Ok(out) => ToolOutput::error(format!(
            "screencap failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )),
        Err(e) => ToolOutput::error(format!("screencap: {e}")),
    }
}

fn ios_screenshot(udid: &str) -> ToolOutput {
    // simctl writes to a file; capture to a temp path then read the bytes.
    let tmp = std::env::temp_dir().join(format!("umide-shot-{udid}.png"));
    let cmd = format!(
        "xcrun simctl io {udid} screenshot --type=png {}",
        shq(&tmp.display().to_string())
    );
    match adb_sh(&cmd) {
        Ok(out) if out.status.success() => match std::fs::read(&tmp) {
            Ok(bytes) if !bytes.is_empty() => {
                let _ = std::fs::remove_file(&tmp);
                ToolOutput::with_image(format!("screenshot of {udid}"), &bytes)
            }
            _ => ToolOutput::error("simctl screenshot produced no image"),
        },
        Ok(out) => ToolOutput::error(format!(
            "simctl screenshot failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )),
        Err(e) => ToolOutput::error(format!("simctl: {e}")),
    }
}

fn ios_press_key(udid: &str, key: &str) -> ToolOutput {
    // idb buttons / HID usage codes for common keys.
    match key.to_ascii_lowercase().as_str() {
        "home" => idb_run(udid, &["ui", "button", "HOME"], "key home".into()),
        "lock" | "power" => {
            idb_run(udid, &["ui", "button", "LOCK"], "key lock".into())
        }
        "siri" => idb_run(udid, &["ui", "button", "SIRI"], "key siri".into()),
        "enter" => idb_run(udid, &["ui", "key", "40"], "key enter".into()),
        "tab" => idb_run(udid, &["ui", "key", "43"], "key tab".into()),
        "del" | "backspace" => {
            idb_run(udid, &["ui", "key", "42"], "key backspace".into())
        }
        other => ToolOutput::error(format!(
            "key '{other}' is not supported on iOS (try home, enter, backspace, tab, lock)"
        )),
    }
}

fn android_logs(serial: &str, lines: i64, filter: &str) -> ToolOutput {
    let mut cmd = format!("adb -s {serial} logcat -d -t {lines}");
    if !filter.is_empty() {
        cmd.push_str(&format!(" | grep -i {}", shq(filter)));
    }
    match adb_sh(&cmd) {
        Ok(out) => ToolOutput::ok(format!(
            "logcat (last {lines} lines)\n{}",
            clip(&String::from_utf8_lossy(&out.stdout), MAX_CMD_OUTPUT)
        )),
        Err(e) => ToolOutput::error(format!("logcat: {e}")),
    }
}

fn ios_logs(udid: &str, lines: i64, filter: &str) -> ToolOutput {
    let mut cmd = format!(
        "xcrun simctl spawn {udid} log show --last 1m --style compact 2>/dev/null"
    );
    if !filter.is_empty() {
        cmd.push_str(&format!(" | grep -i {}", shq(filter)));
    }
    cmd.push_str(&format!(" | tail -n {lines}"));
    match adb_sh(&cmd) {
        Ok(out) => ToolOutput::ok(format!(
            "iOS log (last 1m, ≤{lines} lines)\n{}",
            clip(&String::from_utf8_lossy(&out.stdout), MAX_CMD_OUTPUT)
        )),
        Err(e) => ToolOutput::error(format!("simctl log: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Buffer-aware edit application (UI thread)
// ---------------------------------------------------------------------------

/// Apply an approved edit. If the file is open in the editor, the edit goes
/// through the document so **undo history and cursor are preserved** and the
/// open buffer updates in place, then it's saved. If the file isn't open, it's
/// written to disk directly (there's no editor state to keep).
///
/// Reading the file again here is also a staleness check: if it changed since
/// the agent read it, `old_str` won't match uniquely and we refuse safely.
/// Must be called on the UI thread.
pub fn apply_edit_in_editor(
    window_tab_data: &WindowTabData,
    path: &Path,
    old: &str,
    new: &str,
) -> Result<(), String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    match content.matches(old).count() {
        1 => {}
        0 => {
            return Err(
                "the file changed since it was read (the snippet is no longer \
                 present) — re-read it and try again"
                    .into(),
            );
        }
        n => {
            return Err(format!(
                "the snippet now matches {n}× (the file changed since it was \
                 read) — re-read it and make the match unique"
            ));
        }
    }
    let offset = content.find(old).unwrap();
    let start = lsp_position(&content, offset);
    let end = lsp_position(&content, offset + old.len());
    let url =
        Url::from_file_path(path).map_err(|_| "invalid file path".to_string())?;
    let mut changes = HashMap::new();
    changes.insert(
        url,
        vec![TextEdit {
            range: Range { start, end },
            new_text: new.to_string(),
        }],
    );
    let edit = WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    };

    let was_open = window_tab_data
        .main_split
        .docs
        .with_untracked(|docs| docs.contains_key(path));
    if was_open {
        // Undo-preserving edit through the open document, then persist.
        window_tab_data.main_split.apply_workspace_edit(&edit);
        if let Some(doc) = window_tab_data
            .main_split
            .docs
            .with_untracked(|docs| docs.get(path).cloned())
        {
            doc.save(|| {});
        }
        Ok(())
    } else {
        let new_content = content.replacen(old, new, 1);
        std::fs::write(path, new_content)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        Ok(())
    }
}

/// Byte offset → LSP `Position` (character is a UTF-16 code-unit count, per LSP).
fn lsp_position(content: &str, byte_offset: usize) -> Position {
    let prefix = &content[..byte_offset];
    let line = prefix.matches('\n').count() as u32;
    let line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = content[line_start..byte_offset].encode_utf16().count() as u32;
    Position { line, character }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(out: &ToolOutput) -> String {
        out.content
            .iter()
            .filter_map(|c| match c {
                umide_agent::ToolResultContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn read_file_reads_within_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello world").unwrap();
        let tools = ReadOnlyTools::new(Some(dir.path().to_path_buf()));
        let out = tools.read_file(&serde_json::json!({"path": "a.txt"}));
        assert!(!out.is_error);
        assert!(text_of(&out).contains("hello world"));
    }

    #[test]
    fn read_file_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(dir.path().join("secret.txt"), "TOPSECRET").unwrap();
        let tools = ReadOnlyTools::new(Some(root));
        let out = tools.read_file(&serde_json::json!({"path": "../secret.txt"}));
        assert!(out.is_error);
        assert!(!text_of(&out).contains("TOPSECRET"));
    }

    #[test]
    fn list_dir_marks_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f1.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let tools = ReadOnlyTools::new(Some(dir.path().to_path_buf()));
        let t = text_of(&tools.list_dir(&serde_json::json!({"path": "."})));
        assert!(t.contains("f1.txt"));
        assert!(t.contains("sub/"));
    }

    #[test]
    fn grep_reports_file_and_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn foo() {}\nfn bar() {}\n")
            .unwrap();
        let tools = ReadOnlyTools::new(Some(dir.path().to_path_buf()));
        let t = text_of(&tools.grep(&serde_json::json!({"pattern": "fn \\w+"})));
        assert!(t.contains("code.rs:1"));
        assert!(t.contains("foo"));
    }

    #[test]
    fn clip_truncates_on_char_boundary() {
        let s = "é".repeat(100); // 2 bytes each → 200 bytes
        let clipped = clip(&s, 51); // odd byte index lands mid-char
        assert!(clipped.ends_with("[truncated]"));
        // No panic = the kept prefix was a valid UTF-8 boundary.
    }

    #[test]
    fn diff_preview_marks_old_and_new() {
        let d = diff_preview("old line", "new line");
        assert!(d.contains("- old line"));
        assert!(d.contains("+ new line"));
    }

    #[test]
    fn shq_quotes_and_escapes_single_quotes() {
        assert_eq!(shq("hello"), "'hello'");
        assert_eq!(shq("a b"), "'a b'");
        // a single quote is closed, backslash-escaped, then reopened
        assert_eq!(shq("it's"), "'it'\\''s'");
    }

    #[test]
    fn lsp_position_counts_utf16_units() {
        let content = "héllo\nworld"; // é is 2 bytes / 1 UTF-16 unit
        let p = lsp_position(content, 3); // byte 3 = first 'l'
        assert_eq!(p.line, 0);
        assert_eq!(p.character, 2); // h, é → 2 units
        let p2 = lsp_position(content, 7); // byte 7 = 'w' on line 1
        assert_eq!(p2.line, 1);
        assert_eq!(p2.character, 0);
    }
}

/// A compact unified-style diff preview for an approval card.
fn diff_preview(old: &str, new: &str) -> String {
    let mut out = String::new();
    for line in old.lines().take(30) {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
    }
    for line in new.lines().take(30) {
        out.push_str("+ ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn run_shell(cmd: &str, cwd: Option<&Path>) -> ToolOutput {
    let mut command = shell_command(cmd);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    match output_with_timeout(command, std::time::Duration::from_secs(300)) {
        Ok(out) => {
            let code = out.status.code().unwrap_or(-1);
            let mut body = format!("exit: {code}\n");
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !stdout.trim().is_empty() {
                body.push_str("stdout:\n");
                body.push_str(&clip(&stdout, MAX_CMD_OUTPUT));
                body.push('\n');
            }
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.trim().is_empty() {
                body.push_str("stderr:\n");
                body.push_str(&clip(&stderr, MAX_CMD_OUTPUT));
            }
            ToolOutput {
                summary: format!("ran `{cmd}` (exit {code})"),
                content: vec![umide_agent::ToolResultContent::text(body)],
                is_error: !out.status.success(),
            }
        }
        Err(e) => ToolOutput::error(format!("failed to run `{cmd}`: {e}")),
    }
}

/// Like `Command::output()` but kills the child after `timeout` and drains
/// stdout/stderr concurrently to avoid pipe-buffer deadlock.
fn output_with_timeout(
    mut command: std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::Instant;

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut out_pipe = child.stdout.take().unwrap();
    let mut err_pipe = child.stderr.take().unwrap();
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        buf
    });

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("command timed out after {}s", timeout.as_secs()),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };

    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Truncate on a UTF-8 boundary, appending a marker when clipped.
fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… [truncated]", &s[..end])
}
