use crate::decoder::{VideoDecoder, DecodedFrame, DecodeError};

#[cfg(target_os = "macos")]
pub struct VideoToolboxDecoder {
    // Session, format descriptions, etc. would go here
}

#[cfg(target_os = "macos")]
impl VideoToolboxDecoder {
    pub fn new() -> Result<Self, DecodeError> {
        // Real implementation would verify hardware availability here
        // For now, return Err to test fallback to OpenH264
        // println!("Initializing VideoToolboxDecoder...");
        Err(DecodeError::HardwareUnavailable)
    }
}

#[cfg(target_os = "macos")]
impl VideoDecoder for VideoToolboxDecoder {
    fn decode_frame(&mut self, _data: &[u8]) -> Result<Vec<DecodedFrame>, DecodeError> {
        // Implement NALU parsing and feeding to VTDecompressionSession
        Err(DecodeError::DecodeFailed("Not implemented".to_string()))
    }

    fn flush(&mut self) -> Result<Vec<DecodedFrame>, DecodeError> {
        Ok(vec![])
    }

    fn reset(&mut self) -> Result<(), DecodeError> {
        Ok(())
    }
}

// Dummy impl for non-macOS to satisfy compiler if checked on other platforms
#[cfg(not(target_os = "macos"))]
pub struct VideoToolboxDecoder;

#[cfg(not(target_os = "macos"))]
impl VideoToolboxDecoder {
    pub fn new() -> Result<Self, DecodeError> {
        Err(DecodeError::HardwareUnavailable)
    }
}
