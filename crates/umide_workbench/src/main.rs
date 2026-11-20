mod agent_memory;
mod appearance;
#[cfg(target_os = "macos")]
mod browser;
mod conversation;
mod design;
mod devices;
mod editor_picker;
mod external_editor;
mod file_editor;
mod file_explorer;
mod files_panel;
mod i18n;
mod navigation;
mod onboarding;
mod orphan_drafts;
mod project_files;
mod projects;
mod provider_setup;
mod review;
mod state;
mod terminal_dock;
mod terminal_session;
mod typography;
mod verify;
mod web_preview;
mod web_server;
mod workbench_terminal;
mod worktree;

use std::path::PathBuf;

use conversation::chat;
use design::{button, canvas};
use floem::{
    Application, FileDialogOptions, prelude::*, reactive::Memo, style::TextOverflow,
    window::WindowConfig,
};
use review::changes_view;
use state::{Tab, Workbench};

use design::*;
use i18n::{label, tr};

fn choose_project(wb: Workbench) {
    floem::open_file(
        FileDialogOptions::new()
            .title(tr("Open a project"))
            .select_directories(),
        move |info| {
            if let Some(path) = info.and_then(|x| x.path.into_iter().next()) {
                wb.open(&path);
            }
        },
    );
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

fn output_view(wb: Workbench, tab: Tab) -> impl IntoView {
    let is_logs = tab == Tab::Logs;
    if is_logs {
        floem::reactive::Effect::new(move |_| {
            wb.target.track();
            wb.refresh_logs();
        });
    }
    let title = if is_logs {
        "Device logs"
    } else {
        "Build output"
    };
    let context = Label::derived(move || {
        if is_logs {
            wb.target.with(|t| {
                t.as_ref()
                    .map(|(platform, id)| format!("{platform}  /  {id}"))
                    .unwrap_or_else(|| {
                        tr("Select a device to inspect its recent activity.").into()
                    })
            })
        } else {
            let label = wb.build_label.get();
            if label.is_empty() {
                tr("Run your app on a selected device to follow the build.").into()
            } else {
                label
            }
        }
    })
    .style(|s| {
        s.color(muted())
            .font_size(12)
            .width_full()
            .text_overflow(TextOverflow::Wrap)
    });
    let header = Stack::vertical((
        label(title).style(|s| s.color(ink()).font_size(25).ui_bold()),
        context,
    ))
    .style(|s| s.width_full().gap(4));
    let log_action =
        action_button(Glyph::Refresh, "Refresh", move || wb.refresh_logs()).style(
            move |s| {
                s.apply_if(!is_logs, |s| s.hide())
                    .set_disabled(wb.target.with(Option::is_none))
            },
        );
    let build_action =
        action_button(Glyph::Stop, "Stop build", move || wb.stop_app()).style(
            move |s| s.apply_if(is_logs || !wb.launching.get(), |s| s.hide()),
        );
    let bar = Stack::horizontal((
        icon(Glyph::Terminal, 15., faint),
        label("Console").style(|s| s.color(muted()).font_size(12)),
        Empty::new().style(|s| s.flex_grow(1_f32)),
        log_action,
        build_action,
    ))
    .style(|s| {
        s.width_full()
            .items_center()
            .gap(7)
            .height(41)
            .padding_horiz(12)
            .background(surface())
            .border_bottom(1)
            .border_color(subtle_line())
    });
    let console = Label::derived(move || {
        if is_logs {
            wb.logs.with(|output| output.display())
        } else {
            let output = wb.output.with(|output| output.display());
            if output.is_empty() {
                tr("Waiting for build…").into()
            } else {
                output
            }
        }
    })
    .style(|s| {
        s.font_family(MONO.to_owned())
            .color(muted())
            .font_size(12)
            .width_full()
            .text_overflow(TextOverflow::Wrap)
            .padding(12)
    });
    let output = Stack::vertical((
        bar,
        Scroll::new(console)
            .style(|s| s.width_full().min_height(0).flex_grow(1_f32)),
    ))
    .style(|s| {
        s.width_full()
            .min_height(0)
            .flex_grow(1_f32)
            .background(rail())
            .border(1)
            .border_color(line())
            .border_radius(7)
    });
    Stack::vertical((header, output))
        .style(|s| s.size_full().min_size(0, 0).padding(20).gap(16))
}

#[derive(Clone, Copy, PartialEq)]
enum SidePanel {
    Devices,
    Web,
    Files,
}

#[derive(Clone, Copy)]
struct WorkspaceFocus {
    composer: RwSignal<Option<floem::ViewId>>,
    files_button: RwSignal<Option<floem::ViewId>>,
}

fn toolbar_toggle_style(
    s: floem::style::Style,
    selected: bool,
) -> floem::style::Style {
    s.height(27)
        .padding_horiz(6)
        .border(1)
        .border_radius(5)
        .color(if selected { accent() } else { muted() })
        .border_color(if selected { line() } else { subtle_line() })
        .background(if selected { mint_wash() } else { canvas() })
        .hover(|s| s.background(raised()))
        .focus_visible(|s| s.outline(2).outline_color(accent()))
}

fn workspace(
    wb: Workbench,
    side_panel: RwSignal<Option<SidePanel>>,
    terminals: workbench_terminal::Terminals,
    focus: WorkspaceFocus,
) -> impl IntoView {
    let tight = RwSignal::new(false);
    let dock = terminal_dock::Dock::new();
    let hide_terminal = move || {
        dock.visible.set(false);
        if wb.tab.get_untracked() == Tab::Chat
            && let Some(input) = focus.composer.get_untracked()
        {
            input.request_focus();
        }
    };
    let tab = move |name, glyph, t| {
        let contents = Stack::horizontal((
            icon(glyph, 14., muted),
            label(name).style(move |s| {
                s.apply_if(tight.get() && wb.tab.get() != t, |s| s.hide())
            }),
        ))
        .style(|s| s.items_center().gap(6));
        Button::new(contents)
            .action(move || match t {
                Tab::Changes => wb.show_diff(),
                Tab::Logs if wb.tab.get_untracked() == Tab::Logs => {
                    wb.refresh_logs()
                }
                _ => wb.tab.set(t),
            })
            .style(move |s| {
                let active = wb.tab.get() == t;
                s.padding_horiz(8)
                    .height(45)
                    .min_width(0)
                    .font_size(12)
                    .color(if active { ink() } else { muted() })
                    .background(canvas())
                    .border(0)
                    .border_radius(0)
                    .border_bottom(2)
                    .border_color(if active { accent() } else { canvas() })
                    .apply_if(active, |s| s.ui_bold())
                    .hover(|s| s.background(panel()))
                    .focus_visible(|s| s.outline(2).outline_color(accent()))
            })
            .tooltip(move || tr(name))
    };
    let panel_toggle = move |mode: SidePanel, glyph, name: &'static str| {
        let content = Stack::horizontal((
            icon(glyph, 15., move || {
                if side_panel.get() == Some(mode) {
                    accent()
                } else {
                    muted()
                }
            }),
            label(name)
                .style(move |s| s.font_size(11).apply_if(tight.get(), |s| s.hide())),
        ))
        .style(|s| s.items_center().gap(4));
        let toggle = Button::new(content)
            .action(move || {
                side_panel.update(|v| {
                    *v = if *v == Some(mode) { None } else { Some(mode) }
                })
            })
            .style(move |s| toolbar_toggle_style(s, side_panel.get() == Some(mode)));
        if mode == SidePanel::Files {
            focus.files_button.set(Some(toggle.id()));
        }
        toggle.tooltip(move || {
            tr(match (mode, side_panel.get() == Some(mode)) {
                (SidePanel::Devices, false) => "Show mobile preview",
                (SidePanel::Devices, true) => "Hide mobile preview",
                (SidePanel::Web, false) => "Show web preview",
                (SidePanel::Web, true) => "Hide web preview",
                (SidePanel::Files, false) => "Show project files",
                (SidePanel::Files, true) => "Hide project files",
            })
        })
    };
    let terminal_toggle = Button::new(
        Stack::horizontal((
            icon(Glyph::Terminal, 15., move || {
                if dock.visible.get() {
                    accent()
                } else {
                    muted()
                }
            }),
            label("Terminal")
                .style(move |s| s.font_size(11).apply_if(tight.get(), |s| s.hide())),
        ))
        .style(|s| s.items_center().gap(4)),
    )
    .action(move || {
        if dock.visible.get_untracked() {
            hide_terminal();
        } else {
            dock.visible.set(true);
        }
    })
    .style(move |s| toolbar_toggle_style(s, dock.visible.get()))
    .tooltip(move || {
        if dock.visible.get() {
            tr("Hide terminal")
        } else {
            tr("Show terminal")
        }
    });
    let tabs = Stack::horizontal((
        tab("Conversation", Glyph::Chat, Tab::Chat),
        tab("Changes", Glyph::Diff, Tab::Changes),
        tab("Build", Glyph::Build, Tab::Output),
        tab("Logs", Glyph::Logs, Tab::Logs),
        Empty::new().style(|s| s.flex_grow(1_f32)),
        terminal_toggle,
        panel_toggle(SidePanel::Files, Glyph::Folder, "Files"),
        panel_toggle(SidePanel::Devices, Glyph::Device, "Mobile"),
        panel_toggle(SidePanel::Web, Glyph::Globe, "Web"),
    ))
    .style(|s| {
        s.width_full()
            .items_center()
            .padding_horiz(10)
            .gap(2)
            .height(46)
            .flex_shrink(0_f32)
            .border_bottom(1)
            .border_color(subtle_line())
    });
    let body = dyn_container(
        move || wb.tab.get(),
        move |t| match t {
            Tab::Chat => chat(wb, focus.composer).into_any(),
            Tab::Changes => changes_view(wb).into_any(),
            Tab::Output | Tab::Logs => output_view(wb, t).into_any(),
        },
    )
    .style(|s| s.width_full().flex_grow(1_f32).min_size(0, 0));
    let status = Stack::horizontal((
        Empty::new().style(move |s| {
            s.size(6, 6).border_radius(3).background(
                if wb.approvals.with(|a| !a.is_empty()) {
                    peach()
                } else if wb.busy.get() {
                    accent()
                } else {
                    faint()
                },
            )
        }),
        Label::derived(move || wb.status.display()).style(|s| {
            s.min_width(0)
                .width_full()
                .text_ellipsis()
                .color(muted())
                .font_size(11)
        }),
    ))
    .style(|s| {
        s.width_full()
            .height(27)
            .gap(7)
            .items_center()
            .padding_horiz(16)
            .border_top(1)
            .border_color(subtle_line())
    });
    Stack::vertical((tabs, body, dock.view(wb, terminals, hide_terminal), status))
        .on_resize(move |rect| {
            dock.window_resized(rect.height());
            let cramped = rect.width() < 690.;
            if tight.get_untracked() != cramped {
                tight.set(cramped);
            }
        })
        .style(|s| {
            s.height_full()
                .min_width(0)
                .flex_grow(1_f32)
                .flex_basis(0)
                .background(canvas())
        })
}

