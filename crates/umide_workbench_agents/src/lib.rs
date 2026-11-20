//! A native, per-turn client for Codex App Server and Claude Code.
mod claude;
mod codex;

use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, mpsc},
    thread,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Provider {
    Codex,
    Claude,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Session(String),
    Text(String),
    Activity(String),
    Command(String),
    Approval { id: String, description: String },
    Done,
    Cancelled,
    Error(String),
    ProviderError(String),
}

pub struct Run {
    pub events: mpsc::Receiver<Event>,
    pub control: Control,
}

#[derive(Clone)]
pub struct Control(Arc<ControlInner>);

struct ControlInner {
    sender: mpsc::Sender<Input>,
}

impl Drop for ControlInner {
    fn drop(&mut self) {
        let _ = self.sender.send(Input::Cancel);
    }
}

impl Control {
    pub fn cancel(&self) {
        let _ = self.0.sender.send(Input::Cancel);
    }

    pub fn decide(&self, id: &str, allow: bool) {
        let _ = self.0.sender.send(Input::Decision(id.into(), allow));
    }
}

enum Input {
    Frame(Value),
    Closed,
    Failed(String),
    Cancel,
    Decision(String, bool),
}

pub fn cli_environment(provider: Provider) -> Result<(PathBuf, OsString), String> {
    let (name, key) = match provider {
        Provider::Codex => ("codex", "UMIDE_CODEX_PATH"),
        Provider::Claude => ("claude", "UMIDE_CLAUDE_PATH"),
    };
    let mut dirs: Vec<_> =
        env::split_paths(&env::var_os("PATH").unwrap_or_default())
            .filter(|p| p.is_absolute())
            .collect();
    #[cfg(unix)]
    {
        if let Some(home) = env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".local/bin"));
        }
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    }
    #[cfg(windows)]
    {
        if let Some(home) = env::var_os("USERPROFILE") {
            dirs.push(PathBuf::from(home).join(".local/bin"));
        }
        if let Some(local) = env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Programs/OpenAI/Codex/bin"));
        }
    }
    let path = env::join_paths(&dirs).map_err(|e| e.to_string())?;
    let extensions: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd"]
    } else {
        &[""]
    };
    let binary = if let Some(value) = env::var_os(key) {
        let binary = PathBuf::from(value);
        if !binary.is_absolute() || !binary.is_file() {
            return Err(format!("{key} must point to the installed executable."));
        }
        binary
    } else {
        dirs.iter().flat_map(|dir| extensions.iter().map(move |ext| dir.join(format!("{name}{ext}"))))
            .find(|p| p.is_file())
            .ok_or_else(|| format!("Could not find {name}. Install the CLI or set {key} to its absolute path."))?
    };
    Ok((binary, path))
}

