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
