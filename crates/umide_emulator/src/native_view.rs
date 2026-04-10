use raw_window_handle::RawWindowHandle;
use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use umide_native::emulator::{
    umide_native_attach_device, umide_native_create_emulator,
    umide_native_destroy_emulator, umide_native_hide_emulator,
    umide_native_push_frame, umide_native_resize_emulator, umide_native_send_input,
    umide_native_set_input_callback, umide_native_show_emulator, EmulatorInputEvent,
    EmulatorInputType, EmulatorPlatform, NativeEmulator,
};

/// Commands to send to the gRPC command client
enum GrpcCommand {
    Key(String),
    KeyCode(i32),
    TouchDown(i32, i32),
    TouchMove(i32, i32),
    TouchUp(i32, i32),
    Scroll(i32, i32),
}

/// Context passed to the C++ callback
struct CallbackCtx {
    is_android: bool,
    handle: *mut NativeEmulator,
    grpc_cmd_tx: Option<std::sync::mpsc::Sender<GrpcCommand>>,
}

extern "C" fn input_callback(
    event_type: i32,
    x: i32,
    y: i32,
    user_data: *mut c_void,
) {
    if user_data.is_null() {
        return;
    }
    let ctx = unsafe { &*(user_data as *mut CallbackCtx) };

    if ctx.is_android {
        if let Some(tx) = &ctx.grpc_cmd_tx {
            let cmd = match event_type {
                0 => GrpcCommand::TouchDown(x, y),
                1 => GrpcCommand::TouchMove(x, y),
                2 => GrpcCommand::TouchUp(x, y),
                3 => GrpcCommand::Scroll(x, y), // y is deltaY
                _ => return,
            };
            let _ = tx.send(cmd);
        }
    } else {
        // iOS: send event directly back to C++ `send_input` handler where CGEventPost happens
        let ev_type = match event_type {
            0 => EmulatorInputType::TouchDown,
            1 => EmulatorInputType::TouchMove,
            2 => EmulatorInputType::TouchUp,
            3 => EmulatorInputType::Scroll,
            _ => return,
        };
        let ev = EmulatorInputEvent {
            event_type: ev_type,
            x,
            y,
            key_code: 0,
        };
        unsafe {
            umide_native_send_input(ctx.handle, &ev);
        }
    }
}

pub struct NativeEmulatorView {
    handle: *mut NativeEmulator,
    is_android: bool,
    /// Cancellation flag for the gRPC streaming task
    stream_cancel: Arc<AtomicBool>,
    /// Channel to send commands to the gRPC command client
    grpc_cmd_tx: Option<std::sync::mpsc::Sender<GrpcCommand>>,
    /// Pointer to the callback context (so we can drop it)
    callback_ctx: *mut CallbackCtx,
}

unsafe impl Send for NativeEmulatorView {}
unsafe impl Sync for NativeEmulatorView {}

