<h1 align="center">
  UMIDE
</h1>

<h4 align="center">The mobile AI IDE for Windows and macOS — with emulators built in</h4>

<p align="center">
  <b>🧪 v0.4.0-beta.2 public beta</b><br>
  Try the embedded iOS Simulator and workspace run profiles —
  <a href="https://github.com/bridgerust/umide/issues/new?labels=beta&template=beta_feedback.yml">report a bug</a>
  · <a href="https://github.com/bridgerust/umide/discussions">discuss</a>.
</p>

UMIDE is a **mobile AI IDE for Windows and macOS**, built in Rust for React Native and Flutter. The **Android emulator** runs inside the editor on both platforms. On macOS, the **iOS Simulator** runs headless in the editor with no Simulator.app window. The built-in **AI assistant can see and drive the embedded devices**.

## Download (beta)

Choose the **Pre-release** installer for Windows or macOS from the [Releases page](https://github.com/bridgerust/umide/releases).

- **macOS** (Apple Silicon & Intel) — signed & notarized `.dmg`. Full product: editor, **embedded Android emulator + embedded iOS Simulator (no Simulator.app)**, and the AI assistant. Embedded iOS needs `brew install idb-companion` (without it, UMIDE falls back to the Simulator.app view).
- **Windows** — `.msi` installer. Editor + AI assistant + **embedded, interactive Android emulator (preview)** — live screen with tap/drag, hardware buttons (Home/Back/Recents/Power, volume, rotate), keyboard input, and a screenshot button. iOS Simulator stays macOS-only. _The installer isn't Authenticode-signed yet, so SmartScreen may warn on first run — choose **More info → Run anyway**._

New to the beta? See the [Beta testing guide](BETA.md) for setup, what to try, and known limitations.

## Screenshots

![Android and iOS emulators running side by side](screenshots/emulator-android-ios-running.png)

![Android emulator running](screenshots/emulator-android-running.png)

![Android emulator home screen](screenshots/emulator-android-home.png)

![Emulator device list](screenshots/emulator-device-list.png)

## Features

- **Unified Mobile Environment**: the Android emulator and iOS Simulator embedded directly in the IDE. On macOS the iOS Simulator runs **headless — no Simulator.app window** — booting into the panel with tap/swipe/keyboard input (requires `idb_companion`).
- **Device Logs in the IDE**: the running device's native logs — Android logcat and the iOS simulator's unified log — streamed live in a bottom panel with severity colouring, follow/tail, and Clear. No Android Studio or Xcode needed to read logs.
- **Run on Device** builds and runs your React Native, Flutter, or Expo app in a live terminal. The top bar shows **▶ Android** and **▶ iOS** on macOS and **▶ Run** on Windows. In the command palette, **Run on Device: Choose App Folder** and **Run on Device: Choose Emulator or Simulator** save the target app and device for the workspace. **Device Logs** follows that device after a run.
- **RN/Flutter auto-detection**: UMIDE detects React Native and Flutter workspaces (status-bar badge) and feeds that context to the AI assistant, so the agent knows your stack without being told.
- **AI Coding Assistant — bring your own key**: A built-in, approval-gated agent that connects to **Claude** or **OpenAI** with your API key. It reads your code, proposes edits and runs commands with **per-action approval**, and can see and drive the embedded device to verify its changes.
- **Chat sessions**: New Chat, a session switcher, and per-workspace history so conversations persist across restarts.
- **Framework support** for React Native and Flutter.
- **High Performance**: Built on [Floem](https://github.com/lapce/floem) and Rust for lightning-fast speeds.
- **Based on Lapce**: Forked from [Lapce](https://github.com/lapce/lapce), inheriting its Rust-powered speed and editor features.

## License

Copyright 2026 UMIDE contributors
Portions (original editor) Copyright 2023 Lapce contributors

UMIDE is a fork of [Lapce](https://github.com/lapce/lapce). See the [NOTICE](NOTICE) file for attribution.

Released under the Apache License Version 2.
