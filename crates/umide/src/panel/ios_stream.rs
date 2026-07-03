//! Frame stream for the EMBEDDED iOS simulator (macOS only) — the iOS twin of
//! `emulator_stream.rs`. It boots the simulator headless (framebuffer attached,
//! no Simulator.app), spawns an `idb_companion` gRPC server for it, then polls
//! the `screenshot` RPC at a target fps, decodes each PNG to a `DecodedFrame`,
//! and pushes it into `frame_signal` on the UI thread (floem's channel bridge)
//! — exactly the shape the `VideoFrame` panel view already renders.
//!
//! Why polling rather than an H.264 stream: on a `simctl`-booted device nothing
//! renders, and even with `idb_companion --boot` (which does attach a
//! framebuffer) the companion's `video_stream` H.264 path does not emit frames
//! at v1.1.8 — whereas the on-demand `screenshot` RPC reliably returns the
//! current frame. See `umide_emulator::ios_grpc` for the full recipe. A future
//! optimization can swap in `video_stream` behind this same signal.

use std::sync::mpsc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use floem::ext_event::update_signal_from_channel;
use floem::reactive::RwSignal;
use umide_emulator::decoder::DecodedFrame;
use umide_emulator::ios_grpc;

/// Target capture cadence. NB: the `screenshot` RPC itself costs ~450ms per
/// call at full device resolution (PNG encode + ~3 MB transfer), measured live
/// on an iPhone 16 — so the *effective* rate is ~2 fps regardless of this
/// value; the loop self-paces to whatever the RPC allows. That's a usable
/// "see your app" PREVIEW (navigate with ~0.5s lag), not smooth animation.
/// Getting `video_stream` H.264 working is the future rate optimization.
const TARGET_FPS: u64 = 8;

/// Handle to a running iOS stream. Dropping it (or calling [`IosStreamHandle::
/// stop`]) ends the poll loop and kills the companion, leaving the simulator
/// booted (stopping the *device* is a separate, explicit action).
pub struct IosStreamHandle {
    stop: Arc<AtomicBool>,
}

impl IosStreamHandle {
    /// Signal the poll loop to end; idempotent.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for IosStreamHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Whether the embedded iOS path is usable on this machine (the companion
/// binary is installed). The panel uses this to choose between embedding and
/// the Simulator.app fallback.
pub fn embedded_ios_available() -> bool {
    ios_grpc::IosCompanion::available()
}

/// Boot `udid` headless, serve it over gRPC, and poll frames into
/// `frame_signal`. `native_size` receives the device's native pixel size once
/// (first frame), so pointer input can map to native coordinates like the
/// Android path. Returns `None` if the companion isn't installed; the caller
/// falls back to Simulator.app.
///
/// Spawns a background thread and returns immediately. The `status` signal
/// carries a short human string for the panel's connecting/April overlay.
pub fn start_ios_stream(
    udid: String,
    frame_signal: RwSignal<Option<Arc<DecodedFrame>>>,
    native_size: RwSignal<Option<(u32, u32)>>,
    status: RwSignal<Option<String>>,
) -> Option<IosStreamHandle> {
    if !embedded_ios_available() {
        return None;
    }

    let (tx, rx) = mpsc::channel::<Arc<DecodedFrame>>();
    update_signal_from_channel(frame_signal.write_only(), rx);
    let (ntx, nrx) = mpsc::channel::<(u32, u32)>();
    update_signal_from_channel(native_size.write_only(), nrx);
    let (stx, srx) = mpsc::channel::<String>();
    update_signal_from_channel(status.write_only(), srx);

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();

    std::thread::Builder::new()
        .name("umide-ios-stream".into())
        .spawn(move || {
            let _ = stx.send("Booting simulator (headless)…".to_string());
            // Attach the framebuffer without opening Simulator.app.
            if let Err(e) = ios_grpc::IosCompanion::boot_headless(&udid) {
                let _ = stx.send(format!("Boot failed: {e}"));
                tracing::error!("ios boot_headless failed: {e}");
                return;
            }

            let _ = stx.send("Connecting…".to_string());
            let companion = match ios_grpc::IosCompanion::spawn(&udid) {
                Ok(c) => c,
                Err(e) => {
                    let _ = stx.send(format!("Companion failed: {e}"));
                    tracing::error!("ios companion spawn failed: {e}");
                    return;
                }
            };

            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("ios stream: runtime failed: {e}");
                    return;
                }
            };
            let port = companion.grpc_port;
            rt.block_on(async move {
                let mut client = match ios_grpc::connect(port).await {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = stx.send(format!("gRPC failed: {e}"));
                        tracing::error!("ios gRPC connect failed: {e}");
                        return;
                    }
                };

                let interval = Duration::from_millis(1000 / TARGET_FPS.max(1));
                let mut decoder_for_size_probe = true;
                let mut announced = false;
                while !stop_thread.load(Ordering::SeqCst) {
                    let tick = Instant::now();
                    match ios_grpc::screenshot(&mut client).await {
                        Ok((bytes, _fmt)) => {
                            match decode_png_frame(&bytes) {
                                Some(frame) => {
                                    if decoder_for_size_probe {
                                        let _ =
                                            ntx.send((frame.width, frame.height));
                                        decoder_for_size_probe = false;
                                    }
                                    if !announced {
                                        let _ = stx.send(String::new());
                                        announced = true;
                                    }
                                    if tx.send(Arc::new(frame)).is_err() {
                                        break; // UI receiver dropped
                                    }
                                }
                                None => tracing::warn!(
                                    "ios frame decode failed ({} bytes)",
                                    bytes.len()
                                ),
                            }
                        }
                        Err(e) => {
                            // A transient error (e.g. mid-reboot) shouldn't kill
                            // the panel; log and keep polling.
                            tracing::warn!("ios screenshot rpc: {e}");
                            let _ = stx.send("Reconnecting…".to_string());
                        }
                    }
                    // Keep a steady cadence: sleep the remainder of the interval.
                    if let Some(rem) = interval.checked_sub(tick.elapsed()) {
                        tokio::time::sleep(rem).await;
                    }
                }
                // `companion` drops here → the child idb_companion is killed.
                drop(companion);
            });
        })
        .ok()?;

    Some(IosStreamHandle { stop })
}

/// Decode a companion `screenshot` PNG payload into a software `DecodedFrame`
/// (RGBA) ready for the `VideoFrame` view.
fn decode_png_frame(bytes: &[u8]) -> Option<DecodedFrame> {
    use umide_emulator::decoder::GpuFrame;
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Some(DecodedFrame {
        width: w,
        height: h,
        frame: GpuFrame::Software(Arc::new(rgba.into_raw()), w, h),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_png_into_an_rgba_frame() {
        // A 2x1 PNG (red, green) round-trips to a software RGBA frame.
        let mut buf = std::io::Cursor::new(Vec::new());
        let img =
            image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255])
                .unwrap();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let frame = decode_png_frame(buf.get_ref()).expect("decode");
        assert_eq!((frame.width, frame.height), (2, 1));
    }
}
