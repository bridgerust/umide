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

/// Cap on the streamed frame's longer edge, in device pixels. The panel is a
/// narrow side dock, so streaming the full native resolution (e.g. 1080×2424 =
/// ~10 MB/frame uncompressed) wastes gRPC bandwidth, decode, and GPU upload.
/// The emulator scales server-side; touch input still maps to native pixels.
const MAX_STREAM_DIM: u32 = 1280;

/// Downscale `(w, h)` so its longer edge is at most `max_dim`, preserving the
/// aspect ratio. Returns the input unchanged when it already fits or is unknown.
fn downscaled(w: u32, h: u32, max_dim: u32) -> (u32, u32) {
    let long = w.max(h);
    if long == 0 || long <= max_dim {
        return (w, h);
    }
    let scale = max_dim as f64 / long as f64;
    (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    )
}

/// Start streaming frames from the emulator at `endpoint`
/// (e.g. `http://localhost:8554`) into `frame_signal`.
///
/// `native_size` is populated with the device's native resolution (probed
/// once), so pointer input can map to native device pixels even though the
/// stream itself is downscaled — the emulator's touch input is in native
/// coordinates, independent of the screenshot resolution.
///
/// Spawns background threads and returns immediately. floem marshals each
/// frame onto the UI thread before updating the signal, so this is safe to
/// call from view code. The stream ends when the gRPC connection closes or the
/// UI side (the channel receiver) is dropped.
pub fn start_emulator_stream(
    endpoint: String,
    frame_signal: RwSignal<Option<Arc<DecodedFrame>>>,
    native_size: RwSignal<Option<(u32, u32)>>,
) {
    // floem owns a reader thread on `rx` and applies each item to the signal
    // on the UI thread; we feed `tx` from the gRPC streaming thread below.
    let (tx, rx) = mpsc::channel::<Arc<DecodedFrame>>();
    update_signal_from_channel(frame_signal.write_only(), rx);
    // Same UI-thread bridge for the one-shot native-resolution probe.
    let (ntx, nrx) = mpsc::channel::<(u32, u32)>();
    update_signal_from_channel(native_size.write_only(), nrx);

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("emulator stream: failed to start runtime: {e}");
                return;
            }
        };

        rt.block_on(async move {
            // Native resolution + the downscale size derived from it, probed
            // once and reused across reconnects.
            let mut stream_size: Option<(u32, u32)> = None;

            // Reconnect loop. The emulator emits a frame only when the screen
            // *changes*, so an idle screen legitimately produces none — which is
            // indistinguishable from a dead socket. Rather than treat a stall as
            // fatal (which froze the panel until a manual Stop→Start), drop the
            // stalled stream and reconnect; a fresh stream delivers the current
            // frame immediately, so the panel recovers on its own.
            loop {
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

                // Probe native resolution once (first connect), for input
                // mapping, and derive the downscaled stream size so the aspect
                // ratio is exact regardless of how the emulator scales.
                if stream_size.is_none() {
                    stream_size = Some(match client.get_screenshot().await {
                        Ok(frame) => {
                            let _ = ntx.send((frame.width, frame.height));
                            downscaled(frame.width, frame.height, MAX_STREAM_DIM)
                        }
                        Err(e) => {
                            tracing::warn!(
                                "native-size probe failed: {e}; streaming native"
                            );
                            (0, 0) // 0,0 = native
                        }
                    });
                }
                let (sw, sh) = stream_size.unwrap();

                // The gRPC client streams decoded frames over a bounded channel;
                // a full channel naturally drops older frames (latest-wins).
                let (gtx, mut grx) = tokio::sync::mpsc::channel::<DecodedFrame>(2);
                let stream_task = tokio::spawn(async move {
                    if let Err(e) = client.stream_screenshots(gtx, sw, sh).await {
                        tracing::debug!("emulator frame stream ended: {e}");
                    }
                });

                let mut ui_gone = false;
                loop {
                    match tokio::time::timeout(Duration::from_secs(20), grx.recv())
                        .await
                    {
                        Ok(Some(frame)) => {
                            if tx.send(Arc::new(frame)).is_err() {
                                ui_gone = true; // panel dropped
                                break;
                            }
                        }
                        Ok(None) => break, // stream ended — reconnect
                        Err(_) => {
                            tracing::debug!(
                                "emulator stream: no frame for 20s, reconnecting"
                            );
                            break; // stall or idle — reconnect
                        }
                    }
                }
                stream_task.abort();
                if ui_gone {
                    return; // UI side gone — stop for good
                }
            }
        });
    });
}

/// A default screenshot destination: the user's Pictures folder (or home, or
/// the temp dir), with a unique timestamped name.
pub fn default_screenshot_path() -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from);
    let dir = home
        .as_ref()
        .map(|h| h.join("Pictures"))
        .filter(|p| p.is_dir())
        .or_else(|| home.clone())
        .unwrap_or_else(std::env::temp_dir);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    dir.join(format!("umide-emulator-{ts}.png"))
}

