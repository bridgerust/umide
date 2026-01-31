#ifndef NATIVE_SURFACE_H
#define NATIVE_SURFACE_H

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque handle to a native GPU surface
typedef struct NativeSurface NativeSurface;

// Surface format enum
typedef enum {
    SURFACE_FORMAT_RGBA8 = 0,
    SURFACE_FORMAT_BGRA8 = 1,
} SurfaceFormat;

// Create a new GPU-shareable surface
// Returns NULL on failure
NativeSurface* native_surface_create(uint32_t width, uint32_t height, SurfaceFormat format);

// Resize the surface (recreates underlying buffers)
bool native_surface_resize(NativeSurface* surface, uint32_t width, uint32_t height);

// Get the IOSurfaceRef for sharing (macOS)
// Caller does NOT own the returned reference
void* native_surface_get_iosurface(NativeSurface* surface);

// Get the Metal texture (MTLTexture*) for rendering
// Caller does NOT own the returned reference
void* native_surface_get_metal_texture(NativeSurface* surface);

// Lock surface for writing (call before emulator renders)
bool native_surface_lock(NativeSurface* surface);

// Unlock surface after writing
void native_surface_unlock(NativeSurface* surface);

// Get raw pixel buffer pointer (only valid between lock/unlock)
void* native_surface_get_buffer(NativeSurface* surface);

// Get buffer stride in bytes
uint32_t native_surface_get_stride(NativeSurface* surface);

// Destroy the surface
void native_surface_destroy(NativeSurface* surface);

// ============ Screen Capture (iOS Simulator) ============

// Opaque handle to screen capture session
typedef struct ScreenCapture ScreenCapture;

// Callback for new frames
typedef void (*FrameCallback)(void* context, NativeSurface* surface);

// Start capturing a window by its window ID
// Returns NULL on failure
ScreenCapture* screen_capture_start(uint32_t window_id, FrameCallback callback, void* context);

// Stop capturing
void screen_capture_stop(ScreenCapture* capture);

#ifdef __cplusplus
}
#endif

#endif // NATIVE_SURFACE_H