struct Turn {
    cwd: PathBuf,
    session: Option<String>,
    input: Value,
    verify_server: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct VerifyConfig {
    /// Umide's installed executable, which implements the verification server.
    pub executable: PathBuf,
    /// Per-turn evidence and context directory created by the Workbench.
    pub event_directory: PathBuf,
}

impl VerifyConfig {
    fn server(&self, checkout: &Path) -> Result<Value, String> {
        let path = |p: &Path| {
            if p.is_absolute() {
                p.to_str().map(String::from)
            } else {
                None
            }
            .ok_or_else(|| {
                "Verification requires absolute Unicode paths.".to_owned()
            })
        };
        Ok(json!({
            "command": path(&self.executable)?,
            "args": ["--umide-verify", path(checkout)?, path(&self.event_directory)?]
        }))
    }
}

// These calls only observe the current local verification session. Launch,
// stop, navigation, clicks, text entry and taps retain CLI permission handling.
const READ_ONLY_VERIFY_TOOLS: &[&str] = &[
    "workspace_context",
    "mobile_devices",
    "mobile_status",
    "mobile_logs",
    "mobile_screenshot",
    "web_inspect",
    "web_wait",
    "web_screenshot",
];

const VERIFY_INSTRUCTIONS: &str = "Inside Umide, use its tools to verify UI changes. Consult workspace_context when needed. Report observations and limits.";

pub fn start(
    provider: Provider,
    project: &Path,
    prompt: &str,
    session: Option<&str>,
) -> Result<Run, String> {
    start_with_image(provider, project, prompt, session, None)
}

pub fn start_with_image(
    provider: Provider,
    project: &Path,
    prompt: &str,
    session: Option<&str>,
    image: Option<&Path>,
) -> Result<Run, String> {
    start_with_verification(provider, project, prompt, session, image, None)
}

pub fn start_with_verification(
    provider: Provider,
    project: &Path,
    prompt: &str,
    session: Option<&str>,
    image: Option<&Path>,
    verify: Option<&VerifyConfig>,
) -> Result<Run, String> {
    if prompt.trim().is_empty() {
        return Err("Enter a prompt.".into());
    }
    if !project.is_dir() {
        return Err("The project folder is unavailable.".into());
    }
    if session.is_some_and(|id| {
        id.is_empty()
            || id.len() > 128
            || id.starts_with('-')
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }) {
        return Err("Invalid session ID.".into());
    }
    let cwd = project.canonicalize().map_err(|e| e.to_string())?;
    let (binary, path) = cli_environment(provider)?;
    let mut command = Command::new(binary);
    command.current_dir(&cwd).env("PATH", path);
    // Git commands issued by the agent must discover this task’s checkout from cwd.
    for key in [
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
    ] {
        command.env_remove(key);
    }
    let verify_server = verify.map(|config| config.server(&cwd)).transpose()?;
    match provider {
        Provider::Codex => {
            command.args(["app-server", "--stdio"]);
        }
        Provider::Claude => {
            claude::configure(&mut command, session, verify_server.as_ref())
        }
    }
    // Keep Codex's configured developer instructions intact. Claude snapshots
    // its first system prompt, including across resumes of older conversations.
    let turn_hint =
        verify.is_some() && (provider == Provider::Codex || session.is_some());
    let input = prompt_content(provider, prompt, image, turn_hint)?;
    spawn(
        command,
        provider,
        Turn {
            cwd,
            input,
            session: session.map(String::from),
            verify_server,
        },
    )
}

fn prompt_content(
    provider: Provider,
    prompt: &str,
    image: Option<&Path>,
    turn_hint: bool,
) -> Result<Value, String> {
    let block = |text: &str| match provider {
        Provider::Codex => json!({"type":"text","text":text,"text_elements":[]}),
        Provider::Claude => json!({"type":"text","text":text}),
    };
    let mut content = Vec::new();
    if turn_hint {
        content.push(block(VERIFY_INSTRUCTIONS));
    }
    content.push(block(prompt));
    if let Some(path) = image {
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        const MAX: u64 = 24 * 1024 * 1024;
        let mut bytes = Vec::new();
        file.take(MAX + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("The device screenshot must be a PNG under 24 MiB.".into());
        }
        content.push(match provider {
            Provider::Codex => json!({"type":"localImage","path":path}),
            Provider::Claude => json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":STANDARD.encode(bytes)}}),
        });
    }
    Ok(Value::Array(content))
}

fn spawn(
    mut command: Command,
    provider: Provider,
    turn: Turn,
) -> Result<Run, String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not launch {provider:?}: {e}"))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let (events, receiver) = mpsc::channel();
    let (send, inputs) = mpsc::channel();
    let reader = send.clone();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let input = match line {
                Ok(s) if s.trim().is_empty() => continue,
                Ok(s) => match serde_json::from_str(&s) {
                    Ok(msg) => Input::Frame(msg),
                    Err(e) => Input::Failed(format!(
                        "Invalid {provider:?} protocol message: {e}"
                    )),
                },
                Err(e) => Input::Failed(e.to_string()),
            };
            if reader.send(input).is_err() {
                return;
            }
        }
        let _ = reader.send(Input::Closed);
    });
    let (diagnostics_tx, diagnostics_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = diagnostics_tx.send(diagnostic_tail(stderr));
    });
    thread::spawn(move || {
        let mut io = ProtocolIo {
            writer: stdin,
            events: &events,
            approvals: HashMap::new(),
        };
        let result = drive(provider, &turn, inputs, &mut io);
        // Closing stdin allows the CLI to flush its conversation before another turn resumes it.
        drop(io);
        if matches!(result, Ok(Event::Done)) {
            for _ in 0..20 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        stop_process(&mut child);
        let mut end = result.unwrap_or_else(Event::Error);
        if let Event::Error(detail) | Event::ProviderError(detail) = &mut end {
            // Descendants launched by a tool can keep the pipe open after the CLI exits.
            // Never make ending the UI turn depend on a detached writer.
            let diagnostics = diagnostics_rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .unwrap_or_default();
            if !diagnostics.is_empty() {
                detail.push('\n');
                detail.push_str(&diagnostics);
            }
        }
        let _ = events.send(end);
    });
    Ok(Run {
        events: receiver,
        control: Control(Arc::new(ControlInner { sender: send })),
    })
}

