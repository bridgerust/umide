#import <Cocoa/Cocoa.h>
#import <Metal/Metal.h>
#import <QuartzCore/QuartzCore.h>
#import <CoreGraphics/CoreGraphics.h>
#import <ApplicationServices/ApplicationServices.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>

#include "src/emulator.h"
#include <iostream>
#include <string>
#include <thread>
#include <chrono>


// Forward declaration of the C++ class to use in ObjC
namespace umide { class MacOSEmulator; }

// Input callback type: platform, eventType (0=down,1=move,2=up), x, y
typedef void (^UmideInputCallback)(int eventType, int x, int y);

// Custom view that captures and displays an external window's content using ScreenCaptureKit
API_AVAILABLE(macos(12.3))
@interface UmideEmulatorView : NSView <SCStreamDelegate, SCStreamOutput> {
    SCStream* captureStream;
    CGImageRef latestFrame;
    BOOL isCapturing;
    dispatch_queue_t captureQueue;
    CIContext* ciContext;
    volatile BOOL isBeingDestroyed;
    // Track the captured image dimensions for coordinate mapping
    CGFloat capturedWidth;
    CGFloat capturedHeight;
}
@property (nonatomic, copy) UmideInputCallback inputCallback;
- (void)startCapturingWindowWithID:(CGWindowID)windowID;
- (void)stopCapturing;
- (void)showStatus:(NSString*)status;
- (void)pushFrameWithData:(const uint8_t*)rgbaData width:(uint32_t)width height:(uint32_t)height;
@end

API_AVAILABLE(macos(12.3))
@implementation UmideEmulatorView

- (instancetype)initWithFrame:(NSRect)frameRect {
    self = [super initWithFrame:frameRect];
    if (self) {
        captureStream = nil;
        latestFrame = NULL;
        isCapturing = NO;
        isBeingDestroyed = NO;
        capturedWidth = 0;
        capturedHeight = 0;
        captureQueue = dispatch_queue_create("com.umide.screencapture", DISPATCH_QUEUE_SERIAL);
        ciContext = [CIContext context];
        [self setWantsLayer:YES];
        self.layer.backgroundColor = [[NSColor blackColor] CGColor];
        // Ensure CoreAnimation scales the video frames automatically while maintaining aspect ratio
        self.layer.contentsGravity = kCAGravityResizeAspect;
    }
    return self;
}

- (void)startCapturingWindowWithID:(CGWindowID)windowID {
    if (isCapturing) return;
    
    NSLog(@"UmideEmulatorView: Requesting capture for window ID %u", windowID);
    
    // Find the SCWindow matching our windowID
    [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent * _Nullable content, NSError * _Nullable error) {
        if (error) {
            NSLog(@"UmideEmulatorView: Failed to get shareable content: %@", error);
            NSLog(@"UmideEmulatorView: Make sure Screen Recording permission is granted in System Settings > Privacy & Security");
            return;
        }
        
        // Find the window with matching ID
        SCWindow* targetWindow = nil;
        for (SCWindow* window in content.windows) {
            if (window.windowID == windowID) {
                targetWindow = window;
                break;
            }
        }
        
        if (!targetWindow) {
            NSLog(@"UmideEmulatorView: Could not find window with ID %u among %lu windows", 
                  windowID, (unsigned long)content.windows.count);
            for (SCWindow* window in content.windows) {
                NSLog(@"  Available window: ID=%u app='%@' title='%@'", 
                      window.windowID, window.owningApplication.applicationName, window.title);
            }
            return;
        }
        
        NSLog(@"UmideEmulatorView: Found window '%@' (app: '%@') size=%.0fx%.0f for capture", 
              targetWindow.title, targetWindow.owningApplication.applicationName,
              targetWindow.frame.size.width, targetWindow.frame.size.height);
        
        // Configure stream for window capture
        SCContentFilter* filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:targetWindow];
        
        SCStreamConfiguration* config = [[SCStreamConfiguration alloc] init];
        config.width = (size_t)MAX(targetWindow.frame.size.width, 1);
        config.height = (size_t)MAX(targetWindow.frame.size.height, 1);
        config.minimumFrameInterval = CMTimeMake(1, 60);  // 60 FPS
        config.pixelFormat = kCVPixelFormatType_32BGRA;
        config.showsCursor = NO;
        config.capturesAudio = NO;
        
        // Remove massive macOS window drop shadow from the capture if supported (macOS 14+)
        if ([config respondsToSelector:NSSelectorFromString(@"setIgnoreShadowsSingleWindow:")]) {
            [config setValue:@YES forKey:@"ignoreShadowsSingleWindow"];
        }
        
        self->captureStream = [[SCStream alloc] initWithFilter:filter configuration:config delegate:self];
        
        NSError* addOutputError;
        BOOL added = [self->captureStream addStreamOutput:self type:SCStreamOutputTypeScreen sampleHandlerQueue:self->captureQueue error:&addOutputError];
        if (!added) {
            NSLog(@"UmideEmulatorView: Failed to add stream output: %@", addOutputError);
            return;
        }
        
        [self->captureStream startCaptureWithCompletionHandler:^(NSError * _Nullable startError) {
            if (startError) {
                NSLog(@"UmideEmulatorView: Failed to start capture: %@ (code: %ld)", 
                      startError.localizedDescription, (long)startError.code);
            } else {
                self->isCapturing = YES;
                NSLog(@"UmideEmulatorView: Started capturing window at %zux%zu @ 60fps", 
                      config.width, config.height);
            }
        }];
    }];
}

