//! Isolated task checkouts. Only the task key is stored in conversation history.
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use sha2::{Digest, Sha256};
use uuid::Uuid;

pub fn new_key() -> String {
    Uuid::new_v4().simple().to_string()
}

struct Task {
    repository: PathBuf,
    common_git_dir: PathBuf,
    app: PathBuf,
    storage: PathBuf,
}

pub(crate) fn git(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(root)
        .args([
            "--no-pager",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    for key in [
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
    ] {
        cmd.env_remove(key);
    }
    cmd
}

fn run(command: &mut Command) -> Result<Output, String> {
    let out = command
        .output()
        .map_err(|e| format!("Cannot start Git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Git: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out)
}

fn path_line(mut bytes: Vec<u8>) -> Result<PathBuf, String> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        #[cfg(windows)]
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes).into())
    }
    #[cfg(not(unix))]
    {
        Ok(
            OsString::from(
                String::from_utf8(bytes).map_err(|_| "Invalid Git path")?,
            )
            .into(),
        )
    }
}

fn git_path(root: &Path, arg: &str) -> Result<PathBuf, String> {
    let out = run(git(root).args(["rev-parse", "--path-format=absolute", arg]))?;
    path_line(out.stdout)?
        .canonicalize()
        .map_err(|e| format!("Invalid Git directory: {e}"))
}

fn create_directory(path: &Path) -> Result<(), String> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(format!("Cannot create {}: {e}", path.display())),
    }
}

// Validate paths underneath the configured data directory before using them.
fn checked_dir(path: &Path) -> Result<PathBuf, String> {
    let real = path
        .canonicalize()
        .map_err(|e| format!("Missing task folder {}: {e}", path.display()))?;
    if real != path || !real.is_dir() {
        return Err(format!("Unexpected task folder: {}", path.display()));
    }
    Ok(real)
}

impl Task {
    fn locate(
        project: &Path,
        data_dir: &Path,
        key: &str,
        create: bool,
    ) -> Result<Self, String> {
        if key.len() != 32
            || !key
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("Invalid isolated task key.".into());
        }
        let project = project
            .canonicalize()
            .map_err(|e| format!("Cannot open project: {e}"))?;
        let repository = git_path(&project, "--show-toplevel")?;
        let common_git_dir = git_path(&project, "--git-common-dir")?;
        let app = project
            .strip_prefix(&repository)
            .map_err(|_| "Invalid app folder in repository.")?
            .to_owned();
        if !data_dir.is_absolute() {
            return Err("Workbench storage needs an absolute path.".into());
        }
        if create {
            let ancestor = data_dir
                .ancestors()
                .find(|p| p.exists())
                .ok_or("No storage directory available.")?;
            let ancestor = ancestor.canonicalize().map_err(|e| e.to_string())?;
            if ancestor.starts_with(&repository)
                || ancestor.starts_with(&common_git_dir)
            {
                return Err(
                    "Choose a Workbench data directory outside this repository."
                        .into(),
                );
            }
            fs::create_dir_all(data_dir)
                .map_err(|e| format!("Cannot create Workbench storage: {e}"))?;
        }
        let data = data_dir
            .canonicalize()
            .map_err(|e| format!("Cannot open Workbench storage: {e}"))?;
        let fingerprint = Sha256::digest(project.as_os_str().as_encoded_bytes());
        let mut parent = data.join("worktrees");
        if parent.starts_with(&repository) || parent.starts_with(&common_git_dir) {
            return Err("Task checkouts must be stored outside the project.".into());
        }
        for child in [format!("{fingerprint:x}"), key.to_owned()] {
            if create {
                create_directory(&parent)?;
            }
            checked_dir(&parent)?;
            parent.push(child);
        }
        Ok(Self {
            repository,
            common_git_dir,
            app,
            storage: parent,
        })
    }

    fn root(&self) -> PathBuf {
        self.storage.join("checkout")
    }

    fn validate(&self) -> Result<(), String> {
        checked_dir(&self.storage)?;
        let root = checked_dir(&self.root())?;
        let link = root.join(".git");
        if !fs::symlink_metadata(&link).is_ok_and(|s| s.is_file())
            || git_path(&root, "--show-toplevel")? != root
            || git_path(&root, "--git-common-dir")? != self.common_git_dir
        {
            return Err("This checkout no longer belongs to the project.".into());
        }
        // A copied checkout can share our repo and appear at the expected path.
        // Git's own worktree registration must point back to this exact .git file.
        let admin = git_path(&root, "--absolute-git-dir")?;
        let registered = fs::read(admin.join("gitdir"))
            .map_err(|e| format!("Missing Git worktree registration: {e}"))?;
        let backlink = admin.join(path_line(registered)?);
        if admin.parent() != Some(self.common_git_dir.join("worktrees").as_path())
            || backlink.canonicalize().ok().as_deref() != Some(link.as_path())
        {
            return Err("Git registered this task at a different location.".into());
        }
        Ok(())
    }

    fn app_path(&self) -> Result<PathBuf, String> {
        let root = self.root();
        let selected = root.join(&self.app);
        let real = selected.canonicalize().map_err(|_| "The app folder is missing from this checkout. Is it in the committed project?")?;
        if !real.is_dir() || !real.starts_with(&root) {
            return Err(
                "The app folder points outside its isolated checkout.".into()
            );
        }
        Ok(real)
    }
}

