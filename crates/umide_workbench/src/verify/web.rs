//! A turn owns its preview process and an isolated, headless browser profile.
use std::{
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::web_server::{Status, WebProject, WebServer};

#[derive(Default)]
pub(super) struct Web {
    driver: Option<Driver>,
    server: Option<WebServer>,
    project: Option<PathBuf>,
}

pub(super) fn tools() -> Vec<Value> {
    let control = json!({"type":"integer","minimum":1,"description":"Control number from the most recent inspection."});
    let text = json!({"type":"string","maxLength":4096});
    let offset = json!({"type":"integer","minimum":0});
    [
        ("web_start", "Start or reuse this checkout’s development server and inspect it in a fresh desktop browser. Requires Node.js and an installed Chromium-family browser.", json!({}), vec![]),
        ("web_open", "Navigate the test browser to a path on its own development server and inspect the result.", json!({"path":{"type":"string","description":"A site path beginning with /, optionally with query and fragment.","maxLength":2048}}), vec!["path"]),
        ("web_inspect", "Read up to 1400 visible characters and 20 numbered controls. Use the returned offsets for more.", json!({"text_offset":offset,"control_offset":offset}), vec![]),
        ("web_fill", "Replace a visible text field or textarea using real browser input. Returns a short acknowledgment.", json!({"control":control,"text":text}), vec!["control","text"]),
        ("web_click", "Click a numbered control using a browser pointer. Use web_wait to check the expected result.", json!({"control":control}), vec!["control"]),
        ("web_wait", "Wait up to five seconds for visible text in the page and return the observed result. Match is case-sensitive.", json!({"text":text}), vec!["text"]),
        ("web_screenshot", "Capture the 1024 × 768 viewport only when visual evidence is needed.", json!({}), vec![]),
    ].into_iter().map(|(name, description, properties, required)| json!({
        "name":name, "description":description, "inputSchema":{
            "type":"object","properties":properties,"required":required,"additionalProperties":false
        }
    })).collect()
}

impl Web {
    pub(super) fn url(&self) -> Option<String> {
        match self.server.as_ref()?.snapshot().status {
            Status::Ready(url) => Some(url),
            _ => None,
        }
    }

    pub(super) fn call(
        &mut self,
        name: &str,
        args: &Value,
        root: &Path,
    ) -> Result<Vec<Value>, String> {
        if name == "web_start" {
            let root = root
                .canonicalize()
                .map_err(|e| format!("Cannot open project: {e}"))?;
            let restart = self.project.as_ref() != Some(&root)
                || self.server.as_ref().is_none_or(|s| {
                    matches!(
                        s.snapshot().status,
                        Status::Failed(_) | Status::Stopped
                    )
                });
            if restart {
                let project = WebProject::detect(&root)?;
                self.driver = None;
                self.server = None;
                self.project = None;
                self.server = Some(WebServer::start(project)?);
                self.project = Some(root);
            }
        }
        let server = self
            .server
            .as_ref()
            .ok_or("Run web_start for this checkout first.")?;
        let deadline = Instant::now() + Duration::from_secs(8);
        let url = loop {
            if super::cancelled() {
                return Err("Web verification was cancelled.".into());
            }
            let snapshot = server.snapshot();
            match snapshot.status {
                Status::Ready(url) => break url,
                Status::Failed(error) => {
                    let output = &snapshot.output;
                    let from = output.char_indices().rev().nth(1199).map_or(0, |(pos, _)| pos);
                    return Err(format!("{error}\n{}", &output[from..]));
                }
                Status::Stopped => return Err("The development server stopped. Run web_start again.".into()),
                Status::Starting if Instant::now() < deadline => thread::sleep(Duration::from_millis(100)),
                Status::Starting => return Err("The development server is still starting. Retry web_start in a moment.".into()),
            }
        };
        if self.driver.is_none() {
            let mut driver = Driver::start(&url)?;
            let result = driver.request("web_open", &json!({"path":"/"}))?;
            self.driver = Some(driver);
            if name == "web_start" {
                return Ok(vec![result]);
            }
        }
        let driver = self.driver.as_mut().unwrap();
        let command = if name == "web_start" {
            "web_inspect"
        } else {
            name
        };
        let result = driver.request(command, args);
        if !driver.connected {
            self.driver = None;
        }
        result.map(|block| vec![block])
    }
}

struct Driver {
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<String>,
    connected: bool,
    _profile: tempfile::TempDir,
}

fn node() -> PathBuf {
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    let mut dirs: Vec<_> = env::var_os("PATH")
        .as_deref()
        .map(env::split_paths)
        .into_iter()
        .flatten()
        .filter(|dir| dir.is_absolute())
        .collect();
    #[cfg(unix)]
    dirs.extend([
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
        "/usr/bin".into(),
    ]);
    dirs.into_iter()
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
        .unwrap_or_else(|| name.into())
}

impl Driver {
    fn start(url: &str) -> Result<Self, String> {
        let profile = tempfile::Builder::new()
            .prefix("umide-web-test-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let script = profile.path().join("driver.cjs");
        fs::write(&script, include_str!("web_driver.cjs"))
            .map_err(|e| e.to_string())?;
        let mut command = Command::new(node());
        command
            .arg(&script)
            .arg(url)
            .arg(profile.path().join("profile"))
            .current_dir(profile.path())
            .env_remove("NODE_OPTIONS")
            .env_remove("NODE_PATH")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().map_err(|e| format!("Cannot start the browser driver. Install Node.js and make node available to Umide: {e}"))?;
        let input = child.stdin.take();
        let output = child.stdout.take().unwrap();
        let (send, receive) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            input,
            output: receive,
            connected: true,
            _profile: profile,
        })
    }

    fn request(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let input = self.input.as_mut().unwrap();
        // Submit one complete frame so an anonymous Windows pipe does not have
        // to deliver a second write before readline can finish the request.
        let frame = format!("{}\n", json!({"name":name,"args":args}));
        if let Err(error) = input
            .write_all(frame.as_bytes())
            .and_then(|_| input.flush())
        {
            self.connected = false;
            return Err(format!(
                "Could not send {name} to the browser driver: {error}"
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(35);
        let mut phase = "waiting for the Node driver".to_owned();
        let reply = loop {
            if super::cancelled() {
                self.connected = false;
                return Err("Web verification was cancelled.".into());
            }
            match self.output.recv_timeout(Duration::from_millis(80)) {
                Ok(line) => {
                    let reply: Value = serde_json::from_str(&line).map_err(|e| {
                        format!(
                            "Invalid browser response to {name} during {phase}: {e}"
                        )
                    })?;
                    if let Some(activity) = reply["phase"].as_str() {
                        phase = activity.to_owned();
                    } else {
                        break reply;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout)
                    if Instant::now() < deadline => {}
                Err(reason) => {
                    self.connected = false;
                    let state = match self.child.try_wait() {
                        Ok(Some(status)) => format!("Node exited with {status}"),
                        _ if reason == mpsc::RecvTimeoutError::Timeout => {
                            "Node timed out".into()
                        }
                        _ => "Node closed stdout".into(),
                    };
                    return Err(format!(
                        "{name} failed while {phase}: {state}. Run web_start to reopen the browser."
                    ));
                }
            }
        };
        self.connected = reply["closed"] != true;
        if let Some(error) = reply["error"].as_str() {
            return Err(error.into());
        }
        reply
            .get("content")
            .cloned()
            .ok_or_else(|| format!("No browser result for {name} during {phase}."))
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.input.take(); // EOF asks the driver to close Chromium and release its profile.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        crate::projects::terminate(&mut self.child);
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "exercises an installed Chromium browser, Node.js and loopback HTTP"]
    fn local_form_in_a_real_browser() {
        use std::net::{TcpListener, TcpStream};

        let root = tempfile::tempdir().unwrap();
        let other = TcpListener::bind("127.0.0.1:0").unwrap();
        other.set_nonblocking(true).unwrap();
        let foreign_port = other.local_addr().unwrap().port();
        let controls = "<button>More</button>".repeat(22);
        let filler = "Body ".repeat(340);
        fs::write(root.path().join("index.html"), format!(r#"<!doctype html>
            <title>Test form</title>
            <form onsubmit="event.preventDefault(); setTimeout(() => document.querySelector('output').textContent = 'Saved ' + this.elements.email.value, 100)">
                <label>Email <input name="email" type="email" required></label>
                <button>Save</button>
            </form>
            <output></output>
            <input type="password" aria-label="Secret" value="not-in-observations">
            <img src="http://127.0.0.1:{foreign_port}/outside">
            <p>{filler}</p>{controls}"#)).unwrap();
        let mut web = Web::default();
        let mut observe = |name, args| {
            let content = web
                .call(name, &args, root.path())
                .unwrap_or_else(|e| panic!("{name} failed: {e}"));
            serde_json::from_str::<Value>(content[0]["text"].as_str().unwrap())
                .unwrap()
        };
        let page = observe("web_start", json!({}));
        assert_eq!(page["title"], "Test form");
        assert_eq!(page["controls"][0]["label"], "Email");
        assert_eq!(page["controls"].as_array().unwrap().len(), 20);
        assert!(!page.to_string().contains("not-in-observations"));
        assert_eq!(page["text"].as_str().unwrap().chars().count(), 1400);
        let next = observe(
            "web_inspect",
            json!({"text_offset": page["next_text_offset"], "control_offset": page["next_control_offset"]}),
        );
        assert_eq!(next["controls"][0]["control"], 21);
        assert_eq!(
            observe(
                "web_fill",
                json!({"control":1,"text":"reader@example.test"})
            ),
            json!({"filled":1})
        );
        assert_eq!(
            observe("web_click", json!({"control":2})),
            json!({"clicked":2})
        );
        assert_eq!(
            observe("web_wait", json!({"text":"Saved reader@example.test"})),
            json!({"observed":"Saved reader@example.test"})
        );
        let shot = web.call("web_screenshot", &json!({}), root.path()).unwrap();
        assert_eq!(shot[0]["mimeType"], "image/jpeg");
        assert!(shot[0]["data"].as_str().unwrap().starts_with("/9j/"));
        assert!(
            web.call("web_open", &json!({"path":"//127.0.0.1/"}), root.path())
                .is_err()
        );
        assert_eq!(
            other.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let addr = web
            .url()
            .unwrap()
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_owned();
        drop(web);
        assert!(TcpStream::connect(addr).is_err());
    }
}
