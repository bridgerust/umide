//! Mobile project-type detection.
//!
//! UMIDE is a mobile-first IDE, so it should *know* when the open workspace is
//! a React Native or Flutter app instead of treating every folder the same.
//! Detection is intentionally cheap and offline (two file probes, no shelling
//! out — same house style as `ai/cli/detect.rs`): it runs once per workspace
//! open, on local workspaces only. The result feeds the status-bar badge and,
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

/// Compose the run command for the detected stack and the device the user is
/// viewing (the panel's `active_device`). The target platform follows that
/// device; with none running it defaults to Android (the emulator every OS
/// has), and the tool's own device picker/errors surface in the terminal.
/// Device pinning uses only well-supported flags: `flutter run -d <id>`
/// everywhere, `react-native run-ios --udid`; bare `run-android` and Expo
/// target the running emulator themselves.
pub fn run_on_device_command(
    kind: ProjectKind,
    expo: bool,
    device: Option<&umide_emulator::DeviceInfo>,
) -> RunOnDevice {
    use umide_emulator::DevicePlatform;
    let platform = device
        .map(|d| d.platform)
        .unwrap_or(DevicePlatform::Android);
    let (platform_label, ios) = match platform {
        DevicePlatform::Android => ("Android", false),
        DevicePlatform::Ios => ("iOS", true),
    };
    // Flutter pins by `-d <device id>`: the adb serial for Android, the
    // simulator UDID for iOS (`DeviceInfo.id`).
    let flutter_target = device.and_then(|d| match d.platform {
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
    RunOnDevice {
        name: format!("Run · {stack_label} · {platform_label}"),
        program: program.to_string(),
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

    #[test]
    fn run_command_matrix() {
        // Flutter pins the viewed device by id (adb serial / sim UDID).
        let f =
            run_on_device_command(ProjectKind::Flutter, false, Some(&android_dev()));
        assert_eq!(f.program, "flutter");
        assert_eq!(f.args, ["run", "-d", "emulator-5554"]);
        let fi =
            run_on_device_command(ProjectKind::Flutter, false, Some(&ios_dev()));
        assert_eq!(fi.args, ["run", "-d", "UDID-42"]);
        // No device: flutter's own picker handles it in the terminal.
        let fnone = run_on_device_command(ProjectKind::Flutter, false, None);
        assert_eq!(fnone.args, ["run"]);

        // Bare RN: run-android targets the running emulator itself; run-ios
        // pins the simulator by UDID.
        let rn = run_on_device_command(
            ProjectKind::ReactNative,
            false,
            Some(&android_dev()),
        );
        assert_eq!(rn.program, "npx");
        assert_eq!(rn.args, ["react-native", "run-android"]);
        let rni =
            run_on_device_command(ProjectKind::ReactNative, false, Some(&ios_dev()));
        assert_eq!(rni.args, ["react-native", "run-ios", "--udid", "UDID-42"]);

        // Expo apps go through `npx expo run:*`.
        let ex = run_on_device_command(
            ProjectKind::ReactNative,
            true,
            Some(&android_dev()),
        );
        assert_eq!(ex.args, ["expo", "run:android"]);
        assert!(ex.name.contains("Expo") && ex.name.contains("Android"));

        // No device defaults to Android (the platform every OS can run).
        let none = run_on_device_command(ProjectKind::ReactNative, false, None);
        assert_eq!(none.args, ["react-native", "run-android"]);
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
