"""Exercise the installed Workbench's stdio relay against a private loopback host."""
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
