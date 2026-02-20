//! gRPC client for Android Emulator streaming
//!
//! Connects to the Android emulator's gRPC endpoint and streams
//! screenshots at full framerate for smooth embedded display.

use std::sync::Arc;
use tokio::sync::mpsc;
use tonic::transport::Channel;
use tracing::{info, error, warn};

// Include the generated proto code
pub mod emulator_proto {
    tonic::include_proto!("android.emulation.control");
}

use emulator_proto::{
    emulator_controller_client::EmulatorControllerClient,
    ImageFormat, Image, TouchEvent, Touch,
    image_format::ImgFormat,
};

use crate::decoder::{DecodedFrame, GpuFrame};

/// Error type for gRPC operations
#[derive(Debug, thiserror::Error)]
pub enum GrpcError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Stream error: {0}")]
    StreamError(String),
    #[error("Decode error: {0}")]
    DecodeError(String),
}

/// gRPC streaming client for Android emulator
pub struct EmulatorGrpcClient {
    client: EmulatorControllerClient<Channel>,
}

impl EmulatorGrpcClient {
    /// Connect to the Android emulator gRPC endpoint
    pub async fn connect(endpoint: &str) -> Result<Self, GrpcError> {
        info!("Connecting to Android emulator gRPC at {}", endpoint);
        
        let channel = Channel::from_shared(endpoint.to_string())
            .map_err(|e| GrpcError::ConnectionFailed(e.to_string()))?
            .connect()
            .await
            .map_err(|e| GrpcError::ConnectionFailed(e.to_string()))?;
        
        // Raw RGBA frames at device resolution can be large (1080x2400x4 = ~10MB)
        // Default tonic limit is 4MB — increase to 50MB
        let client = EmulatorControllerClient::new(channel)
            .max_decoding_message_size(50 * 1024 * 1024);
        
        info!("Connected to Android emulator gRPC");
        
        Ok(Self { client })
    }
    
    /// Connect to default emulator endpoint (localhost:8554)
    pub async fn connect_default() -> Result<Self, GrpcError> {
        Self::connect("http://localhost:8554").await
    }
    
    /// Connect with retry and exponential backoff.
    /// The emulator takes time to boot and expose gRPC, so we retry patiently.
    pub async fn connect_with_retry(endpoint: &str, max_duration: std::time::Duration) -> Result<Self, GrpcError> {
        let start = std::time::Instant::now();
        let mut delay = std::time::Duration::from_millis(200);
        let max_delay = std::time::Duration::from_secs(2);
        
        loop {
            match Self::connect(endpoint).await {
                Ok(client) => return Ok(client),
                Err(e) => {
                    if start.elapsed() >= max_duration {
                        return Err(GrpcError::ConnectionFailed(format!(
                            "Failed to connect after {:?}: {}", max_duration, e
                        )));
                    }
                    warn!("gRPC connection failed (retrying in {:?}): {}", delay, e);
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(max_delay);
                }
            }
        }
    }
    
    /// Get a single screenshot
    pub async fn get_screenshot(&mut self) -> Result<DecodedFrame, GrpcError> {
        let format = ImageFormat {
            format: ImgFormat::Rgba8888 as i32,
            rotation: None,
            width: 0,   // Native resolution
            height: 0,
            display: 0, // Main display
        };
        
        let response = self.client
            .get_screenshot(format)
            .await
            .map_err(|e| GrpcError::StreamError(e.to_string()))?;
        
        let image = response.into_inner();
        self.image_to_frame(image)
    }
    
    /// Start streaming screenshots
    /// Returns a channel receiver that yields frames
    pub async fn stream_screenshots(
        &mut self,
        tx: mpsc::Sender<DecodedFrame>,
    ) -> Result<(), GrpcError> {
        let format = ImageFormat {
            format: ImgFormat::Rgba8888 as i32,
            rotation: None,
            width: 0,   // Native resolution
            height: 0,
            display: 0,
        };
        
        info!("Starting screenshot stream...");
        
        let mut stream = self.client
            .stream_screenshot(format)
            .await
            .map_err(|e| GrpcError::StreamError(e.to_string()))?
            .into_inner();
        
        info!("Screenshot stream started");
        
        loop {
            match stream.message().await {
                Ok(Some(image)) => {
                    match self.image_to_frame(image) {
                        Ok(frame) => {
                            if tx.send(frame).await.is_err() {
                                info!("Frame receiver dropped, stopping stream");
                                break;
                            }
                        }
                        Err(e) => {
                            warn!("Failed to decode frame: {}", e);
                        }
                    }
                }
                Ok(None) => {
                    info!("Stream ended");
                    break;
                }
                Err(e) => {
                    error!("Stream error: {}", e);
                    return Err(GrpcError::StreamError(e.to_string()));
                }
            }
        }
        
        Ok(())
    }
    
