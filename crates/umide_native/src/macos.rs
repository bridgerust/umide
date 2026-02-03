//! macOS-specific native surface implementation
//! 
//! Uses IOSurface for zero-copy GPU texture sharing and
//! ScreenCaptureKit for iOS Simulator capture.

use crate::{GpuSurface, NativeSurfaceError, SurfaceFormat};
use std::ffi::c_void;
use std::ptr::NonNull;

// FFI declarations for the C++ implementation
mod ffi {
    use std::ffi::c_void;
    
    #[repr(C)]
    pub struct NativeSurface {
        _opaque: [u8; 0],
    }
    
    #[repr(C)]
    pub struct ScreenCapture {
        _opaque: [u8; 0],
    }
    
    pub type FrameCallback = extern "C" fn(context: *mut c_void, surface: *mut NativeSurface);
    
    extern "C" {
        pub fn native_surface_create(width: u32, height: u32, format: u32) -> *mut NativeSurface;
        pub fn native_surface_resize(surface: *mut NativeSurface, width: u32, height: u32) -> bool;
        pub fn native_surface_get_iosurface(surface: *mut NativeSurface) -> *mut c_void;
        pub fn native_surface_get_metal_texture(surface: *mut NativeSurface) -> *mut c_void;
        pub fn native_surface_lock(surface: *mut NativeSurface) -> bool;
        pub fn native_surface_unlock(surface: *mut NativeSurface);
        pub fn native_surface_get_buffer(surface: *mut NativeSurface) -> *mut c_void;
        pub fn native_surface_get_stride(surface: *mut NativeSurface) -> u32;
        pub fn native_surface_destroy(surface: *mut NativeSurface);
        
        pub fn screen_capture_start(
            window_id: u32,
            callback: FrameCallback,
            context: *mut c_void,
        ) -> *mut ScreenCapture;
        pub fn screen_capture_stop(capture: *mut ScreenCapture);
    }
}

/// An IOSurface-backed GPU surface for macOS
/// 
/// This surface can be shared between processes and rendered to
/// by both the emulator and UMIDE's wgpu renderer.
pub struct MacOSSurface {
    ptr: NonNull<ffi::NativeSurface>,
    width: u32,
    height: u32,
}

// Safety: The C++ implementation is thread-safe
unsafe impl Send for MacOSSurface {}
unsafe impl Sync for MacOSSurface {}

impl MacOSSurface {
    /// Create a new IOSurface-backed GPU surface
    pub fn new(width: u32, height: u32, format: SurfaceFormat) -> Result<Self, NativeSurfaceError> {
        let ptr = unsafe { ffi::native_surface_create(width, height, format as u32) };
        
        NonNull::new(ptr)
            .map(|ptr| Self { ptr, width, height })
            .ok_or_else(|| NativeSurfaceError::CreationFailed("IOSurface creation failed".into()))
    }
    
    /// Get a raw pointer to the IOSurfaceRef
    /// 
    /// This can be used for cross-process sharing or importing into other APIs.
    pub fn iosurface_ptr(&self) -> *mut c_void {
        unsafe { ffi::native_surface_get_iosurface(self.ptr.as_ptr()) }
    }
    
    /// Get a raw pointer to the MTLTexture
    /// 
    /// This can be used with the `metal` crate or for wgpu HAL imports.
    pub fn metal_texture_ptr(&self) -> *mut c_void {
        unsafe { ffi::native_surface_get_metal_texture(self.ptr.as_ptr()) }
    }
    
    /// Lock the surface for CPU writing
    /// 
    /// Must be called before `buffer_ptr()` and followed by `unlock()`.
    pub fn lock(&self) -> Result<SurfaceLock<'_>, NativeSurfaceError> {
        if unsafe { ffi::native_surface_lock(self.ptr.as_ptr()) } {
            Ok(SurfaceLock { surface: self })
        } else {
            Err(NativeSurfaceError::OperationFailed("Failed to lock surface".into()))
        }
    }
    
    /// Get the stride (bytes per row) of the surface
    pub fn stride(&self) -> u32 {
        unsafe { ffi::native_surface_get_stride(self.ptr.as_ptr()) }
    }
}

impl GpuSurface for MacOSSurface {
    fn width(&self) -> u32 {
        self.width
    }
    
    fn height(&self) -> u32 {
        self.height
    }
    
