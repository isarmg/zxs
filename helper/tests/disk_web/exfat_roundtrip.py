"""Manual, unexecuted acceptance fixture for a dedicated Debian 13 test host.

Requires a trusted standalone Helper and independent expected SHA-256 as its
two positional operands, and a separately prepared clean exFAT single-volume
image. Not invoked by a product, packaging or CI.
No device/formatting/repair, secret logging, browser upload, or arbitrary command.
"""
import hashlib
import http.client
import os
import re
import select
import signal
import stat
import sys
import time


def need(ok, reason):
    if not ok:
        raise RuntimeError(reason)


def stamp(path):
    m = os.stat(path, follow_symlinks=False)
    need(stat.S_ISREG(m.st_mode) and m.st_nlink == 1 and 1 << 20 <= m.st_size <= 128 << 20,
         "need a saved single-link ordinary fixture, 1–128 MiB")
    return m.st_dev, m.st_ino, m.st_size, m.st_mtime_ns, m.st_ctime_ns


def executable(path, expected):
    need(os.path.isabs(path) and os.path.normpath(path) == path
         and re.fullmatch("[0-9a-f]{64}", expected) and expected != "0" * 64,
         "need one existing absolute Helper and its independent SHA-256")
    def check(fd):
        m = os.fstat(fd)
        need(stat.S_ISREG(m.st_mode) and m.st_nlink == 1 and 1 <= m.st_size <= 64 << 20
             and m.st_uid in {0, os.getuid()} and stat.S_IMODE(m.st_mode) in {0o500, 0o555, 0o700, 0o755},
             "not a protected regular Helper executable")
        return m.st_dev, m.st_ino, m.st_size, m.st_mtime_ns, m.st_ctime_ns
    # This opt-in test presumes the fixture operator has already admitted
    # the local directory and genuine Linux procfs on the disposable host.
    pin = os.open(path, os.O_PATH | os.O_CLOEXEC | os.O_NOFOLLOW)
    try:
        first = check(pin)
        need(os.readlink("/proc/self") == str(os.getpid()), "unexpected process filesystem")
        fd = os.open(f"/proc/self/fd/{pin}", os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC)
    finally:
        os.close(pin)
    try:
        need(check(fd) == first, "the owned Helper descriptor changed")
        need(os.pread(fd, 7, 0) == b"\x7fELF\x02\x01\x01", "expected a Linux ELF64 Helper")
        digest = hashlib.sha256()
        position = 0
        while position < first[2]:
            part = os.pread(fd, min(65536, first[2] - position), position)
            need(part, "truncated Helper")
            digest.update(part)
            position += len(part)
        need(not os.pread(fd, 1, position) and check(fd) == first and digest.hexdigest() == expected,
             "different or changed Helper")
        return fd
    except BaseException:
        os.close(fd)
        raise