fn inspector(wb: Workbench, narrow: RwSignal<bool>) -> impl IntoView {
    let actions = dyn_container(
        move || (wb.launching.get(), wb.current.get() == wb.build_owner.get()),
        move |(running, owned)| {
            if running && !owned {
                action_button(Glyph::Terminal, "View active build", move || {
                    wb.tab.set(Tab::Output)
                })
                .into_any()
            } else if running {
                action_button(Glyph::Stop, "Stop build", move || wb.stop_app())
                    .into_any()
            } else {
                let content = Stack::horizontal((
                    icon(Glyph::Play, 14., accent),
                    label("Run app"),
                ))
                .style(|s| s.items_center().gap(7));
                Button::new(content)
                    .action(move || wb.launch())
                    .style(move |s| {
                        s.width_full()
                            .height(34)
                            .font_size(12)
                            .ui_bold()
                            .border_radius(7)
                            .background(mint_wash())
                            .color(accent())
                            .border(1)
                            .border_color(line())
                            .hover(|s| s.background(raised()))
                            .focus_visible(|s| s.outline(2).outline_color(accent()))
                            .set_disabled(
                                wb.target.with(Option::is_none)
                                    || wb.root.with(Option::is_none),
                            )
                            .disabled(|s| {
                                s.background(surface())
                                    .color(faint())
                                    .border_color(subtle_line())
                            })
                    })
                    .into_any()
            }
        },
    )
    .style(|s| {
        s.width_full()
            .padding_horiz(15)
            .padding_vert(11)
            .border_top(1)
            .border_color(subtle_line())
    });
    Stack::vertical((
        devices::device_panel(wb.target)
            .style(|s| s.width_full().min_height(0).height(0).flex_grow(1_f32)),
        actions,
    ))
    .style(move |s| {
        s.width(if narrow.get() { 292 } else { 324 })
            .height_full()
            .flex_shrink(0_f32)
            .background(panel())
            .border_left(1)
            .border_color(subtle_line())
    })
}

