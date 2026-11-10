#!/usr/bin/env python3
"""Run only the delivered ELF in an unrelated directory on Debian 13 minimal."""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import pwd
import secrets
import shutil
import socket
import subprocess
import tempfile
import time
from urllib.parse import quote


HOST_PREPARATION = Path("/opt/zxs-host-preparation.json")
SERVICE_USER = "zxs-engine-test"


def stop(child):
    status = child.poll()
    if status is not None:
        return status == 0
    child.terminate()
    try:
        return child.wait(timeout=10) == 0
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait()
        return False


def free_port():
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        return reservation.getsockname()[1]


class Helper:
    def __init__(self, port):
        self.port = port
        self.csrf = None

    def request(self, path, body=None):
        client = http.client.HTTPConnection("127.0.0.1", self.port, timeout=120)
        headers = {} if body is None else {
            "Content-Type": "application/json", "Origin": f"http://127.0.0.1:{self.port}",
            "x-zxs-helper-csrf": self.csrf,
        }
        try:
            client.request("GET" if body is None else "POST", path,
                           None if body is None else json.dumps(body), headers)
            response = client.getresponse()
            data = response.read(1024 * 1024 + 1)
            if len(data) > 1024 * 1024 or response.status != 200:
                raise RuntimeError(f"helper {path}: HTTP {response.status}: {data[:4096]!r}")
            return json.loads(data)
        finally:
            client.close()


def initialize_storage(helper, evidence, record):
    """Use public helper operations, keeping each exact preview and native receipt."""
    def transition(name, original, confirmation, *, extra=None):
        preview = helper.request(f"/api/initialization/{name}-preview", original)
        request = {"input": original, "request_digest": preview["request_digest"],
                   "confirmation": confirmation}
        if extra:
            request.update(extra)
        action = "create" if name == "administrator" else "prepare"
        result = helper.request(f"/api/initialization/{name}-{action}", request)
        receipt = result["receipt"] if name == "administrator" else result
        if receipt["request_digest"] != preview["request_digest"]:
            raise RuntimeError(f"helper {name} receipt belongs to another preview")
        states = {
            "layout": "new_empty_private_layout_prepared",
            "administrator": "first_management_administrator_durably_created",
            "storage-directories": "original_empty_storage_directories_durably_prepared",
        }
        if result["state"] != states[name]:
            raise RuntimeError(f"helper {name} did not prepare a fresh installation")
        evidence[name] = {"preview": preview, "result": result}
        record.write_text(json.dumps(evidence, indent=2) + "\n")

    transition("layout", {"service_user": SERVICE_USER}, "prepare-private-layout-only")
    password = secrets.token_urlsafe(32)
    transition("administrator", {"service_user": SERVICE_USER, "administrator": "acceptance-admin"},
               "initialize-first-management-administrator", extra={
                   "external_writers_stopped": True, "password": password, "repeat_password": password,
               })
    original = {"service_user": SERVICE_USER, "roots": ["/zxs/data/disks/acceptance-primary"]}
    transition("storage-directories", original, "prepare-first-storage-directories-only",
               extra={"external_writers_stopped": True})
    preview = helper.request("/api/initialization/storage-preview", original)
    result = helper.request("/api/initialization/storage-create", {
        "input": {"service_user": SERVICE_USER, "request_digest": preview["request_digest"]},
        "plan": preview["plan"], "confirmation": "initialize-first-storage-batch",
        "external_writers_stopped": True,
    })
    if (result["format"] != "zxs-helper-first-storage-batch-complete"
            or result["request_digest"] != preview["request_digest"]
            or result["storage_activation_completed"] is not True):
        raise RuntimeError("helper did not complete the original first-storage activation")
    evidence["storage"] = {"preview": preview, "result": result}
    record.write_text(json.dumps(evidence, indent=2) + "\n")