- (void)stopCapturing {
    if (!isCapturing || !captureStream) return;
    
    isCapturing = NO;
    SCStream* streamToStop = captureStream;
    captureStream = nil;
    
    [streamToStop stopCaptureWithCompletionHandler:^(NSError * _Nullable error) {
        if (error) {
            NSLog(@"UmideEmulatorView: Error stopping capture: %@", error);
        } else {
            NSLog(@"UmideEmulatorView: Capture stopped cleanly");
        }
    }];
    
    if (latestFrame) {
        CGImageRelease(latestFrame);
        latestFrame = NULL;
    }
}

- (void)showStatus:(NSString*)status {
    NSLog(@"UmideEmulatorView: Status: %@", status);
    [self setNeedsDisplay:YES];
}

// SCStreamOutput delegate method - called for each frame
- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeScreen) return;
    if (isBeingDestroyed) return;
    
    CVImageBufferRef imageBuffer = CMSampleBufferGetImageBuffer(sampleBuffer);
    if (!imageBuffer) return;
    
    // Use cached CIContext to avoid per-frame allocation
    CIImage* ciImage = [CIImage imageWithCVPixelBuffer:imageBuffer];
    CGImageRef newFrame = [ciContext createCGImage:ciImage fromRect:ciImage.extent];
    
    if (newFrame) {
        CGFloat imgW = (CGFloat)CGImageGetWidth(newFrame);
        CGFloat imgH = (CGFloat)CGImageGetHeight(newFrame);
        
        dispatch_async(dispatch_get_main_queue(), ^{
            if (self->isBeingDestroyed) {
                CGImageRelease(newFrame);
                return;
            }
            if (self->latestFrame) {
                CGImageRelease(self->latestFrame);
            }
            self->latestFrame = newFrame;
            self->capturedWidth = imgW;
            self->capturedHeight = imgH;
            
            // Render directly through the layer tree instead of relying on drawRect:
            // This grants hardware-accelerated kCAGravityResizeAspect perfect fit
            self.layer.contents = (__bridge id)self->latestFrame;
        });
    }
}

// SCStreamDelegate method - called on stream errors
- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    NSLog(@"UmideEmulatorView: Stream stopped with error: %@ (code: %ld)", 
          error.localizedDescription, (long)error.code);
    isCapturing = NO;
    
    if (!isBeingDestroyed) {
        NSLog(@"UmideEmulatorView: Will not auto-restart — device may have been stopped");
    }
}

