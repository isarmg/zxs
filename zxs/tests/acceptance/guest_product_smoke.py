#!/usr/bin/env python3
"""Narrow same-guest product smoke; only for the disposable Actions guest."""

import base64
import errno
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import pty
import resource
import secrets
import select
import signal
import socket
import subprocess
import sys
import time

UID = 23100
HOST = "127.0.0.1"
PORT = 38831
SYSTEM = Path("/var/lib/zxs-acceptance-system")
RECYCLE = Path("/mnt/zxs-acceptance-recycle")
DATA = (Path("/mnt/zxs-acceptance-data-a"), Path("/mnt/zxs-acceptance-data-b"))
STATE = Path("/zxs/state/headless")
HELPER = "/zxs/bin/helper"
SERVER = "/zxs/bin/zxs"
ACCOUNT = "fixture"
ROOT = "/zxs/v1/containers"
BODY_LIMIT = 65536


def require(condition, reason):
    if not condition:
        raise RuntimeError(reason)


def guard(*, initialized=False):
    require(os.getuid() == os.geteuid() == UID and os.getgid() == os.getegid() == UID,
            "fixture runner must be the dedicated ordinary account")
    marker = Path("/run/zxs-acceptance-fixture")
    info = marker.lstat()
    require(info.st_uid == 0 and info.st_mode & 0o777 == 0o444
            and not marker.is_symlink()
            and marker.read_bytes() == b"same-sha-disposable-guest\n", "missing fixture admission")
    require(set(os.listdir("/sys/class/net")) == {"lo"}, "guest has an unexpected interface")
    roots = (SYSTEM, RECYCLE, *DATA, STATE)
    require(all(p.resolve(strict=True) == p and p.stat().st_uid == UID
                and p.stat().st_mode & 0o777 == 0o700 for p in roots),
            "fixture roots are not original private directories")
    if initialized:
        require((STATE / "server.conf").is_file()
                and all((p / ".zxs-role-v1").is_file() for p in roots[:-1]),
                "fixture setup records did not survive remount")
    else:
        require(all(not any(p.iterdir()) for p in roots), "fixture roots must start empty")
    require(len({p.stat().st_dev for p in roots[:-1]}) == 4, "fixture disks are not distinct")
    mounts = {
        fields[4]: fields
        for line in Path("/proc/thread-self/mountinfo").read_text().splitlines()
        if len(fields := line.split()) >= 10
    }
    for p in (RECYCLE, *DATA):
        require(str(p) in mounts, "fixture disk is not an exact mountpoint")
        row = mounts[str(p)]
        require(row[3] == "/" and row[row.index("-") + 1] == "ext4"
                and "rw" in row[5].split(","), "fixture disk is not writable ext4")
    for path in (HELPER, SERVER):
        p = Path(path)
        for component in (p, *p.parents):
            info = component.lstat()
            require(info.st_uid == 0 and info.st_mode & 0o022 == 0
                    and not component.is_symlink(), "fixture program path is not root-protected")
    # Keep the process's core dump limit at zero before handling credentials.
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def issue_setup(password):
    paths = tuple(map(str, (SYSTEM, RECYCLE, *DATA)))
    subprocess.run([HELPER, "setup-headless", "check", *paths], check=True, timeout=30)
    pid, fd = pty.fork()
    if pid == 0:
        try:
            os.execv(HELPER, [HELPER, "setup-headless", "new", *paths, ACCOUNT])
        finally:
            os._exit(127)

    buf = b""
    reaped = False
    deadline = time.monotonic() + 90
    prompts = ((b"Type SETUP to approve: ", b"SETUP"), (b"Password: ", password),
               (b"Repeat password: ", password))
    try:
        for prompt, answer in prompts:
            while prompt not in buf:
                require(time.monotonic() < deadline, "setup terminal prompt timeout")
                if select.select([fd], [], [], 1)[0]:
                    block = os.read(fd, 4096)
                    require(block and len(buf) + len(block) <= BODY_LIMIT,
                            "setup terminal closed or excessive output")
                    buf += block
            buf = buf.split(prompt, 1)[1]
            # Do not emit terminal transcript or synthetic credential.
            require(os.write(fd, answer + b"\n") == len(answer) + 1, "short PTY write")
        while time.monotonic() < deadline:
            if select.select([fd], [], [], 0.1)[0]:
                try:
                    block = os.read(fd, 4096)
                except OSError as exc:
                    if exc.errno != errno.EIO:
                        raise
                    block = b""
                require(len(buf) + len(block) <= BODY_LIMIT, "excess setup output")
                buf += block
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                reaped = True
                require(os.waitstatus_to_exitcode(status) == 0, "Helper setup failed")
                require(b"Setup confirmed:" in buf, "missing exact setup confirmation")
                break
        require(reaped, "Helper setup timeout")
    finally:
        os.close(fd)
        if not reaped:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.waitpid(pid, 0)


