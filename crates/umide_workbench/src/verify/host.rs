//! The short-lived stdio relay runs under the CLI. Umide owns the actual
//! browser, simulator controls and build processes, including their cleanup.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, BufReader, Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
use tempfile::TempDir;
use uuid::Uuid;

use super::{Context, Observation, STOP, Service};

#[derive(Serialize, Deserialize)]
struct Endpoint {
    port: u16,
    token: String,
    checkout: PathBuf,
}

pub(crate) struct Host {
    directory: TempDir,
    address: SocketAddr,
    stopped: Arc<AtomicBool>,
    active: Arc<Mutex<Option<TcpStream>>>,
    pub observations: mpsc::Receiver<Observation>,
}

impl Host {
    pub fn start(checkout: &Path, context: Context) -> Result<Self, String> {
        Self::listen(checkout, context)
            .map_err(|e| format!("Could not start Umide verification: {e}"))
    }

    fn listen(checkout: &Path, context: Context) -> io::Result<Self> {
        let checkout = checkout.canonicalize()?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let token = Uuid::new_v4();
        let mut temp = tempfile::Builder::new();
        temp.prefix("umide-turn-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temp.permissions(fs::Permissions::from_mode(0o700));
        }
        let directory = temp.tempdir()?;
        let endpoint = Endpoint {
            port: address.port(),
            token: token.to_string(),
            checkout: checkout.clone(),
        };
        let mut secret = tempfile::NamedTempFile::new_in(directory.path())?;
        serde_json::to_writer(&mut secret, &endpoint)?;
        secret.persist(directory.path().join("endpoint.json"))?;
        let stopped = Arc::new(AtomicBool::new(false));
        let active = Arc::new(Mutex::new(None));
        let (sender, observations) = mpsc::channel();
        let stop = stopped.clone();
        let connection = active.clone();
        thread::Builder::new()
            .name("umide-verification".into())
            .spawn(move || {
                STOP.with_borrow_mut(|flag| *flag = Some(stop.clone()));
                let mut service = Service {
                    root: checkout,
                    context,
                    mobile: Default::default(),
                    web: Default::default(),
                    observations: sender,
                };
                while let Ok((mut socket, _)) = listener.accept() {
                    {
                        let mut current = connection.lock().unwrap();
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        let Ok(handle) = socket.try_clone() else {
                            continue;
                        };
                        *current = Some(handle);
                    }
                    let mut header = [0; 16];
                    let authenticated = socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .is_ok()
                        && socket.read_exact(&mut header).is_ok()
                        && &header == token.as_bytes();
                    if authenticated
                        && socket.set_read_timeout(None).is_ok()
                        && let Ok(read) = socket.try_clone()
                    {
                        let _ = service.serve(BufReader::new(read), &mut socket);
                    }
                    connection.lock().unwrap().take();
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                }
            })?;
        Ok(Self {
            directory,
            address,
            stopped,
            active,
            observations,
        })
    }

    pub fn config(&self) -> Result<umide_workbench_agents::VerifyConfig, String> {
        Ok(umide_workbench_agents::VerifyConfig {
            executable: std::env::current_exe().map_err(|e| e.to_string())?,
            event_directory: self.directory.path().to_owned(),
        })
    }

    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(connection) = self.active.lock().unwrap().as_ref() {
            let _ = connection.shutdown(Shutdown::Both);
        }
        // Wake accept when the CLI has never connected or is between connections.
        let _ =
            TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
    }
}

fn connect(checkout: &Path, directory: &Path) -> io::Result<TcpStream> {
    let endpoint: Endpoint =
        serde_json::from_reader(fs::File::open(directory.join("endpoint.json"))?)?;
    if !checkout.is_absolute()
        || !directory.is_absolute()
        || endpoint.checkout != checkout.canonicalize()?
    {
        return Err(io::Error::other(
            "This verification session belongs to a different checkout.",
        ));
    }
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port));
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    socket.set_nodelay(true)?;
    let token = Uuid::parse_str(&endpoint.token).map_err(io::Error::other)?;
    socket.write_all(token.as_bytes())?;
    Ok(socket)
}

pub(crate) fn relay(checkout: &Path, directory: &Path) -> io::Result<()> {
    let mut reader = connect(checkout, directory)?;
    let mut writer = reader.try_clone()?;
    thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut writer);
        let _ = writer.shutdown(Shutdown::Write);
    });
    let stdout = io::stdout();
    io::copy(&mut reader, &mut stdout.lock())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::io::BufRead;

    #[test]
    fn context_is_scoped_to_its_turn_and_disconnects_on_stop() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("site");
        fs::create_dir(&app).unwrap();
        fs::write(app.join("index.html"), "unpublished source").unwrap();
        let context = Context {
            isolated: true,
            project_name: "Demo".into(),
            provider: "Codex",
            device: Some(("ios".into(), "fixture-phone".into())),
            language: "Français",
            view: crate::verify::ViewContext {
                file: Some("main.swift".into()),
                dirty: true,
                ..Default::default()
            },
        };
        let host = Host::start(root.path(), context).unwrap();
        assert!(connect(&app, host.directory.path()).is_err());
        let mut stream = connect(root.path(), host.directory.path()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut read = BufReader::new(stream.try_clone().unwrap());
        let mut call = |method: &str, params: Value| {
            writeln!(
                stream,
                "{}",
                json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
            )
            .unwrap();
            let mut line = String::new();
            read.read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        };
        assert!(call("tools/list", json!({}))["error"].is_object());
        assert_eq!(
            call("initialize", json!({"protocolVersion":"2025-06-18"}))["result"]["protocolVersion"],
            "2025-06-18"
        );
        let result = call("tools/list", json!({}));
        let tools = result["result"]["tools"].as_array().unwrap();
        for name in ["workspace_context", "web_click", "mobile_launch"] {
            assert!(tools.iter().any(|tool| tool["name"] == name));
        }
        let context = |dir: &str| json!({"name":"workspace_context","arguments":{"project_dir":dir}});
        assert_eq!(
            call("tools/call", context("../"))["result"]["isError"],
            true
        );
        let result = call("tools/call", context("site"));
        let detail: Value = serde_json::from_str(
            result["result"]["content"][0]["text"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(detail["project_markers"], json!(["index.html"]));
        assert_eq!(detail["isolated"], true);
        assert_eq!(detail["selected_device"]["id"], "fixture-phone");
        assert_eq!(detail["open_file"]["unsaved_edits"], true);
        assert!(!detail.to_string().contains("unpublished source"));
        assert!(host.observations.try_recv().is_err());
        host.stop();
        assert_eq!(read.read_line(&mut String::new()).unwrap(), 0);
    }
}
