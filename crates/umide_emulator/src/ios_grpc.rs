//! gRPC client for `idb_companion` — the frame tap into a HEADLESS iOS
//! simulator, so UMIDE can embed the device without ever launching
//! Simulator.app (which is only a viewer; `simctl boot` runs the device fully
//! on its own).
//!
//! This is the iOS twin of `grpc_client.rs` (the Android emulator's gRPC):
//! `idb_companion --udid <udid>` is spawned per device, prints its gRPC port
//! on stdout, and serves the `idb.CompanionService` from `proto/idb.proto`
//! (vendored at the installed companion's version, v1.1.8). Input rides the
//! `hid` RPC: taps/swipes in screen POINTS (verified live — a points-space tap
//! opened Safari; pixel-space landed nowhere), keys as USB-HID usage codes.
//!
//! **Headless recipe (verified live).** The load-bearing discovery: a device
//! booted with `simctl boot` has NO rendering surface, so every frame-capture
//! path fails ("No Image available to encode"; even `simctl io screenshot`
//! hangs ~60s). Booting instead with [`IosCompanion::boot_headless`]
//! (`idb_companion --boot`) attaches the CoreSimulator framebuffer WITHOUT
//! opening Simulator.app, and the `screenshot` RPC then returns a real frame
//! (proven: a live 1179×2556 iPhone-16 home screen with Simulator.app closed).
//! So the embedded panel's frame source is the on-demand `screenshot` RPC
//! polled at a target fps — decoded through the `image` crate to RGBA — which
//! is robust regardless of screen activity. (`video_stream` H.264 is a future
//! low-latency optimization; it currently does not emit over this companion
//! version.)
//!
//! The companion binary is an external dependency (`brew install
//! idb-companion`); callers must degrade gracefully when it's absent — the
//! panel falls back to the Simulator.app flow.

use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Generated from `proto/idb.proto` (facebook/idb v1.1.8).
pub mod idb_proto {
    tonic::include_proto!("idb");
}

use idb_proto::companion_service_client::CompanionServiceClient;
use idb_proto::video_stream_request::{Control, Format, Start};
use idb_proto::{VideoStreamRequest, VideoStreamResponse};

/// The connected companion gRPC client, over a plain TCP channel.
pub type CompanionClient = CompanionServiceClient<tonic::transport::Channel>;

/// How long to wait for a freshly spawned companion to report its gRPC port.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(20);

/// A spawned `idb_companion` process serving one simulator over gRPC. Dropping
/// it kills the companion, which ends any open streams.
pub struct IosCompanion {
    child: Option<Child>,
    pub grpc_port: u16,
}

impl IosCompanion {
    /// Locate the companion binary: PATH first, then the Homebrew locations
    /// (a GUI app's PATH is minimal on macOS).
    fn companion_binary() -> &'static str {
        for candidate in [
            "idb_companion",
            "/opt/homebrew/bin/idb_companion",
            "/usr/local/bin/idb_companion",
        ] {
            if which_ok(candidate) {
                return candidate;
            }
        }
        "idb_companion"
    }

    /// Whether the companion is available at all — the panel uses this to
    /// choose between the embedded path and the Simulator.app fallback.
    pub fn available() -> bool {
        which_ok("idb_companion")
            || which_ok("/opt/homebrew/bin/idb_companion")
            || which_ok("/usr/local/bin/idb_companion")
    }

    /// Boot the simulator HEADLESS with a framebuffer attached, via
    /// `idb_companion --boot`. This is the crucial difference from
    /// `simctl boot`: a `simctl`-booted device has NO rendering surface, so
    /// `screenshot`/`video_stream` return "No Image available to encode" (and
    /// even `simctl io screenshot` hangs). `idb_companion --boot` attaches the
    /// CoreSimulator framebuffer WITHOUT opening Simulator.app, so the device
    /// renders offscreen and frames can be captured — the whole basis of the
    /// embedded iOS panel. Idempotent: a booted device just reports its state.
    /// Runs to completion (the boot command exits once booted; it does not
    /// stay resident — the separate `spawn` companion serves gRPC).
    pub fn boot_headless(udid: &str) -> std::io::Result<()> {
        let status = Command::new(Self::companion_binary())
            .args(["--boot", udid])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "idb_companion --boot exited with {status}"
            )))
        }
    }

    /// Spawn `idb_companion --udid <udid>` and wait for it to print its gRPC
    /// port (a JSON line like `{"grpc_port":10882}` on stdout).
    pub fn spawn(udid: &str) -> std::io::Result<Self> {
        let mut child = Command::new(Self::companion_binary())
            .args(["--udid", udid, "--only", "simulator"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("companion stdout unavailable"))?;

        // The port line arrives once the companion is serving; read it off a
        // thread so a wedged spawn can't hang the caller past the timeout.
        let (tx, rx) = mpsc::channel::<u16>();
        std::thread::Builder::new()
            .name("umide-idb-spawn".into())
            .spawn(move || {
                let reader = std::io::BufReader::new(stdout);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if let Some(port) = parse_grpc_port(&line) {
                        let _ = tx.send(port);
                        break;
                    }
                }
                // Keep draining so the companion never blocks on a full pipe.
            })?;

        match rx.recv_timeout(SPAWN_TIMEOUT) {
            Ok(port) => Ok(Self {
                child: Some(child),
                grpc_port: port,
            }),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "idb_companion did not report a gRPC port in time",
                ))
            }
        }
    }
}

