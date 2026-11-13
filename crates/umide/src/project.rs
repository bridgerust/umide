//! Mobile project-type detection.
//!
//! UMIDE is a mobile-first IDE, so it should *know* when the open workspace is
//! a React Native or Flutter app instead of treating every folder the same.
//! Local project detection feeds the status bar, AI context, and Run on Device.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use umide_emulator::{DeviceInfo, DevicePlatform, DeviceState};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MobileRunProfile {
    /// App directory relative to the workspace. None uses the workspace root.
    pub app_dir: Option<PathBuf>,
    pub device: Option<MobileRunDevice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileRunDevice {
    pub platform: DevicePlatform,
    /// AVD name or simulator UDID. An Android serial can change after reboot.
    pub id: String,
}

impl MobileRunDevice {
    pub fn matches(&self, device: &DeviceInfo) -> bool {
        self.platform == device.platform && self.id == device.id
    }
}

impl From<&DeviceInfo> for MobileRunDevice {
    fn from(device: &DeviceInfo) -> Self {
        Self {
            platform: device.platform,
            id: device.id.clone(),
        }
    }
}

pub fn mobile_app_directory(
    workspace: &Path,
    app_dir: Option<&Path>,
) -> Option<PathBuf> {
    let root = workspace.canonicalize().ok()?;
    let app = root
        .join(app_dir.unwrap_or_else(|| Path::new("")))
        .canonicalize()
        .ok()?;
    (app.starts_with(&root) && app.is_dir()).then_some(app)
}

pub fn platform_label(platform: DevicePlatform) -> &'static str {
    match platform {
        DevicePlatform::Android => "Android",
        DevicePlatform::Ios => "iOS",
    }
}

pub fn run_device_identifier(device: &DeviceInfo) -> Option<&str> {
    if device.state != DeviceState::Running {
        return None;
    }
    let identifier = match device.platform {
        DevicePlatform::Android => device.serial.as_deref()?,
        DevicePlatform::Ios => &device.id,
    };
    (!identifier.is_empty()).then_some(identifier)
}

pub fn select_run_device<'a>(
    devices: &'a [DeviceInfo],
    platform: DevicePlatform,
    preferred: Option<&MobileRunDevice>,
) -> Option<&'a DeviceInfo> {
    devices.iter().find(|device| {
        device.platform == platform
            && preferred.is_none_or(|preferred| preferred.matches(device))
            && run_device_identifier(device).is_some()
    })
}

/// The kind of mobile project the workspace holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectKind {
    ReactNative,
    Flutter,
}

impl ProjectKind {
    /// Short human label (status bar, tooltips).
    pub fn label(self) -> &'static str {
        match self {
            ProjectKind::ReactNative => "React Native",
            ProjectKind::Flutter => "Flutter",
        }
    }
}

/// Detect the project kind from workspace marker files:
/// - Flutter: a `pubspec.yaml` with a Flutter dependency or an SDK constraint.
/// - React Native: a `package.json` with a `react-native` dependency (also
///   matched by Expo apps, which depend on `react-native`).
///
/// Flutter is checked first: an RN dependency can appear in a Flutter
/// monorepo's tooling, but a Flutter `pubspec.yaml` at the root is decisive.
pub fn detect_project_kind(workspace: &Path) -> Option<ProjectKind> {
    let pubspec = workspace.join("pubspec.yaml");
    if let Ok(content) = std::fs::read_to_string(&pubspec) {
        if content.lines().any(|line| {
            let line = line.split('#').next().unwrap_or_default().trim();
            line.starts_with("flutter:")
                || line.split_once(':').is_some_and(|(key, value)| {
                    key.trim() == "sdk"
                        && value.trim().trim_matches(['\'', '"']) == "flutter"
                })
        }) {
            return Some(ProjectKind::Flutter);
        }
    }

    has_package_dependency(workspace, "react-native")
        .then_some(ProjectKind::ReactNative)
}

/// Whether an RN workspace is an Expo app (`"expo"` in package.json): Expo
/// projects are run through `npx expo run:*` — they may not even have the bare
/// `android/`/`ios/` folders `react-native run-*` requires.
pub fn is_expo(workspace: &Path) -> bool {
    has_package_dependency(workspace, "expo")
}

fn has_package_dependency(workspace: &Path, dependency: &str) -> bool {
    let Ok(content) = std::fs::read_to_string(workspace.join("package.json")) else {
        return false;
    };
    let Ok(package) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    ["dependencies", "devDependencies"].iter().any(|field| {
        package
            .get(field)
            .and_then(|deps| deps.get(dependency))
            .is_some()
    })
}