def digest(body):
    return hashlib.sha256(body).hexdigest()


class Smoke:
    def __init__(self, password):
        self.authorization = "Basic " + base64.b64encode(ACCOUNT.encode() + b":" + password).decode("ascii")
        self.server = None
        self.container = ROOT + "/" + "11" * 16
        self.peer = ROOT + "/" + "12" * 16
        self.peer_node = "23" * 16
        self.peer_operation = "33" * 16
        self.recycle_operation = "34" * 16
        self.first_node = "21" * 16
        self.next_node = "22" * 16
        self.first_operation = "31" * 16
        self.next_operation = "32" * 16

    def request(self, method, target, expected, *, headers=None, body=None, auth=True):
        h = dict(headers or {})
        if auth:
            h["Authorization"] = self.authorization
        conn = http.client.HTTPConnection(HOST, PORT, timeout=15)
        try:
            conn.request(method, target, body, h)
            reply = conn.getresponse()
            lengths = reply.headers.get_all("Content-Length")
            allowed = expected if isinstance(expected, tuple) else (expected,)
            require(reply.status in allowed, f"{method} received status {reply.status}; expected {allowed}")
            require(lengths is not None and len(lengths) == 1 and lengths[0].isascii()
                    and lengths[0].isdecimal() and int(lengths[0]) <= BODY_LIMIT,
                    "bad response extent")
            content = reply.read()
            require(len(content) == int(lengths[0]), "short response")
            return (reply.status if isinstance(expected, tuple) else reply.headers), content
        finally:
            conn.close()

    def start(self):
        self.server = subprocess.Popen(["/usr/bin/setpriv", "--no-new-privs", SERVER,
                                        "--listen", f"{HOST}:{PORT}"])
        deadline = time.monotonic() + 40
        while time.monotonic() < deadline:
            require(self.server.poll() is None, "ZXS exited during startup")
            try:
                self.request("GET", ROOT, 401, headers={"Zxs-View": "compact"}, auth=False)
                return
            except (ConnectionRefusedError, ConnectionResetError):
                time.sleep(0.2)
        raise RuntimeError("ZXS startup timeout")

    def stop(self):
        if self.server is None:
            return
        child, self.server = self.server, None
        if child.poll() is None:
            child.terminate()
        try:
            status = child.wait(timeout=20)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)
            raise RuntimeError("ZXS did not drain normally") from None
        require(status == 0, "ZXS did not exit cleanly")

    def save_headers(self, operation, node, body, parent=None, *, name=b"fixture.bin"):
        fields = {"Content-Type": "application/octet-stream", "Content-Length": str(len(body)),
                  "Zxs-Operation": operation, "Zxs-Node": node, "Zxs-Sha256": digest(body),
                  "Zxs-Base-Node": parent or "0"}
        if parent is None:
            fields["Zxs-Initial-Name"] = name.hex()
        return fields

    def receipt(self, operation, expected=200):
        _, raw = self.request("GET", self.container + "/operations/" + operation, expected)
        return json.loads(raw)

    def read_body(self, node, expected, container=None):
        path = (container or self.container) + "/nodes/" + node + "/data"
        headers, body = self.request("GET", path, 200)
        require(headers.get_all("Zxs-Sha256") == [digest(expected)] and body == expected,
                "committed body or digest differs")

    def expect_receipt(self, receipt, operation, node, seq, body, target, container=None):
        require(receipt == {"container_id": (container or self.container).rsplit("/", 1)[1],
                            "operation_id": operation, "sequence": seq, "kind": "save_node",
                            "node_id": node, "length": len(body), "sha256": digest(body),
                            "target": target}, "SAVE receipt does not exactly identify original content")

    def send_without_reading(self, method, target, fields, body, operation):
        wire = (f"{method} {target} HTTP/1.1\r\nHost: {HOST}:{PORT}\r\n"
                f"Authorization: {self.authorization}\r\n"
                + "".join(f"{k}: {v}\r\n" for k, v in fields.items()) + "\r\n").encode("ascii")
        with socket.create_connection((HOST, PORT), timeout=15) as discarded:
            discarded.sendall(wire + body)
            deadline = time.monotonic() + 40
            while time.monotonic() < deadline:
                status, raw = self.request("GET", self.container + "/operations/" + operation,
                                           (200, 404))
                receipt = json.loads(raw)
                if status == 200:
                    return receipt
                require(receipt == {"state": "not_found"}, "unknown operation state")
                time.sleep(0.2)
        raise RuntimeError("receipt not visible after reply was discarded")

    def physical_container(self, container):
        leaf = container.rsplit("/", 1)[1]
        found = [root / "data" / leaf for root in DATA if (root / "data" / leaf).is_dir()]
        require(len(found) == 1 and not found[0].is_symlink(), "expected one ordinary DATA container")
        return found[0]

    def recycle(self, first_receipt, append_receipt, base, new):
        peer_body = b"independent container on other DATA\n"
        peer_target = {"kind": "first_main", "name": "peer.bin"}
        peer_headers = self.save_headers(self.peer_operation, self.peer_node, peer_body, name=b"peer.bin")
        _, raw = self.request("POST", self.peer + "/branches/main/nodes", 200,
                              headers=peer_headers, body=peer_body)
        self.expect_receipt(json.loads(raw), self.peer_operation, self.peer_node, 1, peer_body,
                            peer_target, self.peer)
        source = self.physical_container(self.container)
        other = self.physical_container(self.peer)
        require(source.stat().st_dev != other.stat().st_dev,
                "the two known containers did not land on the two separate DATA disks")
        self.read_body(self.peer_node, peer_body, self.peer)
        require(self.receipt(self.recycle_operation, 404) == {"state": "not_found"},
                "new recycle operation is not absent")

        headers = {"Content-Length": "0", "Zxs-Operation": self.recycle_operation,
                   "Zxs-Base-Generation": "2"}
        result = self.send_without_reading("DELETE", self.container, headers, b"", self.recycle_operation)
        leaf = self.container.rsplit("/", 1)[1]
        expected = {"container_id": leaf, "operation_id": self.recycle_operation,
                    "kind": "recycle", "original_sequence": 2, "source_cleanup_pending": None}
        require(result == expected, "original recycle lookup differs")

        self.stop()
        self.start()
        require(self.receipt(self.recycle_operation) == expected, "recycle receipt did not survive restart")
        _, raw = self.request("DELETE", self.container, 200, headers=headers)
        require(json.loads(raw) == {**expected, "source_cleanup_pending": False},
                "exact recycle retry did not settle source cleanup")
        require(self.receipt(self.first_operation) == first_receipt
                and self.receipt(self.next_operation) == append_receipt,
                "prior operation receipts changed on the recycle disk")
        self.request("GET", self.container, 404, headers={"Zxs-Detail": "paged"})
        self.request("GET", self.container + "/nodes/" + self.first_node + "/data", 404)
        _, raw = self.request("GET", ROOT, 200, headers={"Zxs-View": "compact"})
        require(json.loads(raw) == {"items": [{"container_id": self.peer.rsplit("/", 1)[1],
                                              "sequence": 1, "name": "peer.bin"}],
                                    "next_after": None},
                "active catalogue did not retain exactly the other context")
        self.read_body(self.peer_node, peer_body, self.peer)
        require(not os.path.lexists(source) and other.is_dir(),
                "source was not removed or the other DATA container disappeared")

        # Archived bytes are intentionally not exposed by the live node endpoint.
        # Read only the exact fixture paths, with no directory scan or fallback.
        archive = RECYCLE / "containers" / (leaf + "-" + self.recycle_operation) / "container"
        require(archive.resolve(strict=True) == archive and archive.stat().st_dev == RECYCLE.stat().st_dev,
                "selected archive is not on the independent recycle disk")
        for seq, operation, original in ((1, self.first_operation, base), (2, self.next_operation, new)):
            payload = archive / f"{seq:016x}-{operation}" / "payload"
            require(payload.resolve(strict=True) == payload and payload.stat().st_dev == RECYCLE.stat().st_dev,
                    "archived payload does not belong to the selected recycle disk")
            with payload.open("rb") as stream:
                require(stream.read(len(original) + 1) == original, "recycled whole body differs")

    def run(self):
        endpoint = self.container + "/branches/main/nodes"
        base = b"baseline \x00\xff\n"
        new = b"whole replacement \n\x01\x00" * 37
        first = self.save_headers(self.first_operation, self.first_node, base)
        append = self.save_headers(self.next_operation, self.next_node, new, self.first_node)
        _, raw = self.request("POST", endpoint, 200, headers=first, body=base)
        receipt = json.loads(raw)
        first_target = {"kind": "first_main", "name": "fixture.bin"}
        self.expect_receipt(receipt, self.first_operation, self.first_node, 1, base, first_target)
        self.read_body(self.first_node, base)
        require(self.receipt(self.next_operation, 404) == {"state": "not_found"},
                "new operation is not absent")

        # Send the complete request but never read the reply. This represents a
        # lost client reply, not power loss. Wait for the *same* operation receipt.
        accepted = self.send_without_reading("POST", endpoint, append, new, self.next_operation)
        target = {"kind": "branch", "branch": "main", "expected_head": self.first_node}
        self.expect_receipt(accepted, self.next_operation, self.next_node, 2, new, target)
        self.stop()
        self.start()
        require(self.receipt(self.next_operation) == accepted, "receipt not retained after restart")
        _, raw = self.request("POST", endpoint, 200, headers=append, body=new)
        require(json.loads(raw) == accepted, "same-operation retry did not return the original receipt")
        _, raw = self.request("GET", self.container, 200, headers={"Zxs-Detail": "paged"})
        detail = json.loads(raw)
        require(detail["sequence"] == 2 and detail["heads"] == [{"branch": "main", "head": self.next_node}],
                "retry changed the sequence or main head")
        self.read_body(self.first_node, base)
        self.read_body(self.next_node, new)
        self.recycle(receipt, accepted, base, new)


