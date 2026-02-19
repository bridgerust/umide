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

// Custom view that captures and displays an external window's content using ScreenCaptureKit
API_AVAILABLE(macos(12.3))
@interface UmideEmulatorView : NSView <SCStreamDelegate, SCStreamOutput> {
    SCStream* captureStream;
    CGImageRef latestFrame;
    BOOL isCapturing;
    dispatch_queue_t captureQueue;
    CIContext* ciContext;  // Cached — creating per-frame leaks memory at 60fps
    volatile BOOL isBeingDestroyed;
}
- (void)startCapturingWindowWithID:(CGWindowID)windowID;
- (void)stopCapturing;
- (void)showStatus:(NSString*)status;
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
        captureQueue = dispatch_queue_create("com.umide.screencapture", DISPATCH_QUEUE_SERIAL);
        ciContext = [CIContext context];  // Reused for all frame conversions
        [self setWantsLayer:YES];
        self.layer.backgroundColor = [[NSColor blackColor] CGColor];
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
            // Log available windows for debugging
            for (SCWindow* window in content.windows) {
                NSLog(@"  Available window: ID=%u app='%@' title='%@'", 
                      window.windowID, window.owningApplication.applicationName, window.title);
            }
            return;
        }
        
        NSLog(@"UmideEmulatorView: Found window '%@' (app: '%@') for capture", 
              targetWindow.title, targetWindow.owningApplication.applicationName);
        
        // Configure stream for window capture
        SCContentFilter* filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:targetWindow];
        
        SCStreamConfiguration* config = [[SCStreamConfiguration alloc] init];
        config.width = (size_t)MAX(targetWindow.frame.size.width, 1);
        config.height = (size_t)MAX(targetWindow.frame.size.height, 1);
        config.minimumFrameInterval = CMTimeMake(1, 60);  // 60 FPS
        config.pixelFormat = kCVPixelFormatType_32BGRA;
        config.showsCursor = NO;
        config.capturesAudio = NO;
        
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
    // Update layer with status text — for headless mode
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
        dispatch_async(dispatch_get_main_queue(), ^{
            if (self->isBeingDestroyed) {
                CGImageRelease(newFrame);
                return;
            }
            if (self->latestFrame) {
                CGImageRelease(self->latestFrame);
            }
            self->latestFrame = newFrame;
            
            // Use CALayer.contents for hardware-accelerated compositing
            // instead of drawRect: which uses CPU rendering
            self.layer.contents = (__bridge id)newFrame;
        });
    }
}

// SCStreamDelegate method - called on stream errors
- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    NSLog(@"UmideEmulatorView: Stream stopped with error: %@ (code: %ld)", 
          error.localizedDescription, (long)error.code);
    isCapturing = NO;
    
    // Attempt to restart capture after a brief delay (resilience)
    if (!isBeingDestroyed) {
        NSLog(@"UmideEmulatorView: Will not auto-restart — device may have been stopped");
    }
}

- (void)drawRect:(NSRect)dirtyRect {
    // Only used as fallback when layer.contents is not set
    if (!latestFrame) {
        // Draw placeholder
        [[NSColor blackColor] setFill];
        NSRectFill(dirtyRect);
        
        NSString* text = @"Waiting for emulator...";
        NSDictionary* attrs = @{
            NSForegroundColorAttributeName: [NSColor grayColor],
            NSFontAttributeName: [NSFont systemFontOfSize:14]
        };
        NSSize textSize = [text sizeWithAttributes:attrs];
        [text drawAtPoint:NSMakePoint(dirtyRect.size.width/2 - textSize.width/2, 
                                       dirtyRect.size.height/2) withAttributes:attrs];
    }
    // If latestFrame is set, layer.contents handles rendering via GPU compositing
}

- (void)dealloc {
    isBeingDestroyed = YES;
    [self stopCapturing];
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

    bool initialize(void* parent_window, int32_t x, int32_t y, uint32_t width, uint32_t height) override {
        parentView = (__bridge NSView*)parent_window;

        __block bool success = false;

        runOnMainThread(^{
            NSWindow* parentWin = [parentView window];
            if (!parentWin) {
                std::cerr << "MacOSEmulator: Parent view has no window!" << std::endl;
                return;
            }

            // Create a borderless child window
            childWindow = [[NSPanel alloc] initWithContentRect:NSZeroRect
                                                     styleMask:NSWindowStyleMaskBorderless
                                                       backing:NSBackingStoreBuffered
                                                         defer:NO];
            
            [childWindow setReleasedWhenClosed:NO];
            [childWindow setHidesOnDeactivate:NO];
            [childWindow setCanHide:NO];
            [childWindow setOpaque:YES];
            [childWindow setBackgroundColor:[NSColor blackColor]];
            
            emulatorView = [[UmideEmulatorView alloc] initWithFrame:NSMakeRect(0, 0, width, height)];
            [childWindow setContentView:emulatorView];
            
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

    void resize(int32_t x, int32_t y, uint32_t width, uint32_t height) override {
        if (destroyed) return;
        
        // Use runOnMainThread (dispatch_sync) for consistent timing
        // dispatch_async here was causing races with initialize/cleanup
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
            }
        });
    }

    void attach_device(const std::string& device_id) override {
        currentDeviceId = device_id;
        NSLog(@"MacOSEmulator: Attaching device '%s' (platform=%s)", 
              device_id.c_str(), 
              platform == EMULATOR_PLATFORM_ANDROID ? "Android" : "iOS");
        
        // Find the emulator/simulator window
        dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0), ^{
            if (destroyed) return;
            
            CGWindowID windowID = 0;
            
            // Poll for the window to appear (max 30 seconds, 300 attempts * 100ms)
            int maxAttempts = 300;
            for (int attempt = 0; attempt < maxAttempts && windowID == 0; attempt++) {
                if (destroyed) return;
                
                if (platform == EMULATOR_PLATFORM_ANDROID) {
                    // Android Emulator — try multiple process name variants
                    // The emulator uses different process names depending on version/config
                    windowID = findWindowByProcessName("qemu-system", "");
                    if (windowID == 0) {
                        windowID = findWindowByProcessName("emulator64", "");
                    }
                    if (windowID == 0) {
                        windowID = findWindowByProcessName("emulator", "Android");
                    }
                } else {
                    // iOS Simulator — match by device name in window title
                    // Simulator.app window titles contain the device name, e.g. "iPhone 15 Pro"
                    windowID = findWindowByProcessName("Simulator", device_id);
                }
                
                if (windowID == 0) {
                    if (attempt % 50 == 0) {
                        NSLog(@"MacOSEmulator: Waiting for window (attempt %d/%d)...", attempt, maxAttempts);
                    }
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                }
            }
            
            if (destroyed) return;
            
            if (windowID != 0) {
                embeddedWindowID = windowID;
                NSLog(@"MacOSEmulator: Found window ID %u for device '%s'", windowID, device_id.c_str());
                
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (!destroyed && emulatorView) {
                        [emulatorView startCapturingWindowWithID:windowID];
                    }
                });
            } else {
                NSLog(@"MacOSEmulator: ERROR - Could not find window for device '%s' after %d seconds", 
                      device_id.c_str(), maxAttempts / 10);
                
                // Show error status in the view
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (!destroyed && emulatorView) {
                        [emulatorView showStatus:@"Could not find emulator window.\nCheck Screen Recording permission."];
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
};

Emulator* Emulator::create(EmulatorPlatform platform) {
    return new MacOSEmulator(platform);
}

} // namespace umide
