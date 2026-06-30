//! M2 verification — render the LIVE Android emulator via the new GPU path,
//! independent of umide's (macOS-gated) emulator panel.
//!
//! Prereq: an Android emulator running with the gRPC endpoint enabled, e.g.
//!     emulator -avd <name> -grpc 8554
//! (the emulator can be headless with `-no-window`).
//!
//! Run:
//!     cargo run -p umide-app --example live_emulator
//!
//! You should see the live device screen, streamed over gRPC, decoded, and
//! rendered through the atlas-bypassing wgpu `VideoFrame` primitive. This
//! exercises the full M2 pipeline: producer → frame_signal (UI thread) →
//! GPU texture. It runs on macOS today (Metal) and, once the panel is
//! un-gated, the identical path serves Windows (DX12) and Linux (Vulkan).

use std::sync::Arc;

use floem::Application;
use floem::event::{Event, EventListener};
use floem::kurbo::Size;
use floem::prelude::*;
use floem::reactive::RwSignal;
use floem::views::{RgbaFrame, video_frame};
use floem::window::WindowConfig;
use umide_app::panel::emulator_stream::{
    start_emulator_input, start_emulator_stream, view_to_device,
};
use umide_emulator::decoder::DecodedFrame;

const ENDPOINT: &str = "http://localhost:8554";

fn app() -> impl IntoView {
    let frame_signal: RwSignal<Option<Arc<DecodedFrame>>> = RwSignal::new(None);
    let native_size: RwSignal<Option<(u32, u32)>> = RwSignal::new(None);
    start_emulator_stream(ENDPOINT.to_string(), frame_signal, native_size);
    let input = start_emulator_input(ENDPOINT.to_string());

    let view_size = RwSignal::new(Size::ZERO);
    let pressed = RwSignal::new(false);
    let last = RwSignal::new((0i32, 0i32));

    // Pointer position (view-local) -> native device pixel, through the
    // letterbox. Uses native size (not the downscaled stream) so taps land.
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

    video_frame(move || {
        frame_signal.get().and_then(|f| {
            f.to_rgba().map(|rgba| RgbaFrame {
                data: Arc::new(rgba),
                width: f.width,
                height: f.height,
            })
        })
    })
    .on_resize(move |rect| view_size.set(rect.size()))
    .on_event_stop(EventListener::PointerDown, {
        let input = input.clone();
        move |e| {
            if let Some((x, y)) = to_device(e) {
                pressed.set(true);
                last.set((x, y));
                input.touch_down(x, y);
            }
        }
    })
    .on_event_stop(EventListener::PointerMove, {
        let input = input.clone();
        move |e| {
            if pressed.get_untracked() {
                if let Some((x, y)) = to_device(e) {
                    last.set((x, y));
                    input.touch_move(x, y);
                }
            }
        }
    })
    .on_event_stop(EventListener::PointerUp, {
        let input = input.clone();
        move |e| {
            if pressed.get_untracked() {
                pressed.set(false);
                let (x, y) = to_device(e).unwrap_or_else(|| last.get_untracked());
                input.touch_up(x, y);
            }
        }
    })
    .style(|s| s.size_full())
}

fn main() {
    // Portrait window roughly matching a phone's aspect; the primitive
    // letterboxes to the exact frame aspect regardless.
    Application::new()
        .window(
            move |_| app(),
            Some(WindowConfig::default().size(Size::new(380.0, 820.0))),
        )
        .run();
}
