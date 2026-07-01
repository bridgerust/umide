<h1 align="center">
  UMIDE
</h1>

<h4 align="center">The Unified IDE for Cross-Platform Mobile Development</h4>

UMIDE is a unified IDE for cross-platform mobile development (React Native + Flutter), built in Rust. It embeds Android Emulator and iOS Simulator directly as panels, eliminating context-switching for mobile developers — and ships a built-in AI coding assistant that can see and drive those devices.

## Download

Get the latest build from the [Releases page](https://github.com/bridgerust/umide/releases/latest):

- **macOS** (Apple Silicon & Intel) — signed & notarized `.dmg`. Full product: editor, embedded Android/iOS emulators, and the AI assistant.
- **Windows** — `.msi` installer. Editor + AI assistant + **embedded, interactive Android emulator (preview)** — live screen with tap/drag, hardware buttons (Home/Back/Recents/Power, volume, rotate), keyboard input, and a screenshot button. iOS Simulator stays macOS-only. _The installer isn't Authenticode-signed yet, so SmartScreen may warn on first run — choose **More info → Run anyway**._
- **Linux** — `.deb` or tarball. Editor + AI assistant + **embedded, interactive Android emulator (preview)** — live screen with tap/drag, hardware buttons (Home/Back/Recents/Power, volume, rotate), keyboard input, and a screenshot button. iOS Simulator stays macOS-only.

## Screenshots

![Android and iOS emulators running side by side](screenshots/emulator-android-ios-running.png)

![Android emulator running](screenshots/emulator-android-running.png)

![Android emulator home screen](screenshots/emulator-android-home.png)

![Emulator device list](screenshots/emulator-device-list.png)

## Features

- **Unified Mobile Environment**: Android Emulator and iOS Simulator embedded directly in the IDE.
- **AI Coding Assistant**: A built-in, approval-gated agent (Claude, OpenAI, DeepSeek, Gemini) that reads your code, proposes edits, runs commands, and can see and drive the embedded device.
- **External agent CLIs — no API key needed**: Point the assistant at your own **Claude Code**, **Codex**, or **Gemini** CLI and drive the real agent in your project on your existing login — no key to paste. Claude Code edits and runs commands with **per-action approval** surfaced right in the panel; Codex runs sandboxed (workspace-confined) behind a session-consent gate; Gemini is read-only. Opt-in — the built-in BYO-key providers stay the default.
- **Chat sessions**: New Chat, a session switcher, and per-workspace history so conversations persist across restarts.
- **Cross-Platform Support**: First-class support for React Native and Flutter.
- **High Performance**: Built on [Floem](https://github.com/lapce/floem) and Rust for lightning-fast speeds.
- **Based on Lapce**: Forked from [Lapce](https://github.com/lapce/lapce), inheriting its Rust-powered speed and editor features.

## License

Copyright 2026 UMIDE contributors
Portions (original editor) Copyright 2023 Lapce contributors

UMIDE is a fork of [Lapce](https://github.com/lapce/lapce). See the [NOTICE](NOTICE) file for attribution.

Released under the Apache License Version 2.
