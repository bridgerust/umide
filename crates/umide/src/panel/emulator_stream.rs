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
