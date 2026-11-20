use std::{
    cell::RefCell,
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::Duration,
};

use floem::{
    action::exec_after,
    ext_event::create_ext_action,
    prelude::*,
    reactive::{Memo, Scope},
};
use tempfile::NamedTempFile;
use umide_workbench_agents::{self as agents, Event, Provider, Run};

use crate::{
    devices,
    projects::{self, AppRun, Conversation, History, Message, ToolOutput},
    worktree,
};

pub type Target = (String, String);
type Prepared = Result<(Run, Option<NamedTempFile>), StartError>;

enum StartError {
    CreateWorkspace(String),
    Agent(String),
}

impl From<String> for StartError {
    fn from(error: String) -> Self {
        Self::Agent(error)
    }
}

enum Stage {
    Preparing(Receiver<Prepared>),
    Running {
        run: Run,
        _picture: Option<NamedTempFile>,
    },
}

struct ActiveTurn {
    stage: RefCell<Stage>,
    cancelled: Arc<AtomicBool>,
    status: ToolOutput,
    approvals: Vec<(String, String)>,
    worktree_id: Option<String>,
}

impl ActiveTurn {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Stage::Running { run, .. } = &*self.stage.borrow() {
            run.control.cancel();
        }
    }

    fn events(&self) -> Result<Vec<Event>, String> {
        let mut stage = self.stage.borrow_mut();
        if let Stage::Preparing(rx) = &*stage {
            match rx.try_recv() {
                Ok(Ok((run, picture))) => {
                    if self.cancelled.load(Ordering::Acquire) {
                        run.control.cancel();
                    }
                    *stage = Stage::Running {
                        run,
                        _picture: picture,
                    };
                }
                Ok(Err(StartError::CreateWorkspace(e))) => return Err(e),
                Ok(Err(StartError::Agent(e))) => return Ok(vec![Event::Error(e)]),
                Err(TryRecvError::Disconnected) => {
                    return Ok(vec![Event::Error("Agent failed to start".into())]);
                }
                Err(TryRecvError::Empty) => return Ok(vec![]),
            }
        }
        let Stage::Running { run, .. } = &*stage else {
            unreachable!()
        };
        let mut events = Vec::new();
        for _ in 0..256 {
            match run.events.try_recv() {
                Ok(event) => {
                    let final_event = matches!(
                        event,
                        Event::Done
                            | Event::Cancelled
                            | Event::Error(_)
                            | Event::ProviderError(_)
                    );
                    events.push(event);
                    if final_event {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    events.push(Event::Error("Agent disconnected".into()));
                    break;
                }
            }
        }
        Ok(events)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct Workspace {
    project: PathBuf,
    worktree_id: Option<String>,
}

impl Workspace {
    pub(crate) fn path(&self) -> Result<PathBuf, String> {
        match &self.worktree_id {
            Some(key) => worktree::open(&self.project, &projects::data_dir()?, key),
            None => Ok(self.project.clone()),
        }
    }

    fn diff(&self) -> Result<String, String> {
        if let Some(key) = &self.worktree_id {
            let dir = projects::data_dir()?;
            let (path, base) = worktree::review(&self.project, &dir, key)?;
            projects::git_diff_since(&path, Some(&base))
        } else {
            projects::git_diff(&self.project)
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Chat,
    Changes,
    Output,
    Logs,
}

#[derive(Clone, Copy)]
pub(crate) struct WorkbenchStatus(RwSignal<ToolOutput>);

impl WorkbenchStatus {
    pub(crate) fn set(self, notice: String) {
        self.0.set(ToolOutput::notice(notice));
    }

    pub(crate) fn display(self) -> String {
        self.0.with(ToolOutput::display)
    }
}

#[derive(Clone, Copy)]
pub struct Workbench {
    scope: Scope,
    pub root: RwSignal<Option<PathBuf>>,
    pub history: RwSignal<History>,
    pub current: RwSignal<Option<usize>>,
    pub provider: RwSignal<Provider>,
    pub draft: RwSignal<String>,
    pub target: RwSignal<Option<Target>>,
    pub attach: RwSignal<bool>,
    pub tab: RwSignal<Tab>,
    pub(crate) status: WorkbenchStatus,
    pub changes: RwSignal<String>,
    pub(crate) logs: RwSignal<projects::ToolOutput>,
    pub(crate) output: RwSignal<projects::ToolOutput>,
    pub build_label: RwSignal<String>,
    pub build_owner: RwSignal<Option<usize>>,
    pub busy: RwSignal<bool>,
    pub isolated: RwSignal<bool>,
    pub launching: RwSignal<bool>,
    pub approvals: RwSignal<Vec<(String, String)>>,
    active: RwSignal<HashMap<usize, ActiveTurn>>,
    drafts: RwSignal<HashMap<usize, String>>,
    app: RwSignal<Option<AppRun>>,
    stop_build_requested: RwSignal<bool>,
    log_seq: RwSignal<u64>,
    diff_seq: RwSignal<u64>,
}

pub fn provider_name(p: Provider) -> &'static str {
    match p {
        Provider::Claude => "Claude Code",
        Provider::Codex => "Codex",
    }
}

impl Workbench {
    pub fn new() -> Self {
        Self {
            scope: Scope::current(),
            root: RwSignal::new(None),
            logs: RwSignal::new(projects::ToolOutput::default()),
            log_seq: RwSignal::new(0),
            stop_build_requested: RwSignal::new(false),
            history: RwSignal::new(History::default()),
            current: RwSignal::new(None),
            provider: RwSignal::new(Provider::Codex),
            draft: RwSignal::new(String::new()),
            target: RwSignal::new(None),
            attach: RwSignal::new(false),
            tab: RwSignal::new(Tab::Chat),
            status: WorkbenchStatus(RwSignal::new(ToolOutput::notice("Ready"))),
            changes: RwSignal::new(String::new()),
            output: RwSignal::new(projects::ToolOutput::default()),
            build_label: RwSignal::new(String::new()),
            build_owner: RwSignal::new(None),
            busy: RwSignal::new(false),
            launching: RwSignal::new(false),
            approvals: RwSignal::new(vec![]),
            isolated: RwSignal::new(false),
            active: RwSignal::new(HashMap::new()),
            drafts: RwSignal::new(HashMap::new()),
            app: RwSignal::new(None),
            diff_seq: RwSignal::new(0),
        }
    }

    pub fn open(self, root: &Path) {
        if self.active.with_untracked(|a| !a.is_empty())
            || self.launching.get_untracked()
        {
            return;
        }
        let Ok(root) = root.canonicalize() else {
            self.status.set("Project folder could not be opened".into());
            return;
        };
        if !root.is_dir() {
            self.status.set("Select a project folder".into());
            return;
        }
        match projects::load_history(&root) {
            Ok(history) => {
                let last = history.conversations.len().checked_sub(1);
                self.current.set(None);
                self.drafts.set(HashMap::new());
                self.root.set(Some(root.clone()));
                self.history.set(history);
                self.new_chat();
                if let Some(index) = last {
                    self.select(index);
                }
                self.changes.set(String::new());
                self.output.set(projects::ToolOutput::default());
                self.build_label.set(String::new());
                self.build_owner.set(None);
                self.status.set("Ready".into());
                if let Err(e) = projects::remember_project(&root) {
                    self.status.set(e);
                }
            }
            Err(e) => self
                .status
                .set(format!("Could not read this project's conversations: {e}")),
        }
    }

    pub fn any_busy(self) -> bool {
        self.active.with(|a| !a.is_empty())
    }

    pub fn task_badge(self, index: usize) -> &'static str {
        self.active.with(|turns| match turns.get(&index) {
            Some(t) if !t.approvals.is_empty() => "Needs approval",
            Some(_) => "Running",
            None => "",
        })
    }

    fn remember_draft(self) {
        if let Some(i) = self.current.get_untracked() {
            let text = self.draft.get_untracked();
            self.drafts.update(|drafts| {
                if text.is_empty() {
                    drafts.remove(&i);
                } else {
                    drafts.insert(i, text);
                }
            });
        }
    }

    pub fn new_chat(self) {
        self.remember_draft();
        self.current.set(None);
        self.draft.set(String::new());
        self.attach.set(false);
        self.isolated.set(
            self.active
                .with_untracked(|t| t.values().any(|t| t.worktree_id.is_none())),
        );
        self.tab.set(Tab::Chat);
        self.sync_selection();
    }

    pub fn set_provider(self, p: Provider) {
        if self.provider.get_untracked() == p || self.busy.get_untracked() {
            return;
        }
        if self.current.get_untracked().is_some() {
            let prompt = self.draft.get_untracked();
            self.new_chat();
            self.draft.set(prompt);
        }
        self.provider.set(p);
    }

    pub fn select(self, index: usize) {
        let details = self.history.with_untracked(|h| {
            h.conversations.get(index).map(|c| {
                (
                    if c.provider == "claude" {
                        Provider::Claude
                    } else {
                        Provider::Codex
                    },
                    c.isolated || c.worktree_id.is_some(),
                )
            })
        });
        if let Some((p, isolated)) = details {
            self.remember_draft();
            self.current.set(Some(index));
            self.draft.set(
                self.drafts
                    .with_untracked(|m| m.get(&index).cloned().unwrap_or_default()),
            );
            self.attach.set(false);
            self.provider.set(p);
            self.isolated.set(isolated);
            self.tab.set(Tab::Chat);
            self.sync_selection();
        }
    }

    fn sync_selection(self) {
        let current = self.current.get_untracked();
        let info = self.active.with_untracked(|a| {
            current
                .and_then(|i| a.get(&i))
                .map(|t| (t.status.clone(), t.approvals.clone()))
        });
        self.busy.set(info.is_some());
        let (status, approvals) =
            info.unwrap_or_else(|| (ToolOutput::notice("Ready"), vec![]));
        self.approvals.set(approvals);
        self.status.0.set(status);
    }

    fn persist(self) -> Result<(), String> {
        let root = self.root.get_untracked().ok_or("No project open")?;
        self.history
            .with_untracked(|h| projects::save_history(&root, h))
            .map_err(|e| format!("Could not save conversation: {e}"))
    }

    pub(crate) fn workspace_selection(self) -> Memo<Result<Workspace, String>> {
        Memo::new(move |_| {
            self.root.track();
            self.current.track();
            self.history.track();
            self.isolated.track();
            self.selected_workspace()
        })
    }

    pub(crate) fn selected_workspace(self) -> Result<Workspace, String> {
        let project = self.root.get_untracked().ok_or("Open a project first.")?;
        let Some(index) = self.current.get_untracked() else {
            if self.isolated.get_untracked() {
                return Err(
                    "Send the first message to create this isolated workspace."
                        .into(),
                );
            }
            return Ok(Workspace {
                project,
                worktree_id: None,
            });
        };
        let key = self.history.with_untracked(|h| {
            let c = h.conversations.get(index).ok_or("Conversation unavailable.")?;
            if c.isolated && c.worktree_id.is_none() {
                return Err("The isolated workspace is unavailable. Check the conversation for setup errors.");
            }
            Ok(c.worktree_id.clone())
        })?;
        Ok(Workspace {
            project,
            worktree_id: key,
        })
    }

    pub fn send(self) {
        let current = self.current.get_untracked();
        if self
            .active
            .with_untracked(|a| current.is_some_and(|i| a.contains_key(&i)))
        {
            return;
        }
        let Some(root) = self.root.get_untracked() else {
            return;
        };
        let draft = self.draft.get_untracked().trim().to_owned();
        if draft.is_empty() {
            return;
        }
        let device = if self.attach.get_untracked() {
            let Some(target) = self.target.get_untracked() else {
                self.status
                    .set("Select a running device to attach its screen".into());
                return;
            };
            Some(target)
        } else {
            None
        };
        let p = self.provider.get_untracked();
        let fork = match current {
            None => self.isolated.get_untracked(),
            Some(index) => self.history.with_untracked(|h| {
                let c = &h.conversations[index];
                c.isolated && c.worktree_id.is_none() && c.session_id.is_none()
            }),
        };
        let key = if fork {
            Some(worktree::new_key())
        } else {
            match self.selected_workspace() {
                Ok(ws) => ws.worktree_id,
                Err(e) => {
                    self.status.set(e);
                    return;
                }
            }
        };
        if self
            .active
            .with_untracked(|a| a.values().any(|t| t.worktree_id == key))
        {
            self.status.set("Another task is using this folder. Enable isolation for a new conversation or wait for that task.".into());
            return;
        }
        let index = current.unwrap_or_else(|| {
            self.history.with_untracked(|h| h.conversations.len())
        });
        let mut displayed = draft.clone();
        if let Some((platform, id)) = &device {
            displayed.push_str(&format!("\n[Screen: {platform} · {id}]"));
        }
        self.history.update(|h| {
            if current.is_none() {
                h.conversations.push(Conversation {
                    title: draft.chars().take(52).collect(),
                    provider: match p {
                        Provider::Codex => "codex",
                        Provider::Claude => "claude",
                    }
                    .into(),
                    worktree_id: key.clone(),
                    isolated: key.is_some(),
                    ..Default::default()
                });
            }
            if fork {
                h.conversations[index].worktree_id = key.clone();
            }
            h.conversations[index].messages.extend([
                Message {
                    role: "user".into(),
                    text: displayed,
                },
                Message {
                    role: "assistant".into(),
                    text: String::new(),
                },
            ]);
        });
        if let Err(e) = self.persist() {
            self.history.update(|h| {
                if current.is_none() {
                    h.conversations.pop();
                } else {
                    let c = &mut h.conversations[index];
                    c.messages.truncate(c.messages.len() - 2);
                    if fork {
                        c.worktree_id = None;
                    }
                }
            });
            self.status.set(e);
            return;
        }
        let session = self
            .history
            .with_untracked(|h| h.conversations[index].session_id.clone());
        self.current.set(Some(index));
        self.draft.set(String::new());
        self.attach.set(false);
        self.tab.set(Tab::Chat);
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        self.active.update(|a| {
            a.insert(
                index,
                ActiveTurn {
                    stage: RefCell::new(Stage::Preparing(rx)),
                    cancelled: cancelled.clone(),
                    worktree_id: key.clone(),
                    status: ToolOutput::notice(format!(
                        "Starting {}",
                        provider_name(p)
                    )),
                    approvals: vec![],
                },
            );
        });
        self.sync_selection();
        thread::spawn(move || {
            let result = (|| {
                let cwd = match key {
                    Some(key) if fork => projects::data_dir()
                        .and_then(|dir| worktree::create_with_key(&root, &dir, &key))
                        .map_err(StartError::CreateWorkspace)?,
                    Some(key) => {
                        worktree::open(&root, &projects::data_dir()?, &key)?
                    }
                    None => root,
                };
                if cancelled.load(Ordering::Acquire) {
                    return Err(StartError::Agent("Turn stopped".into()));
                }
                let image = if let Some((platform, id)) = device {
                    let png = devices::capture_png(&platform, &id)?;
                    let mut file = tempfile::Builder::new()
                        .suffix(".png")
                        .tempfile()
                        .map_err(|e| e.to_string())?;
                    file.write_all(&png).map_err(|e| e.to_string())?;
                    Some(file)
                } else {
                    None
                };
                if cancelled.load(Ordering::Acquire) {
                    return Err(StartError::Agent("Turn stopped".into()));
                }
                let run = agents::start_with_image(
                    p,
                    &cwd,
                    &draft,
                    session.as_deref(),
                    image.as_ref().map(|f| f.path()),
                )?;
                if cancelled.load(Ordering::Acquire) {
                    run.control.cancel();
                }
                Ok((run, image))
            })();
            let _ = tx.send(result);
        });
        self.poll_agent(index);
    }

    fn poll_agent(self, index: usize) {
        // A short poll runs only while this task is alive. Receiving without a
        // reactive write avoids invalidating sidebar badges on idle polls.
        let events = self
            .active
            .try_with_untracked(|tasks| tasks?.get(&index).map(ActiveTurn::events));
        let Some(events) = events else {
            return;
        };
        match events {
            Ok(events) => self.apply_events(index, events),
            Err(e) => {
                // Only a failed first creation may be retried with a fresh key.
                // Failure to reopen an existing checkout must preserve its key.
                self.history
                    .update(|h| h.conversations[index].worktree_id = None);
                self.finish(index, Some(Message::notice(e)));
            }
        }
        if self.active.with_untracked(|a| a.contains_key(&index)) {
            exec_after(Duration::from_millis(50), move |_| self.poll_agent(index));
        }
    }

    fn apply_events(self, index: usize, events: Vec<Event>) {
        if events.is_empty() {
            return;
        }
        let mut text = String::new();
        let mut terminal = None;
        let mut session = None;
        let mut activity = None;
        let mut requests = Vec::new();
        for ev in events {
            match ev {
                Event::Text(s) => text.push_str(&s),
                Event::Session(id) => session = Some(id),
                Event::Activity(s) => activity = Some(ToolOutput::notice(s)),
                Event::Command(s) => activity = Some(ToolOutput::verbatim(s)),
                Event::Approval { id, description } => {
                    requests.push((id, description))
                }
                Event::Error(e) => terminal = Some(Some(Message::notice(e))),
                Event::ProviderError(text) => {
                    terminal = Some(Some(Message {
                        role: "provider_error".into(),
                        text,
                    }))
                }
                Event::Cancelled => {
                    terminal = Some(Some(Message::notice("Turn stopped")))
                }
                Event::Done => terminal = Some(None),
            }
        }
        let save = session.is_some();
        if !text.is_empty() || save {
            self.history.update(|h| {
                let c = &mut h.conversations[index];
                if let Some(id) = session {
                    c.session_id = Some(id);
                }
                if let Some(m) = c.messages.last_mut() {
                    m.text.push_str(&text);
                }
            });
        }
        if let Some(issue) = terminal {
            self.finish(index, issue);
            return;
        }
        if activity.is_some() || !requests.is_empty() {
            let requested = !requests.is_empty();
            self.active.update(|a| {
                if let Some(t) = a.get_mut(&index) {
                    t.approvals.extend(requests);
                    if !t.approvals.is_empty() {
                        t.status = ToolOutput::notice("Waiting for permission");
                    } else if let Some(s) = activity {
                        t.status = s;
                    }
                }
            });
            if self.current.get_untracked() == Some(index) {
                self.sync_selection();
                if requested {
                    self.tab.set(Tab::Chat);
                }
            }
        }
        if save && let Err(e) = self.persist() {
            self.status.set(e);
        }
    }

    fn finish(self, index: usize, issue: Option<Message>) {
        self.active.update(|a| {
            a.remove(&index);
        });
        self.history.update(|h| {
            let messages = &mut h.conversations[index].messages;
            if messages
                .last()
                .is_some_and(|m| m.role == "assistant" && m.text.is_empty())
            {
                messages.pop();
            }
            if let Some(e) = &issue {
                messages.push(e.clone());
            }
        });
        if self.current.get_untracked() == Some(index) {
            self.sync_selection();
            if let Some(e) = issue {
                self.status.0.set(if e.role == "provider_error" {
                    ToolOutput::verbatim(e.text)
                } else {
                    ToolOutput::notice(e.text)
                });
            }
            if self.tab.get_untracked() == Tab::Changes {
                self.show_diff();
            }
        }
        if let Err(e) = self.persist() {
            self.status.set(e);
        }
    }

    pub fn cancel(self) {
        if let Some(i) = self.current.get_untracked() {
            self.active.update(|a| {
                if let Some(t) = a.get_mut(&i) {
                    t.cancel();
                    t.status = ToolOutput::notice("Stopping agent…");
                }
            });
            self.sync_selection();
        }
    }

    pub fn decide(self, index: usize, id: &str, allow: bool) {
        if self.current.get_untracked() != Some(index) {
            return;
        }
        self.active.update(|a| {
            if let Some(t) = a.get_mut(&index)
                && t.approvals.first().is_some_and(|(key, _)| key == id)
                && let Stage::Running { run, .. } = &*t.stage.borrow()
            {
                run.control.decide(id, allow);
                t.approvals.remove(0);
                if t.approvals.is_empty() {
                    t.status = ToolOutput::notice("Working…");
                }
            }
        });
        self.sync_selection();
    }

    pub fn show_diff(self) {
        self.tab.set(Tab::Changes);
        let seq = self.diff_seq.get_untracked().wrapping_add(1);
        self.diff_seq.set(seq);
        let workspace = match self.selected_workspace() {
            Ok(ws) => ws,
            Err(e) => {
                self.changes.set(e);
                return;
            }
        };
        self.changes.set("Reading changes…".into());
        let selected = self.current.get_untracked();
        let root = workspace.project.clone();
        let receive =
            create_ext_action(self.scope, move |result: Result<String, String>| {
                if self.current.get_untracked() != selected
                    || self.diff_seq.get_untracked() != seq
                    || self.root.get_untracked() != Some(root.clone())
                {
                    return;
                }
                self.changes.set(match result {
                    Ok(s) if s.trim().is_empty() => "Working tree is clean.".into(),
                    Ok(s) | Err(s) => s,
                });
            });
        thread::spawn(move || receive(workspace.diff()));
    }

    pub fn refresh_logs(self) {
        let next = self.log_seq.get_untracked().wrapping_add(1);
        self.log_seq.set(next);
        let Some((platform, id)) = self.target.get_untracked() else {
            self.logs.set(projects::ToolOutput::notice(
                "Select a running device to read its logs.",
            ));
            return;
        };
        self.logs
            .set(projects::ToolOutput::notice("Reading device logs…"));
        let receive =
            create_ext_action(self.scope, move |result: projects::ToolOutput| {
                if next == self.log_seq.get_untracked() {
                    self.logs.set(result);
                }
            });
        thread::spawn(move || receive(projects::device_logs(&platform, &id)));
    }

    pub fn launch(self) {
        let Some((platform, id)) = self.target.get_untracked() else {
            return;
        };
        if self.launching.get_untracked() {
            return;
        }
        self.tab.set(Tab::Output);
        let owner = self.current.get_untracked();
        let title = self
            .history
            .with_untracked(|h| {
                owner
                    .and_then(|i| h.conversations.get(i))
                    .map(|c| c.title.clone())
            })
            .unwrap_or_else(|| "Project".into());
        self.build_owner.set(owner);
        self.build_label.set(format!("{title} · {id}"));
        let workspace = match self.selected_workspace() {
            Ok(ws) => ws,
            Err(e) => {
                self.output.set(projects::ToolOutput::notice(e));
                return;
            }
        };
        self.launching.set(true);
        self.stop_build_requested.set(false);
        self.output.set(projects::ToolOutput::notice(format!(
            "Starting build on {id}…"
        )));
        let send =
            create_ext_action(self.scope, move |result: Result<AppRun, String>| {
                match result {
                    Ok(run) => {
                        if self.stop_build_requested.get_untracked() {
                            run.stop();
                        }
                        self.app.set(Some(run));
                        self.poll_build();
                    }
                    Err(e) => {
                        self.launching.set(false);
                        self.output.set(projects::ToolOutput::notice(e));
                    }
                }
            });
        thread::spawn(move || {
            send(
                workspace
                    .path()
                    .and_then(|p| AppRun::start(&p, &platform, &id)),
            )
        });
    }

    fn poll_build(self) {
        let Some((running, out)) = self
            .app
            .try_with_untracked(|a| a?.as_ref().map(AppRun::snapshot))
        else {
            return;
        };
        self.output.set(out);
        if running {
            exec_after(Duration::from_millis(350), move |_| self.poll_build());
        } else {
            self.app.set(None);
            self.launching.set(false);
        }
    }

    pub fn stop_app(self) {
        self.stop_build_requested.set(true);
        self.app.with_untracked(|app| {
            if let Some(app) = app {
                app.stop();
            }
        });
    }

    pub fn close(self) {
        let indices = self.active.with_untracked(|a| {
            for t in a.values() {
                t.cancel();
            }
            a.keys().copied().collect::<Vec<_>>()
        });
        self.tab.set(Tab::Chat);
        for i in indices {
            self.finish(i, Some(Message::notice("Window closed during this turn")));
        }
        self.stop_app();
        self.app.set(None);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, process::Command, time::Instant};

    // Both processes deliberately use the same approval ID. Each one writes into
    // its working directory only when its own request is accepted.
    const CLI: &str = r#"#!/bin/sh
if [ "$(pwd -P)" = "$UMIDE_CONCURRENCY_PROJECT" ]; then task=primary; else task=isolated; fi
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"id":1,"result":{}}';;
    *'"method":"thread/start"'*|*'"method":"thread/resume"'*)
      echo "{\"id\":2,\"result\":{\"thread\":{\"id\":\"$task\"}}}";;
    *'"method":"turn/start"'*)
      echo "{\"id\":7,\"method\":\"item/commandExecution/requestApproval\",\"params\":{\"threadId\":\"$task\",\"command\":\"build $task\"}}";;
    *'"id":7'*)
      case "$line" in *'"accept"'*) touch accepted.txt; verdict=accepted;; *) verdict=denied;; esac
      echo "{\"method\":\"item/agentMessage/delta\",\"params\":{\"threadId\":\"$task\",\"itemId\":\"m\",\"delta\":\"$task $verdict\"}}"
      echo "{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"$task\",\"turn\":{\"status\":\"completed\"}}}";;
  esac
