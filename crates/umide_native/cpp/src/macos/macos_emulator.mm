#import <Cocoa/Cocoa.h>
#import <Metal/Metal.h>
#import <QuartzCore/QuartzCore.h>

#include "src/emulator.h"
#include <iostream>

// Forward declaration of the C++ class to use in ObjC
namespace umide { class MacOSEmulator; }

// define a custom NSView to handle rendering/input?
@interface UmideHostView : NSView
@end

@implementation UmideHostView
- (void)drawRect:(NSRect)dirtyRect {
    [[NSColor blackColor] setFill];
    NSRectFill(dirtyRect);

    // Draw a placeholder text
    NSString* text = @"Native Emulator Host";
    NSDictionary* attrs = @{
        NSForegroundColorAttributeName: [NSColor whiteColor],
        NSFontAttributeName: [NSFont systemFontOfSize:24]
    };
    [text drawAtPoint:NSMakePoint(dirtyRect.size.width/2 - 100, dirtyRect.size.height/2) withAttributes:attrs];
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

namespace umide {

class MacOSEmulator : public Emulator {
private:
    NSWindow* childWindow = nil; // The child window hosting the emulator
    UmideHostView* hostView = nil;
    NSView* parentView = nil; // Weak ref

public:
    MacOSEmulator() {}

    ~MacOSEmulator() override {
        if (childWindow) {
            runOnMainThread(^{
                NSWindow* parent = [childWindow parentWindow];
                if (parent) {
                    [parent removeChildWindow:childWindow];
                }
                [childWindow close];
                childWindow = nil;
                hostView = nil;
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

            // Create a borderless child window (NSPanel for floating behavior)
            // Initial frame will be set in resize(), creating 0-size for now
            childWindow = [[NSPanel alloc] initWithContentRect:NSZeroRect
                                                     styleMask:NSWindowStyleMaskBorderless
                                                       backing:NSBackingStoreBuffered
                                                         defer:NO];
            
            [childWindow setReleasedWhenClosed:NO];
            [childWindow setHidesOnDeactivate:NO];
            [childWindow setCanHide:NO];
            
            hostView = [[UmideHostView alloc] initWithFrame:NSMakeRect(0, 0, width, height)];
            [hostView setWantsLayer:YES];
            if (hostView.layer) {
                hostView.layer.backgroundColor = [[NSColor blackColor] CGColor];
            }
            
            [childWindow setContentView:hostView];
            
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
                
                // Get the parent window's frame on screen
                NSRect windowFrame = [parentWin frame];
                NSRect contentRect = [parentWin contentLayoutRect];
                
                // Floem coordinates: (x, y) is top-left of widget in window content coordinates
                // macOS screen coordinates: origin is bottom-left of screen, Y goes up
                
                // Convert: Floem's (x, y from top-left) to screen coords
                // The widget's TOP should be at: windowFrame.origin.y + windowFrame.size.height - contentRect.origin.y - y
                // The widget's BOTTOM should be at: top - height
                
                CGFloat screenX = windowFrame.origin.x + x;
                CGFloat screenTop = windowFrame.origin.y + windowFrame.size.height - contentRect.origin.y - y;
                CGFloat screenY = screenTop - height;  // screenY is BOTTOM of the rect in screen coords
                
                NSRect rectInScreen = NSMakeRect(screenX, screenY, width, height);
                
                NSLog(@"MacOSEmulator resize: input=(%d,%d,%u,%u) winFrame=(%.0f,%.0f,%.0f,%.0f) content=(%.0f,%.0f) screen=(%.0f,%.0f,%.0f,%.0f)",
                    x, y, width, height,
                    windowFrame.origin.x, windowFrame.origin.y, windowFrame.size.width, windowFrame.size.height,
                    contentRect.origin.x, contentRect.origin.y,
                    rectInScreen.origin.x, rectInScreen.origin.y, rectInScreen.size.width, rectInScreen.size.height);
                
                [childWindow setFrame:rectInScreen display:YES];
            }
        });
    }

    void attach_device(const std::string& device_id) override {
        std::cout << "MacOSEmulator: Attaching device " << device_id << std::endl;
        //TODO: Implement Simctl/Metal surface attachment here
    }

    void send_input(const EmulatorInputEvent& /*_event*/) override {
        // Forward input to backend
    }
};

// Android implementation (stub for now, can share NSView logic on MacOS if we use same hosting strategy)
class AndroidEmulator : public Emulator {
    // Android on macOS also ends up being a window we might need to embed via NSView (child window)
    // or just a surface. For now, let's reuse MacOSEmulator-like logic or simpler stub.
    // Actually, on macOS, Android Emulator runs as a separate process (qemu).
    // Embedding it requires ` -qt-hide-window-decorations` and re-parenting the NSWindow using private APIs or
    // simply making the emulator render to a shared texture that we draw in our view.
    // The "Right" way per roadmap is "Native Surface Handle" -> passed to emulator.
    // For now, I'll return a MacOSEmulator for both since we just want to prove embedding.
};

Emulator* Emulator::create(EmulatorPlatform /*_platform*/) {
    // For now, return MacOS implementation for both as a test harness
    // Real implementation would diverge based on backing (Metal vs Vulkan/GFXStream)
    return new MacOSEmulator();
}

} // namespace umide
