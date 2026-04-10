use std::sync::Arc;
use thiserror::Error;

pub trait VideoDecoder: Send {
    fn decode_frame(
        &mut self,
        data: &[u8],
    ) -> Result<Vec<DecodedFrame>, DecodeError>;
    fn flush(&mut self) -> Result<Vec<DecodedFrame>, DecodeError>;
    fn reset(&mut self) -> Result<(), DecodeError>;
}

#[derive(Clone)]
pub enum GpuFrame {
    /// Zero-copy hardware surface (e.g. CVPixelBuffer)
    /// We use a usize handle here to avoid platform-specific types leaking too much into the API
    /// This handle must be valid for the lifetime of the frame usage
    Hardware(HardwareSurfaceHandle),
    /// Software fallback (compatible with standard Floem Image)
    Software(Arc<Vec<u8>>, u32, u32),
}

#[derive(Clone)]
pub struct HardwareSurfaceHandle {
    pub ptr: usize,
    pub width: u32,
    pub height: u32,
}

impl HardwareSurfaceHandle {
    /// Read pixel data from the hardware surface (CVPixelBuffer on macOS).
    /// Returns RGBA data suitable for uploading to wgpu.
    ///
    /// # Safety
    /// The handle must be a valid CVPixelBufferRef.
    #[cfg(target_os = "macos")]
    pub fn read_pixels(&self) -> Option<Vec<u8>> {
        use core_video_sys::{
            CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
            CVPixelBufferGetPixelFormatType, CVPixelBufferLockBaseAddress,
            CVPixelBufferUnlockBaseAddress,
        };

        let pixel_buffer = self.ptr as core_video_sys::CVPixelBufferRef;
        if pixel_buffer.is_null() {
            return None;
        }

        unsafe {
            // Lock the buffer for reading
            let lock_flags = 1u64; // kCVPixelBufferLock_ReadOnly
            let status = CVPixelBufferLockBaseAddress(pixel_buffer, lock_flags);
            if status != 0 {
                return None;
            }

            let base_address = CVPixelBufferGetBaseAddress(pixel_buffer);
            let bytes_per_row = CVPixelBufferGetBytesPerRow(pixel_buffer);
            let pixel_format = CVPixelBufferGetPixelFormatType(pixel_buffer);

            // VideoToolbox typically outputs kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange (NV12)
            // or kCVPixelFormatType_32BGRA
            let rgba_data = if pixel_format == 0x42475241 {
                // 'BGRA' = kCVPixelFormatType_32BGRA
                // BGRA format - convert to RGBA
                let mut rgba =
                    Vec::with_capacity((self.width * self.height * 4) as usize);
                for y in 0..self.height {
                    let row_ptr = (base_address as *const u8)
                        .add((y as usize) * bytes_per_row);
                    for x in 0..self.width {
                        let pixel = row_ptr.add((x * 4) as usize);
                        rgba.push(*pixel.add(2)); // R (from B)
                        rgba.push(*pixel.add(1)); // G
                        rgba.push(*pixel.add(0)); // B (from R)
                        rgba.push(*pixel.add(3)); // A
                    }
                }
                Some(rgba)
            } else {
                // Unsupported format for now
                // TODO: Handle NV12/YUV conversion
                tracing::warn!("Unsupported pixel format: 0x{:08X}", pixel_format);
                None
            };

            CVPixelBufferUnlockBaseAddress(pixel_buffer, lock_flags);
            rgba_data
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn read_pixels(&self) -> Option<Vec<u8>> {
        None
    }
}

#[derive(Clone)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub frame: GpuFrame,
}

impl DecodedFrame {
    /// Convert this frame to PNG bytes for use with Floem's img() view.
    pub fn to_png(&self) -> Option<Vec<u8>> {
        let rgba_data = match &self.frame {
            GpuFrame::Software(data, _, _) => data.as_ref().clone(),
            GpuFrame::Hardware(handle) => handle.read_pixels()?,
        };

        // Encode as PNG using the image crate
        use image::RgbaImage;
        use std::io::Cursor;

        let img = RgbaImage::from_raw(self.width, self.height, rgba_data)?;
        let mut png_bytes = Vec::new();
        let mut cursor = Cursor::new(&mut png_bytes);
        img.write_to(&mut cursor, image::ImageFormat::Png).ok()?;
        Some(png_bytes)
    }

    /// Get RGBA pixel data for direct upload to GPU textures.
    pub fn to_rgba(&self) -> Option<Vec<u8>> {
        match &self.frame {
            GpuFrame::Software(data, _, _) => Some(data.as_ref().clone()),
            GpuFrame::Hardware(handle) => handle.read_pixels(),
        }
    }
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("Hardware decoder unavailable")]
    HardwareUnavailable,
    #[error("Decode failed: {0}")]
    DecodeFailed(String),
    #[error("Input error: {0}")]
    InputError(String),
}

pub fn create_decoder() -> Box<dyn VideoDecoder> {
    #[cfg(target_os = "macos")]
    {
        // Try hardware decoder first
        if let Ok(decoder) = crate::video::macos_hardware::VideoToolboxDecoder::new()
        {
            println!("Using VideoToolbox Hardware Decoder");
            return Box::new(decoder);
        } else {
            println!("VideoToolbox unavailable, falling back to OpenH264");
        }
    }

    // Fallback to software decoder
    println!("Using OpenH264 Software Decoder");
    Box::new(crate::video::openh264::OpenH264Decoder::new())
}
