// macos_surface.mm - IOSurface-backed GPU surface for macOS
// Provides zero-copy GPU texture sharing between emulator and UMIDE

#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <IOSurface/IOSurface.h>
#import <QuartzCore/QuartzCore.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>

#include "native_surface.h"
#include <atomic>

// ============ Native Surface Implementation ============

struct NativeSurface {
    IOSurfaceRef ioSurface;
    id<MTLDevice> device;
    id<MTLTexture> texture;
    uint32_t width;
    uint32_t height;
    SurfaceFormat format;
    std::atomic<bool> locked;
    
    NativeSurface() : ioSurface(nullptr), device(nil), texture(nil), 
                      width(0), height(0), format(SURFACE_FORMAT_BGRA8), locked(false) {}
    
    ~NativeSurface() {
        if (texture) texture = nil;
        if (device) device = nil;
        if (ioSurface) {
            CFRelease(ioSurface);
            ioSurface = nullptr;
        }
    }
    
    bool create(uint32_t w, uint32_t h, SurfaceFormat fmt) {
        width = w;
        height = h;
        format = fmt;
        
        // Get default Metal device
        device = MTLCreateSystemDefaultDevice();
        if (!device) {
            NSLog(@"Failed to create Metal device");
            return false;
        }
        
        // Create IOSurface properties
        uint32_t bytesPerElement = 4; // RGBA/BGRA
        uint32_t bytesPerRow = width * bytesPerElement;
        
        // Align to 64 bytes for Metal
        bytesPerRow = (bytesPerRow + 63) & ~63;
        
        OSType pixelFormat = (fmt == SURFACE_FORMAT_RGBA8) ? 'RGBA' : 'BGRA';
        
        NSDictionary* properties = @{
            (id)kIOSurfaceWidth: @(width),
            (id)kIOSurfaceHeight: @(height),
            (id)kIOSurfaceBytesPerElement: @(bytesPerElement),
            (id)kIOSurfaceBytesPerRow: @(bytesPerRow),
            (id)kIOSurfacePixelFormat: @(pixelFormat),
        };
        
        ioSurface = IOSurfaceCreate((__bridge CFDictionaryRef)properties);
        if (!ioSurface) {
            NSLog(@"Failed to create IOSurface");
            return false;
        }
        
        // Create Metal texture from IOSurface
        MTLTextureDescriptor* desc = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:
            (fmt == SURFACE_FORMAT_RGBA8) ? MTLPixelFormatRGBA8Unorm : MTLPixelFormatBGRA8Unorm
            width:width
            height:height
            mipmapped:NO];
        
        desc.usage = MTLTextureUsageShaderRead | MTLTextureUsageRenderTarget;
        desc.storageMode = MTLStorageModeShared;
        
        texture = [device newTextureWithDescriptor:desc iosurface:ioSurface plane:0];
        if (!texture) {
            NSLog(@"Failed to create Metal texture from IOSurface");
            CFRelease(ioSurface);
            ioSurface = nullptr;
            return false;
        }
        
        NSLog(@"Created NativeSurface: %dx%d", width, height);
        return true;
    }
};

extern "C" {

NativeSurface* native_surface_create(uint32_t width, uint32_t height, SurfaceFormat format) {
    @autoreleasepool {
        NativeSurface* surface = new NativeSurface();
        if (!surface->create(width, height, format)) {
            delete surface;
            return nullptr;
        }
        return surface;
    }
}

bool native_surface_resize(NativeSurface* surface, uint32_t width, uint32_t height) {
    if (!surface) return false;
    
    @autoreleasepool {
        // Release old resources
        if (surface->texture) surface->texture = nil;
        if (surface->ioSurface) {
            CFRelease(surface->ioSurface);
            surface->ioSurface = nullptr;
        }
        
        // Recreate
        return surface->create(width, height, surface->format);
    }
}

void* native_surface_get_iosurface(NativeSurface* surface) {
    if (!surface) return nullptr;
    return (void*)surface->ioSurface;
}

void* native_surface_get_metal_texture(NativeSurface* surface) {
    if (!surface) return nullptr;
    return (__bridge void*)surface->texture;
}

bool native_surface_lock(NativeSurface* surface) {
    if (!surface || !surface->ioSurface) return false;
    
    bool expected = false;
    if (!surface->locked.compare_exchange_strong(expected, true)) {
        return false; // Already locked
    }
    
    kern_return_t result = IOSurfaceLock(surface->ioSurface, 0, nullptr);
    if (result != kIOReturnSuccess) {
        surface->locked = false;
        return false;
    }
    return true;
}

void native_surface_unlock(NativeSurface* surface) {
    if (!surface || !surface->ioSurface) return;
    
    IOSurfaceUnlock(surface->ioSurface, 0, nullptr);
    surface->locked = false;
}

void* native_surface_get_buffer(NativeSurface* surface) {
    if (!surface || !surface->ioSurface || !surface->locked) return nullptr;
    return IOSurfaceGetBaseAddress(surface->ioSurface);
}

uint32_t native_surface_get_stride(NativeSurface* surface) {
    if (!surface || !surface->ioSurface) return 0;
    return (uint32_t)IOSurfaceGetBytesPerRow(surface->ioSurface);
}

void native_surface_destroy(NativeSurface* surface) {
    if (surface) {
        delete surface;
    }
}

// ============ Screen Capture Implementation ============

API_AVAILABLE(macos(12.3))
@interface UMIDEScreenCaptureDelegate : NSObject <SCStreamDelegate, SCStreamOutput>
@property (nonatomic) FrameCallback callback;
@property (nonatomic) void* context;
@property (nonatomic) NativeSurface* surface;
@end

@implementation UMIDEScreenCaptureDelegate

- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeScreen) return;
    
    CVImageBufferRef imageBuffer = CMSampleBufferGetImageBuffer(sampleBuffer);
    if (!imageBuffer) return;
    
    // Get dimensions
    size_t width = CVPixelBufferGetWidth(imageBuffer);
    size_t height = CVPixelBufferGetHeight(imageBuffer);
    
    // Resize surface if needed
    if (!self.surface || self.surface->width != width || self.surface->height != height) {
        if (self.surface) {
            native_surface_destroy(self.surface);
        }
        self.surface = native_surface_create((uint32_t)width, (uint32_t)height, SURFACE_FORMAT_BGRA8);
        if (!self.surface) return;
    }
    
    // Copy frame data to IOSurface
    if (native_surface_lock(self.surface)) {
        CVPixelBufferLockBaseAddress(imageBuffer, kCVPixelBufferLock_ReadOnly);
        
        void* srcBase = CVPixelBufferGetBaseAddress(imageBuffer);
        size_t srcBytesPerRow = CVPixelBufferGetBytesPerRow(imageBuffer);
        void* dstBase = native_surface_get_buffer(self.surface);
        uint32_t dstBytesPerRow = native_surface_get_stride(self.surface);
        
        // Copy row by row
        for (size_t y = 0; y < height; y++) {
            memcpy((uint8_t*)dstBase + y * dstBytesPerRow,
                   (uint8_t*)srcBase + y * srcBytesPerRow,
                   width * 4);
        }
        
        CVPixelBufferUnlockBaseAddress(imageBuffer, kCVPixelBufferLock_ReadOnly);
        native_surface_unlock(self.surface);
        
        // Invoke callback
        if (self.callback) {
            self.callback(self.context, self.surface);
        }
    }
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    NSLog(@"Screen capture stopped: %@", error);
}

