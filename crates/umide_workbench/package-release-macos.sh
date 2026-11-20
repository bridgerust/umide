#!/usr/bin/env bash
# Package release binaries built with --target {aarch64,x86_64}-apple-darwin.
# Usage: package-release-macos.sh workbench-v0.4.0-beta.1 [--arm64-only]
set -euo pipefail

usage() {
    echo "Usage: $0 workbench-vMAJOR.MINOR.PATCH[-PRERELEASE] [--arm64-only]" >&2
    exit 2
}

if [[ $# -lt 1 || $# -gt 2 ]] || [[ $# -eq 2 && $2 != --arm64-only ]]; then
    usage
fi
tag=$1
arm_only=${2:-}
if [[ ! $tag =~ ^workbench-v(([0-9]+\.){2}[0-9]+)(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?$ ]]; then
    usage
fi
short_version=${BASH_REMATCH[1]}
version=${tag#workbench-v}
build_number=${WORKBENCH_BUILD_NUMBER:-${GITHUB_RUN_NUMBER:-1}}
if [[ ! $build_number =~ ^[1-9][0-9]{0,3}(\.[0-9]{1,2}){0,2}$ ]]; then
    echo "WORKBENCH_BUILD_NUMBER must be a positive Apple bundle build number (for example 1 or 123.2)." >&2
    exit 2
fi

cd "$(dirname "$0")/../.."
repo=$PWD
arm=${WORKBENCH_ARM64_BINARY:-target/aarch64-apple-darwin/release/umide-workbench}
intel=${WORKBENCH_X64_BINARY:-target/x86_64-apple-darwin/release/umide-workbench}
inputs=("$arm")
architectures=(arm64)
name=Umide-Workbench-macOS
if [[ -n $arm_only ]]; then
    name+=-arm64
else
    inputs+=("$intel")
    architectures+=(x86_64)
fi
for index in "${!inputs[@]}"; do
    binary=${inputs[$index]}
    arch=${architectures[$index]}
    if [[ ! -f $binary || $(/usr/bin/lipo -archs "$binary") != "$arch" ]]; then
        echo "Expected a $arch release executable at $binary." >&2
        exit 1
    fi
done

out=$repo/dist/workbench
mkdir -p "$out"
stage=$(mktemp -d "$out/.macos.XXXXXXXX")
trap 'rm -rf "$stage"' EXIT
root=$stage/image
app="$root/Umide Workbench.app"
contents=$app/Contents
resources=$contents/Resources
licenses=$resources/Licenses
executable=$contents/MacOS/umide-workbench
plist=$contents/Info.plist
mkdir -p "$contents/MacOS" "$licenses"
if [[ -n $arm_only ]]; then
    install -m 755 "$arm" "$executable"
else
    /usr/bin/lipo -create "${inputs[@]}" -output "$executable"
    chmod 755 "$executable"
fi
for arch in "${architectures[@]}"; do
    /usr/bin/lipo "$executable" -verify_arch "$arch"
done
install -m 644 extra/macos/UMIDE.app/Contents/Resources/umide.icns "$resources/umide.icns"
install -m 644 LICENSE NOTICE crates/umide_workbench/THIRD-PARTY-NOTICES "$licenses/"
install -m 644 icons/codicons/LICENSE "$licenses/Codicons-LICENSE"

cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleExecutable</key><string>umide-workbench</string>
  <key>CFBundleIdentifier</key><string>org.bridgerust.umide.workbench</string>
  <key>CFBundleDisplayName</key><string>Umide Workbench</string>
  <key>CFBundleName</key><string>Umide Workbench</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>umide.icns</string>
  <key>CFBundleSupportedPlatforms</key><array><string>MacOSX</string></array>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleLocalizations</key><array><string>en</string><string>fr</string><string>es</string><string>pt</string></array>
  <key>CFBundleShortVersionString</key><string>$short_version</string>
  <key>CFBundleVersion</key><string>$build_number</string>
  <key>UmideReleaseVersion</key><string>$version</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
/usr/bin/plutil -lint "$plist"

identity=${APPLE_DEVELOPER_ID_APPLICATION_IDENTITY:-}
if [[ -n $identity ]]; then
    /usr/bin/codesign --force --options runtime --timestamp --sign "$identity" "$app"
else
    /usr/bin/codesign --force --options runtime --timestamp=none --sign - "$app"
fi
/usr/bin/codesign --verify --all-architectures --strict "$app"

ln -s /Applications "$root/Applications"
/usr/bin/hdiutil create -format UDZO -fs HFS+ -volname "Umide Workbench" -srcfolder "$root" "$stage/$name.dmg"
if [[ -n $identity ]]; then
    /usr/bin/codesign --force --timestamp --sign "$identity" "$stage/$name.dmg"
    /usr/bin/codesign --verify --strict "$stage/$name.dmg"
fi
/usr/bin/hdiutil verify "$stage/$name.dmg"
mv -f "$stage/$name.dmg" "$out/$name.dmg"
echo "$out/$name.dmg"
