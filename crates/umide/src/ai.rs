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
    ToolResultContent,
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
to SEE the screen, `tap`, `swipe`, `type_text`, `press_key` to interact, \
`read_logs` to read the device log, and `describe_ui` to list on-screen \
elements as text with tap coordinates (an accessibility fallback when a \
screenshot is ambiguous; Android-only). These work on both Android (adb) and \
iOS (simctl + idb); they auto-detect the running device, or pass `platform` \
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

/// The detected project kind as a system-prompt suffix, so the agent KNOWS the
/// stack instead of re-discovering it every session (detection: `project.rs`,
/// surfaced via `CommonData.project_kind`). Byte-stable per kind, so the
/// provider prompt cache stays valid across turns in the same workspace;
/// empty when nothing was detected (plain folders keep the neutral prompt).
pub fn project_context(kind: Option<crate::project::ProjectKind>) -> &'static str {
    match kind {
        Some(crate::project::ProjectKind::ReactNative) => {
            "\n\nThe open workspace is a React Native app (detected from its \
             package.json). Assume React Native idioms and tooling — \
             components/screens/navigation, Metro, `npx react-native` or Expo \
             equivalents — unless the code says otherwise."
        }
        Some(crate::project::ProjectKind::Flutter) => {
            "\n\nThe open workspace is a Flutter app (detected from its \
             pubspec.yaml). Assume Flutter idioms and tooling — widgets, \
             `flutter run`/`flutter test`, pub — unless the code says otherwise."
        }
        None => "",
    }
}

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
    /// Permission-only gate for an external agent CLI (Claude Code): the CLI
    /// performs the action itself, UMIDE only decides allow/deny. Nothing is
    /// applied on the UMIDE side — the outcome is mapped to the CLI's contract.
    CliPermission { tool_name: String },
    /// One-time per-session consent for the assistant to drive the emulator
    /// (tap/swipe/type/keys). Approve unlocks device input for the session.
    DeviceControl,
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
    /// The user allowed a CLI permission gate (the CLI will do the action).
    Allowed,
}

/// Map an approval outcome to a device-control consent decision: anything the
/// user didn't reject (or that didn't fail to apply) unlocks device input.
fn consent_granted(outcome: &ApprovalOutcome) -> bool {
    !matches!(
        outcome,
        ApprovalOutcome::Rejected | ApprovalOutcome::EditFailed(_)
    )
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

/// A process-unique approval id, shared by the LLM tools and the CLI permission
/// bridge so ids never collide across paths.
pub fn next_approval_id() -> u64 {
    NEXT_APPROVAL_ID.fetch_add(1, Ordering::Relaxed)
}

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
    pub device_consent: Arc<Mutex<Option<bool>>>,
    pub selected_device: Option<umide_emulator::DeviceInfo>,
    pub project_kind: Option<crate::project::ProjectKind>,
}

#[async_trait(?Send)]
impl AgentRunner for LlmRunner {
    async fn run(&mut self, user_text: String, push: Push, cancel: CancelHandle) {
        let tools: Arc<dyn ToolExecutor> = Arc::new(EditorTools::new(
            self.workspace.clone(),
            self.approvals.clone(),
            self.trigger,
            self.device_consent.clone(),
            self.selected_device.clone(),
        ));
        let seed = self.history.lock().unwrap().clone();
        let system_prompt =
            format!("{SYSTEM_PROMPT}{}", project_context(self.project_kind));
        let mut agent =
            match Agent::resume(self.provider.clone(), tools, system_prompt, seed) {
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

/// Guards a worker turn so the UI is never left "thinking" forever: if the
/// worker body panics (or returns without a terminal event), `Drop` emits an
/// `Error`, which resets `streaming` in the panel. `finish()` disarms it on a
/// clean completion.
struct TurnGuard {
    push: Push,
    done: bool,
}

impl TurnGuard {
    fn new(push: Push) -> Self {
        Self { push, done: false }
    }
    fn finish(&mut self) {
        self.done = true;
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        if !self.done {
            self.push.emit(AgentEvent::Error(
                "The assistant stopped unexpectedly.".into(),
            ));
        }
    }
}

/// Push a terminal error into the UI when a worker thread can't even start.
fn report_spawn_failure(
    queue: &EventQueue,
    trigger: ExtSendTrigger,
    e: &std::io::Error,
) {
    queue.lock().unwrap().push_back(AgentEvent::Error(format!(
        "Could not start the assistant: {e}"
    )));
    register_ext_trigger(trigger);
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
    device_consent: Arc<Mutex<Option<bool>>>,
    selected_device: Option<umide_emulator::DeviceInfo>,
    project_kind: Option<crate::project::ProjectKind>,
    cancel: Arc<AtomicBool>,
) {
    let err_queue = queue.clone();
    let spawned = std::thread::Builder::new()
        .name("umide-agent".into())
        .spawn(move || {
            let push = Push::new(move |ev: AgentEvent| {
                queue.lock().unwrap().push_back(ev);
                register_ext_trigger(trigger);
            });
            // Emits a terminal Error on panic/early-return so `streaming` resets.
            let mut guard = TurnGuard::new(push.clone());

            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    push.emit(AgentEvent::Error(format!("runtime: {e}")));
                    guard.finish();
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
                    device_consent,
                    selected_device,
                    project_kind,
                };
                runner.run(user_text, push, cancel).await;
            });
            guard.finish();
        });
    if let Err(e) = spawned {
        report_spawn_failure(&err_queue, trigger, &e);
    }
}

