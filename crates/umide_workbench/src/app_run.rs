use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

type RunResult<T> = Result<T, String>;
const OUTPUT_LIMIT: usize = 60_000;

struct Console {
    running: bool,
    text: String,
}

struct Runner {
    project: PathBuf,
    stop: AtomicBool,
    deadline: Option<Instant>,
    console: Mutex<Console>,
    active: Mutex<Option<Child>>,
}

pub struct AppRun(Arc<Runner>);

fn validate_device(platform: &str, device_id: &str) -> RunResult<()> {
    if !matches!(platform, "ios" | "android") {
        return Err("Unknown device platform.".into());
    }
    if platform == "ios" && !cfg!(target_os = "macos") {
        return Err("Running on an iOS simulator requires macOS.".into());
    }
    if device_id.is_empty()
        || !device_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".:_-[]".contains(c))
        || device_id.starts_with('-')
    {
        return Err("Select a running device before starting the app.".into());
    }
    Ok(())
}

pub fn device_logs(platform: &str, device_id: &str) -> RunResult<String> {
    validate_device(platform, device_id)?;
    let reader = Runner::new(
        std::env::temp_dir(),
        Some(Instant::now() + Duration::from_secs(15)),
    );
    let (program, args): (&str, &[&str]) = match platform {
        "ios" => (
            "xcrun",
            &[
                "simctl", "spawn", device_id, "log", "show", "--last", "15s",
                "--style", "compact",
            ],
        ),
        _ => (
            "adb",
            &["-s", device_id, "logcat", "-d", "-t", "200", "-v", "brief"],
        ),
    };
    let output = reader
        .command(program, &args.iter().map(OsStr::new).collect::<Vec<_>>())
        .map_err(|error| {
            format!("{error}\n{}", reader.console.lock().unwrap().text)
        })?;
    let mut lines = output.lines().rev().take(200).collect::<Vec<_>>();
    lines.reverse();
    Ok(lines.join("\n"))
}

impl AppRun {
    pub fn start(
        project: &Path,
        platform: &str,
        device_id: &str,
    ) -> RunResult<Self> {
        validate_device(platform, device_id)?;
        let project = project.canonicalize().map_err(|e| e.to_string())?;
        let plan = Plan::detect(&project, platform)?;
        let runner = Arc::new(Runner::new(project, None));
        let worker = runner.clone();
        let platform = platform.to_owned();
        let device_id = device_id.to_owned();
        thread::Builder::new()
            .name("mobile-app-run".into())
            .spawn(move || {
                let result = plan.run(&worker, &platform, &device_id);
                let message = match result {
                    _ if worker.cancelled() => "\n[Run stopped]\n".into(),
                    Ok(()) => "\n[Command finished]\n".into(),
                    Err(error) => format!("\n{error}\n"),
                };
                worker.append(&message);
                worker.console.lock().unwrap().running = false;
            })
            .map_err(|e| e.to_string())?;
        Ok(Self(runner))
    }

    pub fn snapshot(&self) -> (bool, String) {
        let state = self.0.console.lock().unwrap();
        (state.running, state.text.clone())
    }

    pub fn stop(&self) {
        self.0.stop.store(true, Ordering::Release);
        let mut active = self.0.active.lock().unwrap();
        let Some(child) = active.as_mut() else {
            return;
        };
        if matches!(child.try_wait(), Ok(None)) {
            terminate(child);
        }
    }
}

impl Drop for AppRun {
    fn drop(&mut self) {
        self.stop();
    }
}

fn extend_bounded(text: &mut String, chunk: &str) {
    text.push_str(chunk);
    let mut cut = text.len().saturating_sub(OUTPUT_LIMIT);
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    if cut > 0 {
        text.drain(..cut);
    }
}

impl Runner {
    fn new(project: PathBuf, deadline: Option<Instant>) -> Self {
        Self {
            project,
            deadline,
            stop: AtomicBool::new(false),
            console: Mutex::new(Console {
                running: true,
                text: String::new(),
            }),
            active: Mutex::new(None),
        }
    }

