"""Observe startup prerequisites in the signed, disposable CI guest only."""

import fcntl
import os
import pty
import resource
import signal
import socket
import stat
import struct
import termios
from pathlib import Path


def scope():
    same = os.path.samestat
    if (os.geteuid() != 0 or os.uname().release != "7.0.0-43-generic"
            or os.uname().machine != "x86_64" or os.listdir("/sys/class/net") != ["lo"]
            or not same(os.stat("/"), os.stat("/proc/1/root"))
            or not all(same(os.stat(f"/proc/thread-self/ns/{ns}"), os.stat(f"/proc/1/ns/{ns}"))
                       for ns in ("pid", "user", "mnt", "net"))):
        raise RuntimeError("unexpected disposable guest scope")
    print("TERMINAL-PREFLIGHT scope kernel=7.0.0-43-generic arch=x86_64 root=PID1 interfaces=lo", flush=True)


def surface(fd):
    meta = os.fstat(fd)
    info = dict(line.split(":", 1) for line in Path(f"/proc/thread-self/fdinfo/{fd}").read_text().splitlines())
    return (meta.st_dev, meta.st_ino, meta.st_rdev, int(info["mnt_id"])), meta


def tty_status():
    first, inode = surface(0)
    dev, ino, rdev, mount = first
    direct = stat.S_ISCHR(inode.st_mode) and ino > 0 and mount > 0 and rdev != os.makedev(5, 0)
    rows = (line.split() for line in Path("/proc/thread-self/mountinfo").read_text().splitlines())
    devpts = any(row[0] == str(mount) and row[2] == f"{os.major(dev)}:{os.minor(dev)}"
                 and row[row.index("-") + 1] == "devpts" for row in rows)
    access = [fcntl.fcntl(fd, fcntl.F_GETFL) for fd in (0, 1, 2)]
    normal = (not any(flag & os.O_PATH for flag in access)
              and (access[0] & os.O_ACCMODE) in (os.O_RDONLY, os.O_RDWR)
              and all((flag & os.O_ACCMODE) in (os.O_WRONLY, os.O_RDWR) for flag in access[1:]))
    same = first == surface(1)[0] == surface(2)[0]
    # Linux TIOCGSID is not exported by Python's termios module.
    sid = struct.unpack("=i", fcntl.ioctl(0, 0x5429, bytes(4)))[0]
    pgrp = os.tcgetpgrp(0)
    foreground = sid == pgrp == os.getsid(0) == os.getpgrp() == os.getpid()
    discipline = struct.unpack("=i", fcntl.ioctl(0, termios.TIOCGETD, bytes(4)))[0]
    canonical = bool(termios.tcgetattr(0)[3] & termios.ICANON)
    ready = direct and devpts and normal and same and foreground and discipline == 0 and canonical
    return (f"observed ready={ready} same_stdio={same} devpts={devpts} direct={direct} access={normal} "
            f"foreground={foreground} sid={sid} pgrp={pgrp} n_tty={discipline == 0} canonical={canonical} "
            f"mount={mount} inode={ino} rdev={os.major(rdev)}:{os.minor(rdev)} owner={inode.st_uid}:{inode.st_gid}")


def audit_status():
    # Linux NETLINK_AUDIT (9), AUDIT_TTY_GET (1016), one unicast request.
    with socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 9) as netlink:
        netlink.settimeout(2)
        netlink.bind((0, 0))
        port, groups = netlink.getsockname()
        if port == 0 or groups != 0:
            return "unavailable reason=socket-address"
        request = struct.pack("=IHHII", 16, 1016, 1, 1, port)
        if netlink.sendto(request, (0, 0)) != len(request):
            return "unavailable reason=short-request"
        try:
            reply, _, flags, source = netlink.recvmsg(24)
        except TimeoutError:
            return "unavailable reason=timeout"
    header = struct.pack("=IHHII", 24, 1016, 0, 1, 0)
    if (source != (0, 0) or flags & (socket.MSG_TRUNC | socket.MSG_CTRUNC)
            or len(reply) != 24 or reply[:16] != header):
        return "unavailable reason=not-exact-kernel-reply"
    enabled, password_logging = struct.unpack("=II", reply[16:])
    return f"observed enabled={enabled} log_passwd={password_logging} inactive={enabled == password_logging == 0}"


def memlock_status():
    soft, hard = resource.getrlimit(resource.RLIMIT_MEMLOCK)

    def display(value):
        return "unlimited" if value == resource.RLIM_INFINITY else str(value)

    enough = soft == resource.RLIM_INFINITY or soft > 64 * 1024 * 1024
    return f"observed soft_bytes={display(soft)} hard_bytes={display(hard)} exceeds_64MiB={enough}"


def main():
    scope()
    # Retain serial reporting separately: leave the new PTY's three streams untouched.
    report = os.dup(1)
    child, master = pty.fork()
    if child == 0:
        signal.alarm(8)
        with os.fdopen(report, "w") as output:
            for name, probe in (("pty", tty_status), ("audit", audit_status), ("memlock", memlock_status)):
                try:
                    result = probe()
                except (OSError, termios.error) as error:
                    number = error.args[0] if error.args and isinstance(error.args[0], int) else "unknown"
                    result = f"unavailable errno={number}"
                print(f"TERMINAL-PREFLIGHT {name} {result}", file=output, flush=True)
        os._exit(0)
    os.close(report)
    try:
        _, state = os.waitpid(child, 0)
    finally:
        os.close(master)
    if state != 0:
        raise RuntimeError(f"terminal observation child did not finish: {state}")
    print("TERMINAL-PREFLIGHT observations complete; no product run", flush=True)


if __name__ == "__main__":
    main()