/// A workspace path as a plain string for a child process's working directory.
/// Strips the Windows extended-length ("verbatim", `\\?\`) prefix that
/// `canonicalize()` produces: `cmd.exe` and Gradle's `gradlew.bat` refuse to
/// run with a `\\?\`-prefixed CWD (`'gradlew.bat' is not recognized …`), which
/// silently breaks Run on Device. `\\?\UNC\server\share` → `\\server\share`.
pub fn plain_cwd(path: &Path) -> String {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.into_owned()
    }
}

/// The command "▶ Run on device" executes in UMIDE's terminal: build, install
/// and launch the app on the embedded emulator/simulator — the step that
/// otherwise sends people to Android Studio / Xcode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOnDevice {
    /// Terminal tab title, e.g. `Run · React Native · Android`.
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
}

/// Compose the run command for the framework and chosen platform.
pub fn run_on_device_command(
    kind: ProjectKind,
    expo: bool,
    platform: DevicePlatform,
    device: Option<&DeviceInfo>,
) -> Result<RunOnDevice, &'static str> {
    let device = device.filter(|d| d.platform == platform);
    let identifier = device.and_then(run_device_identifier);
    let (stack_label, program, base_args, target_flag) = match (kind, expo, platform)
    {
        (ProjectKind::Flutter, _, _) => ("Flutter", "flutter", &["run"][..], "-d"),
        (ProjectKind::ReactNative, true, DevicePlatform::Android) => {
            ("Expo", "npx", &["expo", "run:android"][..], "--device")
        }
        (ProjectKind::ReactNative, true, DevicePlatform::Ios) => {
            ("Expo", "npx", &["expo", "run:ios"][..], "--device")
        }
        (ProjectKind::ReactNative, false, DevicePlatform::Android) => (
            "React Native",
            "npx",
            &["react-native", "run-android"][..],
            "--deviceId",
        ),
        (ProjectKind::ReactNative, false, DevicePlatform::Ios) => (
            "React Native",
            "npx",
            &["react-native", "run-ios"][..],
            "--udid",
        ),
    };
    let mut args: Vec<String> =
        base_args.iter().map(|arg| (*arg).to_owned()).collect();
    if let Some(identifier) = identifier {
        // Older Expo SDKs match an Android emulator by its AVD name only.
        let target = match (kind, expo, device) {
            (ProjectKind::ReactNative, true, Some(device))
                if platform == DevicePlatform::Android =>
            {
                device.id.as_str()
            }
            _ => identifier,
        };
        if cfg!(target_os = "windows")
            && (target.is_empty()
                || !target
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-:".contains(&b)))
        {
            return Err(
                "The device name contains characters that Windows cannot pass to the run command.",
            );
        }
        args.push(target_flag.to_owned());
        args.push(target.to_owned());
    }
    // On Windows, `npx` and `flutter` are batch shims (npx.cmd / flutter.bat):
    // the run terminal spawns the program directly via CreateProcess, which
    // only executes real executables — the spawn dies instantly and silently
    // (found live: ▶ produced a ConPTY and nothing else; no node, no gradle).
    // Route through `cmd /C` with fixed tool words and validated target IDs.
    let (program, args) = if cfg!(target_os = "windows") {
        let mut wrapped = vec!["/C".to_string(), program.to_string()];
        wrapped.extend(args);
        ("cmd".to_string(), wrapped)
    } else {
        (program.to_string(), args)
    };
    Ok(RunOnDevice {
        name: format!("Run · {stack_label} · {}", platform_label(platform)),
        program,
        args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_workspace(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("umide-project-detect-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn android_dev() -> umide_emulator::DeviceInfo {
        umide_emulator::DeviceInfo {
            id: "Pixel_9a".into(),
            name: "Pixel 9a".into(),
            platform: umide_emulator::DevicePlatform::Android,
            state: umide_emulator::DeviceState::Running,
            serial: Some("emulator-5554".into()),
        }
    }
    fn ios_dev() -> umide_emulator::DeviceInfo {
        umide_emulator::DeviceInfo {
            id: "UDID-42".into(),
            name: "iPhone 16".into(),
            platform: umide_emulator::DevicePlatform::Ios,
            state: umide_emulator::DeviceState::Running,
            serial: None,
        }
    }

    /// The (program, args) the platform actually spawns: Windows wraps batch
    /// shims (npx.cmd / flutter.bat) in `cmd /C` because CreateProcess only
    /// executes real executables; elsewhere the tool runs bare.
    fn spawned(program: &str, args: &[&str]) -> (String, Vec<String>) {
        if cfg!(target_os = "windows") {
            let mut a = vec!["/C".to_string(), program.to_string()];
            a.extend(args.iter().map(|s| s.to_string()));
            ("cmd".to_string(), a)
        } else {
            (
                program.to_string(),
                args.iter().map(|s| s.to_string()).collect(),
            )
        }
    }

    fn assert_cmd(
        run: &Result<RunOnDevice, &'static str>,
        program: &str,
        args: &[&str],
    ) {
        let run = run.as_ref().unwrap();
        let (p, a) = spawned(program, args);
        assert_eq!(run.program, p);
        assert_eq!(run.args, a);
    }

    #[test]
    fn run_command_matrix() {
        use umide_emulator::DevicePlatform::{Android, Ios};
        // Flutter pins the chosen-platform device by id (adb serial / sim UDID).
        let f = run_on_device_command(
            ProjectKind::Flutter,
            false,
            Android,
            Some(&android_dev()),
        );
        assert_cmd(&f, "flutter", &["run", "-d", "emulator-5554"]);
        let fi = run_on_device_command(
            ProjectKind::Flutter,
            false,
            Ios,
            Some(&ios_dev()),
        );
        assert_cmd(&fi, "flutter", &["run", "-d", "UDID-42"]);
        // No device: flutter's own picker handles it in the terminal.
        let fnone =
            run_on_device_command(ProjectKind::Flutter, false, Android, None);
        assert_cmd(&fnone, "flutter", &["run"]);

        // Bare RN pins Android by adb serial and iOS by simulator UDID.
        let rn = run_on_device_command(
            ProjectKind::ReactNative,
            false,
            Android,
            Some(&android_dev()),
        );
        assert_cmd(
            &rn,
            "npx",
            &["react-native", "run-android", "--deviceId", "emulator-5554"],
        );
        let rni = run_on_device_command(
            ProjectKind::ReactNative,
            false,
            Ios,
            Some(&ios_dev()),
        );
        assert_cmd(
            &rni,
            "npx",
            &["react-native", "run-ios", "--udid", "UDID-42"],
        );

        // Expo apps go through `npx expo run:*`.
        let ex = run_on_device_command(
            ProjectKind::ReactNative,
            true,
            Android,
            Some(&android_dev()),
        );
        assert_cmd(&ex, "npx", &["expo", "run:android", "--device", "Pixel_9a"]);
        let ex = ex.unwrap();
        assert!(ex.name.contains("Expo") && ex.name.contains("Android"));
        let expo_ios = run_on_device_command(
            ProjectKind::ReactNative,
            true,
            Ios,
            Some(&ios_dev()),
        );
        assert_cmd(
            &expo_ios,
            "npx",
            &["expo", "run:ios", "--device", "UDID-42"],
        );

        // Explicit iOS while viewing an ANDROID device: the mismatched device
        // is ignored, so no `--udid` — the tool boots/selects a simulator.
        let ios_no_pin = run_on_device_command(
            ProjectKind::ReactNative,
            false,
            Ios,
            Some(&android_dev()),
        );
        assert_cmd(&ios_no_pin, "npx", &["react-native", "run-ios"]);
        assert!(ios_no_pin.unwrap().name.contains("iOS"));

        // No device, explicit Android.
        let none =
            run_on_device_command(ProjectKind::ReactNative, false, Android, None);
        assert_cmd(&none, "npx", &["react-native", "run-android"]);
    }

    /// The Windows spawn shape must be exactly `cmd /C <tool> <args…>` — the
    /// run terminal's CreateProcess can't execute .cmd/.bat shims directly.
    #[cfg(target_os = "windows")]
    #[test]
    fn run_command_windows_wraps_in_cmd() {
        use umide_emulator::DevicePlatform::Android;
        let rn = run_on_device_command(
            ProjectKind::ReactNative,
            false,
            Android,
            Some(&android_dev()),
        )
        .unwrap();
        assert_eq!(rn.program, "cmd");
        assert_eq!(rn.args[..2], ["/C".to_string(), "npx".to_string()]);
    }

    #[test]
    fn plain_cwd_strips_verbatim_prefix() {
        use std::path::Path;
        assert_eq!(
            plain_cwd(Path::new(r"\\?\C:\Users\me\app")),
            r"C:\Users\me\app"
        );
        assert_eq!(plain_cwd(Path::new(r"C:\Users\me\app")), r"C:\Users\me\app");
        assert_eq!(
            plain_cwd(Path::new(r"\\?\UNC\srv\share\app")),
            r"\\srv\share\app"
        );
    }

    #[test]
    fn run_profile_supports_a_nested_app_and_stable_device_id() {
        let root = tmp_workspace("monorepo");
        let relative = Path::new("apps/mobile");
        let app = root.join(relative);
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("package.json"),
            r#"{"dependencies":{"react-native":"0.76","expo":"52"}}"#,
        )
        .unwrap();
        let first = android_dev();
        let profile = MobileRunProfile {
            app_dir: Some(relative.to_path_buf()),
            device: Some((&first).into()),
        };
        let json = serde_json::to_string(&profile).unwrap();
        let restored: MobileRunProfile = serde_json::from_str(&json).unwrap();
        let directory =
            mobile_app_directory(&root, restored.app_dir.as_deref()).unwrap();
        assert_eq!(directory, app.canonicalize().unwrap());
        assert_eq!(
            detect_project_kind(&directory),
            Some(ProjectKind::ReactNative)
        );
        assert!(is_expo(&directory));
        assert!(mobile_app_directory(&root, Some(Path::new(".."))).is_none());

        let mut rebooted = first;
        rebooted.serial = Some("emulator-5560".into());
        assert!(restored.device.unwrap().matches(&rebooted));
        assert_eq!(run_device_identifier(&rebooted), Some("emulator-5560"));
    }

    #[test]
    fn expo_probe_reads_package_json() {
        let ws = tmp_workspace("expo");
        std::fs::write(
            ws.join("package.json"),
            r#"{ "dependencies": { "expo": "~52.0.0", "react-native": "0.76.0" } }"#,
        )
        .unwrap();
        assert!(is_expo(&ws));
        let ws2 = tmp_workspace("bare-rn");
        std::fs::write(
            ws2.join("package.json"),
            r#"{ "dependencies": { "react-native": "0.75.4" } }"#,
        )
        .unwrap();
        assert!(!is_expo(&ws2));
        assert!(!is_expo(&tmp_workspace("empty")));
    }

    #[test]
    fn detects_react_native_from_package_json() {
        let ws = tmp_workspace("rn");
        std::fs::write(
            ws.join("package.json"),
            r#"{ "dependencies": { "react": "18.3.1", "react-native": "0.75.4" } }"#,
        )
        .unwrap();
        assert_eq!(detect_project_kind(&ws), Some(ProjectKind::ReactNative));
    }

    #[test]
    fn detects_flutter_from_pubspec() {
        let ws = tmp_workspace("flutter");
        std::fs::write(
            ws.join("pubspec.yaml"),
            "name: demo\ndependencies:\n  flutter:\n    sdk: flutter\n",
        )
        .unwrap();
        assert_eq!(detect_project_kind(&ws), Some(ProjectKind::Flutter));
    }

    #[test]
    fn plain_node_or_dart_projects_are_not_mobile() {
        let ws = tmp_workspace("node");
        std::fs::write(
            ws.join("package.json"),
            r#"{ "dependencies": { "react": "18.3.1", "react-native-web": "0.19.0" } }"#,
        )
        .unwrap();
        // react-native-web alone is not an RN app (quote-delimited exact name).
        assert_eq!(detect_project_kind(&ws), None);

        let ws2 = tmp_workspace("dart");
        std::fs::write(
            ws2.join("pubspec.yaml"),
            "name: flutter_tools\ndescription: tooling for flutter\n# flutter: this is a comment\n",
        )
        .unwrap();
        assert_eq!(detect_project_kind(&ws2), None);
    }

    #[test]
    fn run_device_matches_platform_and_never_substitutes_a_saved_target() {
        let mut disconnected = android_dev();
        disconnected.state = DeviceState::Disconnected;
        disconnected.serial = None;
        let mut other = android_dev();
        other.id = "Pixel_7".into();
        other.serial = Some("emulator-5560".into());
        let preferred = MobileRunDevice::from(&disconnected);
        let devices = [ios_dev(), disconnected, other];
        assert!(
            select_run_device(&devices, DevicePlatform::Android, Some(&preferred))
                .is_none()
        );
        let target =
            select_run_device(&devices, DevicePlatform::Android, None).unwrap();
        assert_eq!(run_device_identifier(target), Some("emulator-5560"));
    }

    #[test]
    fn empty_workspace_detects_nothing() {
        let ws = tmp_workspace("empty");
        assert_eq!(detect_project_kind(&ws), None);
    }

    #[test]
    fn flutter_wins_in_a_mixed_root() {
        let ws = tmp_workspace("mixed");
        std::fs::write(
            ws.join("pubspec.yaml"),
            "dependencies:\n  flutter:\n    sdk: flutter\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("package.json"),
            r#"{ "dependencies": { "react-native": "0.75.4" } }"#,
        )
        .unwrap();
        assert_eq!(detect_project_kind(&ws), Some(ProjectKind::Flutter));
    }
}
