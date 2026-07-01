use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DevicePlatform {
    Android,
    Ios,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub platform: DevicePlatform,
    pub state: DeviceState,
    /// adb serial of the running instance (`emulator-<consolePort>`) for Android,
    /// or `None` when the device isn't running / for iOS (which has no adb
    /// serial). Lets the AI agent target the exact device the user is viewing
    /// when several Android emulators run at once.
    #[serde(default)]
    pub serial: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeviceState {
    Disconnected,
    Starting,
    Running,
}

#[async_trait]
pub trait MobileDevice {
    async fn connect(&mut self) -> Result<()>;
    async fn get_screenshot(&mut self) -> Result<Vec<u8>>;
    async fn send_touch(&mut self, x: i32, y: i32) -> Result<()>;
}
