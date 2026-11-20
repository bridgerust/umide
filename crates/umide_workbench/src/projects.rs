//! Project-scoped chat storage and read-only Git inspection.
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

#[path = "app_run.rs"]
mod app_run;
pub use app_run::AppRun;
pub(crate) use app_run::{ToolOutput, device_logs, terminate};

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
    pub isolated: bool,
    pub worktree_id: Option<String>,
    pub messages: Vec<Message>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub text: String,
}

impl Message {
    pub(crate) fn notice(text: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            text: text.into(),
        }
    }
}

pub(crate) fn data_dir() -> Result<PathBuf, String> {
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

pub(crate) fn load<T: DeserializeOwned>(file: &Path) -> Result<Option<T>, String> {
    let bytes = match fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("Cannot read {}: {error}", file.display()))
}

pub(crate) fn save(file: &Path, value: &impl Serialize) -> Result<(), String> {
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
    let mut child = crate::worktree::git(root)
        .arg("--no-optional-locks")
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
            format!("Git: {}", msg.trim())
        });
    }
    let mut text = String::from_utf8_lossy(&bytes).trim_end().to_string();
    if truncated {
        text.push_str("\n[Diff preview truncated]");
    }
    Ok(text)
}

pub fn git_diff(root: &Path) -> Result<String, String> {
    git_diff_since(root, None)
}

pub fn git_diff_since(root: &Path, base: Option<&str>) -> Result<String, String> {
    git(root, &["rev-parse", "--is-inside-work-tree"])?;
    if base.is_some_and(|sha| {
        !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|c| c.is_ascii_hexdigit())
    }) {
        return Err("Invalid worktree base commit".into());
    }
    let scope = if base.is_some() { ":/" } else { "." };
    let diff = |extra: &[&str]| {
        let mut args = vec!["diff", "--no-color", "--no-ext-diff", "--no-textconv"];
        args.extend(extra);
        args.extend(["--", scope]);
        git(root, &args)
    };
    let mut sections = if let Some(sha) = base {
        vec![diff(&[sha])?]
    } else if git(root, &["rev-parse", "--verify", "HEAD"]).is_ok() {
        vec![diff(&["HEAD"])?]
    } else {
        vec![diff(&["--cached"])?, diff(&[])?]
    };
    let untracked = git(
        root,
        &["ls-files", "--others", "--exclude-standard", "--", scope],
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
            isolated: false,
            worktree_id: None,
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
    fn reads_project_folder_conversations_saved_before_isolation() {
        let history: History = serde_json::from_str(r#"{"conversations":[{"title":"Fix screen","provider":"codex","session_id":"session-1","messages":[]}]}"#).unwrap();
        let conversation = &history.conversations[0];
        assert!(!conversation.isolated);
        assert!(conversation.worktree_id.is_none());
        assert_eq!(conversation.session_id.as_deref(), Some("session-1"));
    }

    #[test]
    fn isolated_diff_includes_agent_commits_and_changes_outside_app_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "--quiet"]).unwrap();
        fs::create_dir(root.join("mobile")).unwrap();
        fs::write(root.join("mobile/main"), "base").unwrap();
        let commit = || {
            git(root, &["add", "-A"]).unwrap();
            git(
                root,
                &[
                    "-c",
                    "user.name=Umide",
                    "-c",
                    "user.email=umide@example.invalid",
                    "-c",
                    "commit.gpgSign=false",
                    "commit",
                    "-qm",
                    "fixture",
                ],
            )
            .unwrap();
        };
        commit();
        let base = git(root, &["rev-parse", "HEAD"]).unwrap();
        fs::write(root.join("mobile/main"), "after").unwrap();
        fs::write(root.join("shared"), "sibling").unwrap();
        commit();
        fs::write(root.join("new-file"), "new").unwrap();
        let diff = git_diff_since(&root.join("mobile"), Some(&base)).unwrap();
        for text in ["+after", "+sibling", "new-file"] {
            assert!(diff.contains(text), "{diff}");
        }
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