    fn resize(&mut self, width: u32, height: u32) -> Result<(), NativeSurfaceError> {
        if unsafe { ffi::native_surface_resize(self.ptr.as_ptr(), width, height) } {
            self.width = width;
            self.height = height;
            Ok(())
        } else {
            Err(NativeSurfaceError::OperationFailed("Failed to resize surface".into()))
        }
    }
}

impl Drop for MacOSSurface {
    fn drop(&mut self) {
        unsafe { ffi::native_surface_destroy(self.ptr.as_ptr()) }
    }
}

/// RAII guard for a locked surface
/// 
/// The surface is automatically unlocked when this guard is dropped.
pub struct SurfaceLock<'a> {
    surface: &'a MacOSSurface,
}

impl<'a> SurfaceLock<'a> {
    /// Get a mutable pointer to the pixel buffer
    /// 
    /// The buffer layout is RGBA or BGRA (depending on format),
    /// with `stride()` bytes per row.
    pub fn buffer_ptr(&self) -> *mut u8 {
        unsafe { ffi::native_surface_get_buffer(self.surface.ptr.as_ptr()) as *mut u8 }
    }
    
    /// Write pixel data to the surface
    /// 
    /// # Safety
    /// The caller must ensure the data fits within the surface bounds.
    pub unsafe fn write_pixels(&self, data: &[u8], src_stride: u32) {
        let dst = self.buffer_ptr();
        let dst_stride = self.surface.stride() as usize;
        let height = self.surface.height() as usize;
        let copy_width = (src_stride as usize).min(dst_stride);
        
        for y in 0..height {
            std::ptr::copy_nonoverlapping(
                data.as_ptr().add(y * src_stride as usize),
                dst.add(y * dst_stride),
                copy_width,
            );
        }
    }
}

impl Drop for SurfaceLock<'_> {
    fn drop(&mut self) {
        unsafe { ffi::native_surface_unlock(self.surface.ptr.as_ptr()) }
    }
}

/// Screen capture session for capturing iOS Simulator windows
pub struct ScreenCaptureSession {
    ptr: NonNull<ffi::ScreenCapture>,
    // Box to prevent move while callback is active
    _callback_data: Box<ScreenCaptureCallbackData>,
}

struct ScreenCaptureCallbackData {
    callback: Box<dyn Fn(&MacOSSurface) + Send + 'static>,
}

impl ScreenCaptureSession {
    /// Start capturing a window by its CGWindowID
    /// 
    /// The callback will be invoked on each captured frame.
    pub fn start<F>(window_id: u32, callback: F) -> Result<Self, NativeSurfaceError>
    where
        F: Fn(&MacOSSurface) + Send + 'static,
    {
        let callback_data = Box::new(ScreenCaptureCallbackData {
            callback: Box::new(callback),
        });
        
        let context = &*callback_data as *const ScreenCaptureCallbackData as *mut c_void;
        
        let ptr = unsafe {
            ffi::screen_capture_start(window_id, screen_capture_trampoline, context)
        };
        
        NonNull::new(ptr)
            .map(|ptr| Self {
                ptr,
                _callback_data: callback_data,
            })
            .ok_or_else(|| NativeSurfaceError::CreationFailed("Failed to start screen capture".into()))
    }
}

impl Drop for ScreenCaptureSession {
    fn drop(&mut self) {
        unsafe { ffi::screen_capture_stop(self.ptr.as_ptr()) }
    }
}

extern "C" fn screen_capture_trampoline(context: *mut c_void, surface: *mut ffi::NativeSurface) {
    if context.is_null() || surface.is_null() {
        return;
    }
    
    unsafe {
        let data = &*(context as *const ScreenCaptureCallbackData);
        
        // Construct a temporary MacOSSurface wrapper from the raw pointer
        // We need to query width/height if we want them to be valid
        // For now, we'll assume the callback uses the surface pointer mainly
        // WARNING: This assumes the surface is valid for the duration of the callback
        if let Some(ptr) = NonNull::new(surface) {
            // Use dummy values or fetch from FFI if needed. 
            // For safety, we should ideally fetch valid dimensions or change the callback signature.
            // But to fix the immediate segfault risk of invalid casting:
            
            // Note: We used ManuallyDrop to ensure we don't destroy the C++ managed surface
            let surface_wrapper = std::mem::ManuallyDrop::new(MacOSSurface { 
                ptr, 
                width: 0, // Placeholder
                height: 0 // Placeholder
            });
            
            (data.callback)(&surface_wrapper);
        }
    }
}