def run_workspace_fixture(smoke, password, *, fuse):
    fixture = Path("/opt/zxs-fixture-workspace")
    workspace = fixture / "zxs-workspace"
    implementation = workspace / "tests/qualification/live_r3_loopback.py"
    cert = fixture / "certs"
    require(implementation.resolve(strict=True) == implementation and implementation.stat().st_uid == 0,
            "Workspace fixture source not on its exact root-owned path")
    sys.path[:0] = [str(workspace), str(fixture / "packages"), str(implementation.parent)]
    spec = importlib.util.spec_from_file_location("workspace_live_r3_fixture", implementation)
    require(spec is not None and spec.loader is not None, "Workspace fixture could not be loaded")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    transport = dict(backend_port=PORT, fixture_authorization=smoke.authorization,
                     certfile=cert / "server.pem", keyfile=cert / "server.key",
                     ca_pem=(cert / "ca.pem").read_text(encoding="ascii"))
    module.run_live_r3(**transport)
    if fuse:
        module.run_live_fuse_r3(**transport, fixture_login=ACCOUNT,
                                fixture_password=password.decode("ascii"))


def main():
    if sys.argv[1:] == ["--remount-control"]:
        guard(initialized=True)
        smoke = Smoke(b"unused")  # startup control sends no credentials
        try:
            smoke.start()
        finally:
            smoke.stop()
        print("ZXS-PRODUCT-SMOKE same service became ready after original DATA remount", flush=True)
        return
    require(sys.argv[1:] in ([], ["--workspace-h2"], ["--workspace-fuse"]), "unknown fixture mode")
    guard()
    password = secrets.token_hex(24).encode("ascii")
    issue_setup(password)
    smoke = Smoke(password)
    try:
        smoke.start()
        smoke.run()
        if sys.argv[1:]:
            run_workspace_fixture(smoke, password, fuse=sys.argv[1:] == ["--workspace-fuse"])
    finally:
        smoke.stop()
    print("ZXS-PRODUCT-SMOKE setup, full append, two DATA disks, recycle and exact retries passed", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, subprocess.SubprocessError, ValueError, KeyError) as exc:
        print(f"ZXS-PRODUCT-SMOKE failed: {type(exc).__name__}: {exc}", flush=True)
        raise