impl NativeEmulatorView {
    pub fn new(
        window_handle: RawWindowHandle,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        platform: EmulatorPlatform,
    ) -> Result<Self, String> {
        let parent_ptr = match window_handle {
            #[cfg(target_os = "macos")]
            RawWindowHandle::AppKit(handle) => handle.ns_view.as_ptr(),
            _ => {
                return Err(
                    "Unsupported platform for native emulator embedding".to_string()
                )
            }
        };

        let handle = unsafe {
            umide_native_create_emulator(parent_ptr, x, y, width, height, platform)
        };

        if handle.is_null() {
            Err("Failed to create native emulator instance".to_string())
        } else {
            let is_android = matches!(platform, EmulatorPlatform::Android);
            let (_grpc_cmd_tx, _) = std::sync::mpsc::channel::<GrpcCommand>(); // placeholder, replaced in start_grpc_stream

            let ctx = Box::new(CallbackCtx {
                is_android,
                handle,
                grpc_cmd_tx: None,
            });
            let ctx_ptr = Box::into_raw(ctx);

            unsafe {
                umide_native_set_input_callback(
                    handle,
                    Some(input_callback),
                    ctx_ptr as *mut c_void,
                );
            }

            Ok(Self {
                handle,
                is_android,
                stream_cancel: Arc::new(AtomicBool::new(false)),
                grpc_cmd_tx: None,
                callback_ctx: ctx_ptr,
            })
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

        if !self.is_android {
            self.ensure_idb_installed();
        }
    }

    /// Silently checks and installs `idb` via Homebrew and pip if it is missing on the host.
    /// This is strictly required for iOS interactability (taps/swipes).
    fn ensure_idb_installed(&self) {
        std::thread::spawn(move || {
            // Check if idb exists
            let output = std::process::Command::new("which").arg("idb").output();

            let needs_install = match output {
                Ok(out) => !out.status.success() || out.stdout.is_empty(),
                Err(_) => true,
            };

            if needs_install {
                tracing::info!("'idb' not found. Auto-installing iOS interaction dependencies...");

                // 1. Install idb-companion via brew
                let _ = std::process::Command::new("brew")
                    .args(["tap", "facebook/fb"])
                    .output();
                let _ = std::process::Command::new("brew")
                    .args(["install", "idb-companion"])
                    .output();

                // 2. Install fb-idb via pip3
                let _ = std::process::Command::new("pip3")
                    .args(["install", "fb-idb"])
                    .output();

                tracing::info!("Finished installing iOS idb dependencies.");
            }
        });
    }

    /// Push RGBA frame data for display (used by gRPC streaming for Android)
    pub fn push_frame(&self, rgba_data: &[u8], width: u32, height: u32) {
        unsafe {
            umide_native_push_frame(self.handle, rgba_data.as_ptr(), width, height);
        }
    }

    pub fn show(&self) {
        unsafe {
            umide_native_show_emulator(self.handle);
        }
    }

    pub fn hide(&self) {
        unsafe {
            umide_native_hide_emulator(self.handle);
        }
    }

    /// Send a key event to the Android emulator via gRPC (e.g. "GoHome", "GoBack", "Power")
    pub fn send_key(&self, key: &str) {
        if let Some(tx) = &self.grpc_cmd_tx {
            let _ = tx.send(GrpcCommand::Key(key.to_string()));
        }
    }

    /// Send a key code to the Android emulator via gRPC (e.g. 115=Vol+, 114=Vol-)
    pub fn send_key_code(&self, code: i32) {
        if let Some(tx) = &self.grpc_cmd_tx {
            let _ = tx.send(GrpcCommand::KeyCode(code));
        }
    }

    /// Send a touch down event to the Android emulator via gRPC
    pub fn send_touch_down(&self, x: i32, y: i32) {
        if let Some(tx) = &self.grpc_cmd_tx {
            let _ = tx.send(GrpcCommand::TouchDown(x, y));
        }
    }

    /// Send a touch move event to the Android emulator via gRPC
    pub fn send_touch_move(&self, x: i32, y: i32) {
        if let Some(tx) = &self.grpc_cmd_tx {
            let _ = tx.send(GrpcCommand::TouchMove(x, y));
        }
    }

    /// Send a touch up event to the Android emulator via gRPC
    pub fn send_touch_up(&self, x: i32, y: i32) {
        if let Some(tx) = &self.grpc_cmd_tx {
            let _ = tx.send(GrpcCommand::TouchUp(x, y));
        }
    }

    /// Start gRPC frame streaming for Android emulator.
    /// Uses TWO separate gRPC clients: one for streaming frames, one for commands.
    /// This avoids the deadlock where stream_screenshots holds a mutex forever.
    pub fn start_grpc_stream(&mut self, grpc_endpoint: &str) {
        if !self.is_android {
            tracing::warn!("gRPC streaming is only for Android emulators");
            return;
        }

        // Cancel any existing stream
        self.stream_cancel.store(true, Ordering::SeqCst);
        let cancel = Arc::new(AtomicBool::new(false));
        self.stream_cancel = cancel.clone();

        let endpoint = grpc_endpoint.to_string();
        let handle_addr = self.handle as usize;
        let cancel_flag = cancel.clone();

        // Create a sync channel for commands (key presses, touch events)
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<GrpcCommand>();
        self.grpc_cmd_tx = Some(cmd_tx.clone());

        // Update the callback context so the C++ callback can send to this channel
        unsafe {
            if !self.callback_ctx.is_null() {
                let ctx = &mut *self.callback_ctx;
                ctx.grpc_cmd_tx = Some(cmd_tx);
            }
        }

        // Spawn the streaming task on a background thread with its own tokio runtime
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!(
                        "Failed to create tokio runtime for gRPC stream: {}",
                        e
                    );
                    return;
                }
            };

