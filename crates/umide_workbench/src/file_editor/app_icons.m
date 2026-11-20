#import <AppKit/AppKit.h>
#include <string.h>

// Read the installed application's icon directly from Launch Services. Return
// a tiny self-contained PNG; the caller owns its output buffer.
size_t umide_editor_icon(const char *identifier, unsigned char *output, size_t capacity) {
    @autoreleasepool {
        NSWorkspace *workspace = NSWorkspace.sharedWorkspace;
        NSString *query = [NSString stringWithUTF8String:identifier];
        if (!query) return 0;
        NSURL *url = query.isAbsolutePath ? [NSURL fileURLWithPath:query]
                                          : [workspace URLForApplicationWithBundleIdentifier:query];
        if (!url) return 0;
        NSImage *icon = [workspace iconForFile:url.path];
        NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc]
            initWithBitmapDataPlanes:NULL pixelsWide:40 pixelsHigh:40
            bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO
            colorSpaceName:NSDeviceRGBColorSpace bytesPerRow:0 bitsPerPixel:0];
        NSGraphicsContext *context = [NSGraphicsContext graphicsContextWithBitmapImageRep:bitmap];
        if (!icon || !context) return 0;
        [NSGraphicsContext saveGraphicsState];
        NSGraphicsContext.currentContext = context;
        context.imageInterpolation = NSImageInterpolationHigh;
        [icon drawInRect:NSMakeRect(0, 0, 40, 40) fromRect:NSZeroRect
               operation:NSCompositingOperationCopy fraction:1.0];
        [NSGraphicsContext restoreGraphicsState];
        NSData *data = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
        if (!data || data.length > capacity) return 0;
        memcpy(output, data.bytes, data.length);
        return data.length;
    }
}
