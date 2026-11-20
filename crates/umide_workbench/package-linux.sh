#!/usr/bin/env bash
# Assemble a Debian package from a native x86-64 GNU/Linux release build.
# Usage: package-linux.sh workbench-v0.4.0-beta.1
set -euo pipefail
export LC_ALL=C
umask 022

usage() {
    echo "Usage: $0 workbench-vMAJOR.MINOR.PATCH[-beta.N]" >&2
    exit 2
}

[[ $# == 1 ]] || usage
if [[ ! $1 =~ ^workbench-v((0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*))(-beta\.([1-9][0-9]*))?$ ]]; then
    usage
fi
release=${BASH_REMATCH[1]}
beta=${BASH_REMATCH[6]:-}
version=$release
if [[ -n $beta ]]; then
    version+="~beta.$beta"
fi

cd "$(dirname "$0")/../.."
repo=$PWD
workspace_version=$(awk '
    /^\[workspace.package\]$/ { workspace = 1; next }
    /^\[/ { workspace = 0 }
    workspace && /^version[[:space:]]*=/ { split($0, fields, "\""); print fields[2]; exit }
' Cargo.toml)
if [[ $release != "$workspace_version" ]]; then
    echo "Tag version $release does not match the Workbench workspace version $workspace_version." >&2
    exit 1
fi
if [[ $(dpkg --print-architecture) != amd64 ]]; then
    echo "Build the package in an amd64 Debian or Ubuntu environment." >&2
    exit 1
fi
dpkg --validate-version "$version"
binary=${WORKBENCH_LINUX_BINARY:-target/release/umide-workbench}
if [[ ! -f $binary || ! -x $binary ]]; then
    echo "Missing release binary at $binary. Build with cargo build --locked --release -p umide-workbench." >&2
    exit 1
fi
header=$(readelf -h "$binary")
if ! grep -Eq 'Class:[[:space:]]+ELF64' <<<"$header" ||
    ! grep -Eq 'Machine:[[:space:]]+Advanced Micro Devices X86-64' <<<"$header"; then
    echo "Expected an x86-64 ELF release binary at $binary." >&2
    exit 1
fi

name=umide-workbench
app_id=org.bridgerust.umide.workbench
out=$repo/dist/workbench
artifact=Umide-Workbench-Linux-x86_64.deb
mkdir -p "$out"
stage=$(mktemp -d "$out/.linux.XXXXXXXX")
trap 'rm -rf "$stage"' EXIT
tree=$stage/package
assets=crates/umide_workbench/linux
doc=$tree/usr/share/doc/$name
install -Dm755 "$binary" "$tree/usr/bin/$name"
install -Dm644 "$assets/$app_id.desktop" "$tree/usr/share/applications/$app_id.desktop"
install -Dm644 "$assets/$app_id.svg" "$tree/usr/share/icons/hicolor/scalable/apps/$app_id.svg"
install -Dm644 "$assets/$app_id.metainfo.xml" "$tree/usr/share/metainfo/$app_id.metainfo.xml"
desktop-file-validate "$tree/usr/share/applications/$app_id.desktop"
install -d -m755 "$doc" "$tree/DEBIAN" "$stage/debian"
install -m644 LICENSE NOTICE crates/umide_workbench/THIRD-PARTY-NOTICES "$doc"
install -m644 icons/codicons/LICENSE "$doc/Codicons-LICENSE"
cat NOTICE LICENSE > "$doc/copyright"

maintainer='KOLOGO JOSIAS <kologojosias@gmail.com>'
description='Native workspace for mobile and web development with coding agents'
cat > "$stage/debian/control" <<CONTROL
Source: $name
Section: devel
Priority: optional
Maintainer: $maintainer

Package: $name
Architecture: amd64
Description: $description
CONTROL
# Use the build distribution's shlibs/symbols database, including minimum ABI
# versions, rather than copying a dependency list from a different Ubuntu release.
shlibs=$(cd "$stage" && dpkg-shlibdeps -O -e"$tree/usr/bin/$name")
if [[ $shlibs != shlibs:Depends=?* ]]; then
    echo "Could not determine shared library dependencies for $name." >&2
    exit 1
fi
size=$(du -sk --apparent-size "$tree/usr" | cut -f1)
cat > "$tree/DEBIAN/control" <<CONTROL
Package: $name
Version: $version
Section: devel
Priority: optional
Architecture: amd64
Maintainer: $maintainer
Installed-Size: $size
Depends: ${shlibs#shlibs:Depends=}, git, libxkbcommon0
Recommends: libxkbcommon-x11-0, libvulkan1, mesa-vulkan-drivers | vulkan-icd, xdg-desktop-portal, xdg-utils, fonts-dejavu-core
Homepage: https://github.com/bridgerust/umide
Description: $description
 Work with Codex or Claude Code alongside your project files, terminal,
 Android emulator and web development server.
CONTROL

dpkg-deb --root-owner-group -Zxz --build "$tree" "$stage/$artifact"
[[ $(dpkg-deb -f "$stage/$artifact" Package) == "$name" ]]
[[ $(dpkg-deb -f "$stage/$artifact" Version) == "$version" ]]
[[ $(dpkg-deb -f "$stage/$artifact" Architecture) == amd64 ]]
mv -f "$stage/$artifact" "$out/$artifact"
echo "$out/$artifact"
