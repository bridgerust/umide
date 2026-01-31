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
        Command::new("emulator")
            .arg("-avd")
            .arg(avd_name)
            .arg("-no-window")  // Headless mode - no external window
            .arg("-grpc")
            .arg("5556")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()?;
        Ok(())
    }

    pub fn stop(avd_name: &str) -> Result<()> {
        // To stop an Android emulator without tracking the PID, we use `adb devices -l`
        // to find the emulator that matches the AVD name (label).
        let output = Command::new("adb")
            .arg("devices")
            .arg("-l")
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("Failed to list adb devices"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Look for something like "emulator-5554 device ... model:avd_name"
        for line in stdout.lines() {
            if line.contains(avd_name) {
                if let Some(serial) = line.split_whitespace().next() {
                    Command::new("adb")
                        .arg("-s")
                        .arg(serial)
                        .arg("emu")
                        .arg("kill")
                        .status()?;
                    return Ok(());
                }
            }
        }

        Err(anyhow!("Could not find running emulator for AVD: {}", avd_name))
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
