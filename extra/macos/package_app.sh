#!/usr/bin/env bash
set -euo pipefail

APP_NAME="${APP_NAME:-UMIDE}"
TEMPLATE_APP="${TEMPLATE_APP:-extra/macos/UMIDE.app}"
ENTITLEMENTS_PLIST="${ENTITLEMENTS_PLIST:-extra/entitlements.plist}"
OUTPUT_DIR="${OUTPUT_DIR:-dist/macos}"
VERSION="${VERSION:-}"
BUNDLE_VERSION="${BUNDLE_VERSION:-1}"
UMIDE_X64="${UMIDE_X64:?UMIDE_X64 is required}"
UMIDE_ARM64="${UMIDE_ARM64:?UMIDE_ARM64 is required}"
PROXY_X64="${PROXY_X64:?PROXY_X64 is required}"
PROXY_ARM64="${PROXY_ARM64:?PROXY_ARM64 is required}"

if [[ -z "${VERSION}" ]]; then
  VERSION="$(
    awk '
      /^\[workspace.package\]/ { in_workspace = 1; next }
      /^\[/ && in_workspace { exit }
      in_workspace && /^version[[:space:]]*=/ {
        gsub(/"/, "", $3)
        print $3
        exit
      }
    ' Cargo.toml
  )"
fi

APP_DIR="${OUTPUT_DIR}/${APP_NAME}.app"
DMG_ROOT="${OUTPUT_DIR}/dmg-root"
DMG_PATH="${OUTPUT_DIR}/${APP_NAME}-macos.dmg"

rm -rf "${APP_DIR}" "${DMG_ROOT}" "${DMG_PATH}"
mkdir -p "${OUTPUT_DIR}" "${DMG_ROOT}"

ditto "${TEMPLATE_APP}" "${APP_DIR}"
mkdir -p "${APP_DIR}/Contents/MacOS"

lipo -create "${UMIDE_X64}" "${UMIDE_ARM64}" -output "${APP_DIR}/Contents/MacOS/umide"
lipo -create "${PROXY_X64}" "${PROXY_ARM64}" -output "${APP_DIR}/Contents/MacOS/umide-proxy"
chmod 755 "${APP_DIR}/Contents/MacOS/umide" "${APP_DIR}/Contents/MacOS/umide-proxy"

plutil -replace CFBundleDisplayName -string "${APP_NAME}" "${APP_DIR}/Contents/Info.plist"
plutil -replace CFBundleName -string "${APP_NAME}" "${APP_DIR}/Contents/Info.plist"
plutil -replace CFBundleShortVersionString -string "${VERSION}" "${APP_DIR}/Contents/Info.plist"
plutil -replace CFBundleVersion -string "${BUNDLE_VERSION}" "${APP_DIR}/Contents/Info.plist"

if [[ -n "${APPLE_DEVELOPER_ID_APPLICATION_IDENTITY:-}" ]]; then
  codesign \
    --force \
    --sign "${APPLE_DEVELOPER_ID_APPLICATION_IDENTITY}" \
    --options runtime \
    --timestamp \
    "${APP_DIR}/Contents/MacOS/umide-proxy"

  codesign \
    --force \
    --sign "${APPLE_DEVELOPER_ID_APPLICATION_IDENTITY}" \
    --entitlements "${ENTITLEMENTS_PLIST}" \
    --options runtime \
    --timestamp \
    "${APP_DIR}/Contents/MacOS/umide"

  codesign \
    --force \
    --sign "${APPLE_DEVELOPER_ID_APPLICATION_IDENTITY}" \
    --entitlements "${ENTITLEMENTS_PLIST}" \
    --options runtime \
    --timestamp \
    "${APP_DIR}"

  codesign --verify --strict --verbose=2 "${APP_DIR}"
fi

ditto "${APP_DIR}" "${DMG_ROOT}/${APP_NAME}.app"
ln -s /Applications "${DMG_ROOT}/Applications"

hdiutil create \
  -volname "${APP_NAME}" \
  -srcfolder "${DMG_ROOT}" \
  -ov \
  -format UDZO \
  "${DMG_PATH}"