fn diagnostic_tail(mut stderr: impl Read) -> String {
    let mut tail = Vec::new();
    let mut block = [0; 2048];
    loop {
        match stderr.read(&mut block) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                tail.extend_from_slice(&block[..n]);
                let excess = tail.len().saturating_sub(2000);
                tail.drain(..excess);
            }
        }
    }
    String::from_utf8_lossy(&tail).trim().to_string()
}

fn stop_process(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    #[cfg(unix)]
    {
        // The child starts a dedicated process group, distinct from UMIDE's group.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Some(system) = env::var_os("SystemRoot") {
            let _ =
                Command::new(PathBuf::from(system).join("System32/taskkill.exe"))
                    .creation_flags(0x08000000)
                    .args(["/PID", &child.id().to_string(), "/T", "/F"])
                    .output();
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

struct ProtocolIo<'a, W> {
    writer: W,
    events: &'a mpsc::Sender<Event>,
    approvals: HashMap<String, (Value, Value)>,
}

impl<W: Write> ProtocolIo<'_, W> {
    fn send(&mut self, message: Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.writer, &message)
            .map_err(|e| e.to_string())?;
        self.writer
            .write_all(b"\n")
            .and_then(|_| self.writer.flush())
            .map_err(|e| e.to_string())
    }

    fn event(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn approve(
        &mut self,
        id: String,
        description: String,
        allow: Value,
        deny: Value,
    ) -> Result<(), String> {
        if self.approvals.contains_key(&id) {
            return Err("Agent repeated a pending approval ID.".into());
        }
        self.approvals.insert(id.clone(), (allow, deny));
        self.event(Event::Approval { id, description });
        Ok(())
    }
}

