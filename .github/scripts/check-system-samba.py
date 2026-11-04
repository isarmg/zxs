#!/usr/bin/env python3
"""Real apt smbd + loadable VFS + real engine, isolated loopback fixture only."""
import argparse
import hashlib
import json
import os
import re
import pwd
import stat
from pathlib import Path
import secrets
import shlex
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


def nonroot_io_evidence(logs, uid):
    """Bind each I/O record to its own debug-uid header, including rotation."""
    evidence = {}
    for log in logs:
        # A rotation boundary must never pair one file's header with another
        # file's body. Each complete request/header pair stands on its own.
        header = None
        for line in log.splitlines():
            if line.startswith("["):
                header = line
            elif header is not None:
                for operation, message in (("read-existing", "smbd_smb2_read: report.txt -"),
                                           ("write-new", "smbd_smb2_write: upload.txt -")):
                    if message in line and re.search(rf"effective\({uid}, [0-9]+\)", header):
                        evidence[operation] = {"header": header, "request": line.strip()}
                header = None
    if set(evidence) != {"read-existing", "write-new"}:
        raise RuntimeError("missing non-root Samba read/write request identity evidence")
    return evidence


def assert_owner_mode(path, uid, mode):
    metadata = path.lstat()
    if metadata.st_uid != uid or stat.S_IMODE(metadata.st_mode) != mode:
        raise RuntimeError(f"unexpected isolated fixture ownership/mode: {path}")


