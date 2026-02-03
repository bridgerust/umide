use std::{rc::Rc, sync::Arc};
use floem::{
    View, ViewId, prelude::{SignalGet, SignalUpdate}, reactive::{RwSignal, Effect},
    views::{Decorators, Label, Scroll, Stack, dyn_stack, Container},
    context::{PaintCx, UpdateCx, ComputeLayoutCx},
    peniko::kurbo::Rect,
};

use crate::{
    app::clickable_icon,
    panel::{position::PanelPosition, view::PanelBuilder},
    window_tab::WindowTabData,
    config::{icon::UmideIcons, color::UmideColor},
};
use umide_emulator::{
    list_all_devices, launch_device, stop_device, DeviceInfo, DevicePlatform, DeviceState,
    native_view::NativeEmulatorView,  
};
use umide_native::emulator::EmulatorPlatform;

struct NativeEmulatorWidget {
    id: ViewId,
    native_view: Option<NativeEmulatorView>,
    running_device: RwSignal<Option<DeviceInfo>>,
    #[allow(dead_code)]
    current_device_id: RwSignal<String>,
}

impl NativeEmulatorWidget {
    pub fn new(running_device: RwSignal<Option<DeviceInfo>>, current_device_id: RwSignal<String>) -> Self {
        Self {
            id: ViewId::new(),
            native_view: None,
            running_device,
            current_device_id,
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
        // Handle updates if needed
    }

    fn compute_layout(&mut self, _cx: &mut ComputeLayoutCx) -> Option<Rect> {
        // Get layout after it's been computed
        if let Some(layout) = self.id.get_layout() {
            let width = layout.size.width as u32;
            let height = layout.size.height as u32;
            
            if let Some(view) = &self.native_view {
                view.resize(width, height);
            } else {
                // Initialize the native view if we have a window handle
                use floem::window::WindowIdExt;
                
                if let Some(window_id) = self.id.window_id() {
                    if let Some(handle) = window_id.raw_window_handle() {
                        // Determine the platform from the running device signal
                        if let Some(device) = self.running_device.get_untracked() {
                            let platform = match device.platform {
                                umide_emulator::common::DevicePlatform::Android => EmulatorPlatform::Android,
                                umide_emulator::common::DevicePlatform::Ios => EmulatorPlatform::Ios,
                            };
                            
                            match NativeEmulatorView::new(handle, width, height, platform) {
                                Ok(view) => {
                                    tracing::info!("Successfully created native emulator view for device: {}", device.name);
                                    
                                    // Attach device if ID is available
                                    if !device.id.is_empty() {
                                        view.attach_device(&device.id);
                                    }
                                    
                                    self.native_view = Some(view);
                                }
                                Err(e) => {
                                    tracing::error!("Failed to create native emulator view: {}", e);
                                }
                            }
                        } else {
                            // Should not happen if view is visible only when running, but handle gracefully
                            tracing::warn!("NativeEmulatorWidget layout called but no device is running");
                        }
                    }
                }
            }
        }
        None
    }

    fn paint(&mut self, _cx: &mut PaintCx) {
        // Native view handles painting on its own layer/surface
    }
}

/// Create a single platform panel (Android or iOS)
fn platform_panel(
    platform: DevicePlatform,
    devices: RwSignal<Vec<DeviceInfo>>,
    running_device: RwSignal<Option<DeviceInfo>>,
    frame_signal: RwSignal<Option<Arc<umide_emulator::decoder::DecodedFrame>>>, // Kept for API compat, likely unused
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
            let is_running = device.state == DeviceState::Running;
            let is_starting = device.state == DeviceState::Starting;
            let is_disconnected = device.state == DeviceState::Disconnected;
            
            Stack::new((
                Label::new(device.name.clone())
                    .style(|s| s.flex_grow(1.0).padding_horiz(6.0)),
                clickable_icon(
                    || UmideIcons::DEBUG_CONTINUE,
                    move || {
                        let _ = launch_device(&device_cloned_start);
                        let mut d = device_cloned_start.clone();
                        d.state = DeviceState::Running;
                        running_device.set(Some(d));
                    },
                    || false,
                    move || !is_disconnected,
                    || "Start",
                    config,
                ),
                clickable_icon(
                    || UmideIcons::DEBUG_STOP,
                    move || {
                        let _ = stop_device(&device_cloned_stop);
                        running_device.set(None);
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
            })
        }
    };
    
    Stack::new((
        // Header
        Container::new(
            Label::new(platform_name.to_string())
                .style(move |s| {
                    s.font_size(12.0)
                        .font_bold()
                        .padding(6.0)
                })
        )
        .style(move |s| {
            let config = config.get();
            s.width_full()
                .border_bottom(1.0)
                .border_color(config.color(UmideColor::LAPCE_BORDER))
        }),
        
        // Content: Device list
        Stack::new((
            Scroll::new(
                dyn_stack(
                    move || {
                        // Only show list if no device running (or can show generic list)
                        // Simplified: Show list always, but maybe disable items? 
                        // logic from before: hide if running.
                        if running_device.get().is_some() {
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
                s.flex_grow(1.0)
                    .width_full()
                    .apply_if(running_device.get().is_some(), |s| s.hide())
            }),

            // Native Emulator View
            Stack::new((
                NativeEmulatorWidget::new(running_device, current_device_id),
                clickable_icon(
                    || UmideIcons::CLOSE,
                    move || {
                        running_device.set(None);
                        frame_signal.set(None);
                        current_device_id.set(String::new());
                    },
                    || false,
                    || false,
                    || "Back to list",
                    config,
                ).style(|s| s.absolute().margin_left(5.0).margin_top(5.0))
            ))
            .style(move |s| {
                  s.flex_col()
                    .width_full()
                    .flex_grow(1.0)
                    .min_height(300.0)
                    .apply_if(running_device.get().is_none(), |s| s.hide())
            })
        ))
        .style(|s| s.flex_col().flex_grow(1.0).width_full()),
    ))
    .style(move |s| {
        let config = config.get();
        s.flex_col()
            .flex_grow(1.0)
            .min_width(180.0)
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
    
    // Track current device IDs for capture management
    let current_android_id = RwSignal::new(String::new());
    let current_ios_id = RwSignal::new(String::new());
    
    // Get platform-specific frame signals
    let android_frame = window_tab_data.panel.android_frame;
    let ios_frame = window_tab_data.panel.ios_frame;

    // Effect to fetch devices
    Effect::new(move |_| {
        let dev_list = list_all_devices();
        
        // Update running devices if any are Running
        for device in &dev_list {
            if device.state == DeviceState::Running {
                match device.platform {
                    DevicePlatform::Android => {
                        if running_android.get().is_none() {
                            running_android.set(Some(device.clone()));
                        }
                    }
                    DevicePlatform::Ios => {
                        if running_ios.get().is_none() {
                            running_ios.set(Some(device.clone()));
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
                    android_frame,
                    current_android_id,
                    config,
                ),
                platform_panel(
                    DevicePlatform::Ios,
                    devices,
                    running_ios,
                    ios_frame,
                    current_ios_id,
                    config,
                ),
            ))
            .style(|s| s.flex_row().size_full().gap(5.0).padding(5.0)),
            window_tab_data.panel.section_open(crate::panel::data::PanelSection::Process),
        )
        .build()
}