fn commit_id(bytes: &[u8]) -> Result<String, String> {
    let id = std::str::from_utf8(bytes)
        .map_err(|_| "Invalid task base commit.")?
        .trim_end();
    if !matches!(id.len(), 40 | 64) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid task base commit.".into());
    }
    Ok(id.to_owned())
}

pub fn create_with_key(
    project: &Path,
    data_dir: &Path,
    key: &str,
) -> Result<PathBuf, String> {
    let task = Task::locate(project, data_dir, key, true)?;
    let head =
        run(git(&task.repository).args(["rev-parse", "--verify", "HEAD^{commit}"]))
            .map_err(|e| format!("Isolation requires a committed project. {e}"))?;
    let head = commit_id(&head.stdout)?;
    // Exclusive creation prevents parallel calls from adopting or overwriting a task.
    fs::create_dir(&task.storage)
        .map_err(|e| format!("Cannot reserve isolated task: {e}"))?;
    let base = task.storage.join("base-commit");
    let root = task.root();
    let create = || {
        fs::write(&base, &head)
            .map_err(|e| format!("Cannot save task base commit: {e}"))?;
        run(git(&task.repository)
            .args(["worktree", "add", "--quiet", "--detach", "--"])
            .arg(&root)
            .arg(&head))?;
        task.validate()?;
        task.app_path()
    };
    match create() {
        Ok(path) => Ok(path),
        Err(error) => {
            // Release only the new, still unused checkout. Git refuses to remove
            // a dirty or locked worktree; leave its metadata intact in that case.
            if fs::symlink_metadata(&root).is_ok()
                && run(git(&task.repository)
                    .args(["worktree", "remove", "--"])
                    .arg(&root))
                .is_err()
            {
                return Err(format!(
                    "{error}\nCheck the unfinished task at {}.",
                    root.display()
                ));
            }
            let _ = fs::remove_file(base);
            let _ = fs::remove_dir(task.storage);
            Err(error)
        }
    }
}

pub fn open(project: &Path, data_dir: &Path, key: &str) -> Result<PathBuf, String> {
    let task = Task::locate(project, data_dir, key, false)?;
    task.validate()?;
    task.app_path()
}