def read_json(port, path):
    client = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
    try:
        client.request("GET", path)
        response = client.getresponse()
        data = response.read(64 * 1024 + 1)
        if len(data) > 64 * 1024:
            raise RuntimeError(f"oversized daemon status: {path}")
        return json.loads(data) if response.status == 200 else None
    finally:
        client.close()


def readiness_snapshot(port):
    management = read_json(port, "/api/management/status")
    if management is None:
        return None
    if (management["management"] != "alive" or management["authentication"] != "ready"
            or management["storage"]["state"] != "ready"
            or management["storage"]["recovering"] is not False
            or management["auxiliary"]["web_assets"] != "ready"):
        return None
    readiness = read_json(port, "/readyz")
    if readiness is not None and readiness["ready"] is True:
        return {"management": management, "readiness": readiness}
    return None


LEASE_BUSY = 'service lease root "/zxs" is busy or cannot be locked'


def daemon_command(binary, port):
    return [str(binary), "--root", "/zxs/data/state",
            "--management-root", "/zxs/data/management",
            "--storage-discovery-root", "/zxs/data/disks",
            "--listen", f"127.0.0.1:{port}"]


def require_lease_rejection(command, **options):
    result = subprocess.run(command, capture_output=True, text=True, timeout=10, **options)
    if result.returncode != 1 or LEASE_BUSY not in result.stderr:
        raise RuntimeError(f"expected lifetime lease rejection: {command!r}: {result!r}")
    return {"exit_code": result.returncode, "diagnostic": result.stderr}


def reject_daemon_while_helper(source, account):
    with tempfile.TemporaryDirectory(prefix="zxs-blocked-daemon-") as directory:
        root = Path(directory)
        binary = root / "zxs"
        shutil.copyfile(source, binary)
        binary.chmod(0o700)
        os.chown(root, account.pw_uid, account.pw_gid)
        os.chown(binary, account.pw_uid, account.pw_gid)
        return require_lease_rejection(
            daemon_command(binary, free_port()), cwd=root,
            env={"PATH": os.environ["PATH"], "HOME": directory, "LC_ALL": "C"},
            user=account.pw_uid, group=account.pw_gid, extra_groups=[])


def reject_helper_while_daemon(source):
    with tempfile.TemporaryDirectory(prefix="zxs-blocked-helper-") as directory:
        helper = Path(directory) / "zxs-helper"
        shutil.copyfile(source, helper)
        helper.chmod(0o700)
        return require_lease_rejection(
            [str(helper), "serve", "--listen", f"127.0.0.1:{free_port()}"], cwd=directory,
            env={"PATH": os.environ["PATH"], "LC_ALL": "C"})


