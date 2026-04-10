use std::{rc::Rc, sync::Arc};

use floem::{
    AnyView, IntoView, View, ViewId,
    action::{add_overlay, remove_overlay},
    event::EventListener,
    menu::Menu,
    peniko::{Color, kurbo::Rect},
    reactive::{
        Memo, ReadSignal, RwSignal, Scope, SignalGet, SignalUpdate, SignalWith,
    },
    style::{AlignItems, CursorStyle, JustifyContent},
    views::{Container, Decorators, Empty, Label, Stack, drag_window_area, svg},
};
use umide_core::meta;
use umide_rpc::proxy::ProxyStatus;

use crate::{
    app::{clickable_icon, not_clickable_icon, tooltip_label, window_menu},
    command::{UmideCommand, UmideWorkbenchCommand, WindowCommand},
    config::{UmideConfig, color::UmideColor, icon::UmideIcons},
    listener::Listener,
    main_split::MainSplitData,
    update::ReleaseInfo,
    window_tab::WindowTabData,
    workspace::UmideWorkspace,
};

fn left(
    workspace: Arc<UmideWorkspace>,
    lapce_command: Listener<UmideCommand>,
    workbench_command: Listener<UmideWorkbenchCommand>,
    config: ReadSignal<Arc<UmideConfig>>,
    proxy_status: RwSignal<Option<ProxyStatus>>,
    num_window_tabs: Memo<usize>,
) -> impl View {
    let is_local = workspace.kind.is_local();
    let is_macos = cfg!(target_os = "macos");

    // Build the remote button base view.
    let remote_btn = tooltip_label(
        config,
        Container::new(svg(move || config.get().ui_svg(UmideIcons::REMOTE)).style(
            move |s| {
                let config = config.get();
                let size = (config.ui.icon_size() as f32 + 2.0).min(30.0);
                s.size(size, size).color(if is_local {
                    config.color(UmideColor::LAPCE_ICON_ACTIVE)
                } else {
                    match proxy_status.get() {
                        Some(_) => Color::WHITE,
                        None => config.color(UmideColor::LAPCE_ICON_ACTIVE),
                    }
                })
            },
        )),
        || "Connect to Remote",
    );

    // On macOS popout_menu uses NSMenu.popUpMenuPositioningItem which runs a nested
    // AppKit event loop that conflicts with winit's event loop → clean crash.
    // Use a floem overlay instead.
    #[cfg(target_os = "macos")]
    let remote_btn = {
        let cx = Scope::current();
        let icon_rect = cx.create_rw_signal(Rect::ZERO);
        let overlay_id = cx.create_rw_signal(None::<ViewId>);
        remote_btn
            .on_resize(move |rect| { icon_rect.set(rect); })
            .on_click_stop(move |_| {
                if let Some(id) = overlay_id.get_untracked() {
                    remove_overlay(id);
                    overlay_id.set(None);
                    return;
                }
                let rect = icon_rect.get_untracked();
                let show_disconnect = !is_local
                    && proxy_status.get_untracked().is_some_and(|p| {
                        matches!(p, ProxyStatus::Connecting | ProxyStatus::Connected)
                    });
                let vid = add_overlay(
                    Stack::new((
                        Label::new("Connect to SSH Host".to_string())
                            .on_click_stop(move |_| {
                                if let Some(id) = overlay_id.get_untracked() {
                                    remove_overlay(id);
                                    overlay_id.set(None);
                                }
                                workbench_command.send(UmideWorkbenchCommand::ConnectSshHost);
                            })
                            .style(move |s| {
                                let config = config.get();
                                s.padding_horiz(12.0)
                                    .padding_vert(6.0)
                                    .color(config.color(UmideColor::EDITOR_FOREGROUND))
                                    .cursor(CursorStyle::Pointer)
                                    .hover(|s| s.background(
                                        config.color(UmideColor::PANEL_HOVERED_BACKGROUND),
                                    ))
                            }),
                        Label::new("Disconnect remote".to_string())
                            .on_click_stop(move |_| {
                                if let Some(id) = overlay_id.get_untracked() {
                                    remove_overlay(id);
                                    overlay_id.set(None);
                                }
                                workbench_command.send(UmideWorkbenchCommand::DisconnectRemote);
                            })
                            .style(move |s| {
                                let config = config.get();
                                s.padding_horiz(12.0)
                                    .padding_vert(6.0)
                                    .color(config.color(UmideColor::EDITOR_FOREGROUND))
                                    .cursor(CursorStyle::Pointer)
                                    .apply_if(!show_disconnect, |s| s.hide())
                                    .hover(|s| s.background(
                                        config.color(UmideColor::PANEL_HOVERED_BACKGROUND),
                                    ))
                            }),
                    ))
                    .on_event_stop(EventListener::FocusLost, move |_| {
                        if let Some(id) = overlay_id.get_untracked() {
                            remove_overlay(id);
                            overlay_id.set(None);
                        }
                    })
                    .style(move |s| {
                        let config = config.get();
                        s.flex_col()
                            .inset_top(rect.y1)
                            .inset_left(rect.x0)
                            .background(config.color(UmideColor::PANEL_BACKGROUND))
                            .border(1.0)
                            .border_color(config.color(UmideColor::LAPCE_BORDER))
                            .border_radius(6.0)
                            .min_width(180.0)
                            .focusable(true)
                    }),
                );
                overlay_id.set(Some(vid));
                vid.request_focus();
            })
    };
    #[cfg(not(target_os = "macos"))]
    let remote_btn = remote_btn.popout_menu(move || {
        #[allow(unused_mut)]
        let mut menu = Menu::new()
            .item("Connect to SSH Host", |i| i.action(move || {
                workbench_command.send(UmideWorkbenchCommand::ConnectSshHost);
            }));
        if !is_local
            && proxy_status.get().is_some_and(|p| {
                matches!(p, ProxyStatus::Connecting | ProxyStatus::Connected)
            })
        {
            menu = menu.item("Disconnect remote", |i| i.action(
                move || {
                    workbench_command.send(UmideWorkbenchCommand::DisconnectRemote);
                },
            ));
        }
        #[cfg(windows)]
        {
            menu = menu.item("Connect to WSL Host", |i| i.action(
                move || {
                    workbench_command.send(UmideWorkbenchCommand::ConnectWslHost);
                },
            ));
        }
        menu
    });
    let remote_btn = remote_btn.style(move |s| {
        let config = config.get();
        let color = if is_local {
            Color::TRANSPARENT
        } else {
            match proxy_status.get() {
                Some(ProxyStatus::Connected) => {
                    config.color(UmideColor::LAPCE_REMOTE_CONNECTED)
                }
                Some(ProxyStatus::Connecting) => {
                    config.color(UmideColor::LAPCE_REMOTE_CONNECTING)
                }
                Some(ProxyStatus::Disconnected) => {
                    config.color(UmideColor::LAPCE_REMOTE_DISCONNECTED)
                }
                None => Color::TRANSPARENT,
            }
        };
        s.height_pct(100.0)
            .padding_horiz(10.0)
            .items_center()
            .background(color)
            .hover(|s| {
                s.cursor(CursorStyle::Pointer).background(
                    config.color(UmideColor::PANEL_HOVERED_BACKGROUND),
                )
            })
            .active(|s| {
                s.cursor(CursorStyle::Pointer).background(
                    config.color(UmideColor::PANEL_HOVERED_ACTIVE_BACKGROUND),
                )
            })
    });

    Stack::new((
        Empty::new().style(move |s| {
            let should_hide = if is_macos {
                num_window_tabs.get() > 1
            } else {
                true
            };
            s.width(75.0).apply_if(should_hide, |s| s.hide())
        }),
        Container::new(svg(move || config.get().ui_svg(UmideIcons::LOGO)).style(
            move |s| {
                let config = config.get();
                s.size(16.0, 16.0)
                    .color(config.color(UmideColor::LAPCE_ICON_ACTIVE))
            },
        ))
        .style(move |s| s.margin_horiz(10.0).apply_if(is_macos, |s| s.hide())),
        not_clickable_icon(
            || UmideIcons::MENU,
            || false,
            || false,
            || "Menu",
            config,
        )
        .popout_menu(move || window_menu(lapce_command, workbench_command))
        .style(move |s| {
            s.margin_left(4.0)
                .margin_right(6.0)
                .apply_if(is_macos, |s| s.hide())
        }),
        remote_btn,
        drag_window_area(Empty::new())
            .style(|s| s.height_pct(100.0).flex_basis(0.0).flex_grow(1.0)),
    ))
    .style(move |s| {
        s.height_pct(100.0)
            .flex_basis(0.0)
            .flex_grow(1.0)
            .items_center()
    })
    .debug_name("Left Side of Top Bar")
}

