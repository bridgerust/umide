#!/bin/sh
# Assemble a locally signed development app from the workspace build.
set -eu
cd "$(dirname "$0")/../.."
cargo build --locked --target-dir target -p umide-workbench
workbench_app="target/debug/Umide Workbench.app"
contents="$workbench_app/Contents"
mkdir -p "$contents/MacOS" "$contents/Resources"
install -m 755 target/debug/umide-workbench "$contents/MacOS/umide-workbench"
install -m 644 extra/macos/UMIDE.app/Contents/Resources/umide.icns "$contents/Resources/umide.icns"
licenses="$contents/Resources/Licenses"
mkdir -p "$licenses"
install -m 644 LICENSE NOTICE crates/umide_workbench/THIRD-PARTY-NOTICES "$licenses/"
install -m 644 icons/codicons/LICENSE "$licenses/Codicons-LICENSE"
cat > "$contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>umide-workbench</string>
<key>CFBundleIdentifier</key><string>org.bridgerust.umide.workbench.dev</string>
<key>CFBundleName</key><string>Umide Workbench</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleIconFile</key><string>umide.icns</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
plutil -lint "$contents/Info.plist"
codesign --force --sign - "$workbench_app"
codesign --verify --strict "$workbench_app"
echo "$workbench_app"
