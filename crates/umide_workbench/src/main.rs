mod devices;
mod projects;
mod state;
mod worktree;

use std::path::PathBuf;

use floem::{
    Application, FileDialogOptions, prelude::*, style::TextOverflow,
    window::WindowConfig,
};
use state::{Tab, Workbench, provider_name};
use umide_workbench_agents::Provider;

const INK: Color = Color::from_rgb8(231, 235, 237);
const MUTED: Color = Color::from_rgb8(158, 172, 182);
const CANVAS: Color = Color::from_rgb8(20, 24, 29);
const PANEL: Color = Color::from_rgb8(28, 34, 40);
const LINE: Color = Color::from_rgb8(54, 65, 74);
const ACCENT: Color = Color::from_rgb8(135, 225, 196);

fn button(title: &'static str, action: impl Fn() + 'static) -> impl IntoView {
    Button::new(title).action(action).style(|s| {
        s.background(PANEL)
            .color(INK)
            .font_size(13)
            .border(1)
            .border_color(LINE)
            .border_radius(6)
            .padding_horiz(10)
            .padding_vert(6)
            .hover(|s| s.background(LINE))
            .disabled(|s| s.color(MUTED).background(CANVAS))
    })
}

fn choose_project(wb: Workbench) {
    floem::open_file(FileDialogOptions::new().select_directories(), move |info| {
        if let Some(path) = info.and_then(|x| x.path.into_iter().next()) {
            wb.open(&path);
        }
    });
}

fn sidebar(wb: Workbench) -> impl IntoView {
    let sessions = dyn_stack(
        move || {
            wb.history
                .with(|h| (0..h.conversations.len()).rev().collect::<Vec<_>>())
        },
        |index| *index,
        move |index| {
            let title = Label::derived(move || {
                wb.history.with(|h| {
                    h.conversations
                        .get(index)
                        .map(|c| c.title.clone())
                        .unwrap_or_default()
                })
            })
            .style(|s| s.width_full().text_ellipsis());
            let detail = Label::derived(move || {
                let source = wb.history.with(|h| {
                    h.conversations
                        .get(index)
                        .map(|c| {
                            if c.provider == "claude" {
                                "Claude Code"
                            } else {
                                "Codex"
                            }
                        })
                        .unwrap_or("")
                });
                let badge = wb.task_badge(index);
                if badge.is_empty() {
                    source.to_owned()
                } else {
                    format!("{source} · {badge}")
                }
            })
            .style(move |s| {
                s.font_size(11).width_full().text_ellipsis().color(
                    if wb.task_badge(index).is_empty() {
                        MUTED
                    } else {
                        ACCENT
                    },
                )
            });
            Button::new(
                Stack::vertical((title, detail)).style(|s| s.width_full().gap(3)),
            )
            .action(move || wb.select(index))
            .style(move |s| {
                let active = wb.current.get() == Some(index);
                s.width_full()
                    .justify_start()
                    .min_width(0)
                    .padding(8)
                    .border(0)
                    .background(if active { LINE } else { PANEL })
                    .border_radius(5)
                    .color(if active { INK } else { MUTED })
                    .hover(|s| s.background(LINE))
            })
        },
    )
    .style(|s| s.flex_col().width_full().gap(4));
    Stack::vertical((
        "UMIDE".style(|s| s.font_bold().font_size(19).color(ACCENT)),
        Label::derived(move || {
            wb.root.with(|p| {
                p.as_ref()
                    .and_then(|p| p.file_name())
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "No project open".into())
            })
        })
        .style(|s| {
            s.margin_top(16)
                .font_bold()
                .font_size(15)
                .max_width_full()
                .text_ellipsis()
        }),
        button("Open project…", move || choose_project(wb))
            .style(move |s| s.set_disabled(wb.any_busy() || wb.launching.get())),
        "Conversations".style(|s| s.color(MUTED).font_size(12).margin_top(16)),
        button("+ New conversation", move || wb.new_chat())
            .style(move |s| s.set_disabled(wb.root.with(Option::is_none))),
        Scroll::new(sessions)
            .style(|s| s.width_full().flex_grow(1_f32).min_height(0)),
    ))
    .style(|s| {
        s.width(205)
            .height_full()
            .flex_shrink(0_f32)
            .padding(16)
            .gap(9)
            .background(PANEL)
            .border_right(1)
            .border_color(LINE)
    })
}