fn middle(
    workspace: Arc<UmideWorkspace>,
    main_split: MainSplitData,
    workbench_command: Listener<UmideWorkbenchCommand>,
    config: ReadSignal<Arc<UmideConfig>>,
) -> impl View {
    let local_workspace = workspace.clone();
    let can_jump_backward = {
        let main_split = main_split.clone();
        Memo::new(move |_| main_split.can_jump_location_backward(true))
    };
    let can_jump_forward =
        Memo::new(move |_| main_split.can_jump_location_forward(true));

    let jump_backward = move || {
        clickable_icon(
            || UmideIcons::LOCATION_BACKWARD,
            move || {
                workbench_command.send(UmideWorkbenchCommand::JumpLocationBackward);
            },
            || false,
            move || !can_jump_backward.get(),
            || "Jump Backward",
            config,
        )
        .style(move |s| s.margin_horiz(6.0))
    };
    let jump_forward = move || {
        clickable_icon(
            || UmideIcons::LOCATION_FORWARD,
            move || {
                workbench_command.send(UmideWorkbenchCommand::JumpLocationForward);
            },
            || false,
            move || !can_jump_forward.get(),
            || "Jump Forward",
            config,
        )
        .style(move |s| s.margin_right(6.0))
    };

    let open_folder = move || -> AnyView {
        #[cfg(target_os = "macos")]
        {
            // On macOS, popout_menu uses NSMenu.popUpMenuPositioningItem which runs a
            // nested AppKit event loop that conflicts with winit's event loop (clean crash).
            // Use a floem overlay instead.
            let cx = Scope::current();
            let icon_rect = cx.create_rw_signal(Rect::ZERO);
            let overlay_id = cx.create_rw_signal(None::<ViewId>);
            not_clickable_icon(
                || UmideIcons::PALETTE_MENU,
                || false,
                || false,
                || "Open Folder / Recent Workspace",
                config,
            )
            .on_resize(move |rect| { icon_rect.set(rect); })
            .on_click_stop(move |_| {
                if let Some(id) = overlay_id.get_untracked() {
                    remove_overlay(id);
                    overlay_id.set(None);
                    return;
                }
                let rect = icon_rect.get_untracked();
                let vid = add_overlay(
                    Stack::new((
                        Label::new("Open Folder".to_string())
                            .on_click_stop(move |_| {
                                if let Some(id) = overlay_id.get_untracked() {
                                    remove_overlay(id);
                                    overlay_id.set(None);
                                }
                                workbench_command.send(UmideWorkbenchCommand::OpenFolder);
                            })
                            .style(move |s| {
                                let config = config.get();
                                s.padding_horiz(12.0)
                                    .padding_vert(6.0)
                                    .color(config.color(UmideColor::EDITOR_FOREGROUND))
                                    .cursor(CursorStyle::Pointer)
                                    .hover(|s| s.background(
                                        config.color(UmideColor::PANEL_HOVERED_BACKGROUND),
                                    ))
                            }),
                        Label::new("Open Recent Workspace".to_string())
                            .on_click_stop(move |_| {
                                if let Some(id) = overlay_id.get_untracked() {
                                    remove_overlay(id);
                                    overlay_id.set(None);
                                }
                                workbench_command.send(UmideWorkbenchCommand::PaletteWorkspace);
                            })
                            .style(move |s| {
                                let config = config.get();
                                s.padding_horiz(12.0)
                                    .padding_vert(6.0)
                                    .color(config.color(UmideColor::EDITOR_FOREGROUND))
                                    .cursor(CursorStyle::Pointer)
                                    .hover(|s| s.background(
                                        config.color(UmideColor::PANEL_HOVERED_BACKGROUND),
                                    ))
                            }),
                    ))
                    .on_event_stop(EventListener::FocusLost, move |_| {
                        if let Some(id) = overlay_id.get_untracked() {
                            remove_overlay(id);
                            overlay_id.set(None);
                        }
                    })
                    .style(move |s| {
                        let config = config.get();
                        s.flex_col()
                            .inset_top(rect.y1)
                            .inset_left(rect.x0)
                            .background(config.color(UmideColor::PANEL_BACKGROUND))
                            .border(1.0)
                            .border_color(config.color(UmideColor::LAPCE_BORDER))
                            .border_radius(6.0)
                            .min_width(180.0)
                            .focusable(true)
                    }),
                );
                overlay_id.set(Some(vid));
                vid.request_focus();
            })
            .into_any()
        }
        #[cfg(not(target_os = "macos"))]
        {
            not_clickable_icon(
                || UmideIcons::PALETTE_MENU,
                || false,
                || false,
                || "Open Folder / Recent Workspace",
                config,
            )
            .popout_menu(move || {
                Menu::new()
                    .item("Open Folder", |i| i.action(move || {
                        workbench_command.send(UmideWorkbenchCommand::OpenFolder);
                    }))
                    .item("Open Recent Workspace", |i| i.action(move || {
                        workbench_command.send(UmideWorkbenchCommand::PaletteWorkspace);
                    }))
            })
            .into_any()
        }
    };

    Stack::new((
        Stack::new((
            drag_window_area(Empty::new())
                .style(|s| s.height_pct(100.0).flex_basis(0.0).flex_grow(1.0)),
            jump_backward(),
            jump_forward(),
        ))
        .style(|s| {
            s.flex_basis(0)
                .flex_grow(1.0)
                .justify_content(Some(JustifyContent::FlexEnd))
        }),
        Container::new(
            Stack::new((
                svg(move || config.get().ui_svg(UmideIcons::SEARCH)).style(
                    move |s| {
                        let config = config.get();
                        let icon_size = config.ui.icon_size() as f32;
                        s.size(icon_size, icon_size)
                            .color(config.color(UmideColor::LAPCE_ICON_ACTIVE))
                    },
                ),
                Label::new({
                    if let Some(s) = local_workspace.display() {
                        s
                    } else {
                        "Open Folder".to_string()
                    }
                })
                .style(|s| s.padding_left(10).padding_right(5).selectable(false)),
                open_folder(),
            ))
            .style(|s| s.align_items(Some(AlignItems::Center))),
        )
        .on_event_stop(EventListener::PointerDown, |_| {})
        .on_click_stop(move |_| {
            if workspace.clone().path.is_some() {
                workbench_command.send(UmideWorkbenchCommand::PaletteHelpAndFile);
            } else {
                workbench_command.send(UmideWorkbenchCommand::PaletteWorkspace);
            }
        })
        .style(move |s| {
            let config = config.get();
            s.flex_basis(0)
                .flex_grow(10.0)
                .min_width(200.0)
                .max_width(500.0)
                .height(26.0)
                .justify_content(Some(JustifyContent::Center))
                .align_items(Some(AlignItems::Center))
                .border(1.0)
                .border_color(config.color(UmideColor::LAPCE_BORDER))
                .border_radius(6.0)
                .background(config.color(UmideColor::EDITOR_BACKGROUND))
        }),
        Stack::new((
            clickable_icon(
                || UmideIcons::START,
                move || {
                    workbench_command.send(UmideWorkbenchCommand::PaletteRunAndDebug)
                },
                || false,
                || false,
                || "Run and Debug",
                config,
            )
            .style(move |s| s.margin_horiz(6.0)),
            drag_window_area(Empty::new())
                .style(|s| s.height_pct(100.0).flex_basis(0.0).flex_grow(1.0)),
        ))
        .style(move |s| {
            s.flex_basis(0)
                .flex_grow(1.0)
                .justify_content(Some(JustifyContent::FlexStart))
        }),
    ))
    .style(|s| {
        s.flex_basis(0)
            .flex_grow(2.0)
            .align_items(Some(AlignItems::Center))
            .justify_content(Some(JustifyContent::Center))
    })
    .debug_name("Middle of Top Bar")
}

