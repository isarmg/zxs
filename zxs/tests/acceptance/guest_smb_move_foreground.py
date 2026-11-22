"""The three real foreground phases of one independently approved VM fixture."""

from contextlib import contextmanager
import errno
import os
from pathlib import Path
import pty
import re
import resource
import select
import signal
import termios
import time

from guest_smb_move_media import ROLES, selected
from guest_terminal_preflight import audit_status, tty_status

HELPER = "/zxs/bin/helper"
SERVER = "/zxs/bin/zxs"
UID = 23100
GID = 23100
LOGIN = "movecheck"
HOST = "zxs-lab.test"
ADDRESS = "192.0.2.1"
SCOPE = ADDRESS + "/30"
ENV = {"LC_ALL": "C.UTF-8", "PATH": "/usr/bin:/bin", "HOME": "/nonexistent"}
LOCK_BYTES = 128 * 1024 * 1024
MAX_TTY = 2 * 1024 * 1024
HEX32 = r"([0-9a-f]{32})"
HEX64 = r"([0-9a-f]{64})"
HELPER_PROMPT = "同意以上完整范围时，输入 {} " + HEX32 + " 并回车。此处不输入口令。"
RUN_PROMPT = re.compile("同意以上原件后，请输入 RUN-ZXS " + HEX32 + "。")
PASSWORD = (
    "请输入 Web 与 SMB 共用的同一账户口令（不回显），然后按回车。",
    "请再次输入同一账户口令。",
)
READY = "ZXS：本次 Web 和内置 SMB 已核对同一原 SYSTEM 与凭据代，已共同放行。尚未确认客户端互通或旧单恢复完成。"
REAPED = "ZXS：本次 Web 和内置 SMB 的两个原进程已确认退出；不表示每个文件已确认写入。"
INIT_RESULT = re.compile("fresh-init=confirmed pool=" + HEX32 + " operation=" + HEX32 +
                         " init_request_sha256=" + HEX64 + "; original volume markers verified")
SETUP_RESULT = re.compile("fresh-zxs=confirmed pool=" + HEX32 + " init=" + HEX32 + " instance=" + HEX32 +
                          " setup=" + HEX32 + " site_sha256=" + HEX64 + " generation=1 record=" + HEX32 +
                          "; installed authority only, no service started or login checked")


def require(value):
    if not value:
        raise RuntimeError("one original foreground phase did not confirm")


def remaining(end):
    left = end - time.monotonic()
    require(left > 0)
    return left


def live(pidfd):
    return not select.select((pidfd,), (), (), 0)[0]


class Tty:
    """Retain the original pty and kernel process identity, with no transcript output."""

    def __init__(self, pid, master, pidfd):
        self.pid, self.master, self.pidfd = pid, master, pidfd
        self.pending = b""
        self.total = 0
        self.eof = False
        self.waited = False

    def line(self, end):
        while b"\n" not in self.pending and not self.eof:
            if not select.select((self.master,), (), (), remaining(end))[0]:
                raise RuntimeError("bounded original terminal wait expired")
            try:
                added = os.read(self.master, 8192)
            except OSError as error:
                if error.errno != errno.EIO:  # exit of the final original slave
                    raise
                added = b""
            self.eof = not added
            self.total += len(added)
            require(self.total <= MAX_TTY)
            self.pending += added
        if self.eof and not self.pending:
            return None
        line, delimiter, self.pending = self.pending.partition(b"\n")
        require(delimiter and len(line) <= 96 * 1024)
        return line.removesuffix(b"\r").decode("utf-8", errors="strict")

    def until(self, pattern, *, scope=(), timeout=115):
        end = time.monotonic() + timeout
        preview = []
        while (line := self.line(end)) is not None:
            match = pattern.fullmatch(line)
            if match is not None:
                whole = "\n".join(preview)
                require(all(value in whole for value in scope))
                return match.groups()
            preview.append(line)
        raise RuntimeError("missing exact foreground prompt or receipt")

    def input(self, data, *, hidden):
        require(live(self.pidfd) and os.tcgetpgrp(self.master) == self.pid)
        local = termios.tcgetattr(self.master)[3]
        echo, canonical = local & termios.ECHO, local & termios.ICANON
        require((not echo and not canonical) if hidden else bool(canonical))
        require(len(data) < 96 and data.endswith(b"\n") and b"\n" not in data[:-1])
        require(os.write(self.master, data) == len(data))

    def approve(self, verb, required):
        # Only the fixed action and preview from this original, approved VM.
        # No generic prompt answering, reusable token, or direct product grant.
        pattern = RUN_PROMPT if verb == "RUN-ZXS" else re.compile(HELPER_PROMPT.format(verb))
        token, = self.until(pattern, scope=required)
        require(any(c != "0" for c in token))
        self.input((verb + " " + token + "\n").encode("ascii"), hidden=False)

    def drain(self, *, clean, timeout=40):
        end = time.monotonic() + timeout
        while self.line(end) is not None:
            pass
        status = self.reap(end)
        if clean:
            require(status == 0)

    def reap(self, end):
        require(not self.waited and select.select((self.pidfd,), (), (), remaining(end))[0])
        actual, status = os.waitpid(self.pid, 0)
        self.waited = True
        require(actual == self.pid)
        return status

    def terminate(self):
        if live(self.pidfd):
            try:
                signal.pidfd_send_signal(self.pidfd, signal.SIGTERM)
            except ProcessLookupError:
                pass


