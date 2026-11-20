use std::{
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
    action::exec_after, ext_event::create_ext_action, prelude::*, reactive::Scope,
};
use tempfile::NamedTempFile;
use umide_workbench_agents::{self as agents, Event, Provider, Run};

use crate::{
    devices,
    projects::{self, AppRun, Conversation, History, Message},
};

pub type Target = (String, String);
type Prepared = Result<(Run, Option<NamedTempFile>), String>;

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Chat,
    Changes,
    Output,
    Logs,
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
    pub status: RwSignal<String>,
    pub changes: RwSignal<String>,
    pub logs: RwSignal<String>,
    pub output: RwSignal<String>,
    pub busy: RwSignal<bool>,
    pub launching: RwSignal<bool>,
    pub approvals: RwSignal<Vec<(String, String)>>,
    run: RwSignal<Option<Run>>,
    preparing: RwSignal<Option<Receiver<Prepared>>>,
    picture: RwSignal<Option<NamedTempFile>>,
    app: RwSignal<Option<AppRun>>,
    cancelled: RwSignal<Arc<AtomicBool>>,
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
            logs: RwSignal::new(String::new()),
            log_seq: RwSignal::new(0),
            stop_build_requested: RwSignal::new(false),
            history: RwSignal::new(History::default()),
            current: RwSignal::new(None),
            provider: RwSignal::new(Provider::Codex),
            draft: RwSignal::new(String::new()),
            target: RwSignal::new(None),
            attach: RwSignal::new(false),
            tab: RwSignal::new(Tab::Chat),
            status: RwSignal::new(String::from("Ready")),
            changes: RwSignal::new(String::new()),
            output: RwSignal::new(String::new()),
            busy: RwSignal::new(false),
            launching: RwSignal::new(false),
            approvals: RwSignal::new(vec![]),
            run: RwSignal::new(None),
            preparing: RwSignal::new(None),
            picture: RwSignal::new(None),
            app: RwSignal::new(None),
            cancelled: RwSignal::new(Arc::new(AtomicBool::new(false))),
            diff_seq: RwSignal::new(0),
        }
    }

    pub fn open(self, root: &Path) {
        if self.busy.get_untracked() || self.launching.get_untracked() {
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
                self.root.set(Some(root.clone()));
                self.history.set(history);
                self.new_chat();
                if let Some(index) = last {
                    self.select(index);
                }
                self.changes.set(String::new());
                self.output.set(String::new());
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

    pub fn new_chat(self) {
        if self.busy.get_untracked() {
            return;
        }
        self.current.set(None);
        self.draft.set(String::new());
        self.attach.set(false);
        self.tab.set(Tab::Chat);
    }

    pub fn select(self, index: usize) {
        if self.busy.get_untracked() {
            return;
        }
        let provider = self.history.with_untracked(|h| {
            h.conversations.get(index).map(|c| {
                if c.provider == "claude" {
                    Provider::Claude
                } else {
                    Provider::Codex
                }
            })
        });
        if let Some(p) = provider {
            self.new_chat();
            self.provider.set(p);
            self.current.set(Some(index));
        }
    }

    fn persist(self) {
        let Some(root) = self.root.get_untracked() else {
            return;
        };
        if let Err(e) = self
            .history
            .with_untracked(|h| projects::save_history(&root, h))
        {
            self.status.set(format!("Could not save conversation: {e}"));
        }
    }

    pub fn send(self) {
        if self.busy.get_untracked() {
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
        let index = self.current.get_untracked().unwrap_or_else(|| {
            let index = self.history.with_untracked(|h| h.conversations.len());
            self.history.update(|h| {
                h.conversations.push(Conversation {
                    title: draft.chars().take(52).collect(),
                    provider: match p {
                        Provider::Codex => "codex",
                        Provider::Claude => "claude",
                    }
                    .into(),
                    session_id: None,
                    messages: vec![],
                })
            });
            self.current.set(Some(index));
            index
        });
        let session = self
            .history
            .with_untracked(|h| h.conversations[index].session_id.clone());
        let mut displayed = draft.clone();
        if let Some((platform, id)) = &device {
            displayed.push_str(&format!("\n[Screen: {platform} · {id}]"));
        }
        self.history.update(|h| {
            h.conversations[index].messages.extend([
                Message {
                    role: "user".into(),
                    text: displayed,
                },
                Message {
                    role: "assistant".into(),
                    text: String::new(),
                },
            ])
        });
        self.draft.set(String::new());
        self.attach.set(false);
        self.tab.set(Tab::Chat);
        self.busy.set(true);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.cancelled.set(cancelled.clone());
        self.status.set(format!("Starting {}", provider_name(p)));
        self.persist();
        let (tx, rx) = mpsc::channel();
        self.preparing.set(Some(rx));
        thread::spawn(move || {
            let result = (|| {
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
                    return Err("Turn stopped".into());
                }
                let run = agents::start_with_image(
                    p,
                    &root,
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
        self.poll_agent();
    }

    fn poll_agent(self) {
        if !self.busy.get_untracked() {
            return;
        }
        let ready = self
            .preparing
            .with_untracked(|r| r.as_ref().map(Receiver::try_recv));
        match ready {
            Some(Ok(Ok((run, file)))) => {
                self.preparing.set(None);
                if self
                    .cancelled
                    .with_untracked(|flag| flag.load(Ordering::Acquire))
                {
                    run.control.cancel();
                }
                self.run.set(Some(run));
                self.picture.set(file);
            }
            Some(Ok(Err(e))) => {
                self.finish(Some(e));
                return;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.finish(Some("Agent failed to start".into()));
                return;
            }
            _ => {}
        }
        let mut text = String::new();
        let mut terminal = None;
        self.run.with_untracked(|run| {
            let Some(run) = run else {
                return;
            };
            for _ in 0..256 {
                match run.events.try_recv() {
                    Ok(Event::Text(s)) => text.push_str(&s),
                    Ok(Event::Activity(s)) => self.status.set(s),
                    Ok(Event::Session(id)) => {
                        if let Some(i) = self.current.get_untracked() {
                            self.history.update(|h| {
                                h.conversations[i].session_id = Some(id)
                            });
                        }
                    }
                    Ok(Event::Approval { id, description }) => {
                        self.tab.set(Tab::Chat);
                        self.status.set("Waiting for permission".into());
                        self.approvals.update(|a| a.push((id, description)))
                    }
                    Ok(Event::Error(e)) => {
                        terminal = Some(Some(e));
                        break;
                    }
                    Ok(Event::Cancelled) => {
                        terminal = Some(Some("Turn stopped".into()));
                        break;
                    }
                    Ok(Event::Done) => {
                        terminal = Some(None);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        terminal = Some(Some("Agent disconnected".into()));
                        break;
                    }
                }
            }
        });
        if let Some(i) = self.current.get_untracked().filter(|_| !text.is_empty()) {
            self.history.update(|h| {
                if let Some(m) = h.conversations[i].messages.last_mut() {
                    m.text.push_str(&text);
                }
            });
        }
        if let Some(result) = terminal {
            self.finish(result);
        } else {
            exec_after(Duration::from_millis(50), move |_| self.poll_agent());
        }
    }

    fn finish(self, issue: Option<String>) {
        if let Some(i) = self.current.get_untracked() {
            self.history.update(|h| {
                let m = &mut h.conversations[i].messages;
                if m.last()
                    .is_some_and(|m| m.role == "assistant" && m.text.is_empty())
                {
                    m.pop();
                }
                if let Some(e) = &issue {
                    m.push(Message {
                        role: "system".into(),
                        text: e.clone(),
                    });
                }
            });
        }
        self.preparing.set(None);
        self.run.set(None);
        self.picture.set(None);
        self.busy.set(false);
        self.approvals.set(vec![]);
        self.status.set(issue.unwrap_or_else(|| "Ready".into()));
        self.persist();
        if self.tab.get_untracked() == Tab::Changes {
            self.show_diff();
        }
    }

    pub fn cancel(self) {
        self.cancelled
            .with_untracked(|flag| flag.store(true, Ordering::Release));
        self.run.with_untracked(|r| {
            if let Some(r) = r {
                r.control.cancel();
            }
        });
        self.status.set("Stopping agent…".into());
    }

    pub fn decide(self, allow: bool) {
        if let Some((id, _)) = self.approvals.with_untracked(|a| a.first().cloned())
        {
            self.run.with_untracked(|r| {
                if let Some(r) = r {
                    r.control.decide(&id, allow);
                }
            });
            self.approvals.update(|a| {
                a.remove(0);
            });
        }
    }

    pub fn show_diff(self) {
        self.tab.set(Tab::Changes);
        let Some(root) = self.root.get_untracked() else {
            return;
        };
        let seq = self.diff_seq.get_untracked().wrapping_add(1);
        self.diff_seq.set(seq);
        self.changes.set("Reading changes…".into());
        let expected = root.clone();
        let send =
            create_ext_action(self.scope, move |result: Result<String, String>| {
                if self.diff_seq.get_untracked() == seq
                    && self.root.get_untracked().as_ref() == Some(&expected)
                {
                    self.changes.set(match result {
                        Ok(s) if s.trim().is_empty() => {
                            "Working tree is clean.".into()
                        }
                        Ok(s) | Err(s) => s,
                    });
                }
            });
        thread::spawn(move || send(projects::git_diff(&root)));
    }

    pub fn refresh_logs(self) {
        let next = self.log_seq.get_untracked().wrapping_add(1);
        self.log_seq.set(next);
        let Some((platform, id)) = self.target.get_untracked() else {
            self.logs
                .set("Select a running device to read its logs.".into());
            return;
        };
        self.logs.set("Reading device logs…".into());
        let receive =
            create_ext_action(self.scope, move |result: Result<String, String>| {
                if next == self.log_seq.get_untracked() {
                    self.logs.set(match result {
                        Ok(logs) if logs.trim().is_empty() => {
                            "No recent messages on this device.".into()
                        }
                        Ok(logs) | Err(logs) => logs,
                    });
                }
            });
        thread::spawn(move || receive(projects::device_logs(&platform, &id)));
    }

    pub fn launch(self) {
        let (Some(project), Some((platform, id))) =
            (self.root.get_untracked(), self.target.get_untracked())
        else {
            return;
        };
        if self.launching.get_untracked() {
            return;
        }
        self.tab.set(Tab::Output);
        self.launching.set(true);
        self.stop_build_requested.set(false);
        self.output.set(format!("Starting build on {id}…"));
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
                        self.output.set(e);
                    }
                }
            });
        thread::spawn(move || send(AppRun::start(&project, &platform, &id)));
    }

    fn poll_build(self) {
        let Some((running, out)) = self
            .app
            .with_untracked(|a| a.as_ref().map(AppRun::snapshot))
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
        if self.busy.get_untracked() {
            self.cancel();
            self.finish(Some("Window closed during this turn".into()));
        }
        self.stop_app();
        self.app.set(None);
    }
}
