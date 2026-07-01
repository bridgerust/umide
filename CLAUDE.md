# UMIDE — working notes for Claude

## Before you start — REFRESH TO LATEST (every session, especially when switching machines)
Two machines (Mac + Windows) share this repo but NOT chat history, so always begin from the
latest committed state — never assume your local checkout is current:
0. **Read [`HANDOFF.md`](HANDOFF.md)** — the cross-machine coordination board (who owns what,
   the other machine's active WIP branches, open asks). It's how Mac ⇄ Windows talk *without
   waiting for merges*. Update it and push when your state changes. Sharing WIP needs a
   `git push`, not a merge — the other side sees pushed branches on `git fetch`.
1. **Pull umide**: `git fetch origin --prune`, then `git checkout main && git pull` (the other
   machine may have pushed), then branch for the change. Do this at the start of every session.
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
- **Windows**: editor + AI assistant + **embedded, interactive Android emulator** — live
  frames, pointer tap/drag, hardware buttons (Home/Back/Recents/Power), keyboard text, and
  a screenshot button. Verified live end-to-end on a real `Pixel_9a`. iOS Simulator stays
  macOS-only permanently.
- **Linux**: the *same* portable panel as Windows and it builds in CI (Clippy + the Linux
  `.deb`), but it has **never been run live** — keep it labelled **preview only** until
  someone smoke-tests it. Main runtime unknowns: wgpu/Vulkan rendering the `VideoFrame`
  primitive and the emulator `-gpu host` under X11/Wayland. Decision (2026-07-01): don't
  invest in Linux runtime work now; revisit when there's demand.
- **AI assistant** (all OSes): BYO-key API providers (Claude/OpenAI/DeepSeek/Gemini,
  approval-gated) **and** external agent CLIs — **Claude Code** (reads auto; edits/commands
  with per-action approval), **Codex** (sandboxed workspace-write + session consent; macOS/
  Linux), **Gemini** (read-only). CLI backends are opt-in and never auto-selected.

## Repo conventions
- **Sole commit author**: `dev-josias <kologojosias@gmail.com>`. Do not introduce other
  authors and do not add `Co-Authored-By` trailers.
- `main` is protected (PR required). Branch for changes and open a PR; admin-merge if solo.
- **Releases**: push a `v*` tag → GitHub Actions builds the notarized macOS DMG, Windows
  MSI, and Linux `.deb`, and publishes a GitHub Release.
- **CI cost**: the slow Release build does NOT run on PRs at all. It builds on a `v*`
  tag (real release: build + sign + publish) and on a push to `main` *only when a
  packaging input changed* (`release.yml`/`extra/**`/`umide.spec`/`docker-bake.hcl`/
  `Makefile`) — that main-push case builds to validate packaging but never signs or
  publishes (gated via `meta.outputs.should_build`/`is_release`). Ordinary code/dependency
  PRs run only the CI workflow. Use the release workflow's `workflow_dispatch` button to
  dry-run packaging on demand. NB: pushing changes under `.github/workflows/` needs the
  `gh` token's `workflow` scope (`gh auth refresh -h github.com -s workflow`).

## Current status (keep this fresh — it is the cross-machine handoff)
- **v0.2.0 shipped**: notarized macOS DMG, Windows MSI, Linux `.deb` on GitHub Releases.
- **v0.3.0 in progress (unreleased)**: Windows build enabled + the embedded Android panel
  un-gated for Windows/Linux. Source version bumped to `0.3.0`
  (Cargo.toml/umide.spec/Info.plist); the `docs/index.html` download badge stays `0.2.0`
  until a `v0.3.0` tag is cut. (On the Windows box `origin` was switched to HTTPS via
  `gh` — the SSH key is unauthenticated there; `gh auth setup-git` provides push creds.)
  Landed on `main` toward v0.3.0:
  - **Emulator panel (PR #21, #24, #26)**: pointer tap/drag + **hardware buttons
    (Home/Back/Power) + keyboard text**; idle-screen freeze fixed (frame stream reconnects).
    Verified on a real `Pixel_9a` AVD on Windows.
  - **AI assistant — agent-CLI backends (PR #25)**: the panel can be driven by **Claude
    Code** (reads auto; edits/commands via a per-action approval bridge — an in-process MCP
    `--permission-prompt-tool` server into UMIDE's ApprovalQueue), **Codex** (sandboxed
    `workspace-write` + session consent; gated off on Windows — no sandbox there), and
    **Gemini** (read-only). Seam: `AssistantBackend = Llm | Cli(CliKind)` +
    `AgentRunner`/`CliRunner` in `crates/umide/src/ai/cli/`. Claude+Codex verified live;
    Gemini parser verified from the CLI's source, **live run pending a `gemini` login**.
    Landed a 36-agent adversarial review's must-fixes. Smoke examples: `cli_smoke`,
    `cli_perm_smoke`, `codex_smoke`, `gemini_smoke`.
- **Agent CLOSED LOOP — landed (see→reason→act→see on the embedded emulator).** A code
  audit graded the subsystem 18 Implemented / 4 Partial / 4 Missing; the gaps are now closed:
  - **A2/B3/F3 (#30)**: auto re-observe after a device action (`ToolExecutor::auto_observe`
    hook; `agent.rs` appends a fresh screenshot after `tap`/`swipe`/`type_text`/`press_key`,
    skipped if the model already screenshotted), agent-side screenshot downscale to ≤1280px
    (`downscale_png`; panel button + live stream stay native-res), MockBackend loop test.
  - **E1 + F2 + B4 (#36)**: **E1** one-time per-session device-control consent
    (`ApprovalKind::DeviceControl`; gates tap/swipe/type/keys once). **F2** `adb` 20s timeout
    + single transient-retry (`run_with_retry`). **B4** `describe_ui` — `uiautomator dump` →
    compact tappable-element listing (a11y fallback for RN/Flutter custom UIs).
  - **G2 (#35 Windows producer + #39 Mac consumer)**: the agent targets the device the user
    is viewing (`panel.active_device` → `resolve_target`: explicit `platform` arg wins, else
    selected device — iOS by UDID, Android scoped; else auto-detect).
  - **Windows device-tool fix (#41)**: `adb` now runs via **direct argv** (`run_tool`, no host
    shell) — `cmd /C` was re-parsing `&&`/quotes/`|` and breaking `describe_ui`, `type_text`
    (special chars), and filtered `read_logs` (`grep`) on Windows. Found live on the Pixel.
  - **Remaining follow-ups**: macOS `active_device` producer (Win/Linux mirror
    `running_device`; a `NOTE` marks the macOS spot); consume a panel-provided
    `DeviceInfo.serial` for multi-Android disambiguation (Windows to add); **live-verify on the
    Pixel + capture the demo video/hero screenshots** (Windows — blocked on a provider key).
  - **Coordination**: Mac owns `umide_agent/*` + `crates/umide/src/ai.rs` +
    `ai_assistant_view.rs`; Windows owns the emulator panel files.
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
  - **Prod-readiness pass (done)**: fixed the blockers an adversarial review found —
    B1 stream-latch reset on Stop (panel reconnects on a 2nd Start), B2 async launch (no
    UI freeze), B3 "Connecting…" overlay + header hint, B4 `CREATE_NO_WINDOW` on
    `adb`/`emulator` (`quiet_command` in `android.rs`). Added an in-app PREVIEW badge.
  - **Live plumbing verified on Windows (done)**: against a real `Pixel_9a` AVD on gRPC
    `8554`, `grab_frame` pulled a real 1080×2424 home-screen frame and `tap_test`'s swipe
    opened the notification shade — i.e. `start_emulator_stream` (decode) and
    `start_emulator_input` (touch) both work on Windows.
  - **In-app GUI panel verified on Windows (done)**: ran the real `umide.exe`; the Emulator
    panel renders in the right dock with the PREVIEW badge + `Android · Pixel_9a` header, the
    device is auto-detected, the live screen paints via the wgpu `VideoFrame`, and an
    on-screen tap routed through `view_to_device` → `start_emulator_input` launched an app on
    the device. Window/render path: wgpu Vulkan on Intel Iris Xe. (B1's Stop→full-reboot→Start
    cycle wasn't run live — it forces a slow cold boot — but the latch-reset fix is in place.)
  - **TWO Windows-only runtime bugs found only by running the binary (fixed)**: the build was
    green but the app *crashed on launch* and the AI adb path was broken — neither is caught
    by `cargo build`/`check`. (a) `app/logging.rs` hardcoded `/tmp/umide_debug.log` + a
    `/dev/null` fallback → panic at startup on Windows (os error 3) before any window; now
    `std::env::temp_dir()` + per-OS null device. (b) `ai.rs tool_path_env` returned a
    `:`-joined Unix PATH fed to `cmd /C` → adb unresolvable; now platform PATH separator +
    Android SDK platform-tools. Lesson: a green Windows build says nothing about runtime —
    always launch the binary.
  - **adb/emulator SDK resolution (fixed)**: `android.rs` called `adb`/`emulator` by bare
    name; a stock Android Studio install on Windows doesn't add them to PATH, so the panel
    showed an empty list. Now resolves the SDK from `ANDROID_HOME`/`ANDROID_SDK_ROOT`/the
    per-OS default (`%LOCALAPPDATA%\Android\Sdk`).
  - **Perf — guest was CPU-rendered (fixed)**: interaction felt sluggish because the
    emulator launched with `-gpu auto`, which silently picks the SwiftShader *software*
    rasterizer under `-no-window` (always set for streaming). Now launches `-gpu host`
    (verified: guest GLES renderer went SwiftShader → Intel Iris Xe) with a
    `swiftshader_indirect` fallback if host can't boot. Also: the panel no longer clones
    the ~10 MB RGBA buffer per repaint (`DecodedFrame::rgba_arc` hands over the existing
    `Arc`), and the stream is downscaled to a 1280px long edge while **touch input still
    maps to native pixels** (probed once into a `native_size` signal — verified live: a
    swipe on the downscaled panel still opens the shade).
  - **Hardware buttons + keyboard (PR #24, merged)**: the portable panel has Home/Back/
    Recents/Power buttons in the sidebar (via `EmulatorInput::key` "GoHome"/"GoBack"/
    "AppSwitch"/"Power") and forwards keyboard `KeyDown` (the `video_frame` is `focusable`,
    so clicking into it routes typing to the device). Required fixing
    `EmulatorGrpcClient::send_key` to send a **keydown+keyup** pair — a lone `keypress` is
    ignored by the emulator for non-character keys, which is why keys never reached the
    device (the macOS panel dodges this by shelling out to `adb`). All verified live on
    Windows: Home returns to the home screen, and typing into a device text field (a
    new-contact Name field) shows the characters — keyboard text works too.
  - **Idle-freeze fix (PR #24, merged)**: the frame stream now reconnects on a stall/end
    instead of the #22 watchdog stopping it for good. The emulator only emits a frame when
    the screen *changes*, so an idle screen produced none → the 10s watchdog killed the
    stream and the panel froze (looked like "taps stopped working"). Now `emulator_stream.rs`
    reconnects (a fresh stream repaints immediately); verified live.
- **Next milestones / before tagging v0.3.0**:
  1. **Dry-run the release workflow** (`workflow_dispatch`) before the real `v0.3.0` tag —
     the MSI/`release-lto` path isn't exercised by ordinary CI. Then flip the `docs/index.html`
     badge to `0.3.0`.
  2. Windows Authenticode signing (needs a cert); live-verify the OpenAI/DeepSeek/Gemini
     AI providers (note: `ai.rs` tool PATH on Windows is now fixed, but the providers
     themselves haven't been exercised on Windows). NB: the Mac is working on AI agent
     integration — coordinate before touching AI code.

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
