//! Project-scoped development servers for the Web preview.
use std::{
    fs, io,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;

const OUTPUT_LIMIT: usize = 48 * 1024;
const START_TIMEOUT: Duration = Duration::from_secs(120);
const LOOPBACK: &str = "127.0.0.1";

#[derive(Deserialize)]
struct Manifest {
    #[serde(rename = "packageManager")]
    manager: Option<String>,
    #[serde(default)]
    scripts: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug)]
enum ServerKind {
    Static,
    Script {
        manager: &'static str,
        name: &'static str,
        flags: BindFlags,
    },
}

#[derive(Clone, Copy, Debug)]
enum BindFlags {
    Environment,
    Host,
    Vite,
    Next,
}

impl BindFlags {
    fn from_script(script: &str) -> Self {
        // Forward CLI flags only to a recognized command. Other scripts can read
        // the same loopback host and port from the environment.
        let command = script.rsplit([';', '&', '|']).next().unwrap_or_default();
        for word in command.split_ascii_whitespace() {
            let word = word.trim_matches(['\'', '"']);
            match word.rsplit('/').next() {
                Some("vite") => return Self::Vite,
                Some("next") => return Self::Next,
                Some(
                    "astro" | "nuxt" | "nuxi" | "ng" | "webpack"
                    | "webpack-dev-server" | "remix",
                ) => {
                    return Self::Host;
                }
                _ => {}
            }
        }
        Self::Environment
    }
}

#[derive(Clone, Debug)]
pub struct WebProject {
    root: PathBuf,
    kind: ServerKind,
}

impl WebProject {
    pub fn detect(root: &Path) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|e| format!("Cannot open {}: {e}", root.display()))?;
        if !root.is_dir() {
            return Err("Choose a project folder to preview.".into());
        }
        let manifest = root.join("package.json");
        match fs::File::open(&manifest) {
            Ok(file) => {
                let manifest: Manifest =
                    serde_json::from_reader(file.take(2 * 1024 * 1024))
                        .map_err(|e| format!("Cannot read package.json: {e}"))?;
                for name in ["dev", "start"] {
                    if let Some(script) =
                        manifest.scripts.get(name).filter(|s| !s.trim().is_empty())
                    {
                        return Ok(Self {
                            kind: ServerKind::Script {
                                manager: package_manager(
                                    &root,
                                    manifest.manager.as_deref(),
                                )?,
                                name,
                                flags: BindFlags::from_script(script),
                            },
                            root,
                        });
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Cannot open package.json: {e}")),
        }
        if root.join("index.html").is_file() {
            return Ok(Self {
                root,
                kind: ServerKind::Static,
            });
        }
        Err("This folder needs a dev or start script in package.json, or an index.html file.".into())
    }

    pub fn command_label(&self) -> String {
        match self.kind {
            ServerKind::Static => "Static site · index.html".into(),
            ServerKind::Script { manager, name, .. } => {
                format!("{manager} run {name}")
            }
        }
    }

    fn command(&self, port: u16) -> Command {
        let port = port.to_string();
        let mut cmd = match self.kind {
            ServerKind::Static => {
                #[cfg(windows)]
                let mut cmd = {
                    let mut cmd = Command::new("py");
                    cmd.arg("-3");
                    cmd
                };
                #[cfg(not(windows))]
                let mut cmd = Command::new("python3");
                cmd.args([
                    "-I",
                    "-u",
                    "-c",
                    include_str!("web_server/static_server.py"),
                    &port,
                ]);
                cmd
            }
            ServerKind::Script {
                manager,
                name,
                flags,
            } => {
                #[cfg(windows)]
                let program = match manager {
                    "bun" => "bun.exe".to_owned(),
                    _ => format!("{manager}.cmd"),
                };
                #[cfg(not(windows))]
                let program = manager;
                let mut cmd = Command::new(program);
                cmd.args(["run", name]);
                if !matches!(flags, BindFlags::Environment) {
                    if manager == "npm" {
                        cmd.arg("--");
                    }
                    cmd.args([
                        if matches!(flags, BindFlags::Next) {
                            "--hostname"
                        } else {
                            "--host"
                        },
                        LOOPBACK,
                        "--port",
                        &port,
                    ]);
                    if matches!(flags, BindFlags::Vite) {
                        cmd.arg("--strictPort");
                    }
                }
                cmd.env("HOST", LOOPBACK)
                    .env("HOSTNAME", LOOPBACK)
                    .env("PORT", &port)
                    .env("BROWSER", "none")
                    .env("NO_COLOR", "1")
                    .env("COREPACK_ENABLE_NETWORK", "0")
                    .env("COREPACK_ENABLE_AUTO_PIN", "0");
                cmd
            }
        };
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
            let mut paths: Vec<_> = std::env::var_os("PATH")
                .as_deref()
                .map(std::env::split_paths)
                .into_iter()
                .flatten()
                .collect();
            if let Some(home) = std::env::var_os("HOME") {
                paths.push(Path::new(&home).join(".bun/bin"));
                paths.push(Path::new(&home).join(".local/bin"));
            }
            paths.extend([
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ]);
            if let Ok(paths) = std::env::join_paths(paths) {
                cmd.env("PATH", paths);
            }
        }
        cmd.current_dir(&self.root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }
}

fn package_manager(
    root: &Path,
    declaration: Option<&str>,
) -> Result<&'static str, String> {
    let managers: [(&str, &[&str]); 4] = [
        ("pnpm", &["pnpm-lock.yaml"]),
        ("yarn", &["yarn.lock"]),
        ("bun", &["bun.lock", "bun.lockb"]),
        ("npm", &["package-lock.json", "npm-shrinkwrap.json"]),
    ];
    if let Some(value) = declaration {
        let name = value.split('@').next().unwrap_or_default();
        return managers
            .iter()
            .find(|(m, _)| *m == name)
            .map(|(m, _)| *m)
            .ok_or_else(|| {
                format!("Unsupported packageManager in package.json: {value}.")
            });
    }
    let mut matching = managers
        .iter()
        .filter(|(_, files)| files.iter().any(|file| root.join(file).is_file()));
    let manager = matching.next().map(|(m, _)| *m).unwrap_or("npm");
    if matching.next().is_some() {
        return Err("Multiple package-manager lockfiles found. Set packageManager in package.json to choose one.".into());
    }
    Ok(manager)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Starting,
    Ready(String),
    Failed(String),
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub status: Status,
    pub output: String,
}

