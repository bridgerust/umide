# UMIDE v0.4.0 — Beta testing guide

Thanks for helping test UMIDE! This beta focuses on the **embedded iOS Simulator
(no Simulator.app)** and a **mobile-first run experience**. Below is how to get
set up, what we'd love you to try, and the rough edges we already know about.

## Install

Download the **Pre-release** build from the
[Releases page](https://github.com/bridgerust/umide/releases):

- **macOS** (Apple Silicon / Intel) — `UMIDE-macos.dmg`, signed & notarized.
  For the embedded iOS Simulator, also install the companion:
  ```sh
  brew install idb-companion
  ```
  Without it, UMIDE falls back to the classic Simulator.app view.
- **Windows** — `.msi` (or `.zip`). Not Authenticode-signed yet, so SmartScreen
  may warn — choose **More info → Run anyway**.
- **Linux** — `.deb` or tarball. **Preview only** — the portable panel builds and
  is expected to work, but hasn't been run live end-to-end. Expect rough edges.

You'll also want the platform SDKs you already use: Xcode (iOS, macOS only) and
the Android SDK/emulator. UMIDE resolves `adb`/`emulator` from
`ANDROID_HOME`/`ANDROID_SDK_ROOT` or the per-OS default.

## What to try

- **Embedded iOS (macOS):** open the Emulators panel → **iOS** → **Start** a
  simulator. It should boot and render **inside UMIDE with no Simulator.app
  window**. Tap icons, swipe, and type into the device.
- **Run on Device:** open a React Native or Flutter project. The top bar shows
  **▶ Android** / **▶ iOS** (macOS) or **▶ Run** (Windows/Linux). Click it and
  watch the app build & launch in a terminal tab.
- **Device Logs:** with an app running, open the **Device Logs** panel (bottom
  dock) and confirm native logs stream with severity colours.
- **AI assistant:** it lives in the **left sidebar** now (the ✨ tab). Try a
  key-free backend (Claude Code / Codex) or your own API key.
- **Embedded Android (all OSes):** Start a Pixel emulator; tap/drag, hardware
  buttons, keyboard, screenshot.

## Known limitations (no need to report these)

- **Embedded iOS refreshes at ~2 fps** — it's a usable "see and drive your app"
  preview, not smooth animation. Faster streaming is planned.
- Embedded iOS **requires `idb_companion`**; without it you get the Simulator.app
  fallback.
- The **Windows installer is unsigned** (Authenticode signing is pending).
- **Linux is preview only** — not yet verified live.

## Giving feedback

- **Something broke or is wrong** →
  [file a beta bug](https://github.com/bridgerust/umide/issues/new?labels=beta&template=beta_feedback.yml)
  (the form asks for OS, device, and steps).
- **Ideas, questions, general feedback** →
  [Discussions](https://github.com/bridgerust/umide/discussions).

When reporting, please include your OS, the device/framework, and — if the app
misbehaved — the debug log (`umide_debug.log` in your system temp directory).
Thank you! 🙏
