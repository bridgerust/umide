//! Mobile project-type detection.
//!
//! UMIDE is a mobile-first IDE, so it should *know* when the open workspace is
//! a React Native or Flutter app instead of treating every folder the same.
//! Detection is intentionally cheap and offline (two file probes, no shelling
//! out): it runs once per workspace open, on local workspaces only. The result
//! feeds the status-bar badge and,
//! later, the AI agent's project context and run-on-device commands.

use std::path::Path;

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
/// - Flutter: a `pubspec.yaml` whose content mentions `flutter` (dependency or
///   sdk section) — plain Dart packages without Flutter are not mobile apps.
/// - React Native: a `package.json` with a `react-native` dependency (also
///   matched by Expo apps, which depend on `react-native`).
///
/// Flutter is checked first: an RN dependency can appear in a Flutter
/// monorepo's tooling, but a Flutter `pubspec.yaml` at the root is decisive.
pub fn detect_project_kind(workspace: &Path) -> Option<ProjectKind> {
    let pubspec = workspace.join("pubspec.yaml");
    if let Ok(content) = std::fs::read_to_string(&pubspec) {
        if content.contains("flutter") {
            return Some(ProjectKind::Flutter);
        }
    }

    let package_json = workspace.join("package.json");
    if let Ok(content) = std::fs::read_to_string(&package_json) {
        // Cheap dependency probe: the exact `"react-native"` package name in
        // quotes (as a key in dependencies/devDependencies). Avoids a JSON
        // parse for a hot startup path while not matching e.g.
        // `react-native-web`'s substring (the quote closes the name).
        if content.contains("\"react-native\"") {
            return Some(ProjectKind::ReactNative);
        }
    }

    None
}

/// Whether an RN workspace is an Expo app (`"expo"` in package.json): Expo
/// projects are run through `npx expo run:*` — they may not even have the bare
/// `android/`/`ios/` folders `react-native run-*` requires.
pub fn is_expo(workspace: &Path) -> bool {
    std::fs::read_to_string(workspace.join("package.json"))
        .map(|c| c.contains("\"expo\""))
        .unwrap_or(false)
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

/// Compose the run command for the detected stack on an EXPLICIT target
/// `platform` (macOS can run either iOS or Android, so the caller decides —
/// from a platform button, or the viewed device). `device`, when it matches
/// that platform, pins the target; otherwise the tool's own picker/boot
/// handles it. Device pinning uses only well-supported flags: `flutter run
/// -d <id>` everywhere, `react-native run-ios --udid`; bare `run-android` and
/// Expo target the running emulator themselves.
pub fn run_on_device_command(
    kind: ProjectKind,
    expo: bool,
    platform: umide_emulator::DevicePlatform,
    device: Option<&umide_emulator::DeviceInfo>,
) -> RunOnDevice {
    use umide_emulator::DevicePlatform;
    let (platform_label, ios) = match platform {
        DevicePlatform::Android => ("Android", false),
        DevicePlatform::Ios => ("iOS", true),
    };
    // Only pin the device if it's on the chosen platform (the viewed device
    // may be the other kind). Flutter pins by `-d <device id>`: the adb serial
    // for Android, the simulator UDID for iOS (`DeviceInfo.id`).
    let device = device.filter(|d| d.platform == platform);
    let flutter_target = device.and_then(|d| match platform {
        DevicePlatform::Android => d.serial.clone(),
        DevicePlatform::Ios => Some(d.id.clone()),
    });

    let (stack_label, program, args) = match (kind, expo, ios) {
        (ProjectKind::Flutter, _, _) => {
            let mut a = vec!["run".to_string()];
            if let Some(t) = flutter_target {
                a.push("-d".into());
                a.push(t);
            }
            ("Flutter", "flutter", a)
        }
        (ProjectKind::ReactNative, true, false) => {
            ("Expo", "npx", vec!["expo".into(), "run:android".into()])
        }
        (ProjectKind::ReactNative, true, true) => {
            ("Expo", "npx", vec!["expo".into(), "run:ios".into()])
        }
        (ProjectKind::ReactNative, false, false) => (
            "React Native",
            "npx",
            vec!["react-native".into(), "run-android".into()],
        ),
        (ProjectKind::ReactNative, false, true) => {
            let mut a = vec!["react-native".to_string(), "run-ios".into()];
            if let Some(d) = device {
                a.push("--udid".into());
                a.push(d.id.clone());
            }
            ("React Native", "npx", a)
        }
    };
    // On Windows, `npx` and `flutter` are batch shims (npx.cmd / flutter.bat):
    // the run terminal spawns the program directly via CreateProcess, which
    // only executes real executables — the spawn dies instantly and silently
    // (found live: ▶ produced a ConPTY and nothing else; no node, no gradle).
    // Route through `cmd /C`. The args are fixed tool words plus adb serials /
    // simulator UDIDs — no user-controlled strings — so cmd's re-parsing
    // (the #41 footgun) has nothing to bite on.
    let (program, args) = if cfg!(target_os = "windows") {
        let mut wrapped = vec!["/C".to_string(), program.to_string()];
        wrapped.extend(args);
        ("cmd".to_string(), wrapped)
    } else {
        (program.to_string(), args)
    };
    RunOnDevice {
        name: format!("Run · {stack_label} · {platform_label}"),
        program,
        args,
    }
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

    fn assert_cmd(run: &RunOnDevice, program: &str, args: &[&str]) {
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

        // Bare RN: run-android targets the running emulator itself; run-ios
        // pins the simulator by UDID.
        let rn = run_on_device_command(
            ProjectKind::ReactNative,
            false,
            Android,
            Some(&android_dev()),
        );
        assert_cmd(&rn, "npx", &["react-native", "run-android"]);
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
        assert_cmd(&ex, "npx", &["expo", "run:android"]);
        assert!(ex.name.contains("Expo") && ex.name.contains("Android"));

        // Explicit iOS while viewing an ANDROID device: the mismatched device
        // is ignored, so no `--udid` — the tool boots/selects a simulator.
        let ios_no_pin = run_on_device_command(
            ProjectKind::ReactNative,
            false,
            Ios,
            Some(&android_dev()),
        );
        assert_cmd(&ios_no_pin, "npx", &["react-native", "run-ios"]);
        assert!(ios_no_pin.name.contains("iOS"));

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
        );
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
        std::fs::write(ws2.join("pubspec.yaml"), "name: pure_dart_pkg\n").unwrap();
        assert_eq!(detect_project_kind(&ws2), None);
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
