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

namespace umide {

class MacOSEmulator : public Emulator {
private:
    UmideHostView* hostView = nil;
    NSView* parentView = nil; // Weak ref

public:
    MacOSEmulator() {}

    ~MacOSEmulator() override {
        if (hostView) {
            dispatch_sync(dispatch_get_main_queue(), ^{
                [hostView removeFromSuperview];
                // hostView is autoreleased or strong ref depending on usage,
                // but removeFromSuperview releases the parent's hold.
                hostView = nil;
            });
        }
    }

    bool initialize(void* parent_window, uint32_t width, uint32_t height) override {
        parentView = (__bridge NSView*)parent_window;

        // Ensure we run UI updates on main thread
        __block bool success = false;

        // We block here because init is expected to be synchronous for safety
        dispatch_sync(dispatch_get_main_queue(), ^{
            hostView = [[UmideHostView alloc] initWithFrame:NSMakeRect(0, 0, width, height)];
            if (!hostView) return;

            // Auto resize with parent
            [hostView setAutoresizingMask:NSViewWidthSizable | NSViewHeightSizable];

            // Add to parent
            [parentView addSubview:hostView];

            // Use a CALayer for potential Metal backing
            [hostView setWantsLayer:YES];
            hostView.layer.backgroundColor = [[NSColor colorWithRed:0.1 green:0.1 blue:0.1 alpha:1.0] CGColor];

            success = true;
        });

        return success;
    }

    void resize(uint32_t width, uint32_t height) override {
       // Auto-resizing mask should handle frame changes if parent changes,
       // but if we need explicit sizing:
       dispatch_async(dispatch_get_main_queue(), ^{
           if (hostView) {
               [hostView setFrameSize:NSMakeSize(width, height)];
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