/// Like [`spawn_turn`], but backed by an external agent CLI (Claude Code, …).
/// The CLI runs its own loop in `workspace` and streams events into `queue`;
/// `trigger`/`cancel` behave exactly as for the LLM path. `session` carries the
/// CLI's conversation id across turns so the agent keeps multi-turn context.
#[allow(clippy::too_many_arguments)]
pub fn spawn_cli_turn(
    kind: cli::CliKind,
    workspace: PathBuf,
    session: Arc<Mutex<Option<String>>>,
    user_text: String,
    queue: EventQueue,
    approvals: ApprovalQueue,
    trigger: ExtSendTrigger,
    selected_device: Option<umide_emulator::DeviceInfo>,
    project_kind: Option<crate::project::ProjectKind>,
    cancel: Arc<AtomicBool>,
) {
    // The device-MCP tools drive Android over adb, so pin the panel-selected
    // Android serial (`None` ⇒ the device server targets the first running one).
    let serial = selected_device.and_then(|d| {
        matches!(d.platform, umide_emulator::DevicePlatform::Android)
            .then_some(d.serial)
            .flatten()
    });
    let err_queue = queue.clone();
    let spawned = std::thread::Builder::new()
        .name("umide-agent-cli".into())
        .spawn(move || {
            let push = Push::new(move |ev: AgentEvent| {
                queue.lock().unwrap().push_back(ev);
                register_ext_trigger(trigger);
            });
            let mut guard = TurnGuard::new(push.clone());

            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    push.emit(AgentEvent::Error(format!("runtime: {e}")));
                    guard.finish();
                    return;
                }
            };

            let cancel = CancelHandle::new(cancel);
            rt.block_on(async move {
                let mut runner = cli::runner::CliRunner::new(
                    kind,
                    workspace,
                    session,
                    approvals,
                    trigger,
                    serial,
                    project_kind,
                );
                runner.run(user_text, push, cancel).await;
            });
            guard.finish();
        });
    if let Err(e) = spawned {
        report_spawn_failure(&err_queue, trigger, &e);
    }
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
    /// Per-session consent for driving the emulator: `None` until the user is
    /// asked once, then `Some(granted)`. Shared across turns (owned by the panel)
    /// so the agent is gated once per session, not once per turn.
    device_consent: Arc<Mutex<Option<bool>>>,
    /// The device the user is viewing in the Emulator panel at turn start, if
    /// any. Device tools target it (so the agent drives what the user sees)
    /// unless the model passes an explicit `platform`. `None` → auto-detect.
    selected_device: Option<umide_emulator::DeviceInfo>,
}

impl EditorTools {
    pub fn new(
        root: Option<PathBuf>,
        approvals: ApprovalQueue,
        trigger: ExtSendTrigger,
        device_consent: Arc<Mutex<Option<bool>>>,
        selected_device: Option<umide_emulator::DeviceInfo>,
    ) -> Self {
        Self {
            reader: ReadOnlyTools::new(root),
            approvals,
            trigger,
            device_consent,
            selected_device,
        }
    }

