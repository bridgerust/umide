use std::ffi::c_void;

#[repr(C)]
pub struct NativeEmulator {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum EmulatorPlatform {
    Android = 0,
    Ios = 1,
}

#[repr(C)]
pub enum EmulatorInputType {
    TouchDown = 0,
    TouchMove = 1,
    TouchUp = 2,
    KeyDown = 3,
    KeyUp = 4,
    Scroll = 5,
}

#[repr(C)]
pub struct EmulatorInputEvent {
    pub event_type: EmulatorInputType,
    pub x: i32,
    pub y: i32,
    pub key_code: i32,
}

pub type EmulatorInputCallback =
    Option<extern "C" fn(event_type: i32, x: i32, y: i32, user_data: *mut c_void)>;

// On macOS these symbols are implemented in native (AppKit/Metal) code linked
// via the build script. The embedded-emulator surface only exists on macOS.
#[cfg(target_os = "macos")]
extern "C" {
    pub fn umide_native_create_emulator(
        parent_window: *mut c_void,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        platform: EmulatorPlatform,
    ) -> *mut NativeEmulator;

    pub fn umide_native_destroy_emulator(emulator: *mut NativeEmulator);

    pub fn umide_native_resize_emulator(
        emulator: *mut NativeEmulator,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    );

    pub fn umide_native_send_input(
        emulator: *mut NativeEmulator,
        event: *const EmulatorInputEvent,
    );

    pub fn umide_native_set_input_callback(
        emulator: *mut NativeEmulator,
        callback: EmulatorInputCallback,
        user_data: *mut c_void,
    );

    pub fn umide_native_attach_device(
        emulator: *mut NativeEmulator,
        device_id: *const i8,
    );

    pub fn umide_native_push_frame(
        emulator: *mut NativeEmulator,
        rgba_data: *const u8,
        width: u32,
        height: u32,
    );

    pub fn umide_native_show_emulator(emulator: *mut NativeEmulator);

    pub fn umide_native_hide_emulator(emulator: *mut NativeEmulator);
}

// Non-macOS platforms have no native emulator surface yet (Windows/Linux
// embedding is tracked in docs/RELEASE-AND-PARITY.md). These no-op stubs keep
// the same C ABI so the editor + AI assistant build and link; at runtime
// `NativeEmulatorView::new` returns an error for non-AppKit windows, so the
// emulator panel reports itself unavailable rather than calling these.
#[cfg(not(target_os = "macos"))]
mod stubs {
    use super::*;

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn umide_native_create_emulator(
        _parent_window: *mut c_void,
        _x: i32,
        _y: i32,
        _width: u32,
        _height: u32,
        _platform: EmulatorPlatform,
    ) -> *mut NativeEmulator {
        std::ptr::null_mut()
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn umide_native_destroy_emulator(
        _emulator: *mut NativeEmulator,
    ) {
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn umide_native_resize_emulator(
        _emulator: *mut NativeEmulator,
        _x: i32,
        _y: i32,
        _width: u32,
        _height: u32,
    ) {
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn umide_native_send_input(
        _emulator: *mut NativeEmulator,
        _event: *const EmulatorInputEvent,
    ) {
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn umide_native_set_input_callback(
        _emulator: *mut NativeEmulator,
        _callback: EmulatorInputCallback,
        _user_data: *mut c_void,
    ) {
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn umide_native_attach_device(
        _emulator: *mut NativeEmulator,
        _device_id: *const i8,
    ) {
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn umide_native_push_frame(
        _emulator: *mut NativeEmulator,
        _rgba_data: *const u8,
        _width: u32,
        _height: u32,
    ) {
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn umide_native_show_emulator(
        _emulator: *mut NativeEmulator,
    ) {
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn umide_native_hide_emulator(
        _emulator: *mut NativeEmulator,
    ) {
    }
}

// Re-export the stub symbols at `emulator::umide_native_*` so callers (e.g.
// `umide_emulator::native_view`) resolve the same paths on every platform.
#[cfg(not(target_os = "macos"))]
pub use stubs::*;
