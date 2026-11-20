#!/usr/bin/env bash
# Mount read-only and exercise the application after copying it off the image.
set -euo pipefail
if [[ $# != 1 ]]; then
    echo "Usage: $0 path/to/Umide-Workbench-macOS.dmg" >&2
    exit 2
fi
dmg=$1
stage=$(mktemp -d "${TMPDIR:-/tmp}/umide-install.XXXXXXXX")
mountpoint=$stage/volume
mkdir "$mountpoint" "$stage/Installed"
mounted=false
cleanup() {
    if [[ $mounted == true ]]; then
        /usr/bin/hdiutil detach "$mountpoint" >/dev/null || return 1
    fi
    rm -rf "$stage"
}
trap cleanup EXIT
/usr/bin/hdiutil attach -quiet -readonly -nobrowse -mountpoint "$mountpoint" "$dmg"
mounted=true
[[ $(readlink "$mountpoint/Applications") == /Applications ]]
app="$stage/Installed/Umide Workbench.app"
/usr/bin/ditto "$mountpoint/Umide Workbench.app" "$app"
/usr/bin/hdiutil detach "$mountpoint" >/dev/null
mounted=false

/usr/bin/codesign --verify --all-architectures --strict "$app"
/usr/bin/plutil -lint "$app/Contents/Info.plist"
for notice in LICENSE NOTICE THIRD-PARTY-NOTICES Codicons-LICENSE; do
    test -s "$app/Contents/Resources/Licenses/$notice"
done
test -s "$app/Contents/Resources/umide.icns"
binary="$app/Contents/MacOS/umide-workbench"
case "${dmg##*/}" in
    Umide-Workbench-macOS.dmg)
        /usr/bin/lipo "$binary" -verify_arch arm64
        /usr/bin/lipo "$binary" -verify_arch x86_64 ;;
    Umide-Workbench-macOS-arm64.dmg) /usr/bin/lipo "$binary" -verify_arch arm64 ;;
    *) echo "Unexpected Workbench DMG filename: $dmg" >&2; exit 2 ;;
esac
/usr/bin/lipo -archs "$binary"

# The agent launches this executable from its checkout. Exercise its stdio
# relay against a temporary authenticated loopback host without opening the UI.
python3 - "$binary" "$stage" <<'PY'
import concurrent.futures
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import uuid

binary = Path(sys.argv[1])
stage = Path(sys.argv[2]).resolve()
checkout = stage / "checkout"
session = stage / "session"
checkout.mkdir()
session.mkdir()
token = uuid.uuid4()
request = b'{"jsonrpc":"2.0","id":1,"method":"ping"}\n'
response = b'{"jsonrpc":"2.0","id":1,"result":{}}\n'

def exchange(server):
    connection, _ = server.accept()
    with connection:
        connection.settimeout(12)
        with connection.makefile("rb") as reader:
            if reader.read(16) != token.bytes or reader.readline() != request:
                raise RuntimeError("The installed application sent an invalid verification handshake.")
        connection.sendall(response)

with socket.socket() as server:
    server.bind(("127.0.0.1", 0))
    server.listen(1)
    server.settimeout(12)
    endpoint = {"port": server.getsockname()[1], "checkout": str(checkout), "token": str(token)}
    (session / "endpoint.json").write_text(json.dumps(endpoint), encoding="utf-8")
    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as executor:
        check = executor.submit(exchange, server)
        process = subprocess.run(
            [str(binary), "--umide-verify", str(checkout), str(session)],
            input=request, capture_output=True, timeout=12, check=True, cwd=checkout,
            env={**os.environ, "UMIDE_WORKBENCH_DATA_DIR": str(stage / "profile")},
        )
        check.result()
        if process.stdout != response:
            raise RuntimeError("The installed application did not relay the verification response.")
print("Installed application relayed an authenticated verification request.")

# Test first launch with providers absent, so an installed CLI never reads or
# updates a real user's account. Match the window by process ID, never by title.
env = {
    **os.environ, "UMIDE_WORKBENCH_DATA_DIR": str(stage / "profile"),
    "CODEX_HOME": str(stage / "codex-settings"), "CLAUDE_CONFIG_DIR": str(stage / "claude-settings"),
    "UMIDE_CODEX_PATH": str(stage / "no-codex"), "UMIDE_CLAUDE_PATH": str(stage / "no-claude"),
}
probe = r"""import CoreGraphics
import Foundation
let pid = Int(CommandLine.arguments[1])!
let until = Date().addingTimeInterval(20)
repeat {
    let rows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    for row in rows {
        guard row[kCGWindowOwnerPID as String] as? Int == pid,
              row[kCGWindowLayer as String] as? Int == 0,
              let bounds = row[kCGWindowBounds as String] as? [String: Double],
              let w = bounds["Width"], let h = bounds["Height"], w >= 800, h >= 500 else { continue }
        print("Installed application opened its own window (\(Int(w)) × \(Int(h))).")
        exit(0)
    }
    Thread.sleep(forTimeInterval: 0.2)
} while Date() < until
exit(1)
"""
script = stage / "window.swift"
script.write_text(probe, encoding="utf-8")
window = subprocess.Popen(
    [str(binary), str(checkout), "-ApplePersistenceIgnoreState", "YES"],
    cwd=checkout, env=env, stdin=subprocess.DEVNULL,
)
try:
    subprocess.run(["/usr/bin/swift", str(script), str(window.pid)], check=True, timeout=90)
    if window.poll() is not None:
        raise RuntimeError("Workbench exited immediately after opening its window.")
finally:
    window.terminate()
    try:
        window.wait(timeout=5)
    except subprocess.TimeoutExpired:
        window.kill()
        window.wait()
PY
