use crate::common::{DeviceInfo, DevicePlatform, DeviceState, MobileDevice};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Candidate Android SDK roots, most-specific first: the standard env vars, then
/// the per-OS default install location. A typical Android Studio install on
/// Windows does NOT put `adb`/`emulator` on PATH, so relying on PATH alone left
/// the panel showing an empty device list.
fn sdk_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                roots.push(PathBuf::from(v));
            }
        }
    }
    #[cfg(windows)]
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("Android").join("Sdk"));
    }
    #[cfg(target_os = "macos")]
    if let Ok(home) = std::env::var("HOME") {
        roots.push(
            PathBuf::from(home)
                .join("Library")
                .join("Android")
                .join("sdk"),
        );
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Ok(home) = std::env::var("HOME") {
        roots.push(PathBuf::from(home).join("Android").join("Sdk"));
    }
    roots
}

/// Resolve an SDK tool (`adb` / `emulator`) to an absolute path, or fall back to
/// the bare name (PATH lookup) when the SDK can't be located.
fn sdk_tool(name: &str) -> String {
    let subdir = match name {
        "adb" => "platform-tools",
        "emulator" => "emulator",
        _ => "",
    };
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    for root in sdk_roots() {
        let candidate = root.join(subdir).join(&exe);
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    name.to_string() // not found under any SDK root — rely on PATH
}

/// Build a `Command` for an SDK tool that (a) resolves the SDK location instead
/// of relying on PATH, and (b) does not pop a console window on Windows.
///
/// `adb` / `emulator` are console apps; spawning them from the GUI flashes a
/// black console window per call (several when the panel opens). `CREATE_NO_WINDOW`
/// suppresses it — same convention as `proxy.rs` / `palette.rs` (`0x08000000`).
fn quiet_command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(sdk_tool(program));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    cmd
}

pub struct AndroidEmulator {
    pub device_id: String,
    pub grpc_address: String,
}

impl AndroidEmulator {
    pub fn new(device_id: String, grpc_address: String) -> Self {
        Self {
            device_id,
            grpc_address,
        }
    }