@end

struct ScreenCapture {
    SCStream* stream API_AVAILABLE(macos(12.3));
    UMIDEScreenCaptureDelegate* delegate API_AVAILABLE(macos(12.3));
    dispatch_queue_t queue;
};

ScreenCapture* screen_capture_start(uint32_t window_id, FrameCallback callback, void* context) {
    if (@available(macOS 12.3, *)) {
        @autoreleasepool {
            __block ScreenCapture* capture = new ScreenCapture();
            capture->queue = dispatch_queue_create("com.umide.screencapture", DISPATCH_QUEUE_SERIAL);
            
            // Find the window
            [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent* content, NSError* error) {
                if (error) {
                    NSLog(@"Failed to get shareable content: %@", error);
                    delete capture;
                    return;
                }
                
                SCWindow* targetWindow = nil;
                for (SCWindow* window in content.windows) {
                    if (window.windowID == window_id) {
                        targetWindow = window;
                        break;
                    }
                }
                
                if (!targetWindow) {
                    NSLog(@"Window not found: %u", window_id);
                    delete capture;
                    return;
                }
                
                // Create filter for this window
                SCContentFilter* filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:targetWindow];
                
                // Configure stream
                SCStreamConfiguration* config = [[SCStreamConfiguration alloc] init];
                config.width = targetWindow.frame.size.width * 2; // Retina
                config.height = targetWindow.frame.size.height * 2;
                config.minimumFrameInterval = CMTimeMake(1, 60); // 60 FPS
                config.pixelFormat = kCVPixelFormatType_32BGRA;
                config.showsCursor = NO;
                
                // Create delegate
                capture->delegate = [[UMIDEScreenCaptureDelegate alloc] init];
                capture->delegate.callback = callback;
                capture->delegate.context = context;
                
                // Create and start stream
                capture->stream = [[SCStream alloc] initWithFilter:filter configuration:config delegate:capture->delegate];
                
                NSError* addError = nil;
                [capture->stream addStreamOutput:capture->delegate type:SCStreamOutputTypeScreen sampleHandlerQueue:capture->queue error:&addError];
                
                if (addError) {
                    NSLog(@"Failed to add stream output: %@", addError);
                    delete capture;
                    return;
                }
                
                [capture->stream startCaptureWithCompletionHandler:^(NSError* startError) {
                    if (startError) {
                        NSLog(@"Failed to start capture: %@", startError);
                        delete capture;
                    } else {
                        NSLog(@"Screen capture started for window %u", window_id);
                    }
                }];
            }];
            
            return capture;
        }
    } else {
        NSLog(@"ScreenCaptureKit requires macOS 12.3+");
        return nullptr;
    }
}

void screen_capture_stop(ScreenCapture* capture) {
    if (!capture) return;
    
    if (@available(macOS 12.3, *)) {
        @autoreleasepool {
            if (capture->stream) {
                [capture->stream stopCaptureWithCompletionHandler:^(NSError* error) {
                    if (error) {
                        NSLog(@"Error stopping capture: %@", error);
                    }
                }];
            }
            if (capture->delegate.surface) {
                native_surface_destroy(capture->delegate.surface);
            }
        }
    }
    
    delete capture;
}

} // extern "C"
