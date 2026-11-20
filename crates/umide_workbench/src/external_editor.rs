//! Hand off the selected checkout and file using each editor's native launcher.
use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use crate::project_files;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Editor {
    VsCode,
    AndroidStudio,
    #[cfg(target_os = "macos")]
    Xcode,
    Custom,
}

impl Editor {
    pub(crate) const ALL: &'static [Self] = &[
        Self::VsCode,
        Self::AndroidStudio,
        #[cfg(target_os = "macos")]
        Self::Xcode,
        Self::Custom,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::VsCode => "VS Code",
            Self::AndroidStudio => "Android Studio",
            #[cfg(target_os = "macos")]
            Self::Xcode => "Xcode",
            Self::Custom => "Other editor…",
        }
    }
}

pub(crate) fn launch(
    editor: Editor,
    root: &Path,
    relative: &Path,
) -> Result<(), String> {
    let (root, file) = project_files::external_target(root, relative)?;
    let search = env::var_os("PATH").unwrap_or_default();
    let cmd = editor_command(editor, &root, &file, &search)?;
    start(cmd, editor.label())
}

pub(crate) fn launch_custom(
    application: &Path,
    root: &Path,
    relative: &Path,
) -> Result<(), String> {
    let (root, file) = project_files::external_target(root, relative)?;
    if !application.is_absolute() {
        return Err("Choose an installed editor using its full path.".into());
    }
    #[cfg(target_os = "macos")]
    let mut cmd = {
        if !application.is_dir()
            || !application
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("app"))
        {
            return Err("Choose an installed .app application.".into());
        }
        let mut cmd = Command::new("/usr/bin/open");
        cmd.arg("-a").arg(application);
        cmd
    };
    #[cfg(not(target_os = "macos"))]
    let mut cmd = {
        if !is_executable(application) {
            return Err("Choose an installed editor executable.".into());
        }
        Command::new(application)
    };
    cmd.current_dir(&root).arg(editor_path(&file));
    let app_name = application
        .file_stem()
        .unwrap_or(application.as_os_str())
        .to_string_lossy();
    start(cmd, &app_name)
}

fn editor_command(
    editor: Editor,
    root: &Path,
    file: &Path,
    search: &OsStr,
) -> Result<Command, String> {
    let mut cmd = match editor {
        Editor::VsCode | Editor::AndroidStudio => {
            let project = if editor == Editor::AndroidStudio {
                android_project(root, file)
            } else {
                root
            };
            let mut cmd = installed_editor(editor, search, root)?;
            cmd.arg(editor_path(project)).arg(editor_path(file));
            cmd
        }
        #[cfg(target_os = "macos")]
        Editor::Xcode => {
            let project = xcode_project(root, file);
            // The bundled xed also works when xcode-select points to the standalone CLT.
            if let Some(xed) =
                mac_bundle_tool("Xcode.app/Contents/Developer/usr/bin/xed", root)
            {
                let mut cmd = Command::new(xed);
                if let Some(project) = project {
                    cmd.arg("--project").arg(project);
                }
                cmd.arg(file);
                cmd
            } else {
                let mut cmd = mac_open("com.apple.dt.Xcode");
                cmd.args(project).arg(file);
                cmd
            }
        }
        Editor::Custom => return Err("Choose an editor application.".into()),
    };
    cmd.current_dir(root);
    Ok(cmd)
}

fn editor_path(path: &Path) -> &Path {
    // Remove a Windows extended-length prefix only when the ordinary path refers
    // to the same file. Some editor CLIs treat verbatim drive paths as UNC shares.
    #[cfg(windows)]
    {
        dunce::simplified(path)
    }
    #[cfg(not(windows))]
    {
        path
    }
}

fn android_project<'a>(root: &'a Path, file: &'a Path) -> &'a Path {
    file.parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .take_while(|dir| dir.starts_with(root))
        .find(|dir| {
            ["settings.gradle", "settings.gradle.kts"]
                .iter()
                .any(|name| dir.join(name).is_file())
        })
        .unwrap_or(root)
}

