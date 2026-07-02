use floem::{
    View,
    prelude::{SignalGet, SignalUpdate},
    reactive::{Effect, RwSignal},
    views::{Container, Decorators, Label, Scroll, Stack, dyn_stack, svg},
};
#[cfg(target_os = "macos")]
use floem::{ViewId, context::ComputeLayoutCx, peniko::kurbo::Rect};
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

/// Create a single platform panel (Android or iOS) backed by the macOS-only
/// native overlay (`NativeEmulatorWidget`). The portable wgpu/`video_frame`
/// path is in [`android_panel_portable`] below.
#[cfg(target_os = "macos")]
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

/// Helper to create a sidebar button that executes an arbitrary shell command.
/// macOS-only — uses `sh -c` with Homebrew's PATH; the portable Windows/Linux
/// panel currently exposes a smaller set of controls (no shell shellouts).
#[cfg(target_os = "macos")]
fn action_button(
    icon: &'static str,
    _tooltip: &'static str,
    device_id: RwSignal<String>,
    cmd_builder: impl Fn(String) -> String + 'static + Send + Sync,
    config: floem::reactive::ReadSignal<Arc<crate::config::UmideConfig>>,
) -> impl View {
    // Themed SVG icon (same set as the portable panel's hardware buttons), not
    // an emoji glyph — emoji render as color bitmaps and clash with the IDE.
    Stack::new((svg(move || config.get().ui_svg(icon)).style(move |s| {
        s.size(16.0, 16.0)
            .color(config.get().color(UmideColor::LAPCE_ICON_ACTIVE))
    }),))
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

/// Emulator sidebar with control buttons (Stop/Hide for all, hardware buttons
/// for Android). macOS-only — the portable panel has its own minimal sidebar.
#[cfg(target_os = "macos")]
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
            action_button(UmideIcons::DEVICE_HOME, "Home", current_device_id, |_id| "adb shell input keyevent 3".to_string(), config),
            action_button(UmideIcons::DEVICE_BACK, "Back", current_device_id, |_id| "adb shell input keyevent 4".to_string(), config),
            action_button(UmideIcons::DEVICE_RECENTS, "Overview", current_device_id, |_id| "adb shell input keyevent 187".to_string(), config),
            Label::new("").style(|s| s.height(8.0)),
            action_button(UmideIcons::DEVICE_VOLUME_UP, "Vol+", current_device_id, |_id| "adb shell input keyevent 24".to_string(), config),
            action_button(UmideIcons::DEVICE_VOLUME_DOWN, "Vol-", current_device_id, |_id| "adb shell input keyevent 25".to_string(), config),
            Label::new("").style(|s| s.height(8.0)),
            action_button(UmideIcons::DEVICE_POWER, "Power", current_device_id, |_id| "adb shell input keyevent 26".to_string(), config),
            Label::new("").style(|s| s.height(8.0)),
            action_button(UmideIcons::DEVICE_SCREENSHOT, "Screenshot", current_device_id, |_id| "adb exec-out screencap -p > ~/Desktop/umide_screenshot_$(date +%s).png".to_string(), config),
        ))
        .style(move |s| {
            s.flex_col()
                .items_center()
                .gap(4.0)
                .apply_if(!is_android, |s| s.hide())
        }),

        // iOS hardware controls
        Stack::new((
            action_button(UmideIcons::DEVICE_HOME, "Home", current_device_id, |id| format!("idb ui button --udid {} HOME", id), config),
            Label::new("").style(|s| s.height(8.0)),
            action_button(UmideIcons::DEVICE_SCREENSHOT, "Screenshot", current_device_id, |id| format!("idb screenshot --udid {} ~/Desktop/umide_screenshot_$(date +%s).png", id), config),
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

/// Cross-platform Android emulator panel using floem's `video_frame` view to
/// render frames from `start_emulator_stream` directly on the GPU, with
/// pointer input forwarded to the device via M3's `start_emulator_input`.
///
/// This is the portable counterpart to the macOS `platform_panel` overlay
/// path. iOS is not exposed here — iOS Simulator is permanently macOS-only.
/// Pointer (tap/drag), hardware buttons (Home/Back/Recents/Power), and keyboard
/// are all wired — each forwarded to the device over gRPC.
#[cfg(not(target_os = "macos"))]
fn android_panel_portable(
    devices: RwSignal<Vec<DeviceInfo>>,
    running_device: RwSignal<Option<DeviceInfo>>,
    is_visible: RwSignal<bool>,
    frame_signal: RwSignal<Option<Arc<DecodedFrame>>>,
    current_device_id: RwSignal<String>,
    config: floem::reactive::ReadSignal<Arc<crate::config::UmideConfig>>,
) -> impl View {
    use crate::panel::emulator_stream::{
        EmulatorInput, capture_screenshot, default_screenshot_path,
        start_emulator_input, start_emulator_stream, view_to_device,
    };
    use floem::event::{Event, EventListener};
    use floem::ext_event::update_signal_from_channel;
    use floem::kurbo::Size;
    use floem::prelude::{Key, NamedKey};
    use floem::style::Position;
    use floem::views::{RgbaFrame, video_frame};

    const ENDPOINT: &str = "http://localhost:8554";

    // The gRPC endpoint of the device being viewed: its discovered port (so the
    // panel connects to the *right* emulator, not just the default 8554) or the
    // 8554 fallback. The serial resolves after boot; before then — or if the
    // discovery file is absent — we use 8554, which is what the panel launches
    // with. (A running emulator detected at app start already carries its
    // serial, so an external emulator on a non-default port connects correctly.)
    let endpoint_for = move || -> String {
        running_device
            .get_untracked()
            .and_then(|d| d.serial)
            .and_then(|s| umide_emulator::AndroidEmulator::grpc_port(&s))
            .map(|port| format!("http://localhost:{port}"))
            .unwrap_or_else(|| ENDPOINT.to_string())
    };

    // Start the gRPC frame stream and the input command connection exactly once
    // when an Android device first runs. Both stay alive for the panel's
    // lifetime — a future polish is per-launch lifecycle, but for the preview
    // the first stream is sufficient: `connect_with_retry` waits for boot.
    let stream_started = RwSignal::new(false);
    let input_handle: RwSignal<Option<EmulatorInput>> = RwSignal::new(None);
    // Native device resolution, probed by the stream; pointer input maps to it
    // (the emulator's touch input is in native pixels, not the downscaled
    // stream resolution).
    let native_size: RwSignal<Option<(u32, u32)>> = RwSignal::new(None);
    Effect::new(move |_| {
        if running_device.get().is_some() && !stream_started.get_untracked() {
            let endpoint = endpoint_for();
            start_emulator_stream(endpoint.clone(), frame_signal, native_size);
            input_handle.set(Some(start_emulator_input(endpoint)));
            stream_started.set(true);
        }
    });

    // A device launched from the Start button isn't booted yet, so its adb
    // serial (`emulator-<port>`) can't be known at click time. The Start handler
    // resolves it off the UI thread once boot completes and delivers `(avd_id,
    // serial)` here; we reconcile it into `running_device` so G2's
    // `active_device` carries the serial for the AI agent to target. Keyed by
    // AVD id and guarded on the current device, so a Stop (or a different Start)
    // before boot can't resurrect a stale device.
    let (serial_tx, serial_rx) = std::sync::mpsc::channel::<(String, String)>();
    let resolved_serial: RwSignal<Option<(String, String)>> = RwSignal::new(None);
    update_signal_from_channel(resolved_serial.write_only(), serial_rx);
    Effect::new(move |_| {
        if let Some((avd_id, serial)) = resolved_serial.get() {
            if let Some(mut dev) = running_device.get_untracked() {
                if dev.id == avd_id && dev.serial.as_deref() != Some(serial.as_str())
                {
                    dev.serial = Some(serial);
                    running_device.set(Some(dev));
                }
            }
        }
    });

    // Pointer state for touch_down → move → up gesture forwarding.
    let view_size = RwSignal::new(Size::ZERO);
    let pressed = RwSignal::new(false);
    let last = RwSignal::new((0i32, 0i32));

    // Map a view-local pointer position to a device pixel, through the
    // aspect-preserving letterbox. `None` when there is no frame yet or the
    // point is in the letterbox margins. The device size is the native
    // resolution (not the downscaled stream), so taps land correctly.
    let to_device = move |e: &Event| -> Option<(i32, i32)> {
        let p = e.point()?;
        let sz = view_size.get_untracked();
        let f = frame_signal.get_untracked()?;
        let (dw, dh) = match native_size.get_untracked() {
            Some((w, h)) if w > 0 && h > 0 => (w, h),
            _ => (f.width, f.height),
        };
        view_to_device(p.x, p.y, sz.width, sz.height, dw, dh)
    };

    // A hardware-control button: a themed monochrome icon (rendered exactly like
    // the Stop/Hide buttons) that forwards a named device key over gRPC (e.g.
    // "GoHome") — the same path as touch, no adb shell. No-op until the input
    // command channel has connected.
    let hw_button =
        move |icon: &'static str, tooltip: &'static str, key: &'static str| {
            clickable_icon(
                move || icon,
                move || {
                    if let Some(input) = input_handle.get_untracked() {
                        input.key(key);
                    }
                },
                || false,
                || false,
                move || tooltip,
                config,
            )
        };

    // Volume + Rotate go through `adb` (using the running device's serial),
    // not the gRPC key path: the emulator ignores a lone gRPC key-press for
    // volume (non-character key), and rotate has no gRPC equivalent. Run off the
    // UI thread (B2); a no-op until the device serial has resolved after boot.
    let adb_key_button =
        move |icon: &'static str, tooltip: &'static str, keycode: i32| {
            clickable_icon(
                move || icon,
                move || {
                    if let Some(serial) =
                        running_device.get_untracked().and_then(|d| d.serial)
                    {
                        std::thread::spawn(move || {
                            let _ = umide_emulator::AndroidEmulator::press_keyevent(
                                &serial, keycode,
                            );
                        });
                    }
                },
                || false,
                || false,
                move || tooltip,
                config,
            )
        };

    // Map a floem key press to the device key string the emulator's gRPC
    // `sendKey` accepts (DOM-style names for the named keys; the raw character
    // otherwise). Returns `None` for keys we don't forward (e.g. bare modifiers).
    let to_device_key = |key: &Key| -> Option<String> {
        Some(match key {
            Key::Character(c) => c.to_string(),
            Key::Named(NamedKey::Enter) => "Enter".to_string(),
            Key::Named(NamedKey::Backspace) => "Backspace".to_string(),
            Key::Named(NamedKey::Tab) => "Tab".to_string(),
            Key::Named(NamedKey::Escape) => "Escape".to_string(),
            Key::Named(NamedKey::Delete) => "Delete".to_string(),
            Key::Named(NamedKey::ArrowUp) => "ArrowUp".to_string(),
            Key::Named(NamedKey::ArrowDown) => "ArrowDown".to_string(),
            Key::Named(NamedKey::ArrowLeft) => "ArrowLeft".to_string(),
            Key::Named(NamedKey::ArrowRight) => "ArrowRight".to_string(),
            _ => return None,
        })
    };

    let device_item = move |device: DeviceInfo| {
        let device_cloned_start = device.clone();
        let device_cloned_stop = device.clone();
        let device_cloned_resume = device.clone();
        // Per-item sender so the async launch can report the resolved adb serial.
        let serial_tx = serial_tx.clone();
        let is_running = device.state == DeviceState::Running;
        let is_starting = device.state == DeviceState::Starting;
        let is_disconnected = device.state == DeviceState::Disconnected;

        Stack::new((
            Label::new(device.name.clone())
                .style(|s| s.flex_grow(1.0).padding_horiz(6.0).text_ellipsis()),
            clickable_icon(
                || UmideIcons::DEBUG_CONTINUE,
                move || {
                    if let Some(ref running) = running_device.get_untracked() {
                        if running.id == device_cloned_resume.id {
                            current_device_id.set(device_cloned_resume.id.clone());
                            is_visible.set(true);
                            return;
                        }
                    }
                    // Launch off the UI thread — AndroidEmulator::launch blocks
                    // ~30s polling adb, which would freeze floem (B2). The frame
                    // stream's connect_with_retry waits for the device to boot.
                    let to_launch = device_cloned_start.clone();
                    let serial_tx = serial_tx.clone();
                    std::thread::spawn(move || match launch_device(&to_launch) {
                        Ok(()) => {
                            // Booted — resolve the adb serial and hand it back to
                            // the UI thread to fill into the running device.
                            if let Some(serial) =
                                umide_emulator::AndroidEmulator::running_serial(
                                    &to_launch.id,
                                )
                            {
                                let _ =
                                    serial_tx.send((to_launch.id.clone(), serial));
                            }
                        }
                        Err(e) => tracing::error!(
                            "Failed to launch {}: {}",
                            to_launch.name,
                            e
                        ),
                    });
                    let mut d = device_cloned_start.clone();
                    d.state = DeviceState::Running;
                    running_device.set(Some(d));
                    current_device_id.set(device_cloned_start.id.clone());
                    is_visible.set(true);
                },
                || false,
                move || {
                    if is_running {
                        is_visible.get()
                    } else {
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
                    // Reset the stream/input latch so a later Start re-connects
                    // (B1) — otherwise the panel stays black after the first Stop.
                    stream_started.set(false);
                    input_handle.set(None);
                    pressed.set(false);
                },
                || false,
                move || !is_running,
                || "Stop",
                config,
            ),
            Label::new(if is_starting { "Starting..." } else { "" })
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
    };

    Stack::new((
        // Header
        Container::new(
            Stack::horizontal((
                Label::new("Android".to_string())
                    .style(|s| s.font_size(12.0).font_bold().padding(6.0)),
                // Make the preview status self-evident in the panel itself, not
                // just in the docs (B3-adjacent): pointer + hardware buttons +
                // keyboard are wired; still preview-grade (single emulator, etc.).
                Label::new("PREVIEW".to_string()).style(move |s| {
                    let config = config.get();
                    s.font_size(8.0)
                        .padding_horiz(4.0)
                        .border_radius(3.0)
                        .border(1.0)
                        .border_color(config.color(UmideColor::LAPCE_BORDER))
                        .color(config.color(UmideColor::EDITOR_DIM))
                }),
                Label::derived(move || {
                    if let Some(device) = running_device.get() {
                        if is_visible.get() {
                            if frame_signal.get().is_none() {
                                format!(" - {} (connecting…)", device.name)
                            } else {
                                format!(" - {}", device.name)
                            }
                        } else {
                            format!(" - {} (hidden)", device.name)
                        }
                    } else {
                        String::new()
                    }
                })
                .style(|s| s.font_size(10.0).flex_grow(1.0).padding_horiz(6.0)),
            ))
            .style(|s| s.width_full().items_center()),
        )
        .style(move |s| {
            let config = config.get();
            s.width_full()
                .border_bottom(1.0)
                .border_color(config.color(UmideColor::LAPCE_BORDER))
        }),
        // Content: device list OR live emulator view
        Stack::new((
            Scroll::new(
                dyn_stack(
                    move || {
                        let visible = is_visible.get();
                        let has_running = running_device.get().is_some();
                        if has_running && visible {
                            return Vec::new();
                        }
                        devices
                            .get()
                            .into_iter()
                            .filter(|d| d.platform == DevicePlatform::Android)
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
            // Live device view + minimal sidebar (Stop/Hide).
            Stack::horizontal((
                // Video + a "Connecting…" overlay while no frame has arrived,
                // so an empty panel reads as connecting rather than broken (B3).
                Stack::new((
                    video_frame(move || {
                        frame_signal.get().and_then(|f| {
                            // Reuse the frame's existing Arc — cloning the RGBA
                            // buffer here is a ~10 MB memcpy on every repaint.
                            f.rgba_arc().map(|data| RgbaFrame {
                                data,
                                width: f.width,
                                height: f.height,
                            })
                        })
                    })
                    .on_resize(move |rect| view_size.set(rect.size()))
                    .on_event_stop(EventListener::PointerDown, move |e| {
                        if let Some(input) = input_handle.get_untracked() {
                            if let Some((x, y)) = to_device(e) {
                                pressed.set(true);
                                last.set((x, y));
                                input.touch_down(x, y);
                            }
                        }
                    })
                    .on_event_stop(EventListener::PointerMove, move |e| {
                        if pressed.get_untracked() {
                            if let Some(input) = input_handle.get_untracked() {
                                if let Some((x, y)) = to_device(e) {
                                    last.set((x, y));
                                    input.touch_move(x, y);
                                }
                            }
                        }
                    })
                    .on_event_stop(EventListener::PointerUp, move |e| {
                        if pressed.get_untracked() {
                            pressed.set(false);
                            if let Some(input) = input_handle.get_untracked() {
                                let (x, y) = to_device(e)
                                    .unwrap_or_else(|| last.get_untracked());
                                input.touch_up(x, y);
                            }
                        }
                    })
                    // Forward each key press over gRPC; the view is `focusable`
                    // (below) so clicking into the panel routes the keyboard here.
                    .on_event_stop(EventListener::KeyDown, move |e| {
                        if let Event::Key(ke) = e {
                            if let Some(input) = input_handle.get_untracked() {
                                if let Some(k) = to_device_key(&ke.key) {
                                    input.key(&k);
                                }
                            }
                        }
                    })
                    .style(|s| s.size_full().focusable(true)),
                    // Centered status overlay; pointer-transparent so taps still
                    // reach the device, and hidden once frames start arriving.
                    Container::new(
                        Label::derived(move || {
                            if frame_signal.get().is_none() {
                                "Connecting to emulator…".to_string()
                            } else {
                                String::new()
                            }
                        })
                        .style(move |s| {
                            s.font_size(13.0)
                                .color(config.get().color(UmideColor::EDITOR_DIM))
                        }),
                    )
                    .style(move |s| {
                        s.position(Position::Absolute)
                            .size_full()
                            .items_center()
                            .justify_center()
                            .pointer_events_none()
                            .apply_if(frame_signal.get().is_some(), |s| s.hide())
                    }),
                ))
                .style(|s| s.flex_grow(1.0).min_width(0.0).min_height(0.0)),
                Stack::new((
                    clickable_icon(
                        || UmideIcons::DEBUG_STOP,
                        move || {
                            if let Some(device) = running_device.get_untracked() {
                                tracing::info!("Stopping device: {}", device.name);
                                if let Err(e) = stop_device(&device) {
                                    tracing::error!(
                                        "Failed to stop device {}: {}",
                                        device.name,
                                        e
                                    );
                                }
                            }
                            running_device.set(None);
                            is_visible.set(false);
                            frame_signal.set(None);
                            current_device_id.set(String::new());
                            // Reset the stream/input latch so a later Start
                            // re-connects (B1).
                            stream_started.set(false);
                            input_handle.set(None);
                            pressed.set(false);
                        },
                        || false,
                        || false,
                        || "Stop",
                        config,
                    ),
                    Label::new("Stop").style(|s| s.font_size(10.0)),
                    Label::new("").style(|s| s.height(8.0)),
                    clickable_icon(
                        || UmideIcons::CLOSE,
                        move || {
                            tracing::info!(
                                "Hiding emulator view (device still running)"
                            );
                            is_visible.set(false);
                        },
                        || false,
                        || false,
                        || "Hide",
                        config,
                    ),
                    Label::new("Hide").style(|s| s.font_size(10.0)),
                    Label::new("").style(|s| s.height(10.0)),
                    // Hardware buttons — forwarded to the device over gRPC.
                    hw_button(UmideIcons::DEVICE_HOME, "Home", "GoHome"),
                    hw_button(UmideIcons::DEVICE_BACK, "Back", "GoBack"),
                    hw_button(UmideIcons::DEVICE_RECENTS, "Recents", "AppSwitch"),
                    hw_button(UmideIcons::DEVICE_POWER, "Power", "Power"),
                    // Volume (adb keyevent 24/25) + Rotate (adb emu rotate).
                    adb_key_button(UmideIcons::DEVICE_VOLUME_UP, "Volume up", 24),
                    adb_key_button(
                        UmideIcons::DEVICE_VOLUME_DOWN,
                        "Volume down",
                        25,
                    ),
                    clickable_icon(
                        || UmideIcons::DEVICE_ROTATE,
                        move || {
                            if let Some(serial) =
                                running_device.get_untracked().and_then(|d| d.serial)
                            {
                                std::thread::spawn(move || {
                                    let _ = umide_emulator::AndroidEmulator::rotate(
                                        &serial,
                                    );
                                });
                            }
                        },
                        || false,
                        || false,
                        || "Rotate",
                        config,
                    ),
                    Label::new("").style(|s| s.height(6.0)),
                    // Screenshot — save a native-res PNG and reveal it.
                    clickable_icon(
                        || UmideIcons::DEVICE_SCREENSHOT,
                        move || {
                            capture_screenshot(
                                endpoint_for(),
                                default_screenshot_path(),
                            );
                        },
                        || false,
                        || false,
                        || "Screenshot",
                        config,
                    ),
                ))
                .style(move |s| {
                    let config_val = config.get();
                    s.flex_col()
                        .items_center()
                        .padding(4.0)
                        .gap(2.0)
                        .border_left(1.0)
                        .border_color(config_val.color(UmideColor::LAPCE_BORDER))
                }),
            ))
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

pub fn emulator_panel(
    window_tab_data: Rc<WindowTabData>,
    position: PanelPosition,
) -> impl View {
    #[cfg(not(target_os = "macos"))]
    {
        let config = window_tab_data.common.config;
        let devices = RwSignal::new(Vec::<DeviceInfo>::new());
        let running_device = RwSignal::new(None::<DeviceInfo>);
        let is_visible = RwSignal::new(false);
        let current_device_id = RwSignal::new(String::new());
        let frame_signal = window_tab_data.panel.android_frame;

        Effect::new(move |_| {
            let dev_list = list_all_devices();
            for device in &dev_list {
                if device.state == DeviceState::Running
                    && device.platform == DevicePlatform::Android
                    && running_device.get().is_none()
                {
                    running_device.set(Some(device.clone()));
                    current_device_id.set(device.id.clone());
                }
            }
            devices.set(dev_list);
        });

        // G2 producer: mirror the running device into shared panel state so the
        // AI agent (resolve_target in ai.rs) can target the device the user is
        // viewing. None when nothing is running.
        let active_device = window_tab_data.panel.active_device;
        Effect::new(move |_| {
            active_device.set(running_device.get());
        });

        return PanelBuilder::new(config, position)
            .add(
                "Emulators",
                android_panel_portable(
                    devices,
                    running_device,
                    is_visible,
                    frame_signal,
                    current_device_id,
                    config,
                ),
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

        // G2 producer (macOS): mirror the device the user is viewing into shared
        // panel state so the AI agent (`resolve_target` in `ai.rs`) targets it.
        // Both an Android emulator and an iOS simulator can run at once here, so
        // prefer Android — matching `resolve_target`'s auto-detect order; the
        // model can still override per-tool with an explicit `platform` arg. The
        // adb serial (for multi-Android) rides along from `list_all_devices`.
        let active_device = window_tab_data.panel.active_device;
        Effect::new(move |_| {
            active_device.set(running_android.get().or_else(|| running_ios.get()));
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
                    Stack::horizontal((
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
                            s.apply_if(!android_panel_visible.get(), |s| s.hide())
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
                            s.apply_if(!ios_panel_visible.get(), |s| s.hide())
                        }),
                    ))
                    .style(|s| {
                        s.flex_row()
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
