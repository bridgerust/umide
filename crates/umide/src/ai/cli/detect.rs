//! Detect whether an agent CLI is available, its version, and (best-effort)
//! whether it looks authenticated.
//!
//! Detection is intentionally cheap and non-interactive: a `PATH` lookup plus a
//! `--version` probe. Authentication is only a *hint* — these CLIs can be logged
//! in via the user's account (credentials we can't reliably introspect, e.g. a
//! macOS Keychain entry) or via an API-key env var. We never block a backend on
//! a negative auth guess; if credentials are truly missing, the run surfaces the
//! CLI's own error. Run this off the UI thread (it shells out).

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use super::CliKind;

/// Best-effort authentication signal for the picker. Deliberately conservative:
/// we report [`AuthHint::Ready`] only on a positive signal and otherwise
/// [`AuthHint::Unknown`] — never a false "not authenticated".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthHint {
    /// An API key env var is set, or a credentials file exists.
    Ready,
    /// No positive signal found; may still be logged in via the account.
    Unknown,
}

/// The result of probing one CLI.
#[derive(Debug, Clone)]
pub struct CliStatus {
    pub kind: CliKind,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub auth: AuthHint,
}

impl CliStatus {
    pub fn installed(&self) -> bool {
        self.path.is_some()
    }

    /// Probe one CLI: resolve on `PATH`, read `--version`, sniff auth.
    pub fn detect(kind: CliKind) -> Self {
        let path = which::which(kind.binary_name()).ok();
        let version = path.as_ref().and_then(|p| read_version(p));
        let auth = detect_auth(kind);
        Self {
            kind,
            path,
            version,
            auth,
        }
    }
}

/// Run `<bin> --version` with a short bound and return the first trimmed line.
fn read_version(bin: &std::path::Path) -> Option<String> {
    let mut child = Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    // `--version` returns near-instantly; guard against a hang anyway.
    let deadline = Duration::from_secs(5);
    let step = Duration::from_millis(25);
    let mut waited = Duration::ZERO;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if waited >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => {
                std::thread::sleep(step);
                waited += step;
            }
            Err(_) => return None,
        }
    }

    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Positive-only auth sniff: an API-key env var or a known credentials file.
fn detect_auth(kind: CliKind) -> AuthHint {
    let env_keys: &[&str] = match kind {
        CliKind::ClaudeCode => &["ANTHROPIC_API_KEY"],
        CliKind::Codex => &["OPENAI_API_KEY"],
        CliKind::GeminiCli => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
    };
    if env_keys.iter().any(|k| {
        std::env::var(k)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    }) {
        return AuthHint::Ready;
    }

    // A credentials file under the CLI's config dir is a strong positive signal
    // (covers account/OAuth logins that don't set an env key).
    if let Some(home) = home_dir() {
        let candidates: &[&str] = match kind {
            CliKind::ClaudeCode => &[".claude/.credentials.json", ".claude.json"],
            CliKind::Codex => &[".codex/auth.json"],
            CliKind::GeminiCli => {
                &[".gemini/oauth_creds.json", ".gemini/google_accounts.json"]
            }
        };
        if candidates.iter().any(|rel| home.join(rel).exists()) {
            return AuthHint::Ready;
        }
    }

    AuthHint::Unknown
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let var = "USERPROFILE";
    #[cfg(not(windows))]
    let var = "HOME";
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}
