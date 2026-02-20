#pragma once

#include <string>
#include <cstdint>
#include "umide_native_api.h"

namespace umide {

class Emulator {
public:
    virtual ~Emulator() = default;

    // Initialize the native surface/window attachment
    virtual bool initialize(void* parent_window, int32_t x, int32_t y, uint32_t width, uint32_t height) = 0;

    // Handle resizing and moving
    virtual void resize(int32_t x, int32_t y, uint32_t width, uint32_t height) = 0;

    // Attach a specific emulator instance
    virtual void attach_device(const std::string& device_id) = 0;

    // Handle input
    virtual void send_input(const EmulatorInputEvent& event) = 0;

    // Push an RGBA frame for display (used by gRPC streaming for Android)
    virtual void push_frame(const uint8_t* rgba_data, uint32_t width, uint32_t height) = 0;

    // Factory method
    static Emulator* create(EmulatorPlatform platform);
};

} // namespace umide
