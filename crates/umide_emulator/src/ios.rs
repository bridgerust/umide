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

    /// Get list of currently booted simulator UDIDs
    pub fn get_booted_udids() -> Vec<String> {
        let output = match Command::new("xcrun")
            .args(["simctl", "list", "--json", "devices", "booted"])
            .output() {
                Ok(out) => out,
                Err(_) => return Vec::new(),
            };

        if !output.status.success() {
            return Vec::new();
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let v: Value = match serde_json::from_str(&stdout) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let mut udids = Vec::new();
        if let Some(runtimes) = v.get("devices").and_then(|d| d.as_object()) {
            for (_runtime, devices) in runtimes {
                if let Some(devices) = devices.as_array() {
                    for device in devices {
                        if let Some(udid) = device.get("udid").and_then(|u| u.as_str()) {
                            udids.push(udid.to_string());
                        }
                    }
                }
            }
        }
        udids
    }

    /// Check if a simulator is currently booted
    pub fn is_running(udid: &str) -> bool {
        Self::get_booted_udids().contains(&udid.to_string())
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
        tracing::info!("Launching iOS Simulator for UDID: {}", udid);
        
        // Check if already booted
        if Self::is_running(udid) {
            tracing::info!("Simulator {} is already booted", udid);
            // Still open Simulator.app to ensure window is visible
            let _ = Command::new("open")
                .args(["-a", "Simulator"])
                .output();
            return Ok(());
        }
        
        // Boot the simulator synchronously — wait for it to complete
        let output = Command::new("xcrun")
            .args(["simctl", "boot", udid])
            .output()?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "already booted" errors
            if !stderr.contains("current state: Booted") {
                tracing::error!("Failed to boot simulator {}: {}", udid, stderr);
                return Err(anyhow!("Failed to boot simulator: {}", stderr));
            }
            tracing::info!("Simulator {} was already booted", udid);
        } else {
            tracing::info!("Simulator {} booted successfully", udid);
        }

        // Open Simulator.app so the window is visible for ScreenCaptureKit capture
        let _ = Command::new("open")
            .args(["-a", "Simulator"])
            .output()?;
        
        // Wait for the Simulator.app window to appear
        // ScreenCaptureKit needs the window to be on-screen
        tracing::info!("Waiting for Simulator.app window to appear...");
        for attempt in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            // Check if any simulator window exists
            let check = Command::new("osascript")
                .args([
                    "-e",
                    r#"tell application "System Events" to count windows of application process "Simulator""#,
                ])
                .output();
            
            if let Ok(out) = check {
                let count_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if let Ok(count) = count_str.parse::<i32>() {
                    if count > 0 {
                        tracing::info!("Simulator window appeared after {}ms", (attempt + 1) * 500);
                        // Give the window a moment to fully render
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        return Ok(());
                    }
                }
            }
        }
        
        tracing::warn!("Simulator window may not have appeared after 10s, proceeding anyway");
        Ok(())
    }

    pub fn stop(udid: &str) -> Result<()> {
        // Shutdown the simulator
        let output = Command::new("xcrun")
            .arg("simctl")
            .arg("shutdown")
            .arg(udid)
            .output()?;
        
        // simctl shutdown may return non-zero if already shutdown, which is OK
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "already shutdown" errors
            if !stderr.contains("current state: Shutdown") && !stderr.contains("Unable to shutdown") {
                return Err(anyhow!("Failed to shutdown iOS Simulator {}: {}", udid, stderr));
            }
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
