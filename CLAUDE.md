# UMIDE — working notes for Claude

## Release & docs hygiene — DO THIS WITH EVERY CHANGE SET
Before finishing any change, check whether these need updating and keep them in sync:
1. **README.md** — features list and Download/install instructions.
2. **GitHub Pages site** (`docs/index.html`) — version badge, feature list, download
   links and copy. (Served from `docs/` on `main`; no separate Pages workflow.)
3. **Version** — the workspace version in `Cargo.toml` (`[workspace.package] version`),
   mirrored in `umide.spec` and `extra/macos/UMIDE.app/Contents/Info.plist`. The WiX MSI
   version (`extra/windows/wix/umide.wxs`) is templated from `Cargo.toml` at build time.
   Bump on release and keep all copies consistent.

## Product positioning (keep accurate everywhere)
- **macOS**: full product — editor + embedded Android/iOS emulators + AI assistant.
- **Windows/Linux**: editor + AI assistant only. Embedded emulators are "coming soon"
  (iOS Simulator is macOS-only permanently; Android embedding is a pending wgpu port).

## Repo conventions
- **Sole commit author**: `dev-josias <kologojosias@gmail.com>`. Do not introduce other
  authors and do not add `Co-Authored-By` trailers.
- `main` is protected (PR required). Branch for changes and open a PR; admin-merge if solo.
- **Releases**: push a `v*` tag → GitHub Actions builds the notarized macOS DMG, Windows
  MSI, and Linux `.deb`, and publishes a GitHub Release.
- **CI cost**: the slow Release build runs ONLY on packaging changes (`release.yml`,
  `extra/**`, `umide.spec`, `docker-bake.hcl`, `Makefile`); ordinary code/dependency PRs
  run only the CI workflow. Use the release workflow's `workflow_dispatch` button to
  dry-run packaging on demand.

## Current status (keep this fresh — it is the cross-machine handoff)
- **v0.2.0 shipped**: notarized macOS DMG, Windows MSI, Linux `.deb` on GitHub Releases.
- **floem** is pinned at `bridgerust/floem@e07fcd5ff148…` (branch `feat/external-texture`
  = upstream-latest + the wgpu external-texture / `VideoFrame` primitive + aspect letterbox).
  It is fetched from git automatically — you only need a local floem clone to iterate on
  the primitive itself.
- **Cross-platform Android embedding** — the portable pure-Rust wgpu path is built & verified:
  - **M1 (done)**: atlas-bypassing `VideoFrame` GPU primitive (in the floem fork).
  - **M2 (done)**: live gRPC stream → `frame_signal` → `VideoFrame`, verified on a real
    Pixel emulator. Code: `crates/umide/src/panel/emulator_stream.rs`. Demos:
    `cargo run -p umide-app --example live_emulator` (GUI) and `… --example grab_frame`
    (headless) with an emulator running on gRPC `8554`.
- **Next milestones**:
  1. **M3 — input**: map floem pointer/keyboard on the view → emulator gRPC (tap/scroll/type).
     Currently view-only ("can't interact with it").
  2. **Un-gate the emulator panel** for Windows/Linux — it is still
     `#[cfg(target_os = "macos")]` in `crates/umide/src/panel/emulator_view.rs`; wiring the
     portable path there is the in-app integration.
  3. Windows Authenticode signing (needs a cert); fix `umide_native/build.rs` host-vs-target
     cfg (use `CARGO_CFG_TARGET_OS`); live-verify the OpenAI/DeepSeek/Gemini AI providers.

## Working across machines (Mac + Windows)
Claude Code conversation history is **local to each machine — it does not sync**. A chat
started on the Mac will not appear in `claude` history on Windows; that is expected. Keep
both machines in sync this way:
- **Code** syncs via git (`github.com/bridgerust/umide`): `git clone`, then branch + PR as
  usual. Pull before starting, push when done.
- **Context** syncs via this file: Claude Code auto-loads `CLAUDE.md` in any session in this
  repo on any OS. The "Current status" above is the handoff — keep it updated as work lands.
- **Windows build prereqs**: a recent Rust toolchain (edition 2024), `protoc` (protobuf
  compiler — the emulator gRPC build needs it), and the MSVC C++ build tools. Then
  `cargo build` works the same as on macOS.
