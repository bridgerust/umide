//! Cross-platform Android-emulator frame producer.
//!
//! Connects to the emulator's gRPC endpoint on a background thread, streams
//! decoded frames, and pushes them into `frame_signal` **on the UI thread**
//! via floem's channel bridge (`update_signal_from_channel`). The `VideoFrame`
//! view in the emulator panel reads `frame_signal` and renders each frame on
//! the GPU, bypassing vger's image atlas.
//!
//! This is the portable counterpart to the macOS native-view path: instead of
//! a native NSView + IOSurface, frames flow entirely Rust-side into a wgpu
//! texture, so it works on Windows and Linux too.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use floem::ext_event::update_signal_from_channel;
use floem::reactive::RwSignal;
use umide_emulator::decoder::DecodedFrame;
use umide_emulator::grpc_client::EmulatorGrpcClient;

/// Start streaming frames from the emulator at `endpoint`
/// (e.g. `http://localhost:8554`) into `frame_signal`.
///
/// Spawns background threads and returns immediately. floem marshals each
/// frame onto the UI thread before updating the signal, so this is safe to
/// call from view code. The stream ends when the gRPC connection closes or the
/// UI side (the channel receiver) is dropped.
pub fn start_emulator_stream(
    endpoint: String,
    frame_signal: RwSignal<Option<Arc<DecodedFrame>>>,
) {
    // floem owns a reader thread on `rx` and applies each item to the signal
    // on the UI thread; we feed `tx` from the gRPC streaming thread below.
    let (tx, rx) = mpsc::channel::<Arc<DecodedFrame>>();
    update_signal_from_channel(frame_signal.write_only(), rx);

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("emulator stream: failed to start runtime: {e}");
                return;
            }
        };

        rt.block_on(async move {
            let mut client = match EmulatorGrpcClient::connect_with_retry(
                &endpoint,
                Duration::from_secs(60),
            )
            .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("emulator gRPC connect failed: {e}");
                    return;
                }
            };

            // The gRPC client streams decoded frames over a bounded channel;
            // a full channel naturally drops older frames (latest-frame-wins).
            let (gtx, mut grx) = tokio::sync::mpsc::channel::<DecodedFrame>(2);
            tokio::spawn(async move {
                if let Err(e) = client.stream_screenshots(gtx).await {
                    tracing::error!("emulator frame stream ended: {e}");
                }
            });

            while let Some(frame) = grx.recv().await {
                if tx.send(Arc::new(frame)).is_err() {
                    break; // UI side gone — stop streaming
                }
            }
        });
    });
}

/// An input event to forward to the emulator over gRPC.
enum InputEvent {
    TouchDown(i32, i32),
    TouchMove(i32, i32),
    TouchUp(i32, i32),
    Key(String),
    KeyCode(i32),
}

/// Handle for sending input (touch/keys) to the emulator. Cheap to clone; call
/// from the UI thread — events are dispatched on a background gRPC command
/// connection (separate from the frame stream, to avoid lock contention).
#[derive(Clone)]
pub struct EmulatorInput {
    tx: tokio::sync::mpsc::UnboundedSender<InputEvent>,
}

impl EmulatorInput {
    pub fn touch_down(&self, x: i32, y: i32) {
        let _ = self.tx.send(InputEvent::TouchDown(x, y));
    }
    pub fn touch_move(&self, x: i32, y: i32) {
        let _ = self.tx.send(InputEvent::TouchMove(x, y));
    }
    pub fn touch_up(&self, x: i32, y: i32) {
        let _ = self.tx.send(InputEvent::TouchUp(x, y));
    }
    pub fn key(&self, key: &str) {
        let _ = self.tx.send(InputEvent::Key(key.to_string()));
    }
    pub fn key_code(&self, code: i32) {
        let _ = self.tx.send(InputEvent::KeyCode(code));
    }
}

/// Open a gRPC command connection to the emulator at `endpoint` for sending
/// input. Returns immediately; events sent via the returned handle are
/// forwarded on a background thread.
pub fn start_emulator_input(endpoint: String) -> EmulatorInput {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<InputEvent>();

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("emulator input: failed to start runtime: {e}");
                return;
            }
        };
        rt.block_on(async move {
            let mut client = match EmulatorGrpcClient::connect_with_retry(
                &endpoint,
                Duration::from_secs(30),
            )
            .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("emulator input gRPC connect failed: {e}");
                    return;
                }
            };
            while let Some(ev) = rx.recv().await {
                let r = match ev {
                    InputEvent::TouchDown(x, y) => {
                        client.send_touch_down(x, y).await
                    }
                    InputEvent::TouchMove(x, y) => {
                        client.send_touch_move(x, y).await
                    }
                    InputEvent::TouchUp(x, y) => client.send_touch_up(x, y).await,
                    InputEvent::Key(k) => client.send_key(&k).await,
                    InputEvent::KeyCode(c) => client.send_key_code(c).await,
                };
                if let Err(e) = r {
                    tracing::warn!("emulator input send failed: {e}");
                }
            }
        });
    });

    EmulatorInput { tx }
}

/// Map a pointer position in the view's local coordinates to a device pixel
/// coordinate, accounting for the aspect-preserving letterbox. Returns `None`
/// if the point falls in the letterbox margins (outside the displayed frame).
pub fn view_to_device(
    px: f64,
    py: f64,
    view_w: f64,
    view_h: f64,
    frame_w: u32,
    frame_h: u32,
) -> Option<(i32, i32)> {
    if view_w <= 0.0 || view_h <= 0.0 || frame_w == 0 || frame_h == 0 {
        return None;
    }
    let frame_aspect = frame_w as f64 / frame_h as f64;
    let view_aspect = view_w / view_h;
    let (dw, dh) = if frame_aspect > view_aspect {
        (view_w, view_w / frame_aspect)
    } else {
        (view_h * frame_aspect, view_h)
    };
    let ox = (view_w - dw) / 2.0;
    let oy = (view_h - dh) / 2.0;
    let (lx, ly) = (px - ox, py - oy);
    if lx < 0.0 || ly < 0.0 || lx > dw || ly > dh {
        return None;
    }
    let dx = (lx / dw * frame_w as f64).round() as i32;
    let dy = (ly / dh * frame_h as f64).round() as i32;
    Some((
        dx.clamp(0, frame_w as i32 - 1),
        dy.clamp(0, frame_h as i32 - 1),
    ))
}
