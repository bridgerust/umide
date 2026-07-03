//! Grab a live frame off a HEADLESS iOS simulator — no Simulator.app — and
//! save a PNG: the milestone proof of the embedded-iOS acquisition path
//! (idb_companion gRPC → frame → RGBA), the iOS twin of the Android
//! `grab_frame` example. Tries the H.264 `video_stream` first (the low-latency
//! path); falls back to the on-demand `screenshot` RPC (always yields a frame).
//! Diagnostics go to stderr (unbuffered). Usage:
//!     xcrun simctl boot <udid>          # headless; do NOT open Simulator.app
//!     cargo run -p umide_emulator --example ios_grab_frame -- [udid] [out.png]

use std::time::{Duration, Instant};

use umide_emulator::ios_grpc;

#[tokio::main]
async fn main() {
    let udid = std::env::args().nth(1).unwrap_or_else(|| {
        umide_emulator::IosSimulator::get_booted_udids()
            .into_iter()
            .next()
            .expect("no booted simulator — `xcrun simctl boot <udid>` first")
    });
    let out = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "/tmp/umide_ios_frame.png".to_string());

    eprintln!("target simulator: {udid}");
    assert!(
        ios_grpc::IosCompanion::available(),
        "idb_companion not found — brew install idb-companion"
    );

    let companion =
        ios_grpc::IosCompanion::spawn(&udid).expect("spawn idb_companion");
    eprintln!("companion gRPC port: {}", companion.grpc_port);

    let mut client = ios_grpc::connect(companion.grpc_port)
        .await
        .expect("connect to companion gRPC");
    eprintln!("connected to companion gRPC");

    // Probe: a unary RPC first, to isolate connect/gRPC health from the bidi
    // video stream.
    match tokio::time::timeout(
        Duration::from_secs(8),
        ios_grpc::screenshot(&mut client),
    )
    .await
    {
        Ok(Ok((bytes, fmt))) => {
            eprintln!("PROBE screenshot ok: {} bytes, fmt={fmt:?}", bytes.len());
            // Save whatever the companion returned so we can identify it.
            std::fs::write(format!("{out}.raw"), &bytes).ok();
            match image::load_from_memory(&bytes) {
                Ok(img) => {
                    eprintln!(
                        "  decoded as image: {}x{}",
                        img.width(),
                        img.height()
                    );
                    img.save(&out).expect("save png");
                    eprintln!("SCREENSHOT ok → {out}");
                    println!("ok: screenshot {}x{}", img.width(), img.height());
                    return;
                }
                Err(e) => eprintln!("  not a standard image: {e}"),
            }
        }
        Ok(Err(e)) => eprintln!("PROBE screenshot RPC error: {e}"),
        Err(_) => eprintln!("PROBE screenshot TIMED OUT (unary rpc hung)"),
    }

    // --- Path A: H.264 video_stream (low-latency; may be quiet on a static
    // screen). Give it a short window to produce a decodable frame.
    if let Some((w, h)) = try_video_stream(&mut client, &out).await {
        eprintln!("VIDEO_STREAM ok: decoded {w}x{h}, saved {out}");
        println!("ok: video_stream {w}x{h}");
        return;
    }
    eprintln!("video_stream produced no frame in the window; trying screenshot RPC");

    // --- Path B: screenshot RPC (always yields a frame on demand).
    let (bytes, fmt) = ios_grpc::screenshot(&mut client)
        .await
        .expect("screenshot RPC");
    eprintln!("screenshot RPC: {} bytes, format={fmt}", bytes.len());
    // The companion returns an encoded image (png); decode to read the size,
    // then save verbatim.
    let img = image::load_from_memory(&bytes).expect("decode screenshot image");
    let (w, h) = (img.width(), img.height());
    std::fs::write(&out, &bytes).expect("write image");
    eprintln!("SCREENSHOT ok: {w}x{h}, saved {out}");
    println!("ok: screenshot {w}x{h} ({fmt})");
}

/// Try to decode one frame off the H.264 stream within a short window.
async fn try_video_stream(
    client: &mut ios_grpc::CompanionClient,
    out: &str,
) -> Option<(u32, u32)> {
    let mut stream = match ios_grpc::video_stream_h264(client, 30).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("video_stream start failed: {e}");
            return None;
        }
    };
    eprintln!("H.264 stream started");
    let mut decoder = umide_emulator::create_decoder();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut chunks = 0usize;
    let mut bytes = 0usize;
    while Instant::now() < deadline {
        let next = tokio::time::timeout(
            Duration::from_secs(3),
            tonic::codec::Streaming::message(&mut stream),
        )
        .await;
        let msg = match next {
            Ok(Ok(Some(m))) => m,
            Ok(Ok(None)) => {
                eprintln!("video stream ended");
                break;
            }
            Ok(Err(e)) => {
                eprintln!("video stream error: {e}");
                break;
            }
            Err(_) => {
                eprintln!("video stream quiet ({chunks} chunks so far)");
                continue;
            }
        };
        let Some(data) = ios_grpc::payload_bytes(&msg) else {
            continue;
        };
        chunks += 1;
        bytes += data.len();
        if let Ok(frames) = decoder.decode_frame(data) {
            if let Some(frame) = frames.into_iter().next() {
                if let Some(png) = frame.to_png() {
                    std::fs::write(out, &png).expect("write PNG");
                    eprintln!("decoded after {chunks} chunks / {bytes} H.264 bytes");
                    return Some((frame.width, frame.height));
                }
            }
        }
    }
    eprintln!("no decodable frame ({chunks} chunks / {bytes} bytes)");
    None
}
