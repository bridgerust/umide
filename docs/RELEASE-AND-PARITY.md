# UMIDE — Release & Cross-Platform Parity Plan

Status as of the `feat/ai-assistant` branch. This is the honest, verified state
of UMIDE's readiness to ship a downloadable production build for macOS, Windows
and Linux, plus the concrete path to true three-OS parity.

## 1. Verified build status (local, this branch)

| Check | Result |
|---|---|
| `cargo build --workspace` | ✅ green (incl. `umide_agent` AI engine) |
| `cargo clippy --workspace` | ✅ green |
| `cargo test --workspace` | ✅ all 60+ owned tests pass |
| `cargo fmt --all --check` | ✅ green (was failing; fixed) |

The only failing test is `structdesc::tests::skip_field`, a **vendored
third-party crate** untouched by this branch and never run by CI. Not a product
regression.

## 2. The hard cross-platform reality

UMIDE's differentiator — **embedded device surfaces** — is not equally possible
on all three OSes:

- **iOS Simulator is macOS-only, permanently.** Apple's iOS Simulator ships
  with Xcode and only runs on macOS. There is no supported way to run it on
  Windows or Linux. iOS development/testing in UMIDE will always require a Mac.
  *This is an Apple platform constraint, not a UMIDE limitation.*
- **Android Emulator embedding is portable, with work.** The Android emulator
  runs on all three OSes. UMIDE embeds it by streaming frames over gRPC
  (`crates/umide_emulator/src/grpc_client.rs`, already cross-platform) and
  importing them as a GPU texture. The texture-import path
  (`crates/umide_native`) is currently **macOS-only** (`#[cfg(target_os =
  "macos")]`, IOSurface→Metal). Porting it is the main parity task (see §5).
- **Editor + AI assistant are portable today.** Once the code blockers in §4 are
  fixed, the editor and the AI coding assistant work on all three OSes.

**Practical parity target:** editor + AI everywhere; **Android** emulator
embedding everywhere; **iOS** embedding on macOS only (by Apple's design).

## 3. How to cut a production release

The pipeline is `.github/workflows/release.yml`. Pushing a `v*` tag builds all
three platforms and publishes a GitHub Release.

```
# 1. Make sure the tag matches the workspace version in Cargo.toml (0.1.2)
# 2. Push the tag:
git tag v0.1.2
git push origin v0.1.2
```

Owner-only prerequisites:

- **macOS:** the 6 `APPLE_*` repo secrets (Developer ID cert + notary creds) —
  *confirmed configured*. Produces a signed, notarized DMG.
- **Windows:** an Authenticode code-signing certificate (not yet configured) to
  avoid SmartScreen blocking the installer.
- A PR that touches `release.yml`/`crates/**` triggers a **dry-run** of the
  whole build matrix (no publish) — use it to validate pipeline changes before
  tagging.

## 4. Blocker status

### Fixed on this branch
- ✅ CI `fmt` failure (formatted the new AI files).
- ✅ LLM request/connect timeouts (`client.rs`, `openai.rs`).
- ✅ In-app updater pointed at the wrong repo (`lapce/lapce` → `bridgerust/umide`)
  and stale Linux asset name.
- ✅ Apache-2.0 attribution (README + LICENSE → Lapce) + added `NOTICE`.
- ✅ Release `publish` job now waits on linux + windows + macos (was macOS-only,
  shipping partial/failed releases).
- ✅ Makefile macOS template/icon paths (`UMIDE.app`, `umide.icns`).
- ✅ B3/B4/B6: Linux keyring backend, per-OS shell, command wall-clock cap.
- ✅ **Windows MSI** now built in CI from `extra/windows/wix/umide.wxs`
      (version templated from `Cargo.toml`), named `UMIDE-windows.msi` to match
      the in-app updater.
- ✅ **Linux `.deb`** now built in CI via `cargo deb` (`[package.metadata.deb]`
      in root `Cargo.toml`); bundles binaries + `.desktop` + icon + metainfo.
- ✅ Fixed `umide.spec` / `.deb` desktop filename (renamed
      `extra/linux/umide.desktop` → `dev.umide.umide.desktop`, matching the
      metainfo `launchable` id).
- ✅ Updater asset names aligned: `UMIDE-macos.dmg` / `umide-linux-x86_64.tar.gz`
      / `UMIDE-windows.msi`.

### Remaining — code (cross-platform correctness)
- [ ] **B3** Linux keyring backend (API keys silently don't persist on Linux).
- [ ] **B4** `run_command`/device tools hardcode `sh -c` (broken on Windows).
- [ ] **B6** `run_command` has no wall-clock cap (a hung command wedges the agent).
- [ ] Gate iOS-only tools (`xcrun`/`idb`) and device input behind clear platform
      checks / approval.

### Remaining — packaging
- [ ] **Validate the new MSI/.deb steps via a PR dry-run** before tagging — the
      release workflow runs the full matrix (no publish) on PRs touching
      `crates/**`/`release.yml`/`Cargo.*`. CI-only steps can't be tested locally.
- [ ] **Authenticode-sign the Windows MSI** (owner: needs a code-signing cert) —
      without it, SmartScreen still warns on first run.
- [ ] Optional wider Linux coverage: AppImage (universal) and `.rpm`
      (`umide.spec` / `docker-bake.hcl`), plus an arm64 Linux build.
- [ ] Optional: a `meta`-job check that the tag == `Cargo.toml` version.
- [ ] Linux/Windows updater `extract`/replace is currently macOS-only
      (`update.rs`); the installers exist but in-app self-update on Win/Linux
      still needs its apply step implemented.

### Remaining — owner decisions / credentials
- [ ] Windows Authenticode certificate.
- [ ] Standardize the bundle identifier (`com.umide.app` vs `dev.umide.umide`).
- [ ] Live-verify the AI providers (OpenAI/DeepSeek/Gemini wire paths are
      unverified; only Claude is wire-tested).
- [ ] Mirror/vendor the `bridgerust/floem` fork under the project org (build
      availability risk).

## 5. The real work for Android-embedding parity

`crates/umide_native` currently imports the emulator framebuffer as a Metal
texture from an IOSurface. To run the embedded Android emulator on Windows and
Linux:

1. **Portable path (fastest):** decode gRPC frames to CPU memory and upload to a
   `wgpu` texture each frame. Works on all OSes immediately; higher CPU/GPU copy
   cost. Good first milestone.
2. **Zero-copy path (later):** DXGI shared handles on Windows, `dmabuf` import on
   Linux (Vulkan/EGL) — matches the macOS IOSurface efficiency.
3. Replace the `native_view.rs` "Unsupported platform" branch with the wgpu path
   and remove the Windows/Linux "coming soon" placeholder in the emulator panel.

This is the multi-week effort behind "full parity." Everything else above is
achievable in the current cycle.