// Calculate the aspect-fit drawing rect for the current frame
- (NSRect)aspectFitRect {
    if (capturedWidth <= 0 || capturedHeight <= 0) return self.bounds;
    
    NSRect bounds = self.bounds;
    CGFloat scaleX = bounds.size.width / capturedWidth;
    CGFloat scaleY = bounds.size.height / capturedHeight;
    CGFloat scale = MIN(scaleX, scaleY);
    
    CGFloat drawW = capturedWidth * scale;
    CGFloat drawH = capturedHeight * scale;
    CGFloat drawX = (bounds.size.width - drawW) / 2.0;
    CGFloat drawY = (bounds.size.height - drawH) / 2.0;
    
    return NSMakeRect(drawX, drawY, drawW, drawH);
}

// Map a view point to emulator coordinates (0..capturedWidth, 0..capturedHeight)
- (NSPoint)viewPointToEmulatorPoint:(NSPoint)viewPoint {
    NSRect drawRect = [self aspectFitRect];
    if (drawRect.size.width <= 0 || drawRect.size.height <= 0) return NSMakePoint(-1, -1);
    
    // Map from view-space draw rect to image-space
    CGFloat relX = (viewPoint.x - drawRect.origin.x) / drawRect.size.width;
    CGFloat relY = (viewPoint.y - drawRect.origin.y) / drawRect.size.height;
    
    // Clamp to valid range
    if (relX < 0 || relX > 1 || relY < 0 || relY > 1) return NSMakePoint(-1, -1);
    
    // NSView is bottom-up, emulator is top-down — flip Y
    CGFloat emuX = relX * capturedWidth;
    CGFloat emuY = (1.0 - relY) * capturedHeight;
    
    return NSMakePoint(emuX, emuY);
}

// Mouse event handling for interaction
- (BOOL)acceptsFirstResponder {
    return YES;
}

- (BOOL)acceptsFirstMouse:(NSEvent *)event {
    return YES;
}

- (void)mouseDown:(NSEvent *)event {
    NSPoint viewPoint = [self convertPoint:[event locationInWindow] fromView:nil];
    NSPoint emuPoint = [self viewPointToEmulatorPoint:viewPoint];
    if (emuPoint.x >= 0 && self.inputCallback) {
        self.inputCallback(0, (int)emuPoint.x, (int)emuPoint.y);
    }
}

- (void)mouseDragged:(NSEvent *)event {
    NSPoint viewPoint = [self convertPoint:[event locationInWindow] fromView:nil];
    NSPoint emuPoint = [self viewPointToEmulatorPoint:viewPoint];
    if (emuPoint.x >= 0 && self.inputCallback) {
        self.inputCallback(1, (int)emuPoint.x, (int)emuPoint.y);
    }
}

- (void)mouseUp:(NSEvent *)event {
    NSPoint viewPoint = [self convertPoint:[event locationInWindow] fromView:nil];
    NSPoint emuPoint = [self viewPointToEmulatorPoint:viewPoint];
    if (emuPoint.x >= 0 && self.inputCallback) {
        self.inputCallback(2, (int)emuPoint.x, (int)emuPoint.y);
    }
}

- (void)scrollWheel:(NSEvent *)event {
    NSPoint viewPoint = [self convertPoint:[event locationInWindow] fromView:nil];
    NSPoint emuPoint = [self viewPointToEmulatorPoint:viewPoint];
    if (emuPoint.x >= 0 && self.inputCallback) {
        // Event type 3 = scroll, pass deltaY as the y coordinate  
        int deltaY = (int)([event scrollingDeltaY] * 3);  // Amplify for emulator
        self.inputCallback(3, (int)emuPoint.x, deltaY);
    }
}

// Removed manual drawRect: - replaced by CALayer rendering

- (void)dealloc {
    isBeingDestroyed = YES;
    [self stopCapturing];
}

