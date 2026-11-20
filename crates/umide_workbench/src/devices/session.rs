use std::{process::Command, sync::Arc, time::Duration};

use floem::{ext_event::ArcRwSignal, views::RgbaFrame};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};
#[cfg(target_os = "macos")]
use umide_emulator::ios_grpc;
use umide_emulator::{AndroidEmulator, EmulatorGrpcClient};

type Result<T> = std::result::Result<T, String>;
type Point = (i32, i32);

#[derive(Clone, Copy)]
pub(super) struct Gesture {
    pub start: Point,
    pub end: Point,
    pub duration: Duration,
}

impl Gesture {
    fn is_tap(&self) -> bool {
        (self.start.0 - self.end.0).abs() < 6
            && (self.start.1 - self.end.1).abs() < 6
    }
}

#[derive(Clone)]
pub(super) struct Frame {
    pub image: RgbaFrame,
    pub input_size: Option<(u32, u32)>,
}

pub(super) struct Preview {
    pub frame: ArcRwSignal<Option<Frame>>,
    pub status: ArcRwSignal<String>,
    pub control_status: ArcRwSignal<String>,
    pub gestures: mpsc::Sender<Gesture>,
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

async fn execute(command: Command, seconds: u64) -> Result<Vec<u8>> {
    let name = command.get_program().to_string_lossy().into_owned();
    let mut cmd = tokio::process::Command::from(command);
    cmd.kill_on_drop(true);
    let out = timeout(Duration::from_secs(seconds), cmd.output())
        .await
        .map_err(|_| format!("{name} timed out"))?
        .map_err(|e| format!("{name}: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let detail = String::from_utf8_lossy(&out.stderr);
        Err(format!(
            "{name}: {}",
            detail.trim().chars().take(240).collect::<String>()
        ))
    }
}

#[cfg(target_os = "macos")]
fn simctl(args: &[&str]) -> Command {
    let mut cmd = Command::new("/usr/bin/xcrun");
    cmd.arg("simctl").args(args);
    cmd
}

pub(super) fn boot(platform: &str, id: &str) -> Result<()> {
    match platform {
        "android" => AndroidEmulator::launch(id).map_err(|e| e.to_string()),
        #[cfg(target_os = "macos")]
        "ios" => runtime()?
            .block_on(execute(simctl(&["bootstatus", id, "-b"]), 180))
            .map(|_| ()),
        _ => Err("Unsupported platform".into()),
    }
}

async fn screenshot(platform: &str, id: &str) -> Result<Vec<u8>> {
    let bytes = match platform {
        "android" => {
            let mut cmd = AndroidEmulator::adb_command(id);
            cmd.args(["exec-out", "screencap", "-p"]);
            execute(cmd, 10).await?
        }
        #[cfg(target_os = "macos")]
        "ios" => {
            let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
            let path = temp.path().join("screen.png");
            let mut cmd = simctl(&["io", id, "screenshot", "--type=png"]);
            cmd.arg(&path);
            execute(cmd, 12).await?;
            std::fs::read(path).map_err(|e| e.to_string())?
        }
        _ => return Err("Unsupported platform".into()),
    };
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("Device did not return a PNG screenshot".into());
    }
    Ok(bytes)
}

/// Capture the selected adb serial or simulator UDID. Call on a worker thread.
pub fn capture_png(platform: &str, id: &str) -> Result<Vec<u8>> {
    runtime()?.block_on(screenshot(platform, id))
}

pub(super) fn start(platform: String, id: String) -> Preview {
    let frame = ArcRwSignal::new(None);
    let status = ArcRwSignal::new("Connecting…".to_string());
    let (gestures, mut rx) = mpsc::channel::<Gesture>(8);
    let frame_out = frame.clone();
    let status_out = status.clone();
    let control_status = ArcRwSignal::new(if platform == "ios" {
        "Connecting controls…".to_owned()
    } else {
        String::new()
    });
    let control_out = control_status.clone();
    std::thread::spawn(move || {
        let rt = match runtime() {
            Ok(rt) => rt,
            Err(err) => {
                status_out.set(err);
                return;
            }
        };
        rt.block_on(async {
            // Closing the panel drops the sender and cancels both capture and input,
            // even while awaiting a frame on an idle screen.
            let (inputs, inputs_rx) = mpsc::channel(8);
            let work = run(
                platform.as_str(),
                &id,
                &frame_out,
                &status_out,
                &control_out,
                inputs_rx,
            );
            tokio::pin!(work);
            loop {
                tokio::select! {
                    result = &mut work => {
                        if let Err(message) = result { status_out.set(message); }
                        break;
                    }
                    input = rx.recv() => match input {
                        Some(gesture) => { let _ = inputs.try_send(gesture); }
                        None => break,
                    }
                }
            }
        });
    });
    Preview {
        frame,
        status,
        control_status,
        gestures,
    }
}

type Frames = ArcRwSignal<Option<Frame>>;
type Status = ArcRwSignal<String>;

async fn run(
    platform: &str,
    id: &str,
    frames: &Frames,
    status: &Status,
    control_status: &Status,
    inputs: mpsc::Receiver<Gesture>,
) -> Result<()> {
    if platform == "android" {
        let capture = async {
            if let Some(details) = AndroidEmulator::grpc_discovery(id) {
                let endpoint = format!("http://127.0.0.1:{}", details.port);
                let _ = android_video(
                    &endpoint,
                    details.token.as_deref(),
                    frames,
                    status,
                )
                .await;
            }
            poll_images(platform, id, frames, status).await
        };
        tokio::select! {
            result = capture => result,
            () = android_input(id, inputs, control_status) => Ok(()),
        }
    } else {
        #[cfg(target_os = "macos")]
        {
            let controls = ios_input(id, inputs, frames, control_status);
            tokio::select! {
                result = poll_images(platform, id, frames, status) => result,
                () = controls => Ok(()),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("iOS Simulator requires macOS".into())
        }
    }
}

async fn android_video(
    endpoint: &str,
    token: Option<&str>,
    frames: &Frames,
    status: &Status,
) -> Result<()> {
    let mut client = timeout(
        Duration::from_secs(5),
        EmulatorGrpcClient::connect(endpoint, token),
    )
    .await
    .map_err(|_| "Device connection timed out".to_string())?
    .map_err(|e| e.to_string())?;
    let native = timeout(Duration::from_secs(5), client.get_screenshot())
        .await
        .map_err(|_| "Device capture timed out".to_string())?
        .map_err(|e| e.to_string())?;
    let (width, height) = (native.width, native.height);
    if width == 0 || height == 0 {
        return Err("Invalid display dimensions".into());
    }
    let ratio = (1280. / width.max(height) as f64).min(1.);
    let (tx, mut rx) = mpsc::channel(1);
    let stream = client.stream_screenshots(
        tx,
        (width as f64 * ratio).round() as u32,
        (height as f64 * ratio).round() as u32,
    );
    tokio::pin!(stream);
    loop {
        tokio::select! {
            result = &mut stream => return result.map_err(|e| e.to_string()),
            next = rx.recv() => {
                let Some(frame) = next else { return Ok(()); };
                if let Some(data) = frame.rgba_arc() {
                    let rotated = (width > height) != (frame.width > frame.height);
                    let input_size = if rotated { (height,width) } else { (width,height) };
                    frames.set(Some(Frame { image: RgbaFrame { data, width: frame.width, height: frame.height }, input_size: Some(input_size) }));
                    if !status.read().is_empty() { status.set(String::new()); }
                }
            }
        }
    }
}

async fn poll_images(
    platform: &str,
    id: &str,
    frames: &Frames,
    status: &Status,
) -> Result<()> {
    loop {
        let image = screenshot(platform, id).await.and_then(|png| {
            image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                .map(|img| img.into_rgba8())
                .map_err(|e| e.to_string())
        });
        match image {
            Ok(rgba) => {
                let (width, height) = rgba.dimensions();
                // For iOS the companion reports the actual scale in screen points.
                // Its input worker maps pixels to points for this frame at release.
                frames.set(Some(Frame {
                    image: RgbaFrame {
                        data: Arc::new(rgba.into_raw()),
                        width,
                        height,
                    },
                    input_size: Some((width, height)),
                }));
                if !status.read().is_empty() {
                    status.set(String::new());
                }
            }
            Err(message) => {
                frames.set(None);
                status.set(message);
            }
        }
        sleep(Duration::from_millis(450)).await;
    }
}

async fn android_input(
    id: &str,
    mut inputs: mpsc::Receiver<Gesture>,
    status: &Status,
) {
    while let Some(gesture) = inputs.recv().await {
        let mut cmd = AndroidEmulator::adb_command(id);
        let action = if gesture.is_tap() { "tap" } else { "swipe" };
        cmd.args(["shell", "input", action])
            .arg(gesture.start.0.to_string())
            .arg(gesture.start.1.to_string());
        if !gesture.is_tap() {
            cmd.arg(gesture.end.0.to_string())
                .arg(gesture.end.1.to_string())
                .arg(gesture.duration.as_millis().clamp(80, 1500).to_string());
        }
        if let Err(message) = execute(cmd, 8).await {
            status.set(message);
        }
    }
}

#[cfg(target_os = "macos")]
async fn ios_input(
    id: &str,
    mut inputs: mpsc::Receiver<Gesture>,
    frames: &Frames,
    status: &Status,
) {
    if !ios_grpc::IosCompanion::available() {
        status.set("Preview only · install idb_companion to control iOS".into());
        while inputs.recv().await.is_some() {}
        return;
    }
    let id = id.to_owned();
    let connection = async {
        let companion =
            tokio::task::spawn_blocking(move || ios_grpc::IosCompanion::spawn(&id))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
        let mut client = ios_grpc::connect(companion.grpc_port)
            .await
            .map_err(|e| e.to_string())?;
        let (_, _, w, h, _) = ios_grpc::screen_dimensions(&mut client)
            .await
            .map_err(|e| e.to_string())?
            .filter(|it| it.2 > 0 && it.3 > 0)
            .ok_or("Simulator did not report its screen size")?;
        Ok::<_, String>((companion, client, w, h))
    }
    .await;
    let (_companion, mut client, w, h) = match connection {
        Ok(ready) => ready,
        Err(e) => {
            status.set(format!("Preview only · {e}"));
            while inputs.recv().await.is_some() {}
            return;
        }
    };
    status.set(String::new());
    while let Some(gesture) = inputs.recv().await {
        let Some(frame) = frames.get_untracked() else {
            continue;
        };
        let (fw, fh) = (frame.image.width as f64, frame.image.height as f64);
        let (w, h) = if (w > h) != (fw > fh) { (h, w) } else { (w, h) };
        let points = |p: Point| {
            (
                f64::from(p.0) * w as f64 / fw,
                f64::from(p.1) * h as f64 / fh,
            )
        };
        let from = points(gesture.start);
        let result = if gesture.is_tap() {
            ios_grpc::hid_tap(&mut client, from.0, from.1).await
        } else {
            ios_grpc::hid_swipe(
                &mut client,
                from,
                points(gesture.end),
                gesture.duration.as_secs_f64().clamp(0.08, 1.5),
            )
            .await
        };
        if let Err(err) = result {
            status.set(format!("Gesture failed: {err}"));
        }
    }
}
