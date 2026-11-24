#!/usr/bin/env python3
"""The ordinary-user half of the isolated, two-original installed H2 fixture."""
import base64
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
import errno
import hashlib
import http.client
import os
from pathlib import Path
import pty
import re
import resource
import secrets
import select
import signal
import sqlite3
import ssl
import stat
import subprocess
import sys
import termios
import time

from h2_relay import BACKEND, HOST, FixtureFailure, Step, json_exact, listener, require, serve_one, tls_context

UID = 23100
ACCOUNT = "fixture"
ROOT = "/zxs/v1/containers"
MARKER = Path("/run/zxs-core-qualification-fixture")
CONTROL = Path("/run/zxs-core-qualification-control")
SOURCE = Path("/opt/zxs-core-qualification")
CLIENT = Path("/mnt/zxs-core-qualification-client")
SYSTEM = Path("/var/lib/zxs-acceptance-system")
RECYCLE = Path("/mnt/zxs-acceptance-recycle")
DATA = (Path("/mnt/zxs-acceptance-data-a"), Path("/mnt/zxs-acceptance-data-b"))
STATE = Path("/zxs/state/headless")
OLD = SOURCE / "old" / "bin" / "zxs-client"
NEW = SOURCE / "new" / "bin" / "zxs-client"
TLS = SOURCE / "fixture-tls"
HELPER = Path("/zxs/bin/helper")
SERVER = Path("/zxs/bin/zxs")
CA_BUNDLE = Path("/etc/ssl/certs/ca-certificates.crt")
# The pinned openssl-probe Linux order chooses this Ubuntu bundle first.
CA_DIRS = (CA_BUNDLE.parent, Path("/etc/pki/tls/certs"), Path("/etc/security/certificates"))
CA_ISSUING = Path("/usr/local/share/ca-certificates/zxs-core-fixture-issuing.crt")
CA_UNRELATED = Path("/usr/local/share/ca-certificates/zxs-core-fixture-unrelated.crt")
CERTIFICATE = re.compile(rb"-----BEGIN CERTIFICATE-----\r?\n(?:[A-Za-z0-9+/=]+\r?\n)+-----END CERTIFICATE-----")
BASIC_PASSWORD_PROMPT = "ZXS 口令（不回显）：".encode()
RECEIPT_PASSWORD_PROMPT = "ZXS 回执会话口令（不回显）：".encode()
PASSWORD_PROMPTS = (BASIC_PASSWORD_PROMPT, RECEIPT_PASSWORD_PROMPT)
NO_H2 = b"the same authenticated TLS channel did not negotiate HTTP/2"
PREPARE_PROMPT = "输入 prepare 只封存原号，不上传；其他内容取消：".encode()
TIMEOUT = 42
ID = re.compile("[0-9a-f]{32}", re.ASCII)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def ca_der_digests(path):
    limit = 2 * 1024 * 1024
    with path.open("rb") as file:
        raw = file.read(limit + 1)
    require(len(raw) <= limit, "native CA input exceeded its bound")
    certificates = CERTIFICATE.findall(raw)
    require(len(certificates) <= 512
            and len(certificates) == raw.count(b"-----BEGIN CERTIFICATE-----")
            == raw.count(b"-----END CERTIFICATE-----"), "unexpected PEM in native CA input")
    return {hashlib.sha256(ssl.PEM_cert_to_DER_cert(pem.decode("ascii"))).digest()
            for pem in certificates}


def native_ca_set():
    # Independent PEM -> DER observer. Compare its original digests to both
    # actual installed clients' original saved trust below.
    bundle = ca_der_digests(CA_BUNDLE)
    require(bundle, "guest lacked its native CA bundle")
    roots = set(bundle)
    info = CA_BUNDLE.stat()
    seen = {(info.st_dev, info.st_ino)}
    for directory in CA_DIRS:
        if directory.is_dir():
            for index, path in enumerate(directory.iterdir()):
                require(index < 1024, "native CA directory exceeded its bound")
                try:
                    info = path.stat()
                except FileNotFoundError:
                    continue  # OpenSSL's root directory can contain dangling aliases.
                identity = (info.st_dev, info.st_ino)
                if stat.S_ISREG(info.st_mode) and identity not in seen:
                    roots.update(ca_der_digests(path))
                    seen.add(identity)
    return roots, bundle


