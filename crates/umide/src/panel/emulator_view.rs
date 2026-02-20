use std::{rc::Rc, sync::Arc};
use floem::{
    View, ViewId, prelude::{SignalGet, SignalUpdate}, 
    reactive::{RwSignal, Effect},
    views::{Decorators, Label, Scroll, Stack, dyn_stack, Container},
    context::{UpdateCx, ComputeLayoutCx},
    peniko::kurbo::Rect,
};

use crate::{
    app::clickable_icon,
    panel::{position::PanelPosition, view::PanelBuilder},
    window_tab::WindowTabData,
    config::{icon::UmideIcons, color::UmideColor},
};
use umide_emulator::{
    list_all_devices, launch_device, stop_device,
    DeviceInfo, DevicePlatform, DeviceState,
    native_view::NativeEmulatorView,
    decoder::DecodedFrame,
};
use umide_native::emulator::EmulatorPlatform;

struct NativeEmulatorWidget {
    id: ViewId,
    native_view: Option<NativeEmulatorView>,
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
            native_view: None,
            running_device,
            is_visible,
            current_device_id,
            last_device_id: None,
            frame_signal,
        }
    }
    
    /// Cleanup the native view
    fn cleanup(&mut self) {
        if self.native_view.is_some() {
            tracing::info!("Cleaning up native emulator view");
            self.native_view = None;
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

    fn update(&mut self, _cx: &mut UpdateCx, _state: Box<dyn std::any::Any>) {
        let current_device = self.running_device.get_untracked();
        let is_visible = self.is_visible.get_untracked();
        
        if !is_visible && self.native_view.is_some() {
            tracing::info!("Hiding emulator view, cleaning up native view");
            self.native_view = None;
        }
        
        match (&current_device, &self.last_device_id) {
            (None, Some(_)) => {
                self.cleanup();
            }
            (Some(dev), Some(last_id)) if &dev.id != last_id => {
                self.cleanup();
            }
            _ => {}
        }
    }

    fn compute_layout(&mut self, _cx: &mut ComputeLayoutCx) -> Option<Rect> {
        let is_visible = self.is_visible.get_untracked();
        
        if !is_visible {
            if self.native_view.is_some() {
                self.cleanup();
            }
            return None;
        }
        
        let current_device = self.running_device.get_untracked();
        if current_device.is_none() && self.native_view.is_some() {
            self.cleanup();
            return None;
        }
        
        None
    }

    fn paint(&mut self, _cx: &mut floem::context::PaintCx) {
        let is_visible = self.is_visible.get_untracked();
        let current_device = self.running_device.get_untracked();

        const HEADER_HEIGHT: i32 = 35;
        let window_origin = self.id.get_window_origin();
        let size = self.id.get_layout().map(|l| (l.size.width as u32, l.size.height as u32));
        
        if let Some((width, height)) = size {
            if width == 0 || height == 0 {
                return;
            }
            
            let x = window_origin.x as i32;
            let y = window_origin.y as i32 + HEADER_HEIGHT;
            let device_name = current_device.as_ref().map(|d| d.name.as_str()).unwrap_or("unknown");
            
            tracing::debug!(
                "NativeEmulatorWidget [{}]: origin=({},{}) adjusted_y={} size={}x{}",
                device_name, window_origin.x as i32, window_origin.y as i32, y, width, height
            );

            if let Some(view) = &self.native_view {
                view.resize(x, y, width, height);
            } else if is_visible {
                if let Some(device) = current_device {
                    use floem::window::WindowIdExt;
                    
                    if let Some(window_id) = self.id.window_id() {
                        if let Some(handle) = window_id.raw_window_handle() {
                            let platform = match device.platform {
                                umide_emulator::common::DevicePlatform::Android => EmulatorPlatform::Android,
                                umide_emulator::common::DevicePlatform::Ios => EmulatorPlatform::Ios,
                            };
                            
                            tracing::info!(
                                "Creating native emulator view for device: {} at ({},{}) size {}x{}", 
                                device.name, x, y, width, height
                            );
                            
                            match NativeEmulatorView::new(
                                handle, 
                                x, 
                                y, 
                                width, 
                                height, 
                                platform
                            ) {
                                Ok(view) => {
                                    if !device.id.is_empty() {
                                        view.attach_device(&device.id);
                                    }
                                    
                                    // Android: start gRPC frame streaming (headless, no window)
                                    // iOS: uses ScreenCaptureKit via attach_device (no gRPC needed)
                                    if view.is_android() {
                                        view.start_grpc_stream("http://localhost:8554");
                                    }
                                    
                                    self.native_view = Some(view);
                                    self.last_device_id = Some(device.id.clone());
                                }
                                Err(e) => {
                                    tracing::error!("Failed to create native emulator view: {}", e);
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
                                is_visible.set(true);
                                return;
                            }
                        }
                        // Start new device
                        let _ = launch_device(&device_cloned_start);
                        let mut d = device_cloned_start.clone();
                        d.state = DeviceState::Running;
                        running_device.set(Some(d));
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
                Label::new({
                    if is_starting { "Starting..." } else { "" }
                }).style(|s| s.padding_horiz(5.0).font_size(10.0)),
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
        // Header with device name if running
        Container::new(
            Stack::horizontal((
                Label::new(platform_name.to_string())
                    .style(move |s| {
                        s.font_size(12.0)
                            .font_bold()
                            .padding(6.0)
                    }),
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
                .style(|s| s.font_size(10.0).padding_right(6.0)),
            ))
            .style(|s| s.width_full().items_center())
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
                        
                        let platform_filter = platform.clone();
                        devices.get().into_iter()
                            .filter(|d| d.platform == platform_filter)
                            .collect::<Vec<_>>()
                    },
                    |d| format!("{}-{}", d.id, d.state as u32),
                    device_item
                ).style(|s| s.flex_col().width_full())
            )
            .style(move |s| {
                let visible = is_visible.get();
                let has_running = running_device.get().is_some();
                s.flex_grow(1.0)
                    .width_full()
                    .apply_if(has_running && visible, |s| s.hide())
            }),

            // Native Emulator View (shown when device running AND visible)
            Stack::new((
                NativeEmulatorWidget::new(running_device, is_visible, current_device_id, frame_signal)
                    .style(|s| s.flex_grow(1.0).width_full().height_full()),
                // Control buttons overlay
                Stack::horizontal((
                    // Stop button - actually stops the emulator/simulator
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
                        || "Stop Emulator",
                        config,
                    ),
                    // Hide button - hides without stopping
                    clickable_icon(
                        || UmideIcons::CLOSE,
                        move || {
                            tracing::info!("Hiding emulator view (device still running)");
                            is_visible.set(false);
                        },
                        || false,
                        || false,
                        || "Hide (keep running)",
                        config,
                    ),
                ))
                .style(|s| s.absolute().margin_left(5.0).margin_top(5.0).gap(5.0))
            ))
            .style(move |s| {
                let visible = is_visible.get();
                let has_running = running_device.get().is_some();
                s.flex_col()
                    .width_full()
                    .flex_grow(1.0)
                    .apply_if(!has_running || !visible, |s| s.hide())
            })
        ))
        .style(|s| s.flex_col().flex_grow(1.0).width_full()),
    ))
    .style(move |s| {
        let config = config.get();
        s.flex_col()
            .flex_grow(1.0)
            .flex_basis(0.0)
            .min_width(200.0)
            .min_height(300.0)
            .height_full()
            .border(1.0)
            .border_color(config.color(UmideColor::LAPCE_BORDER))
    })
}

