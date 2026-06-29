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

use floem::prelude::*;
use floem::reactive::RwSignal;
use floem::views::{RgbaFrame, video_frame};
use umide_app::panel::emulator_stream::start_emulator_stream;
use umide_emulator::decoder::DecodedFrame;

fn app() -> impl IntoView {
    let frame_signal: RwSignal<Option<Arc<DecodedFrame>>> = RwSignal::new(None);

    // Start streaming from the running emulator's gRPC endpoint.
    start_emulator_stream("http://localhost:8554".to_string(), frame_signal);

    video_frame(move || {
        frame_signal.get().and_then(|f| {
            f.to_rgba().map(|rgba| RgbaFrame {
                data: Arc::new(rgba),
                width: f.width,
                height: f.height,
            })
        })
    })
    .style(|s| s.size_full())
}

fn main() {
    floem::launch(app);
}