@contextmanager
def foreground(argv, *, lock_budget=False):
    pid, master = pty.fork()
    if pid == 0:
        try:
            require(tty_status().startswith("observed ready=True "))
            require(audit_status() == "observed enabled=0 log_passwd=0 inactive=True")
            if lock_budget:
                # Exact runtime approval is required before dispatch. Change
                # only this new credential process, never VM/host limits.
                require(argv[0:3] == (HELPER, "setup-pair", "issue-fresh"))
                resource.setrlimit(resource.RLIMIT_MEMLOCK, (LOCK_BYTES, LOCK_BYTES))
                require(resource.getrlimit(resource.RLIMIT_MEMLOCK) == (LOCK_BYTES, LOCK_BYTES))
            os.execve(argv[0], argv, ENV)
        except (OSError, RuntimeError, termios.error):
            os._exit(125)
    try:
        pidfd = os.pidfd_open(pid)
    except OSError:
        os.close(master)
        raise
    tty = Tty(pid, master, pidfd)
    try:
        yield tty
    finally:
        if not tty.waited:
            tty.terminate()
            try:
                # Preserve ownership through actual process exit, even if
                # the terminal already produced an incomplete final line.
                tty.reap(time.monotonic() + 30)
            except (OSError, RuntimeError):
                pass  # The outer disposable VM still fails; no alternate process is touched.
        os.close(pidfd)
        os.close(master)


def initialize(ids, mounts):
    subject = ("--init-actor", ids["actor"], "--storage-uid", str(UID), "--storage-gid", str(GID))
    argv = (HELPER, "install-pool", "new-deployment", *subject, "--op", ids["init"],
            "--approval", ids["init_approval"], "--system", str(selected("system")),
            "--recycle", str(selected("recycle")),
            "--data", str(selected("source")), "--data", str(selected("target")))
    original = [ids["actor"], ids["init"], ids["init_approval"], f"{UID}/{GID}"]
    for role, name in ROLES.items():
        dev, mount, _, inode = mounts[role]
        original.append(f"{selected(name)}  [dev {dev}, inode {inode}, mount {mount}]")
    with foreground(argv) as tty:
        tty.approve("INIT-FRESH-POOL", original)
        pool, operation, request = tty.until(INIT_RESULT, timeout=160)
        tty.drain(clean=True)
    require(operation == ids["init"] and any(c != "0" for c in pool) and any(c != "0" for c in request))
    return subject, (pool, operation, request)