impl Drop for IosCompanion {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Extract the port from the companion's `{"grpc_port":10882,…}` stdout line.
fn parse_grpc_port(line: &str) -> Option<u16> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    v.get("grpc_port")?.as_u64()?.try_into().ok()
}

/// Connect to a running companion's gRPC endpoint.
pub async fn connect(
    port: u16,
) -> Result<CompanionServiceClient<tonic::transport::Channel>, tonic::transport::Error>
{
    CompanionServiceClient::connect(format!("http://127.0.0.1:{port}")).await
}

/// Start an H.264 screen stream from the simulator. The returned stream yields
/// Annex-B chunks; feed them straight into [`crate::create_decoder`]'s
/// `decode_frame`. The request stream is held open for the call's lifetime —
/// dropping the response stream (or the companion) ends it.
pub async fn video_stream_h264(
    client: &mut CompanionServiceClient<tonic::transport::Channel>,
    fps: u64,
) -> Result<tonic::Streaming<VideoStreamResponse>, tonic::Status> {
    let start = VideoStreamRequest {
        control: Some(Control::Start(Start {
            // Empty file path ⇒ stream the payload over gRPC.
            file_path: String::new(),
            fps,
            format: Format::H264 as i32,
            compression_quality: 1.0,
            scale_factor: 1.0,
            avg_bitrate: 0.0,
        })),
    };
    // Send Start, then keep the request side open (pending never yields), so
    // the companion keeps streaming until the response side is dropped.
    let requests =
        tokio_stream::StreamExt::chain(tokio_stream::once(start), pending());
    let response = client.video_stream(requests).await?;
    Ok(response.into_inner())
}

/// Pull a single full frame on demand via the `screenshot` RPC. Returns the
/// encoded image bytes and their format string (e.g. `"png"`). Unlike
/// `video_stream`, this always yields a frame regardless of screen activity —
/// the panel uses it for the first paint and as the robust fallback when the
/// H.264 stream is quiet.
pub async fn screenshot(
    client: &mut CompanionServiceClient<tonic::transport::Channel>,
) -> Result<(Vec<u8>, String), tonic::Status> {
    let resp = client
        .screenshot(idb_proto::ScreenshotRequest {})
        .await?
        .into_inner();
    Ok((resp.image_data, resp.image_format))
}

/// The device's screen geometry from the `describe` RPC:
/// `(width_px, height_px, width_points, height_points, density)`. The HID
/// touch space is POINTS, the frames are PIXELS — the panel divides by
/// `width_px / width_points` (or `density`) to map between them.
pub async fn screen_dimensions(
    client: &mut CompanionClient,
) -> Result<Option<(u64, u64, u64, u64, f64)>, tonic::Status> {
    let resp = client
        .describe(idb_proto::TargetDescriptionRequest {
            fetch_diagnostics: false,
        })
        .await?
        .into_inner();
    Ok(resp
        .target_description
        .and_then(|t| t.screen_dimensions)
        .map(|d| {
            (
                d.width,
                d.height,
                d.width_points,
                d.height_points,
                d.density,
            )
        }))
}

/// Synthesize a TAP at `(x, y)` via the `hid` client-streaming RPC: touch
/// press DOWN → short delay → touch press UP, one stream per gesture (the
/// pattern `idb ui tap` itself uses). Coordinate space: screen POINTS
/// (logical), not pixels — verified empirically on an iPhone 16 (pixels ÷
/// scale; see `ios_stream`'s mapping).
pub async fn hid_tap(
    client: &mut CompanionClient,
    x: f64,
    y: f64,
) -> Result<(), tonic::Status> {
    let events = vec![
        touch_press(hid_event::HidDirection::Down, x, y),
        hid_delay(0.05),
        touch_press(hid_event::HidDirection::Up, x, y),
    ];
    client.hid(tokio_stream::iter(events)).await?;
    Ok(())
}

/// Synthesize a SWIPE from `start` to `end` over `duration` seconds (screen
/// points). One `HIDSwipe` event; the companion interpolates the path.
pub async fn hid_swipe(
    client: &mut CompanionClient,
    start: (f64, f64),
    end: (f64, f64),
    duration: f64,
) -> Result<(), tonic::Status> {
    let ev = idb_proto::HidEvent {
        event: Some(hid_event::Event::Swipe(hid_event::HidSwipe {
            start: Some(idb_proto::Point {
                x: start.0,
                y: start.1,
            }),
            end: Some(idb_proto::Point { x: end.0, y: end.1 }),
            delta: 0.0,
            duration,
        })),
    };
    client.hid(tokio_stream::iter(vec![ev])).await?;
    Ok(())
}

