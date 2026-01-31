# H.264 Hardware Decoding Implementation Plan (VideoToolbox + Fallback)

Goal: Implement high-performance H.264 decoding on macOS using VideoToolbox, falling back to OpenH264 if unavailable.

## User Review Required

> [!IMPORTANT]
>
> - **VideoToolbox** is the primary decoder on macOS.
> - **OpenH264** is the fallback.
> - **Dependencies**: `video-toolbox-sys`, `core-media-sys`, `core-video-sys` (VideoToolbox bindings), `h264-reader` (NALU parsing), `openh264`.

## Proposed Changes

### [umide_emulator]

#### [MODIFY] [Cargo.toml](file:///Users/Apple/src/contributions/umide/crates/umide_emulator/Cargo.toml)

- Remove `ffmpeg-sys-next`.
- Add `openh264 = "0.9.3"`.
- Add bindings: `video-toolbox-sys = "0.1"`, `core-media-sys = "0.1"`, `core-video-sys = "0.1"`.
- Add `h264-reader = "0.7"` for parsing NAL units.
- Add `thiserror` for error handling.

#### [NEW] [src/decoder.rs](file:///Users/Apple/src/contributions/umide/crates/umide_emulator/src/decoder.rs)

- **Trait Definition**:

  ```rust
  pub trait VideoDecoder: Send {
      fn decode_frame(&mut self, data: &[u8]) -> Result<Vec<DecodedFrame>, DecodeError>;
      fn flush(&mut self) -> Result<Vec<DecodedFrame>, DecodeError>;
      fn reset(&mut self) -> Result<(), DecodeError>;
  }

  pub struct DecodedFrame {
      pub width: u32,
      pub height: u32,
      pub data: Vec<u8>, // BGRA or RGBA
      pub stride: u32,
  }

  #[derive(Debug, thiserror::Error)]
  pub enum DecodeError {
      #[error("Hardware decoder unavailable")]
      HardwareUnavailable,
      #[error("Decode failed: {0}")]
      DecodeFailed(String),
      #[error("Input error: {0}")]
      InputError(String),
  }
  ```

- **Factory**:
  ```rust
  pub fn create_decoder() -> Box<dyn VideoDecoder> {
      #[cfg(target_os = "macos")]
      if let Ok(decoder) = crate::video::macos::VideoToolboxDecoder::new() {
           return Box::new(decoder);
      }
      Box::new(crate::video::openh264::OpenH264Decoder::new())
  }
  ```

#### [NEW] [src/video/mod.rs](file:///Users/Apple/src/contributions/umide/crates/umide_emulator/src/video/mod.rs)

- Module declarations.

#### [NEW] [src/video/macos.rs](file:///Users/Apple/src/contributions/umide/crates/umide_emulator/src/video/macos.rs)

- `VideoToolboxDecoder` implementation.
- Uses `h264-reader` to extract NALUs.
- Feeds NALUs to `VTDecompressionSessionDecodeFrame`.
- Converts `CVPixelBuffer` to RGBA `DecodedFrame`.

#### [NEW] [src/video/openh264.rs](file:///Users/Apple/src/contributions/umide/crates/umide_emulator/src/video/openh264.rs)

- `OpenH264Decoder` implementation.
- Uses `openh264::decoder::Decoder`.
- Manually converts YUV420p to RGBA (simd optimized if possible, but basic for now).

#### [MODIFY] [src/video.rs](file:///Users/Apple/src/contributions/umide/crates/umide_emulator/src/video.rs)

- Deprecate/Remove old file content, re-export new module logic or integrate into this.

## Verification Plan

### Automated Tests

- `cargo test -p umide_emulator`
- Test instantiation of `create_decoder()`.
- If possible, verify fallback logic by mocking failure of `VideoToolboxDecoder::new()`.

### Manual Verification

- Build release `cargo build --release`.
- Verify logs when initializing decoder.
