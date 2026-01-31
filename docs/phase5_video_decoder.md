# H.264 Decoding with Hardware Acceleration (VideoToolbox)

Goal: Implement high-performance H.264 decoding on macOS using VideoToolbox, falling back to OpenH264 if hardware decoding is unavailable or fails.

## User Review Required

> [!IMPORTANT]
> The implementation prioritizes **VideoToolbox** (via `objc2-video-toolbox` or manual `core-foundation` bindings) for hardware decoding.
> **Fallback**: `openh264` crate will be used if hardware decoding fails initialization (e.g. constrained environment).

## Proposed Changes

### [umide_emulator]

#### [MODIFY] [Cargo.toml](file:///Users/Apple/src/contributions/umide/crates/umide_emulator/Cargo.toml)

- Remove `ffmpeg-sys-next` (due to build failures).
- Add `openh264 = "0.9.3"` for fallback.
- Add `objc2 = "0.5.2"`, `objc2-video-toolbox = "0.2.2"`, `objc2-core-media = "0.2.2"`, `objc2-core-foundation = "0.2.2"` (or appropriate versions) for VideoToolbox support.

#### [NEW] [decoder.rs](file:///Users/Apple/src/contributions/umide/crates/umide_emulator/src/decoder.rs)

- Define `Trait VideoDecoder` with `decode(&mut self, data: &[u8]) -> Result<Vec<u8>>`.
- Implement `VideoToolboxDecoder` struct.
- Implement `OpenH264Decoder` struct.

#### [MODIFY] [video.rs](file:///Users/Apple/src/contributions/umide/crates/umide_emulator/src/video.rs)

- Modify `VideoDecoder` to be an enum or wrapper that holds either `VideoToolboxDecoder` or `OpenH264Decoder`.
- `new()` attempts to create `VideoToolboxDecoder`. If it returns Err, it creates `OpenH264Decoder`.

#### [MODIFY] [android.rs](file:///Users/Apple/src/contributions/umide/crates/umide_emulator/src/android.rs), [ios.rs](file:///Users/Apple/src/contributions/umide/crates/umide_emulator/src/ios.rs)

- Update usages to the new decoder interface.

## Verification Plan

### Automated Tests

- `cargo test -p umide_emulator`
- New tests in `decoder.rs` ensuring `VideoToolboxDecoder::new()` acts as expected on macOS (might need to be skipped in CI if no screen attached, but fine locally).

### Manual Verification

- Build release `cargo build --release`.
- Verify logs show "Initialized VideoToolbox decoder" (when implemented).
