use std::sync::Arc;
use crate::decoder::{VideoDecoder, DecodedFrame, DecodeError};
use openh264::decoder::{Decoder, DecodedYUV};
use openh264::formats::YUVSource;

pub struct OpenH264Decoder {
    decoder: Decoder,
}

impl OpenH264Decoder {
    pub fn new() -> Self {
        let decoder = Decoder::new().expect("Failed to create OpenH264 decoder");
        Self { decoder }
    }
}

impl Default for OpenH264Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoDecoder for OpenH264Decoder {
    fn decode_frame(&mut self, data: &[u8]) -> Result<Vec<DecodedFrame>, DecodeError> {
        let mut frames = Vec::new();
        
        match self.decoder.decode(data) {
            Ok(Some(yuv_frame)) => {
                let (width, height) = yuv_frame.dimensions();
                let width = width as u32;
                let height = height as u32;
                
                // Convert YUV to RGBA
                let mut rgba_raw = vec![0u8; (width * height * 4) as usize];
                yuv_to_rgba(&yuv_frame, &mut rgba_raw, width, height);

                let rgba_data = Arc::new(rgba_raw);

                frames.push(DecodedFrame {
                    width,
                    height,
                    frame: crate::decoder::GpuFrame::Software(rgba_data, width * 4, height), // stride = width * 4
                });
            }
            Ok(None) => {},
            Err(e) => {
                return Err(DecodeError::DecodeFailed(format!("OpenH264 error: {:?}", e)));
            }
        }

        Ok(frames)
    }

    fn flush(&mut self) -> Result<Vec<DecodedFrame>, DecodeError> {
        Ok(vec![])
    }

    fn reset(&mut self) -> Result<(), DecodeError> {
        match Decoder::new() {
            Ok(d) => self.decoder = d,
            Err(e) => return Err(DecodeError::DecodeFailed(format!("Failed to reset decoder: {:?}", e))),
        }
        Ok(())
    }
}

fn yuv_to_rgba(yuv: &DecodedYUV<'_>, rgba: &mut [u8], width: u32, height: u32) {
    // Use YUVSource trait methods
    let strides = yuv.strides();
    let y_stride = strides.0;
    let u_stride = strides.1;
    let v_stride = strides.2;
    
    // YUVSource returns slices for the whole plane
    let y_plane = yuv.y();
    let u_plane = yuv.u();
    let v_plane = yuv.v();

    for y in 0..height {
        for x in 0..width {
            let y_idx = (y as usize * y_stride) + x as usize;
            let uv_idx = ((y as usize / 2) * u_stride) + (x as usize / 2);
            let v_idx = ((y as usize / 2) * v_stride) + (x as usize / 2);

            if y_idx >= y_plane.len() || uv_idx >= u_plane.len() || v_idx >= v_plane.len() { continue; }

            let y_val = y_plane[y_idx] as f32;
            let u_val = u_plane[uv_idx] as f32 - 128.0;
            let v_val = v_plane[v_idx] as f32 - 128.0;

            let r = (y_val + 1.402 * v_val) as i32;
            let g = (y_val - 0.344136 * u_val - 0.714136 * v_val) as i32;
            let b = (y_val + 1.772 * u_val) as i32;

            let dest_idx = ((y as usize * width as usize) + x as usize) * 4;
            rgba[dest_idx] = r.clamp(0, 255) as u8;
            rgba[dest_idx + 1] = g.clamp(0, 255) as u8;
            rgba[dest_idx + 2] = b.clamp(0, 255) as u8;
            rgba[dest_idx + 3] = 255;
        }
    }
}
