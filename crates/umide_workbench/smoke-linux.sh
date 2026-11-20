#!/usr/bin/env bash
# Exercise the installed menu entry and binary on a disposable Debian/Ubuntu host.
set -euo pipefail

if [[ $# != 1 || $(uname -s) != Linux ]]; then
    echo "Usage on Linux: $0 path/to/Umide-Workbench-Linux-x86_64.deb" >&2
    exit 2
fi
for tool in dpkg dpkg-query dpkg-deb apt-get desktop-file-validate gio python3 xvfb-run xauth xdotool dbus-run-session; do
    command -v "$tool" >/dev/null || { echo "Missing smoke-test tool: $tool" >&2; exit 2; }
done
deb=$(realpath -- "$1")
package=umide-workbench
app_id=org.bridgerust.umide.workbench
binary=/usr/bin/umide-workbench
desktop=/usr/share/applications/$app_id.desktop
icon=/usr/share/icons/hicolor/scalable/apps/$app_id.svg
notices=/usr/share/doc/$package
helper=$(dirname -- "$0")/verify-installed-relay.py

[[ $(dpkg-deb -f "$deb" Package) == "$package" ]]
[[ $(dpkg-deb -f "$deb" Architecture) == amd64 ]]
[[ $(dpkg --print-architecture) == amd64 ]]
if dpkg-query -W "$package" >/dev/null 2>&1 || [[ -e $binary || -e $desktop || -e $icon || -e $notices ]]; then
    echo "Use a clean host. A Workbench installation or one of its files already exists." >&2
    exit 2
fi

elevate=()
if [[ $EUID != 0 ]]; then
    sudo -n true
    elevate=(sudo -n)
fi
# Explicitly exercise Mesa lavapipe even on machines with a hardware Vulkan ICD.
mesa_icd=
for manifest in /usr/share/vulkan/icd.d/lvp_icd*.json; do
    if [[ -r $manifest ]]; then mesa_icd=$manifest; break; fi
done
if [[ -z $mesa_icd ]]; then
    echo "No Mesa software Vulkan driver found. Install mesa-vulkan-drivers." >&2
    exit 2
fi
stage=$(mktemp -d "${TMPDIR:-/tmp}/umide-linux-install.XXXXXXXX")
cleanup() {
    local result=$?
    trap - EXIT
    if dpkg-query -W "$package" >/dev/null 2>&1; then
        "${elevate[@]}" dpkg --purge "$package" || result=1
    fi
    rm -rf -- "$stage"
    exit "$result"
}
trap cleanup EXIT

"${elevate[@]}" env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$deb"
[[ $(dpkg-query -W -f='${Status}' "$package") == 'install ok installed' ]]
[[ -x $binary && -s $desktop && -s $icon ]]
for notice in copyright LICENSE NOTICE THIRD-PARTY-NOTICES Codicons-LICENSE; do
    test -s "$notices/$notice" || { echo "Missing installed notice: $notice" >&2; exit 1; }
done
desktop-file-validate "$desktop"
python3 - "$desktop" "$app_id" "$icon" <<'PY'
import configparser
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

entry = configparser.ConfigParser(interpolation=None)
entry.read(sys.argv[1], encoding="utf-8")
menu = entry["Desktop Entry"]
assert menu["Type"] == "Application" and menu["Name"] == "Umide Workbench", "Invalid menu item"
assert not menu.getboolean("Hidden", False) and not menu.getboolean("NoDisplay", False), "Hidden menu item"
assert menu["Icon"] == sys.argv[2] and menu["Exec"].split() == ["umide-workbench", "%f"], "Incorrect menu icon or launcher"
assert ET.parse(Path(sys.argv[3])).getroot().tag == "{http://www.w3.org/2000/svg}svg", "Invalid app icon"
PY

python3 "$helper" "$binary" "$stage"
mkdir -m 700 "$stage/runtime" "$stage/test project"
# A private display, D-Bus session and app profile keep this first launch separate
# from installed providers and desktop sessions. Mesa selects a software adapter.
export VK_DRIVER_FILES="$mesa_icd" VK_ICD_FILENAMES="$mesa_icd"
export XDG_RUNTIME_DIR="$stage/runtime" XDG_SESSION_TYPE=x11 GDK_BACKEND=x11 LIBGL_ALWAYS_SOFTWARE=1
export XDG_CONFIG_HOME="$stage/config" XDG_DATA_HOME="$stage/data" XDG_CACHE_HOME="$stage/cache"
export UMIDE_WORKBENCH_DATA_DIR="$stage/profile" UMIDE_CODEX_PATH="$stage/missing-codex" UMIDE_CLAUDE_PATH="$stage/missing-claude"
unset WAYLAND_DISPLAY
xvfb-run -a -s "-screen 0 1440x900x24 -nolisten tcp" dbus-run-session -- \
    env CODEX_HOME="$stage/codex-settings" CLAUDE_CONFIG_DIR="$stage/claude-settings" \
    python3 - "$binary" "$desktop" "$stage" <<'PY'
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

binary, desktop, stage = map(Path, sys.argv[1:])
project = stage / "test project"
profile = os.fsencode("UMIDE_WORKBENCH_DATA_DIR=" + str(stage / "profile"))

def workbench_processes():
    for proc in Path("/proc").glob("[0-9]*"):
        try:
            if (proc / "exe").readlink() == binary and profile in (proc / "environ").read_bytes().split(b"\0"):
                yield int(proc.name)
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            continue

def xdo(*args):
    result = subprocess.run(["xdotool", *args], capture_output=True, text=True, timeout=5)
    return result.stdout.strip() if result.returncode == 0 else ""

try:
    subprocess.run(["gio", "launch", str(desktop), str(project)], check=True, timeout=15)
    deadline = time.monotonic() + 40
    while time.monotonic() < deadline:
        for window in xdo("search", "--onlyvisible", "--name", "^Umide Workbench$").splitlines():
            pid = xdo("getwindowpid", window)
            if not pid.isdigit() or int(pid) not in workbench_processes():
                continue
            geometry = dict(line.split("=", 1) for line in xdo("getwindowgeometry", "--shell", window).splitlines() if "=" in line)
            if int(geometry.get("WIDTH", 0)) < 800 or int(geometry.get("HEIGHT", 0)) < 500:
                continue
            argv = Path("/proc", pid, "cmdline").read_bytes().split(b"\0")
            assert os.fsencode(project) in argv, "The desktop launcher did not pass the selected project"
            time.sleep(1)
            if int(pid) in workbench_processes() and window in xdo("search", "--onlyvisible", "--pid", pid).splitlines():
                print(f"Installed desktop entry opened a visible {geometry['WIDTH']} × {geometry['HEIGHT']} Workbench window.")
                break
        else:
            time.sleep(0.2)
            continue
        break
    else:
        raise RuntimeError("The installed Workbench did not open its own visible window on the virtual X11 display.")
finally:
    for pid in workbench_processes():
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
PY

"${elevate[@]}" dpkg --purge "$package"
if dpkg-query -W "$package" >/dev/null 2>&1 || [[ -e $binary || -e $desktop || -e $icon || -e $notices ]]; then
    echo "Workbench or its desktop integration is still installed after purge." >&2
    exit 1
fi
echo "The installed Workbench, desktop entry, icon and notices were removed."
