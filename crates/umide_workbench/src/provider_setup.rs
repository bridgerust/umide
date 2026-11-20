//! First-run installation and sign-in using the same standalone CLIs as an agent turn.
use std::{
    io::Read,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use floem::{ext_event::create_ext_action, prelude::*, reactive::Scope};
use umide_workbench_agents::{Provider, cli_environment};

use crate::projects;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Checking,
    Missing,
    Installing,
    SignedOut(String),
    SigningIn,
    Ready(String),
    Error(String),
}

#[derive(Clone, Copy)]
pub struct ProviderSetup {
    scope: Scope,
    rows: [RwSignal<Stage>; 2],
    jobs: [RwSignal<Option<Arc<AtomicBool>>>; 2],
}

#[derive(Clone, Copy)]
enum Action {
    Check,
    Install,
    SignIn,
}

impl ProviderSetup {
    pub fn new() -> Self {
        Self {
            scope: Scope::current(),
            rows: std::array::from_fn(|_| RwSignal::new(Stage::Checking)),
            jobs: std::array::from_fn(|_| RwSignal::new(None)),
        }
    }

    fn index(p: Provider) -> usize {
        usize::from(p == Provider::Claude)
    }
    pub fn stage(self, p: Provider) -> Stage {
        self.rows[Self::index(p)].get()
    }
    pub fn refresh(self, p: Provider) {
        self.start(p, Action::Check);
    }
    pub fn install(self, p: Provider) {
        self.start(p, Action::Install);
    }
    pub fn sign_in(self, p: Provider) {
        self.start(p, Action::SignIn);
    }

    pub fn guide(self, p: Provider) {
        let address = match p {
            Provider::Codex => "https://developers.openai.com/codex/cli/",
            Provider::Claude => "https://code.claude.com/docs/en/setup",
        };
        if let Err(error) = open::that_detached(address) {
            self.rows[Self::index(p)].set(Stage::Error(format!(
                "Could not open the setup guide: {error}"
            )));
        }
    }

    pub fn cancel(self, p: Provider) {
        if let Some(token) = self.jobs[Self::index(p)].get_untracked() {
            token.store(true, Ordering::Release);
        }
    }

    pub fn close(self) {
        for p in [Provider::Codex, Provider::Claude] {
            self.cancel(p);
        }
    }

    fn start(self, p: Provider, action: Action) {
        let i = Self::index(p);
        if self.jobs[i].with_untracked(Option::is_some) {
            return;
        }
        let token = Arc::new(AtomicBool::new(false));
        self.jobs[i].set(Some(token.clone()));
        self.rows[i].set(match action {
            Action::Check => Stage::Checking,
            Action::Install => Stage::Installing,
            Action::SignIn => Stage::SigningIn,
        });
        let complete = create_ext_action(self.scope, move |stage: Stage| {
            self.jobs[i].set(None);
            self.rows[i].set(stage);
        });
        thread::spawn(move || {
            let result = match action {
                Action::Check => Ok(()),
                Action::Install if cli_environment(p).is_err() => install(p, &token),
                Action::Install => Ok(()),
                Action::SignIn => sign_in(p, &token),
            };
            if token.load(Ordering::Acquire) {
                complete(Stage::Error(
                    "Cancelled. Check again to see the current state.".into(),
                ));
            } else {
                complete(
                    result
                        .and_then(|_| probe(p, &token))
                        .unwrap_or_else(Stage::Error),
                );
            }
        });
    }
}

fn cli(p: Provider) -> Result<Command, String> {
    let (executable, path) = cli_environment(p)?;
    let mut cmd = Command::new(executable);
    cmd.env("PATH", path);
    Ok(cmd)
}