// Push RGBA frame data from gRPC (Android native resolution)
- (void)pushFrameWithData:(const uint8_t*)rgbaData width:(uint32_t)width height:(uint32_t)height {
    if (!rgbaData || width == 0 || height == 0) return;
    if (isBeingDestroyed) return;
    
    // Create CGImage from RGBA data
    size_t bytesPerRow = width * 4;
    size_t dataSize = bytesPerRow * height;
    
    CGColorSpaceRef colorSpace = CGColorSpaceCreateDeviceRGB();
    CFDataRef cfData = CFDataCreate(NULL, rgbaData, dataSize);
    CGDataProviderRef provider = CGDataProviderCreateWithCFData(cfData);
    
    CGImageRef newFrame = CGImageCreate(
        width, height,
        8,              // bits per component
        32,             // bits per pixel
        bytesPerRow,
        colorSpace,
        kCGBitmapByteOrderDefault | kCGImageAlphaLast,  // RGBA
        provider,
        NULL,           // decode
        false,          // shouldInterpolate
        kCGRenderingIntentDefault
    );
    
    CGDataProviderRelease(provider);
    CFRelease(cfData);
    CGColorSpaceRelease(colorSpace);
    
    if (newFrame) {
        // Must update UI on main thread
        if ([NSThread isMainThread]) {
            if (latestFrame) CGImageRelease(latestFrame);
            latestFrame = newFrame;
            capturedWidth = width;
            capturedHeight = height;
            
            // Render directly through the layer tree instead of relying on drawRect:
            self.layer.contents = (__bridge id)self->latestFrame;
        } else {
            dispatch_async(dispatch_get_main_queue(), ^{
                if (self->isBeingDestroyed) {
                    CGImageRelease(newFrame);
                    return;
                }
                if (self->latestFrame) CGImageRelease(self->latestFrame);
                self->latestFrame = newFrame;
                self->capturedWidth = width;
                self->capturedHeight = height;
                
                // Render directly through the layer tree instead of relying on drawRect:
                self.layer.contents = (__bridge id)self->latestFrame;
            });
        }
    }
}

@end

// Helper for safe main thread execution
static void runOnMainThread(void (^block)(void)) {
    if ([NSThread isMainThread]) {
        block();
    } else {
        dispatch_sync(dispatch_get_main_queue(), block);
    }
}

// Find window ID by process name and optional window title
static CGWindowID findWindowByProcessName(const std::string& processName, const std::string& titleContains = "") {
    CFArrayRef windowList = CGWindowListCopyWindowInfo(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID
    );
    
    if (!windowList) return 0;
    
    CGWindowID foundWindowID = 0;
    CGWindowID bestMatch = 0;
    CFIndex count = CFArrayGetCount(windowList);
    
    NSLog(@"findWindowByProcessName: Looking for process='%s' title='%s' among %ld windows",
          processName.c_str(), titleContains.c_str(), (long)count);
    
    for (CFIndex i = 0; i < count; i++) {
        CFDictionaryRef windowInfo = (CFDictionaryRef)CFArrayGetValueAtIndex(windowList, i);
        
        // Get owner name (process name)
        CFStringRef ownerName = (CFStringRef)CFDictionaryGetValue(windowInfo, kCGWindowOwnerName);
        if (!ownerName) continue;
        
        char ownerBuffer[256];
        if (!CFStringGetCString(ownerName, ownerBuffer, sizeof(ownerBuffer), kCFStringEncodingUTF8)) {
            continue;
        }
        
        // Check if process name matches
        std::string owner(ownerBuffer);
        if (owner.find(processName) == std::string::npos) {
            continue;
        }
        
        // Get window layer — skip menu bar items and status items (layer != 0)
        CFNumberRef layerRef = (CFNumberRef)CFDictionaryGetValue(windowInfo, kCGWindowLayer);
        int32_t layer = 0;
        if (layerRef) {
            CFNumberGetValue(layerRef, kCFNumberSInt32Type, &layer);
        }
        if (layer != 0) continue;  // Only normal windows (layer 0)
        
        // Get window ID
        CFNumberRef windowIDRef = (CFNumberRef)CFDictionaryGetValue(windowInfo, kCGWindowNumber);
        CGWindowID windowID = 0;
        if (windowIDRef) {
            CFNumberGetValue(windowIDRef, kCGWindowIDCFNumberType, &windowID);
        }
        if (windowID == 0) continue;
        
        // Get window title
        CFStringRef windowName = (CFStringRef)CFDictionaryGetValue(windowInfo, kCGWindowName);
        char titleBuffer[512] = "";
        if (windowName) {
            CFStringGetCString(windowName, titleBuffer, sizeof(titleBuffer), kCFStringEncodingUTF8);
        }
        
        NSLog(@"  Candidate: process='%s' title='%s' windowID=%u layer=%d", 
              ownerBuffer, titleBuffer, windowID, layer);
        
        // If title filter specified, prefer windows matching it
        if (!titleContains.empty()) {
            if (strlen(titleBuffer) > 0 && std::string(titleBuffer).find(titleContains) != std::string::npos) {
                NSLog(@"  -> Title match! Using window %u", windowID);
                foundWindowID = windowID;
                break;
            }
            // Save as fallback in case no title match
            if (bestMatch == 0) {
                bestMatch = windowID;
            }
        } else {
            // No title filter, take the first match
            foundWindowID = windowID;
            break;
        }
    }
    
    CFRelease(windowList);
    
    // Use best match if no exact title match was found
    if (foundWindowID == 0 && bestMatch != 0) {
        NSLog(@"findWindowByProcessName: No title match, using fallback window %u", bestMatch);
        foundWindowID = bestMatch;
    }
    
    return foundWindowID;
}

