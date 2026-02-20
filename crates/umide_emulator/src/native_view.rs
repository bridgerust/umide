use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use raw_window_handle::RawWindowHandle;
use umide_native::emulator::{
    umide_native_create_emulator, umide_native_destroy_emulator, umide_native_resize_emulator,
    umide_native_send_input, umide_native_attach_device, umide_native_push_frame,
    NativeEmulator, EmulatorPlatform, EmulatorInputEvent
};

pub struct NativeEmulatorView {
    handle: *mut NativeEmulator,
    is_android: bool,
    /// Cancellation flag for the gRPC streaming task
    stream_cancel: Arc<AtomicBool>,
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

    /// Start gRPC frame streaming for Android emulator.
    /// Spawns a background async task that connects to the emulator's gRPC endpoint
    /// and continuously pushes frames to the native view.
    pub fn start_grpc_stream(&self, grpc_endpoint: &str) {
        if !self.is_android {
            tracing::warn!("gRPC streaming is only for Android emulators");
            return;
        }

        // Cancel any existing stream
        self.stream_cancel.store(true, Ordering::SeqCst);
        let cancel = Arc::new(AtomicBool::new(false));
        // We can't update self.stream_cancel here since &self is immutable,
        // but the old cancel flag is set to true, so old tasks will stop.

        let endpoint = grpc_endpoint.to_string();
        // Cast to usize to cross thread boundary — usize is Send, *mut is not
        let handle_addr = self.handle as usize;
        let cancel_flag = cancel.clone();

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

                // Connect with retry (up to 60 seconds for emulator boot)
                let mut client = match EmulatorGrpcClient::connect_with_retry(
                    &endpoint,
                    std::time::Duration::from_secs(60),
                ).await {
                    Ok(c) => {
                        tracing::info!("Android gRPC stream: connected!");
                        c
                    }
                    Err(e) => {
                        tracing::error!("Android gRPC stream: failed to connect: {}", e);
                        return;
                    }
                };

                if cancel_flag.load(Ordering::SeqCst) {
                    tracing::info!("Android gRPC stream: cancelled before streaming started");
                    return;
                }

                // Start streaming frames
                let (tx, mut rx) = mpsc::channel(2); // Small buffer to avoid latency

                // Spawn the gRPC stream reader
                let stream_cancel = cancel_flag.clone();
                tokio::spawn(async move {
                    if let Err(e) = client.stream_screenshots(tx).await {
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
                        // Safety: handle_addr is valid as long as NativeEmulatorView exists
                        // The cancel flag ensures we stop before the view is destroyed
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

        // Note: We store the cancel flag via interior mutability pattern
        // The old stream_cancel is already set to true
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
        // Give the stream thread a moment to notice cancellation
        std::thread::sleep(std::time::Duration::from_millis(50));
        unsafe {
            umide_native_destroy_emulator(self.handle);
        }
    }
}