def issue(ids, request, providers, subject, init, password):
    pool, operation, init_sha = init
    zxs, helper = request["server_sha256"], request["helper_sha256"]
    argv = (HELPER, "setup-pair", "issue-fresh", *subject, "--system", str(selected("system")),
            "--pool", pool, "--init", operation,
            "--instance", ids["instance"], "--bind", ids["bind"], "--setup-op", ids["setup"],
            "--init-sha256", init_sha, "--approval", ids["setup_approval"], "--zxs-sha256", zxs,
            "--helper-sha256", helper, "--gcrypt-sha256", providers["gcrypt"],
            "--gpgerror-sha256", providers["gpgerror"], "--host", HOST, "--login", LOGIN,
            "--recycle-name", "Recycle", "--listen", SCOPE, "--share", str(selected("source")), "Source", "rw",
            "--share", str(selected("target")), "Target", "rw")
    mounted = [str(selected(role)) for role in ROLES.values()]
    scope = (pool, operation, init_sha, ids["actor"], ids["instance"], ids["bind"], ids["setup"],
             ids["setup_approval"], zxs, helper, *providers.values(), *mounted, HOST, LOGIN,
             f"SMB {ADDRESS}:445 / 子网前缀 30", '目录 "Source"', '目录 "Target"')
    with foreground(argv, lock_budget=True) as tty:
        tty.approve("ISSUE-FRESH-ACCOUNT", scope)
        for line in PASSWORD:
            tty.until(re.compile(re.escape(line)), timeout=180)
            tty.input(password + b"\n", hidden=True)
        receipt = tty.until(SETUP_RESULT, timeout=200)
        tty.drain(clean=True)
    require(receipt[:4] == (pool, operation, ids["instance"], ids["setup"])
            and all(any(c != "0" for c in value) for value in receipt[4:]))
    return receipt


def proc_read(pid, member, limit=65536):
    with (Path("/proc") / str(pid) / member).open("rb") as original:
        value = original.read(limit + 1)
    require(len(value) <= limit)
    return value


def lowerings(tty):
    image = os.stat(SERVER, follow_symlinks=False)
    require(os.path.samestat(image, os.stat(f"/proc/{tty.pid}/exe")) and live(tty.pidfd))
    children = proc_read(tty.pid, f"task/{tty.pid}/children", 128).split()
    roles = {}
    try:
        require(len(children) == 2 and len(set(children)) == 2 and all(p.isdigit() for p in children))
        for raw in children:
            pid = int(raw)
            pidfd = os.pidfd_open(pid)
            try:
                require(live(pidfd) and os.path.samestat(image, os.stat(f"/proc/{pid}/exe")))
                role = proc_read(pid, "cmdline", 80)
                require(role in (b"zxs:internal-smb-311\0", b"zxs:internal-web-1\0") and role not in roles)
                status = dict(line.split(b":", 1) for line in proc_read(pid, "status").splitlines())
                require(status[b"PPid"].split() == [str(tty.pid).encode("ascii")]
                        and status[b"NoNewPrivs"].split() == [b"1"]
                        and status[b"Seccomp"].split() == [b"2"]
                        and status[b"Uid"].split() == [str(UID).encode("ascii")] * 4
                        and status[b"Gid"].split() == [str(GID).encode("ascii")] * 4
                        and not status[b"Groups"].split()
                        and all(int(status[b"Cap" + cap], 16) == 0
                                for cap in (b"Inh", b"Prm", b"Eff", b"Bnd", b"Amb")))
                roles[role] = pidfd
                pidfd = None
            finally:
                if pidfd is not None:
                    os.close(pidfd)
        require(live(tty.pidfd) and all(live(fd) for fd in roles.values()))
        return roles
    except BaseException:
        for fd in roles.values():
            os.close(fd)
        raise


@contextmanager
def run_server(ids, init, server_sha):
    pool, operation, _ = init
    expected = (f"Pool {pool} / INIT {operation} / Setup {ids['setup']} / Instance {ids['instance']} / Bind {ids['bind']}。",
                f'原 SYSTEM "{selected("system")}"', f"存储 UID/GID {UID}/{GID}",
                f'"{SERVER}" SHA-256 {server_sha}', HOST, LOGIN, f"{ADDRESS}:445",
                '目录 "Source"', '目录 "Target"', '专用回收目录 "Recycle"')
    with foreground((SERVER,)) as tty:
        tty.approve("RUN-ZXS", expected)
        tty.until(re.compile(re.escape(READY)), timeout=140)
        roles = lowerings(tty)
        try:
            yield tty, (READY, tuple(sorted(key.decode("ascii").removesuffix("\0") for key in roles)))
            require(live(tty.pidfd) and all(live(fd) for fd in roles.values()))
            tty.terminate()
            tty.until(re.compile(re.escape(REAPED)), timeout=80)
            tty.drain(clean=True)
            require(not any(live(fd) for fd in roles.values()))
        finally:
            for fd in roles.values():
                os.close(fd)
