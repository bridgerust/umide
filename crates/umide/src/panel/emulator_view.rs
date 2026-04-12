use floem::{
    View, ViewId,
    context::ComputeLayoutCx,
    peniko::kurbo::Rect,
    prelude::{SignalGet, SignalUpdate},
    reactive::{Effect, RwSignal},
    views::{Container, Decorators, Label, Scroll, Stack, dyn_stack},
};
use std::{rc::Rc, sync::Arc};

use crate::{
    app::clickable_icon,
    config::{color::UmideColor, icon::UmideIcons},
    panel::{position::PanelPosition, view::PanelBuilder},
    window_tab::WindowTabData,
};
#[cfg(target_os = "macos")]
use umide_emulator::native_view::NativeEmulatorView;
use umide_emulator::{
    DeviceInfo, DevicePlatform, DeviceState, decoder::DecodedFrame, launch_device,
    list_all_devices, stop_device,
};
#[cfg(target_os = "macos")]
use umide_native::emulator::EmulatorPlatform;

#[cfg(not(target_os = "macos"))]
#[derive(Clone, Copy)]
enum EmulatorPlatform {
    Android,
    Ios,
}

#[cfg(not(target_os = "macos"))]
struct NativeEmulatorView;

#[cfg(not(target_os = "macos"))]
impl NativeEmulatorView {
    fn new<T>(
        _handle: T,
        _x: i32,
        _y: i32,
        _width: u32,
        _height: u32,
        _platform: EmulatorPlatform,
    ) -> Result<Self, String> {
        Err("Native emulator embedding is only supported on macOS".to_string())
    }

    fn resize(&self, _x: i32, _y: i32, _width: u32, _height: u32) {}
    fn show(&self) {}
    fn hide(&self) {}
    fn attach_device(&mut self, _device_id: &str) {}
    fn is_android(&self) -> bool {
        false
    }
    fn start_grpc_stream(&mut self, _grpc_endpoint: &str) {}
}

struct NativeEmulatorWidget {
    id: ViewId,
    native_view: Rc<std::cell::RefCell<Option<NativeEmulatorView>>>,
    running_device: RwSignal<Option<DeviceInfo>>,
    is_visible: RwSignal<bool>,
    #[allow(dead_code)]
    current_device_id: RwSignal<String>,
    /// Track the last device ID we initialized for, to detect changes
    last_device_id: Option<String>,
    /// Floem frame signal (for panel data, not used for rendering directly)
    #[allow(dead_code)]
    frame_signal: RwSignal<Option<Arc<DecodedFrame>>>,
}

impl NativeEmulatorWidget {
    pub fn new(
        running_device: RwSignal<Option<DeviceInfo>>,
        is_visible: RwSignal<bool>,
        current_device_id: RwSignal<String>,
        frame_signal: RwSignal<Option<Arc<DecodedFrame>>>,
    ) -> Self {
        Self {
            id: ViewId::new(),
            native_view: Rc::new(std::cell::RefCell::new(None)),
            running_device,
            is_visible,
            current_device_id,
            last_device_id: None,
            frame_signal,
        }
    }

    /// Cleanup the native view
    fn cleanup(&mut self) {
        let mut view_lock = self.native_view.borrow_mut();
        if view_lock.is_some() {
            tracing::info!("Cleaning up native emulator view");
            *view_lock = None;
            self.last_device_id = None;
        }
    }
}

impl View for NativeEmulatorWidget {
    fn id(&self) -> ViewId {
        self.id
    }