    fn cancelled(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn append(&self, chunk: &str) {
        extend_bounded(&mut self.console.lock().unwrap().text, chunk);
    }

    fn command(
        &self,
        program: impl AsRef<OsStr>,
        args: &[&OsStr],
    ) -> RunResult<String> {
        if self.cancelled() {
            return Err("Run stopped.".into());
        }
        let program = program.as_ref();
        let name = program.to_string_lossy();
        self.append(&format!(
            "$ {} {}\n",
            name,
            args.iter()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        ));
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .current_dir(&self.project)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(path) = tool_path() {
            cmd.env("PATH", path);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let (tx, rx) = mpsc::sync_channel(40);
        {
            // Register the child under the same lock used by stop(). Closing
            // a window while a build starts must never leave it running.
            let mut active = self.active.lock().unwrap();
            if self.cancelled() {
                return Err("Run stopped.".into());
            }
            let mut child = cmd
                .spawn()
                .map_err(|error| format!("Cannot start {name}: {error}"))?;
            pipe(child.stdout.take().unwrap(), true, tx.clone());
            pipe(child.stderr.take().unwrap(), false, tx);
            *active = Some(child);
        }
        let mut output = String::new();
        let mut status = None;
        let mut exited_at = None;
        loop {
            if status.is_none() {
                if self
                    .deadline
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    if let Some(mut child) = self.active.lock().unwrap().take() {
                        if matches!(child.try_wait(), Ok(None)) {
                            terminate(&mut child);
                        }
                        let _ = child.wait();
                    }
                    return Err(format!("{name} timed out."));
                }
                status = self
                    .active
                    .lock()
                    .unwrap()
                    .as_mut()
                    .expect("registered child")
                    .try_wait()
                    .map_err(|error| error.to_string())?;
                if status.is_some() {
                    exited_at = Some(Instant::now());
                }
            }
            if exited_at.is_some_and(|t| t.elapsed() > Duration::from_secs(1)) {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok((is_stdout, chunk)) => {
                    let chunk = String::from_utf8_lossy(&chunk);
                    self.append(&chunk);
                    if is_stdout {
                        extend_bounded(&mut output, &chunk);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) if status.is_some() => {
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    thread::sleep(Duration::from_millis(50))
                }
                Err(_) => (),
            }
        }
        self.active.lock().unwrap().take();
        let status = status.expect("run exited");
        if !status.success() {
            return Err(format!("{name} exited with {status}."));
        }
        Ok(output)
    }
}

fn pipe(
    mut reader: impl Read + Send + 'static,
    stdout: bool,
    tx: mpsc::SyncSender<(bool, Vec<u8>)>,
) {
    thread::spawn(move || {
        let mut bytes = [0; 4096];
        while let Ok(size) = reader.read(&mut bytes) {
            if size == 0 || tx.send((stdout, bytes[..size].to_vec())).is_err() {
                break;
            }
        }
    });
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        // Each build is placed in its own process group before exec. Stop
        // wrappers and the build tools they spawned together.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .status();
    }
    let _ = child.kill();
}

fn tool_path() -> Option<OsString> {
    let mut paths = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    #[cfg(unix)]
    {
        if let Some(home) = std::env::var_os("HOME") {
            paths.push(Path::new(&home).join(".local/bin"));
        }
        paths.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
    }
    let sdk = std::env::var_os("ANDROID_HOME")
        .or_else(|| std::env::var_os("ANDROID_SDK_ROOT"))
        .map(PathBuf::from)
        .or_else(|| {
            directories::BaseDirs::new().map(|dirs| {
                if cfg!(target_os = "macos") {
                    dirs.home_dir().join("Library/Android/sdk")
                } else if cfg!(windows) {
                    dirs.data_local_dir().join("Android/Sdk")
                } else {
                    dirs.home_dir().join("Android/Sdk")
                }
            })
        });
    if let Some(sdk) = sdk {
        paths.push(sdk.join("platform-tools"));
    }
    std::env::join_paths(paths).ok()
}

enum Plan {
    Flutter,
    Expo,
    ReactNative,
    Gradle,
    Xcode(PathBuf),
}

impl Plan {
    fn detect(root: &Path, platform: &str) -> RunResult<Self> {
        if fs::read_to_string(root.join("pubspec.yaml")).is_ok_and(|s| {
            s.lines().any(|line| {
                let line = line.split('#').next().unwrap_or("").trim();
                line.starts_with("flutter:")
                    || line.split_once(':').is_some_and(|(k, v)| {
                        k.trim() == "sdk"
                            && v.trim().trim_matches(['\'', '"']) == "flutter"
                    })
            })
        }) {
            return Ok(Self::Flutter);
        }
        if let Ok(text) = fs::read_to_string(root.join("package.json")) {
            let json = serde_json::from_str::<Value>(&text)
                .map_err(|e| format!("Invalid package.json: {e}"))?;
            let has = |package| {
                ["dependencies", "devDependencies"]
                    .iter()
                    .any(|field| json[field].get::<&str>(package).is_some())
            };
            if has("expo") {
                return Ok(Self::Expo);
            }
            if has("react-native") {
                return Ok(Self::ReactNative);
            }
        }
        if platform == "android"
            && root
                .join(if cfg!(windows) {
                    "gradlew.bat"
                } else {
                    "gradlew"
                })
                .is_file()
        {
            return Ok(Self::Gradle);
        }
        if platform == "ios" {
            for ext in ["xcworkspace", "xcodeproj"] {
                let candidates = matching_files(root, ext)?;
                match candidates.as_slice() {
                    [file] => return Ok(Self::Xcode(file.clone())),
                    [] => (),
                    _ => {
                        return Err(format!(
                            "Multiple .{ext} files found. Open the folder of a single iOS application."
                        ));
                    }
                }
            }
        }
        Err(format!(
            "Open a Flutter, Expo, React Native or native {} app folder to run it on this device.",
            if platform == "ios" {
                "Xcode"
            } else {
                "Android Gradle"
            }
        ))
    }

