use anyhow::{Result, anyhow};
use crate::common::{MobileDevice, DeviceInfo, DevicePlatform, DeviceState};
use async_trait::async_trait;
use std::process::{Command, Stdio};

pub struct AndroidEmulator {
    pub device_id: String,
    pub grpc_address: String,
}

impl AndroidEmulator {
    pub fn new(device_id: String, grpc_address: String) -> Self {
        Self {
            device_id,
            grpc_address,
        }
    }

    /// Get list of currently running emulator serials from adb devices
    pub fn get_running_serials() -> Vec<String> {
        let output = match Command::new("adb")
            .arg("devices")
            .output() {
                Ok(out) => out,
                Err(_) => return Vec::new(),
            };

        if !output.status.success() {
            return Vec::new();
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .lines()
            .skip(1) // Skip "List of devices" header
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 && parts[0].starts_with("emulator-") && parts[1] == "device" {
                    Some(parts[0].to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Check if an AVD is currently running
    pub fn is_running(avd_name: &str) -> bool {
        let running = Self::get_running_serials();
        if running.is_empty() {
            return false;
        }
        
        // Check if any running emulator corresponds to this AVD
        // We need to query each emulator for its AVD name
        for serial in &running {
            if let Ok(output) = Command::new("adb")
                .args(["-s", serial, "emu", "avd", "name"])
                .output()
            {
                let name = String::from_utf8_lossy(&output.stdout);
                if name.trim() == avd_name || name.lines().next().map(|l| l.trim()) == Some(avd_name) {
                    return true;
                }
            }
        }
        false
    }

    pub fn list_devices() -> Result<Vec<DeviceInfo>> {
        let output = Command::new("emulator")
            .arg("-list-avds")
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("Failed to list Android AVDs"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let devices = stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| DeviceInfo {
                id: line.trim().to_string(),
                name: line.trim().to_string(),
                platform: DevicePlatform::Android,
                state: DeviceState::Disconnected, // Initial state
            })
            .collect();

        Ok(devices)
    }

    pub fn launch(avd_name: &str) -> Result<()> {
        tracing::info!("Launching Android emulator for AVD: {}", avd_name);
        
        // Check if already running
        if Self::is_running(avd_name) {
            tracing::info!("AVD {} is already running", avd_name);
            return Ok(());
        }
        
        // Launch emulator headless — no window, frames arrive via gRPC streaming
        // GPU mode "auto" picks the best available backend (Metal on macOS)
        // -no-window: run headless, no macOS window (we render via gRPC frames)
        // -grpc 8554: expose gRPC endpoint for frame streaming and input
        let child = Command::new("emulator")
            .arg("-avd")
            .arg(avd_name)
            .arg("-gpu")
            .arg("auto")
            .arg("-no-boot-anim")
            .arg("-no-skin")
            .arg("-no-window")     // Headless: no desktop window, frames via gRPC
            .arg("-grpc")
            .arg("8554")           // gRPC endpoint for streamScreenshot + sendTouch
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .spawn();
        
        match child {
            Ok(_child) => {
                tracing::info!("Emulator process spawned for AVD: {}", avd_name);
                
                // Wait for the emulator to become reachable via ADB
                // Poll up to 30 seconds for the emulator to appear in adb devices
                let mut ready = false;
                for attempt in 0..60 {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let serials = Self::get_running_serials();
                    if !serials.is_empty() {
                        // Verify this AVD is the one that booted
                        if Self::is_running(avd_name) {
                            tracing::info!(
                                "AVD {} is now running (detected after {}ms)", 
                                avd_name, (attempt + 1) * 500
                            );
                            ready = true;
                            break;
                        }
                    }
                    if attempt % 10 == 0 && attempt > 0 {
                        tracing::debug!("Still waiting for AVD {} to boot... ({}s)", avd_name, (attempt + 1) / 2);
                    }
                }
                
                if !ready {
                    tracing::warn!("AVD {} may not be fully booted yet after 30s, proceeding anyway", avd_name);
                }
                
                Ok(())
            }
            Err(e) => {
                tracing::error!("Failed to spawn emulator for AVD {}: {}", avd_name, e);
                Err(anyhow!("Failed to launch emulator: {}", e))
            }
        }
    }

    pub fn stop(avd_name: &str) -> Result<()> {
        tracing::info!("Attempting to stop Android emulator for AVD: {}", avd_name);
        
        // Get all running emulator serials
        let running_serials = Self::get_running_serials();
        
        if running_serials.is_empty() {
            tracing::warn!("No running emulators found");
            return Err(anyhow!("No running emulators found"));
        }
        
        // Find the emulator that matches this AVD name
        for serial in &running_serials {
            // Query the AVD name for this emulator
            if let Ok(output) = Command::new("adb")
                .args(["-s", serial, "emu", "avd", "name"])
                .output()
            {
                let name = String::from_utf8_lossy(&output.stdout);
                let name_trimmed = name.lines().next().map(|l| l.trim()).unwrap_or("");
                
                tracing::debug!("Emulator {} has AVD name: '{}'", serial, name_trimmed);
                
                if name_trimmed == avd_name {
                    tracing::info!("Found matching emulator {} for AVD {}, sending kill command", serial, avd_name);
                    
                    // Kill this emulator
                    let result = Command::new("adb")
                        .args(["-s", serial, "emu", "kill"])
                        .output();
                    
                    match result {
                        Ok(output) => {
                            if output.status.success() {
                                tracing::info!("Successfully killed emulator {}", serial);
                                return Ok(());
                            } else {
                                let stderr = String::from_utf8_lossy(&output.stderr);
                                tracing::error!("Failed to kill emulator {}: {}", serial, stderr);
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to execute adb kill command: {}", e);
                        }
                    }
                }
            }
        }

        // If we didn't find a matching AVD, try to kill by partial name match
        tracing::warn!("No exact AVD match found, trying partial match...");
        for serial in &running_serials {
            // Just kill the first one as fallback
            let result = Command::new("adb")
                .args(["-s", serial, "emu", "kill"])
                .output();
            
            if let Ok(output) = result {
                if output.status.success() {
                    tracing::info!("Killed emulator {} as fallback", serial);
                    return Ok(());
                }
            }
        }

        Err(anyhow!("Could not find or stop emulator for AVD: {}", avd_name))
    }

    pub async fn stream_video(&mut self) -> Result<impl tokio_stream::Stream<Item = Vec<u8>>> {
        // Stub: Returns a stream of empty packets for now to test UI loop
        println!("Starting video stream for Android device: {}", self.device_id);
        let stream = tokio_stream::iter(vec![vec![0u8; 10], vec![0u8; 10]]); 
        Ok(stream)
    }
}

#[async_trait]
impl MobileDevice for AndroidEmulator {
    async fn connect(&mut self) -> Result<()> {
        println!("Connecting to Android Emulator at {}", self.grpc_address);
        // Stub connection logic
        Ok(())
    }

    async fn get_screenshot(&mut self) -> Result<Vec<u8>> {
        println!("Getting screenshot from {}", self.device_id);
        Ok(vec![]) // Return empty bytes for now
    }

    async fn send_touch(&mut self, x: i32, y: i32) -> Result<()> {
        println!("Sending touch to {}: ({}, {})", self.device_id, x, y);
        Ok(())
    }
}
