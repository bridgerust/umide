//! A verification session owns its build and can only address installed virtual devices.
use std::{
    collections::HashSet,
    io::Cursor,
    path::{Path, PathBuf},
    process::Child,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use umide_emulator::{AndroidEmulator, DeviceInfo, DevicePlatform, DeviceState};

use crate::{
    devices::{capture_png, device_command, tap},
    projects::{AppRun, device_logs},
};

type Result<T> = std::result::Result<T, String>;
const BOOT_LIMIT: Duration = Duration::from_secs(180);
const BUILD_LIMIT: Duration = Duration::from_secs(15 * 60);
const MAX_LOG: usize = 4000;
const MAX_STATUS_LOG: usize = 1600;
const MAX_PNG: usize = 2 * 1024 * 1024;

pub(super) fn tools() -> Vec<Value> {
    let platform = json!({"type":"string","enum":["ios","android"]});
    let device_id = json!({"type":"string","minLength":1,"maxLength":256,"description":"Installed virtual device ID from mobile_devices, or its listed running Android emulator serial. Defaults to a running device on the requested platform, if available."});
    let pixel = json!({"type":"integer","minimum":0,"maximum":1279});
    [
        ("mobile_devices","List installed Android AVDs and iOS simulators and their running state.",json!({"platform":platform}),vec![]),
        ("mobile_boot","Select an installed virtual device and start it if necessary. Poll mobile_status until ready.",json!({"platform":platform,"device_id":device_id}),vec!["platform"]),
        ("mobile_launch","Build, install and launch this checkout with Umide on the selected emulator or simulator. Pass platform on first use. A stopped virtual device is booted automatically. Poll mobile_status for progress and build output.",json!({"platform":platform,"device_id":device_id}),vec![]),
        ("mobile_status","Read device readiness and the latest build state, diagnostics and output.",json!({}),vec![]),
        ("mobile_logs","Read the latest device system logs or build output (up to 4 KB). Defaults to device. Use source=build to revisit recent compiler diagnostics.",json!({"source":{"type":"string","enum":["device","build"]}}),vec![]),
        ("mobile_screenshot","Capture the currently selected virtual device as a PNG. The returned dimensions are the coordinate space for mobile_tap.",json!({}),vec![]),
        ("mobile_tap","Tap the selected virtual device at integer pixel coordinates in the most recent mobile_screenshot, from the top left. iOS input requires idb_companion.",json!({"x":pixel,"y":pixel}),vec!["x","y"]),
        ("mobile_stop","Cancel this verification session’s build or queued launch and leave the virtual device running.",json!({}),vec![]),
    ].into_iter().map(|(name,description,properties,required)| json!({
        "name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}
    })).collect()
}

fn kind(device: &DeviceInfo) -> &'static str {
    match device.platform {
        DevicePlatform::Android => "android",
        DevicePlatform::Ios => "ios",
    }
}

fn adb_serial(id: &str) -> bool {
    id.strip_prefix("emulator-").is_some_and(|port| {
        port.parse::<u16>().is_ok_and(|p| p > 0)
            && port.bytes().all(|b| b.is_ascii_digit())
    })
}

fn address(device: &DeviceInfo) -> Option<&str> {
    if device.state != DeviceState::Running {
        return None;
    }
    match device.platform {
        DevicePlatform::Android => {
            device.serial.as_deref().filter(|s| adb_serial(s))
        }
        DevicePlatform::Ios => Some(&device.id),
    }
}

fn entry(device: &DeviceInfo) -> Value {
    let state = match device.state {
        DeviceState::Running => "running",
        DeviceState::Starting => "starting",
        DeviceState::Disconnected => "stopped",
    };
    json!({"platform":kind(device),"device_id":device.id,"name":device.name,"state":state,"target_id":address(device)})
}

fn text(value: Value) -> Value {
    json!({"type":"text","text":value.to_string()})
}

fn command(cmd: std::process::Command, deadline: Instant) -> Result<Vec<u8>> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() || super::cancelled() {
        return Err("Device query timed out or was cancelled.".into());
    }
    device_command(cmd, remaining.as_secs().clamp(1, 10))
}

