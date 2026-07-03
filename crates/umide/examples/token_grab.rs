//! Verify #45 (gRPC token auth for externally-launched emulators).
//!
//! Resolves the running emulator's gRPC discovery (port + token) exactly as the
//! panel does, then proves the token is BOTH necessary (a call without it is
//! rejected) AND sufficient (a call with it pulls a real frame).
//!
//! Run with a token-authed emulator up (launched OUTSIDE UMIDE, e.g. Android
//! Studio, or `emulator -avd … ` with no `-grpc`):
//!   cargo run -p umide-app --example token_grab [emulator-5554]

use std::time::Duration;
use umide_emulator::AndroidEmulator;
use umide_emulator::grpc_client::EmulatorGrpcClient;

#[tokio::main]
async fn main() {
    let serial = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "emulator-5554".to_string());

    let disc = AndroidEmulator::grpc_discovery(&serial)
        .expect("no running emulator discovery for that serial");
    let endpoint = format!("http://localhost:{}", disc.port);
    println!(
        "discovery: serial={serial} port={} token={}",
        disc.port,
        if disc.token.is_some() {
            "present"
        } else {
            "none"
        }
    );

    // 1) WITHOUT the token → the RPC should be rejected (proves token auth is on
    //    and that we were previously failing for want of the token).
    match EmulatorGrpcClient::connect_with_retry(
        &endpoint,
        Duration::from_secs(5),
        None,
    )
    .await
    {
        Ok(mut c) => match c.get_screenshot().await {
            Ok(_) => {
                println!("NO-TOKEN: unexpectedly got a frame (not token-authed?)")
            }
            Err(e) => println!("NO-TOKEN: rejected as expected -> {e}"),
        },
        Err(e) => println!("NO-TOKEN: connect failed -> {e}"),
    }

    // 2) WITH the token → a real frame, i.e. UMIDE can now stream an emulator it
    //    did not launch.
    let mut client = EmulatorGrpcClient::connect_with_retry(
        &endpoint,
        Duration::from_secs(10),
        disc.token.as_deref(),
    )
    .await
    .expect("connect WITH token");
    let frame = client
        .get_screenshot()
        .await
        .expect("get_screenshot WITH token");
    println!(
        "WITH-TOKEN: got frame {}x{} ({} bytes) — #45 WORKS",
        frame.width,
        frame.height,
        frame.width * frame.height * 4
    );
}