struct State {
    snapshot: Snapshot,
    child: Option<Child>,
}

pub struct WebServer(Arc<Mutex<State>>);

impl WebServer {
    pub fn start(project: WebProject) -> Result<Self, String> {
        let state = Arc::new(Mutex::new(State {
            snapshot: Snapshot {
                status: Status::Starting,
                output: format!("{}\n", project.command_label()),
            },
            child: None,
        }));
        let worker = Arc::clone(&state);
        thread::Builder::new()
            .name("web-preview".into())
            .spawn(move || {
                if let Err(error) = supervise(&project, &worker) {
                    let mut state = worker.lock().unwrap();
                    if state.snapshot.status != Status::Stopped {
                        state.snapshot.status = Status::Failed(error);
                    }
                }
            })
            .map_err(|e| format!("Could not start web preview: {e}"))?;
        Ok(Self(state))
    }

    pub fn snapshot(&self) -> Snapshot {
        self.0.lock().unwrap().snapshot.clone()
    }

    pub fn stop(&self) {
        let mut state = self.0.lock().unwrap();
        state.snapshot.status = Status::Stopped;
        state.terminate();
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        self.stop();
    }
}

impl State {
    fn terminate(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        #[cfg(unix)]
        {
            // The dev tool often outlives its package-manager wrapper.
            // Signal the group even when try_wait already reaped the leader.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/T", "/F", "/PID", &child.id().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn drain(
    pipe: impl Read + Send + 'static,
    shared: Arc<Mutex<State>>,
    done: mpsc::Sender<()>,
) {
    thread::spawn(move || {
        let mut pipe = pipe;
        let mut chunk = [0; 4096];
        while let Ok(n) = pipe.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let mut state = shared.lock().unwrap();
            let out = &mut state.snapshot.output;
            out.push_str(&String::from_utf8_lossy(&chunk[..n]));
            let mut cut = out.len().saturating_sub(OUTPUT_LIMIT);
            while !out.is_char_boundary(cut) {
                cut += 1;
            }
            if cut > 0 {
                out.drain(..cut);
            }
        }
        let _ = done.send(());
    });
}

fn answers_http(port: u16) -> bool {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let timeout = Duration::from_millis(650);
    let Ok(mut stream) = TcpStream::connect_timeout(&address.into(), timeout) else {
        return false;
    };
    if stream.set_write_timeout(Some(timeout)).is_err()
        || stream.set_read_timeout(Some(timeout)).is_err()
    {
        return false;
    }
    if write!(
        stream,
        "GET / HTTP/1.1\r\nHost: {LOOPBACK}:{port}\r\nConnection: close\r\n\r\n"
    )
    .is_err()
    {
        return false;
    }
    // A TCP listener can be alive before the development server can serve HTTP.
    // Only read the status line prefix, never fetch or follow a remote redirect.
    let mut prefix = [0; 13];
    let deadline = Instant::now() + timeout;
    let mut read = 0;
    while read < prefix.len() && Instant::now() < deadline {
        match stream.read(&mut prefix[read..]) {
            Ok(0) | Err(_) => return false,
            Ok(n) => read += n,
        }
    }
    read == prefix.len()
        && (prefix.starts_with(b"HTTP/1.1 ") || prefix.starts_with(b"HTTP/1.0 "))
        && (b'2'..=b'5').contains(&prefix[9])
        && prefix[10..12].iter().all(u8::is_ascii_digit)
}

fn supervise(
    project: &WebProject,
    shared: &Arc<Mutex<State>>,
) -> Result<(), String> {
    for attempt in 1..=3 {
        let reserved = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|e| format!("Cannot reserve a local preview port: {e}"))?;
        let port = reserved.local_addr().map_err(|e| e.to_string())?.port();
        let (done_tx, done_rx) = mpsc::channel();
        {
            let mut state = shared.lock().unwrap();
            if state.snapshot.status == Status::Stopped {
                return Ok(());
            }
            let mut command = project.command(port);
            // Most dev tools cannot inherit a pre-bound socket. Keep the
            // reservation until the last moment and retry an address-in-use exit.
            drop(reserved);
            let mut child = command.spawn().map_err(|e| {
                format!("Cannot launch {}: {e}", project.command_label())
            })?;
            drain(
                child.stdout.take().unwrap(),
                shared.clone(),
                done_tx.clone(),
            );
            drain(child.stderr.take().unwrap(), shared.clone(), done_tx);
            state.child = Some(child);
        }
        let started = Instant::now();
        let mut ready = false;
        let failure = loop {
            {
                let mut state = shared.lock().unwrap();
                let Some(child) = state.child.as_mut() else {
                    return Ok(());
                };
                let error = match child.try_wait() {
                    Ok(Some(status)) => Some(format!(
                        "{} exited with {status}.",
                        project.command_label()
                    )),
                    Err(e) => Some(format!("Could not monitor the web server: {e}")),
                    Ok(None) if !ready && started.elapsed() >= START_TIMEOUT => {
                        Some(format!(
                            "No HTTP response at {LOOPBACK}:{port}. A custom dev script must use the PORT environment variable."
                        ))
                    }
                    Ok(None) => None,
                };
                if let Some(error) = error {
                    state.terminate();
                    break error;
                }
            }
            if !ready && answers_http(port) {
                let mut state = shared.lock().unwrap();
                if state.snapshot.status == Status::Stopped {
                    return Ok(());
                }
                state.snapshot.status =
                    Status::Ready(format!("http://{LOOPBACK}:{port}/"));
                ready = true;
            }
            thread::sleep(Duration::from_millis(if ready { 200 } else { 100 }));
        };
        for _ in 0..2 {
            let _ = done_rx.recv_timeout(Duration::from_millis(150));
        }
        let mut state = shared.lock().unwrap();
        if state.snapshot.status == Status::Stopped {
            return Ok(());
        }
        let log = state.snapshot.output.to_ascii_lowercase();
        if ready
            || attempt == 3
            || !(log.contains("eaddrinuse")
                || log.contains("address already in use")
                || log.contains("is already in use"))
        {
            return Err(failure);
        }
        state
            .snapshot
            .output
            .push_str("\nPort in use. Trying another local port…\n");
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spin_until(
        server: &WebServer,
        predicate: impl Fn(&Status) -> bool,
    ) -> Snapshot {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            let snapshot = server.snapshot();
            if predicate(&snapshot.status) {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "server did not reach expected state: {snapshot:?}"
            );
            thread::sleep(Duration::from_millis(35));
        }
    }