fn chat(wb: Workbench) -> impl IntoView {
    let conversation = dyn_container(move || (wb.current.get(),wb.root.with(Option::is_some)),
        move |(index,open)| {
            let Some(index) = index else {
                return Stack::vertical((
                    (if open { "Start a conversation" } else { "Open a project" })
                        .style(|s|s.font_size(24).font_bold()),
                    (if open { "Choose an agent and describe a change to your app." }
                     else { "Choose the folder containing your app to start working with an agent." })
                        .style(|s|s.font_size(14).color(MUTED).text_overflow(TextOverflow::Wrap)),
                )).style(|s|s.width_full().padding(24).gap(10)).into_any();
            };
            let messages = dyn_stack(move || wb.history.with(|h| {
                let n = h.conversations.get(index).map_or(0,|c|c.messages.len());
                0..n
            }), |n| *n, move |n| {
                let role = Label::derived(move || wb.history.with(|h| h.conversations.get(index)
                    .and_then(|c| c.messages.get(n).map(|m| match m.role.as_str() {
                        "user" => "You", "system" => "Umide", _ if c.provider=="claude" => "Claude Code", _ => "Codex"
                    })).unwrap_or_default().to_string()))
                    .style(|s|s.font_bold().font_size(12).color(ACCENT));
                let body = Label::derived(move || wb.history.with(|h| h.conversations.get(index)
                    .and_then(|c| c.messages.get(n)).map(|m|m.text.clone()).unwrap_or_default()))
                    .style(|s|s.width_full().font_size(14).text_overflow(TextOverflow::Wrap));
                Stack::vertical((role,body)).style(|s|s.width_full().gap(6).padding_vert(8))
            }).style(|s|s.flex_col().width_full().padding_horiz(18).gap(6));
            Scroll::new(messages).scroll_to_percent(move || { wb.history.track(); 100. })
                .style(|s|s.size_full().min_height(0)).into_any()
        }).style(|s|s.width_full().flex_grow(1_f32).min_height(0));

    let approvals = dyn_container(
        move || {
            wb.current.get().and_then(|index| {
                wb.approvals.with(|items| {
                    items.first().cloned().map(|(id, desc)| (index, id, desc))
                })
            })
        },
        move |next| {
            let Some((index, id, description)) = next else {
                return floem::views::Empty::new().into_any();
            };
            let allow_id = id.clone();
            Stack::vertical((
                "Agent requests permission".style(|s| s.font_bold()),
                Scroll::new(
                    description
                        .style(|s| s.width_full().text_overflow(TextOverflow::Wrap)),
                )
                .style(|s| s.max_height(120).width_full()),
                Stack::horizontal((
                    button("Allow once", move || wb.decide(index, &allow_id, true)),
                    button("Deny", move || wb.decide(index, &id, false)),
                ))
                .style(|s| s.gap(8)),
            ))
            .style(|s| {
                s.width_full()
                    .gap(8)
                    .padding(12)
                    .background(PANEL)
                    .border(1)
                    .border_color(ACCENT)
            })
            .into_any()
        },
    );
    let provider_button = move |p| {
        button(provider_name(p), move || wb.set_provider(p)).style(move |s| {
            s.set_disabled(wb.busy.get())
                .apply_if(wb.provider.get() == p, |s| {
                    s.color(ACCENT).border_color(ACCENT)
                })
        })
    };
    let controls = Stack::horizontal((
        provider_button(Provider::Codex),
        provider_button(Provider::Claude),
        button("Attach screen", move || wb.attach.update(|v| *v = !*v)).style(
            move |s| {
                s.set_disabled(wb.busy.get() || wb.target.with(Option::is_none))
                    .apply_if(wb.attach.get(), |s| {
                        s.color(ACCENT).border_color(ACCENT)
                    })
            },
        ),
    ))
    .style(|s| s.gap(6).items_center());
    let input = text_input(wb.draft)
        .placeholder("Describe a change to your app…")
        .on_key_down(
            Key::Named(NamedKey::Enter),
            |m| m.is_empty(),
            move |_| wb.send(),
        )
        .style(move |s| {
            s.min_width(0)
                .flex_grow(1_f32)
                .height(38)
                .padding_horiz(10)
                .background(PANEL)
                .border(1)
                .border_radius(6)
                .border_color(LINE)
                .color(INK)
                .set_disabled(wb.root.with(Option::is_none) || wb.busy.get())
        });
    let send = dyn_container(
        move || wb.busy.get(),
        move |busy| {
            if busy {
                button("Stop", move || wb.cancel()).into_any()
            } else {
                button("Send", move || wb.send())
                    .style(move |s| {
                        s.background(ACCENT).color(CANVAS).set_disabled(
                            wb.root.with(Option::is_none)
                                || wb.draft.with(|v| v.trim().is_empty()),
                        )
                    })
                    .into_any()
            }
        },
    );
    let location = dyn_container(
        move || wb.current.get().is_some(),
        move |existing| {
            if existing {
                return Label::derived(move || {
                    if wb.isolated.get() {
                        "Workspace · Isolated Git copy"
                    } else {
                        "Workspace · Project folder"
                    }
                })
                .style(|s| s.color(MUTED).font_size(12))
                .into_any();
            }
            let option = move |name, value| {
                button(name, move || wb.isolated.set(value)).style(move |s| {
                    s.font_size(12)
                        .padding_vert(3)
                        .set_disabled(wb.root.with(Option::is_none))
                        .apply_if(wb.isolated.get() == value, |s| {
                            s.color(ACCENT).border_color(ACCENT)
                        })
                })
            };
            Stack::vertical((
            Stack::horizontal(("Workspace".style(|s|s.font_size(12).color(MUTED)),
                option("Project folder",false),option("Isolated Git copy",true)))
                .style(|s|s.items_center().gap(6)),
            Label::derived(move || if wb.isolated.get() { "Starts at the latest commit and keeps edits in a separate folder." }
                else { "Edits the files in the open project." })
                .style(|s|s.font_size(11).color(MUTED).width_full().text_overflow(TextOverflow::Wrap)),
        )).style(|s|s.gap(4).width_full()).into_any()
        },
    );
    let composer = Stack::vertical((
        approvals,
        location,
        controls,
        Stack::horizontal((input, send))
            .style(|s| s.gap(8).width_full().items_center()),
    ))
    .style(|s| {
        s.gap(9)
            .width_full()
            .padding(14)
            .border_top(1)
            .border_color(LINE)
    });
    Stack::vertical((conversation, composer)).style(|s| s.size_full().min_size(0, 0))
}