fn inventory(platform: &str) -> Result<Vec<DeviceInfo>> {
    let deadline = Instant::now() + Duration::from_secs(12);
    match platform {
        "android" => {
            let avds = command(AndroidEmulator::avd_list_command(), deadline)?;
            let mut devices: Vec<_> = String::from_utf8_lossy(&avds)
                .lines()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(|id| DeviceInfo {
                    id: id.into(),
                    name: id.into(),
                    platform: DevicePlatform::Android,
                    state: DeviceState::Disconnected,
                    serial: None,
                })
                .collect();
            if !devices.is_empty() {
                let serials =
                    command(AndroidEmulator::device_list_command(), deadline)?;
                for line in String::from_utf8_lossy(&serials).lines() {
                    let mut fields = line.split_whitespace();
                    let Some(serial) = fields.next().filter(|s| adb_serial(s))
                    else {
                        continue;
                    };
                    if fields.next() != Some("device") {
                        continue;
                    }
                    let mut cmd = AndroidEmulator::adb_command(serial);
                    cmd.args(["emu", "avd", "name"]);
                    let bytes = command(cmd, deadline)?;
                    let name = String::from_utf8_lossy(&bytes);
                    if let Some(device) = devices.iter_mut().find(|d| {
                        Some(d.id.as_str()) == name.lines().next().map(str::trim)
                    }) {
                        device.state = DeviceState::Running;
                        device.serial = Some(serial.into());
                    }
                }
            }
            Ok(devices)
        }
        #[cfg(target_os = "macos")]
        "ios" => {
            let mut cmd = std::process::Command::new("/usr/bin/xcrun");
            cmd.args(["simctl", "list", "devices", "available", "--json"]);
            let bytes = command(cmd, deadline)?;
            umide_emulator::IosSimulator::parse_devices(&bytes)
                .map_err(|e| e.to_string())
        }
        #[cfg(not(target_os = "macos"))]
        "ios" => Err("iOS Simulator requires macOS and Xcode.".into()),
        _ => Err("Choose platform ios or android.".into()),
    }
}

fn choose(devices: &[DeviceInfo], id: Option<&str>) -> Result<DeviceInfo> {
    let preferred = devices
        .iter()
        .filter(|device| {
            id.is_none_or(|id| device.id == id || address(device) == Some(id))
        })
        .min_by_key(|device| {
            let other_ios = device.platform == DevicePlatform::Ios
                && !device.name.to_ascii_lowercase().contains("iphone");
            (
                address(device).is_none(),
                other_ios,
                &device.name,
                &device.id,
            )
        });
    match (preferred, id) {
        (Some(device), _) => Ok(device.clone()),
        (None, Some(id)) => Err(format!("No installed virtual device has ID {id}. Use mobile_devices to list available devices.")),
        (None, None) => Err("No virtual device is installed for this platform. Create an Android AVD or install an iOS simulator runtime.".into()),
    }
}

fn android_ready(device: &DeviceInfo) -> bool {
    let Some(serial) = address(device) else {
        return false;
    };
    let mut cmd = AndroidEmulator::adb_command(serial);
    cmd.args(["shell", "getprop", "sys.boot_completed"]);
    device_command(cmd, 4).is_ok_and(|value| value.trim_ascii() == b"1")
}

