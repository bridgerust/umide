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