namespace umide {

class MacOSEmulator : public Emulator {
private:
    NSWindow* childWindow = nil;
    UmideEmulatorView* emulatorView = nil;
    NSView* parentView = nil;
    CGWindowID embeddedWindowID = 0;
    std::string currentDeviceId;
    EmulatorPlatform platform;
    volatile bool destroyed = false;
    uint32_t viewWidth = 400;
    uint32_t viewHeight = 860;

public:
    MacOSEmulator(EmulatorPlatform p) : platform(p) {}

    ~MacOSEmulator() override {
        destroyed = true;
        if (childWindow) {
            runOnMainThread(^{
                if (emulatorView) {
                    [emulatorView stopCapturing];
                }
                NSWindow* parent = [childWindow parentWindow];
                if (parent) {
                    [parent removeChildWindow:childWindow];
                }
                [childWindow orderOut:nil];
                [childWindow close];
                childWindow = nil;
                emulatorView = nil;
            });
        }
    }

    void (*inputCallbackFn)(int32_t, int32_t, int32_t, void*) = nullptr;
    void* inputCallbackUserData = nullptr;

    bool initialize(void* parent_window, int32_t x, int32_t y, uint32_t width, uint32_t height) override {
        parentView = (__bridge NSView*)parent_window;

        __block bool success = false;

        runOnMainThread(^{
            NSWindow* parentWin = [parentView window];
            if (!parentWin) {
                std::cerr << "MacOSEmulator: Parent view has no window!" << std::endl;
                return;
            }

            // Calculate exact screen coordinates for initialization to avoid 0x0 bugs
            NSRect windowFrame = [parentWin frame];
            NSRect contentRect = [parentWin contentLayoutRect];
            CGFloat screenX = windowFrame.origin.x + x;
            CGFloat screenTop = windowFrame.origin.y + windowFrame.size.height - contentRect.origin.y - y;
            CGFloat screenY = screenTop - height;
            NSRect initialChildFrame = NSMakeRect(screenX, screenY, width, height);

            // Create a borderless child window sized precisely
            childWindow = [[NSPanel alloc] initWithContentRect:initialChildFrame
                                                     styleMask:NSWindowStyleMaskBorderless
                                                       backing:NSBackingStoreBuffered
                                                         defer:NO];
            
            [childWindow setReleasedWhenClosed:NO];
            [childWindow setHidesOnDeactivate:NO];
            [childWindow setCanHide:NO];
            [childWindow setOpaque:YES];
            [childWindow setBackgroundColor:[NSColor blackColor]];
            
            emulatorView = [[UmideEmulatorView alloc] initWithFrame:NSMakeRect(0, 0, width, height)];
            
            // Wire up input callback to forward mouse events to Rust
            emulatorView.inputCallback = ^(int eventType, int emuX, int emuY) {
                if (this->inputCallbackFn) {
                    this->inputCallbackFn(eventType, emuX, emuY, this->inputCallbackUserData);
                }
            };
            
            [childWindow setContentView:emulatorView];
            
            // Make the child window accept mouse events for interaction
            [childWindow setIgnoresMouseEvents:NO];
            
            // Attach to parent window
            [parentWin addChildWindow:childWindow ordered:NSWindowAbove];
            
            // Initial positioning
            this->resize(x, y, width, height);
            
            NSLog(@"MacOSEmulator: Initialized child window for %s platform", 
                  platform == EMULATOR_PLATFORM_ANDROID ? "Android" : "iOS");
            
            success = true;
        });

        return success;
    }

