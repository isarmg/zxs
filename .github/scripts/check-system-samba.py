#!/usr/bin/env python3
"""Real apt smbd + loadable VFS + real engine, isolated loopback fixture only."""
import argparse
import json
import os
from pathlib import Path
import secrets
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
TEST = "smb_rpc::tests::system_samba_tests::system_samba_real_engine_fixture"


def publish_marker(work, name):
    temporary = work / (name + ".new")
    with temporary.open("xb") as output:
        output.write(b"ok\n")
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(work / name)


def wait_file(path, process, timeout=120):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.is_file():
            return
        if process.poll() is not None:
            raise RuntimeError(f"fixture exited {process.returncode} before {path.name}")
        time.sleep(0.1)
    raise TimeoutError(path)


def run(args):
    if os.geteuid() != 0 or os.environ.get("ZXS_ISOLATED_SAMBA_TEST") != "1":
        raise RuntimeError("requires explicit isolated Debian fixture, never a user host")
    target = json.loads((ROOT / ".github/samba-debian-target.json").read_text())
    for package in ("samba", "samba-common-bin", "smbclient"):
        actual = subprocess.check_output(["dpkg-query", "-W", "-f=${Version}", package], text=True)
        if actual != target["package_version"]:
            raise RuntimeError(f"wrong system Samba package: {package}: {actual}")
    if subprocess.run(["sh", "-c", "command -v cc || command -v cargo"], capture_output=True).returncode == 0:
        raise RuntimeError("runtime acceptance image must not contain compiler or Cargo")
    result = {"package_version": target["package_version"], "vfs_interface": target["vfs_interface"],
              "host": subprocess.check_output(["smbd", "--version"], text=True).strip(), "checks": []}
    with tempfile.TemporaryDirectory(prefix="zxs-apt-samba-") as temp:
        work = Path(temp)
        password = secrets.token_hex(24)
        auth = work / "auth"
        auth.write_text(f"username = root\npassword = {password}\n")
        auth.chmod(0o600)
        bad_auth = work / "bad-auth"
        bad_auth.write_text("username = root\npassword = definitely-wrong\n")
        bad_auth.chmod(0o600)
        environment = {**os.environ, "ZXS_SMB_E2E_DIR": temp, "ZXS_SMB_E2E_USER": "root"}
        for stale in ("LD_LIBRARY_PATH", "ZXS_SAMBA_ROOT", "ZXS_SMB_SDK_SOURCE", "ZXS_SMB_SDK_BUILD"):
            environment.pop(stale, None)
        children = []
        with (work / "engine.log").open("wb") as engine_log, (work / "smbd.log").open("wb") as daemon_log:
            try:
                fixture = subprocess.Popen([str(args.fixture.resolve()), TEST, "--ignored", "--exact", "--nocapture"],
                    env=environment, stdout=engine_log, stderr=subprocess.STDOUT, start_new_session=True)
                children.append(fixture)
                wait_file(work / "ready.json", fixture)
                ready = json.loads((work / "ready.json").read_text())
                for name in ("empty", "private", "state", "cache", "lock", "pid", "ncalrpc"):
                    (work / name).mkdir(mode=0o700)
                config = work / "smb.conf"
                config.write_text(f'''[global]
server role = standalone server
security = user
map to guest = Never
restrict anonymous = 2
interfaces = 127.0.0.1
bind interfaces only = yes
smb ports = 44545
server min protocol = SMB3_00
server signing = mandatory
smb encrypt = required
passdb backend = tdbsam:{work}/passdb.tdb
private dir = {work}/private
state directory = {work}/state
cache directory = {work}/cache
lock directory = {work}/lock
pid directory = {work}/pid
ncalrpc dir = {work}/ncalrpc
log file = {work}/samba-log.%m
load printers = no
disable spoolss = yes
printcap name = /dev/null
[zxs]
path = {work}/empty
vfs objects = {args.module.resolve()}
zxs:socket = {ready['socket']}
zxs:repository = {ready['repository_id']}
valid users = root
guest ok = no
read only = no
smb encrypt = required
oplocks = no
level2 oplocks = no
kernel oplocks = no
durable handles = no
aio read size = 1
aio write size = 1
use sendfile = no
store dos attributes = no
ea support = no
follow symlinks = no
''')
                subprocess.run(["testparm", "--suppress-prompt", str(config)], env=environment,
                               stdout=daemon_log, stderr=subprocess.STDOUT, check=True)
                subprocess.run(["smbpasswd", "-s", "-a", "root", "-c", str(config)],
                    input=f"{password}\n{password}\n", text=True, env=environment,
                    stdout=daemon_log, stderr=subprocess.STDOUT, check=True)
                daemon = subprocess.Popen(["smbd", "--foreground", "--no-process-group", "--debug-stdout", "--configfile", str(config)],
                    env=environment, stdout=daemon_log, stderr=subprocess.STDOUT, start_new_session=True)
                children.append(daemon)
                base = ["smbclient", "//127.0.0.1/zxs", "-p", "44545", "--client-protection=encrypt", "-m", "SMB3"]
                def client(command, *, credential=auth, succeeds=True):
                    completed = subprocess.run(base + (["-A", str(credential)] if credential else ["-N"]) + ["-c", command],
                        env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
                    if (completed.returncode == 0) != succeeds:
                        raise RuntimeError(f"unexpected SMB result for {command}: {completed.returncode}\n{completed.stdout}")
                    if not succeeds and not any(status in completed.stdout for status in
                            ("NT_STATUS_LOGON_FAILURE", "NT_STATUS_ACCESS_DENIED")):
                        raise RuntimeError("SMB rejection lacked an authentication/authorization status: " + completed.stdout)
                    return completed.stdout
                deadline = time.monotonic() + 30
                while True:
                    if daemon.poll() is not None:
                        raise RuntimeError("apt smbd exited before handshake")
                    probe = subprocess.run(base + ["-A", str(auth), "-c", "ls"], env=environment,
                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=5)
                    if probe.returncode == 0:
                        break
                    if time.monotonic() >= deadline:
                        raise RuntimeError("apt smbd/VFS handshake failed:\n" + probe.stdout)
                    time.sleep(0.2)
                result["checks"].append("authenticated-encrypted-signed-handshake")
                client("ls", credential=bad_auth, succeeds=False)
                client("ls", credential=None, succeeds=False)
                client("ls")
                result["checks"].extend(["wrong-password-rejected", "guest-rejected"])
                payload = work / "payload.bin"
                payload.write_bytes(ready["payload"].encode("utf-8"))
                received = work / "received.bin"
                client(f'put "{payload}" upload.txt; get upload.txt "{received}"; rename upload.txt kept.txt')
                if received.read_bytes() != payload.read_bytes():
                    raise RuntimeError("SMB download differs from uploaded bytes")
                publish_marker(work, "uploaded")
                wait_file(work / "revoked", fixture)
                denial = client("ls", succeeds=False)
                if "NT_STATUS_ACCESS_DENIED" not in denial:
                    raise RuntimeError("revoked binding did not produce ACCESS_DENIED: " + denial)
                publish_marker(work, "denied")
                wait_file(work / "enabled", fixture)
                client("del kept.txt")
                publish_marker(work, "done")
                wait_file(work / "verified.json", fixture)
                if fixture.wait(timeout=30) != 0:
                    raise RuntimeError("real engine authoritative validation failed")
                result["engine"] = json.loads((work / "verified.json").read_text())
                result["checks"].extend(["put-get-bytes", "rename", "binding-revocation-rejected", "delete", "authoritative-engine-state"])
            finally:
                for child in reversed(children):
                    if child.poll() is None:
                        os.killpg(child.pid, signal.SIGTERM)
                        try:
                            child.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            os.killpg(child.pid, signal.SIGKILL)
                            child.wait()
                args.output.parent.mkdir(parents=True, exist_ok=True)
                # Keep diagnostic output, never synthetic passwords/auth files.
                for name in ("engine.log", "smbd.log"):
                    text = (work / name).read_text(errors="replace").replace(password, "[redacted]")
                    (args.output.parent / name).write_text(text)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--module", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    run(parser.parse_args())
