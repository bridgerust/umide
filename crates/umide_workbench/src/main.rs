mod appearance;
#[cfg(target_os = "macos")]
mod browser;
mod conversation;
mod design;
mod devices;
mod navigation;
mod onboarding;
mod projects;
mod provider_setup;
mod review;
mod state;
mod terminal_dock;
mod terminal_session;
mod typography;
mod web_preview;
mod web_server;
mod workbench_terminal;
mod worktree;

use std::path::PathBuf;

use conversation::chat;
use design::{button, canvas};
use floem::{
    Application, FileDialogOptions, prelude::*, style::TextOverflow,
    window::WindowConfig,
};
use review::changes_view;
use state::{Tab, Workbench};

use design::*;

fn choose_project(wb: Workbench) {
    floem::open_file(FileDialogOptions::new().select_directories(), move |info| {
        if let Some(path) = info.and_then(|x| x.path.into_iter().next()) {
            wb.open(&path);
        }
    });
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
                        "Select a device to inspect its recent activity.".into()
                    })
            })
        } else {
            let label = wb.build_label.get();
            if label.is_empty() {
                "Run your app on a selected device to follow the build.".into()
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
        title.style(|s| s.color(ink()).font_size(25).ui_bold()),
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
        "Console".style(|s| s.color(muted()).font_size(12)),
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
            wb.logs.get()
        } else {
            let output = wb.output.get();
            if output.is_empty() {
                "Waiting for build…".into()
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
enum PreviewMode {
    Devices,
    Web,
}

fn preview_toggle_style(
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
    inspection: RwSignal<Option<PreviewMode>>,
    terminals: workbench_terminal::Terminals,
) -> impl IntoView {
    let tight = RwSignal::new(false);
    let dock = terminal_dock::Dock::new();
    let composer_focus: RwSignal<Option<floem::ViewId>> = RwSignal::new(None);
    let hide_terminal = move || {
        dock.visible.set(false);
        if wb.tab.get_untracked() == Tab::Chat
            && let Some(input) = composer_focus.get_untracked()
        {
            input.request_focus();
        }
    };
    let tab = move |name, glyph, t| {
        let contents = Stack::horizontal((
            icon(glyph, 14., muted),
            Label::new(name).style(move |s| {
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
            .tooltip(move || name)
    };
    let preview_toggle = move |mode: PreviewMode, glyph, name: &'static str| {
        let content = Stack::horizontal((
            icon(glyph, 15., move || {
                if inspection.get() == Some(mode) {
                    accent()
                } else {
                    muted()
                }
            }),
            name.style(move |s| s.font_size(11).apply_if(tight.get(), |s| s.hide())),
        ))
        .style(|s| s.items_center().gap(4));
        Button::new(content)
            .action(move || {
                inspection.update(|v| {
                    *v = if *v == Some(mode) { None } else { Some(mode) }
                })
            })
            .style(move |s| preview_toggle_style(s, inspection.get() == Some(mode)))
            .tooltip(move || {
                format!(
                    "{} {name} preview",
                    if inspection.get() == Some(mode) {
                        "Hide"
                    } else {
                        "Show"
                    }
                )
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
            "Terminal"
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
    .style(move |s| preview_toggle_style(s, dock.visible.get()))
    .tooltip(move || {
        if dock.visible.get() {
            "Hide terminal"
        } else {
            "Show terminal"
        }
    });
    let tabs = Stack::horizontal((
        tab("Conversation", Glyph::Chat, Tab::Chat),
        tab("Changes", Glyph::Diff, Tab::Changes),
        tab("Build", Glyph::Build, Tab::Output),
        tab("Logs", Glyph::Logs, Tab::Logs),
        Empty::new().style(|s| s.flex_grow(1_f32)),
        terminal_toggle,
        preview_toggle(PreviewMode::Devices, Glyph::Device, "Mobile"),
        preview_toggle(PreviewMode::Web, Glyph::Globe, "Web"),
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
            Tab::Chat => chat(wb, composer_focus).into_any(),
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
        Label::derived(move || wb.status.get()).style(|s| {
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
            let cramped = rect.width() < 560.;
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
                let content =
                    Stack::horizontal((icon(Glyph::Play, 14., accent), "Run app"))
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
    let wb = Workbench::new();
    if let Some(project) = initial.or_else(projects::last_project) {
        wb.open(&project);
    }
    let narrow = RwSignal::new(false);
    let rail_override = RwSignal::new(None::<bool>);
    let inspection = RwSignal::new(Some(PreviewMode::Devices));
    let setup_open = RwSignal::new(provider_setup::first_run());
    let setup = provider_setup::ProviderSetup::new();
    let web = web_preview::WebPreview::new();
    let terminals = workbench_terminal::Terminals::new();
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
            let preview = dyn_container(
                move || inspection.get(),
                move |mode| match mode {
                    Some(PreviewMode::Devices) => inspector(wb, narrow).into_any(),
                    Some(PreviewMode::Web) => web_preview::panel(wb, web)
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
                    None => Empty::new().into_any(),
                },
            )
            .style(|s| s.height_full().flex_shrink(0_f32));
            Stack::horizontal((rail, workspace(wb, inspection, terminals), preview))
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
    .on_event_cont(floem::event::EventListener::WindowClosed, move |_| {
        setup.close();
        web.close();
        terminals.close();
        wb.close();
    })
}

fn main() {
    let project = std::env::args_os().nth(1).map(PathBuf::from);
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
