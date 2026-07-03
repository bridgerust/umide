//! Headless exercise of the embedded-iOS FRAME LOOP (I2 core): boot the sim
//! headless via idb_companion, then poll the `screenshot` RPC at a target fps
//! and decode each frame — proving a sustained live stream (not just one grab),
//! independent of any window/GPU. Saves the last frame. Usage:
//!     cargo run -p umide_emulator --example ios_poll_stream -- [udid] [n] [out.png]

use std::time::{Duration, Instant};

use umide_emulator::ios_grpc;

#[tokio::main]
async fn main() {
    let udid = std::env::args().nth(1).unwrap_or_else(|| {
        umide_emulator::IosSimulator::get_booted_udids()
            .into_iter()
            .next()
            .expect("no booted simulator")
    });
    let n: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let out = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "/tmp/umide_ios_stream.png".to_string());

    eprintln!("boot headless: {udid}");
    ios_grpc::IosCompanion::boot_headless(&udid).expect("boot_headless");
    let companion = ios_grpc::IosCompanion::spawn(&udid).expect("companion");
    let mut client = ios_grpc::connect(companion.grpc_port)
        .await
        .expect("connect");

    let interval = Duration::from_millis(125); // ~8 fps
    let mut ok = 0usize;
    let mut last_size = (0u32, 0u32);
    let t0 = Instant::now();
    for i in 0..n {
        let tick = Instant::now();
        let (bytes, _fmt) =
            ios_grpc::screenshot(&mut client).await.expect("screenshot");
        let img = image::load_from_memory(&bytes).expect("decode");
        last_size = (img.width(), img.height());
        ok += 1;
        eprintln!(
            "frame {:>2}/{n}: {}x{}  {} bytes  {:>3}ms",
            i + 1,
            img.width(),
            img.height(),
            bytes.len(),
            tick.elapsed().as_millis()
        );
        if i + 1 == n {
            img.save(&out).expect("save");
        }
        if let Some(rem) = interval.checked_sub(tick.elapsed()) {
            tokio::time::sleep(rem).await;
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    eprintln!(
        "STREAM ok: {ok}/{n} frames, {}x{}, {:.1} fps effective → {out}",
        last_size.0,
        last_size.1,
        ok as f64 / secs
    );
    println!("ok: {ok} frames {}x{}", last_size.0, last_size.1);
}
