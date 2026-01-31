pub mod android;
pub mod ios;
pub mod common;
pub mod video;
pub mod decoder;

pub use android::AndroidEmulator;
pub use ios::IosSimulator;
pub use decoder::create_decoder;
pub use common::{DeviceInfo, DevicePlatform, DeviceState};

pub fn list_all_devices() -> Vec<DeviceInfo> {
    let mut devices = Vec::new();
    
    if let Ok(mut android_devices) = AndroidEmulator::list_devices() {
        devices.append(&mut android_devices);
    }
    
    #[cfg(target_os = "macos")]
    {
        if let Ok(mut ios_devices) = IosSimulator::list_devices() {
            devices.append(&mut ios_devices);
        }
    }
    
    devices
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