    /// Gate on the one-time per-session consent to control the device. Reads and
    /// screenshots stay ungated; this only fences the *input* tools.
    async fn ensure_device_consent(&self) -> bool {
        if let Some(v) = *self.device_consent.lock().unwrap() {
            return v;
        }
        let outcome = self
            .request_approval(
                "Let the assistant control the emulator?".into(),
                "It will tap, swipe, type, and press keys on the running device \
                 to test and verify — for this session."
                    .into(),
                ApprovalKind::DeviceControl,
            )
            .await;
        let granted = consent_granted(&outcome);
        *self.device_consent.lock().unwrap() = Some(granted);
        granted
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
            ApprovalOutcome::CommandApproved | ApprovalOutcome::Allowed => {
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
        match resolve_target(input, self.selected_device.as_ref()) {
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
        match resolve_target(input, self.selected_device.as_ref()) {
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
        match resolve_target(input, self.selected_device.as_ref()) {
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
        match resolve_target(input, self.selected_device.as_ref()) {
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
        match resolve_target(input, self.selected_device.as_ref()) {
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
        match resolve_target(input, self.selected_device.as_ref()) {
            Ok(Target::Android(serial)) => android_logs(&serial, lines, filter),
            Ok(Target::Ios(udid)) => ios_logs(&udid, lines, filter),
            Err(e) => ToolOutput::error(e),
        }
    }

    fn describe_ui(&self, input: &serde_json::Value) -> ToolOutput {
        match resolve_target(input, self.selected_device.as_ref()) {
            Ok(Target::Android(serial)) => android_describe_ui(&serial),
            Ok(Target::Ios(_)) => ToolOutput::error(
                "describe_ui is Android-only for now — use screenshot_device on \
                 iOS.",
            ),
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
            // Device INPUT is gated by a one-time per-session consent; reads
            // (screenshot/logs) are not.
            "tap" | "swipe" | "type_text" | "press_key" => {
                if !self.ensure_device_consent().await {
                    return ToolOutput::error(
                        "Device control was declined for this session.",
                    );
                }
                match call.name.as_str() {
                    "tap" => self.tap(&call.input),
                    "swipe" => self.swipe(&call.input),
                    "type_text" => self.type_text(&call.input),
                    _ => self.press_key(&call.input),
                }
            }
            "read_logs" => self.read_logs(&call.input),
            "describe_ui" => self.describe_ui(&call.input),
            other => ToolOutput::error(format!("unknown tool: {other}")),
        }
    }

    /// A2 — after the agent drives the device (tap/swipe/type/key), capture a
    /// fresh screenshot and hand it back automatically, so the model always sees
    /// the result of its action. Skipped if the model already screenshotted this
    /// turn (no double image) or if no device action ran.
    async fn auto_observe(&self, executed: &[ToolInvocation]) -> Vec<ContentBlock> {
        if !should_auto_observe(executed) {
            return Vec::new();
        }
        const MUTATING: &[&str] = &["tap", "swipe", "type_text", "press_key"];
        // Screenshot the same platform the device actions targeted.
        let platform = executed
            .iter()
            .rev()
            .find(|c| MUTATING.contains(&c.name.as_str()))
            .and_then(|c| c.input.get("platform").cloned());
        let input = match platform {
            Some(p) => serde_json::json!({ "platform": p }),
            None => serde_json::json!({}),
        };
        let out = self.screenshot_device(&input);
        if out.is_error {
            return Vec::new();
        }
        let mut blocks = vec![ContentBlock::text(
            "Screenshot of the device after the action(s) above:",
        )];
        for c in out.content {
            if let ToolResultContent::Image { source } = c {
                blocks.push(ContentBlock::Image { source });
            }
        }
        blocks
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
        ToolDef {
            name: "describe_ui".into(),
            description: "List the on-screen UI elements as text — each labelled \
                or tappable element with its center coordinate, label, class and \
                id. An accessibility fallback for when a screenshot is ambiguous \
                (custom-rendered React Native / Flutter UIs). Android-only."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "platform": platform() }
            }),
            cache_control: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// Device helpers: Android (adb) and iOS (simctl + idb)
// ---------------------------------------------------------------------------

/// Which device a tool call targets.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Android(String), // adb serial, e.g. emulator-5554
    Ios(String),     // simulator UDID
}

/// Resolve which device a tool call targets. Precedence:
/// 1. an explicit `platform` arg from the model (an override);
/// 2. otherwise the device the user is viewing in the panel (`selected`), so the
///    agent drives what the user sees rather than "first adb device";
/// 3. otherwise auto-detect — a running Android emulator first, then a booted
///    iOS simulator.
fn resolve_target(
    input: &serde_json::Value,
    selected: Option<&umide_emulator::DeviceInfo>,
) -> Result<Target, String> {
    // 1. Explicit model override always wins.
    match input.get("platform").and_then(|v| v.as_str()) {
        Some("android") => return android_serial().map(Target::Android),
        Some("ios") => return ios_udid().map(Target::Ios),
        Some(other) => {
            return Err(format!(
                "unknown platform '{other}' (use 'android' or 'ios')"
            ));
        }
        None => {}
    }
    // 2. Target the device the user is viewing in the Emulator panel.
    if let Some(dev) = selected {
        match dev.platform {
            // An iOS `DeviceInfo.id` IS the simulator UDID, so target it
            // directly — this picks the right sim when several are booted.
            umide_emulator::DevicePlatform::Ios => {
                return Ok(Target::Ios(dev.id.clone()));
            }
            // Prefer the panel-resolved adb serial (`emulator-<port>`) when the
            // producer has it — that targets the *exact* device the user is
            // viewing even with several Android emulators up. Fall back to the
            // first running serial (the `.id` is only the AVD name, not a serial).
            umide_emulator::DevicePlatform::Android => {
                return match &dev.serial {
                    Some(serial) => Ok(Target::Android(serial.clone())),
                    None => android_serial().map(Target::Android),
                };
            }
        }
    }
    // 3. Nothing selected: auto-detect Android first, then iOS.
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

/// PATH augmented with the Android SDK platform-tools (and, on Unix, Homebrew
/// and the usual idb install locations) so `adb`/`idb`/`xcrun` resolve
/// regardless of the shell. Uses the platform's PATH separator — a `:`-joined
/// Unix path is malformed on Windows and broke `adb` resolution there.
fn tool_path_env() -> String {
    let base = std::env::var("PATH").unwrap_or_default();
    #[cfg(windows)]
    {
        let mut parts: Vec<String> = Vec::new();
        for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
            if let Ok(v) = std::env::var(var) {
                if !v.is_empty() {
                    parts.push(format!("{v}\\platform-tools"));
                }
            }
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            parts.push(format!("{local}\\Android\\Sdk\\platform-tools"));
        }
        parts.push(base);
        parts.join(";")
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        format!(
            "/opt/homebrew/bin:/usr/local/bin:{home}/.local/bin:\
             {home}/Library/Android/sdk/platform-tools:\
             {home}/Android/Sdk/platform-tools:{base}"
        )
    }
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

/// Wall-clock cap for a single device shell command (`adb`/`simctl`/`idb`).
/// Device tools must stay responsive: a hung `adb` — device mid-boot, offline,
/// or a wedged daemon — must never freeze the agent turn indefinitely.
const DEVICE_CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Run a command (built fresh each attempt by `build`) with a timeout, retrying
/// **once** on a transient failure — a timeout or an adb daemon/attach race that
/// typically clears immediately. Genuine failures (bad command, missing binary)
/// return as-is.
fn run_with_retry(
    build: impl Fn() -> std::process::Command,
) -> std::io::Result<std::process::Output> {
    match output_with_timeout(build(), DEVICE_CMD_TIMEOUT) {
        // Success, or a real failure a retry wouldn't fix — return it.
        Ok(out)
            if out.status.success()
                || !adb_stderr_is_transient(&String::from_utf8_lossy(
                    &out.stderr,
                )) =>
        {
            Ok(out)
        }
        // Spawn error that isn't a timeout (e.g. binary not found): no retry.
        Err(e) if e.kind() != std::io::ErrorKind::TimedOut => Err(e),
        // Transient adb state or a timeout: back off briefly, then try once more.
        _ => {
            std::thread::sleep(std::time::Duration::from_millis(400));
            output_with_timeout(build(), DEVICE_CMD_TIMEOUT)
        }
    }
}

/// Run a command string through the platform shell (`sh -c` / `cmd /C`). Use
/// this only for macOS-only tools (simctl/idb) or commands with no arguments to
/// escape — on Windows, `cmd /C` re-parses `> | & && '…'` as operators and keeps
/// quotes, so it must NOT carry device commands with shell metacharacters. For
/// `adb` prefer [`run_tool`], which bypasses the shell entirely.
fn adb_sh(cmd: &str) -> std::io::Result<std::process::Output> {
    run_with_retry(|| shell_command(cmd))
}

/// Run a tool binary (`adb`, …) **directly** — argv, no host shell — so its
/// arguments are never re-parsed by `cmd.exe`/`sh`. This is the correct path for
/// `adb`: on Windows a `cmd /C "<string>"` treats `> | & && '…'` as operators and
/// leaves quotes in place, mangling device commands (uiautomator `&&`, `input
/// text '…'`, `logcat | grep`). Same PATH, timeout and transient-retry as
/// [`adb_sh`], plus `CREATE_NO_WINDOW` so the GUI app flashes no console.
fn run_tool(program: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    run_with_retry(|| {
        let mut c = std::process::Command::new(program);
        c.args(args).env("PATH", tool_path_env());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            c.creation_flags(CREATE_NO_WINDOW);
        }
        c
    })
}

/// adb surfaces a few transient states (daemon-restart race, a device still
/// attaching, a dropped connection) that usually clear on an immediate retry.
/// Match only those, so a genuinely bad command isn't retried pointlessly.
fn adb_stderr_is_transient(stderr: &str) -> bool {
    let e = stderr.to_lowercase();
    e.contains("device offline")
        || e.contains("error: closed")
        || e.contains("daemon not running")
        || e.contains("protocol fault")
        || e.contains("device still connecting")
}

/// Single-quote a value for safe inclusion in a `sh -c` command.
fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Serial of the first running Android device (e.g. `emulator-5554`).
fn android_serial() -> Result<String, String> {
    let out = run_tool("adb", &["devices"]).map_err(|e| {
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

/// Run `adb -s <serial> shell <device_cmd>` and report success/failure for a
/// card. `device_cmd` is the whole command line for the DEVICE's shell (e.g.
/// `input tap 100 200`, `input text 'a b'`); it is passed as a single argv
/// element to `adb` — the HOST shell is bypassed, so `cmd.exe` on Windows can't
/// mangle its quotes/metacharacters. The device's own `sh` parses it.
fn adb_input(serial: &str, device_cmd: &str, summary: String) -> ToolOutput {
    match run_tool("adb", &["-s", serial, "shell", device_cmd]) {
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

/// Whether to auto-capture a screenshot after a tool batch: only when the agent
/// drove the device (tap/swipe/type/key) AND didn't already screenshot this turn
/// (so we never send a redundant second image). Pure, so it's unit-tested.
fn should_auto_observe(executed: &[ToolInvocation]) -> bool {
    const MUTATING: &[&str] = &["tap", "swipe", "type_text", "press_key"];
    let mutated = executed.iter().any(|c| MUTATING.contains(&c.name.as_str()));
    let already = executed.iter().any(|c| c.name == "screenshot_device");
    mutated && !already
}

/// Long-edge cap for screenshots handed to the model. Native phone frames are
/// ~1080×2400; downscaling to this keeps each screenshot affordable so the
/// agent can auto-observe every step without exhausting the vision token budget.
const SHOT_MAX_EDGE: u32 = 1280;

/// Downscale a PNG so its long edge is ≤ `max_edge`, re-encoding as PNG. Returns
/// the bytes unchanged if it's already small enough or can't be decoded — taps
/// still map 1:1 because the model is told to reason over device coordinates.
fn downscale_png(bytes: &[u8], max_edge: u32) -> Vec<u8> {
    let img = match image::load_from_memory(bytes) {
        Ok(i) => i,
        Err(_) => return bytes.to_vec(),
    };
    let (w, h) = (img.width(), img.height());
    if w.max(h) <= max_edge {
        return bytes.to_vec();
    }
    let scale = max_edge as f32 / w.max(h) as f32;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    let resized = img.resize(nw, nh, image::imageops::FilterType::Triangle);
    let mut out = std::io::Cursor::new(Vec::new());
    match resized.write_to(&mut out, image::ImageFormat::Png) {
        Ok(()) => out.into_inner(),
        Err(_) => bytes.to_vec(),
    }
}

fn android_screenshot(serial: &str) -> ToolOutput {
    match run_tool("adb", &["-s", serial, "exec-out", "screencap", "-p"]) {
        Ok(out) if out.status.success() && !out.stdout.is_empty() => {
            let png = downscale_png(&out.stdout, SHOT_MAX_EDGE);
            ToolOutput::with_image(format!("screenshot of {serial}"), &png)
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
                let png = downscale_png(&bytes, SHOT_MAX_EDGE);
                ToolOutput::with_image(format!("screenshot of {udid}"), &png)
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
    // Run bare logcat and filter in Rust: piping to `grep` would go through the
    // host shell (broken on Windows, where `grep` isn't on PATH and `cmd.exe`
    // keeps the quotes) — this is identical to `grep -i` and works everywhere.
    match run_tool(
        "adb",
        &["-s", serial, "logcat", "-d", "-t", &lines.to_string()],
    ) {
        Ok(out) => ToolOutput::ok(format!(
            "logcat (last {lines} lines)\n{}",
            clip(
                &filter_lines(&String::from_utf8_lossy(&out.stdout), filter),
                MAX_CMD_OUTPUT
            )
        )),
        Err(e) => ToolOutput::error(format!("logcat: {e}")),
    }
}

/// Keep only lines that contain `needle` (case-insensitive). Empty `needle`
/// returns the text unchanged. The in-process replacement for `| grep -i`.
fn filter_lines(text: &str, needle: &str) -> String {
    if needle.is_empty() {
        return text.to_string();
    }
    let needle = needle.to_lowercase();
    text.lines()
        .filter(|l| l.to_lowercase().contains(&needle))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Dump the current Android view hierarchy (`uiautomator dump`) and render it as
/// a compact, tappable element listing. Done as **two plain adb calls** — dump
/// the XML to a file on the device, then `cat` it back — with no shell operators
/// (`&&`, `>`) or quotes, so `cmd.exe` on Windows can't mis-parse the command
/// (a single `exec-out '… && …'` string fails there).
fn android_describe_ui(serial: &str) -> ToolOutput {
    // 1. Write the hierarchy to a file on the device. uiautomator prints its
    //    status to stdout ("UI hierchary dumped to: …"); we ignore that and read
    //    the file next. A spawn/timeout error is fatal; a non-zero exit is not
    //    (some builds warn yet still write the file), so the `<node` check below
    //    is the real gate.
    if let Err(e) = run_tool(
        "adb",
        &[
            "-s",
            serial,
            "shell",
            "uiautomator",
            "dump",
            "/sdcard/umide_ui.xml",
        ],
    ) {
        return ToolOutput::error(format!("uiautomator dump: {e}"));
    }
    // 2. Read the XML back with a plain `exec-out cat` (no operators/quotes).
    match run_tool(
        "adb",
        &["-s", serial, "exec-out", "cat", "/sdcard/umide_ui.xml"],
    ) {
        Ok(out) => {
            let xml = String::from_utf8_lossy(&out.stdout);
            if xml.contains("<node") {
                let listing = parse_ui_dump(&xml);
                ToolOutput::ok(format!("UI elements (x,y = tap center):\n{listing}"))
            } else {
                ToolOutput::error(format!(
                    "uiautomator produced no hierarchy: {}",
                    clip(xml.trim(), 200)
                ))
            }
        }
        Err(e) => ToolOutput::error(format!("uiautomator: {e}")),
    }
}

/// Parse a uiautomator XML dump into one line per labelled or interactive node:
/// `(cx,cy) [tap] "label" <Class> #id`. Nodes with no label and no interaction
/// are dropped so the output stays a useful, skimmable map rather than the raw
/// tree. Output is capped so a deep hierarchy can't flood the context.
fn parse_ui_dump(xml: &str) -> String {
    // Each `<node ...>` (container or leaf) carries its own attributes, so a
    // flat scan over opening tags captures every element regardless of nesting.
    // Attribute values are XML-escaped, so `>` never appears inside one.
    let node_re = regex::Regex::new(r"<node\s+([^>]*?)/?>")
        .expect("static node regex is valid");
    let attr_re = regex::Regex::new(r#"([\w:-]+)="([^"]*)""#)
        .expect("static attr regex is valid");

    let mut lines = Vec::new();
    for node in node_re.captures_iter(xml) {
        let mut attrs = std::collections::HashMap::new();
        for a in attr_re.captures_iter(&node[1]) {
            attrs.insert(a[1].to_string(), xml_unescape(&a[2]));
        }
        let get = |k: &str| attrs.get(k).map(String::as_str).unwrap_or("");
        let text = get("text").trim();
        let desc = get("content-desc").trim();
        let clickable = get("clickable") == "true";
        let label = if !text.is_empty() { text } else { desc };
        // Keep only what a user could read or act on.
        if label.is_empty() && !clickable {
            continue;
        }
        let Some((cx, cy)) = bounds_center(get("bounds")) else {
            continue;
        };
        let mut line = format!("({cx},{cy})");
        if clickable {
            line.push_str(" [tap]");
        }
        if !label.is_empty() {
            line.push_str(&format!(" \"{}\"", clip(label, 80)));
        }
        let class = get("class").rsplit('.').next().unwrap_or("");
        if !class.is_empty() {
            line.push_str(&format!(" <{class}>"));
        }
        let rid = get("resource-id");
        if !rid.is_empty() {
            line.push_str(&format!(" #{}", rid.rsplit('/').next().unwrap_or(rid)));
        }
        lines.push(line);
    }
    if lines.is_empty() {
        return "No labelled or interactive elements found (the screen may be \
                fully custom-rendered — use screenshot_device)."
            .to_string();
    }
    const MAX: usize = 200;
    let total = lines.len();
    if total > MAX {
        lines.truncate(MAX);
        lines.push(format!("… ({} more elements omitted)", total - MAX));
    }
    lines.join("\n")
}

/// Center `(x,y)` of a uiautomator `bounds` value `"[x1,y1][x2,y2]"`.
fn bounds_center(bounds: &str) -> Option<(i64, i64)> {
    let nums: Vec<i64> = bounds
        .split(|c: char| c != '-' && !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    match nums.as_slice() {
        [x1, y1, x2, y2] => Some(((x1 + x2) / 2, (y1 + y2) / 2)),
        _ => None,
    }
}

/// Unescape the five XML entities that appear in uiautomator attribute values.
/// `&amp;` is replaced last so an escaped entity isn't double-decoded.
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
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

    fn call(name: &str) -> ToolInvocation {
        ToolInvocation {
            id: name.into(),
            name: name.into(),
            input: serde_json::json!({}),
        }
    }

    #[test]
    fn auto_observe_fires_only_after_a_device_mutation() {
        // A device action → observe.
        assert!(should_auto_observe(&[call("tap")]));
        assert!(should_auto_observe(&[call("read_file"), call("swipe")]));
        // Model already screenshotted → don't double up.
        assert!(!should_auto_observe(&[
            call("tap"),
            call("screenshot_device")
        ]));
        // No device action → nothing to re-observe.
        assert!(!should_auto_observe(&[
            call("read_file"),
            call("edit_file")
        ]));
        assert!(!should_auto_observe(&[]));
    }

    #[test]
    fn downscale_png_caps_the_long_edge() {
        // A 2000x1000 PNG downscales to a 1280 long edge; aspect preserved.
        let big = image::DynamicImage::new_rgb8(2000, 1000);
        let mut buf = std::io::Cursor::new(Vec::new());
        big.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let small = downscale_png(&buf.into_inner(), 1280);
        let out = image::load_from_memory(&small).unwrap();
        assert_eq!(out.width(), 1280);
        assert_eq!(out.height(), 640);
    }

    #[test]
    fn downscale_png_leaves_small_images_untouched() {
        let small = image::DynamicImage::new_rgb8(800, 600);
        let mut buf = std::io::Cursor::new(Vec::new());
        small.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let bytes = buf.into_inner();
        assert_eq!(downscale_png(&bytes, 1280), bytes);
    }

    #[test]
    fn resolve_target_honors_selected_device() {
        use umide_emulator::{DeviceInfo, DevicePlatform, DeviceState};
        let ios = DeviceInfo {
            id: "UDID-123".into(),
            name: "iPhone 15".into(),
            platform: DevicePlatform::Ios,
            state: DeviceState::Running,
            serial: None, // iOS has no adb serial
        };
        // No explicit arg → target the selected iOS sim directly by its UDID
        // (this branch resolves without touching a real device).
        assert_eq!(
            resolve_target(&serde_json::json!({}), Some(&ios)),
            Ok(Target::Ios("UDID-123".into()))
        );
        // An explicit `platform` arg overrides the selection: asking for android
        // ignores the selected iOS sim and tries to resolve an android serial
        // (which, with no device in the test env, errors — proving the override
        // took precedence rather than returning the iOS target).
        assert!(
            resolve_target(&serde_json::json!({"platform": "android"}), Some(&ios))
                .is_err()
        );
        // A selected Android device with a panel-resolved serial targets that
        // exact serial (no device needed — the serial is taken as-is).
        let android = DeviceInfo {
            id: "Pixel_9a".into(),
            name: "Pixel 9a".into(),
            platform: DevicePlatform::Android,
            state: DeviceState::Running,
            serial: Some("emulator-5556".into()),
        };
        assert_eq!(
            resolve_target(&serde_json::json!({}), Some(&android)),
            Ok(Target::Android("emulator-5556".into()))
        );
    }

    /// End-to-end smoke test against a real running Android emulator/device.
    /// Ignored by default (CI has no device); run with one booted:
    ///   cargo test -p umide-app --lib live_android -- --ignored --nocapture
    #[test]
    #[ignore = "requires a running Android device/emulator"]
    fn live_android_device_tools() {
        let serial = android_serial().expect("a running Android device");
        eprintln!("• android_serial → {serial}");

        // Real screencap → PNG bytes (with_image, not an error).
        let shot = android_screenshot(&serial);
        assert!(!shot.is_error, "screenshot errored: {}", shot.summary);
        let has_png = shot
            .content
            .iter()
            .any(|c| matches!(c, umide_agent::ToolResultContent::Image { .. }));
        assert!(has_png, "screenshot returned no image");
        eprintln!("• android_screenshot → {} (image ✓)", shot.summary);

        // Real uiautomator dump → parsed, tappable element listing.
        let ui = ui_text(&android_describe_ui(&serial));
        assert!(
            ui.contains('(') && ui.contains(','),
            "describe_ui had no coordinates:\n{ui}"
        );
        eprintln!(
            "• android_describe_ui → {} lines, e.g.\n    {}",
            ui.lines().count(),
            ui.lines().next().unwrap_or("")
        );

        // type_text with the exact chars that broke cmd.exe on Windows — must
        // run cleanly via the direct-argv path (#41).
        let typed =
            adb_input(&serial, "input text 'a&b<c>d|e'", "type special".into());
        assert!(!typed.is_error, "type_text errored: {}", typed.summary);
        eprintln!("• adb_input (special chars) → ok");

        // Filtered logs (Rust-side filter, no `grep`).
        let logs = android_logs(&serial, 60, "activitymanager");
        assert!(!logs.is_error, "read_logs errored: {}", logs.summary);
        eprintln!("• android_logs (filtered) → {}", logs.summary);

        // G2: a selected Android device with a serial targets that exact serial.
        let dev = umide_emulator::DeviceInfo {
            id: "Pixel_9a".into(),
            name: "Pixel 9a".into(),
            platform: umide_emulator::DevicePlatform::Android,
            state: umide_emulator::DeviceState::Running,
            serial: Some(serial.clone()),
        };
        assert_eq!(
            resolve_target(&serde_json::json!({}), Some(&dev)),
            Ok(Target::Android(serial))
        );
        eprintln!("• resolve_target(selected) → the viewed device ✓");
    }

    fn ui_text(out: &ToolOutput) -> String {
        assert!(!out.is_error, "describe_ui errored: {}", out.summary);
        text_of(out)
    }

    #[test]
    fn bounds_center_computes_midpoint() {
        assert_eq!(bounds_center("[0,0][100,200]"), Some((50, 100)));
        assert_eq!(bounds_center("[10,20][30,60]"), Some((20, 40)));
        assert_eq!(bounds_center("garbage"), None);
        assert_eq!(bounds_center("[1,2][3]"), None); // only 3 numbers
    }

    #[test]
    fn parse_ui_dump_lists_labelled_and_tappable_nodes() {
        let xml = r#"<?xml version='1.0'?>
<hierarchy>
  <node class="android.widget.FrameLayout" bounds="[0,0][1080,2400]">
    <node text="Settings" resource-id="com.app:id/title"
          class="android.widget.TextView" clickable="false"
          bounds="[40,100][300,180]" />
    <node content-desc="Search &amp; more" class="android.widget.Button"
          clickable="true" bounds="[900,100][1000,200]" />
    <node text="" content-desc="" class="android.view.View"
          clickable="false" bounds="[0,300][1080,400]" />
  </node>
</hierarchy>"#;
        let out = parse_ui_dump(xml);
        // Labelled TextView is listed with its center + short class + short id.
        assert!(out.contains("(170,140)"));
        assert!(out.contains("\"Settings\""));
        assert!(out.contains("<TextView>"));
        assert!(out.contains("#title"));
        // Clickable button: [tap] marker + unescaped content-desc.
        assert!(out.contains("[tap]"));
        assert!(out.contains("\"Search & more\""));
        // The empty, non-clickable spacer node is dropped.
        assert!(!out.contains("(540,350)"));
    }

    #[test]
    fn parse_ui_dump_reports_empty_hierarchy() {
        let out = parse_ui_dump("<hierarchy></hierarchy>");
        assert!(out.contains("custom-rendered"));
    }

    #[test]
    fn filter_lines_matches_grep_i() {
        let log =
            "I Choreographer: skipped\nE MyApp: NullPointerError\nW System: low mem";
        // Case-insensitive substring, like `grep -i error` (matches "Error").
        assert_eq!(filter_lines(log, "error"), "E MyApp: NullPointerError");
        // Empty needle → unchanged.
        assert_eq!(filter_lines(log, ""), log);
        // No match → empty.
        assert_eq!(filter_lines(log, "zzz"), "");
    }

    #[test]
    fn adb_transient_errors_are_retried() {
        // Daemon/attach races → retry.
        assert!(adb_stderr_is_transient("error: device offline"));
        assert!(adb_stderr_is_transient("error: closed"));
        assert!(adb_stderr_is_transient(
            "adb: error: failed to start daemon: daemon not running"
        ));
        assert!(adb_stderr_is_transient(
            "protocol fault (couldn't read status)"
        ));
        // Real failures → no retry.
        assert!(!adb_stderr_is_transient(
            "Exception occurred while executing 'input'"
        ));
        assert!(!adb_stderr_is_transient(""));
    }

    #[test]
    fn project_context_names_the_detected_stack() {
        use crate::project::ProjectKind;
        // Detected stacks are named so the agent doesn't re-discover them…
        assert!(
            project_context(Some(ProjectKind::ReactNative)).contains("React Native")
        );
        assert!(project_context(Some(ProjectKind::Flutter)).contains("Flutter"));
        // …and a plain folder keeps the neutral prompt (byte-stable: empty).
        assert_eq!(project_context(None), "");
    }

    #[test]
    fn device_consent_maps_approve_and_reject() {
        // Approving (Allowed / CommandApproved / EditApplied) unlocks device
        // input; only Rejected or a failed edit keeps it locked.
        assert!(consent_granted(&ApprovalOutcome::Allowed));
        assert!(consent_granted(&ApprovalOutcome::CommandApproved));
        assert!(consent_granted(&ApprovalOutcome::EditApplied));
        assert!(!consent_granted(&ApprovalOutcome::Rejected));
        assert!(!consent_granted(&ApprovalOutcome::EditFailed("io".into())));
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