fn drive<W: Write>(
    provider: Provider,
    turn: &Turn,
    inputs: mpsc::Receiver<Input>,
    io: &mut ProtocolIo<'_, W>,
) -> Result<Event, String> {
    let mut codex = codex::State::default();
    let mut claude = claude::State::default();
    match provider {
        Provider::Codex => io.send(json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"umide","title":"Umide","version":env!("CARGO_PKG_VERSION")},"capabilities":null}}))?,
        Provider::Claude => io.send(json!({"type":"user","message":{"role":"user","content":turn.input},"parent_tool_use_id":null,"session_id":""}))?,
    }
    while let Ok(input) = inputs.recv() {
        match input {
            Input::Cancel => return Ok(Event::Cancelled),
            Input::Closed => {
                return Err(format!(
                    "{provider:?} disconnected before the turn finished."
                ));
            }
            Input::Failed(error) => return Err(error),
            Input::Decision(id, allowed) => {
                if let Some((allow, deny)) = io.approvals.remove(&id) {
                    io.send(if allowed { allow } else { deny })?;
                }
            }
            Input::Frame(msg) => {
                let end = match provider {
                    Provider::Codex => codex.handle(io, &msg, turn)?,
                    Provider::Claude => claude.handle(io, &msg)?,
                };
                if let Some(event) = end {
                    return Ok(event);
                }
            }
        }
    }
    Ok(Event::Cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn exercise(provider: Provider, frames: Vec<Input>) -> (Vec<Event>, Vec<Value>) {
        exercise_with_mcp(provider, frames, false)
    }

    fn exercise_with_mcp(
        provider: Provider,
        frames: Vec<Input>,
        mcp: bool,
    ) -> (Vec<Event>, Vec<Value>) {
        let (events, receiver) = mpsc::channel();
        let (send, inputs) = mpsc::channel();
        for frame in frames {
            send.send(frame).unwrap();
        }
        drop(send);
        let mut io = ProtocolIo {
            writer: Vec::new(),
            events: &events,
            approvals: HashMap::new(),
        };
        let turn = Turn {
            cwd: PathBuf::from("/my-project"),
            input: json!([{"type":"text","text":"hello"}]),
            session: Some("my-session".into()),
            verify_server: mcp.then(|| json!({"command":"fixture","args":[]})),
        };
        let end = drive(provider, &turn, inputs, &mut io).unwrap();
        let mut emitted: Vec<_> = receiver.try_iter().collect();
        emitted.push(end);
        let messages = io
            .writer
            .lines()
            .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
            .collect();
        (emitted, messages)
    }

    #[test]
    fn codex_resumes_and_routes_only_approved_requests_for_this_thread() {
        let frame = Input::Frame;
        let ask = |thread| {
            frame(
                json!({"id":6,"method":"item/commandExecution/requestApproval","params":{"threadId":thread,"command":"cargo check"}}),
            )
        };
        let (events, messages) = exercise(
            Provider::Codex,
            vec![
                frame(json!({"id":1,"result":{}})),
                frame(json!({"id":2,"result":{"thread":{"id":"t"}}})),
                frame(
                    json!({"method":"item/started","params":{"threadId":"t","item":{"type":"commandExecution","command":"Build"}}}),
                ),
                ask("other"),
                ask("t"),
                Input::Decision("6".into(), false),
                frame(
                    json!({"method":"item/agentMessage/delta","params":{"threadId":"t","itemId":"m","delta":"ok"}}),
                ),
                frame(
                    json!({"method":"item/completed","params":{"threadId":"t","item":{"id":"m","type":"agentMessage","text":"ok"}}}),
                ),
                frame(
                    json!({"method":"turn/completed","params":{"threadId":"t","turn":{"status":"completed"}}}),
                ),
            ],
        );
        assert_eq!(messages[2]["method"], "thread/resume");
        assert_eq!(messages[2]["params"]["sandbox"], "workspace-write");
        assert_eq!(messages[2]["params"]["threadId"], "my-session");
        assert_eq!(messages[4]["result"]["decision"], "decline");
        assert_eq!(messages[5]["result"]["decision"], "decline");
        assert_eq!(
            events,
            vec![
                Event::Session("t".into()),
                Event::Command("Build".into()),
                Event::Approval {
                    id: "6".into(),
                    description: "Allow command\ncargo check".into()
                },
                Event::Text("ok".into()),
                Event::Done
            ]
        );
    }

    #[test]
    fn claude_preserves_requested_input_and_does_not_repeat_partial_text() {
        let frame = Input::Frame;
        let (events, messages) = exercise(
            Provider::Claude,
            vec![
                frame(json!({"type":"system","subtype":"init","session_id":"t"})),
                frame(
                    json!({"type":"control_request","request_id":"q","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"test.rs"}}}),
                ),
                Input::Decision("expired".into(), true),
                Input::Decision("q".into(), true),
                frame(
                    json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"m"}}}),
                ),
                frame(
                    json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"he"}}}),
                ),
                frame(
                    json!({"type":"assistant","message":{"id":"m","content":[{"type":"text","text":"hello"}]}}),
                ),
                frame(
                    json!({"type":"result","subtype":"success","is_error":false,"result":"hello"}),
                ),
            ],
        );
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[1]["response"]["response"],
            json!({"behavior":"allow","updatedInput":{"file_path":"test.rs"}})
        );
        assert_eq!(
            events[2..],
            [
                Event::Text("he".into()),
                Event::Text("llo".into()),
                Event::Done
            ]
        );
    }

    #[test]
    fn codex_routes_only_local_confirmation_forms_for_the_active_turn() {
        let request = |id, thread, required| {
            Input::Frame(
                json!({"id":id,"method":"mcpServer/elicitation/request","params":{"serverName":"umide","threadId":thread,"mode":"form","message":"Launch the app?","requestedSchema":{"type":"object","properties":{},"required":required}}}),
            )
        };
        let (events, sent) = exercise_with_mcp(
            Provider::Codex,
            vec![
                Input::Frame(json!({"id":1,"result":{}})),
                Input::Frame(json!({"id":2,"result":{"thread":{"id":"current"}}})),
                request(12, "other", vec![]),
                request(13, "current", vec!["input"]),
                request(14, "current", vec![]),
                Input::Decision("14".into(), true),
                Input::Cancel,
            ],
            true,
        );
        assert_eq!(
            &sent[4..],
            [
                json!({"id":12,"result":{"action":"decline","content":null}}),
                json!({"id":13,"result":{"action":"decline","content":null}}),
                json!({"id":14,"result":{"action":"accept","content":{}}}),
            ]
        );
        assert!(
            matches!(&events[..], [Event::Session(_), Event::Approval{id,..}, Event::Cancelled] if id=="14")
        );
    }

    #[test]
    fn provider_errors_are_distinct_from_translatable_statuses() {
        let codex = Input::Frame(json!({"id":1,"error":{"message":"Build"}}));
        let claude =
            Input::Frame(json!({"type":"result","is_error":true,"result":"Build"}));
        for (provider, frame) in
            [(Provider::Codex, codex), (Provider::Claude, claude)]
        {
            let (events, _) = exercise(provider, vec![frame]);
            assert_eq!(events, [Event::ProviderError("Build".into())]);
        }
    }

    #[test]
    fn verification_protocol_keeps_literal_paths_and_existing_approvals_on_start_and_resume()
     {
        let root = env::temp_dir().join("Démo 'mobile'");
        let config = VerifyConfig {
            executable: root.join("Umide application"),
            event_directory: root.join("events $2"),
        };
        let server = config.server(&root).unwrap();
        assert_eq!(
            server["args"],
            json!(["--umide-verify", root, config.event_directory])
        );
        for session in [None, Some("old-thread")] {
            let turn = Turn {
                cwd: root.clone(),
                session: session.map(String::from),
                input: json!([]),
                verify_server: Some(server.clone()),
            };
            let request = codex::thread_request(&turn);
            assert_eq!(
                request["method"],
                if session.is_some() {
                    "thread/resume"
                } else {
                    "thread/start"
                }
            );
            let params = &request["params"];
            assert!(params.get("developerInstructions").is_none());
            let blocks =
                prompt_content(Provider::Codex, "request", None, true).unwrap();
            assert_eq!(blocks[0]["text"], VERIFY_INSTRUCTIONS);
            assert_eq!(blocks[1]["text"], "request");
            assert_eq!(
                params["config"]["mcp_servers.umide"]["args"],
                server["args"]
            );
            let tool_policy = &params["config"]["mcp_servers.umide"]["tools"];
            assert_eq!(tool_policy["workspace_context"]["approval_mode"], "approve");
            assert!(tool_policy.get("mobile_tap").is_none());
            assert_eq!(params["approvalPolicy"], "on-request");
            assert_eq!(params["sandbox"], "workspace-write");
            let mut cmd = Command::new("claude");
            claude::configure(&mut cmd, session, Some(&server));
            let args: Vec<_> = cmd.get_args().map(|s| s.to_str().unwrap()).collect();
            let after = |flag: &str| {
                args.iter().position(|s| *s == flag).map(|i| args[i + 1])
            };
            assert_eq!(after("--resume"), session);
            assert_eq!(after("--permission-prompt-tool"), Some("stdio"));
            assert_eq!(
                after("--append-system-prompt"),
                session.is_none().then_some(VERIFY_INSTRUCTIONS)
            );
            let parsed: Value =
                serde_json::from_str(after("--mcp-config").unwrap()).unwrap();
            assert_eq!(parsed["mcpServers"]["umide"], server);
            assert!(args.contains(&"--strict-mcp-config"));
            let allowed = after("--allowedTools").unwrap();
            assert!(
                allowed
                    .split(',')
                    .any(|t| t == "mcp__umide__workspace_context")
            );
            assert!(!allowed.contains("mobile_tap") && !allowed.contains("Bash"));
            assert!(after("--tools").unwrap().contains("ToolSearch"));
        }
        let bad = VerifyConfig {
            executable: "relative".into(),
            ..config
        };
        assert!(bad.server(&root).is_err());
    }

    #[test]
    fn cancellation_kills_an_idle_process_and_ends_receiver() {
        #[cfg(unix)]
        {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "read line; sleep 30"]);
            let turn = Turn {
                cwd: env::temp_dir(),
                session: None,
                input: json!([]),
                verify_server: None,
            };
            let run = spawn(command, Provider::Codex, turn).unwrap();
            run.control.cancel();
            assert_eq!(
                run.events
                    .recv_timeout(std::time::Duration::from_secs(3))
                    .unwrap(),
                Event::Cancelled
            );
        }
    }
}
