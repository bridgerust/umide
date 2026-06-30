//! M3 input self-test (headless): drive a top→down swipe through the gRPC
//! input path (`EmulatorInput`) to open the notification shade — verifies that
//! touch_down/move/up actually reach the device, no GUI needed. Then run
//! `grab_frame` to capture the result.
//!
//! Run with an emulator on gRPC 8554:
//!     cargo run -p umide-app --example tap_test

use std::time::Duration;

use umide_app::panel::emulator_stream::start_emulator_input;

fn main() {
    let input = start_emulator_input("http://localhost:8554".to_string());
    std::thread::sleep(Duration::from_secs(3)); // let the command client connect

    // Swipe down from the very top → pulls down the notification shade.
    input.touch_down(540, 20);
    let mut y = 20;
    while y < 1600 {
        input.touch_move(540, y);
        std::thread::sleep(Duration::from_millis(16));
        y += 80;
    }
    input.touch_up(540, 1600);

    std::thread::sleep(Duration::from_secs(2)); // let queued events flush over gRPC
    println!("swipe-down sent");
}
