use std::ffi::{CString};
use raw_window_handle::{RawWindowHandle};
use umide_native::emulator::{
    umide_native_create_emulator, umide_native_destroy_emulator, umide_native_resize_emulator,
    umide_native_send_input, umide_native_attach_device,
    NativeEmulator, EmulatorPlatform, EmulatorInputEvent
};

pub struct NativeEmulatorView {
    handle: *mut NativeEmulator,
}

unsafe impl Send for NativeEmulatorView {}
unsafe impl Sync for NativeEmulatorView {}

impl NativeEmulatorView {
    pub fn new(window_handle: RawWindowHandle, x: i32, y: i32, width: u32, height: u32, platform: EmulatorPlatform) -> Result<Self, String> {
        let parent_ptr = match window_handle {
            #[cfg(target_os = "macos")]
            RawWindowHandle::AppKit(handle) => handle.ns_view.as_ptr(),
            _ => return Err("Unsupported platform for native emulator embedding".to_string()),
        };

        let handle = unsafe {
            umide_native_create_emulator(parent_ptr, x, y, width, height, platform)
        };

        if handle.is_null() {
            Err("Failed to create native emulator instance".to_string())
        } else {
            Ok(Self { handle })
        }
    }

    pub fn resize(&self, x: i32, y: i32, width: u32, height: u32) {
        unsafe {
            umide_native_resize_emulator(self.handle, x, y, width, height);
        }
    }

    pub fn attach_device(&self, device_id: &str) {
        let c_str = CString::new(device_id).unwrap_or_default();
        unsafe {
            umide_native_attach_device(self.handle, c_str.as_ptr());
        }
    }

    pub fn send_input(&self, event: EmulatorInputEvent) {
        unsafe {
            umide_native_send_input(self.handle, &event);
        }
    }
}

impl Drop for NativeEmulatorView {
    fn drop(&mut self) {
        unsafe {
            umide_native_destroy_emulator(self.handle);
        }
    }
}