def one_anchor(path):
    digests = ca_der_digests(path)
    require(len(digests) == 1, "synthetic guest CA is not one exact DER certificate")
    return next(iter(digests))


def private_directory(path):
    info = path.lstat()
    return (stat.S_ISDIR(info.st_mode) and info.st_uid == UID and info.st_gid == UID
            and stat.S_IMODE(info.st_mode) == 0o700 and path.resolve(strict=True) == path)


def guard():
    require(os.getuid() == os.geteuid() == os.getgid() == os.getegid() == UID, "not guest UID")
    require(set(os.listdir("/sys/class/net")) == {"lo"}, "guest has an unexpected interface")
    marker = MARKER.lstat()
    require(stat.S_ISREG(marker.st_mode) and marker.st_uid == 0
            and stat.S_IMODE(marker.st_mode) == 0o444
            and MARKER.read_bytes() == b"zxs-core-no-nic-qualification-v1\n", "no guest custody marker")
    require(re.search(r"^CapEff:\s*0+$", Path("/proc/thread-self/status").read_text(), re.M),
            "ordinary guest must not hold capabilities")
    root_dirs = (CLIENT, SYSTEM, RECYCLE, *DATA, STATE)
    require(all(private_directory(p) and not any(p.iterdir()) for p in root_dirs), "nonempty fixture root")
    require(len({p.stat().st_dev for p in (CLIENT, SYSTEM, RECYCLE, *DATA)}) == 5, "non-distinct fixture disks")
    mounts = {
        fields[4]: fields
        for line in Path("/proc/thread-self/mountinfo").read_text().splitlines()
        if len(fields := line.split()) >= 10
    }
    for p in (CLIENT, RECYCLE, *DATA):
        row = mounts.get(str(p))
        require(row is not None and row[3] == "/" and row[row.index("-") + 1] == "ext4"
                and "rw" in row[5].split(","), "fixture data is not exact writable ext4")
    for path in (OLD, NEW, HELPER, SERVER):
        for p in (path, *path.parents):
            m = p.lstat()
            require(m.st_uid == 0 and not m.st_mode & 0o022 and not p.is_symlink(),
                    "fixture executable is not a protected real path")
        require(path.is_file() and os.access(path, os.X_OK), "missing installed fixture binary")
    require(OLD.read_bytes() != NEW.read_bytes(), "old and candidate binaries are not distinct")
    require(not any((SOURCE / sibling).exists() for sibling in ("zxs", "helper", "zxs-workspace")),
            "client fixture contains sibling product source")
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    os.umask(0o077)


def safe_env():
    return dict(PATH="/usr/bin:/bin", LC_ALL="C.UTF-8", TZ="UTC", HOME=str(CLIENT / "home"))


