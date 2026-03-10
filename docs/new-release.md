# Updating versioning for package managers and whatnot

- App metainfo: `extra/linux/dev.lapce.lapce.metainfo.xml`
- macOS plist template (`CFBundleShortVersionString`): `extra/macos/Lapce.app/Contents/Info.plist`
- Rust: `Cargo.toml`
- Obviously changelog: `CHANGELOG.md`
- RPM spec: `lapce.spec`
- Windows wix spec (`<Product [...] Version=X.X.X>`): `extra/windows/wix/lapce.wxs`

# macOS release signing and notarization

The Release workflow now builds `UMIDE.app`, wraps it in `UMIDE-macos.dmg`, and notarizes that DMG on non-PR runs.

GitHub Actions secrets required for tagged and manual releases:

- `APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64`: base64-encoded Developer ID Application `.p12`
- `APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD`: password for that `.p12`
- `APPLE_DEVELOPER_ID_APPLICATION_IDENTITY`: signing identity name from `codesign -s`, for example `Developer ID Application: ...`
- `APPLE_NOTARY_APPLE_ID`: Apple ID used for notarization
- `APPLE_NOTARY_APP_PASSWORD`: app-specific password for that Apple ID
- `APPLE_NOTARY_TEAM_ID`: Apple Developer team ID

PR runs still build an unsigned `UMIDE-macos.dmg` for packaging validation only. That artifact will still show Gatekeeper warnings because only signed and notarized DMGs avoid the malware warning on first launch.
