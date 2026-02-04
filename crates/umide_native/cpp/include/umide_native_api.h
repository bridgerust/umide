#ifndef UMIDE_NATIVE_API_H
#define UMIDE_NATIVE_API_H

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque handle to the Emulator C++ object
typedef struct NativeEmulator NativeEmulator;

// Platform types
typedef enum {
    EMULATOR_PLATFORM_ANDROID = 0,
    EMULATOR_PLATFORM_IOS = 1,
} EmulatorPlatform;

// Input event types
typedef enum {
    EMULATOR_INPUT_TOUCH_DOWN = 0,
    EMULATOR_INPUT_TOUCH_MOVE = 1,
    EMULATOR_INPUT_TOUCH_UP = 2,
    EMULATOR_INPUT_KEY_DOWN = 3,
    EMULATOR_INPUT_KEY_UP = 4,
} EmulatorInputType;

typedef struct {
    EmulatorInputType type;
    int32_t x;
    int32_t y;
    int32_t key_code;
    // Add more fields as needed
} EmulatorInputEvent;

// Create a new emulator view hosted within the given parent window.
// parent_window: On macOS, this is a void* pointer to an NSView.
//                On Windows, an HWND. Linux, XID/Wayland surface.
NativeEmulator* umide_native_create_emulator(void* parent_window, int32_t x, int32_t y, uint32_t width, uint32_t height, EmulatorPlatform platform);

// Destroy the emulator view
void umide_native_destroy_emulator(NativeEmulator* emulator);

// Resize and move the emulator view
void umide_native_resize_emulator(NativeEmulator* emulator, int32_t x, int32_t y, uint32_t width, uint32_t height);

// Send input to the emulator
void umide_native_send_input(NativeEmulator* emulator, const EmulatorInputEvent* event);

// Attach a specific device/AVD to this view
// device_id: serial for ADB, UDID for iOS
void umide_native_attach_device(NativeEmulator* emulator, const char* device_id);

#ifdef __cplusplus
}
#endif

#endif // UMIDE_NATIVE_API_H
