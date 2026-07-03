//! Empirical I3 check on a HEADLESS simulator: which coordinate space does the
//! idb `hid` touch use — screen POINTS or pixels? Taps a dock-icon location
//! expressed in each space and screenshots after each, so the changed frame
//! tells us. Also exercises hid_swipe + hid_key end-to-end. Usage:
//!     cargo run -p umide_emulator --example ios_tap_test -- [udid]

use std::time::Duration;

use umide_emulator::ios_grpc;

async fn shot(
    client: &mut ios_grpc::CompanionClient,
    path: &str,
) -> (u32, u32, u64) {
    let (bytes, _) = ios_grpc::screenshot(client).await.expect("screenshot");
    let img = image::load_from_memory(&bytes).expect("decode");
    img.save(path).expect("save");
    // Cheap content hash to detect screen changes.
    let sum = bytes.iter().map(|b| *b as u64).sum::<u64>();
    (img.width(), img.height(), sum)
}

#[tokio::main]
async fn main() {
    let udid = std::env::args().nth(1).unwrap_or_else(|| {
        umide_emulator::IosSimulator::get_booted_udids()
            .into_iter()
            .next()
            .expect("no booted simulator — pass a udid")
    });
    eprintln!("boot headless: {udid}");
    ios_grpc::IosCompanion::boot_headless(&udid).expect("boot");
    let companion = ios_grpc::IosCompanion::spawn(&udid).expect("companion");
    let mut client = ios_grpc::connect(companion.grpc_port)
        .await
        .expect("connect");

    // Give SpringBoard a moment, then baseline.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let (w, h, base) = shot(&mut client, "/tmp/ios_tap_0_before.png").await;
    eprintln!("baseline: {w}x{h}");

    // Candidate A: POINTS (pixels / 3 on iPhone 16) — Safari dock icon.
    let (px, py) = (w as f64 / 3.0 * 0.385, h as f64 / 3.0 * 0.93);
    eprintln!("tap A (points space): ({px:.0}, {py:.0})");
    ios_grpc::hid_tap(&mut client, px, py).await.expect("tap A");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (_, _, after_a) = shot(&mut client, "/tmp/ios_tap_1_points.png").await;
    let a_changed = after_a != base;
    eprintln!("after A: changed={a_changed}");

    if !a_changed {
        // Candidate B: PIXELS — same physical spot.
        let (qx, qy) = (w as f64 * 0.385, h as f64 * 0.93);
        eprintln!("tap B (pixel space): ({qx:.0}, {qy:.0})");
        ios_grpc::hid_tap(&mut client, qx, qy).await.expect("tap B");
        tokio::time::sleep(Duration::from_secs(3)).await;
        let (_, _, after_b) = shot(&mut client, "/tmp/ios_tap_2_pixels.png").await;
        eprintln!("after B: changed={}", after_b != base);
        println!(
            "space: {}",
            if after_b != base { "PIXELS" } else { "UNKNOWN" }
        );
        return;
    }
    println!("space: POINTS");

    // Bonus: swipe up from the bottom (home gesture) in the same space.
    let (sx, sy) = (w as f64 / 6.0, h as f64 / 3.0 * 0.98);
    ios_grpc::hid_swipe(&mut client, (sx, sy), (sx, sy - 200.0), 0.3)
        .await
        .expect("swipe");
    tokio::time::sleep(Duration::from_secs(2)).await;
    shot(&mut client, "/tmp/ios_tap_3_swipe.png").await;
    eprintln!("swipe sent; /tmp/ios_tap_3_swipe.png");
}
