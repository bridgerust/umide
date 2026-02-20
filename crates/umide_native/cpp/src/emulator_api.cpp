#include "umide_native_api.h"
#include "src/emulator.h"
#include <iostream>

using namespace umide;

extern "C" {

NativeEmulator* umide_native_create_emulator(void* parent_window, int32_t x, int32_t y, uint32_t width, uint32_t height, EmulatorPlatform platform) {
    if (!parent_window) {
        std::cerr << "Error: parent_window is null" << std::endl;
        return nullptr;
    }

    Emulator* emulator = Emulator::create(platform);
    if (!emulator) {
        std::cerr << "Error: Failed to create emulator instance" << std::endl;
        return nullptr;
    }

    if (!emulator->initialize(parent_window, x, y, width, height)) {
        std::cerr << "Error: Failed to initialize emulator" << std::endl;
        delete emulator;
        return nullptr;
    }

    return reinterpret_cast<NativeEmulator*>(emulator);
}

void umide_native_destroy_emulator(NativeEmulator* emulator) {
    if (emulator) {
        delete reinterpret_cast<Emulator*>(emulator);
    }
}

void umide_native_resize_emulator(NativeEmulator* emulator, int32_t x, int32_t y, uint32_t width, uint32_t height) {
    if (emulator) {
        reinterpret_cast<Emulator*>(emulator)->resize(x, y, width, height);
    }
}

void umide_native_send_input(NativeEmulator* emulator, const EmulatorInputEvent* event) {
    if (emulator && event) {
        reinterpret_cast<Emulator*>(emulator)->send_input(*event);
    }
}

void umide_native_attach_device(NativeEmulator* emulator, const char* device_id) {
    if (emulator && device_id) {
        reinterpret_cast<Emulator*>(emulator)->attach_device(std::string(device_id));
    }
}

void umide_native_push_frame(NativeEmulator* emulator, const uint8_t* rgba_data, uint32_t width, uint32_t height) {
    if (emulator && rgba_data && width > 0 && height > 0) {
        reinterpret_cast<Emulator*>(emulator)->push_frame(rgba_data, width, height);
    }
}

} // extern "C"