def pty_run(name, exe, args, choices, *, connected=None, rejected=False, watched=(), seconds=TIMEOUT):
    # Fork before starting the relay's worker thread. Secrets are inserted through
    # the private terminal only, never argv, env or output/logging.
    pid, master = pty.fork()
    if pid == 0:
        try:
            # Bash starts the guest worker asynchronously and ignores INT and QUIT.
            for sig in (signal.SIGINT, signal.SIGQUIT, signal.SIGPIPE, signal.SIGXFSZ):
                signal.signal(sig, signal.SIG_DFL)
            # The prompt can be printed before rpassword changes the terminal.
            # Disable echo from the start, including for Helper's two prompts.
            settings = termios.tcgetattr(0)
            settings[3] &= ~(termios.ECHO | termios.ECHONL)
            termios.tcsetattr(0, termios.TCSANOW, settings)
            os.chdir(CLIENT / "cwd")
            os.execve(exe, [str(exe), *map(str, args)], safe_env())
        finally:
            os._exit(127)
    code = None
    data = b""
    cursor = count = 0
    closed = False
    try:
        with ThreadPoolExecutor(max_workers=1) as pool:
            task = pool.submit(connected) if connected is not None else None
            end = time.monotonic() + seconds
            while not closed or code is None:
                remaining = end - time.monotonic()
                require(remaining > 0, name + " process timeout")
                if code is None:
                    child, status = os.waitpid(pid, os.WNOHANG)
                    if child:
                        code = os.waitstatus_to_exitcode(status)
                if closed:
                    time.sleep(min(0.025, remaining))
                    continue
                if not select.select([master], [], [], min(0.15, remaining))[0]:
                    continue
                try:
                    block = os.read(master, 4096)
                except OSError as exc:
                    if exc.errno != errno.EIO:
                        raise
                    block = b""
                if not block:
                    closed = True
                    continue
                data += block
                require(len(data) <= 32 * 1024, name + " terminal bound")
                if rejected:
                    require(not any(prompt in data for prompt in PASSWORD_PROMPTS),
                            name + " reached a credential prompt")
                while count < len(choices):
                    marker, answer = choices[count]
                    n = data.find(marker, cursor)
                    if n < 0:
                        break
                    cursor = n + len(marker)
                    require(code is None and os.write(master, answer + b"\n") == len(answer) + 1,
                            name + " short terminal response")
                    count += 1
            wire = task.result(timeout=TIMEOUT) if task is not None else (0, 0)
        require(count == len(choices), name + " did not reach all expected confirmations")
        require(code == (1 if rejected else 0), name + " returned unexpected exit status")
        require(not watched or not select.select(watched, [], [], 0)[0], name + " opened an extra TCP connection")
        return data, wire
    finally:
        os.close(master)
        if code is None:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.waitpid(pid, 0)


