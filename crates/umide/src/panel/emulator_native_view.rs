//! Native GPU emulator view using IOSurface for zero-copy rendering
//! 
//! This view imports a native GPU surface directly into wgpu for maximum performance.

use floem::{
    prelude::*,
    reactive::{create_rw_signal, create_effect},
    views::{container, label, stack, Decorators},
    peniko::Color,
};

#[cfg(target_os = "macos")]
use umide_native::{MacOSSurface, SurfaceFormat};

/// Native emulator view that renders directly from a shared GPU surface
/// 
/// Currently falls back to a placeholder on all platforms while
/// the wgpu texture import is being implemented.
pub fn emulator_native_view(
    device_name: impl Fn() -> String + 'static,
    is_running: impl SignalGet<bool> + Copy + 'static,
) -> impl View {
    let status = create_rw_signal("Initializing native surface...".to_string());
    
    #[cfg(target_os = "macos")]
    {
        // Suppress unused variable warnings during development
        let _ = (MacOSSurface::new, SurfaceFormat::Bgra8);
        
        // Create effect to initialize native surface when device starts
        create_effect(move |_| {
            if is_running.get() {
                status.set("Native GPU surface active".to_string());
                
                // TODO: The actual GPU surface integration requires:
                // 1. Get the emulator's window ID (for iOS) or gRPC framebuffer (for Android)
                // 2. Create/import the IOSurface
                // 3. Import as wgpu texture using HAL
                // 4. Render texture in this view
                //
                // For now this is a placeholder showing the architecture is in place
            } else {
                status.set("Waiting for device...".to_string());
            }
        });
    }
    
    #[cfg(not(target_os = "macos"))]
    {
        status.set("Native GPU not available on this platform".to_string());
    }
    
    stack((
        // Main surface area (will be replaced by actual GPU texture)
        container(
            label(move || status.get())
                .style(|s| s.color(Color::from_rgb8(180, 180, 180)))
        )
        .style(|s| {
            s.width_full()
                .height_full()
                .background(Color::from_rgb8(25, 25, 35))
                .items_center()
                .justify_center()
        }),
        
        // Device name overlay
        container(
            label(move || device_name())
                .style(|s| s.color(Color::from_rgb8(120, 120, 120)).font_size(10.0))
        )
        .style(|s| {
            s.absolute()
                .padding(8.0)
                .margin_left(0.0)
                .margin_bottom(0.0)
        }),
    ))
    .style(|s| s.width_full().height_full().min_width(200.0).min_height(400.0))
}

/// Utility to find iOS Simulator window ID by device UDID
#[cfg(target_os = "macos")]
pub fn find_simulator_window_id(_device_udid: &str) -> Option<u32> {
    // TODO: Use CoreGraphics CGWindowListCopyWindowInfo to find Simulator windows
    tracing::info!("Looking for simulator window for device: {}", _device_udid);
    None
}

/// Utility to get Android emulator gRPC endpoint
pub fn get_android_grpc_endpoint(_avd_name: &str) -> String {
    "localhost:5556".to_string()
}

