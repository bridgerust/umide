# WGPU Video Rendering Implementation Plan

Goal: Implement high-performance video rendering using WGPU directly within Floem.

## User Review Required

> [!IMPORTANT]
> This requires accessing `floem`'s internal `wgpu` context. If `floem` abstracts this away too strictly, we might need to fork/patch or use `cpu` fallback.

## Proposed Changes

### [umide]

#### [NEW] [crates/umide/src/panel/video_view.rs](file:///Users/Apple/src/contributions/umide/crates/umide/src/panel/video_view.rs)

- Struct `VideoView`.
- Implements `floem::view::View` trait.
- **State**:
  - `wgpu::Texture` (created on first paint or resize).
  - `wgpu::BindGroup`.
  - `YUV` or `RGBA` buffer from decoder.
- **Paint**:
  - Access `PaintCx`.
  - Check for `gpu_resources` or `wgpu_device`.
  - Upload frame data to texture ( `queue.write_texture` ).
  - Issue draw command (or just draw a textured quad using `floem`'s renderer if it exposes textured rects).

#### [MODIFY] [crates/umide/src/panel/emulator_view.rs](file:///Users/Apple/src/contributions/umide/crates/umide/src/panel/emulator_view.rs)

- Use `video_view()` instead of `img()` or `canvas()`.
- Pass `Stream` of frames to `video_view`.

## Verification Plan

### Automated Build Check

- `cargo check -p umide` to ensure `wgpu` types are accessible.

### Manual Verification

- Run app.
- Verify video renders with low latency.