fn command(
    cmd: &mut Command,
    cancel: &AtomicBool,
    timeout: Duration,
    capture_version: bool,
) -> Result<(bool, String), String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Cancelled.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    cmd.stdout(if capture_version {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = cmd.spawn().map_err(|e| {
        format!(
            "Could not start {}: {e}",
            cmd.get_program().to_string_lossy()
        )
    })?;
    let reader = child.stdout.take().map(|mut out| {
        thread::spawn(move || {
            // Drain even unexpected long version banners, retaining just the first line.
            let mut bytes = Vec::new();
            let _ = (&mut out).take(512).read_to_end(&mut bytes);
            let _ = std::io::copy(&mut out, &mut std::io::sink());
            String::from_utf8_lossy(&bytes)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        })
    });
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            let output = reader.and_then(|r| r.join().ok()).unwrap_or_default();
            return Ok((status.success(), output));
        }
        if cancel.load(Ordering::Acquire) || start.elapsed() >= timeout {
            projects::terminate(&mut child);
            let _ = child.wait();
            return Err(if cancel.load(Ordering::Acquire) {
                "Cancelled."
            } else {
                "Timed out. Try again or open the setup guide."
            }
            .into());
        }
        thread::sleep(Duration::from_millis(80));
    }
}

fn probe(p: Provider, cancel: &AtomicBool) -> Result<Stage, String> {
    let mut tool = match cli(p) {
        Ok(tool) => tool,
        Err(message) if message.starts_with("Could not find ") => {
            return Ok(Stage::Missing);
        }
        Err(message) => return Err(message),
    };
    let quick = Duration::from_secs(15);
    let (ok, version) = command(tool.arg("--version"), cancel, quick, true)?;
    if !ok {
        return Err("The CLI could not start. Open its setup guide to repair the installation.".into());
    }
    let mut auth = cli(p)?;
    match p {
        Provider::Codex => auth.args(["login", "status"]),
        Provider::Claude => auth.args(["auth", "status"]),
    };
    let (connected, _) = command(&mut auth, cancel, quick, false)?;
    Ok(if connected {
        Stage::Ready(version)
    } else {
        Stage::SignedOut(version)
    })
}

fn sign_in(p: Provider, cancel: &AtomicBool) -> Result<(), String> {
    let mut tool = cli(p)?;
    match p {
        Provider::Codex => tool.arg("login"),
        Provider::Claude => tool.args(["auth", "login"]),
    };
    let (ok, _) = command(&mut tool, cancel, Duration::from_secs(300), false)?;
    ok.then_some(()).ok_or_else(|| {
        "Sign-in did not finish. Try again or open the setup guide.".to_owned()
    })
}

fn install(p: Provider, cancel: &AtomicBool) -> Result<(), String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let windows = cfg!(windows);
    let source = match (p, windows) {
        (Provider::Codex, false) => "https://chatgpt.com/codex/install.sh",
        (Provider::Codex, true) => "https://chatgpt.com/codex/install.ps1",
        (Provider::Claude, false) => "https://claude.ai/install.sh",
        (Provider::Claude, true) => "https://claude.ai/install.ps1",
    };
    let file = dir
        .path()
        .join(if windows { "install.ps1" } else { "install.sh" });
    let mut fetch = Command::new(if windows { "curl.exe" } else { "curl" });
    fetch
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--output",
        ])
        .arg(&file)
        .arg(source);
    if !command(&mut fetch, cancel, Duration::from_secs(90), false)?.0 {
        return Err("Could not download the official installer. Check the network connection and try again.".into());
    }
    let mut installer = if windows {
        let mut shell = Command::new("powershell.exe");
        shell.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ]);
        shell
    } else {
        Command::new(if p == Provider::Claude {
            "/bin/bash"
        } else {
            "/bin/sh"
        })
    };
    installer.arg(&file);
    if p == Provider::Codex {
        installer.env("CODEX_NON_INTERACTIVE", "1");
    }
    if !command(&mut installer, cancel, Duration::from_secs(600), false)?.0 {
        return Err("The official installer did not finish. Check again or open the provider’s setup guide.".into());
    }
    cli_environment(p).map(|_| ()).map_err(|_| "The installer finished, but Umide could not find the CLI. Check its installation path in the setup guide.".into())
}

pub fn first_run() -> bool {
    projects::data_dir().ok().and_then(|dir| {
        projects::load::<bool>(&dir.join("setup-complete.json"))
            .ok()
            .flatten()
    }) != Some(true)
}

pub fn remember() -> Result<(), String> {
    projects::save(&projects::data_dir()?.join("setup-complete.json"), &true)
}

#[cfg(test)]
mod tests;
