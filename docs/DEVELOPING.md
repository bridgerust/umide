# Developing UMIDE

Cross-machine dev notes (macOS · Windows · Linux). Claude Code's `CLAUDE.md` (auto-loaded)
holds the live status + conventions; this is the human-facing setup/workflow.

> **Claude Code chat history does not sync across machines** — it's local per machine, by
> design. The shared context lives in `CLAUDE.md` (committed), and the work syncs via git.

## Prerequisites
- **Rust** (rustup; the repo's edition-2024 toolchain) and **protoc** (protobuf compiler —
  the emulator gRPC build needs it).
- **macOS**: Xcode Command Line Tools. **Windows**: MSVC C++ build tools (Visual Studio
  Build Tools). **Linux**: `sudo make ubuntu-deps`.
- For the emulator work: Android SDK + emulator + a system image, launched with the gRPC
  endpoint, e.g. `emulator -avd <name> -grpc 8554` (`-no-window` for headless is fine).

## Clone & build
```bash
git clone git@github.com:bridgerust/umide.git
cd umide
cargo build
```
`floem` is fetched from its pinned git rev automatically — you do **not** need a floem clone
to build. Clone `bridgerust/floem` separately only to iterate on the floem-side primitive;
then add a **dev-only** `[patch."https://github.com/bridgerust/floem"]` pointing at the local
path (never commit that patch), and re-pin the rev when you push the floem change.

## Refresh to latest — before every session
1. `git checkout main && git pull` (sync the other machine's work), then branch.
2. To pull in the latest **upstream** floem (`lapce/floem`): sync the `bridgerust/floem`
   fork with `upstream/main`, push, then bump umide's floem `rev` in `Cargo.toml` and PR.
   (Details in `CLAUDE.md` → "Before you start".)

## Run the cross-platform emulator demo
With an Android emulator running on gRPC `8554`:
```bash
cargo run -p umide-app --example live_emulator   # GUI: live device via the wgpu primitive
cargo run -p umide-app --example grab_frame      # headless: save one frame to a PNG
```
This is the portable Android-embedding path (M1 + M2) — verified on macOS (Metal); the same
code targets Windows (DX12) and Linux (Vulkan).

## Workflow & conventions
- `main` is protected — branch + PR (admin-merge if solo). **Sole author: dev-josias** (no
  `Co-Authored-By` trailers).
- Code/dependency PRs run **CI only**; the full release build runs on **packaging** changes
  and on `v*` tags (which publish the macOS DMG / Windows MSI / Linux `.deb`).
- Next milestones: **M3 input** (pointer/keyboard → emulator gRPC), un-gating the
  `#[cfg(macos)]` emulator panel for Windows/Linux, Windows Authenticode signing.
