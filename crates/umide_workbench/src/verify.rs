//! Per-turn tools for inspecting the conversation's own checkout and local UI.
mod host;
#[cfg(test)]
mod installed_providers;
mod mobile;
mod web;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    io::{self, BufRead, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub(crate) use host::{Host, relay};

thread_local! {
    static STOP: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

fn cancelled() -> bool {
    STOP.with_borrow(|flag| flag.as_ref().is_some_and(|f| f.load(Ordering::Acquire)))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ViewContext {
    pub file: Option<PathBuf>,
    pub dirty: bool,
    pub preview: Option<String>,
    pub panel: Option<&'static str>,
}

pub(crate) struct Context {
    pub isolated: bool,
    pub project_name: String,
    pub provider: &'static str,
    pub device: Option<(String, String)>,
    pub language: &'static str,
    pub view: ViewContext,
}

impl Context {
    fn describe(&self, checkout: &Path, dir: &Path) -> Value {
        let mut markers: Vec<String> = [
            "package.json",
            "index.html",
            "pubspec.yaml",
            "app.json",
            "android",
            "ios",
            "gradlew",
            "gradlew.bat",
            "settings.gradle",
            "settings.gradle.kts",
            "Package.swift",
            "Cargo.toml",
            "pyproject.toml",
        ]
        .into_iter()
        .filter(|name| dir.join(name).exists())
        .map(str::to_owned)
        .collect();
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut xcode: Vec<_> = entries
                .take(512)
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    matches!(
                        p.extension().and_then(|s| s.to_str()),
                        Some("xcodeproj" | "xcworkspace")
                    )
                })
                .filter_map(|p| {
                    p.file_name().map(|n| n.to_string_lossy().into_owned())
                })
                .collect();
            xcode.sort();
            markers.extend(xcode.into_iter().take(8));
        }
        let mut result = json!({
            "app":"Umide Workbench","agent":self.provider,"project":self.project_name,
            "checkout":checkout,"isolated":self.isolated,"host":std::env::consts::OS,
            "interface_language":self.language,"project_markers":markers
        });
        if dir != checkout
            && let Ok(relative) = dir.strip_prefix(checkout)
        {
            result["project_dir"] = json!(relative);
        }
        if let Some((platform, id)) = &self.device {
            result["selected_device"] = json!({"platform":platform,"id":id});
        }
        if let Some(file) = &self.view.file {
            result["open_file"] =
                json!({"relative_path":file,"unsaved_edits":self.view.dirty});
        }
        if let Some(url) = &self.view.preview {
            result["manual_web_preview"] = json!(url);
        }
        if let Some(panel) = self.view.panel {
            result["visible_panel_at_send"] = json!(panel);
        }
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BrowserObservation {
    pub action: &'static str,
    pub page: String,
    pub capture: Option<Arc<Vec<u8>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Preview {
    Mobile(String, String),
    Web,
}

pub(crate) enum Observation {
    Mobile(String, String),
    Web(BrowserObservation),
}

struct Service {
    root: PathBuf,
    context: Context,
    mobile: mobile::Mobile,
    web: web::Web,
    observations: mpsc::Sender<Observation>,
}

fn tools() -> Vec<Value> {
    let mut list = vec![json!({
        "name":"workspace_context","description":"Read the current Umide task checkout, open file, selected device and project markers on demand.",
        "inputSchema":{"type":"object","properties":{},"additionalProperties":false}
    })];
    list.extend(mobile::tools());
    list.extend(web::tools());
    for tool in &mut list {
        if matches!(
            tool["name"].as_str(),
            Some("workspace_context" | "mobile_launch" | "web_start")
        ) {
            tool["inputSchema"]["properties"]["project_dir"] = json!({
                "type":"string","description":"Optional application directory relative to the task checkout."
            });
        }
    }
    list
}

impl Service {
    fn call(&mut self, name: &str, mut args: Value) -> Result<Vec<Value>, String> {
        let args_map = args
            .as_object_mut()
            .ok_or("Expected an object for tool arguments.")?;
        let mut dir = self.root.clone();
        if let Some(path) = args_map.remove("project_dir") {
            if !matches!(name, "workspace_context" | "web_start" | "mobile_launch") {
                return Err(
                    "Select the application directory when starting it.".into()
                );
            }
            let relative = path
                .as_str()
                .ok_or("Expected a relative project directory.")?;
            dir = crate::project_files::resolve(&dir, Path::new(relative))?;
            if !dir.is_dir() {
                return Err("The selected project path must be a directory.".into());
            }
        }
        if name == "workspace_context" {
            return Ok(vec![
                json!({"type":"text","text":self.context.describe(&self.root,&dir).to_string()}),
            ]);
        }
        if name.starts_with("mobile_") {
            if matches!(name, "mobile_boot" | "mobile_launch")
                && !args_map.contains_key("device_id")
                && let Some((platform, id)) = &self.context.device
                && args_map
                    .get("platform")
                    .and_then(Value::as_str)
                    .is_none_or(|p| p == platform)
                && !self.mobile.has_selection()
            {
                args_map.insert("platform".into(), json!(platform));
                args_map.insert("device_id".into(), json!(id));
            }
            let output = self.mobile.call(name, &args, &dir)?;
            if name != "mobile_devices"
                && let Some((p, id)) = self.mobile.target()
            {
                let _ = self.observations.send(Observation::Mobile(p, id));
            }
            return Ok(output);
        }
        let output = self.web.call(name, &args, &dir)?;
        if self.web.url().is_some() {
            let capture = output
                .iter()
                .find(|c| {
                    c["type"] == "image"
                        && matches!(
                            c["mimeType"].as_str(),
                            Some("image/png" | "image/jpeg")
                        )
                })
                .and_then(|c| c["data"].as_str())
                .and_then(|s| STANDARD.decode(s).ok())
                .map(Arc::new);
            let text = output
                .iter()
                .find_map(|c| c["text"].as_str())
                .unwrap_or_default();
            let page = match serde_json::from_str::<Value>(text) {
                Ok(page) => [
                    page["title"].as_str(),
                    page["path"].as_str(),
                    page["text"].as_str(),
                    page["observed"].as_str(),
                ]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
                Err(_) => text.to_owned(),
            };
            let _ = self.observations.send(Observation::Web(BrowserObservation {
                action: match name {
                    "web_fill" => "Filled field",
                    "web_click" => "Clicked control",
                    "web_wait" => "Observed text",
                    "web_screenshot" => "Captured page",
                    "web_inspect" => "Inspected page",
                    _ => "Opened test page",
                },
                page: page.chars().take(500).collect(),
                capture,
            }));
        }
        Ok(output)
    }

    fn serve(
        &mut self,
        mut reader: impl BufRead,
        mut writer: impl Write,
    ) -> io::Result<()> {
        let mut initialized = false;
        let catalog = tools();
        let mut line = Vec::new();
        while !cancelled() {
            line.clear();
            if reader.by_ref().take(262_145).read_until(b'\n', &mut line)? == 0 {
                break;
            }
            if line.len() > 262_144 {
                return Err(io::Error::other("Oversized MCP request"));
            }
            let request: Value =
                serde_json::from_slice(&line).unwrap_or(Value::Null);
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let response = if request["jsonrpc"] != "2.0"
                || !request["method"].is_string()
            {
                json!({"error":{"code":-32600,"message":"Invalid JSON-RPC request"}})
            } else if id.is_null() {
                continue;
            } else {
                let params = &request["params"];
                match request["method"].as_str().unwrap() {
                    "initialize" => {
                        initialized = true;
                        let version = params["protocolVersion"]
                            .as_str()
                            .filter(|v| {
                                matches!(
                                    *v,
                                    "2024-11-05" | "2025-03-26" | "2025-06-18"
                                )
                            })
                            .unwrap_or("2025-11-25");
                        json!({"result":{"protocolVersion":version,"capabilities":{"tools":{}},
                            "serverInfo":{"name":"umide","version":env!("CARGO_PKG_VERSION")}}})
                    }
                    "ping" => json!({"result":{}}),
                    _ if !initialized => {
                        json!({"error":{"code":-32002,"message":"Initialize Umide first."}})
                    }
                    "tools/list" => json!({"result":{"tools":catalog}}),
                    "tools/call" => {
                        let name = params["name"].as_str().unwrap_or_default();
                        if !catalog.iter().any(|t| t["name"] == name) {
                            json!({"error":{"code":-32602,"message":"Unknown Umide tool"}})
                        } else {
                            match self.call(
                                name,
                                params
                                    .get("arguments")
                                    .cloned()
                                    .unwrap_or_else(|| json!({})),
                            ) {
                                Ok(content) => json!({"result":{"content":content}}),
                                Err(error) => {
                                    json!({"result":{"isError":true,"content":[{"type":"text","text":error}]}})
                                }
                            }
                        }
                    }
                    _ => {
                        json!({"error":{"code":-32601,"message":"Method not found"}})
                    }
                }
            };
            let mut response = response;
            response["jsonrpc"] = json!("2.0");
            response["id"] = id;
            serde_json::to_writer(&mut writer, &response)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
        Ok(())
    }
}
