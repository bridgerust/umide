//! Local protocol checks with the installed CLIs. No user message or model turn
//! is sent. The macOS network policy permits only this fixture's MCP listener.
#![cfg(target_os = "macos")]

use super::{Context, Host, ViewContext};
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};
use umide_workbench_agents::{Provider, cli_environment};

struct Client {
    child: Child,
    input: ChildStdin,
    lines: Receiver<Value>,
}

impl Client {
    fn new(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Ok(message) = serde_json::from_str(&line)
                    && tx.send(message).is_err()
                {
                    break;
                }
            }
        });
        let input = child.stdin.take().unwrap();
        Self {
            child,
            input,
            lines,
        }
    }
    fn send(&mut self, message: Value) {
        writeln!(self.input, "{message}").unwrap();
        self.input.flush().unwrap();
    }
    fn request(
        &mut self,
        message: Value,
        matches: impl Fn(&Value) -> bool,
    ) -> Value {
        self.send(message);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let msg = self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("CLI protocol request timed out");
            if matches(&msg) {
                return msg;
            }
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "requires both installed CLIs and a built Workbench at UMIDE_TEST_BINARY"]
fn installed_clients_connect_to_umide_without_a_model_turn() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Démo mobile");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("index.html"), "<title>Umide</title>").unwrap();
    let binary = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(
            env::var_os("UMIDE_TEST_BINARY")
                .expect("set UMIDE_TEST_BINARY to the built Workbench"),
        )
        .canonicalize()
        .unwrap();
    for provider in [Provider::Codex, Provider::Claude] {
        let host = Host::start(
            &root,
            Context {
                isolated: true,
                project_name: "protocol fixture".into(),
                provider: "test",
                device: None,
                language: "en",
                view: ViewContext::default(),
            },
        )
        .unwrap();
        let dir = host.config().unwrap().event_directory;
        let endpoint: Value =
            serde_json::from_slice(&fs::read(dir.join("endpoint.json")).unwrap())
                .unwrap();
        let port = endpoint["port"].as_u64().unwrap();
        let server = json!({"command":binary,"args":["--umide-verify",root.canonicalize().unwrap(),dir]});
        let (cli, path) = cli_environment(provider).unwrap();
        let settings = temp.path().join(format!("{provider:?}"));
        fs::create_dir(&settings).unwrap();
        let mut cmd = Command::new("/usr/bin/sandbox-exec");
        let policy = format!(
            r#"(version 1)(allow default)(deny network-outbound)(allow network-outbound (remote ip "localhost:{port}"))(deny mach-lookup (global-name "com.apple.securityd"))"#
        );
        cmd.arg("-p")
            .arg(policy)
            .arg(cli)
            .current_dir(&root)
            .env_clear()
            .env("PATH", path)
            .env("HOME", temp.path())
            .env("TMPDIR", temp.path())
            .env("CODEX_HOME", &settings)
            .env("CLAUDE_CONFIG_DIR", &settings)
            .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")
            .env("OPENAI_BASE_URL", "http://127.0.0.1:1")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
        match provider {
            Provider::Codex => {
                cmd.args(["app-server", "--stdio"]);
                let mut client = Client::new(cmd);
                let rpc = |client: &mut Client, id, method: &str, params: Value| {
                    let reply = client.request(
                        json!({"id":id,"method":method,"params":params}),
                        |r| r["id"] == id,
                    );
                    assert!(reply.get("error").is_none(), "{reply}");
                    reply["result"].clone()
                };
                rpc(
                    &mut client,
                    1,
                    "initialize",
                    json!({"clientInfo":{"name":"umide","version":"0.4"},"capabilities":null}),
                );
                client.send(json!({"method":"initialized"}));
                let thread = rpc(
                    &mut client,
                    2,
                    "thread/start",
                    json!({"ephemeral":true,"cwd":root,"sandbox":"workspace-write","approvalPolicy":"on-request","config":{"mcp_servers.umide":server}}),
                );
                let id = thread["thread"]["id"].as_str().unwrap();
                let inventory = rpc(
                    &mut client,
                    3,
                    "mcpServerStatus/list",
                    json!({"serverName":"umide","threadId":id}),
                );
                let tools = &inventory["data"][0]["tools"];
                for name in ["workspace_context", "mobile_launch", "web_inspect"] {
                    assert!(tools.get(name).is_some(), "{inventory}");
                }
                let result = rpc(
                    &mut client,
                    4,
                    "mcpServer/tool/call",
                    json!({"threadId":id,"server":"umide","tool":"workspace_context","arguments":{}}),
                );
                let detail: Value = serde_json::from_str(
                    result["content"][0]["text"].as_str().expect("tool text"),
                )
                .unwrap();
                assert_eq!(detail["project"], "protocol fixture");
                assert_eq!(detail["checkout"], json!(root.canonicalize().unwrap()));
                assert!(
                    detail["project_markers"]
                        .as_array()
                        .unwrap()
                        .contains(&json!("index.html"))
                );
            }
            Provider::Claude => {
                cmd.args([
                    "--bare",
                    "--no-session-persistence",
                    "--strict-mcp-config",
                    "--print",
                    "--verbose",
                    "--input-format",
                    "stream-json",
                    "--output-format",
                    "stream-json",
                    "--mcp-config",
                ])
                .arg(json!({"mcpServers":{"umide":server}}).to_string());
                let mut client = Client::new(cmd);
                let control = |client: &mut Client, id: &str, subtype: &str| {
                    client.request(json!({"type":"control_request","request_id":id,"request":{"subtype":subtype}}), |r| r["type"]=="control_response" && r["response"]["request_id"]==id)
                };
                control(&mut client, "1", "initialize");
                let deadline = Instant::now() + Duration::from_secs(20);
                loop {
                    let reply = control(&mut client, "2", "mcp_status");
                    let rows = reply
                        .pointer("/response/response/mcpServers")
                        .and_then(Value::as_array)
                        .expect("MCP inventory");
                    if let Some(row) = rows
                        .iter()
                        .find(|s| s["name"] == "umide" && s["status"] == "connected")
                    {
                        let tools = row["tools"].as_array().unwrap();
                        for name in
                            ["workspace_context", "mobile_launch", "web_inspect"]
                        {
                            assert!(
                                tools.iter().any(|t| t["name"] == name),
                                "{row}"
                            );
                        }
                        break;
                    }
                    assert!(Instant::now() < deadline, "{reply}");
                    thread::sleep(Duration::from_millis(150));
                }
            }
        }
    }
}
