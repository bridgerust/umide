pub mod android;
pub mod common;
pub mod decoder;
pub mod grpc_client;
pub mod ios;
pub mod native_view;
pub mod video;

pub use android::AndroidEmulator;
pub use common::{DeviceInfo, DevicePlatform, DeviceState};
pub use decoder::create_decoder;
pub use grpc_client::{EmulatorGrpcClient, GrpcError};
pub use ios::IosSimulator;

/// List all available devices with their current state
pub fn list_all_devices() -> Vec<DeviceInfo> {
    let mut devices = Vec::new();

    if let Ok(mut android_devices) = AndroidEmulator::list_devices() {
        // Update states based on actual running status
        let running_serials = AndroidEmulator::get_running_serials();
        for device in &mut android_devices {
            if AndroidEmulator::is_running(&device.id)
                || running_serials
                    .iter()
                    .any(|s| s.contains(&device.id) || device.id.contains(s))
            {
                device.state = DeviceState::Running;
            }
        }
        devices.append(&mut android_devices);
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(mut ios_devices) = IosSimulator::list_devices() {
            // iOS devices already have correct state from list_devices
            // but let's double-check with booted list
            let booted = IosSimulator::get_booted_udids();
            for device in &mut ios_devices {
                if booted.contains(&device.id) {
                    device.state = DeviceState::Running;
                }
            }
            devices.append(&mut ios_devices);
        }
    }

    devices
}

/// Check if a specific device is currently running
pub fn is_device_running(device: &DeviceInfo) -> bool {
    match device.platform {
        DevicePlatform::Android => AndroidEmulator::is_running(&device.id),
        DevicePlatform::Ios => IosSimulator::is_running(&device.id),
    }
}

/// Refresh the state of a single device
pub fn refresh_device_state(device: &mut DeviceInfo) {
    device.state = if is_device_running(device) {
        DeviceState::Running
    } else {
        DeviceState::Disconnected
    };
}

pub fn launch_device(device: &DeviceInfo) -> anyhow::Result<()> {
    match device.platform {
        DevicePlatform::Android => AndroidEmulator::launch(&device.id),
        DevicePlatform::Ios => IosSimulator::launch(&device.id),
    }
}

pub fn stop_device(device: &DeviceInfo) -> anyhow::Result<()> {
    match device.platform {
        DevicePlatform::Android => AndroidEmulator::stop(&device.id),
        DevicePlatform::Ios => IosSimulator::stop(&device.id),
    }
}