    fn run(&self, worker: &Runner, platform: &str, device: &str) -> RunResult<()> {
        let ios = platform == "ios";
        let expo_name;
        let device = if matches!(self, Self::Expo) && !ios {
            expo_name = expo_android_target(worker, device)?;
            OsStr::new(&expo_name)
        } else {
            OsStr::new(device)
        };
        match self {
            Self::Flutter => {
                worker.command(
                    if cfg!(windows) {
                        "flutter.bat"
                    } else {
                        "flutter"
                    },
                    &["run".as_ref(), "-d".as_ref(), device],
                )?;
            }
            Self::Expo | Self::ReactNative => {
                let args = if matches!(self, Self::Expo) {
                    [
                        "--no-install",
                        "expo",
                        if ios { "run:ios" } else { "run:android" },
                        "--device",
                    ]
                } else {
                    [
                        "--no-install",
                        "react-native",
                        if ios { "run-ios" } else { "run-android" },
                        if ios { "--udid" } else { "--device" },
                    ]
                };
                let mut args = args.map(OsStr::new).to_vec();
                args.push(device);
                worker
                    .command(if cfg!(windows) { "npx.cmd" } else { "npx" }, &args)?;
            }
            Self::Gradle => run_gradle(worker, device)?,
            Self::Xcode(project) => run_xcode(worker, device, project)?,
        }
        Ok(())
    }
}

// Older Expo CLIs choose the first device with a matching display name. Use
// a name only when it identifies the requested serial unambiguously. Recent
// CLIs also accept the serial, which stays exact for duplicate AVD instances.
fn expo_android_target(worker: &Runner, selected: &str) -> RunResult<String> {
    let output = worker.command("adb", &["devices".as_ref(), "-l".as_ref()])?;
    let mut devices = Vec::new();
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        let (Some(serial), Some("device")) = (fields.next(), fields.next()) else {
            continue;
        };
        let name = if serial.starts_with("emulator-") {
            let args = ["-s", serial, "emu", "avd", "name"].map(OsStr::new);
            let output = worker.command("adb", &args)?;
            output.lines().next().unwrap_or_default().trim().to_owned()
        } else {
            fields
                .find_map(|f| f.strip_prefix("model:"))
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Device {serial}"))
        };
        devices.push((serial, name));
    }
    expo_target_name(&devices, selected)
}

fn expo_target_name(
    devices: &[(&str, String)],
    selected: &str,
) -> RunResult<String> {
    let (_, name) = devices
        .iter()
        .find(|(serial, _)| *serial == selected)
        .ok_or("The selected Android device is no longer connected.")?;
    let unique = !name.is_empty()
        && !name.starts_with('-')
        && devices.iter().all(|(serial, other)| {
            *serial == selected || (other != name && *serial != name)
        });
    Ok(if unique {
        name.clone()
    } else {
        selected.into()
    })
}

fn matching_files(root: &Path, ext: &str) -> RunResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension() == Some(OsStr::new(ext)) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn run_gradle(worker: &Runner, device: &OsStr) -> RunResult<()> {
    if !worker.project.join("app").is_dir() {
        return Err("This launcher expects an Android application module named app with a debug variant.".into());
    }
    let wrapper = worker.project.join(if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    });
    worker.command(
        wrapper,
        &["--no-daemon".as_ref(), ":app:assembleDebug".as_ref()],
    )?;
    let dir = worker.project.join("app/build/outputs/apk/debug");
    let text = fs::read(dir.join("output-metadata.json")).map_err(|_| "The debug build has no single app/build/outputs/apk/debug variant. Product flavors require a separate run configuration.".to_owned())?;
    let metadata: Value =
        serde_json::from_slice(&text).map_err(|e| e.to_string())?;
    let package = metadata["applicationId"]
        .as_str()
        .ok_or("Missing APK applicationId.")?;
    let artifacts = metadata["elements"]
        .as_array()
        .ok_or("Missing APK artifacts.")?;
    let [artifact] = artifacts.as_slice() else {
        return Err("This launcher requires a single debug APK.".into());
    };
    let apk = artifact["outputFile"]
        .as_str()
        .ok_or("Missing debug APK filename.")?;
    let apk = dir.join(apk).canonicalize().map_err(|e| e.to_string())?;
    if !apk.starts_with(dir.canonicalize().map_err(|e| e.to_string())?)
        || apk.extension() != Some("apk".as_ref())
    {
        return Err("Invalid debug APK output path.".into());
    }
    if package.is_empty()
        || !package
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._".contains(&c))
    {
        return Err("Invalid Android application ID.".into());
    }
    worker.command(
        "adb",
        &[
            "-s".as_ref(),
            device,
            "install".as_ref(),
            "-r".as_ref(),
            apk.as_os_str(),
        ],
    )?;
    worker.command(
        "adb",
        &[
            "-s".as_ref(),
            device,
            "shell".as_ref(),
            "monkey".as_ref(),
            "-p".as_ref(),
            package.as_ref(),
            "-c".as_ref(),
            "android.intent.category.LAUNCHER".as_ref(),
            "1".as_ref(),
        ],
    )?;
    Ok(())
}