    fn ready_port(server: &WebServer) -> u16 {
        let snapshot = spin_until(server, |s| !matches!(s, Status::Starting));
        let Status::Ready(url) = snapshot.status else {
            panic!("Expected Ready, got {snapshot:?}")
        };
        url.strip_prefix("http://127.0.0.1:")
            .unwrap()
            .strip_suffix('/')
            .unwrap()
            .parse()
            .unwrap()
    }

    fn request(port: u16, path: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\n\r\n"
        )
        .unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).unwrap();
        body
    }

    #[test]
    fn honors_declared_manager_lockfiles_and_dev_preference() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let manifest = root.join("package.json");
        fs::write(
            &manifest,
            r#"{"scripts":{"start":"next start","dev":"vite"}}"#,
        )
        .unwrap();
        assert_eq!(
            WebProject::detect(root).unwrap().command_label(),
            "npm run dev"
        );
        for (manager, lock) in [
            ("pnpm", "pnpm-lock.yaml"),
            ("yarn", "yarn.lock"),
            ("bun", "bun.lockb"),
            ("bun", "bun.lock"),
            ("npm", "package-lock.json"),
        ] {
            fs::write(root.join(lock), "").unwrap();
            assert_eq!(
                WebProject::detect(root).unwrap().command_label(),
                format!("{manager} run dev")
            );
            fs::remove_file(root.join(lock)).unwrap();
        }
        fs::write(root.join("yarn.lock"), "").unwrap();
        fs::write(
            &manifest,
            r#"{"packageManager":"pnpm@9.1.0","scripts":{"start":"next start"}}"#,
        )
        .unwrap();
        let project = WebProject::detect(root).unwrap();
        assert_eq!(project.command_label(), "pnpm run start");
        assert_eq!(
            project.command(1234).get_args().collect::<Vec<_>>(),
            ["run", "start", "--hostname", "127.0.0.1", "--port", "1234"]
        );
        fs::write(
            &manifest,
            r#"{"packageManager":"npm;echo bad","scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        assert!(WebProject::detect(root).is_err());
        fs::write(&manifest, r#"{"scripts":{"test":"node test.js"}}"#).unwrap();
        fs::write(root.join("index.html"), "").unwrap();
        assert!(matches!(
            WebProject::detect(root).unwrap().kind,
            ServerKind::Static
        ));
    }

    #[test]
    fn non_http_or_incomplete_status_never_becomes_ready() {
        for response in [
            &b"garbage data!\r\n"[..],
            b"HTTP/1.1 ",
            b"HTTP/1.1 999 Ignore\r\n",
        ] {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            let responder = thread::spawn(move || {
                let (mut client, _) = listener.accept().unwrap();
                let mut request = [0u8; 128];
                let _ = client.read(&mut request);
                client.write_all(response).unwrap();
            });
            assert!(!answers_http(port));
            responder.join().unwrap();
        }
    }

    #[test]
    #[cfg(unix)]
    fn static_site_serves_worktree_assets_and_blocks_escaping_symlinks() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("isolated project");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("index.html"), "<main>worktree preview</main>").unwrap();
        fs::write(root.join("site.css"), "body { color: red }").unwrap();
        fs::write(dir.path().join("private.txt"), "outside").unwrap();
        fs::write(root.join(".env"), "secret").unwrap();
        symlink(dir.path().join("private.txt"), root.join("link.txt")).unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        symlink(dir.path().join("private.txt"), root.join("sub/index.html"))
            .unwrap();
        let server = WebServer::start(WebProject::detect(&root).unwrap()).unwrap();
        let port = ready_port(&server);
        assert!(request(port, "/").ends_with("<main>worktree preview</main>"));
        assert!(request(port, "/site.css").ends_with("body { color: red }"));
        for path in ["/link.txt", "/sub/", "/.env"] {
            assert!(request(port, path).starts_with("HTTP/1.0 404"));
        }
        drop(server);
        assert!(!answers_http(port));
    }

    #[test]
    #[cfg(unix)]
    fn npm_retries_port_conflict_and_stops_the_server_process_tree() {
        if Command::new("npm")
            .arg("--version")
            .stdout(Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("npm is unavailable; skipping package-manager integration");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("package.json"),
            r#"{"scripts":{"dev":"node server.cjs"}}"#,
        )
        .unwrap();
        fs::write(
            root.join("server.cjs"),
            r#"
const fs = require('node:fs'), http = require('node:http');
if (!fs.existsSync('first-port')) {
  fs.writeFileSync('first-port', process.env.PORT);
  console.error('EADDRINUSE');
  process.exit(1);
}
console.log('http://example.invalid:9999/');
http.createServer((req, res) => res.end(process.cwd())).listen(
  Number(process.env.PORT), process.env.HOST);
"#,
        )
        .unwrap();
        let server = WebServer::start(WebProject::detect(root).unwrap()).unwrap();
        let port = ready_port(&server);
        assert!(
            request(port, "/")
                .ends_with(root.canonicalize().unwrap().to_str().unwrap())
        );
        assert!(
            server
                .snapshot()
                .output
                .contains("Trying another local port")
        );
        server.stop();
        assert_eq!(server.snapshot().status, Status::Stopped);
        assert!(!answers_http(port));
    }

    #[test]
    #[cfg(unix)]
    fn failed_script_retains_bounded_diagnostics_including_stderr() {
        if Command::new("npm")
            .arg("--version")
            .stdout(Stdio::null())
            .status()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"start":"node fail.cjs"}}"#,
        )
        .unwrap();
        fs::write(dir.path().join("fail.cjs"), r#"process.stderr.write('Ω'.repeat(100000) + '\nfixture-failure\n', () => process.exit(3))"#).unwrap();
        let server =
            WebServer::start(WebProject::detect(dir.path()).unwrap()).unwrap();
        let state = spin_until(&server, |s| matches!(s, Status::Failed(_)));
        assert!(state.output.len() <= OUTPUT_LIMIT);
        assert!(state.output.contains("fixture-failure"));
    }
}