#[cfg(target_os = "macos")]
fn xcode_project(root: &Path, file: &Path) -> Option<PathBuf> {
    for dir in file
        .parent()?
        .ancestors()
        .take_while(|dir| dir.starts_with(root))
    {
        // A known workspace takes precedence over its constituent project. Never
        // guess between multiple containers of the same kind in a monorepo.
        if let Ok(children) = dir.read_dir() {
            let dirs: Vec<_> = children
                .filter_map(Result::ok)
                .filter(|child| child.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|child| child.path())
                .collect();
            for extension in ["xcworkspace", "xcodeproj"] {
                let mut containers = dirs
                    .iter()
                    .filter(|p| p.extension() == Some(OsStr::new(extension)));
                if let Some(first) = containers.next() {
                    return containers.next().is_none().then(|| first.clone());
                }
            }
        }
        let package = dir.join("Package.swift");
        if package.symlink_metadata().is_ok_and(|meta| meta.is_file()) {
            return Some(package);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(windows)]
    {
        path.extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
    }
}

fn installed(path: &Path, root: &Path) -> bool {
    // A project can add local tools to PATH. Named editors must resolve to an
    // installation outside this checkout; a custom application is chosen explicitly.
    is_executable(path) && path.canonicalize().is_ok_and(|p| !p.starts_with(root))
}

fn installed_editor(
    editor: Editor,
    search: &OsStr,
    root: &Path,
) -> Result<Command, String> {
    #[cfg(unix)]
    let names: &[&str] = if editor == Editor::VsCode {
        &["code"]
    } else {
        &["studio", "studio.sh", "android-studio"]
    };
    #[cfg(windows)]
    let names: &[&str] = if editor == Editor::VsCode {
        &["Code.exe"]
    } else {
        &["studio64.exe"]
    };

    for bin in env::split_paths(search).filter(|p| p.is_absolute()) {
        for name in names {
            let candidate = bin.join(name);
            if installed(&candidate, root) {
                return Ok(Command::new(candidate));
            }
        }
        #[cfg(windows)]
        if editor == Editor::VsCode && bin.join("code.cmd").is_file() {
            // The PATH entry installed by VS Code points to the bin directory. Use
            // the signed executable in its parent directly so no cmd.exe is involved.
            if let Some(parent) = bin.parent() {
                let exe = parent.join("Code.exe");
                if installed(&exe, root) {
                    return Ok(Command::new(exe));
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let (relative, bundle_id) = if editor == Editor::VsCode {
            (
                "Visual Studio Code.app/Contents/Resources/app/bin/code",
                "com.microsoft.VSCode",
            )
        } else {
            (
                "Android Studio.app/Contents/MacOS/studio",
                "com.google.android.studio",
            )
        };
        Ok(mac_bundle_tool(relative, root)
            .map(Command::new)
            .unwrap_or_else(|| mac_open(bundle_id)))
    }

    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(windows)]
        let candidates: Vec<PathBuf> = {
            let mut candidates = Vec::new();
            for (dir, relative) in if editor == Editor::VsCode {
                [
                    ("LOCALAPPDATA", "Programs/Microsoft VS Code/Code.exe"),
                    ("ProgramFiles", "Microsoft VS Code/Code.exe"),
                ]
            } else {
                [
                    ("ProgramFiles", "Android/Android Studio/bin/studio64.exe"),
                    ("LOCALAPPDATA", "Programs/Android Studio/bin/studio64.exe"),
                ]
            } {
                if let Some(base) = env::var_os(dir) {
                    candidates.push(PathBuf::from(base).join(relative));
                }
            }
            candidates
        };
        #[cfg(not(windows))]
        let candidates: Vec<PathBuf> = if editor == Editor::VsCode {
            ["/usr/bin/code", "/usr/local/bin/code", "/snap/bin/code"]
                .map(PathBuf::from)
                .into()
        } else {
            [
                "/opt/android-studio/bin/studio",
                "/opt/android-studio/bin/studio.sh",
                "/usr/local/android-studio/bin/studio",
                "/usr/local/android-studio/bin/studio.sh",
                "/snap/bin/android-studio",
            ]
            .map(PathBuf::from)
            .into()
        };
        candidates
            .into_iter()
            .find(|p| installed(p, root))
            .map(Command::new)
            .ok_or_else(|| {
                format!(
                    "Could not find {}. Use Other editor… to select its executable.",
                    editor.label()
                )
            })
    }
}

#[cfg(target_os = "macos")]
fn mac_open(bundle: &str) -> Command {
    let mut cmd = Command::new("/usr/bin/open");
    cmd.arg("-b").arg(bundle);
    cmd
}

#[cfg(target_os = "macos")]
fn mac_bundle_tool(relative: &str, root: &Path) -> Option<PathBuf> {
    let user =
        env::var_os("HOME").map(|dir| PathBuf::from(dir).join("Applications"));
    user.into_iter()
        .chain([PathBuf::from("/Applications")])
        .map(|dir| dir.join(relative))
        .find(|path| installed(path, root))
}

fn start(mut cmd: Command, label: &str) -> Result<(), String> {
    let error = |e| format!("Could not launch {label}: {e}");
    #[cfg(target_os = "macos")]
    if cmd.get_program() == OsStr::new("/usr/bin/open")
        || Path::new(cmd.get_program()).ends_with("Contents/Developer/usr/bin/xed")
    {
        // These commands hand off to Launch Services or Xcode, then exit. Wait for
        // the actual result even if the editor takes a while to start.
        let output = cmd.stdin(Stdio::null()).output().map_err(error)?;
        if output.status.success() {
            return Ok(());
        }
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        return Err(if detail.is_empty() {
            format!("{label} could not open this file ({}).", output.status)
        } else {
            format!("{label} could not open this file: {detail}")
        });
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(error)?;
    // Some launchers forward the file to a running instance and exit immediately;
    // others become the editor process. Report immediate failures and reap both.
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = send.send(child.wait());
    });
    match receive.recv_timeout(Duration::from_millis(400)) {
        Ok(status) => {
            let status = status.map_err(error)?;
            if status.success() {
                Ok(())
            } else {
                Err(format!(
                    "{label} could not open this file ({status}). Check that the editor is installed."
                ))
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(format!("Could not check the {label} launcher."))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn android_uses_closest_gradle_build_within_selected_checkout() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("isolated");
        let android = root.join("mobile/android");
        let file = android.join("app/src/main/kotlin/Main.kt");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(tmp.path().join("settings.gradle"), "").unwrap();
        assert_eq!(android_project(&root, &file), root);
        fs::write(android.join("settings.gradle.kts"), "").unwrap();
        assert_eq!(android_project(&root, &file), android);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn xcode_uses_nearest_unambiguous_workspace_project_or_package() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let file = root.join("app/Sources/Store.swift");
        let app = root.join("app");
        let workspace = app.join("Store.xcworkspace");
        let manifest = app.join("Package.swift");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::create_dir(root.join("Outer.xcodeproj")).unwrap();
        fs::write(&manifest, "").unwrap();
        assert_eq!(xcode_project(root, &file), Some(manifest));
        fs::create_dir(app.join("Store.xcodeproj")).unwrap();
        fs::create_dir(&workspace).unwrap();
        assert_eq!(xcode_project(root, &file), Some(workspace));
        fs::create_dir(app.join("Other.xcworkspace")).unwrap();
        assert_eq!(xcode_project(root, &file), None);
    }

    #[cfg(unix)]
    #[test]
    fn launcher_passes_literals_and_uses_installed_editor_for_selected_checkout() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        let root = tmp.path().join("task checkout");
        let bin = tmp.path().join("installed tools");
        let local = root.join("local tools");
        let relative = Path::new("src").join("draft $price; 'quote'.rs");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(&local).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::write(root.join(&relative), [0, 255]).unwrap();
        let (root, file) = project_files::external_target(&root, &relative).unwrap();
        for directory in [&local, &bin] {
            let code = directory.join("code");
            fs::write(
                &code,
                "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\" > \"$RECEIPT\"\n",
            )
            .unwrap();
            fs::set_permissions(code, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let search = env::join_paths([local, bin.clone()]).unwrap();
        let mut cmd = editor_command(Editor::VsCode, &root, &file, &search).unwrap();
        assert_eq!(Path::new(cmd.get_program()), bin.join("code"));
        let receipt = tmp.path().join("received");
        cmd.env("RECEIPT", &receipt);
        start(cmd, "fixture editor").unwrap();
        // A busy runner can hand back the launcher before the fixture has written its receipt.
        let expected = format!(
            "{}\n{}\n{}\n",
            root.display(),
            root.display(),
            file.display()
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while fs::read_to_string(&receipt).ok().as_deref() != Some(expected.as_str())
        {
            assert!(
                std::time::Instant::now() < deadline,
                "The editor never received the selected file"
            );
            thread::sleep(Duration::from_millis(20));
        }
        let bad = Command::new("/usr/bin/false");
        assert!(
            start(bad, "fixture editor")
                .unwrap_err()
                .contains("fixture editor")
        );
    }

    #[cfg(unix)]
    #[test]
    fn cannot_handoff_a_directory_git_internal_or_escaping_symlink() {
        use std::os::unix::fs::symlink;
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("selected");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join(".git/config"), "").unwrap();
        let elsewhere = tmp.path().join("other");
        fs::write(&elsewhere, "").unwrap();
        symlink(elsewhere, root.join("alias")).unwrap();
        for relative in ["src", ".git/config", "../other", "alias", "missing"] {
            assert!(
                project_files::external_target(&root, Path::new(relative)).is_err()
            );
        }
    }
}
