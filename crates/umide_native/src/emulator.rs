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

pub type EmulatorInputCallback = Option<extern "C" fn(event_type: i32, x: i32, y: i32, user_data: *mut c_void)>;

extern "C" {
    pub fn umide_native_create_emulator(
        parent_window: *mut c_void, 
        x: i32,
        y: i32,
        width: u32, 
        height: u32, 
        platform: EmulatorPlatform
    ) -> *mut NativeEmulator;

    pub fn umide_native_destroy_emulator(emulator: *mut NativeEmulator);

    pub fn umide_native_resize_emulator(emulator: *mut NativeEmulator, x: i32, y: i32, width: u32, height: u32);

    pub fn umide_native_send_input(emulator: *mut NativeEmulator, event: *const EmulatorInputEvent);

    pub fn umide_native_set_input_callback(emulator: *mut NativeEmulator, callback: EmulatorInputCallback, user_data: *mut c_void);

    pub fn umide_native_attach_device(emulator: *mut NativeEmulator, device_id: *const i8);

    pub fn umide_native_push_frame(emulator: *mut NativeEmulator, rgba_data: *const u8, width: u32, height: u32);
}
