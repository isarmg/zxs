#!/usr/bin/env python3
"""Narrow same-guest product smoke; only for the disposable Actions guest."""

import base64
import errno
import hashlib
import http.client
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


def guard():
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
                and p.stat().st_mode & 0o777 == 0o700 and not any(p.iterdir())
                for p in roots), "fixture roots are not original empty private directories")
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
        self.server = subprocess.Popen([SERVER, "--listen", f"{HOST}:{PORT}"])
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

    def save_headers(self, operation, node, body, parent=None):
        fields = {"Content-Type": "application/octet-stream", "Content-Length": str(len(body)),
                  "Zxs-Operation": operation, "Zxs-Node": node, "Zxs-Sha256": digest(body),
                  "Zxs-Base-Node": parent or "0"}
        if parent is None:
            fields["Zxs-Initial-Name"] = b"fixture.bin".hex()
        return fields

    def receipt(self, operation, expected=200):
        _, raw = self.request("GET", self.container + "/operations/" + operation, expected)
        return json.loads(raw)

    def read_body(self, node, expected):
        headers, body = self.request("GET", self.container + "/nodes/" + node + "/data", 200)
        require(headers.get_all("Zxs-Sha256") == [digest(expected)] and body == expected,
                "committed body or digest differs")

    def expect_receipt(self, receipt, operation, node, seq, body, target):
        require(receipt == {"container_id": self.container.rsplit("/", 1)[1],
                            "operation_id": operation, "sequence": seq, "kind": "save_node",
                            "node_id": node, "length": len(body), "sha256": digest(body),
                            "target": target}, "SAVE receipt does not exactly identify original content")

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
        wire = (f"POST {endpoint} HTTP/1.1\r\nHost: {HOST}:{PORT}\r\n"
                f"Authorization: {self.authorization}\r\n"
                + "".join(f"{k}: {v}\r\n" for k, v in append.items()) + "\r\n").encode("ascii")
        with socket.create_connection((HOST, PORT), timeout=15) as discarded:
            discarded.sendall(wire + new)
            deadline = time.monotonic() + 40
            while time.monotonic() < deadline:
                status, raw = self.request("GET", self.container + "/operations/" + self.next_operation,
                                           (200, 404))
                if status == 200:
                    accepted = json.loads(raw)
                    break
                require(json.loads(raw) == {"state": "not_found"}, "unknown operation state")
                time.sleep(0.2)
            else:
                raise RuntimeError("receipt not visible after reply was discarded")
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


def main():
    guard()
    password = secrets.token_hex(24).encode("ascii")
    issue_setup(password)
    smoke = Smoke(password)
    try:
        smoke.start()
        smoke.run()
    finally:
        smoke.stop()
    print("ZXS-PRODUCT-SMOKE setup, full append, lost reply, restart and exact retry passed", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, subprocess.SubprocessError, ValueError, KeyError) as exc:
        print(f"ZXS-PRODUCT-SMOKE failed: {type(exc).__name__}: {exc}", flush=True)
        raise