    void set_input_callback(void (*callback)(int32_t, int32_t, int32_t, void*), void* user_data) override {
        inputCallbackFn = callback;
        inputCallbackUserData = user_data;
    }

    void resize(int32_t x, int32_t y, uint32_t width, uint32_t height) override {
        if (destroyed) return;
        
        runOnMainThread(^{
            if (parentView && childWindow) {
                NSWindow* parentWin = [parentView window];
                if (!parentWin) return;
                
                NSRect windowFrame = [parentWin frame];
                NSRect contentRect = [parentWin contentLayoutRect];
                
                CGFloat screenX = windowFrame.origin.x + x;
                CGFloat screenTop = windowFrame.origin.y + windowFrame.size.height - contentRect.origin.y - y;
                CGFloat screenY = screenTop - height;
                
                NSRect rectInScreen = NSMakeRect(screenX, screenY, width, height);
                [childWindow setFrame:rectInScreen display:YES];
                
                viewWidth = width;
                viewHeight = height;
                
                // CRITICAL: Explicitly update the content view's frame
                // The NSPanel starts at NSZeroRect, so autoresizingMask from 0x0 stays 0x0
                // We must manually set the view frame to match the window's content area
                if (emulatorView) {
                    [emulatorView setFrame:NSMakeRect(0, 0, width, height)];
                    NSLog(@"MacOSEmulator::resize: window=%dx%d view=%dx%d",
                          (int)width, (int)height,
                          (int)emulatorView.frame.size.width, (int)emulatorView.frame.size.height);
                }
            }
        });
    }