pub fn review(
    project: &Path,
    data_dir: &Path,
    key: &str,
) -> Result<(PathBuf, String), String> {
    let task = Task::locate(project, data_dir, key, false)?;
    task.validate()?;
    let base = task.storage.join("base-commit");
    if !fs::symlink_metadata(&base).is_ok_and(|s| s.is_file() && s.len() <= 66) {
        return Err("Missing task base commit.".into());
    }
    let id = commit_id(
        &fs::read(base).map_err(|e| format!("Cannot read task base commit: {e}"))?,
    )?;
    Ok((task.root(), id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("mobile repo");
        let app = project.join("apps/ios");
        fs::create_dir_all(&app).unwrap();
        run(git(&project).arg("init").arg("--quiet")).unwrap();
        fs::write(app.join("app.txt"), "published\n").unwrap();
        run(git(&project).args(["add", "--", "."])).unwrap();
        commit(&project);
        let data = temp.path().join("Workbench storage");
        (temp, app, data)
    }

    fn commit(root: &Path) {
        run(git(root).args([
            "-c",
            "user.name=Workbench Test",
            "-c",
            "user.email=test@localhost",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qam",
            "test",
        ]))
        .unwrap();
    }

    #[test]
    fn isolates_nested_app_and_tracks_changes_after_agent_commits() {
        let (_temp, app, data) = fixture();
        fs::write(app.join("app.txt"), "user's local changes\n").unwrap();
        let first = new_key();
        let second = new_key();
        let a = create_with_key(&app, &data, &first).unwrap();
        let b = create_with_key(&app, &data, &second).unwrap();
        assert_ne!(a, b);
        assert_eq!(
            fs::read_to_string(a.join("app.txt")).unwrap(),
            "published\n"
        );
        assert_eq!(a, open(&app, &data, &first).unwrap());
        let (root, base) = review(&app, &data, &first).unwrap();
        assert_eq!(a, root.join("apps/ios"));
        assert_eq!(
            git(&root)
                .args(["symbolic-ref", "--quiet", "HEAD"])
                .status()
                .unwrap()
                .code(),
            Some(1)
        );
        assert!(create_with_key(&app, &data, &first).is_err());

        fs::write(a.join("app.txt"), "agent edited\n").unwrap();
        commit(&a);
        let changes = run(git(&root).arg("diff").arg(&base)).unwrap().stdout;
        assert!(String::from_utf8_lossy(&changes).contains("+agent edited"));
        assert_eq!(base, review(&app, &data, &first).unwrap().1);
        assert_eq!(
            fs::read_to_string(b.join("app.txt")).unwrap(),
            "published\n"
        );
        assert_eq!(
            fs::read_to_string(app.join("app.txt")).unwrap(),
            "user's local changes\n"
        );
    }

    #[test]
    fn rejects_missing_malformed_and_replaced_task_checkouts() {
        let (_temp, app, data) = fixture();
        let key = new_key();
        assert!(open(&app, &data, &key).is_err());
        assert!(!data.exists());
        assert!(create_with_key(&app, &data, "../escape").is_err());
        assert!(create_with_key(&app, &app.join("nested"), &key).is_err());
        assert!(!app.join("nested").exists());
        create_with_key(&app, &data, &key).unwrap();
        let (old, _) = review(&app, &data, &key).unwrap();
        let other = new_key();
        create_with_key(&app, &data, &other).unwrap();
        let (other_root, _) = review(&app, &data, &other).unwrap();
        // Even the other task's valid worktree registration is not valid here.
        fs::copy(other_root.join(".git"), old.join(".git")).unwrap();
        assert!(open(&app, &data, &key).is_err());
        fs::rename(&old, old.with_file_name("moved-by-user")).unwrap();
        assert!(open(&app, &data, &key).is_err());
        assert!(!old.exists());
    }

    #[test]
    fn uncommitted_app_folder_fails_without_registering_an_empty_task() {
        let (_temp, app, data) = fixture();
        let new_app = app.join("untracked");
        fs::create_dir(&new_app).unwrap();
        let key = new_key();
        assert!(create_with_key(&new_app, &data, &key).is_err());
        let trees =
            run(git(&app).args(["worktree", "list", "--porcelain"])).unwrap();
        assert_eq!(
            String::from_utf8(trees.stdout)
                .unwrap()
                .lines()
                .filter(|line| line.starts_with("worktree "))
                .count(),
            1
        );
        assert!(open(&new_app, &data, &key).is_err());
    }
}