fn changes_view(wb: Workbench) -> impl IntoView {
    let open = button("Open task folder", move || {
        let Some(project) = wb.root.get_untracked() else {
            return;
        };
        let Some(index) = wb.current.get_untracked() else {
            return;
        };
        let Some(key) = wb.history.with_untracked(|h| {
            h.conversations
                .get(index)
                .and_then(|c| c.worktree_id.clone())
        }) else {
            return;
        };
        let expected = project.clone();
        let report = floem::ext_event::create_ext_action(
            floem::reactive::Scope::current(),
            move |result: Result<(), String>| {
                let Err(error) = result else {
                    return;
                };
                if wb.current.get_untracked() == Some(index)
                    && wb.root.get_untracked().as_ref() == Some(&expected)
                {
                    wb.status.set(error);
                }
            },
        );
        std::thread::spawn(move || {
            let result = projects::data_dir()
                .and_then(|data| worktree::open(&project, &data, &key))
                .and_then(|path| open_folder(&path));
            report(result);
        });
    })
    .style(move |s| {
        s.apply_if(
            !wb.current.get().is_some_and(|i| {
                wb.history.with(|h| {
                    h.conversations
                        .get(i)
                        .is_some_and(|c| c.worktree_id.is_some())
                })
            }),
            |s| s.hide(),
        )
    });
    let context = Label::derived(move || {
        if wb.isolated.get() && wb.current.get().is_some() {
            "Changes since the task started"
        } else {
            "Project changes"
        }
    })
    .style(|s| s.font_bold().font_size(14));
    let bar = Stack::horizontal((
        context,
        open,
        button("Refresh", move || wb.show_diff()),
    ))
    .style(|s| s.items_center().gap(8).width_full());
    let diff = Label::derived(move || wb.changes.get()).style(|s| {
        s.font_family("monospace".to_string())
            .font_size(12)
            .width_full()
            .text_overflow(TextOverflow::Wrap)
    });
    Stack::vertical((
        bar,
        Scroll::new(diff).style(|s| s.width_full().min_height(0).flex_grow(1_f32)),
    ))
    .style(|s| s.size_full().padding(12).min_size(0, 0).gap(12))
}

fn open_folder(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let program = "/usr/bin/open";
    #[cfg(target_os = "windows")]
    let program = "explorer.exe";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let program = "xdg-open";
    let status = std::process::Command::new(program)
        .arg(path)
        .status()
        .map_err(|e| format!("Could not open the task folder: {e}"))?;
    if status.success() || (cfg!(windows) && status.code() == Some(1)) {
        Ok(())
    } else {
        Err(format!("Could not open the task folder ({status})"))
    }
}