def run(args):
    args.output.unlink(missing_ok=True)  # A failed rerun must not retain a prior success receipt.
    if os.geteuid() != 0 or os.environ.get("ZXS_ISOLATED_SAMBA_TEST") != "1":
        raise RuntimeError("requires explicit isolated Debian fixture, never a user host")
    engine_user = pwd.getpwnam("zxs-engine-test")
    smb_user = pwd.getpwnam("zxs-smb-test")
    if 0 in (engine_user.pw_uid, smb_user.pw_uid) or engine_user.pw_uid == smb_user.pw_uid:
        raise RuntimeError("engine and Samba accounts must be distinct non-root identities")
    target = json.loads((ROOT / ".github/samba-debian-target.json").read_text())
    for package in ("samba", "samba-common-bin", "smbclient"):
        actual = subprocess.check_output(["dpkg-query", "-W", "-f=${Version}", package], text=True)
        if actual != target["package_version"]:
            raise RuntimeError(f"wrong system Samba package: {package}: {actual}")
    if subprocess.run(["sh", "-c", "command -v cc || command -v cargo"], capture_output=True).returncode == 0:
        raise RuntimeError("runtime acceptance image must not contain compiler or Cargo")
    # Samba's supported loader accepts a module name, never an arbitrary path.
    # CI mounts this exact artifact read-only into the isolated distro module directory.
    build_options = subprocess.check_output(["smbd", "-b"], text=True)
    module_dirs = re.findall(r"^\s*MODULESDIR:\s+(/[^\s]+)$", build_options, re.MULTILINE)
    if len(module_dirs) != 1:
        raise RuntimeError("apt smbd must identify exactly one absolute module directory")
    installed = Path(module_dirs[0]) / "vfs/zxs.so"
    module_sha = hashlib.sha256(args.module.read_bytes()).hexdigest()
    if hashlib.sha256(installed.read_bytes()).hexdigest() != module_sha:
        raise RuntimeError("isolated installed VFS differs from the candidate artifact")
    result = {"module_sha256": module_sha, "package_version": target["package_version"], "vfs_interface": target["vfs_interface"],
              "host": subprocess.check_output(["smbd", "--version"], text=True).strip(), "checks": []}
    with tempfile.TemporaryDirectory(prefix="zxs-apt-samba-") as temp:
        work = Path(temp)
        # Public export ancestry is root-owned; engine state remains private.
        work.chmod(0o755)
        engine_work = work / "engine"
        engine_work.mkdir(mode=0o700)
        os.chown(engine_work, engine_user.pw_uid, engine_user.pw_gid)
        password = secrets.token_hex(24)
        auth = work / "auth"
        auth.write_text(f"username = {smb_user.pw_name}\npassword = {password}\n")
        auth.chmod(0o600)
        bad_auth = work / "bad-auth"
        bad_auth.write_text(f"username = {smb_user.pw_name}\npassword = definitely-wrong\n")
        bad_auth.chmod(0o600)
        environment = {**os.environ, "ZXS_SMB_E2E_DIR": str(engine_work), "ZXS_SMB_E2E_USER": smb_user.pw_name}
        for stale in ("LD_LIBRARY_PATH", "ZXS_SAMBA_ROOT", "ZXS_SMB_SDK_SOURCE", "ZXS_SMB_SDK_BUILD"):
            environment.pop(stale, None)
        children = []
        with (work / "engine.log").open("wb") as engine_log, (work / "smbd.log").open("wb") as daemon_log:
            try:
                fixture = subprocess.Popen([str(args.fixture.resolve()), TEST, "--ignored", "--exact", "--nocapture"],
                    env=environment, stdout=engine_log, stderr=subprocess.STDOUT, start_new_session=True,
                    user=engine_user.pw_uid, group=engine_user.pw_gid, extra_groups=[])
                children.append(fixture)
                wait_file(engine_work / "ready.json", fixture)
                ready = json.loads((engine_work / "ready.json").read_text())
                for name in ("empty", "private", "state", "cache", "lock", "pid", "ncalrpc"):
                    (work / name).mkdir(mode=0o700)
                (work / "empty").chmod(0o555)
                assert_owner_mode(work, 0, 0o755)
                assert_owner_mode(work / "empty", 0, 0o555)
                assert_owner_mode(engine_work, engine_user.pw_uid, 0o700)
                assert_owner_mode(Path(ready["socket"]).parent, engine_user.pw_uid, 0o700)
                assert_owner_mode(Path(ready["socket"]), engine_user.pw_uid, 0o600)
                if ready["engine_uid"] != engine_user.pw_uid or ready["allowed_peer_uid"] != 0:
                    raise RuntimeError("engine identity/peer admission differs from deployment model")
                result["identity"] = {"engine_uid": engine_user.pw_uid, "samba_account": smb_user.pw_name,
                                      "samba_uid": smb_user.pw_uid, "allowed_peer_uid": 0,
                                      "socket_mode": "0600", "socket_directory_mode": "0700",
                                      "export_owner_uid": 0, "export_mode": "0555"}
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
log file = {work}/samba-identity.log
# Isolated synthetic fixture only: distinguish parent ACL checks from VFS/RPC failures.
log level = 10
max log size = 4096
debug uid = yes
debug pid = yes
load printers = no
disable spoolss = yes
printcap name = /dev/null
[zxs]
path = {work}/empty
vfs objects = zxs
zxs:socket = {ready['socket']}
zxs:repository = {ready['repository_id']}
valid users = {smb_user.pw_name}
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
[forced]
copy = zxs
force user = {smb_user.pw_name}
''')
                subprocess.run(["testparm", "--suppress-prompt", str(config)], env=environment,
                               stdout=daemon_log, stderr=subprocess.STDOUT, check=True)
                subprocess.run(["smbpasswd", "-s", "-a", smb_user.pw_name, "-c", str(config)],
                    input=f"{password}\n{password}\n", text=True, env=environment,
                    stdout=daemon_log, stderr=subprocess.STDOUT, check=True)
                daemon = subprocess.Popen(["smbd", "--foreground", "--no-process-group", "--configfile", str(config)],
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
                forced = subprocess.run([base[0], "//127.0.0.1/forced", *base[2:], "-A", str(auth), "-c", "ls"],
                    env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
                if forced.returncode == 0 or "NT_STATUS_UNSUCCESSFUL" not in forced.stdout:
                    raise RuntimeError("force-user tree was not rejected by connect hook: " + forced.stdout)
                result["checks"].append("force-user-rejected")
                result["checks"].extend(["wrong-password-rejected", "guest-rejected"])
                original = work / "original.bin"
                client(f'get report.txt "{original}"')
                if original.read_bytes() != b"old":
                    raise RuntimeError("existing engine body read via Samba differs")
                result["checks"].append("read-existing-file")
                payload = work / "payload.bin"
                payload.write_bytes(ready["payload"].encode("utf-8"))
                received = work / "received.bin"
                client(f'put "{payload}" upload.txt; get upload.txt "{received}"; rename upload.txt kept.txt')
                if received.read_bytes() != payload.read_bytes():
                    raise RuntimeError("SMB download differs from uploaded bytes")
                # Establish this tree before revocation. smbclient executes its local
                # shell marker only after authentication/tree connect, and executes
                # later stdin commands serially on that same connection.
                session_ready = work / "existing-tree-ready"
                session = subprocess.Popen(base + ["-A", str(auth)], env=environment,
                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                    text=True, start_new_session=True)
                children.append(session)
                session.stdin.write("!touch " + shlex.quote(str(session_ready)) + "\n")
                session.stdin.flush()
                wait_file(session_ready, session, timeout=30)
                publish_marker(engine_work, "uploaded")
                wait_file(engine_work / "revoked", fixture)
                # No earlier remote command ran on this session, so the only
                # possible ACCESS_DENIED below belongs to this post-revoke ls.
                denial, _ = session.communicate("ls\nquit\n", timeout=30)
                if session.returncode != 0:
                    raise RuntimeError(f"existing SMB session did not exit cleanly: {session.returncode}: {denial}")
                if "NT_STATUS_ACCESS_DENIED" not in denial:
                    raise RuntimeError("revoked existing tree did not produce ACCESS_DENIED: " + denial)
                # Samba itself collapses every connect-hook failure to UNSUCCESSFUL.
                # Keep that new-tree denial separately; it is not permission proof.
                new_tree = subprocess.run(base + ["-A", str(auth), "-c", "ls"], env=environment,
                    text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
                if new_tree.returncode == 0 or "NT_STATUS_UNSUCCESSFUL" not in new_tree.stdout:
                    raise RuntimeError("revoked new tree was not rejected by connect hook: " + new_tree.stdout)
                result["revoked_existing_tree"] = {"authority_revoked": True, "returncode": session.returncode, "output": denial}
                result["revoked_new_tree"] = new_tree.stdout
                publish_marker(engine_work, "denied")
                wait_file(engine_work / "enabled", fixture)
                client("del kept.txt")
                publish_marker(engine_work, "done")
                wait_file(engine_work / "verified.json", fixture)
                if fixture.wait(timeout=30) != 0:
                    raise RuntimeError("real engine authoritative validation failed")
                identity_logs = [work / "samba-identity.log.old", work / "samba-identity.log"]
                result["nonroot_io"] = nonroot_io_evidence(
                    [path.read_text() for path in identity_logs if path.is_file()], smb_user.pw_uid)
                if list((work / "empty").iterdir()):
                    raise RuntimeError("VFS wrote into host export instead of the engine")
                result["checks"].append("nonroot-engine-and-samba-io-private-socket")
                result["engine"] = json.loads((engine_work / "verified.json").read_text())
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
                for name in ("engine.log", "smbd.log", "samba-identity.log.old", "samba-identity.log"):
                    if not (work / name).exists():
                        continue
                    text = (work / name).read_text(errors="replace").replace(password, "[redacted]")
                    (args.output.parent / name).write_text(text)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--module", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    run(parser.parse_args())
