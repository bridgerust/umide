//! Persisted AI chat sessions for the assistant panel.
//!
//! Each chat keeps its own continuous context (so you never re-explain a task
//! mid-conversation); this module lets the panel hold *several* such chats,
//! switch between them, and remember them across restarts.
//!
//! On disk we store a **light** form: the visible transcript plus the external
//! CLI's resume id — never the raw LLM `history`, which carries base64 device
//! screenshots and is `Serialize`-only anyway. On reload, a text-only LLM
//! history is rebuilt from the transcript (`StoredSession::history`); CLI
//! backends (Claude Code / Codex) resume from their own `cli_session` id, so
//! their full context comes back regardless.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use umide_agent::{ContentBlock, Message};
use umide_core::directory::Directory;

/// Who authored a transcript line. Mirrors the panel's `MsgRole`, but owned here
/// so the on-disk format doesn't depend on view code.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    User,
    Assistant,
}

/// One transcript line, in the round-trippable on-disk form.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredMessage {
    pub role: Role,
    pub text: String,
    #[serde(default)]
    pub tools: Vec<String>,
}

/// One chat: its transcript, a derived title, and the CLI resume id (if the
/// chat ran on an external agent CLI).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredSession {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub messages: Vec<StoredMessage>,
    #[serde(default)]
    pub cli_session: Option<String>,
}

impl StoredSession {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            title: "New chat".to_string(),
            messages: Vec::new(),
            cli_session: None,
        }
    }

    /// Rebuild a text-only LLM history from the transcript, so an LLM chat still
    /// has its prior conversation in context after a restart. Tool calls and
    /// device screenshots are dropped (the model can re-observe); the wording of
    /// the conversation is what matters for continuity.
    pub fn history(&self) -> Vec<Message> {
        self.messages
            .iter()
            .filter(|m| !m.text.trim().is_empty())
            .map(|m| {
                let blocks = vec![ContentBlock::text(m.text.clone())];
                match m.role {
                    Role::User => Message::user(blocks),
                    Role::Assistant => Message::assistant(blocks),
                }
            })
            .collect()
    }

    /// Title = the first user line, trimmed to a chip-sized length. Called
    /// whenever the transcript changes so the switcher stays meaningful.
    pub fn retitle(&mut self) {
        let Some(first) = self.messages.iter().find(|m| m.role == Role::User) else {
            return;
        };
        let text = first.text.trim();
        if text.is_empty() {
            return;
        }
        const MAX: usize = 42;
        let short: String = text.chars().take(MAX).collect();
        self.title = if text.chars().count() > MAX {
            format!("{}…", short.trim_end())
        } else {
            short
        };
    }
}

/// Per-workspace store file: `…/ai-chats/<hash-of-workspace>.json`. Keyed by the
/// workspace path so each project keeps its own chats; a null workspace shares a
/// `global` file.
fn store_path(workspace: Option<&Path>) -> Option<PathBuf> {
    let dir = Directory::data_local_directory()?.join("ai-chats");
    std::fs::create_dir_all(&dir).ok()?;
    let key = match workspace {
        Some(p) => {
            // `DefaultHasher` uses fixed keys → stable across runs (unlike
            // `RandomState`), so the same workspace maps to the same file.
            let mut h = std::collections::hash_map::DefaultHasher::new();
            p.hash(&mut h);
            format!("{:016x}", h.finish())
        }
        None => "global".to_string(),
    };
    Some(dir.join(format!("{key}.json")))
}

/// Load this workspace's saved chats (newest-first order is the caller's job).
/// A missing or corrupt file yields an empty list rather than an error.
pub fn load(workspace: Option<&Path>) -> Vec<StoredSession> {
    let Some(path) = store_path(workspace) else {
        return Vec::new();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist this workspace's chats. Best-effort: a write failure is silent (chat
/// history is convenience state, never something to block the UI on).
pub fn save(workspace: Option<&Path>, sessions: &[StoredSession]) {
    let Some(path) = store_path(workspace) else {
        return;
    };
    if let Ok(json) = serde_json::to_string_pretty(sessions) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(t: &str) -> StoredMessage {
        StoredMessage {
            role: Role::User,
            text: t.into(),
            tools: vec![],
        }
    }
    fn asst(t: &str) -> StoredMessage {
        StoredMessage {
            role: Role::Assistant,
            text: t.into(),
            tools: vec![],
        }
    }

    #[test]
    fn retitle_uses_first_user_line_trimmed() {
        let mut s = StoredSession::new(1);
        s.messages = vec![asst("hi"), user("Add a dark-mode toggle to the app")];
        s.retitle();
        assert_eq!(s.title, "Add a dark-mode toggle to the app");

        let mut long = StoredSession::new(2);
        long.messages = vec![user(
            "Explain in great detail exactly how the whole build pipeline works end to end",
        )];
        long.retitle();
        assert!(long.title.ends_with('…'));
        assert!(long.title.chars().count() <= 43);
    }

    #[test]
    fn retitle_ignores_empty_and_missing() {
        let mut s = StoredSession::new(1);
        s.retitle(); // no messages
        assert_eq!(s.title, "New chat");
        s.messages = vec![asst("only assistant")];
        s.retitle();
        assert_eq!(s.title, "New chat");
    }

    #[test]
    fn history_rebuilds_text_only_and_skips_blank() {
        let mut s = StoredSession::new(1);
        s.messages = vec![user("hello"), asst(""), asst("world")];
        let h = s.history();
        assert_eq!(h.len(), 2); // blank assistant dropped
    }

    #[test]
    fn stored_session_round_trips_through_json() {
        let mut s = StoredSession::new(7);
        s.messages = vec![user("q"), asst("a")];
        s.cli_session = Some("resume-abc".into());
        s.retitle();
        let json = serde_json::to_string(&[s.clone()]).unwrap();
        let back: Vec<StoredSession> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, 7);
        assert_eq!(back[0].cli_session.as_deref(), Some("resume-abc"));
        assert_eq!(back[0].messages.len(), 2);
        assert_eq!(back[0].messages[0].role, Role::User);
    }
}