fn right(
    window_command: Listener<WindowCommand>,
    workbench_command: Listener<UmideWorkbenchCommand>,
    latest_release: ReadSignal<Arc<Option<ReleaseInfo>>>,
    update_in_progress: RwSignal<bool>,
    num_window_tabs: Memo<usize>,
    window_maximized: RwSignal<bool>,
    config: ReadSignal<Arc<UmideConfig>>,
) -> impl View {
    let latest_version = Memo::new(move |_| {
        let latest_release = latest_release.get();
        let latest_version =
            latest_release.as_ref().as_ref().map(|r| r.version.clone());
        if latest_version.is_some()
            && latest_version.as_deref() != Some(meta::VERSION)
        {
            latest_version
        } else {
            None
        }
    });

    let has_update = move || latest_version.with(|v| v.is_some());

    // Build settings icon; on macOS use overlay to avoid nested event loop crash.
    let settings_icon = not_clickable_icon(
        || UmideIcons::SETTINGS,
        || false,
        || false,
        || "Settings",
        config,
    );
    #[cfg(target_os = "macos")]
    let settings_icon = {
        let cx = Scope::current();
        let icon_rect = cx.create_rw_signal(Rect::ZERO);
        let overlay_id = cx.create_rw_signal(None::<ViewId>);
        settings_icon
            .on_resize(move |rect| { icon_rect.set(rect); })
            .on_click_stop(move |_| {
                if let Some(id) = overlay_id.get_untracked() {
                    remove_overlay(id);
                    overlay_id.set(None);
                    return;
                }
                let rect = icon_rect.get_untracked();
                // Determine update label/action at open time
                let update_label = if let Some(v) = latest_version.get_untracked() {
                    if update_in_progress.get_untracked() {
                        format!("Update in progress ({v})")
                    } else {
                        format!("Restart to update ({v})")
                    }
                } else {
                    "No update available".to_string()
                };
                let update_clickable = latest_version.get_untracked().is_some()
                    && !update_in_progress.get_untracked();
                let divider = move || Empty::new().style(move |s| {
                    let config = config.get();
                    s.height(1.0).width_pct(100.0).margin_vert(2.0)
                        .background(config.color(UmideColor::LAPCE_BORDER))
                });
                let item = move |text: &'static str, action: Box<dyn Fn()>| {
                    Label::new(text.to_string())
                        .on_click_stop(move |_| {
                            if let Some(id) = overlay_id.get_untracked() {
                                remove_overlay(id);
                                overlay_id.set(None);
                            }
                            action();
                        })
                        .style(move |s| {
                            let config = config.get();
                            s.padding_horiz(12.0).padding_vert(6.0)
                                .color(config.color(UmideColor::EDITOR_FOREGROUND))
                                .cursor(CursorStyle::Pointer)
                                .hover(|s| s.background(
                                    config.color(UmideColor::PANEL_HOVERED_BACKGROUND),
                                ))
                        })
                };
                let vid = add_overlay(
                    Stack::new((
                        item("Command Palette", Box::new(move || {
                            workbench_command.send(UmideWorkbenchCommand::PaletteCommand);
                        })),
                        divider(),
                        item("Open Settings", Box::new(move || {
                            workbench_command.send(UmideWorkbenchCommand::OpenSettings);
                        })),
                        item("Open Keyboard Shortcuts", Box::new(move || {
                            workbench_command.send(UmideWorkbenchCommand::OpenKeyboardShortcuts);
                        })),
                        item("Open Theme Color Settings", Box::new(move || {
                            workbench_command.send(UmideWorkbenchCommand::OpenThemeColorSettings);
                        })),
                        divider(),
                        Label::new(update_label)
                            .on_click_stop(move |_| {
                                if !update_clickable { return; }
                                if let Some(id) = overlay_id.get_untracked() {
                                    remove_overlay(id);
                                    overlay_id.set(None);
                                }
                                workbench_command.send(UmideWorkbenchCommand::RestartToUpdate);
                            })
                            .style(move |s| {
                                let config = config.get();
                                s.padding_horiz(12.0).padding_vert(6.0)
                                    .color(if update_clickable {
                                        config.color(UmideColor::EDITOR_FOREGROUND)
                                    } else {
                                        config.color(UmideColor::EDITOR_DIM)
                                    })
                                    .apply_if(update_clickable, |s| {
                                        s.cursor(CursorStyle::Pointer).hover(|s| s.background(
                                            config.color(UmideColor::PANEL_HOVERED_BACKGROUND),
                                        ))
                                    })
                            }),
                        divider(),
                        item("About UMIDE", Box::new(move || {
                            workbench_command.send(UmideWorkbenchCommand::ShowAbout);
                        })),
                    ))
                    .on_event_stop(EventListener::FocusLost, move |_| {
                        if let Some(id) = overlay_id.get_untracked() {
                            remove_overlay(id);
                            overlay_id.set(None);
                        }
                    })
                    .style(move |s| {
                        let config = config.get();
                        // Right-align menu to the icon's right edge
                        let menu_width = 220.0_f64;
                        let left = (rect.x1 - menu_width).max(0.0);
                        s.flex_col()
                            .inset_top(rect.y1)
                            .inset_left(left)
                            .background(config.color(UmideColor::PANEL_BACKGROUND))
                            .border(1.0)
                            .border_color(config.color(UmideColor::LAPCE_BORDER))
                            .border_radius(6.0)
                            .min_width(menu_width)
                            .focusable(true)
                    }),
                );
                overlay_id.set(Some(vid));
                vid.request_focus();
            })
    };
    #[cfg(not(target_os = "macos"))]
    let settings_icon = settings_icon.popout_menu(move || {
        Menu::new()
            .item("Command Palette", |i| i.action(move || {
                workbench_command.send(UmideWorkbenchCommand::PaletteCommand)
            }))
            .separator()
            .item("Open Settings", |i| i.action(move || {
                workbench_command.send(UmideWorkbenchCommand::OpenSettings)
            }))
            .item("Open Keyboard Shortcuts", |i| i.action(
                move || {
                    workbench_command
                        .send(UmideWorkbenchCommand::OpenKeyboardShortcuts)
                },
            ))
            .item("Open Theme Color Settings", |i| i.action(
                move || {
                    workbench_command
                        .send(UmideWorkbenchCommand::OpenThemeColorSettings)
                },
            ))
            .separator()
            .item(
                if let Some(v) = latest_version.get_untracked() {
                    if update_in_progress.get_untracked() {
                        format!("Update in progress ({v})")
                    } else {
                        format!("Restart to update ({v})")
                    }
                } else {
                    "No update available".to_string()
                },
                |i| {
                    if latest_version.get_untracked().is_some() {
                        if update_in_progress.get_untracked() {
                            i.enabled(false)
                        } else {
                            i.action(move || {
                                workbench_command
                                    .send(UmideWorkbenchCommand::RestartToUpdate)
                            })
                        }
                    } else {
                        i.enabled(false)
                    }
                }
            )
            .separator()
            .item("About UMIDE", |i| i.action(move || {
                workbench_command.send(UmideWorkbenchCommand::ShowAbout)
            }))
    });

    Stack::new((
        drag_window_area(Empty::new())
            .style(|s| s.height_pct(100.0).flex_basis(0.0).flex_grow(1.0)),
        Stack::new((
            settings_icon,
            Container::new(Label::new("1".to_string()).style(move |s| {
                let config = config.get();
                s.font_size(10.0)
                    .color(config.color(UmideColor::EDITOR_BACKGROUND))
                    .border_radius(100.0)
                    .margin_left(5.0)
                    .margin_top(10.0)
                    .background(config.color(UmideColor::EDITOR_CARET))
            }))
            .style(move |s| {
                let has_update = has_update();
                s.absolute()
                    .size_pct(100.0, 100.0)
                    .justify_end()
                    .items_end()
                    .pointer_events_none()
                    .apply_if(!has_update, |s| s.hide())
            }),
        ))
        .style(move |s| s.margin_horiz(6.0)),
        window_controls_view(
            window_command,
            true,
            num_window_tabs,
            window_maximized,
            config,
        ),
    ))
    .style(|s| {
        s.flex_basis(0)
            .flex_grow(1.0)
            .justify_content(Some(JustifyContent::FlexEnd))
    })
    .debug_name("Right of top bar")
}