    fn debug_name(&self) -> std::borrow::Cow<'static, str> {
        "NativeEmulatorWidget".into()
    }

    fn update(
        &mut self,
        _cx: &mut floem::context::UpdateCx,
        _state: Box<dyn std::any::Any>,
    ) {
        let current_device = self.running_device.get_untracked();
        let is_visible = self.is_visible.get_untracked();

        let mut should_cleanup = false;

        if !is_visible || current_device.is_none() {
            should_cleanup = true;
        } else if let Some(ref dev) = current_device {
            if let Some(ref last_id) = self.last_device_id {
                if dev.id != *last_id {
                    should_cleanup = true;
                }
            }
        }

        if should_cleanup {
            let has_view = self.native_view.borrow().is_some();
            if has_view {
                if let Some(ref view) = *self.native_view.borrow() {
                    view.hide();
                }
                self.cleanup();
            }
        } else if is_visible {
            if let Some(ref view) = *self.native_view.borrow() {
                view.show();
            }
        }
    }

    fn compute_layout(&mut self, _cx: &mut ComputeLayoutCx) -> Option<Rect> {
        let is_visible = self.is_visible.get_untracked();

        if !is_visible {
            if self.native_view.borrow().is_some() {
                self.cleanup();
            }
            return None;
        }

        let current_device = self.running_device.get_untracked();
        if current_device.is_none() && self.native_view.borrow().is_some() {
            self.cleanup();
            return None;
        }

        None
    }

    fn paint(&mut self, _cx: &mut floem::context::PaintCx) {
        let is_visible = self.is_visible.get_untracked();
        let current_device = self.running_device.get_untracked();

        // Cleanup if hidden or no device
        if !is_visible || current_device.is_none() {
            let has_view = self.native_view.borrow().is_some();
            if has_view {
                if let Some(ref view) = *self.native_view.borrow() {
                    view.hide();
                }
                self.cleanup();
            }
            return;
        }

        let window_origin = self.id.get_window_origin();
        let size = self
            .id
            .get_layout()
            .map(|l| (l.size.width as u32, l.size.height as u32));

        if let Some((width, height)) = size {
            if width == 0 || height == 0 {
                return;
            }

            // Use the widget's own layout position — Floem already accounts for
            // the toolbar, header, and sidebar in the layout tree
            let x = window_origin.x as i32;
            let y = window_origin.y as i32;
            let device_name = current_device
                .as_ref()
                .map(|d| d.name.as_str())
                .unwrap_or("unknown");

            tracing::debug!(
                "NativeEmulatorWidget [{}]: origin=({},{}) size={}x{}",
                device_name,
                x,
                y,
                width,
                height
            );

            let has_view = self.native_view.borrow().is_some();
            if has_view {
                if let Some(view) = &*self.native_view.borrow() {
                    view.resize(x, y, width, height);
                    view.show();
                }
            } else if is_visible {
                if let Some(device) = current_device {
                    use floem::window::WindowIdExt;

                    if let Some(window_id) = self.id.window_id() {
                        if let Some(handle) = window_id.raw_window_handle() {
                            let platform = match device.platform {
                                umide_emulator::common::DevicePlatform::Android => {
                                    EmulatorPlatform::Android
                                }
                                umide_emulator::common::DevicePlatform::Ios => {
                                    EmulatorPlatform::Ios
                                }
                            };

                            tracing::info!(
                                "Creating native emulator view for device: {} at ({},{}) size {}x{}",
                                device.name,
                                x,
                                y,
                                width,
                                height
                            );

                            match NativeEmulatorView::new(
                                handle, x, y, width, height, platform,
                            ) {
                                Ok(mut view) => {
                                    if !device.id.is_empty() {
                                        view.attach_device(&device.id);
                                    }

                                    // Android: start gRPC frame streaming (headless, no window)
                                    // iOS: uses ScreenCaptureKit via attach_device (no gRPC needed)
                                    if view.is_android() {
                                        view.start_grpc_stream(
                                            "http://localhost:8554",
                                        );
                                    }

                                    *self.native_view.borrow_mut() = Some(view);
                                    self.last_device_id = Some(device.id.clone());
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "Failed to create native emulator view: {}",
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Create a single platform panel (Android or iOS)
fn platform_panel(
    platform: DevicePlatform,
    devices: RwSignal<Vec<DeviceInfo>>,
    running_device: RwSignal<Option<DeviceInfo>>,
    is_visible: RwSignal<bool>,
    frame_signal: RwSignal<Option<Arc<umide_emulator::decoder::DecodedFrame>>>,
    current_device_id: RwSignal<String>,
    config: floem::reactive::ReadSignal<Arc<crate::config::UmideConfig>>,
) -> impl View {
    let platform_name = match &platform {
        DevicePlatform::Android => "Android",
        DevicePlatform::Ios => "iOS",
    };

    // Device item renderer
    let device_item = {
        move |device: DeviceInfo| {
            let device_cloned_start = device.clone();
            let device_cloned_stop = device.clone();
            let device_cloned_resume = device.clone();
            let is_running = device.state == DeviceState::Running;
            let is_starting = device.state == DeviceState::Starting;
            let is_disconnected = device.state == DeviceState::Disconnected;

            Stack::new((
                Label::new(device.name.clone())
                    .style(|s| s.flex_grow(1.0).padding_horiz(6.0).text_ellipsis()),
                // Start/Resume/Show button
                clickable_icon(
                    || UmideIcons::DEBUG_CONTINUE,
                    move || {
                        // Check if this device is already running but hidden
                        if let Some(ref running) = running_device.get_untracked() {
                            if running.id == device_cloned_resume.id {
                                // Just show it again
                                current_device_id
                                    .set(device_cloned_resume.id.clone());
                                is_visible.set(true);
                                return;
                            }
                        }
                        // Start new device
                        let _ = launch_device(&device_cloned_start);
                        let mut d = device_cloned_start.clone();
                        d.state = DeviceState::Running;
                        running_device.set(Some(d));
                        current_device_id.set(device_cloned_start.id.clone());
                        is_visible.set(true);
                    },
                    || false,
                    move || {
                        // Visible if:
                        // 1. Device is disconnected (to Start)
                        // 2. OR Device is running AND hidden (to Show/Resume)
                        // So hidden if: device is running AND visible
                        if is_running {
                            // If running, hide "Start" button unless view is hidden
                            is_visible.get()
                        } else {
                            // If not running, show "Start" button unless starting?
                            // Original logic: !is_disconnected && !is_running -> hidden.
                            // Wait, is_disconnected means state == Disconnected.
                            // If state == Starting, is_disconnected = false, is_running = false.
                            // Then hidden = true. Correct (don't show start while starting).
                            !is_disconnected
                        }
                    },
                    move || if is_running { "Show" } else { "Start" },
                    config,
                ),
                clickable_icon(
                    || UmideIcons::DEBUG_STOP,
                    move || {
                        let _ = stop_device(&device_cloned_stop);
                        running_device.set(None);
                        is_visible.set(false);
                        frame_signal.set(None);
                        current_device_id.set(String::new());
                    },
                    || false,
                    move || !is_running,
                    || "Stop",
                    config,
                ),
                Label::new({ if is_starting { "Starting..." } else { "" } })
                    .style(|s| s.padding_horiz(5.0).font_size(10.0)),
            ))
            .style(|s| {
                s.width_full()
                    .flex_row()
                    .items_center()
                    .padding_vert(5.0)
                    .border_bottom(1.0)
                    .gap(2.0)
            })
        }
    };

    Stack::new((
        // Header with device name (ABOVE native view so always visible)
        Container::new(
            Stack::horizontal((
                Label::new(platform_name.to_string())
                    .style(move |s| s.font_size(12.0).font_bold().padding(6.0)),
                // Show running device name in header
                Label::derived(move || {
                    if let Some(device) = running_device.get() {
                        if is_visible.get() {
                            format!(" - {}", device.name)
                        } else {
                            format!(" - {} (hidden)", device.name)
                        }
                    } else {
                        String::new()
                    }
                })
                .style(|s| s.font_size(10.0).flex_grow(1.0).padding_right(6.0)),
            ))
            .style(|s| s.width_full().items_center()),
        )
        .style(move |s| {
            let config = config.get();
            s.width_full()
                .border_bottom(1.0)
                .border_color(config.color(UmideColor::LAPCE_BORDER))
        }),
        // Content: Device list OR Emulator view
        Stack::new((
            // Device list (shown when no device or hidden)
            Scroll::new(
                dyn_stack(
                    move || {
                        let visible = is_visible.get();
                        let has_running = running_device.get().is_some();

                        // Show list if not visible or no device running
                        if has_running && visible {
                            return Vec::new();
                        }

                        devices
                            .get()
                            .into_iter()
                            .filter(|d| d.platform == platform)
                            .collect::<Vec<_>>()
                    },
                    |d| format!("{}-{}", d.id, d.state as u32),
                    device_item,
                )
                .style(|s| s.flex_col().width_full()),
            )
            .style(move |s| {
                let visible = is_visible.get();
                let has_running = running_device.get().is_some();
                s.flex_grow(1.0)
                    .width_full()
                    .min_width(0.0)
                    .min_height(0.0)
                    .apply_if(has_running && visible, |s| s.hide())
            }),
            // Native Emulator View + Sidebar (shown when device running AND visible)
            Stack::horizontal({
                let native_widget = NativeEmulatorWidget::new(
                    running_device,
                    is_visible,
                    current_device_id,
                    frame_signal,
                );
                let widget_id = native_widget.id();

                // Effect to force an update when signals change
                Effect::new(move |_| {
                    let _ = is_visible.get();
                    let _ = running_device.get();
                    widget_id.update_state(());
                    widget_id.request_paint();
                });

                (
                    // Emulator display
                    native_widget
                        .style(|s| s.flex_grow(1.0).width_full().min_height(0.0)),
                    // Emulator sidebar controls
                    emulator_sidebar(
                        platform,
                        running_device,
                        is_visible,
                        frame_signal,
                        current_device_id,
                        config,
                    ),
                )
            })
            .style(move |s| {
                let visible = is_visible.get();
                let has_running = running_device.get().is_some();
                s.flex_grow(1.0)
                    .width_full()
                    .min_width(0.0)
                    .min_height(0.0)
                    .apply_if(!has_running || !visible, |s| s.hide())
            }),
        ))
        .style(|s| {
            s.flex_col()
                .flex_grow(1.0)
                .width_full()
                .min_width(0.0)
                .min_height(0.0)
        }),
    ))
    .style(move |s| {
        let config = config.get();
        s.flex_col()
            .flex_grow(1.0)
            .flex_basis(0.0)
            .min_width(0.0)
            .min_height(0.0)
            .border(1.0)
            .border_color(config.color(UmideColor::LAPCE_BORDER))
    })
}

/// Helper to create a sidebar button that executes an arbitrary shell command
fn action_button(
    label: &'static str,
    _tooltip: &'static str,
    device_id: RwSignal<String>,
    cmd_builder: impl Fn(String) -> String + 'static + Send + Sync,
    config: floem::reactive::ReadSignal<Arc<crate::config::UmideConfig>>,
) -> impl View {
    let label_text = label.to_string();
    Stack::new((Label::new(label_text).style(|s| s.font_size(14.0)),))
        .on_click_stop(move |_| {
            let id = device_id.get_untracked();
            if id.is_empty() {
                return;
            }

            let cmd = cmd_builder(id);
            std::thread::spawn(move || {
                let env_path = std::env::var("PATH").unwrap_or_default();
                let _ = std::process::Command::new("sh")
                    .env(
                        "PATH",
                        format!("/opt/homebrew/bin:/usr/local/bin:{}", env_path),
                    )
                    .arg("-c")
                    .arg(&cmd)
                    .output();
            });
        })
        .style(move |s| {
            let config_val = config.get();
            s.width(28.0)
                .height(28.0)
                .items_center()
                .justify_center()
                .border_radius(4.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .border(1.0)
                .border_color(config_val.color(UmideColor::LAPCE_BORDER))
                .hover(|s| {
                    s.background(floem::peniko::Color::from_rgba8(255, 255, 255, 20))
                })
        })
}

/// Emulator sidebar with control buttons (Stop/Hide for all, hardware buttons for Android)
fn emulator_sidebar(
    platform: DevicePlatform,
    running_device: RwSignal<Option<DeviceInfo>>,
    is_visible: RwSignal<bool>,
    frame_signal: RwSignal<Option<Arc<umide_emulator::decoder::DecodedFrame>>>,
    current_device_id: RwSignal<String>,
    config: floem::reactive::ReadSignal<Arc<crate::config::UmideConfig>>,
) -> impl View {
    let is_android = matches!(platform, DevicePlatform::Android);

    Stack::new((
        // Generic controls (both platforms)
        Stack::new((
            // Stop button
            clickable_icon(
                || UmideIcons::DEBUG_STOP,
                move || {
                    if let Some(device) = running_device.get_untracked() {
                        tracing::info!("Stopping device: {}", device.name);
                        if let Err(e) = stop_device(&device) {
                            tracing::error!("Failed to stop device {}: {}", device.name, e);
                        }
                    }
                    running_device.set(None);
                    is_visible.set(false);
                    frame_signal.set(None);
                    current_device_id.set(String::new());
                },
                || false,
                || false,
                || "Stop",
                config,
            ),
            Label::new("Stop").style(|s| s.font_size(10.0)),
            // Separator
            Label::new("").style(|s| s.height(8.0)),
            // Hide button
            clickable_icon(
                || UmideIcons::CLOSE,
                move || {
                    tracing::info!("Hiding emulator view (device still running)");
                    is_visible.set(false);
                },
                || false,
                || false,
                || "Hide",
                config,
            ),
            Label::new("Hide").style(|s| s.font_size(10.0)),
            // Separator
            Label::new("").style(|s| s.height(8.0)),
        ))
        .style(|s| s.flex_col().items_center().gap(2.0)),

        // Android hardware controls
        Stack::new((
            action_button("🏠", "Home", current_device_id, |_id| "adb shell input keyevent 3".to_string(), config),
            action_button("◀", "Back", current_device_id, |_id| "adb shell input keyevent 4".to_string(), config),
            action_button("▦", "Overview", current_device_id, |_id| "adb shell input keyevent 187".to_string(), config),
            Label::new("").style(|s| s.height(8.0)),
            action_button("🔊", "Vol+", current_device_id, |_id| "adb shell input keyevent 24".to_string(), config),
            action_button("🔉", "Vol-", current_device_id, |_id| "adb shell input keyevent 25".to_string(), config),
            Label::new("").style(|s| s.height(8.0)),
            action_button("⏻", "Power", current_device_id, |_id| "adb shell input keyevent 26".to_string(), config),
            Label::new("").style(|s| s.height(8.0)),
            action_button("📷", "Screenshot", current_device_id, |_id| "adb exec-out screencap -p > ~/Desktop/umide_screenshot_$(date +%s).png".to_string(), config),
        ))
        .style(move |s| {
            s.flex_col()
                .items_center()
                .gap(4.0)
                .apply_if(!is_android, |s| s.hide())
        }),

        // iOS hardware controls
        Stack::new((
            action_button("🏠", "Home", current_device_id, |id| format!("idb ui button --udid {} HOME", id), config),
            Label::new("").style(|s| s.height(8.0)),
            action_button("📷", "Screenshot", current_device_id, |id| format!("idb screenshot --udid {} ~/Desktop/umide_screenshot_$(date +%s).png", id), config),
        ))
        .style(move |s| {
            s.flex_col()
                .items_center()
                .gap(4.0)
                .apply_if(is_android, |s| s.hide()) // Hide if android
        })
    ))
    .style(move |s| {
        let config_val = config.get();
        s.flex_col()
            .items_center()
            .padding(4.0)
            .border_left(1.0)
            .border_color(config_val.color(UmideColor::LAPCE_BORDER))
    })
}

pub fn emulator_panel(
    window_tab_data: Rc<WindowTabData>,
    position: PanelPosition,
) -> impl View {
    #[cfg(not(target_os = "macos"))]
    {
        let config = window_tab_data.common.config;
        return PanelBuilder::new(config, position)
            .add(
                "Emulators",
                Stack::new((
                    Label::new(
                        "Emulator embedding is currently macOS only.".to_string(),
                    )
                    .style(|s| s.padding(12.0).font_size(13.0)),
                    Label::new("Windows and Linux support coming soon.".to_string())
                        .style(move |s| {
                            s.padding_horiz(12.0)
                                .padding_bottom(12.0)
                                .font_size(12.0)
                                .color(config.get().color(UmideColor::EDITOR_DIM))
                        }),
                ))
                .style(|s| s.flex_col().width_full()),
                window_tab_data
                    .panel
                    .section_open(crate::panel::data::PanelSection::Process),
            )
            .build();
    }

    #[cfg(target_os = "macos")]
    {
        let config = window_tab_data.common.config;
        let devices = RwSignal::new(Vec::<DeviceInfo>::new());

        let running_android = RwSignal::new(None::<DeviceInfo>);
        let running_ios = RwSignal::new(None::<DeviceInfo>);

        let android_visible = RwSignal::new(false);
        let ios_visible = RwSignal::new(false);

        let current_android_id = RwSignal::new(String::new());
        let current_ios_id = RwSignal::new(String::new());

        let android_frame = window_tab_data.panel.android_frame;
        let ios_frame = window_tab_data.panel.ios_frame;

        Effect::new(move |_| {
            let dev_list = list_all_devices();
            for device in &dev_list {
                if device.state == DeviceState::Running {
                    match device.platform {
                        DevicePlatform::Android => {
                            if running_android.get().is_none() {
                                running_android.set(Some(device.clone()));
                                current_android_id.set(device.id.clone());
                            }
                        }
                        DevicePlatform::Ios => {
                            if running_ios.get().is_none() {
                                running_ios.set(Some(device.clone()));
                                current_ios_id.set(device.id.clone());
                            }
                        }
                    }
                }
            }
            devices.set(dev_list);
        });

        let android_panel_visible = RwSignal::new(true);
        let ios_panel_visible = RwSignal::new(true);

        let toggle_btn = move |label: &'static str, visible_sig: RwSignal<bool>| {
            Label::derived(move || {
                if visible_sig.get() {
                    format!("☑ {}", label)
                } else {
                    format!("☐ {}", label)
                }
            })
            .on_click_stop(move |_| {
                visible_sig.update(|v| *v = !*v);
            })
            .style(move |s| {
                s.cursor(floem::style::CursorStyle::Pointer)
                    .padding_right(15.0)
                    .font_size(12.0)
            })
        };

        let toggle_bar = Stack::horizontal((
            toggle_btn("Android", android_panel_visible),
            toggle_btn("iOS", ios_panel_visible),
        ))
        .style(move |s| {
            let config_val = config.get();
            s.width_full()
                .padding(5.0)
                .border_bottom(1.0)
                .border_color(config_val.color(UmideColor::LAPCE_BORDER))
        });

        return PanelBuilder::new(config, position)
            .add(
                "Emulators",
                Stack::new((
                    toggle_bar,
                    Stack::new((
                        platform_panel(
                            DevicePlatform::Android,
                            devices,
                            running_android,
                            android_visible,
                            android_frame,
                            current_android_id,
                            config,
                        )
                        .style(move |s| {
                            s.flex_grow(1.0)
                                .flex_basis(0.0)
                                .min_height(0.0)
                                .apply_if(!android_panel_visible.get(), |s| s.hide())
                        }),
                        platform_panel(
                            DevicePlatform::Ios,
                            devices,
                            running_ios,
                            ios_visible,
                            ios_frame,
                            current_ios_id,
                            config,
                        )
                        .style(move |s| {
                            s.flex_grow(1.0)
                                .flex_basis(0.0)
                                .min_height(0.0)
                                .apply_if(!ios_panel_visible.get(), |s| s.hide())
                        }),
                    ))
                    .style(|s| {
                        s.flex_col()
                            .flex_grow(1.0)
                            .width_full()
                            .min_height(0.0)
                            .items_stretch()
                            .gap(5.0)
                            .padding(5.0)
                    }),
                ))
                .style(|s| s.flex_col().flex_grow(1.0).width_full().min_height(0.0)),
                window_tab_data
                    .panel
                    .section_open(crate::panel::data::PanelSection::Process),
            )
            .build();
    }
}