// The child is ours only until successful startup. A cancelled or failed startup
// releases its resources, while a ready emulator stays available to the workbench.
struct StartingEmulator(Option<Child>);
impl Drop for StartingEmulator {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn boot(selected: DeviceInfo, cancel: &AtomicBool) -> Result<DeviceInfo> {
    let deadline = Instant::now() + BOOT_LIMIT;
    #[cfg(target_os = "macos")]
    if selected.platform == DevicePlatform::Ios {
        // A non-blocking simctl boot lets us poll our own deadline and cancellation.
        if selected.state != DeviceState::Running {
            let mut cmd = std::process::Command::new("/usr/bin/xcrun");
            cmd.args(["simctl", "boot", &selected.id]);
            if let Err(err) = device_command(cmd, 20)
                && choose(&inventory("ios")?, Some(&selected.id))?.state
                    != DeviceState::Running
            {
                return Err(err);
            }
        }
        // CoreSimulator's bootstatus includes springboard/system-app readiness.
        let mut cmd = std::process::Command::new("/usr/bin/xcrun");
        cmd.args(["simctl", "bootstatus", &selected.id])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut status =
            StartingEmulator(Some(cmd.spawn().map_err(|e| e.to_string())?));
        loop {
            check_boot_time(deadline, cancel)?;
            if let Some(exit) = status
                .0
                .as_mut()
                .unwrap()
                .try_wait()
                .map_err(|e| e.to_string())?
            {
                if !exit.success() {
                    return Err(format!("simctl bootstatus exited with {exit}."));
                }
                return choose(&inventory("ios")?, Some(&selected.id));
            }
            thread::sleep(Duration::from_millis(250));
        }
    }
    if selected.platform != DevicePlatform::Android {
        return Err("Unsupported mobile platform.".into());
    }
    let mut emulator = StartingEmulator(None);
    if selected.state != DeviceState::Running {
        emulator.0 = Some(
            AndroidEmulator::spawn_headless(&selected.id, "host", None)
                .map_err(|e| e.to_string())?,
        );
    }
    let mut retried = false;
    loop {
        check_boot_time(deadline, cancel)?;
        if let Some(child) = emulator.0.as_mut()
            && let Some(exit) = child.try_wait().map_err(|e| e.to_string())?
        {
            if retried {
                return Err(format!("Android emulator exited with {exit}."));
            }
            emulator.0 = Some(
                AndroidEmulator::spawn_headless(
                    &selected.id,
                    "swiftshader_indirect",
                    None,
                )
                .map_err(|e| e.to_string())?,
            );
            retried = true;
        }
        let device = choose(&inventory("android")?, Some(&selected.id))?;
        if android_ready(&device) {
            if let Some(mut child) = emulator.0.take() {
                thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            return Ok(device);
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn check_boot_time(deadline: Instant, cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) || super::cancelled() {
        return Err("Virtual device startup was cancelled.".into());
    }
    if Instant::now() >= deadline {
        return Err(
            "Virtual device did not finish starting within three minutes.".into(),
        );
    }
    Ok(())
}

struct PendingBoot {
    result: mpsc::Receiver<Result<DeviceInfo>>,
    cancel: Arc<AtomicBool>,
    launch: Option<PathBuf>,
}
impl Drop for PendingBoot {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
struct Screen {
    display: (u32, u32),
    native: (u32, u32),
}

fn dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    let (w, h) =
        image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png)
            .into_dimensions()
            .map_err(|e| e.to_string())?;
    if w == 0
        || h == 0
        || w as u64 * h as u64 > 32_000_000
        || bytes.len() > 96 * 1024 * 1024
    {
        return Err("Screenshot exceeds the supported image size.".into());
    }
    Ok((w, h))
}

fn png(bytes: Vec<u8>) -> Result<(Screen, Value)> {
    let native = dimensions(&bytes)?;
    let (display, data) = if native.0.max(native.1) <= 1280 && bytes.len() <= MAX_PNG
    {
        (native, bytes)
    } else {
        let source =
            image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                .map_err(|e| e.to_string())?;
        let mut limit = 1280.min(native.0.max(native.1));
        loop {
            let image = source.thumbnail(limit, limit);
            let mut out = Cursor::new(Vec::new());
            image
                .write_to(&mut out, image::ImageFormat::Png)
                .map_err(|e| e.to_string())?;
            if out.get_ref().len() <= MAX_PNG {
                break ((image.width(), image.height()), out.into_inner());
            }
            limit = (limit / 2).max(1);
        }
    };
    Ok((
        Screen { display, native },
        json!({"type":"image","mimeType":"image/png","data":STANDARD.encode(data)}),
    ))
}

fn coords(screen: Screen, args: &Value) -> Result<(u32, u32)> {
    let axis = |key: &str, display: u32, native: u32| {
        args[key]
            .as_u64()
            .filter(|&p| p < u64::from(display))
            .map(|p| {
                (p * u64::from(native - 1) / u64::from((display - 1).max(1))) as u32
            })
            .ok_or_else(|| {
                format!(
                    "{key} must be an integer from 0 to {} in the last screenshot.",
                    display - 1
                )
            })
    };
    Ok((
        axis("x", screen.display.0, screen.native.0)?,
        axis("y", screen.display.1, screen.native.1)?,
    ))
}

// Multiple chats run independent workers in the same workbench process.
type DeviceKey = (&'static str, String);
static RESERVED: LazyLock<Mutex<HashSet<DeviceKey>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

struct Reservation(DeviceKey);

impl Reservation {
    fn acquire(
        devices: &[DeviceInfo],
        id: Option<&str>,
        current: Option<&Arc<Self>>,
    ) -> Result<(DeviceInfo, Arc<Self>)> {
        let preferred = choose(devices, id)?;
        let mut owned = RESERVED.lock().unwrap_or_else(|e| e.into_inner());
        let available = |device: &DeviceInfo| {
            let key = (kind(device), device.id.clone());
            current.is_some_and(|c| c.0 == key) || !owned.contains(&key)
        };
        let chosen = if available(&preferred) {
            preferred
        } else if id.is_some() {
            return Err(
                "This virtual device is in use by another conversation.".into()
            );
        } else {
            let free: Vec<_> =
                devices.iter().filter(|d| available(d)).cloned().collect();
            choose(&free, None).map_err(|_| "All available virtual devices are in use by another conversation.")?
        };
        let key = (kind(&chosen), chosen.id.clone());
        if let Some(current) = current.filter(|c| c.0 == key) {
            return Ok((chosen, Arc::clone(current)));
        }
        owned.insert(key.clone());
        Ok((chosen, Arc::new(Self(key))))
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        RESERVED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

#[derive(Default)]
pub(super) struct Mobile {
    selected: Option<DeviceInfo>,
    pending: Option<PendingBoot>,
    failure: Option<String>,
    run: Option<AppRun>,
    screen: Option<Screen>,
    build_cursor: u64,
    // Drop after the build has been stopped and the boot worker cancelled.
    reservation: Option<Arc<Reservation>>,
}

fn tail(s: &str, limit: usize) -> &str {
    let mut offset = s.len().saturating_sub(limit);
    while !s.is_char_boundary(offset) {
        offset += 1;
    }
    &s[offset..]
}

impl Mobile {
    pub(super) fn has_selection(&self) -> bool {
        self.selected.is_some()
    }

    pub(super) fn target(&self) -> Option<(String, String)> {
        let device = self.selected.as_ref()?;
        Some((kind(device).into(), address(device)?.into()))
    }

    fn start_build(&mut self, root: &Path) -> Result<()> {
        let (platform, id) =
            self.target().ok_or("The virtual device is not ready.")?;
        self.run = None;
        self.build_cursor = 0;
        match AppRun::start_until(
            root,
            &platform,
            &id,
            Some(Instant::now() + BUILD_LIMIT),
        ) {
            Ok(run) => {
                self.run = Some(run);
                Ok(())
            }
            Err(error) => {
                self.failure = Some(error.clone());
                Err(error)
            }
        }
    }

    fn poll(&mut self) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        let result = match pending.result.try_recv() {
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Virtual device worker exited.".into())
            }
            Ok(result) => result,
        };
        let launch = self.pending.take().and_then(|mut p| p.launch.take());
        match result {
            Ok(device) => {
                self.selected = Some(device);
                if let Some(project) = launch {
                    self.failure = self.start_build(&project).err();
                }
            }
            Err(error) => {
                if let Some(device) = self.selected.as_mut() {
                    device.state = DeviceState::Disconnected;
                }
                self.failure = Some(error);
            }
        }
    }

    fn status(&mut self) -> Value {
        let build = if let Some(run) = &self.run {
            let (state, written, output) = run.report();
            let (log, diagnostic) = output.parts();
            let delta = written.saturating_sub(self.build_cursor);
            self.build_cursor = written;
            let latest = tail(log, delta.min(MAX_STATUS_LOG as u64) as usize);
            Some(
                json!({"state":state,"output":latest,"output_truncated":delta > latest.len() as u64,"diagnostic":diagnostic}),
            )
        } else {
            self.pending
                .as_ref()
                .filter(|p| p.launch.is_some())
                .map(|_| json!({"state":"waiting_for_device"}))
        };
        json!({"device":self.selected.as_ref().map(entry),"build":build,"error":self.failure})
    }

    fn prepare(
        &mut self,
        platform: &str,
        id: Option<&str>,
        launch: Option<&Path>,
    ) -> Result<()> {
        if let Some(p) = self.pending.as_mut() {
            let same = self.selected.as_ref().is_some_and(|d| {
                kind(d) == platform
                    && id.is_none_or(|id| {
                        id == d.id || d.serial.as_deref() == Some(id)
                    })
            });
            if !same {
                return Err("Another virtual device is starting. Wait for mobile_status before changing devices.".into());
            }
            if let Some(root) = launch {
                p.launch = Some(root.to_owned());
            }
            return Ok(());
        }
        let list = inventory(platform)?;
        let preferred = self
            .selected
            .as_ref()
            .filter(|d| kind(d) == platform && list.iter().any(|v| v.id == d.id));
        let (device, reservation) = Reservation::acquire(
            &list,
            id.or_else(|| preferred.map(|d| d.id.as_str())),
            self.reservation.as_ref(),
        )?;
        let same = self.selected.as_ref().is_some_and(|d| {
            kind(d) == platform
                && d.id == device.id
                && address(d) == address(&device)
        });
        if self.run.as_ref().is_some_and(AppRun::is_running)
            && (!same || launch.is_some())
        {
            return Err("A build is active. Use mobile_status or mobile_stop before starting another build.".into());
        }
        if !same {
            self.run = None;
        }
        if !same || launch.is_some() {
            self.screen = None;
        }
        self.reservation = Some(Arc::clone(&reservation));
        self.failure = None;
        let ready = address(&device).is_some()
            && (platform == "ios" || android_ready(&device));
        self.selected = Some(device.clone());
        if ready {
            if let Some(root) = launch {
                self.start_build(root)?;
            }
            return Ok(());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancel);
        let (tx, result) = mpsc::channel();
        thread::Builder::new()
            .name("verify-device-boot".into())
            .spawn(move || {
                let _reservation = reservation;
                let _ = tx.send(boot(device, &stop));
            })
            .map_err(|e| e.to_string())?;
        self.selected.as_mut().unwrap().state = DeviceState::Starting;
        self.pending = Some(PendingBoot {
            result,
            cancel,
            launch: launch.map(Path::to_owned),
        });
        Ok(())
    }

    // ADB can reassign an emulator's console port. Recheck the original AVD/UDID
    // before each operation so a restarted or replaced target never receives input.
    fn require_current(&mut self) -> Result<(String, String)> {
        let (platform,id) = self.target().ok_or("Select and start a virtual device with mobile_boot or mobile_launch first.")?;
        let listed = inventory(&platform)?;
        let valid = listed.iter().any(|d| {
            Some(&d.id) == self.selected.as_ref().map(|s| &s.id)
                && address(d) == Some(&id)
        });
        if !valid {
            self.screen = None;
            if let Some(d) = self.selected.as_mut() {
                d.state = DeviceState::Disconnected;
            }
            return Err("Selected device changed or stopped. Use mobile_boot to select it again.".into());
        }
        Ok((platform, id))
    }

    pub(super) fn call(
        &mut self,
        name: &str,
        args: &Value,
        root: &Path,
    ) -> Result<Vec<Value>> {
        if name == "mobile_stop"
            && let Some(pending) = self.pending.as_mut()
        {
            pending.launch = None;
        }
        self.poll();
        let platform = match args.get("platform") {
            Some(v) => Some(
                v.as_str()
                    .filter(|s| matches!(*s, "ios" | "android"))
                    .ok_or("Choose platform ios or android.")?,
            ),
            None => None,
        };
        let id = match args.get("device_id") {
            Some(v) => Some(
                v.as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 256)
                    .ok_or("Invalid virtual device ID.")?,
            ),
            None => None,
        };
        match name {
            "mobile_devices" => {
                let platforms: &[_] = match platform {
                    Some("ios") => &["ios"],
                    Some("android") => &["android"],
                    _ if cfg!(target_os = "macos") => &["android", "ios"],
                    _ => &["android"],
                };
                let mut entries = Vec::new();
                let mut issues = Vec::new();
                for p in platforms {
                    match inventory(p) {
                        Ok(devices) => entries.extend(devices.iter().map(entry)),
                        Err(error) => issues.push(format!("{p}: {error}")),
                    }
                }
                Ok(vec![text(json!({"devices":entries,"diagnostics":issues}))])
            }
            "mobile_launch" | "mobile_boot" => {
                let platform = platform
                    .or_else(|| {
                        if name == "mobile_launch" {
                            self.selected.as_ref().map(kind)
                        } else {
                            None
                        }
                    })
                    .ok_or("Choose platform ios or android on first use.")?;
                self.prepare(
                    platform,
                    id,
                    (name == "mobile_launch").then_some(root),
                )?;
                Ok(vec![text(self.status())])
            }
            "mobile_status" => Ok(vec![text(self.status())]),
            "mobile_stop" => {
                if let Some(run) = &self.run {
                    run.stop();
                }
                Ok(vec![text(self.status())])
            }
            "mobile_logs" => match args.get("source").and_then(Value::as_str) {
                Some("build") => {
                    let run = self
                        .run
                        .as_ref()
                        .ok_or("This session has no build output yet.")?;
                    let (state, _, output) = run.report();
                    let (log, diagnostic) = output.parts();
                    Ok(vec![text(
                        json!({"state":state, "log":tail(log,MAX_LOG), "diagnostic":diagnostic}),
                    )])
                }
                Some("device") | None
                    if args.get("source").is_none_or(Value::is_string) =>
                {
                    let (platform, id) = self.require_current()?;
                    let logs = device_logs(&platform, &id);
                    let (log, detail) = logs.parts();
                    Ok(vec![text(
                        json!({"platform":platform,"target_id":id,"log":tail(log,MAX_LOG),"diagnostic":detail}),
                    )])
                }
                _ => Err("Choose device or build as the source of logs.".into()),
            },
            "mobile_screenshot" => {
                let (platform, id) = self.require_current()?;
                self.screen = None;
                let (screen, image) = png(capture_png(&platform, &id)?)?;
                self.screen = Some(screen);
                Ok(vec![
                    text(
                        json!({"platform":platform,"target_id":id,"width":screen.display.0,"height":screen.display.1}),
                    ),
                    image,
                ])
            }
            "mobile_tap" => {
                let screen = self.screen.ok_or("Take a mobile_screenshot of the current virtual device before tapping.")?;
                let point = coords(screen, args)?;
                let (platform, id) = self.require_current()?;
                if dimensions(&capture_png(&platform, &id)?)? != screen.native {
                    self.screen = None;
                    return Err("Device orientation or screen size changed. Take a new mobile_screenshot before tapping.".into());
                }
                tap(&platform, &id, point, screen.native)?;
                self.screen = None;
                Ok(vec![text(
                    json!({"platform":platform,"target_id":id,"tapped":true}),
                )])
            }
            _ => Err(format!("Unknown mobile tool {name}.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avd(id: &str, serial: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            id: id.into(),
            name: id.into(),
            platform: DevicePlatform::Android,
            state: if serial.is_some() {
                DeviceState::Running
            } else {
                DeviceState::Disconnected
            },
            serial: serial.map(str::to_owned),
        }
    }

    #[test]
    fn automatic_selection_prefers_running_virtual_targets() {
        let devices = vec![avd("A", None), avd("B", Some("emulator-5554"))];
        assert_eq!(choose(&devices, None).unwrap().id, "B");
        assert_eq!(choose(&devices, Some("A")).unwrap().id, "A");
        assert_eq!(choose(&devices, Some("emulator-5554")).unwrap().id, "B");
        assert!(choose(&devices, Some("emulator-5678")).is_err());
        for physical in [
            "R1234FUNC",
            "192.168.0.1:5555",
            "emulator-usb",
            "emulator-+23",
        ] {
            assert!(address(&avd("physical", Some(physical))).is_none());
        }
    }

    #[test]
    fn simultaneous_conversations_use_separate_virtual_devices() {
        let devices = vec![
            avd("reserve-a", Some("emulator-5554")),
            avd("reserve-b", None),
        ];
        let (one, lease) = Reservation::acquire(&devices, None, None).unwrap();
        assert_eq!(one.id, "reserve-a");
        assert!(Reservation::acquire(&devices, Some("reserve-a"), None).is_err());
        let (two, _other) = Reservation::acquire(&devices, None, None).unwrap();
        assert_eq!(two.id, "reserve-b");
        let (_, same) =
            Reservation::acquire(&devices, Some("reserve-a"), Some(&lease)).unwrap();
        drop(lease);
        assert!(Reservation::acquire(&devices, Some("reserve-a"), None).is_err());
        drop(same);
        assert!(Reservation::acquire(&devices, Some("reserve-a"), None).is_ok());
    }

    #[test]
    fn screenshot_coordinates_reach_native_edges_and_reject_invalid_input() {
        let mut raw = Cursor::new(vec![]);
        image::DynamicImage::new_rgb8(1600, 800)
            .write_to(&mut raw, image::ImageFormat::Png)
            .unwrap();
        let (screen, image) = png(raw.into_inner()).unwrap();
        assert_eq!(screen.display, (1280, 640));
        let data = STANDARD.decode(image["data"].as_str().unwrap()).unwrap();
        assert_eq!(dimensions(&data).unwrap(), screen.display);
        assert_eq!(
            coords(screen, &json!({"x":1279,"y":639})).unwrap(),
            (1599, 799)
        );
        assert!(coords(screen, &json!({"x":1280,"y":639})).is_err());
        assert!(coords(screen, &json!({"x":-1,"y":0})).is_err());
        assert!(coords(screen, &json!({"x":1.5,"y":0})).is_err());
    }

    #[test]
    fn input_requires_a_screenshot_and_platform_is_validated_before_device_queries()
    {
        let mut mobile = Mobile::default();
        assert!(
            mobile
                .call("mobile_tap", &json!({"x":0,"y":0}), Path::new("."))
                .is_err()
        );
        assert!(
            mobile
                .call("mobile_boot", &json!({"platform":"usb"}), Path::new("."))
                .is_err()
        );
        assert!(mobile.target().is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn simulator_inventory_excludes_non_ios_runtimes() {
        let listed = serde_json::to_vec(&json!({"devices":{
            "com.apple.CoreSimulator.SimRuntime.iOS-26-0":[{"name":"Test iPhone","udid":"ios","state":"Booted"}],
            "com.apple.CoreSimulator.SimRuntime.watchOS-26-0":[{"name":"Apple Watch","udid":"watch","state":"Booted"}]
        }})).unwrap();
        let devices = umide_emulator::IosSimulator::parse_devices(&listed).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(address(&devices[0]), Some("ios"));
    }

    #[cfg(unix)]
    #[test]
    fn polling_reports_new_output_and_retains_a_larger_build_log() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let project = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join("app")).unwrap();
        let wrapper = project.path().join("gradlew");
        let output = "multibyte é ".repeat(550);
        fs::write(
            &wrapper,
            format!("#!/bin/sh\nprintf '%s' '{output}'\nexit 7\n"),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let run = AppRun::start(project.path(), "android", "emulator-5554").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while run.is_running() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(15));
        }
        let mut mobile = Mobile {
            run: Some(run),
            ..Default::default()
        };
        let status = mobile.status();
        assert_eq!(status["build"]["state"], "failed");
        assert_eq!(status["build"]["output_truncated"], true);
        assert!(status["build"]["output"].as_str().unwrap().len() <= MAX_STATUS_LOG);
        assert_eq!(mobile.status()["build"]["output"], "");
        let result = mobile
            .call("mobile_logs", &json!({"source":"build"}), project.path())
            .unwrap();
        let details: Value =
            serde_json::from_str(result[0]["text"].as_str().unwrap()).unwrap();
        let details = details["log"].as_str().unwrap();
        assert!(details.len() > MAX_STATUS_LOG && details.len() <= MAX_LOG);
        assert!(output.ends_with(details));
    }
}