pub fn title(window_tab_data: Rc<WindowTabData>) -> impl View {
    let workspace = window_tab_data.workspace.clone();
    let lapce_command = window_tab_data.common.lapce_command;
    let workbench_command = window_tab_data.common.workbench_command;
    let window_command = window_tab_data.common.window_common.window_command;
    let latest_release = window_tab_data.common.window_common.latest_release;
    let proxy_status = window_tab_data.common.proxy_status;
    let num_window_tabs = window_tab_data.common.window_common.num_window_tabs;
    let window_maximized = window_tab_data.common.window_common.window_maximized;
    let title_height = window_tab_data.title_height;
    let update_in_progress = window_tab_data.update_in_progress;
    let config = window_tab_data.common.config;
    Stack::new((
        left(
            workspace.clone(),
            lapce_command,
            workbench_command,
            config,
            proxy_status,
            num_window_tabs,
        ),
        middle(
            workspace,
            window_tab_data.main_split.clone(),
            workbench_command,
            config,
        ),
        right(
            window_command,
            workbench_command,
            latest_release,
            update_in_progress,
            num_window_tabs,
            window_maximized,
            config,
        ),
    ))
    .on_resize(move |rect| {
        let height = rect.height();
        if height != title_height.get_untracked() {
            title_height.set(height);
        }
    })
    .style(move |s| {
        let config = config.get();
        s.width_pct(100.0)
            .height(37.0)
            .items_center()
            .background(config.color(UmideColor::PANEL_BACKGROUND))
            .border_bottom(1.0)
            .border_color(config.color(UmideColor::LAPCE_BORDER))
    })
    .debug_name("Title / Top Bar")
}