done
"#;

    #[test]
    fn concurrent_turns_route_approvals_output_and_cancellation_to_their_owner() {
        let Ok(project) = std::env::var("UMIDE_CONCURRENCY_PROJECT") else {
            let tmp = tempfile::tempdir().unwrap();
            let project = tmp.path().join("project");
            fs::create_dir(&project).unwrap();
            let cli = tmp.path().join("fixture-agent");
            fs::write(&cli, CLI).unwrap();
            fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
            for args in [
                vec!["init", "--quiet"],
                vec![
                    "-c",
                    "user.name=Tester",
                    "-c",
                    "user.email=test@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "--quiet",
                    "--allow-empty",
                    "-m",
                    "Base",
                ],
            ] {
                assert!(
                    Command::new("git")
                        .args(args)
                        .current_dir(&project)
                        .status()
                        .unwrap()
                        .success()
                );
            }
            // The provider override and data directory belong only to this subprocess.
            let out = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "state::tests::concurrent_turns_route_approvals_output_and_cancellation_to_their_owner", "--nocapture"])
                .env("UMIDE_WORKBENCH_DATA_DIR",tmp.path().join("data"))
                .env("UMIDE_CODEX_PATH",cli)
                .env("UMIDE_CONCURRENCY_PROJECT",project.canonicalize().unwrap())
                .output().unwrap();
            assert!(
                out.status.success(),
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        };
        let project = Path::new(&project);
        floem::reactive::Runtime::init_on_ui_thread();
        let wb = Workbench::new();
        wb.open(project);
        wb.draft.set("Primary task".into());
        wb.send();
        wb.new_chat();
        assert!(wb.isolated.get_untracked());
        wb.isolated.set(false);
        wb.draft.set("Second task".into());
        wb.send();
        assert_eq!(wb.history.with_untracked(|h| h.conversations.len()), 1);
        wb.isolated.set(true);
        wb.send();

        let wait = |condition: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !condition() {
                assert!(
                    Instant::now() < deadline,
                    "Timed out: {} {:?}",
                    wb.status.display(),
                    wb.history.get_untracked()
                );
                wb.poll_agent(0);
                wb.poll_agent(1);
                thread::sleep(Duration::from_millis(15));
            }
        };
        wait(&|| {
            wb.task_badge(0) == "Needs approval"
                && wb.task_badge(1) == "Needs approval"
        });
        assert!(wb.approvals.get_untracked()[0].1.contains("build isolated"));
        wb.decide(0, "7", true); // stale callback after switching conversations
        assert_eq!(wb.task_badge(0), "Needs approval");
        assert_eq!(wb.task_badge(1), "Needs approval");
        wb.decide(1, "7", true);
        wait(&|| wb.task_badge(1).is_empty());
        assert!(wb.any_busy());
        assert!(!wb.busy.get_untracked());
        wb.select(0);
        assert!(wb.approvals.get_untracked()[0].1.contains("build primary"));
        wb.decide(0, "7", false);
        wait(&|| !wb.any_busy());
        assert!(!project.join("accepted.txt").exists());

        let restored = projects::load_history(project).unwrap();
        let primary = &restored.conversations[0];
        let isolated = &restored.conversations[1];
        assert_eq!(primary.session_id.as_deref(), Some("primary"));
        assert_eq!(isolated.session_id.as_deref(), Some("isolated"));
        assert_eq!(primary.messages.last().unwrap().text, "primary denied");
        assert_eq!(isolated.messages.last().unwrap().text, "isolated accepted");
        let checkout = worktree::open(
            project,
            &projects::data_dir().unwrap(),
            isolated.worktree_id.as_ref().unwrap(),
        )
        .unwrap();
        assert!(checkout.join("accepted.txt").is_file());

        wb.draft.set("Resume primary".into());
        wb.send();
        wb.select(1);
        wb.draft.set("Resume isolated".into());
        wb.send();
        wait(&|| {
            wb.task_badge(0) == "Needs approval"
                && wb.task_badge(1) == "Needs approval"
        });
        wb.cancel();
        wait(&|| wb.task_badge(1).is_empty());
        assert_eq!(wb.task_badge(0), "Needs approval");
        wb.select(0);
        wb.cancel();
        wait(&|| !wb.any_busy());
        assert_eq!(
            projects::load_history(project).unwrap().conversations[1]
                .messages
                .last()
                .unwrap()
                .text,
            "Turn stopped"
        );

        // Losing a previously used checkout must not launch a provider in a
        // fresh checkout or in the main project.
        fs::rename(&checkout, project.parent().unwrap().join("moved-checkout"))
            .unwrap();
        wb.select(1);
        wb.draft.set("Reopen missing worktree".into());
        wb.send();
        wait(&|| !wb.any_busy());
        let after = projects::load_history(project).unwrap();
        assert_eq!(after.conversations[1].worktree_id, isolated.worktree_id);
        assert_eq!(
            after.conversations[1].messages.last().unwrap().role,
            "system"
        );

        // An app folder absent from HEAD fails during the first creation. Once
        // committed, the same conversation can retry with a new reservation.
        let nested = project.join("new-app");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("app.txt"), "app").unwrap();
        wb.open(&nested);
        wb.isolated.set(true);
        wb.draft.set("Start isolated".into());
        wb.send();
        wait(&|| !wb.any_busy());
        let failed = projects::load_history(&nested).unwrap();
        assert!(failed.conversations[0].isolated);
        assert!(failed.conversations[0].worktree_id.is_none());
        for args in [
            vec!["add", "new-app/app.txt"],
            vec![
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "Track app",
            ],
        ] {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(project)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        wb.draft.set("Retry setup".into());
        wb.send();
        wait(&|| wb.task_badge(0) == "Needs approval");
        wb.decide(0, "7", true);
        wait(&|| !wb.any_busy());
        assert!(
            projects::load_history(&nested).unwrap().conversations[0]
                .worktree_id
                .is_some()
        );
        assert!(!nested.join("accepted.txt").exists());
    }
}
