//! Grab ONE live frame off the emulator's gRPC endpoint, decode it, and save a
//! PNG — a headless verification of the M2 acquisition path (gRPC → decode →
//! RGBA), independent of any window/GPU. Usage:
//!     cargo run -p umide-app --example grab_frame -- /path/out.png

use std::time::Duration;

use umide_emulator::grpc_client::EmulatorGrpcClient;

#[tokio::main]
async fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/umide_frame.png".to_string());

    let mut client =
        EmulatorGrpcClient::connect_with_retry("http://localhost:8554", Duration::from_secs(30))
            .await
            .expect("connect to emulator gRPC");

    let frame = client.get_screenshot().await.expect("get_screenshot");
    let rgba = frame.to_rgba().expect("to_rgba");
    println!(
        "got live frame: {}x{}, {} bytes RGBA",
        frame.width,
        frame.height,
        rgba.len()
    );

    let img = image::RgbaImage::from_raw(frame.width, frame.height, rgba)
        .expect("RgbaImage::from_raw (size mismatch?)");
    img.save(&out).expect("save png");
    println!("saved {out}");
}
