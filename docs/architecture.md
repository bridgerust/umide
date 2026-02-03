# UMIDE Architecture Overview

This document describes the high-level architecture of UMIDE (based on Umide and Floem) and outlines the plan for integrating mobile emulator features.

## Core Architecture

UMIDE is built using **Rust** and the **Floem** UI framework. It uses a reactive architecture based on signals (`RwSignal`, `ReadSignal`, `Memo`) to manage state and UI updates.

### Hierarchy

1.  **App (`app.rs`)**:
    - Entry point: `crates/umide/src/bin/lapce.rs`.
    - Manages the application lifecycle and multiple windows.
    - `AppData` holds global state.

2.  **Window (`window.rs`)**:
    - Represents an operating system window.
    - `WindowData` manages window-specific state (size, position).
    - Contains a collection of "Window Tabs" (usually one).

3.  **Window Tab (`window_tab.rs`)**:
    - The main workspace container within a window.
    - `WindowTabData` is the central hub for a workspace, holding:
      - `MainSplitData`: The central editor area (splits, tabs, editors).
      - `PanelData`: Side panels (Terminal, File Explorer, etc.).
      - `TerminalPanelData`: Terminal state.
      - `PluginData`: Plugin system state.
      - `CommonData`: Shared data like keypresses, focus, and configuration.

4.  **Editor (`editor.rs`)**:
    - core text editing component.
    - Managed by `MainSplitData`.
    - Uses `ropey` / `lapce-xi-rope` for efficient text manipulation.

### Panel System

Side panels (File Explorer, Search, Source Control, etc.) are managed by the `PanelData` struct in `src/panel/data.rs`.

- **PanelKind (`src/panel/kind.rs`)**: An enum that identifies each panel type (e.g., `FileExplorer`, `Terminal`, `Search`).
- **PanelData**: Manages the layout, order, and visibility of panels.
  - `panels`: A map of `PanelPosition` (LeftTop, BottomLeft, etc.) to a list of `PanelKind`.
  - `styles`: Stores visibility (`shown`), active tab index, and maximization state.
- **PanelView (`src/panel/view.rs`)**:
  - `panel_view` function switches on `PanelKind` to render the appropriate widget.
  - Panel containers (`panel_container_view`) handle resizing and drag-and-drop.

### Floem and Reactivity

- **Views**: UI building blocks (e.g., `stack`, `label`, `container`).
- **Signals**: State wrappers that trigger UI updates when changed.
  - `create_rw_signal(initial_value)`: Creates a read-write signal.
  - `signal.get()`: Reads value and subscribes the current effect.
  - `signal.set(new_value)`: Updates value and notifies subscribers.
- **Memo**: Derived state that updates only when dependencies change.

## Plan for Emulator Integration

We will add a new **Emulator Panel** to the IDE.

### 1. New Crate: `umide_emulator`

We will create a separate crate to handle the low-level communication with Android and iOS emulators.

- **Android**: Use gRPC to communicate with the Android Emulator (requires custom .proto generation/compilation). Alternatively, generic interaction via adb/window capture if gRPC is too complex for phase 1. _Plan: start with gRPC._
- **iOS**: Use `simctl` for control and standard screen capture (or `xcrun simctl io`) for video.

### 2. UI Integration (`crates/umide`)

1.  **Define `PanelKind::Emulator`**:
    - Update `crates/umide/src/panel/kind.rs` to include `Emulator`.
    - Add an icon in `crates/umide/src/config/icon.rs`.

2.  **Create `EmulatorPanel` Widget**:
    - Create `crates/umide/src/panel/emulator_view.rs`.
    - This view will observe the `umide_emulator` state and render:
      - Connection status.
      - Device list dropdown.
      - Video stream (using a texture or image widget updated periodically).

3.  **Register Panel**:
    - Update `crates/umide/src/panel/view.rs` to handle `PanelKind::Emulator`.
    - Update `crates/umide/src/panel/data.rs` to place it in `PanelPosition::RightTop` by default.

### 3. Video Rendering

Rendering high-frequency video frames in Floem might require efficient handling.

- **Strategy**: Decode frames in `umide_emulator` and pass `Arc<Vec<u8>>` or `wgpu::Texture` to the UI view.
- Floem supports `img` and `svg`, but we might need a custom `Paint` implementation for high-performance video rendering if standard widgets are too slow.

### Data Flow

```mermaid
graph TD
    UserInput[User Input (Click/Key)] --> EmulatorPanel
    EmulatorPanel --> EmulatorInterop[umide_emulator]
    EmulatorInterop -->|gRPC/simctl| Device[Android/iOS Device]
    Device -->|Video Stream| EmulatorInterop
    EmulatorInterop -->|Frames| EmulatorPanel
    EmulatorPanel -->|Floem Signal| UI[Display]
```
