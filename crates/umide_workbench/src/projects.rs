//! Project-scoped chat storage and read-only Git inspection.
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

#[path = "app_run.rs"]
mod app_run;
pub use app_run::{AppRun, device_logs};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub conversations: Vec<Conversation>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Conversation {
    pub title: String,
    pub provider: String,
    pub session_id: Option<String>,
    pub messages: Vec<Message>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub text: String,
}

fn data_dir() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("UMIDE_WORKBENCH_DATA_DIR") {
        let dir = PathBuf::from(path);
        return dir.is_absolute().then_some(dir).ok_or_else(|| {
            "UMIDE_WORKBENCH_DATA_DIR must be an absolute path.".into()
        });
    }
    ProjectDirs::from("org", "BridgeRust", "Umide Workbench")
        .map(|dirs| dirs.data_local_dir().to_owned())
        .ok_or_else(|| "Cannot find the application data directory.".to_owned())
}

fn history_file(root: &Path) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let digest = Sha256::digest(root.as_os_str().as_encoded_bytes());
    Ok(data_dir()?.join("history").join(format!("{digest:x}.json")))
}

fn load<T: DeserializeOwned>(file: &Path) -> Result<Option<T>, String> {
    let bytes = match fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("Cannot read {}: {error}", file.display()))
}

fn save(file: &Path, value: &impl Serialize) -> Result<(), String> {
    let persist = || -> io::Result<()> {
        let dir = file.parent().expect("app data file has a parent");
        fs::create_dir_all(dir)?;
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(&mut temp, value)?;
        temp.flush()?;
        temp.as_file().sync_all()?;
        temp.persist(file)?;
        Ok(())
    };
    persist().map_err(|error| format!("Cannot save {}: {error}", file.display()))
}

pub fn load_history(root: &Path) -> Result<History, String> {
    Ok(load(&history_file(root)?)?.unwrap_or_default())
}

pub fn save_history(root: &Path, history: &History) -> Result<(), String> {
    save(&history_file(root)?, history)
}

pub fn last_project() -> Option<PathBuf> {
    load::<PathBuf>(&data_dir().ok()?.join("last-project.json"))
        .ok()
        .flatten()
        .filter(|project| project.is_dir())
}

pub fn remember_project(root: &Path) -> Result<(), String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    save(&data_dir()?.join("last-project.json"), &root)
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    // Read only enough diff data to display. Drop the pipe on overflow so a
    // large generated or minified file cannot exhaust desktop memory.
    const LIMIT: u64 = 160 * 1024;
    let mut child = Command::new("git")
        .args([
            "--no-pager",
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot start Git: {error}"))?;
    let mut bytes = Vec::new();
    let stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.take(8192).read_to_end(&mut bytes);
        bytes
    });
    let read = child
        .stdout
        .take()
        .unwrap()
        .take(LIMIT + 1)
        .read_to_end(&mut bytes);
    let truncated = bytes.len() > LIMIT as usize;
    if truncated || read.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    let errors = errors.join().unwrap_or_default();
    read.map_err(|error| error.to_string())?;
    if !status.success() && !truncated {
        let msg = String::from_utf8_lossy(&errors);
        return Err(if msg.trim().is_empty() {
            format!("Git exited with {status}")
        } else {
            msg.trim().into()
        });
    }
    let mut text = String::from_utf8_lossy(&bytes).trim_end().to_string();
    if truncated {
        text.push_str("\n[Diff preview truncated]");
    }
    Ok(text)
}

pub fn git_diff(root: &Path) -> Result<String, String> {
    git(root, &["rev-parse", "--is-inside-work-tree"])?;
    let has_head = git(root, &["rev-parse", "--verify", "HEAD"]).is_ok();
    let diff = |extra: &[&str]| {
        let mut args = vec!["diff", "--no-color", "--no-ext-diff", "--no-textconv"];
        args.extend(extra);
        args.extend(["--", "."]);
        git(root, &args)
    };
    let mut sections = if has_head {
        vec![diff(&["HEAD"])?]
    } else {
        vec![diff(&["--cached"])?, diff(&[])?]
    };
    let untracked = git(
        root,
        &["ls-files", "--others", "--exclude-standard", "--", "."],
    )?;
    if !untracked.is_empty() {
        sections.push(format!("Untracked files\n{untracked}"));
    }
    sections.retain(|section| !section.is_empty());
    Ok(sections.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_replaces_the_old_snapshot_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("project/history.json");
        assert!(load::<History>(&path).unwrap().is_none());
        let mut history = History::default();
        history.conversations.push(Conversation {
            title: "Test project".into(),
            provider: "codex".into(),
            session_id: None,
            messages: vec![Message {
                role: "user".into(),
                text: "Hello".into(),
            }],
        });
        save(&path, &history).unwrap();
        history.conversations[0].session_id = Some("resumed-session".into());
        save(&path, &history).unwrap();
        let restored: History = load(&path).unwrap().unwrap();
        assert_eq!(restored.conversations.len(), 1);
        assert_eq!(
            restored.conversations[0].session_id.as_deref(),
            Some("resumed-session")
        );
        assert_eq!(restored.conversations[0].messages[0].text, "Hello");
    }

    #[test]
    fn git_diff_covers_staged_unstaged_and_untracked_in_an_unborn_project() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "--quiet"]).unwrap();
        fs::write(root.join("hello.txt"), "staged\n").unwrap();
        git(root, &["add", "hello.txt"]).unwrap();
        fs::write(root.join("hello.txt"), "unstaged\n").unwrap();
        fs::write(root.join("new.txt"), "").unwrap();
        let diff = git_diff(root).unwrap();
        assert!(diff.contains("+staged"));
        assert!(diff.contains("+unstaged"));
        assert!(diff.contains("Untracked files\nnew.txt"));
    }
}