pub fn window_controls_view(
    window_command: Listener<WindowCommand>,
    is_title: bool,
    num_window_tabs: Memo<usize>,
    window_maximized: RwSignal<bool>,
    config: ReadSignal<Arc<UmideConfig>>,
) -> impl View {
    Stack::new((
        clickable_icon(
            || UmideIcons::WINDOW_MINIMIZE,
            || {
                floem::action::minimize_window();
            },
            || false,
            || false,
            || "Minimize",
            config,
        )
        .style(|s| s.margin_right(16.0).margin_left(10.0)),
        clickable_icon(
            move || {
                if window_maximized.get() {
                    UmideIcons::WINDOW_RESTORE
                } else {
                    UmideIcons::WINDOW_MAXIMIZE
                }
            },
            move || {
                floem::action::set_window_maximized(
                    !window_maximized.get_untracked(),
                );
            },
            || false,
            || false,
            || "Maximize",
            config,
        )
        .style(|s| s.margin_right(16.0)),
        clickable_icon(
            || UmideIcons::WINDOW_CLOSE,
            move || {
                window_command.send(WindowCommand::CloseWindow);
            },
            || false,
            || false,
            || "Close Window",
            config,
        )
        .style(|s| s.margin_right(6.0)),
    ))
    .style(move |s| {
        s.apply_if(
            cfg!(target_os = "macos")
                || !config.get_untracked().core.custom_titlebar
                || (is_title && num_window_tabs.get() > 1),
            |s| s.hide(),
        )
    })
}