pub fn emulator_panel(
    window_tab_data: Rc<WindowTabData>,
    position: PanelPosition,
) -> impl View {
    let config = window_tab_data.common.config;
    let devices = RwSignal::new(Vec::<DeviceInfo>::new());
    
    // Separate running device signals for each platform
    let running_android = RwSignal::new(None::<DeviceInfo>);
    let running_ios = RwSignal::new(None::<DeviceInfo>);
    
    // Visibility signals (separate from running state for hide/resume)
    let android_visible = RwSignal::new(false);
    let ios_visible = RwSignal::new(false);
    
    // Track current device IDs for capture management
    let current_android_id = RwSignal::new(String::new());
    let current_ios_id = RwSignal::new(String::new());
    
    // Get platform-specific frame signals
    let android_frame = window_tab_data.panel.android_frame;
    let ios_frame = window_tab_data.panel.ios_frame;

    // Effect to fetch devices and update running state
    Effect::new(move |_| {
        let dev_list = list_all_devices();
        
        // Update running devices if any are Running
        for device in &dev_list {
            if device.state == DeviceState::Running {
                match device.platform {
                    DevicePlatform::Android => {
                        if running_android.get().is_none() {
                            running_android.set(Some(device.clone()));
                            // Don't auto-show, let user click "Show"
                            // android_visible.set(true);
                        }
                    }
                    DevicePlatform::Ios => {
                        if running_ios.get().is_none() {
                            running_ios.set(Some(device.clone()));
                            // Don't auto-show, let user click "Show"
                            // ios_visible.set(true);
                        }
                    }
                }
            }
        }
        
        devices.set(dev_list);
    });

    PanelBuilder::new(config, position)
        .add(
            "Emulators",
            Stack::horizontal((
                platform_panel(
                    DevicePlatform::Android,
                    devices,
                    running_android,
                    android_visible,
                    android_frame,
                    current_android_id,
                    config,
                ),
                platform_panel(
                    DevicePlatform::Ios,
                    devices,
                    running_ios,
                    ios_visible,
                    ios_frame,
                    current_ios_id,
                    config,
                ),
            ))
            .style(|s| {
                s.flex_row()
                    .size_full()
                    .gap(5.0)
                    .padding(5.0)
            }),
            window_tab_data.panel.section_open(crate::panel::data::PanelSection::Process),
        )
        .build()
}

