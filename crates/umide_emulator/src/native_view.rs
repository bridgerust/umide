use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use raw_window_handle::RawWindowHandle;
use umide_native::emulator::{
    umide_native_create_emulator, umide_native_destroy_emulator, umide_native_resize_emulator,
    umide_native_send_input, umide_native_attach_device, umide_native_push_frame,
    NativeEmulator, EmulatorPlatform, EmulatorInputEvent
};

/// Commands to send to the gRPC command client
enum GrpcCommand {
    Key(String),
    KeyCode(i32),
    TouchDown(i32, i32),
    TouchMove(i32, i32),
    TouchUp(i32, i32),
}

pub struct NativeEmulatorView {
    handle: *mut NativeEmulator,
    is_android: bool,
    /// Cancellation flag for the gRPC streaming task
    stream_cancel: Arc<AtomicBool>,
    /// Channel to send commands to the gRPC command client
    grpc_cmd_tx: Option<std::sync::mpsc::Sender<GrpcCommand>>,
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
            Ok(Self {
                handle,
                is_android: matches!(platform, EmulatorPlatform::Android),
                stream_cancel: Arc::new(AtomicBool::new(false)),
                grpc_cmd_tx: None,
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
    }

    pub fn send_input(&self, event: EmulatorInputEvent) {
        unsafe {
            umide_native_send_input(self.handle, &event);
        }
    }

    /// Push RGBA frame data for display (used by gRPC streaming for Android)
    pub fn push_frame(&self, rgba_data: &[u8], width: u32, height: u32) {
        unsafe {
            umide_native_push_frame(self.handle, rgba_data.as_ptr(), width, height);
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
        self.grpc_cmd_tx = Some(cmd_tx);

        // Spawn the streaming task on a background thread with its own tokio runtime
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("Failed to create tokio runtime for gRPC stream: {}", e);
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
            umide_native_destroy_emulator(self.handle);
        }
    }
}
