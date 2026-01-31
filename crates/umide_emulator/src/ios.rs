use anyhow::{Result, anyhow};
use crate::common::{MobileDevice, DeviceInfo, DevicePlatform, DeviceState};
use async_trait::async_trait;
use std::process::Command;
use serde_json::Value;

pub struct IosSimulator {
    pub udid: String,
}

impl IosSimulator {
    pub fn new(udid: String) -> Self {
        Self { udid }
    }

    pub fn list_devices() -> Result<Vec<DeviceInfo>> {
        let output = Command::new("xcrun")
            .arg("simctl")
            .arg("list")
            .arg("--json")
            .arg("devices")
            .arg("available")
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("Failed to list iOS Simulators"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let v: Value = serde_json::from_str(&stdout)?;
        
        let mut devices = Vec::new();
        if let Some(runtimes) = v.get("devices").and_then(|d| d.as_object()) {
            for (_runtime, run_devices) in runtimes {
                if let Some(run_devices) = run_devices.as_array() {
                    for device in run_devices {
                        let name = device.get("name").and_then(|n| n.as_str()).unwrap_or("Unknown").to_string();
                        let udid = device.get("udid").and_then(|u| u.as_str()).unwrap_or("").to_string();
                        let state_str = device.get("state").and_then(|s| s.as_str()).unwrap_or("Shutdown");
                        
                        let state = if state_str == "Booted" {
                            DeviceState::Running
                        } else {
                            DeviceState::Disconnected
                        };

                        if !udid.is_empty() {
                            devices.push(DeviceInfo {
                                id: udid,
                                name,
                                platform: DevicePlatform::Ios,
                                state,
                            });
                        }
                    }
                }
            }
        }

        Ok(devices)
    }

    pub fn launch(udid: &str) -> Result<()> {
        // Boot the simulator (no external window)
        Command::new("xcrun")
            .arg("simctl")
            .arg("boot")
            .arg(udid)
            .spawn()?;
            
        // Note: We don't open Simulator.app - we capture screenshots directly
        // via simctl without needing the visible window
            
        Ok(())
    }

    pub fn stop(udid: &str) -> Result<()> {
        let output = Command::new("xcrun")
            .arg("simctl")
            .arg("shutdown")
            .arg(udid)
            .output()?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to shutdown iOS Simulator {}: {}", udid, stderr));
        }
        
        Ok(())
    }

    pub fn detect_simulator() -> Result<Option<String>> {
        // Stub: In reality, run `xcrun simctl list` and parse output
        println!("Detecting iOS Simulator...");
        Ok(Some("00000000-0000-0000-0000-000000000000".to_string()))
    }
}

#[async_trait]
impl MobileDevice for IosSimulator {
    async fn connect(&mut self) -> Result<()> {
        println!("Connecting to iOS Simulator {}", self.udid);
        Ok(())
    }

    async fn get_screenshot(&mut self) -> Result<Vec<u8>> {
        println!("Getting screenshot from iOS Simulator {}", self.udid);
        // Stub: `xcrun simctl io <udid> screenshot -`
        Ok(vec![])
    }

    async fn send_touch(&mut self, x: i32, y: i32) -> Result<()> {
        println!("Sending touch to {}: ({}, {})", self.udid, x, y);
        // Stub: `xcrun simctl ui <udid> tap x y` is not a real command strictly speaking, 
        // usually needs AppleScript or idb. 
        // We will implement what we can later.
        Ok(())
    }
}