def prepare_installation(source, output, daemon_source):
    if os.geteuid() != 0 or os.environ.get("ZXS_ISOLATED_RELEASE_TEST") != "1":
        raise RuntimeError("requires the isolated root release-test container")
    account = pwd.getpwnam(SERVICE_USER)
    if account.pw_uid == 0 or account.pw_gid == 0:
        raise RuntimeError("runtime service identity must be non-root")
    evidence = {"host": json.loads(HOST_PREPARATION.read_text()),
                "helper_sha256": hashlib.sha256(source.read_bytes()).hexdigest()}
    if evidence["host"]["helper_sha256"] != evidence["helper_sha256"]:
        raise RuntimeError("host preparation used a different delivered helper")
    output.parent.mkdir(parents=True, exist_ok=True)
    record = output.parent / "debian13-single-binary-helper.json"
    record.write_text(json.dumps(evidence, indent=2) + "\n")
    with tempfile.TemporaryDirectory(prefix="zxs-release-helper-") as directory:
        executable = Path(directory) / "zxs-helper"
        shutil.copyfile(source, executable)
        executable.chmod(0o700)
        port = free_port()
        with (output.parent / "debian13-single-binary-helper.log").open("wb") as log:
            child = subprocess.Popen([str(executable), "serve", "--listen", f"127.0.0.1:{port}"],
                                     cwd=directory, env={"PATH": os.environ["PATH"], "LC_ALL": "C"},
                                     stdout=log, stderr=subprocess.STDOUT)
            try:
                helper = Helper(port)
                deadline = time.monotonic() + 30
                while True:
                    if child.poll() is not None:
                        raise RuntimeError("delivered helper exited before serving initialization")
                    try:
                        status = helper.request("/api/status")
                    except OSError:
                        if time.monotonic() >= deadline:
                            raise TimeoutError("delivered helper HTTP readiness")
                        time.sleep(0.1)
                        continue
                    helper.csrf = status["csrf"]
                    break
                initialize_storage(helper, evidence, record)
                if child.poll() is not None:
                    raise RuntimeError("helper exited before lifetime exclusion probe")
                evidence["daemon_rejected_while_helper"] = reject_daemon_while_helper(daemon_source, account)
                if child.poll() is not None:
                    raise RuntimeError("helper exited during lifetime exclusion probe")
                record.write_text(json.dumps(evidence, indent=2) + "\n")
            finally:
                clean_exit = stop(child)
            if not clean_exit:
                raise RuntimeError("delivered helper failed graceful shutdown")
    # The helper is stopped and its private executable removed before normal ZXS starts.
    return account, evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--web", type=Path, required=True, help="validator reference only, never passed to server")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.unlink(missing_ok=True)  # A failed rerun must not retain a prior success receipt.
    release = platform.freedesktop_os_release()
    if (release.get("ID"), release.get("VERSION_ID"), platform.machine()) != ("debian", "13", "x86_64"):
        raise RuntimeError("requires Debian 13 amd64 runtime container")
    if shutil.which("cargo") or shutil.which("cc"):
        raise RuntimeError("runtime evidence must not depend on a compiler")
    manifest = json.loads((args.web / "build-manifest.json").read_text())
    account, preparation = prepare_installation(args.helper, args.output, args.binary)
    with tempfile.TemporaryDirectory(prefix="zxs-single-elf-") as directory:
        root = Path(directory)
        binary = root / "zxs"
        shutil.copyfile(args.binary, binary)
        binary.chmod(0o700)
        os.chown(root, account.pw_uid, account.pw_gid)
        os.chown(binary, account.pw_uid, account.pw_gid)
        port = free_port()
        command = daemon_command(binary, port)
        environment = {"PATH": os.environ["PATH"], "HOME": directory, "LC_ALL": "C"}
        with (root / "server.log").open("wb") as log:
            child = subprocess.Popen(command, cwd=root, env=environment, stdout=log, stderr=subprocess.STDOUT,
                                     user=account.pw_uid, group=account.pw_gid, extra_groups=[])
            try:
                deadline = time.monotonic() + 30
                while True:
                    if child.poll() is not None:
                        raise RuntimeError("standalone ELF exited before serving embedded Web")
                    try:
                        ready = readiness_snapshot(port)
                    except OSError:
                        ready = False
                    if ready:
                        break
                    if time.monotonic() >= deadline:
                        raise TimeoutError("standalone ELF HTTP setup readiness")
                    time.sleep(0.1)
                preparation["helper_rejected_while_daemon"] = reject_helper_while_daemon(args.helper)
                if child.poll() is not None or readiness_snapshot(port) is None:
                    raise RuntimeError("daemon changed readiness during helper exclusion probe")
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
                          "compiler_present": False, "samba_started": False,
                          "service_uid": account.pw_uid, "service_gid": account.pw_gid,
                          "first_storage_preparation": preparation, "cold_start": ready}
            finally:
                clean_exit = stop(child)
                args.output.parent.mkdir(parents=True, exist_ok=True)
                (args.output.parent / "debian13-single-binary.log").write_bytes((root / "server.log").read_bytes())
            if not clean_exit:
                raise RuntimeError("standalone ELF failed graceful shutdown")
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
