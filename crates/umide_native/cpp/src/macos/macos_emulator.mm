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
}
- (void)startCapturingWindowWithID:(CGWindowID)windowID;
- (void)stopCapturing;
@end

API_AVAILABLE(macos(12.3))
@implementation UmideEmulatorView

- (instancetype)initWithFrame:(NSRect)frameRect {
    self = [super initWithFrame:frameRect];
    if (self) {
        captureStream = nil;
        latestFrame = NULL;
        isCapturing = NO;
        captureQueue = dispatch_queue_create("com.umide.screencapture", DISPATCH_QUEUE_SERIAL);
        [self setWantsLayer:YES];
        self.layer.backgroundColor = [[NSColor blackColor] CGColor];
    }
    return self;
}

- (void)startCapturingWindowWithID:(CGWindowID)windowID {
    if (isCapturing) return;
    
    // Find the SCWindow matching our windowID
    [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent * _Nullable content, NSError * _Nullable error) {
        if (error) {
            NSLog(@"UmideEmulatorView: Failed to get shareable content: %@", error);
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
            NSLog(@"UmideEmulatorView: Could not find window with ID %u", windowID);
            return;
        }
        
        NSLog(@"UmideEmulatorView: Found window '%@' for capture", targetWindow.title);
        
        // Configure stream for window capture
        SCContentFilter* filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:targetWindow];
        
        SCStreamConfiguration* config = [[SCStreamConfiguration alloc] init];
        config.width = (size_t)targetWindow.frame.size.width;
        config.height = (size_t)targetWindow.frame.size.height;
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
                NSLog(@"UmideEmulatorView: Failed to start capture: %@", startError);
            } else {
                self->isCapturing = YES;
                NSLog(@"UmideEmulatorView: Started capturing window");
            }
        }];
    }];
}

- (void)stopCapturing {
    if (!isCapturing || !captureStream) return;
    
    isCapturing = NO;
    [captureStream stopCaptureWithCompletionHandler:^(NSError * _Nullable error) {
        if (error) {
            NSLog(@"UmideEmulatorView: Error stopping capture: %@", error);
        }
    }];
    captureStream = nil;
    
    if (latestFrame) {
        CGImageRelease(latestFrame);
        latestFrame = NULL;
    }
}

// SCStreamOutput delegate method - called for each frame
- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeScreen) return;
    
    CVImageBufferRef imageBuffer = CMSampleBufferGetImageBuffer(sampleBuffer);
    if (!imageBuffer) return;
    
    // Create CGImage from pixel buffer
    CIImage* ciImage = [CIImage imageWithCVPixelBuffer:imageBuffer];
    CIContext* context = [CIContext context];
    CGImageRef newFrame = [context createCGImage:ciImage fromRect:ciImage.extent];
    
    if (newFrame) {
        dispatch_async(dispatch_get_main_queue(), ^{
            if (self->latestFrame) {
                CGImageRelease(self->latestFrame);
            }
            self->latestFrame = newFrame;
            [self setNeedsDisplay:YES];
        });
    }
}

// SCStreamDelegate method - called on stream errors
- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    NSLog(@"UmideEmulatorView: Stream stopped with error: %@", error);
    isCapturing = NO;
}

- (void)drawRect:(NSRect)dirtyRect {
    if (latestFrame) {
        // Draw the captured frame scaled to fit
        CGContextRef ctx = [[NSGraphicsContext currentContext] CGContext];
        CGContextDrawImage(ctx, self.bounds, latestFrame);
    } else {
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
}

- (void)dealloc {
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
    CFIndex count = CFArrayGetCount(windowList);
    
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
        if (std::string(ownerBuffer).find(processName) == std::string::npos) {
            continue;
        }
        
        // Get window title if we need to filter by it
        if (!titleContains.empty()) {
            CFStringRef windowName = (CFStringRef)CFDictionaryGetValue(windowInfo, kCGWindowName);
            if (windowName) {
                char titleBuffer[512];
                if (CFStringGetCString(windowName, titleBuffer, sizeof(titleBuffer), kCFStringEncodingUTF8)) {
                    if (std::string(titleBuffer).find(titleContains) == std::string::npos) {
                        continue;
                    }
                }
            }
        }
        
        // Get window ID
        CFNumberRef windowIDRef = (CFNumberRef)CFDictionaryGetValue(windowInfo, kCGWindowNumber);
        if (windowIDRef) {
            CFNumberGetValue(windowIDRef, kCGWindowIDCFNumberType, &foundWindowID);
            NSLog(@"Found window: process='%s' windowID=%u", ownerBuffer, foundWindowID);
            break;
        }
    }
    
    CFRelease(windowList);
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

public:
    MacOSEmulator(EmulatorPlatform p) : platform(p) {}

    ~MacOSEmulator() override {
        if (childWindow) {
            runOnMainThread(^{
                if (emulatorView) {
                    [emulatorView stopCapturing];
                }
                NSWindow* parent = [childWindow parentWindow];
                if (parent) {
                    [parent removeChildWindow:childWindow];
                }
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
            
            emulatorView = [[UmideEmulatorView alloc] initWithFrame:NSMakeRect(0, 0, width, height)];
            [childWindow setContentView:emulatorView];
            
            // Attach to parent window
            [parentWin addChildWindow:childWindow ordered:NSWindowAbove];
            
            // Initial positioning
            this->resize(x, y, width, height);
            
            success = true;
        });

        return success;
    }

    void resize(int32_t x, int32_t y, uint32_t width, uint32_t height) override {
        dispatch_async(dispatch_get_main_queue(), ^{
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
            CGWindowID windowID = 0;
            
            // Poll for the window to appear (max 10 seconds)
            for (int attempt = 0; attempt < 100 && windowID == 0; attempt++) {
                if (platform == EMULATOR_PLATFORM_ANDROID) {
                    // Android Emulator process name - try different variants
                    windowID = findWindowByProcessName("qemu-system", "");
                    if (windowID == 0) {
                        windowID = findWindowByProcessName("emulator64", "");
                    }
                    if (windowID == 0) {
                        windowID = findWindowByProcessName("emulator", "");
                    }
                } else {
                    // iOS Simulator - window title doesn't contain UDID, just look for "Simulator"
                    windowID = findWindowByProcessName("Simulator", "");
                }
                
                if (windowID == 0) {
                    if (attempt % 20 == 0) {
                        NSLog(@"MacOSEmulator: Waiting for window (attempt %d/100)...", attempt);
                    }
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                }
            }
            
            if (windowID != 0) {
                embeddedWindowID = windowID;
                NSLog(@"MacOSEmulator: Found window ID %u for device '%s'", windowID, device_id.c_str());
                
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (emulatorView) {
                        [emulatorView startCapturingWindowWithID:windowID];
                    }
                });
            } else {
                NSLog(@"MacOSEmulator: ERROR - Could not find window for device '%s' after 10 seconds", device_id.c_str());
            }
        });
    }

    void send_input(const EmulatorInputEvent& /*_event*/) override {
        // TODO: Forward input to emulator/simulator
    }
};

Emulator* Emulator::create(EmulatorPlatform platform) {
    return new MacOSEmulator(platform);
}

} // namespace umide