def setup(password):
    disks = [SYSTEM, RECYCLE, *DATA]
    check = subprocess.run([HELPER, "setup-headless", "check", *disks], env=safe_env(),
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    require(check.returncode == 0, "actual Helper setup check failed")
    results, _ = pty_run("setup", HELPER, ["setup-headless", "new", *disks, ACCOUNT], (
        (b"Type SETUP to approve: ", b"SETUP"),
        (b"Password: ", password),
        (b"Repeat password: ", password),
    ), seconds=90)
    require(b"Setup confirmed:" in results, "actual Helper did not confirm setup")


def ready():
    deadline = time.monotonic() + TIMEOUT
    while time.monotonic() < deadline:
        conn = http.client.HTTPConnection(HOST, BACKEND, timeout=1)
        try:
            conn.request("GET", ROOT, headers={"Zxs-View": "compact", "Connection": "close"})
            response = conn.getresponse()
            require(response.status == 401 and response.read(1) == b"", "wrong backend readiness state")
            return
        except (ConnectionRefusedError, ConnectionResetError, TimeoutError):
            time.sleep(0.1)
        finally:
            conn.close()
    raise RuntimeError("backend readiness timed out")


def read_database(state, query, *values):
    db = state / "workspace.sqlite"
    info = db.lstat()
    require(stat.S_ISREG(info.st_mode) and stat.S_IMODE(info.st_mode) == 0o600
            and info.st_uid == UID and info.st_nlink == 1, "unqualified fixture DB")
    require(not any((state / ("workspace.sqlite" + s)).exists()
                    for s in ("-journal", "-wal", "-shm")), "unexpected fixture journal")
    with db.open("rb") as handle:
        header = handle.read(100)
    require(len(header) == 100 and header[:16] == b"SQLite format 3\x00" and header[18:20] == b"\x01\x01"
            and int.from_bytes(header[60:64], "big") == 1
            and int.from_bytes(header[68:72], "big") == 0x5A584331, "not expected fixture rollback DB")
    conn = sqlite3.connect(db.as_uri() + "?mode=ro", uri=True)
    try:
        return conn.execute(query, values).fetchall()
    finally:
        conn.close()


def profile(state, *, trust=None):
    [(str_row,)] = read_database(state, "SELECT record FROM profile WHERE id=1")
    raw = str_row.encode()
    p = json_exact(raw)
    require(set(p) == {"origin", "account", "trust"} and p["account"] == ACCOUNT
            and len(p["trust"]) == 64, "invalid original enrolled identity")
    if trust is not None:
        require(p["trust"] == trust, "old enrollment disagreed with the decoded native CA set")
    return sha(raw)


@dataclass(frozen=True)
class Original:
    state: Path
    file: Path
    body: bytes
    later: bytes
    cid: str
    op: str
    node: str
    target: dict
    original_profile: str

    @property
    def receipt_path(self):
        return f"{ROOT}/{self.cid}/operations/{self.op}"

    def receipt(self):
        return dict(kind="save_node", container_id=self.cid, operation_id=self.op,
                    node_id=self.node, sequence=1, length=len(self.body), sha256=sha(self.body),
                    target=self.target)


def network(name, exe, args, choices, wire, context, authority, steps, basic,
            *, refused_tls=None):
    data, counts = pty_run(name, exe, args, choices,
                           connected=lambda: serve_one(wire, context, authority, steps, basic,
                                                       refused_tls=refused_tls),
                           rejected=refused_tls is not None, watched=(wire,))
    require(counts == (len(steps), sum(x.lose_reply for x in steps)), name + " unexpected wire counts")
    if refused_tls == "no-h2":
        require(NO_H2 in data, "client did not report the verified no-HTTP/2 outcome")


def enroll_and_leave_original(lane, wire, context, authority, origin, password, basic, trust):
    state = CLIENT / f"private-{lane}"
    edit = CLIENT / f"files-{lane}"
    state.mkdir(0o700)
    edit.mkdir(0o700)
    file = edit / "synthetic.bin"
    same = (b"enrolled-complete-body-with-binary\x00\xff-" + lane.encode()) * 19
    later = (b"newer-normal-editor-change\x00\x10-" + lane.encode()) * 13
    file.write_bytes(same)
    login = ((BASIC_PASSWORD_PROMPT, password),)
    network("enrol-old", OLD, [origin, ACCOUNT, state, "init"], login, wire, context, authority,
            [Step("GET", ROOT, 200, fields={"zxs-view": "compact"})], basic)
    pty_run("prepare-old", OLD, [state, "prepare", file, "first", f"fixture-{lane}"],
            [(PREPARE_PROMPT, b"prepare")], watched=(wire,))
    [(record, phase)] = read_database(state, "SELECT record,phase FROM original")
    value = json_exact(record)
    intent = value["intent"]
    c, o, n = (intent[k] for k in ("container_id", "operation_id", "node_id"))
    target = dict(kind="first_main", name=f"fixture-{lane}")
    require(all(ID.fullmatch(x) and x != "0" * 32 for x in (c, o, n))
            and phase == "prepared" and value["path"] == str(file) and intent["target"] == target
            and intent["body"] == {"length": len(same), "sha256": sha(same)}, "bad original intent")
    orig = Original(state, file, same, later, c, o, n, target, profile(state, trust=trust))
    absent = Step("GET", orig.receipt_path, 404, response_body=b'{"state":"not_found"}')
    save = Step("POST", f"{ROOT}/{c}/branches/main/nodes", 200, fields={
        "content-type": "application/octet-stream", "zxs-base-node": "0",
        "zxs-initial-name": target["name"].encode().hex(), "zxs-operation": o, "zxs-node": n,
        "zxs-sha256": sha(same),
    }, request_body=same, response_json=orig.receipt(), lose_reply=True)
    approval = (f"输入 send {o} 确认；其他内容取消：".encode(), f"send {o}".encode())
    network("lose-old-reply", OLD, [origin, ACCOUNT, state, "send", o], (*login, approval),
            wire, context, authority, [absent, absent, save], basic)
    require(read_database(state, "SELECT phase,receipt FROM original WHERE op=?", o)
            == [("attempted", None)], "old client did not retain unresolved original")
    temporary = edit / "editor.tmp"
    temporary.write_bytes(later)
    os.replace(temporary, file)
    return orig


def guest_phase(name):
    (CONTROL / "request" / name).mkdir(0o700)
    ready_marker = CONTROL / "ready" / name
    end = time.monotonic() + 240
    while time.monotonic() < end:
        try:
            m = ready_marker.lstat()
            require(stat.S_ISREG(m.st_mode) and m.st_uid == 0 and stat.S_IMODE(m.st_mode) == 0o444
                    and ready_marker.read_bytes() == b"ok\n", "bad root phase marker")
            return
        except FileNotFoundError:
            time.sleep(0.1)
    raise RuntimeError("guest root phase timed out")


def confirm_old(state, wire):
    require(profile(state.state) == state.original_profile, "stored principal row changed")
    require(read_database(state.state, "SELECT phase,receipt FROM original WHERE op=?", state.op)
            == [("confirmed", "1")], "cold original receipt not recorded")
    [(raw,)] = read_database(state.state, "SELECT record FROM basis WHERE path=?", str(state.file))
    require(raw and json_exact(raw) == {
        "container": state.cid, "branch": "main", "node": state.node, "seen_sequence": 1,
        "body": {"length": len(state.body), "sha256": sha(state.body)},
    }, "cold original basis was not exactly advanced")
    require(state.file.read_bytes() == state.later, "cold open overwrote newer edit")
    result, _ = pty_run("dirty", NEW, [state.state, "dirty", state.file], [], watched=(wire,))
    require("本机仍有未提交修改".encode() in result, "post-recovery local edit not marked dirty")


def run():
    guard()
    for p in ("home", "cwd"):
        (CLIENT / p).mkdir(0o700)
    password = secrets.token_hex(24).encode("ascii")
    basic = "Basic " + base64.b64encode(ACCOUNT.encode() + b":" + password).decode("ascii")
    setup(password)
    server = subprocess.Popen([SERVER, "--listen", f"{HOST}:{BACKEND}"], cwd=CLIENT / "cwd",
                              env=safe_env(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        ready()
        with listener() as wire:
            authority = f"{HOST}:{wire.getsockname()[1]}"
            origin = f"https://{authority}"
            context = tls_context(TLS / "server.pem", TLS / "server.key")
            starting_roots, starting_bundle = native_ca_set()
            issuing = one_anchor(CA_ISSUING)
            require(issuing in starting_bundle, "original issuing CA missing from native bundle")
            trust = sha(b"zxs-core/native-roots/v1\x00" + b"".join(sorted(starting_roots)))
            a = enroll_and_leave_original("receipt", wire, context, authority, origin, password, basic, trust)
            b = enroll_and_leave_original("basic", wire, context, authority, origin, password, basic, trust)
            require(a.cid != b.cid and a.op != b.op, "cold lanes shared a single original")
            print("ZXS-CORE-INSTALLED-H2 old installs committed two original replies intentionally withheld", flush=True)

            guest_phase("add-unrelated")
            newer_roots, newer_bundle = native_ca_set()
            unrelated = one_anchor(CA_UNRELATED)
            require(unrelated not in starting_roots and newer_roots == starting_roots | {unrelated}
                    and newer_bundle == starting_bundle | {unrelated}, "wrong decoded native CA addition")
            receipt_args = ["--receipt-session", origin, ACCOUNT, a.state, "status", a.op]
            basic_args = [origin, ACCOUNT, b.state, "reconcile"]
            receipt_login = ((RECEIPT_PASSWORD_PROMPT, password),)
            basic_login = ((BASIC_PASSWORD_PROMPT, password),)
            network("cold-receipt", NEW, receipt_args, receipt_login, wire, context, authority, [
                Step("POST", "/zxs/v1/sessions", 200, fields={"content-length": "0"}, mint=True),
                Step("GET", a.receipt_path, 200, auth="bearer", response_json=a.receipt()),
            ], basic)
            network("cold-basic", NEW, basic_args, basic_login, wire, context, authority, [
                Step("GET", b.receipt_path, 200, response_json=b.receipt()),
            ], basic)
            confirm_old(a, wire)
            confirm_old(b, wire)
            print("ZXS-CORE-INSTALLED-H2 fresh unrelated-root receipt and Basic each read exact original only", flush=True)

            dst = b.file.with_name("backend-readback.bin")
            detail = Step("GET", f"{ROOT}/{b.cid}", 200, fields={"zxs-detail": "paged"})
            network("readback", NEW, [origin, ACCOUNT, b.state, "checkout", b.cid, "main", dst],
                    basic_login, wire, context, authority, [
                detail,
                Step("GET", f"{ROOT}/{b.cid}/branches/main/nodes", 200,
                     fields={"zxs-history-head": b.node}),
                detail,
                Step("GET", f"{ROOT}/{b.cid}/nodes/{b.node}/data", 200, response_body=b.body),
            ], basic)
            require(dst.read_bytes() == b.body and b.file.read_bytes() == b.later, "whole-file readback differs")

            wrong_san = tls_context(TLS / "wrong-san.pem", TLS / "wrong-san.key")
            h1 = tls_context(TLS / "server.pem", TLS / "server.key", "http/1.1")
            for label, bad, refusal in (("wrong-san", wrong_san, "certificate"), ("h1-only", h1, "no-h2")):
                for args in (receipt_args, basic_args):
                    network(label, NEW, args, (), wire, bad, authority, [], basic, refused_tls=refusal)
            with listener() as alternate:
                alternate_origin = f"https://{HOST}:{alternate.getsockname()[1]}"
                for proto, state, args in (
                    (["--receipt-session"], a.state, ["status", a.op]), ([], b.state, ["reconcile"]),
                ):
                    for provided, account in ((alternate_origin, ACCOUNT), (origin, "other")):
                        observed, _ = pty_run("not-original", NEW,
                                              [*proto, provided, account, state, *args], [],
                                              rejected=True, watched=(wire, alternate))
                        require(b"original workspace HTTPS authority and account" in observed,
                                "wrong-origin guard returned another error")
            guest_phase("remove-issuing")
            remaining_roots, remaining_bundle = native_ca_set()
            require(remaining_roots == newer_roots - {issuing} and remaining_bundle == newer_bundle - {issuing},
                    "wrong decoded native CA removal")
            for args in (receipt_args, basic_args):
                network("missing-trust", NEW, args, (), wire, context, authority, [], basic,
                        refused_tls="certificate")
            require(profile(a.state) == a.original_profile and profile(b.state) == b.original_profile,
                    "negative cases changed exact stored identities")
            print("ZXS-CORE-INSTALLED-H2 cold verified readback and all pre-credential refusals passed", flush=True)
    finally:
        server.terminate()
        try:
            require(server.wait(timeout=18) == 0, "actual backend did not drain")
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=5)
            raise RuntimeError("backend drain timed out") from None


if __name__ == "__main__":
    try:
        run()
    except Exception as failure:
        # No terminal, header, secret or backend response bytes in serial output.
        # FixtureFailure messages are constant assertion names, not peer data.
        reason = str(failure) if isinstance(failure, FixtureFailure) else type(failure).__name__
        print("ZXS-CORE-INSTALLED-H2 failed " + reason, flush=True)
        sys.exit(1)