    void attach_device(const std::string& device_id) override {
        currentDeviceId = device_id;
        NSLog(@"MacOSEmulator: Attaching device '%s' (platform=%s)", 
              device_id.c_str(), 
              platform == EMULATOR_PLATFORM_ANDROID ? "Android" : "iOS");
        
        if (platform == EMULATOR_PLATFORM_ANDROID) {
            // Android: emulator runs headless (-no-window), frames arrive via gRPC pushFrame.
            // No window to find, no ScreenCaptureKit, no AppleScript.
            // The Rust side handles gRPC connection and frame streaming.
            NSLog(@"MacOSEmulator: Android device attached (headless mode, frames via gRPC)");
            
            dispatch_async(dispatch_get_main_queue(), ^{
                if (!destroyed && emulatorView) {
                    [emulatorView showStatus:@"Connecting to Android emulator..."];
                }
            });
            return;
        }
        
        // iOS Simulator: Find the simulator window and capture via ScreenCaptureKit
        dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0), ^{
            if (destroyed) return;
            
            CGWindowID windowID = 0;
            
            // Poll for the window to appear (max 30 seconds, 300 attempts * 100ms)
            int maxAttempts = 300;
            for (int attempt = 0; attempt < maxAttempts && windowID == 0; attempt++) {
                if (destroyed) return;
                
                // iOS Simulator — match by device name in window title
                // Simulator.app window titles contain the device name, e.g. "iPhone 15 Pro"
                windowID = findWindowByProcessName("Simulator", device_id);
                
                if (windowID == 0) {
                    if (attempt % 50 == 0) {
                        NSLog(@"MacOSEmulator: Waiting for Simulator window (attempt %d/%d)...", attempt, maxAttempts);
                    }
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                }
            }
            
            if (destroyed) return;
            
            if (windowID != 0) {
                embeddedWindowID = windowID;
                NSLog(@"MacOSEmulator: Found Simulator window ID %u for device '%s'", windowID, device_id.c_str());
                
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (!destroyed && emulatorView) {
                        [emulatorView startCapturingWindowWithID:windowID];
                    }
                });
            } else {
                NSLog(@"MacOSEmulator: ERROR - Could not find Simulator window for device '%s' after %d seconds", 
                      device_id.c_str(), maxAttempts / 10);
                
                // Show error status in the view
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (!destroyed && emulatorView) {
                        [emulatorView showStatus:@"Could not find Simulator window.\nCheck Screen Recording permission."];
                    }
                });
            }
        });
    }

    void send_input(const EmulatorInputEvent& event) override {
        if (destroyed) return;
        
        if (platform == EMULATOR_PLATFORM_ANDROID) {
            // Forward touch events via ADB
            if (event.type == EMULATOR_INPUT_TOUCH_DOWN || event.type == EMULATOR_INPUT_TOUCH_UP) {
                dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0), ^{
                    std::string cmd = std::string("adb shell input tap ") + 
                                     std::to_string(event.x) + " " + std::to_string(event.y);
                    int result = system(cmd.c_str());
                    if (result != 0) {
                        NSLog(@"MacOSEmulator: ADB input command failed with code %d", result);
                    }
                });
            } else if (event.type == EMULATOR_INPUT_KEY_DOWN) {
                dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0), ^{
                    std::string cmd = std::string("adb shell input keyevent ") + 
                                     std::to_string(event.key_code);
                    system(cmd.c_str());
                });
            }
        } else {
            // Forward touch events to iOS Simulator
            // Post CGEvents to the simulator window
            if (embeddedWindowID != 0) {
                if (event.type == EMULATOR_INPUT_TOUCH_DOWN) {
                    CGPoint point = CGPointMake(event.x, event.y);
                    CGEventRef mouseDown = CGEventCreateMouseEvent(
                        NULL, kCGEventLeftMouseDown, point, kCGMouseButtonLeft);
                    if (mouseDown) {
                        CGEventPost(kCGHIDEventTap, mouseDown);
                        CFRelease(mouseDown);
                    }
                } else if (event.type == EMULATOR_INPUT_TOUCH_UP) {
                    CGPoint point = CGPointMake(event.x, event.y);
                    CGEventRef mouseUp = CGEventCreateMouseEvent(
                        NULL, kCGEventLeftMouseUp, point, kCGMouseButtonLeft);
                    if (mouseUp) {
                        CGEventPost(kCGHIDEventTap, mouseUp);
                        CFRelease(mouseUp);
                    }
                } else if (event.type == EMULATOR_INPUT_TOUCH_MOVE) {
                    CGPoint point = CGPointMake(event.x, event.y);
                    CGEventRef mouseDrag = CGEventCreateMouseEvent(
                        NULL, kCGEventLeftMouseDragged, point, kCGMouseButtonLeft);
                    if (mouseDrag) {
                        CGEventPost(kCGHIDEventTap, mouseDrag);
                        CFRelease(mouseDrag);
                    }
                }
            }
        }
    }

    void show() override {
        runOnMainThread(^{
            if (childWindow) {
                [childWindow orderFront:nil];
            }
        });
    }

    void hide() override {
        runOnMainThread(^{
            if (childWindow) {
                [childWindow orderOut:nil];
            }
        });
    }

    void push_frame(const uint8_t* rgba_data, uint32_t width, uint32_t height) override {
        if (destroyed) return;
        
        // Copy data — dispatch_async outlives the caller's buffer
        size_t dataSize = (size_t)width * height * 4;
        uint8_t* dataCopy = (uint8_t*)malloc(dataSize);
        if (!dataCopy) return;
        memcpy(dataCopy, rgba_data, dataSize);
        
        dispatch_async(dispatch_get_main_queue(), ^{
            if (!destroyed && emulatorView) {
                [emulatorView pushFrameWithData:dataCopy width:width height:height];
            }
            free(dataCopy);
        });
    }
};

Emulator* Emulator::create(EmulatorPlatform platform) {
    return new MacOSEmulator(platform);
}

} // namespace umide
