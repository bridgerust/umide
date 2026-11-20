//! Local web development servers belong to a workspace, independently of the
//! lifetime of its inspector, conversation, or native browser view.
use std::{collections::HashMap, thread, time::Duration};

use floem::{
    action::exec_after,
    ext_event::create_ext_action,
    prelude::*,
    reactive::{Memo, Scope},
    style::TextOverflow,
};

use crate::{
    choose_project,
    design::{
        Glyph, MONO, UiText, accent, action_button, button, danger, faint, icon,
        ink, line, mint_wash, muted, panel as panel_color, raised, subtle_line,
        surface,
    },
    state::{Workbench, Workspace},
    web_server::{Snapshot, Status, WebProject, WebServer},
};

struct Session {
    generation: u64,
    command: String,
    server: Option<WebServer>,
    snapshot: Snapshot,
}

impl Session {
    fn starting(generation: u64) -> Self {
        Self {
            generation,
            command: String::new(),
            snapshot: Snapshot {
                status: Status::Starting,
                output: String::new(),
            },
            server: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct WebPreview {
    scope: Scope,
    sessions: RwSignal<HashMap<Workspace, Session>>,
    closed: RwSignal<bool>,
}

fn active(status: &Status) -> bool {
    matches!(status, Status::Starting | Status::Ready(_))
}

impl WebPreview {
    pub(crate) fn new() -> Self {
        Self {
            scope: Scope::current(),
            sessions: RwSignal::new(HashMap::new()),
            closed: RwSignal::new(false),
        }
    }

    pub(crate) fn close(self) {
        self.closed.set(true);
        // Dropping each server stops and reaps its process group.
        self.sessions.update(HashMap::clear);
    }

    fn start(self, workspace: Workspace) {
        if self.closed.get_untracked() {
            return;
        }
        let generation = self.sessions.with_untracked(|sessions| {
            match sessions.get(&workspace) {
                Some(session) if active(&session.snapshot.status) => None,
                Some(session) => Some(session.generation.wrapping_add(1)),
                None => Some(1),
            }
        });
        let Some(generation) = generation else { return };
        self.sessions.update(|sessions| {
            sessions.insert(workspace.clone(), Session::starting(generation));
        });
        let selected = workspace.clone();
        let receive = create_ext_action(self.scope, move |result| {
            self.finish_detection(selected, generation, result);
        });
        thread::spawn(move || {
            receive(workspace.path().and_then(|path| WebProject::detect(&path)))
        });
    }

    fn finish_detection(
        self,
        workspace: Workspace,
        generation: u64,
        result: Result<WebProject, String>,
    ) {
        if self.closed.try_get_untracked() != Some(false) {
            return;
        }
        self.sessions.update(|sessions| {
            let Some(session) = sessions.get_mut(&workspace) else {
                return;
            };
            // Stop or a new Start may have overtaken project detection.
            if session.generation != generation
                || !matches!(session.snapshot.status, Status::Starting)
            {
                return;
            }
            match result.and_then(|project| {
                session.command = project.command_label();
                WebServer::start(project)
            }) {
                Ok(server) => {
                    session.snapshot = server.snapshot();
                    session.server = Some(server);
                }
                Err(message) => session.snapshot.status = Status::Failed(message),
            }
        });
        self.poll(workspace, generation);
    }

    fn stop(self, workspace: &Workspace) {
        self.sessions.update(|sessions| {
            if let Some(session) = sessions.get_mut(workspace) {
                session.generation = session.generation.wrapping_add(1);
                session.snapshot = match session.server.take() {
                    Some(server) => {
                        server.stop();
                        server.snapshot()
                    }
                    None => Snapshot {
                        status: Status::Stopped,
                        output: String::new(),
                    },
                };
            }
        });
    }

    fn poll(self, workspace: Workspace, generation: u64) {
        if self.closed.try_get_untracked() != Some(false) {
            return;
        }
        let observation = self.sessions.with_untracked(|sessions| {
            let session = sessions.get(&workspace)?;
            if session.generation != generation {
                return None;
            }
            let snapshot = session.server.as_ref()?.snapshot();
            let running = active(&snapshot.status);
            Some((running, (snapshot != session.snapshot).then_some(snapshot)))
        });
        let Some((running, change)) = observation else {
            return;
        };
        if let Some(snapshot) = change {
            self.sessions.update(|sessions| {
                if let Some(session) = sessions.get_mut(&workspace) {
                    session.snapshot = snapshot;
                }
            });
        }
        if running {
            exec_after(Duration::from_millis(350), move |_| {
                self.poll(workspace, generation)
            });
        }
    }
}

fn note(
    title: &'static str,
    detail: impl Into<String>,
    failed: bool,
) -> impl IntoView {
    let detail = detail.into();
    Stack::vertical((
        icon(
            if failed { Glyph::Terminal } else { Glyph::Code },
            27.,
            move || {
                if failed { danger() } else { accent() }
            },
        ),
        title.style(|s| s.font_size(18).ui_bold().color(ink())),
        Label::new(detail)
            .label_style(|s| s.selectable(true))
            .style(|s| {
                s.width_full()
                    .font_size(13)
                    .color(muted())
                    .line_height(1.4)
                    .text_overflow(TextOverflow::Wrap)
            }),
    ))
    .style(|s| {
        s.width_full()
            .min_height(0)
            .padding(18)
            .gap(8)
            .items_start()
    })
}

fn controls(
    wb: Workbench,
    web: WebPreview,
    workspace: Workspace,
    status: Option<Status>,
) -> impl IntoView {
    let running = status.as_ref().is_some_and(active);
    let command = if running {
        "Stop server"
    } else {
        "Start server"
    };
    let primary = action_button(
        if running { Glyph::Stop } else { Glyph::Play },
        command,
        move || {
            if running {
                web.stop(&workspace)
            } else {
                web.start(workspace.clone())
            }
        },
    )
    .style(move |s| {
        s.color(if running { ink() } else { accent() })
            .background(if running { surface() } else { mint_wash() })
    });
    let link = match status {
        Some(Status::Ready(url)) => {
            let copy_url = url.clone();
            Stack::horizontal((
                action_button(Glyph::Open, "Open browser", move || {
                    if let Err(error) = open::that_detached(&url) {
                        wb.status
                            .set(format!("Could not open the browser: {error}"));
                    }
                }),
                button("Copy URL", move || {
                    wb.status.set(
                        match floem::Clipboard::set_contents(copy_url.clone()) {
                            Ok(()) => "Preview URL copied".into(),
                            Err(_) => {
                                "Could not copy the preview URL to the clipboard"
                                    .into()
                            }
                        },
                    );
                }),
            ))
            .style(|s| s.items_center().gap(6))
            .into_any()
        }
        _ => Empty::new().into_any(),
    };
    Stack::horizontal((primary, link))
        .style(|s| s.width_full().items_center().gap(6))
}

fn frame(status: Option<Status>, refresh: RwSignal<u64>) -> impl IntoView {
    #[cfg(not(target_os = "macos"))]
    let _ = refresh;
    match status {
        Some(Status::Ready(url)) => {
            #[cfg(target_os = "macos")]
            {
                crate::browser::viewport(url, refresh)
                    .style(|s| s.width_full().min_height(0).flex_grow(1_f32))
                    .into_any()
            }
            #[cfg(not(target_os = "macos"))]
            {
                note(
                    "Server is live",
                    format!("Open {url} in your browser to preview this workspace."),
                    false,
                )
                .into_any()
            }
        }
        Some(Status::Failed(error)) => {
            Scroll::new(note("Server could not run", error, true))
                .style(|s| s.size_full())
                .into_any()
        }
        Some(Status::Starting) => {
            note("Starting server…", "Waiting for a local address.", false)
                .into_any()
        }
        Some(Status::Stopped) => note(
            "Server stopped",
            "Start the server to return to this preview.",
            false,
        )
        .into_any(),
        None => note(
            "Preview this workspace",
            "Start the local server to see your changes.",
            false,
        )
        .into_any(),
    }
}

fn workspace_panel(
    wb: Workbench,
    web: WebPreview,
    workspace: Workspace,
) -> impl IntoView {
    let status_key = workspace.clone();
    // Log lines must never recreate the embedded browser or move keyboard focus.
    let status = Memo::new(move |_| {
        web.sessions
            .with(|s| s.get(&status_key).map(|s| s.snapshot.status.clone()))
    });
    let controls_key = workspace.clone();
    let toolbar = dyn_container(
        move || status.get(),
        move |s| controls(wb, web, controls_key.clone(), s),
    );
    let refresh = RwSignal::new(0u64);
    let address = Label::derived(move || match status.get() {
        Some(Status::Ready(url)) => url,
        _ => String::new(),
    })
    .label_style(|s| s.selectable(true))
    .style(|s| {
        s.flex_grow(1_f32)
            .min_width(0)
            .text_ellipsis()
            .font_family(MONO.to_owned())
            .font_size(12)
            .color(accent())
    });
    #[cfg(target_os = "macos")]
    let address = Stack::horizontal((
        address,
        action_button(Glyph::Refresh, "Reload preview", move || {
            refresh.update(|generation| *generation = generation.wrapping_add(1));
        })
        .style(|s| s.height(25).padding_horiz(6).font_size(11)),
    ))
    .style(|s| s.items_center().gap(6));
    let address = address.style(move |s| {
        s.width_full()
            .apply_if(!matches!(status.get(), Some(Status::Ready(_))), |s| {
                s.hide()
            })
    });
    let browser = dyn_container(move || status.get(), move |s| frame(s, refresh))
        .style(|s| {
            s.width_full()
                .flex_col()
                .flex_grow(1_f32)
                .flex_basis(0)
                .min_height(0)
                .border(1)
                .border_color(subtle_line())
                .background(surface())
                .border_radius(6)
        });

    let logs_open = RwSignal::new(false);
    let command_key = workspace.clone();
    let command = Label::derived(move || {
        web.sessions.with(|s| {
            s.get(&command_key)
                .map(|s| s.command.clone())
                .unwrap_or_default()
        })
    })
    .label_style(|s| s.selectable(true))
    .style(|s| {
        s.flex_grow(1_f32)
            .min_width(0)
            .text_ellipsis()
            .font_family(MONO.to_owned())
            .font_size(11)
            .color(muted())
    });
    let log_button = Button::new(Label::derived(move || {
        if logs_open.get() {
            "Hide logs"
        } else {
            "Show logs"
        }
    }))
    .action(move || logs_open.update(|show| *show = !*show))
    .style(|s| {
        s.height(27)
            .padding_horiz(7)
            .border(0)
            .border_radius(5)
            .background(panel_color())
            .color(muted())
            .font_size(12)
            .hover(|s| s.background(raised()))
            .focus_visible(|s| s.outline(2).outline_color(accent()))
    });
    let logbar =
        Stack::horizontal((icon(Glyph::Terminal, 14., faint), command, log_button))
            .style(|s| s.width_full().items_center().gap(6));
    let logs = Label::derived(move || {
        web.sessions.with(|s| {
            s.get(&workspace)
                .map(|s| s.snapshot.output.as_str())
                .filter(|out| !out.is_empty())
                .unwrap_or("Waiting for server output…")
                .to_owned()
        })
    })
    .label_style(|s| s.selectable(true))
    .style(|s| {
        s.width_full()
            .font_family(MONO.to_owned())
            .font_size(12)
            .line_height(1.35)
            .color(ink())
            .text_overflow(TextOverflow::Wrap)
            .padding(8)
    });
    let console = Scroll::new(logs).style(move |s| {
        s.width_full()
            .height(130)
            .background(surface())
            .border(1)
            .border_color(line())
            .border_radius(5)
            .apply_if(!logs_open.get(), |s| s.hide())
    });
    Stack::vertical((toolbar, address, browser, logbar, console))
        .style(|s| s.size_full().min_size(0, 0).gap(8))
}

pub(crate) fn panel(wb: Workbench, web: WebPreview) -> impl IntoView {
    let selected = Memo::new(move |_| {
        wb.root.track();
        wb.current.track();
        wb.history.track();
        wb.isolated.track();
        wb.selected_workspace()
    });
    let title = "Web preview".style(|s| s.font_size(21).ui_bold().color(ink()));
    let folder = Label::derived(move || {
        wb.root.with(|root| {
            let Some(root) = root else {
                return String::new();
            };
            let name = root
                .file_name()
                .unwrap_or(root.as_os_str())
                .to_string_lossy();
            if wb.isolated.get() {
                format!("{name} · Isolated Git copy")
            } else {
                format!("{name} · Project folder")
            }
        })
    })
    .style(|s| s.width_full().text_ellipsis().font_size(12).color(muted()));
    let body = dyn_container(
        move || selected.get(),
        move |selection| match selection {
            Ok(workspace) => workspace_panel(wb, web, workspace).into_any(),
            Err(error) => {
                let open = wb.root.with_untracked(Option::is_none);
                Stack::vertical((
                    note(
                        if open {
                            "Open a web project"
                        } else {
                            "Workspace unavailable"
                        },
                        error,
                        !open,
                    ),
                    button("Open project…", move || choose_project(wb)).style(
                        move |s| s.margin_left(18).apply_if(!open, |s| s.hide()),
                    ),
                ))
                .style(|s| s.items_start())
                .into_any()
            }
        },
    )
    .style(|s| s.width_full().min_height(0).flex_grow(1_f32));
    Stack::vertical((title, folder, body)).style(|s| {
        s.size_full()
            .min_size(0, 0)
            .padding(14)
            .gap(8)
            .background(panel_color())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::Conversation;

    #[test]
    fn late_detection_cannot_restart_a_stopped_workspace_or_replace_another_preview()
    {
        let wb = Workbench::new();
        wb.root.set(Some("project".into()));
        let primary = wb.selected_workspace().unwrap();
        wb.history.update(|history| {
            history.conversations.push(Conversation {
                isolated: true,
                worktree_id: Some("isolated-key".into()),
                ..Default::default()
            });
        });
        wb.current.set(Some(0));
        let isolated = wb.selected_workspace().unwrap();
        let web = WebPreview::new();
        web.sessions.update(|sessions| {
            sessions.insert(primary.clone(), Session::starting(1));
            sessions.insert(isolated.clone(), Session::starting(1));
        });
        web.stop(&primary);
        web.finish_detection(primary.clone(), 1, Err("Late detection".into()));
        web.finish_detection(
            isolated.clone(),
            1,
            Err("No development script".into()),
        );
        web.sessions.with_untracked(|sessions| {
            assert_eq!(sessions[&primary].snapshot.status, Status::Stopped);
            assert_eq!(
                sessions[&isolated].snapshot.status,
                Status::Failed("No development script".into())
            );
        });
        web.close();
        web.finish_detection(isolated, 1, Err("Late after close".into()));
        assert!(web.sessions.with_untracked(HashMap::is_empty));
    }
}
