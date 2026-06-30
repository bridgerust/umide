# UMIDE — working notes for Claude

## Before you start — REFRESH TO LATEST (every session, especially when switching machines)
Two machines (Mac + Windows) share this repo but NOT chat history, so always begin from the
latest committed state — never assume your local checkout is current:
1. **Pull umide**: `git checkout main && git pull` (the other machine may have pushed), then
   branch for the change. Do this at the start of every session.
2. **Keep floem current with its original repo**: umide pins a specific `bridgerust/floem`
   rev. To pull in the latest *upstream* floem (`lapce/floem`) features/fixes, periodically
   sync the fork and re-pin:
   - in a `bridgerust/floem` clone: `git remote add upstream https://github.com/lapce/floem`,
     `git fetch upstream`, merge `upstream/main` into the fork branch, push;
   - then bump the `floem` / `floem_renderer` / `floem-editor-core` `rev` in umide's
     `Cargo.toml` to the new commit, `cargo build` to refresh `Cargo.lock`, and open a PR.
   Do this when you want the latest floem, or before starting significant emulator/UI work.

See `docs/DEVELOPING.md` for the full cross-machine setup/build steps.

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
- **Windows/Linux**: editor + AI assistant + **embedded, interactive Android emulator
  (preview)** — live frames + pointer tap/drag (M3). iOS Simulator stays macOS-only
  permanently. Still preview-grade: hardware buttons (Home/Back/Power) and keyboard text
  are not yet wired into the portable panel, and a live device run on Windows is pending.

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
  - **M3 (done)**: emulator touch input over gRPC — `start_emulator_input` +
    `view_to_device` in `emulator_stream.rs` (merged from the Mac, PR #20). Demos:
    `… --example live_emulator` (now interactive) and `… --example tap_test` (headless swipe).
  - **In-app integration (done)**: `crates/umide/src/panel/emulator_view.rs` ships a
    `(not macos)` `android_panel_portable` that drives the panel via `video_frame` +
    `start_emulator_stream`, and forwards pointer tap/drag through `start_emulator_input` +
    `view_to_device` (PointerDown/Move/Up → touch_down/move/up). iOS panel is omitted on
    Windows/Linux (Simulator is macOS-only permanently).
- **Next milestones**:
  1. **Live-verify the Windows panel** end-to-end with an Android emulator on `localhost:8554`
     (build is green for the real binary; needs a human-driven device run to confirm frames
     render and taps land).
  2. **Hardware buttons + keyboard** in the portable panel (Home/Back/Power via
     `EmulatorInput::key_code`; text via `key`) — the macOS sidebar has these; the portable
     panel currently wires pointer only.
  3. Windows Authenticode signing (needs a cert); live-verify the OpenAI/DeepSeek/Gemini
     AI providers.

## Working across machines (Mac + Windows)
Claude Code conversation history is **local to each machine — it does not sync**. A chat
started on the Mac will not appear in `claude` history on Windows; that is expected. Keep
both machines in sync this way:
- **Code** syncs via git (`github.com/bridgerust/umide`): `git clone`, then branch + PR as
  usual. Pull before starting, push when done.
- **Context** syncs via this file: Claude Code auto-loads `CLAUDE.md` in any session in this
  repo on any OS. The "Current status" above is the handoff — keep it updated as work lands.

### Windows build prereqs
A recent Rust toolchain (edition 2024), `protoc`, and the **MSVC C++ build tools +
Windows SDK** (VS 2022 Build Tools or Community with the "Desktop development with C++"
workload covers both). Without the Windows SDK, even tiny crates fail to link —
`kernel32.lib` / `user32.lib` can't be found. Install via winget:
```powershell
winget install --id Microsoft.WindowsSDK.10.0.26100   # or newer SDK
winget install --id Google.Protobuf                   # protoc for emulator gRPC
```

### Windows build invocation (Claude AND humans)
`link.exe` is only on PATH inside a Visual Studio Developer environment. Humans: use the
**Developer PowerShell for VS 2022** shortcut. For Claude Code's PowerShell tool calls
(env doesn't persist between calls), prefix every build call with the dev shell:
```powershell
Import-Module "C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\Tools\Microsoft.VisualStudio.DevShell.dll"
Enter-VsDevShell -VsInstallPath "C:\Program Files\Microsoft Visual Studio\2022\Community" -SkipAutomaticLocation -DevCmdArguments "-arch=x64 -host_arch=x64" | Out-Null
$env:PROTOC = "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe\bin\protoc.exe"
$env:Path = (Split-Path $env:PROTOC) + ";" + $env:Path
cargo build   # or check, test, etc.
```
The `'vswhere.exe' is not recognized` warning from `Enter-VsDevShell` is harmless.
