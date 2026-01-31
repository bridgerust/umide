use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