fn app(
    initial: Option<PathBuf>,
    preference: appearance::Preference,
) -> impl IntoView {
    let appearance = appearance::Appearance::install(preference);
    i18n::install();
    let wb = Workbench::new();
    if let Some(project) = initial.or_else(projects::last_project) {
        wb.open(&project);
    }
    let narrow = RwSignal::new(false);
    let window_width = RwSignal::new(1280.);
    let rail_override = RwSignal::new(None::<bool>);
    let side_panel = RwSignal::new(Some(SidePanel::Devices));
    let setup_open = RwSignal::new(provider_setup::first_run());
    let setup = provider_setup::ProviderSetup::new();
    let web = web_preview::WebPreview::new();
    let terminals = workbench_terminal::Terminals::new();
    let files = file_editor::FileEditor::new();
    let selected_workspace = wb.workspace_selection();
    floem::reactive::Effect::new(move |_| {
        let workspace = selected_workspace.get().ok();
        let file = workspace.as_ref().and_then(|w| files.selected(w));
        let dirty = workspace
            .as_ref()
            .zip(file.as_ref())
            .is_some_and(|(w, f)| files.is_dirty(w, f));
        let preview = workspace.as_ref().and_then(|w| web.ready_url(w));
        let panel = side_panel.get().map(|mode| match mode {
            SidePanel::Devices => "mobile",
            SidePanel::Web => "web",
            SidePanel::Files => "files",
        });
        wb.view_context.set(verify::ViewContext {
            file,
            dirty,
            preview,
            panel,
        });
    });
    floem::reactive::Effect::new(move |_| match wb.preview_request.get() {
        Some(verify::Preview::Mobile(platform, id)) => {
            wb.target.set(Some((platform, id)));
            side_panel.set(Some(SidePanel::Devices));
        }
        Some(verify::Preview::Web) => side_panel.set(Some(SidePanel::Web)),
        None => {}
    });
    let focus = WorkspaceFocus {
        composer: RwSignal::new(None),
        files_button: RwSignal::new(None),
    };
    let leave_editor = move || {
        let target =
            if wb.tab.get_untracked() == Tab::Chat && !wb.busy.get_untracked() {
                focus.composer.get_untracked()
            } else {
                focus.files_button.get_untracked()
            };
        if let Some(target) = target {
            target.request_focus();
        }
    };
    let page = dyn_container(
        move || setup_open.get(),
        move |first_run| {
            if first_run {
                for provider in [
                    umide_workbench_agents::Provider::Codex,
                    umide_workbench_agents::Provider::Claude,
                ] {
                    setup.refresh(provider);
                }
                return onboarding::panel(setup, move || {
                    if let Err(error) = provider_setup::remember() {
                        wb.status.set(error);
                    }
                    setup_open.set(false);
                })
                .into_any();
            }
            let rail = dyn_container(
                move || rail_override.get().unwrap_or_else(|| narrow.get()),
                move |compact| {
                    navigation::sidebar(
                        wb,
                        compact,
                        move || rail_override.set(Some(!compact)),
                        move || setup_open.set(true),
                    )
                },
            )
            .style(|s| s.height_full().flex_shrink(0_f32));
            // Keep the native editor, caret, scroll and expanded tree alive when
            // visiting a preview. Defer filesystem access until Files is opened.
            let files_visited = Memo::new(move |previous| {
                previous == Some(&true) || side_panel.get() == Some(SidePanel::Files)
            });
            let files_view = dyn_container(
                move || files_visited.get(),
                move |visited| {
                    if visited {
                        files_panel::panel(wb, files, leave_editor)
                            .style(|s| s.size_full())
                            .into_any()
                    } else {
                        Empty::new().into_any()
                    }
                },
            )
            .style(move |s| {
                let width: f64 = window_width.get();
                let compact = rail_override.get().unwrap_or_else(|| narrow.get());
                let rail = if compact { 62. } else { 220. };
                let available = width - rail - 400.;
                s.width((width * 0.43).min(available).clamp(316., 780.))
                    .height_full()
                    .min_height(0)
                    .flex_shrink(0_f32)
                    .background(panel())
                    .border_left(1)
                    .border_color(subtle_line())
                    .apply_if(side_panel.get() != Some(SidePanel::Files), |s| {
                        s.hide()
                    })
            });
            let preview = dyn_container(
                move || side_panel.get(),
                move |mode| match mode {
                    Some(SidePanel::Devices) => inspector(wb, narrow).into_any(),
                    Some(SidePanel::Web) => web_preview::panel(wb, web)
                        .style(move |s| {
                            s.width(if narrow.get() { 378 } else { 490 })
                                .height_full()
                                .min_height(0)
                                .flex_shrink(0_f32)
                                .background(panel())
                                .border_left(1)
                                .border_color(subtle_line())
                        })
                        .into_any(),
                    None | Some(SidePanel::Files) => Empty::new().into_any(),
                },
            )
            .style(|s| s.height_full().flex_shrink(0_f32));
            Stack::horizontal((
                rail,
                workspace(wb, side_panel, terminals, focus),
                files_view,
                preview,
            ))
            .style(|s| s.size_full())
            .into_any()
        },
    );
    page.style(|s| {
        s.size_full()
            .background(canvas())
            .color(ink())
            .font_family(SANS.to_owned())
            .font_size(14)
    })
    .on_resize(move |rect| {
        if rect.width() != window_width.get_untracked() {
            window_width.set(rect.width());
        }
        let small = rect.width() < 1080.;
        if narrow.get_untracked() != small {
            narrow.set(small);
        }
    })
    .on_event_cont(floem::event::EventListener::ThemeChanged, move |event| {
        if let floem::event::Event::ThemeChanged(theme) = event {
            appearance.window_changed(*theme);
        }
    })
    .on_event_cont(floem::event::EventListener::WindowGotFocus, move |_| {
        if let Ok(workspace) = wb.selected_workspace() {
            files.refresh_external(&workspace);
        }
    })
    .on_event_cont(floem::event::EventListener::WindowClosed, move |_| {
        if let Err(error) = files.close() {
            eprintln!("Could not store unsaved editor drafts: {error}");
        }
        setup.close();
        web.close();
        terminals.close();
        wb.close();
    })
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new("--umide-verify")) {
        let result = match (args.next(), args.next(), args.next()) {
            (Some(root), Some(session), None) => {
                verify::relay(root.as_ref(), session.as_ref())
            }
            _ => Err(std::io::Error::other(
                "Expected checkout and session directory.",
            )),
        };
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    let project = first.map(PathBuf::from);
    typography::configure();
    let preference = appearance::Preference::load();
    let config = WindowConfig::default()
        .title("Umide Workbench")
        .size((1280., 800.))
        .min_size((960., 600.));
    let config = match preference.native() {
        Some(theme) => config.theme_override(theme),
        None => config,
    };
    Application::new()
        .window(move |_| app(project, preference), Some(config))
        .run();
}
