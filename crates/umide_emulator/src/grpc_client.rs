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
    touch::TouchType,
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
        
        let client = EmulatorControllerClient::new(channel);
        
        info!("Connected to Android emulator gRPC");
        
        Ok(Self { client })
    }
    
    /// Connect to default emulator endpoint (localhost:5556)
    pub async fn connect_default() -> Result<Self, GrpcError> {
        Self::connect("http://localhost:5556").await
    }
    
    /// Get a single screenshot
    pub async fn get_screenshot(&mut self) -> Result<DecodedFrame, GrpcError> {
        let format = ImageFormat {
            format: ImgFormat::Rgba8888 as i32,
            width: 0,  // Native resolution
            height: 0,
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
            width: 0,
            height: 0,
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
    pub async fn send_touch(&mut self, x: i32, y: i32, touch_type: TouchType) -> Result<(), GrpcError> {
        let touch = Touch {
            x,
            y,
            identifier: 0,
            pressure: 1.0,
            r#type: touch_type as i32,
        };
        
        let event = TouchEvent {
            touches: vec![touch],
        };
        
        self.client
            .send_touch(event)
            .await
            .map_err(|e| GrpcError::StreamError(e.to_string()))?;
        
        Ok(())
    }
    
    /// Send a tap (down + up) at coordinates
    pub async fn tap(&mut self, x: i32, y: i32) -> Result<(), GrpcError> {
        self.send_touch(x, y, TouchType::Down).await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        self.send_touch(x, y, TouchType::Up).await?;
        Ok(())
    }
    
    /// Convert gRPC Image to DecodedFrame
    fn image_to_frame(&self, image: Image) -> Result<DecodedFrame, GrpcError> {
        let width = image.width;
        let height = image.height;
        let data = image.image;
        
        if data.is_empty() {
            return Err(GrpcError::DecodeError("Empty image data".to_string()));
        }
        
        // Image data should be RGBA8888
        let expected_size = (width * height * 4) as usize;
        if data.len() != expected_size {
            return Err(GrpcError::DecodeError(format!(
                "Image size mismatch: expected {} bytes, got {}",
                expected_size,
                data.len()
            )));
        }
        
        Ok(DecodedFrame {
            width,
            height,
            frame: GpuFrame::Software(Arc::new(data), width * 4, height),
        })
    }
}