    /// Send a touch event to the emulator
    /// pressure > 0 means touching, pressure = 0 means release
    pub async fn send_touch_down(&mut self, x: i32, y: i32) -> Result<(), GrpcError> {
        let touch = Touch {
            x,
            y,
            identifier: 0,
            pressure: 1,  // Non-zero = touching
            touch_major: 0,
            touch_minor: 0,
        };
        
        let event = TouchEvent {
            touches: vec![touch],
            display: 0,
        };
        
        self.client
            .send_touch(event)
            .await
            .map_err(|e| GrpcError::StreamError(e.to_string()))?;
        
        Ok(())
    }
    
    /// Release a touch at coordinates (pressure = 0)
    pub async fn send_touch_up(&mut self, x: i32, y: i32) -> Result<(), GrpcError> {
        let touch = Touch {
            x,
            y,
            identifier: 0,
            pressure: 0,  // 0 = release
            touch_major: 0,
            touch_minor: 0,
        };
        
        let event = TouchEvent {
            touches: vec![touch],
            display: 0,
        };
        
        self.client
            .send_touch(event)
            .await
            .map_err(|e| GrpcError::StreamError(e.to_string()))?;
        
        Ok(())
    }
    
    /// Send a tap (down + up) at coordinates
    pub async fn tap(&mut self, x: i32, y: i32) -> Result<(), GrpcError> {
        self.send_touch_down(x, y).await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        self.send_touch_up(x, y).await?;
        Ok(())
    }
    
    /// Send a key event using a key string (e.g. "GoHome", "GoBack", "Power", "AppSwitch")
    pub async fn send_key(&mut self, key: &str) -> Result<(), GrpcError> {
        use emulator_proto::{KeyboardEvent, keyboard_event::{KeyEventType, KeyCodeType}};
        
        let event = KeyboardEvent {
            code_type: KeyCodeType::Evdev as i32,
            event_type: KeyEventType::Keypress as i32,
            key_code: 0,
            key: key.to_string(),
            text: String::new(),
        };
        
        self.client
            .send_key(event)
            .await
            .map_err(|e| GrpcError::StreamError(e.to_string()))?;
        
        Ok(())
    }
    
    /// Send a key event using an Evdev keycode (e.g. 115=Vol+, 114=Vol-)
    pub async fn send_key_code(&mut self, code: i32) -> Result<(), GrpcError> {
        use emulator_proto::{KeyboardEvent, keyboard_event::{KeyEventType, KeyCodeType}};
        
        let event = KeyboardEvent {
            code_type: KeyCodeType::Evdev as i32,
            event_type: KeyEventType::Keypress as i32,
            key_code: code,
            key: String::new(),
            text: String::new(),
        };
        
        self.client
            .send_key(event)
            .await
            .map_err(|e| GrpcError::StreamError(e.to_string()))?;
        
        Ok(())
    }
    
    /// Convert gRPC Image to DecodedFrame
    fn image_to_frame(&self, image: Image) -> Result<DecodedFrame, GrpcError> {
        // AOSP: width/height can be in Image (deprecated) or Image.format
        // Try format first, fall back to deprecated fields
        let (width, height) = if let Some(ref fmt) = image.format {
            let w = if fmt.width > 0 { fmt.width } else { image.width };
            let h = if fmt.height > 0 { fmt.height } else { image.height };
            (w, h)
        } else {
            (image.width, image.height)
        };
        
        let data = image.image;
        
        if data.is_empty() {
            return Err(GrpcError::DecodeError("Empty image data".to_string()));
        }
        
        if width == 0 || height == 0 {
            return Err(GrpcError::DecodeError(format!(
                "Invalid dimensions: {}x{}", width, height
            )));
        }
        
        // Image data should be RGBA8888
        let expected_size = (width * height * 4) as usize;
        if data.len() != expected_size {
            return Err(GrpcError::DecodeError(format!(
                "Image size mismatch: expected {} ({}x{}x4), got {}",
                expected_size, width, height, data.len()
            )));
        }
        
        Ok(DecodedFrame {
            width,
            height,
            frame: GpuFrame::Software(Arc::new(data), width * 4, height),
        })
    }
}