fn run_xcode(worker: &Runner, device: &OsStr, project: &Path) -> RunResult<()> {
    let kind = if project.extension() == Some("xcworkspace".as_ref()) {
        "workspace"
    } else {
        "project"
    };
    let flag = format!("-{kind}");
    let list = worker.command(
        "xcodebuild",
        &[
            flag.as_ref(),
            project.as_os_str(),
            "-list".as_ref(),
            "-json".as_ref(),
        ],
    )?;
    let list: Value = serde_json::from_str(&list)
        .map_err(|e| format!("Cannot read Xcode schemes: {e}"))?;
    let schemes = list[kind]["schemes"]
        .as_array()
        .ok_or("No schemes found.")?;
    let name = project.file_stem().and_then(OsStr::to_str).unwrap_or("");
    let scheme = schemes.iter().filter_map(Value::as_str).find(|s| *s == name)
        .or_else(|| (schemes.len() == 1).then(|| schemes[0].as_str()).flatten())
        .ok_or("Multiple Xcode schemes found. The app scheme must match the project or workspace filename.")?;
    let derived = tempfile::Builder::new()
        .prefix("umide-xcode-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let destination =
        format!("platform=iOS Simulator,id={}", device.to_string_lossy());
    worker.command(
        "xcodebuild",
        &[
            flag.as_ref(),
            project.as_os_str(),
            "-scheme".as_ref(),
            scheme.as_ref(),
            "-configuration".as_ref(),
            "Debug".as_ref(),
            "-destination".as_ref(),
            destination.as_ref(),
            "-derivedDataPath".as_ref(),
            derived.path().as_os_str(),
            "build".as_ref(),
        ],
    )?;
    let apps = matching_files(
        &derived.path().join("Build/Products/Debug-iphonesimulator"),
        "app",
    )?;
    let app = apps.iter().find(|p| p.file_stem() == Some(scheme.as_ref())).or_else(|| (apps.len() == 1).then(|| &apps[0])).ok_or("Could not identify a single simulator application in the build products.")?;
    let id = worker.command(
        "/usr/libexec/PlistBuddy",
        &[
            "-c".as_ref(),
            "Print CFBundleIdentifier".as_ref(),
            app.join("Info.plist").as_os_str(),
        ],
    )?;
    worker.command(
        "xcrun",
        &[
            "simctl".as_ref(),
            "install".as_ref(),
            device,
            app.as_os_str(),
        ],
    )?;
    worker.command(
        "xcrun",
        &[
            "simctl".as_ref(),
            "launch".as_ref(),
            device,
            id.trim().as_ref(),
        ],
    )?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn expo_targets_the_selected_serial_when_avd_names_collide() {
        let mut devices = vec![("emulator-5554", "Pixel".into())];
        assert_eq!(
            expo_target_name(&devices, "emulator-5554").unwrap(),
            "Pixel"
        );
        devices.push(("emulator-5556", "Pixel".into()));
        assert_eq!(
            expo_target_name(&devices, "emulator-5556").unwrap(),
            "emulator-5556"
        );
        assert!(expo_target_name(&devices, "emulator-5558").is_err());
    }

    #[test]
    fn stop_interrupts_the_running_gradle_process_and_its_children() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("app")).unwrap();
        let wrapper = dir.path().join("gradlew");
        fs::write(
            &wrapper,
            "#!/bin/sh\n(touch started; sleep 1; touch survived) &\nwait\n",
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        let run = AppRun::start(dir.path(), "android", "emulator-5554").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !dir.path().join("started").exists() {
            assert!(Instant::now() < deadline, "Gradle fixture did not start");
            thread::sleep(Duration::from_millis(20));
        }
        run.stop();
        while run.snapshot().0 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        assert!(run.snapshot().1.contains("[Run stopped]"));
        thread::sleep(Duration::from_millis(1200));
        assert!(!dir.path().join("survived").exists());
    }
}