fn output_text(value: impl Fn() -> String + 'static) -> impl IntoView {
    let label = Label::derived(value).style(|s| {
        s.font_family("monospace".to_string())
            .font_size(12)
            .width_full()
            .text_overflow(TextOverflow::Wrap)
    });
    Scroll::new(label)
        .style(|s| s.width_full().min_size(0, 0).flex_grow(1_f32).padding(16))
}

fn workspace(wb: Workbench) -> impl IntoView {
    let tab = move |name, t| {
        button(name, move || {
            if t == Tab::Changes {
                wb.show_diff()
            } else if t == Tab::Logs {
                let refresh = wb.tab.get_untracked() == Tab::Logs;
                wb.tab.set(t);
                if refresh {
                    wb.refresh_logs();
                }
            } else {
                wb.tab.set(t)
            }
        })
        .style(move |s| {
            s.apply_if(wb.tab.get() == t, |s| s.color(ACCENT).border_color(ACCENT))
        })
    };
    let tabs = Stack::horizontal((
        tab("Conversation", Tab::Chat),
        tab("Changes", Tab::Changes),
        tab("Build", Tab::Output),
        tab("Device logs", Tab::Logs),
    ))
    .style(|s| {
        s.gap(6)
            .padding(12)
            .width_full()
            .border_bottom(1)
            .border_color(LINE)
    });
    let body = dyn_container(move || wb.tab.get(), move |tab| {
        match tab {
            Tab::Chat => chat(wb).into_any(),
            Tab::Changes => changes_view(wb).into_any(),
            Tab::Logs => {
                floem::reactive::Effect::new(move |_|{
                    wb.target.track();
                    wb.refresh_logs();
                });
                output_text(move ||wb.logs.get()).into_any()
            }
            Tab::Output => {
                let owner = Label::derived(move || wb.build_label.get())
                    .style(|s|s.width_full().text_ellipsis().font_bold());
                let stop = button("Stop build", move ||wb.stop_app())
                    .style(move |s|s.apply_if(!wb.launching.get(),|s|s.hide()));
                Stack::vertical((
                    Stack::vertical((owner,stop)).style(move |s|s.gap(4).width_full()
                        .padding(12).apply_if(wb.build_label.with(String::is_empty),|s|s.hide())),
                    output_text(move ||{
                        let out = wb.output.get();
                        if out.is_empty() { "Select a running device and use Run app to see build output.".into() } else {out}
                    })
                )).style(|s|s.size_full().min_size(0,0)).into_any()
            }
        }
    }).style(|s|s.flex_grow(1_f32).min_height(0).width_full());
    Stack::vertical((
        tabs,
        body,
        Label::derived(move || wb.status.get()).style(|s| {
            s.width_full()
                .text_ellipsis()
                .font_size(12)
                .color(MUTED)
                .padding_horiz(14)
                .padding_vert(6)
        }),
    ))
    .style(|s| s.flex_grow(1_f32).flex_basis(0).min_width(0).height_full())
}

fn app(initial: Option<PathBuf>) -> impl IntoView {
    let wb = Workbench::new();
    if let Some(project) = initial.or_else(projects::last_project) {
        wb.open(&project);
    }
    let actions = dyn_container(
        move || (wb.launching.get(), wb.current.get() == wb.build_owner.get()),
        move |(running, owned)| {
            if running && !owned {
                button("View active build", move || wb.tab.set(Tab::Output))
                    .into_any()
            } else if running {
                button("Stop build", move || wb.stop_app()).into_any()
            } else {
                button("Run app", move || wb.launch())
                    .style(move |s| {
                        s.set_disabled(
                            wb.target.with(Option::is_none)
                                || wb.root.with(Option::is_none),
                        )
                    })
                    .into_any()
            }
        },
    );
    let device =
        Stack::vertical((devices::device_panel(wb.target), actions)).style(|s| {
            s.width(330)
                .height_full()
                .padding(12)
                .gap(10)
                .flex_shrink(0_f32)
                .background(PANEL)
                .border_left(1)
                .border_color(LINE)
        });
    Stack::horizontal((sidebar(wb), workspace(wb), device))
        .style(|s| s.size_full().background(CANVAS).color(INK).font_size(14))
        .on_cleanup(move || wb.close())
}

fn main() {
    let project = std::env::args_os().nth(1).map(PathBuf::from);
    let config = WindowConfig::default()
        .title("Umide Workbench")
        .theme_override(floem::window::Theme::Dark)
        .size((1200., 790.))
        .min_size((960., 600.));
    Application::new()
        .window(move |_| app(project), Some(config))
        .run();
}