/// Capture one native-resolution screenshot from the emulator at `endpoint`,
/// save it as a PNG to `out`, and reveal it in the file manager. Runs on a
/// background thread and returns immediately (a fresh short-lived gRPC
/// connection, so it's independent of the live stream).
pub fn capture_screenshot(endpoint: String, out: std::path::PathBuf) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("screenshot: failed to start runtime: {e}");
                return;
            }
        };
        rt.block_on(async move {
            let mut client = match EmulatorGrpcClient::connect_with_retry(
                &endpoint,
                Duration::from_secs(10),
            )
            .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("screenshot gRPC connect failed: {e}");
                    return;
                }
            };
            let frame = match client.get_screenshot().await {
                Ok(f) => f,
                Err(e) => {
                    tracing::error!("screenshot: get_screenshot failed: {e}");
                    return;
                }
            };
            let Some(png) = frame.to_png() else {
                tracing::error!("screenshot: PNG encode failed");
                return;
            };
            if let Err(e) = std::fs::write(&out, &png) {
                tracing::error!("screenshot: write {}: {e}", out.display());
                return;
            }
            tracing::info!("saved emulator screenshot: {}", out.display());
            reveal_in_file_manager(&out);
        });
    });
}

/// Open the platform file manager with `path` selected, so the user can find the
/// screenshot just saved.
fn reveal_in_file_manager(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(dir) = path.parent() {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    }
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
/// Backed by a bounded channel: events are dropped (latest-wins) if the command
/// client can't keep up or hasn't connected yet, so input can't grow unbounded.
#[derive(Clone)]
pub struct EmulatorInput {
    tx: tokio::sync::mpsc::Sender<InputEvent>,
}

impl EmulatorInput {
    pub fn touch_down(&self, x: i32, y: i32) {
        let _ = self.tx.try_send(InputEvent::TouchDown(x, y));
    }
    pub fn touch_move(&self, x: i32, y: i32) {
        let _ = self.tx.try_send(InputEvent::TouchMove(x, y));
    }
    pub fn touch_up(&self, x: i32, y: i32) {
        let _ = self.tx.try_send(InputEvent::TouchUp(x, y));
    }
    pub fn key(&self, key: &str) {
        let _ = self.tx.try_send(InputEvent::Key(key.to_string()));
    }
    pub fn key_code(&self, code: i32) {
        let _ = self.tx.try_send(InputEvent::KeyCode(code));
    }
}

/// Open a gRPC command connection to the emulator at `endpoint` for sending
/// input. Returns immediately; events sent via the returned handle are
/// forwarded on a background thread.
pub fn start_emulator_input(endpoint: String) -> EmulatorInput {
    // Bounded: drop input on full/disconnected (latest-wins) rather than
    // queueing unboundedly during the connect window.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<InputEvent>(256);

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("emulator input: failed to start runtime: {e}");
                return;
            }
        };
        rt.block_on(async move {
            // Connect eagerly so the command channel is warm before the first
            // tap; a failure here is non-fatal — the loop reconnects on demand.
            let mut client = EmulatorGrpcClient::connect_with_retry(
                &endpoint,
                Duration::from_secs(30),
            )
            .await
            .map_err(|e| tracing::error!("emulator input gRPC connect failed: {e}"))
            .ok();

            while let Some(ev) = rx.recv().await {
                // (Re)connect if we have no live client — either the initial
                // connect failed or a prior send dropped it. This mirrors the
                // frame stream, which reconnects on a stalled/ended stream
                // rather than giving up: without it, a single dropped input
                // connection silently kills taps/keys forever while frames
                // keep painting (looks like "input stopped working").
                if client.is_none() {
                    match EmulatorGrpcClient::connect_with_retry(
                        &endpoint,
                        Duration::from_secs(30),
                    )
                    .await
                    {
                        Ok(c) => client = Some(c),
                        Err(e) => {
                            tracing::warn!(
                                "emulator input: reconnect failed, dropping event: {e}"
                            );
                            continue;
                        }
                    }
                }

                if let Err(e) =
                    send_input_event(client.as_mut().unwrap(), &ev).await
                {
                    // The connection likely dropped mid-gesture. Reconnect once
                    // and retry this same event so a lone transient blip doesn't
                    // break the gesture; if the retry also fails, clear the
                    // client so the next event forces a fresh connect.
                    tracing::warn!("emulator input send failed, reconnecting: {e}");
                    match EmulatorGrpcClient::connect_with_retry(
                        &endpoint,
                        Duration::from_secs(30),
                    )
                    .await
                    {
                        Ok(mut c2) => {
                            if let Err(e2) =
                                send_input_event(&mut c2, &ev).await
                            {
                                tracing::warn!(
                                    "emulator input retry after reconnect failed: {e2}"
                                );
                            }
                            client = Some(c2);
                        }
                        Err(e2) => {
                            tracing::warn!(
                                "emulator input reconnect failed: {e2}"
                            );
                            client = None;
                        }
                    }
                }
            }
        });
    });

    EmulatorInput { tx }
}

/// Dispatch one input event on an open command client. Errors are stringified
/// so callers can log/retry without naming the client's error type.
async fn send_input_event(
    client: &mut EmulatorGrpcClient,
    ev: &InputEvent,
) -> Result<(), String> {
    let r = match ev {
        InputEvent::TouchDown(x, y) => client.send_touch_down(*x, *y).await,
        InputEvent::TouchMove(x, y) => client.send_touch_move(*x, *y).await,
        InputEvent::TouchUp(x, y) => client.send_touch_up(*x, *y).await,
        InputEvent::Key(k) => client.send_key(k).await,
        InputEvent::KeyCode(c) => client.send_key_code(*c).await,
    };
    r.map_err(|e| e.to_string())
}

/// Map a pointer position in the view's local coordinates to a device pixel
/// coordinate, accounting for the aspect-preserving letterbox. Returns `None`
/// if the point falls in the letterbox margins (outside the displayed frame).
///
/// NOTE: this letterbox math must stay in lockstep with floem's
/// `draw_external_texture` centering (vger/src/lib.rs); if they diverge, taps
/// land on the wrong device pixel with no error.
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