def run():
    image = os.environ["HELPER_TEST_EXFAT_CLEAN_IMAGE"]
    digest = os.environ["HELPER_TEST_EXFAT_CLEAN_SHA256"].lower()
    need(os.getuid() != 0 and os.path.isabs(image) and re.fullmatch("[0-9a-f]{64}", digest),
         "need an unprivileged worker, explicit image and independent SHA-256")
    baseline = stamp(image)
    need(len(sys.argv) == 3 and os.execve in os.supports_fd,
         "usage: exfat_roundtrip.py /absolute/path/to/helper EXPECTED_HELPER_SHA256")
    helper = executable(sys.argv[1], sys.argv[2])
    try:
        pid, tty = os.forkpty()
        if pid == 0:
            try:
                os.execve(helper, ["helper", "disk-web", "--exfat-copy", image, "--sha256", digest], {})
            finally:
                os._exit(127)
    finally:
        os.close(helper)
    origin = ""
    creds = {}
    try:
        end = time.monotonic() + 50
        banner = b""
        while True:
            m = re.search(rb"http://127[.]0[.]0[.]1:([0-9]{1,5})\r?\n[^\n]*?([a-f0-9]{64})\r?\n", banner)
            if m:
                break
            left = end - time.monotonic()
            need(left > 0 and len(banner) < 8192 and select.select([tty], [], [], left)[0], "no local pairing prompt")
            piece = os.read(tty, 1024)
            need(bool(piece), "panel ended before pairing")
            banner += piece
        port = int(m[1]); code = m[2]
        origin = "http://127.0.0.1:" + str(port)

        def post(path, body=None, headers=None):
            conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
            fields = {"Origin": origin, **(headers or {})}
            if body is not None:
                fields["Content-Type"] = "application/octet-stream"
            try:
                conn.request("POST", path, body=body, headers=fields)
                reply = conn.getresponse()
                raw = reply.read(8193)
                need(len(raw) <= 8192, "oversized response")
                return reply.status, raw.decode("utf-8")
            finally:
                conn.close()

        need(post("/api/copy/preview")[0] == 401, "anonymous preview must fail")
        status, pair = post("/api/pair", code)
        need(status == 200 and re.fullmatch("[0-9a-f]{64}\n[0-9a-f]{64}", pair), "pairing failed")
        bearer, csrf = pair.splitlines()
        creds = {"Authorization": "Bearer " + bearer, "X-Helper-CSRF": csrf}
        status, msg = post("/api/copy/preview", headers=creds)
        parts = msg.splitlines()
        need(status == 200 and len(parts) == 7 and re.fullmatch("[0-9a-f]{64}", parts[0])
             and parts[1:4] == [digest, "exFAT", str(baseline[2])]
             and parts[4] in {"512", "1024", "2048", "4096"} and parts[5:] == ["-", "prepared"],
             "preview must bind this exact exFAT copy")
        ticket = parts[0].encode("ascii")
        wrong = bytes([ord("0") if ticket[0] != ord("0") else ord("1")]) + ticket[1:]
        need(post("/api/copy/check", wrong, creds)[0] == 403, "foreign ticket accepted")
        need(post("/api/copy/check", ticket, {**creds, "Origin": "null"})[0] == 403, "foreign origin accepted")
        need(post("/api/copy/result", ticket, creds) == (200, "尚未开始检查。"), "preview or invalid submit started a checker")
        need(post("/api/copy/check", ticket, creds)[0] == 200, "could not claim selected fixture")
        end = time.monotonic() + 50
        while True:
            status, result = post("/api/copy/result", ticket, creds)
            need(status == 200 and time.monotonic() < end, "no confirmed clean fixture receipt")
            if result.startswith("exFAT："):
                break
            need(result.startswith("检查进行中"), "unexpected intermediate result")
            time.sleep(0.1)
        need("本次只读检查未报告问题" in result and "目录数：1（含根）；文件数：0" in result, "wrong clean-empty fixture result")
        need(post("/api/copy/check", ticket, creds) == (200, result), "resubmit did not return the original receipt")
        need(post("/api/close", headers=creds)[0] == 200 and stamp(image) == baseline, "closing or fixture identity failed")
        end = time.monotonic() + 5
        while not os.waitid(os.P_PID, pid, os.WEXITED | os.WNOWAIT | os.WNOHANG):
            need(time.monotonic() < end, "original panel did not exit after closing")
            time.sleep(0.05)
        child, status = os.waitpid(pid, 0); pid = None
        need(os.waitstatus_to_exitcode(status) == 0, "panel did not confirm worker receipt and exit")
    finally:
        os.close(tty)
        if pid is not None:  # Never signal a reaped PID. A failed test is not a drain receipt.
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            end = time.monotonic() + 3
            while not os.waitid(os.P_PID, pid, os.WEXITED | os.WNOWAIT | os.WNOHANG):
                if time.monotonic() >= end:
                    os.kill(pid, signal.SIGKILL)
                    break
                time.sleep(0.05)
            os.waitpid(pid, 0)


if __name__ == "__main__":
    run()
