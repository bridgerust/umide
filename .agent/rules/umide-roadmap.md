---
trigger: always_on
---

UMIDE — Global System Architecture (Source of Truth)

1. Vision & Non-Negotiable Goals
   Vision

UMIDE (Unified Mobile IDE) is a native, high-performance mobile development IDE that eliminates context switching for Flutter and React Native developers by embedding real device emulators/simulators directly inside the IDE, with zero abstraction leaks.

Primary Pain Points UMIDE Solves

External emulators breaking focus & flow

Slow hot reload feedback loops

Fragmented tooling (terminal, logs, inspector, emulator)

Weak debugging across RN / Flutter layers

Poor GPU embedding in existing IDEs

Platform-specific emulator handling chaos

Non-Negotiable Constraints

Native performance

GPU-first rendering

Cross-platform (macOS, Linux, Windows)

Future-proof for complex graphics (Metal/Vulkan/OpenGL)

No IDE rewrite

Lapce remains the editor core

2. High-Level Architecture (Layered & Decoupled)
   ┌────────────────────────────────────────────┐
   │ UMIDE (Shell) │
   │ │
   │ ┌───────────── Lapce / Floem ───────────┐ │
   │ │ Editor, LSP, Git, Commands, Panels │ │
   │ └────────────────────────────────────────┘ │
   │ │
   │ ┌──────────── Emulator Surface Layer ───┐ │
   │ │ Native GPU View (wgpu-backed) │ │
   │ │ Zero Floem image rendering │ │
   │ └────────────────────────────────────────┘ │
   │ │
   │ ┌──────────── Native Bridge Layer ──────┐ │
   │ │ Rust ↔ C++ ABI boundary │ │
   │ │ Async, message-based IPC │ │
   │ └────────────────────────────────────────┘ │
   │ │
   │ ┌──────────── Emulator Core (C++) ──────┐ │
   │ │ Android Emulator / iOS Simulators │ │
   │ │ GPU, Input, Audio, Sensors │ │
   │ └────────────────────────────────────────┘ │
   │ │
   └────────────────────────────────────────────┘

3. Why Lapce + C++ Is the Correct Choice
   Why NOT rewrite the IDE

Editor/LSP/keybindings are hard problems already solved

Lapce is fast, native, modern

Floem is evolving rapidly

Rewriting = multi-year distraction

Why Rust alone is insufficient (for THIS use case)

Rust is excellent for:

State management

Safety

Tooling orchestration

IPC

Plugin systems

Rust is not ideal for:

Emulator internals

GPU driver interfacing

Existing Android/iOS emulator codebases

Low-level platform APIs (Metal, EGL, Hypervisor)

Why C++ (not C)

C++ gives:

RAII for GPU resources

Object-oriented emulator abstractions

Easier binding to existing emulator SDKs

Safer long-term evolution than pure C

👉 Decision:

Rust = IDE + orchestration

C++ = emulators + GPU + platform integration

4. Core Principle: Emulator Is NOT a Widget

Critical rule for agents:

The emulator is not a Floem image, not a canvas, not a view tree element.

It is a native GPU surface embedded into the window.

5. Emulator Embedding Strategy (Future-Proof)
   The Only Correct Approach

Create a native GPU surface

Attach it to the same window Lapce owns

Share the graphics context

Let the emulator render directly to GPU

Why This Matters

No pixel copies

No CPU bottlenecks

No frame drops

Works for:

Vulkan

Metal

OpenGL

Future rendering APIs

6. Emulator Surface Architecture
   EmulatorView (Rust)
   │
   ├─ Window Handle (platform-specific)
   ├─ Surface ID
   ├─ Event Forwarder
   │ ├─ Pointer
   │ ├─ Keyboard
   │ └─ Scroll
   │
   └─ NativeSurfaceHandle ─────▶ C++ Emulator Core

Responsibilities
Rust (Lapce side)

Layout & docking

Window handle acquisition

Input capture & forwarding

Lifecycle management (start/stop/pause)

IPC orchestration

C++ (Emulator side)

GPU surface creation

Frame rendering

Emulator lifecycle

Input injection

Audio, sensors, clipboard

7. Rust ↔ C++ Boundary (CRITICAL)
   Rule: No direct object sharing

Communication must be:

Message-based

Asynchronous

ABI-stable

Recommended Interface

extern "C" API

Thin C ABI layer

Internals remain pure C++

Example Responsibilities (Conceptual)

emulator_create(window_handle)

emulator_resize(width, height)

emulator_send_input(event)

emulator_shutdown()

Rust never touches emulator internals.

8. IPC & Event Flow
   Input Flow
   User Input
   → Floem Event
   → Rust Input Mapper
   → Native Bridge
   → Emulator Input Injection

Frame Rendering Flow
Emulator GPU Render
→ Native Surface
→ Window Compositor
→ Screen

🚫 No CPU readback
🚫 No image decoding
🚫 No Floem canvas

9. Emulator Abstraction Layer (Multi-Platform)

Design C++ emulator core like this:

EmulatorCore (abstract)
│
├─ AndroidEmulator
│ ├─ AVD / Emulator Engine
│ └─ Vulkan/OpenGL
│
├─ IosSimulator
│ ├─ CoreSimulator
│ └─ Metal
│
└─ Future Devices
├─ Physical Devices
├─ Cloud Devices

UMIDE never cares which emulator is underneath.

10. Flutter & React Native First-Class Support
    Key Integrations (Non-Optional)
    Logs

Structured logs panel

Filter by app / isolate / JS thread

Hyperlinks to source

Hot Reload

One-click

Per emulator instance

Status feedback inline

Debugging

RN:

Metro integration

JS thread inspector

Flutter:

Dart VM Service

Widget inspector

DevTools Embedding (Future)

Embed Flutter DevTools

Embed React DevTools

Dockable panels

11. Process Model
    Emulator Lifecycle

Lazy-start

Reusable sessions

Fast suspend/resume

Multiple emulators simultaneously

Crash Isolation

Emulator crash ≠ IDE crash

Emulator runs in isolated process

Rust supervises

12. Why This Architecture Is Future-Proof

GPU-native

Emulator-agnostic

Language-agnostic

Toolchain-extensible

No UI toolkit lock-in

No rendering shortcuts

You can later add:

VisionOS simulators

Embedded physical devices

Cloud streaming

AI-powered debugging overlays

Without changing fundamentals.

13. Explicit Agent Instructions

Agent MUST:

Never render emulator frames via Floem images

Never decode RGBA in Rust

Never bind Rust to emulator internals

Treat emulator as a native GPU client

Agent SHOULD:

Optimize for zero-copy paths

Prefer async IPC

Assume multi-emulator scenarios

Design APIs versioned & stable

14. Final North Star

UMIDE must feel like Xcode-level integration
with VS Code flexibility
and native performance
— without ever leaving the editor.
