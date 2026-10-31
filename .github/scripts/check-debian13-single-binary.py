#!/usr/bin/env python3
"""Run only the delivered ELF in an unrelated directory on Debian 13 minimal."""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import shutil
import socket
import subprocess
import tempfile
import time
from urllib.parse import quote


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--web", type=Path, required=True, help="validator reference only, never passed to server")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    release = platform.freedesktop_os_release()
    if (release.get("ID"), release.get("VERSION_ID"), platform.machine()) != ("debian", "13", "x86_64"):
        raise RuntimeError("requires Debian 13 amd64 runtime container")
    if shutil.which("cargo") or shutil.which("cc"):
        raise RuntimeError("runtime evidence must not depend on a compiler")
    manifest = json.loads((args.web / "build-manifest.json").read_text())
    with tempfile.TemporaryDirectory(prefix="zxs-single-elf-") as directory:
        root = Path(directory)
        binary = root / "zxs"
        shutil.copyfile(args.binary, binary)
        binary.chmod(0o700)
        disk = root / "fixture-disk"
        disk.mkdir(mode=0o700)
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        command = [str(binary), "--management-root", str(root / "management"),
                   "--listen", f"127.0.0.1:{port}", "--allow-development-setup",
                   "--setup-disk", f"fixture-primary={disk}"]
        environment = {"PATH": os.environ["PATH"], "HOME": directory, "LC_ALL": "C"}
        with (root / "server.log").open("wb") as log:
            child = subprocess.Popen(command, cwd=root, env=environment, stdout=log, stderr=subprocess.STDOUT)
            try:
                deadline = time.monotonic() + 30
                while True:
                    if child.poll() is not None:
                        raise RuntimeError("standalone ELF exited before serving embedded Web")
                    try:
                        client = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
                        client.request("GET", "/")
                        response = client.getresponse()
                        response.read()
                        ready = response.status == 200
                        client.close()
                    except OSError:
                        ready = False
                    if ready:
                        break
                    if time.monotonic() >= deadline:
                        raise TimeoutError("standalone ELF HTTP setup readiness")
                    time.sleep(0.1)
                checked = {}
                for name, checksum in sorted(manifest["files"].items()):
                    reference = args.web / name
                    with reference.open("rb") as source:
                        if hashlib.file_digest(source, "sha256").hexdigest() != checksum:
                            raise ValueError("validator Web reference changed")
                    client = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
                    client.request("GET", "/" + quote(name, safe="/"))
                    response = client.getresponse()
                    data = response.read(reference.stat().st_size + 1)
                    if response.status != 200 or hashlib.sha256(data).hexdigest() != checksum:
                        raise ValueError(f"embedded asset differs from delivered-source reference: {name}")
                    client.close()
                    checked[name] = checksum
                result = {"distribution": "debian13-amd64", "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                          "web_assets": checked, "server_web_directory_argument": False,
                          "compiler_present": False, "samba_started": False}
            finally:
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()
                args.output.parent.mkdir(parents=True, exist_ok=True)
                (args.output.parent / "debian13-single-binary.log").write_bytes((root / "server.log").read_bytes())
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