    /// Get list of currently running emulator serials from adb devices
    pub fn get_running_serials() -> Vec<String> {
        let output = match quiet_command("adb").arg("devices").output() {
            Ok(out) => out,
            Err(_) => return Vec::new(),
        };

        if !output.status.success() {
            return Vec::new();
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .lines()
            .skip(1) // Skip "List of devices" header
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2
                    && parts[0].starts_with("emulator-")
                    && parts[1] == "device"
                {
                    Some(parts[0].to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Check if an AVD is currently running.
    pub fn is_running(avd_name: &str) -> bool {
        Self::running_serial(avd_name).is_some()
    }

    /// The adb serial (`emulator-<consolePort>`) of the running emulator for
    /// `avd_name`, or `None` if that AVD isn't currently running. Each running
    /// emulator is queried for its AVD name (`adb -s <serial> emu avd name`) so
    /// the right serial is returned even when several emulators run at once.
    pub fn running_serial(avd_name: &str) -> Option<String> {
        for serial in Self::get_running_serials() {
            if let Ok(output) = quiet_command("adb")
                .args(["-s", &serial, "emu", "avd", "name"])
                .output()
            {
                let name = String::from_utf8_lossy(&output.stdout);
                if name.trim() == avd_name
                    || name.lines().next().map(|l| l.trim()) == Some(avd_name)
                {
                    return Some(serial);
                }
            }
        }
        None
    }

    /// Press a hardware keyevent on the device by Android keycode
    /// (e.g. 24 = Volume Up, 25 = Volume Down). Used for the panel's Volume
    /// buttons: the emulator ignores a lone gRPC key-*press* for non-character
    /// keys, so these go through `adb shell input keyevent` (a full down+up).
    pub fn press_keyevent(serial: &str, keycode: i32) -> Result<()> {
        quiet_command("adb")
            .args([
                "-s",
                serial,
                "shell",
                "input",
                "keyevent",
                &keycode.to_string(),
            ])
            .output()?;
        Ok(())
    }

    /// Rotate the device 90° via the emulator console (`emu rotate`). There is
    /// no gRPC equivalent, so the panel's Rotate button uses this.
    pub fn rotate(serial: &str) -> Result<()> {
        quiet_command("adb")
            .args(["-s", serial, "emu", "rotate"])
            .output()?;
        Ok(())
    }

    /// A ready-to-spawn `adb logcat` follow-mode command for `serial`, for the
    /// Device Logs panel: SDK-resolved adb (PATH-independent), no console
    /// window on Windows, `-v time` for a stable line format. The caller pipes
    /// stdout and owns the child's lifetime (kill on panel close).
    pub fn logcat_command(serial: &str) -> Command {
        let mut cmd = quiet_command("adb");
        cmd.args(["-s", serial, "logcat", "-v", "time"]);
        cmd
    }

    /// The gRPC port a running emulator is serving on, read from its discovery
    /// file so the panel connects to the *right* device instead of assuming the
    /// default 8554. Each running emulator writes `<temp>/avd/running/pid_*.ini`
    /// with `port.serial=<consolePort>` and `grpc.port=<port>`; we match on the
    /// serial's console port. `None` (→ caller falls back to 8554) if the file
    /// isn't found, e.g. the emulator predates discovery files.
    pub fn grpc_port(serial: &str) -> Option<u16> {
        let console: u32 = serial.strip_prefix("emulator-")?.parse().ok()?;
        let dir = std::env::temp_dir().join("avd").join("running");
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("ini") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut this_serial: Option<u32> = None;
            let mut this_grpc: Option<u16> = None;
            for line in text.lines() {
                if let Some(v) = line.strip_prefix("port.serial=") {
                    this_serial = v.trim().parse().ok();
                } else if let Some(v) = line.strip_prefix("grpc.port=") {
                    this_grpc = v.trim().parse().ok();
                }
            }
            if this_serial == Some(console) {
                return this_grpc;
            }
        }
        None
    }

    pub fn list_devices() -> Result<Vec<DeviceInfo>> {
        let output = quiet_command("emulator").arg("-list-avds").output()?;

        if !output.status.success() {
            return Err(anyhow!("Failed to list Android AVDs"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let devices = stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| DeviceInfo {
                id: line.trim().to_string(),
                name: line.trim().to_string(),
                platform: DevicePlatform::Android,
                state: DeviceState::Disconnected, // Initial state
                serial: None, // Resolved once running (see running_serial)
            })
            .collect();

        Ok(devices)
    }

    pub fn launch(avd_name: &str) -> Result<()> {
        tracing::info!("Launching Android emulator for AVD: {}", avd_name);

        // Check if already running
        if Self::is_running(avd_name) {
            tracing::info!("AVD {} is already running", avd_name);
            return Ok(());
        }

        // Prefer the host GPU; fall back to software if it can't come up.
        // "auto" is deliberately NOT used: with -no-window (which we always set
        // for gRPC streaming) it silently selects the SwiftShader *software*
        // rasterizer, so the whole guest UI is CPU-rendered and sluggish even on
        // capable GPUs (verified on Windows). "host" renders on the real GPU; a
        // machine without a usable GPU falls back to swiftshader_indirect.
        if Self::spawn_and_wait(avd_name, "host")? {
            return Ok(());
        }
        tracing::warn!(
            "emulator did not come up with `-gpu host`; \
             retrying with software rendering"
        );
        if Self::spawn_and_wait(avd_name, "swiftshader_indirect")? {
            return Ok(());
        }
        Err(anyhow!(
            "emulator {} failed to boot with host or software GPU",
            avd_name
        ))
    }

    /// Spawn the headless emulator with the given `-gpu` mode and wait for it to
    /// register with adb. Returns `Ok(true)` if it came up, `Ok(false)` if the
    /// process exited early (this GPU mode is unsupported on this host, so the
    /// caller should fall back), or `Err` if it could not be spawned at all.
    fn spawn_and_wait(avd_name: &str, gpu: &str) -> Result<bool> {
        let mut child = quiet_command("emulator")
            .arg("-avd")
            .arg(avd_name)
            .arg("-gpu")
            .arg(gpu)
            .arg("-no-boot-anim")
            .arg("-no-skin")
            .arg("-no-window") // Headless: no desktop window, frames via gRPC
            .arg("-grpc")
            .arg("8554") // gRPC endpoint for streamScreenshot + sendTouch
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|e| anyhow!("Failed to spawn emulator: {}", e))?;

        tracing::info!("Emulator spawned for AVD {} (-gpu {})", avd_name, gpu);

        // Poll up to 30s for the emulator to register with adb. If the process
        // exits early, this GPU mode is unsupported here — report failure so the
        // caller can fall back.
        for attempt in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if let Ok(Some(status)) = child.try_wait() {
                tracing::warn!("emulator (-gpu {}) exited early ({})", gpu, status);
                return Ok(false);
            }
            if Self::is_running(avd_name) {
                tracing::info!(
                    "AVD {} running after {}ms (-gpu {})",
                    avd_name,
                    (attempt + 1) * 500,
                    gpu
                );
                return Ok(true);
            }
        }

        // Still alive but slow to boot — don't kill a working launch; the
        // stream's connect-retry waits for it.
        tracing::warn!(
            "AVD {} not confirmed booted in 30s (-gpu {}); proceeding",
            avd_name,
            gpu
        );
        Ok(true)
    }

    pub fn stop(avd_name: &str) -> Result<()> {
        tracing::info!("Attempting to stop Android emulator for AVD: {}", avd_name);

        // Get all running emulator serials
        let running_serials = Self::get_running_serials();

        if running_serials.is_empty() {
            tracing::warn!("No running emulators found");
            return Err(anyhow!("No running emulators found"));
        }

        // Find the emulator that matches this AVD name
        for serial in &running_serials {
            // Query the AVD name for this emulator
            if let Ok(output) = quiet_command("adb")
                .args(["-s", serial, "emu", "avd", "name"])
                .output()
            {
                let name = String::from_utf8_lossy(&output.stdout);
                let name_trimmed =
                    name.lines().next().map(|l| l.trim()).unwrap_or("");

                tracing::debug!(
                    "Emulator {} has AVD name: '{}'",
                    serial,
                    name_trimmed
                );

                if name_trimmed == avd_name {
                    tracing::info!("Found matching emulator {} for AVD {}, sending kill command", serial, avd_name);

                    // Kill this emulator
                    let result = quiet_command("adb")
                        .args(["-s", serial, "emu", "kill"])
                        .output();

                    match result {
                        Ok(output) => {
                            if output.status.success() {
                                tracing::info!(
                                    "Successfully killed emulator {}",
                                    serial
                                );
                                return Ok(());
                            } else {
                                let stderr = String::from_utf8_lossy(&output.stderr);
                                tracing::error!(
                                    "Failed to kill emulator {}: {}",
                                    serial,
                                    stderr
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to execute adb kill command: {}",
                                e
                            );
                        }
                    }
                }
            }
        }

        // If we didn't find a matching AVD, try to kill by partial name match
        tracing::warn!("No exact AVD match found, trying partial match...");
        for serial in &running_serials {
            // Just kill the first one as fallback
            let result = quiet_command("adb")
                .args(["-s", serial, "emu", "kill"])
                .output();

            if let Ok(output) = result {
                if output.status.success() {
                    tracing::info!("Killed emulator {} as fallback", serial);
                    return Ok(());
                }
            }
        }

        Err(anyhow!(
            "Could not find or stop emulator for AVD: {}",
            avd_name
        ))
    }

    pub async fn stream_video(
        &mut self,
    ) -> Result<impl tokio_stream::Stream<Item = Vec<u8>>> {
        // Stub: Returns a stream of empty packets for now to test UI loop
        println!(
            "Starting video stream for Android device: {}",
            self.device_id
        );
        let stream = tokio_stream::iter(vec![vec![0u8; 10], vec![0u8; 10]]);
        Ok(stream)
    }
}

#[async_trait]
impl MobileDevice for AndroidEmulator {
    async fn connect(&mut self) -> Result<()> {
        println!("Connecting to Android Emulator at {}", self.grpc_address);
        // Stub connection logic
        Ok(())
    }

    async fn get_screenshot(&mut self) -> Result<Vec<u8>> {
        println!("Getting screenshot from {}", self.device_id);
        Ok(vec![]) // Return empty bytes for now
    }

    async fn send_touch(&mut self, x: i32, y: i32) -> Result<()> {
        println!("Sending touch to {}: ({}, {})", self.device_id, x, y);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grpc_port_rejects_non_emulator_serials() {
        // No `emulator-<port>` prefix / non-numeric console port → None (the
        // caller then falls back to the default 8554). No device needed.
        assert_eq!(AndroidEmulator::grpc_port("weird"), None);
        assert_eq!(AndroidEmulator::grpc_port("emulator-abc"), None);
        // A well-formed serial for a device that isn't running → None (no
        // discovery file matches the console port).
        assert_eq!(AndroidEmulator::grpc_port("emulator-1"), None);
    }
}