            rt.block_on(async move {
                use crate::grpc_client::EmulatorGrpcClient;
                use tokio::sync::mpsc;

                tracing::info!("Android gRPC stream: connecting to {}...", endpoint);

                // Connect STREAMING client with retry
                let mut stream_client = match EmulatorGrpcClient::connect_with_retry(
                    &endpoint,
                    std::time::Duration::from_secs(60),
                ).await {
                    Ok(c) => {
                        tracing::info!("Android gRPC stream: streaming client connected!");
                        c
                    }
                    Err(e) => {
                        tracing::error!("Android gRPC stream: failed to connect streaming client: {}", e);
                        return;
                    }
                };

                if cancel_flag.load(Ordering::SeqCst) {
                    tracing::info!("Android gRPC stream: cancelled before streaming started");
                    return;
                }

                // Connect COMMAND client (separate connection, no lock contention)
                let cmd_endpoint = endpoint.clone();
                let cmd_cancel = cancel_flag.clone();
                tokio::spawn(async move {
                    let mut cmd_client = match EmulatorGrpcClient::connect_with_retry(
                        &cmd_endpoint,
                        std::time::Duration::from_secs(10),
                    ).await {
                        Ok(c) => {
                            tracing::info!("Android gRPC: command client connected!");
                            c
                        }
                        Err(e) => {
                            tracing::error!("Android gRPC: failed to connect command client: {}", e);
                            return;
                        }
                    };

                    loop {
                        if cmd_cancel.load(Ordering::SeqCst) {
                            break;
                        }
                        match cmd_rx.try_recv() {
                            Ok(cmd) => {
                                match cmd {
                                    GrpcCommand::Key(key) => {
                                        if let Err(e) = cmd_client.send_key(&key).await {
                                            tracing::warn!("gRPC send_key error: {}", e);
                                        }
                                    }
                                    GrpcCommand::KeyCode(code) => {
                                        if let Err(e) = cmd_client.send_key_code(code).await {
                                            tracing::warn!("gRPC send_key_code error: {}", e);
                                        }
                                    }
                                    GrpcCommand::TouchDown(x, y) => {
                                        if let Err(e) = cmd_client.send_touch_down(x, y).await {
                                            tracing::warn!("gRPC touch_down error: {}", e);
                                        }
                                    }
                                    GrpcCommand::TouchMove(x, y) => {
                                        if let Err(e) = cmd_client.send_touch_down(x, y).await {
                                            tracing::warn!("gRPC touch_move error: {}", e);
                                        }
                                    }
                                    GrpcCommand::TouchUp(x, y) => {
                                        if let Err(e) = cmd_client.send_touch_up(x, y).await {
                                            tracing::warn!("gRPC touch_up error: {}", e);
                                        }
                                    }
                                    GrpcCommand::Scroll(x, delta_y) => {
                                        let start_y = 500;
                                        let end_y = start_y - delta_y * 5;
                                        let _ = cmd_client.send_touch_down(x, start_y).await;
                                        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                                        let _ = cmd_client.send_touch_move(x, end_y).await;
                                        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                                        let _ = cmd_client.send_touch_up(x, end_y).await;
                                    }
                                }
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {
                                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                            }
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                break;
                            }
                        }
                    }
                });

                // Start streaming frames (this runs until cancelled or error)
                let (tx, mut rx) = mpsc::channel(2);
                let stream_cancel = cancel_flag.clone();
                tokio::spawn(async move {
                    if let Err(e) = stream_client.stream_screenshots(tx).await {
                        if !stream_cancel.load(Ordering::SeqCst) {
                            tracing::error!("Android gRPC stream error: {}", e);
                        }
                    }
                });

                // Push frames to native view
                while let Some(frame) = rx.recv().await {
                    if cancel_flag.load(Ordering::SeqCst) {
                        tracing::info!("Android gRPC stream: cancelled");
                        break;
                    }

                    if let Some(rgba_data) = frame.to_rgba() {
                        let ptr = handle_addr as *mut NativeEmulator;
                        unsafe {
                            umide_native_push_frame(
                                ptr,
                                rgba_data.as_ptr(),
                                frame.width,
                                frame.height,
                            );
                        }
                    }
                }

                tracing::info!("Android gRPC stream: ended");
            });
        });
    }

    /// Stop the gRPC streaming task
    pub fn stop_stream(&self) {
        self.stream_cancel.store(true, Ordering::SeqCst);
    }

    /// Check if this is an Android emulator
    pub fn is_android(&self) -> bool {
        self.is_android
    }
}

impl Drop for NativeEmulatorView {
    fn drop(&mut self) {
        // Cancel any running stream first
        self.stream_cancel.store(true, Ordering::SeqCst);
        // Drop the command channel
        self.grpc_cmd_tx = None;
        // Give the stream thread a moment to notice cancellation
        std::thread::sleep(std::time::Duration::from_millis(50));
        unsafe {
            if !self.callback_ctx.is_null() {
                let _ = Box::from_raw(self.callback_ctx);
            }
            umide_native_destroy_emulator(self.handle);
        }
    }
}