/// Press-and-release one USB-HID keyboard usage code (page 0x07), with
/// optional Left-Shift wrapped around it for shifted characters.
pub async fn hid_key(
    client: &mut CompanionClient,
    keycode: u64,
    shifted: bool,
) -> Result<(), tonic::Status> {
    const LEFT_SHIFT: u64 = 225;
    let mut events = Vec::new();
    if shifted {
        events.push(key_press(hid_event::HidDirection::Down, LEFT_SHIFT));
    }
    events.push(key_press(hid_event::HidDirection::Down, keycode));
    events.push(key_press(hid_event::HidDirection::Up, keycode));
    if shifted {
        events.push(key_press(hid_event::HidDirection::Up, LEFT_SHIFT));
    }
    client.hid(tokio_stream::iter(events)).await?;
    Ok(())
}

fn touch_press(
    direction: hid_event::HidDirection,
    x: f64,
    y: f64,
) -> idb_proto::HidEvent {
    idb_proto::HidEvent {
        event: Some(hid_event::Event::Press(hid_event::HidPress {
            action: Some(hid_event::HidPressAction {
                action: Some(hid_event::hid_press_action::Action::Touch(
                    hid_event::HidTouch {
                        point: Some(idb_proto::Point { x, y }),
                    },
                )),
            }),
            direction: direction as i32,
        })),
    }
}

fn key_press(
    direction: hid_event::HidDirection,
    keycode: u64,
) -> idb_proto::HidEvent {
    idb_proto::HidEvent {
        event: Some(hid_event::Event::Press(hid_event::HidPress {
            action: Some(hid_event::HidPressAction {
                action: Some(hid_event::hid_press_action::Action::Key(
                    hid_event::HidKey { keycode },
                )),
            }),
            direction: direction as i32,
        })),
    }
}

fn hid_delay(seconds: f64) -> idb_proto::HidEvent {
    idb_proto::HidEvent {
        event: Some(hid_event::Event::Delay(hid_event::HidDelay {
            duration: seconds,
        })),
    }
}

use idb_proto::hid_event;

/// Map a typed character to its USB-HID usage code (page 0x07) + whether Shift
/// is needed. Covers the practical typing set; `None` for unmapped chars.
pub fn char_to_hid(c: char) -> Option<(u64, bool)> {
    Some(match c {
        'a'..='z' => (4 + (c as u64 - 'a' as u64), false),
        'A'..='Z' => (4 + (c.to_ascii_lowercase() as u64 - 'a' as u64), true),
        '1'..='9' => (30 + (c as u64 - '1' as u64), false),
        '0' => (39, false),
        '!' => (30, true),
        '@' => (31, true),
        '#' => (32, true),
        '$' => (33, true),
        '%' => (34, true),
        '^' => (35, true),
        '&' => (36, true),
        '*' => (37, true),
        '(' => (38, true),
        ')' => (39, true),
        ' ' => (44, false),
        '-' => (45, false),
        '_' => (45, true),
        '=' => (46, false),
        '+' => (46, true),
        '[' => (47, false),
        '{' => (47, true),
        ']' => (48, false),
        '}' => (48, true),
        '\\' => (49, false),
        '|' => (49, true),
        ';' => (51, false),
        ':' => (51, true),
        '\'' => (52, false),
        '"' => (52, true),
        '`' => (53, false),
        '~' => (53, true),
        ',' => (54, false),
        '<' => (54, true),
        '.' => (55, false),
        '>' => (55, true),
        '/' => (56, false),
        '?' => (56, true),
        _ => return None,
    })
}

/// USB-HID usage codes for the named keys the panel forwards.
pub mod hid_usage {
    pub const ENTER: u64 = 40;
    pub const ESCAPE: u64 = 41;
    pub const BACKSPACE: u64 = 42;
    pub const TAB: u64 = 43;
    pub const ARROW_RIGHT: u64 = 79;
    pub const ARROW_LEFT: u64 = 80;
    pub const ARROW_DOWN: u64 = 81;
    pub const ARROW_UP: u64 = 82;
}

/// H.264 payload bytes out of one stream message, if it carries any.
pub fn payload_bytes(msg: &VideoStreamResponse) -> Option<&[u8]> {
    use idb_proto::payload::Source;
    use idb_proto::video_stream_response::Output;
    match msg.output.as_ref()? {
        Output::Payload(p) => match p.source.as_ref()? {
            Source::Data(d) => Some(d.as_slice()),
            _ => None,
        },
        Output::LogOutput(_) => None,
    }
}

/// A request stream that never ends (after the initial Start message).
fn pending() -> tokio_stream::Pending<VideoStreamRequest> {
    tokio_stream::pending()
}

fn which_ok(bin: &str) -> bool {
    if bin.contains('/') {
        std::path::Path::new(bin).exists()
    } else {
        Command::new("which")
            .arg(bin)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_companion_port_line() {
        assert_eq!(
            parse_grpc_port(r#"{"grpc_port":10882,"grpc_swift_port":10882}"#),
            Some(10882)
        );
        assert_eq!(parse_grpc_port("not json"), None);
        assert_eq!(parse_grpc_port(r#"{"other":1}"#), None);
    }
}
