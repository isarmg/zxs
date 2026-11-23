"""Private adapter for the already installed distribution FUSE3 helper.

Only the selected Linux helper, its audited descriptor handoff and normal unmount.
"""
from array import array
from contextlib import contextmanager
import fcntl
import os
from pathlib import Path
import socket
import stat
import subprocess

from .linux_authority import owned_directory
from .model import Rejected

FUSE_VERSION = b"3.18.3"
_HELPER = "/usr/bin/fusermount3"
_DEVICE = "/dev/fuse"
_ENV = {"PATH": "/usr/bin:/bin", "LC_ALL": "C"}
_INT = array("i").itemsize


class _InstalledFuse:
    def __init__(self, proc, binary, identity, device):
        self.proc, self.binary, self._identity, self._device = proc, binary, identity, device

    def _spawn(self, args, *, extra=(), stdout=subprocess.DEVNULL):
        m = os.fstat(self.binary)
        if ((m.st_dev, m.st_ino, m.st_mode, m.st_size, m.st_mtime_ns, m.st_ctime_ns) != self._identity
                or not os.path.samestat(os.stat("/proc/self/fd"), os.fstat(self.proc.fds))):
            raise Rejected("the original system FUSE helper or procfs changed")
        # A concrete root-owned installed ELF opened before use. No shell, PATH
        # resolution, app wrapper, inherited user env or extra application FDs.
        return subprocess.Popen([_HELPER, *args], executable=f"/proc/self/fd/{self.binary}",
                                pass_fds=(self.binary, *extra), cwd="/", env=_ENV, shell=False,
                                close_fds=True, stdin=subprocess.DEVNULL, stdout=stdout,
                                stderr=subprocess.DEVNULL)

    def check_version(self):
        with self._spawn(["--version"], stdout=subprocess.PIPE) as child:
            out = child.stdout.read(128)
        if child.returncode != 0 or out != b"fusermount3 version: " + FUSE_VERSION + b"\n":
            raise Rejected("the installed fusermount3 is not the approved editable-volume profile")

    def mount(self, point, source, flags):
        # Exact protocol of libfuse 3.18.3's installed companion. Only that
        # subprocess can send on this private socketpair.
        left, right = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        received = []
        try:
            with left, right:
                options = ",".join(sorted({*flags, "default_permissions", "fsname=" + source}))
                with self._spawn(["--comm-fd", str(right.fileno()), "-o", options, "--", str(point)],
                                 extra=(right.fileno(),)) as child:
                    right.close()
                    marker, control, bits, _ = left.recvmsg(1, socket.CMSG_SPACE(_INT), socket.MSG_CMSG_CLOEXEC)
                    for level, kind, data in control:
                        if (level, kind) == (socket.SOL_SOCKET, socket.SCM_RIGHTS):
                            actual = array("i")
                            actual.frombytes(data[:len(data) // _INT * _INT])
                            received.extend(actual)
                    exact = (len(control) == 1 and len(control[0][2]) == _INT and len(received) == 1
                             and marker == b"\0" and not bits & (socket.MSG_TRUNC | socket.MSG_CTRUNC))
                    more, extra, end_bits, _ = left.recvmsg(1, 0, socket.MSG_CMSG_CLOEXEC)
                    if child.wait() != 0 or not exact or more or extra or end_bits & socket.MSG_CTRUNC:
                        raise Rejected("the native FUSE helper did not return one exact connection descriptor")
            fd = received[0]
            m = os.fstat(fd)
            if (fd < 3 or not stat.S_ISCHR(m.st_mode) or m.st_rdev != self._device
                    or fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_ACCMODE != os.O_RDWR
                    or os.get_inheritable(fd)):
                raise Rejected("the native helper did not return the original FUSE character device")
            received.clear()
            return fd
        finally:
            for fd in received:
                os.close(fd)

    def unmount(self, point):
        # No lazy/force/auto flag; the caller requires a real child exit 0,
        # followed by the exact native absence and original directory witnesses.
        with self._spawn(["-u", "--", str(point)]) as child:
            return child.wait()


@contextmanager
def installed_fuse(hold, proc):
    parent, _ = owned_directory(hold, Path("/usr/bin"), 0, proc.read, metadata_only=True)
    fd = os.open("fusermount3", os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
    hold.callback(os.close, fd)
    m = os.fstat(fd)
    if (not stat.S_ISREG(m.st_mode) or m.st_uid != 0 or m.st_mode & 0o022
            or not m.st_mode & stat.S_ISUID or not m.st_mode & 0o111
            or os.readlink(str(fd), dir_fd=proc.fds) != _HELPER or os.pread(fd, 4, 0) != b"\x7fELF"):
        raise Rejected("the original system FUSE helper is unavailable; no permission change")
    dev = os.stat(_DEVICE, follow_symlinks=False)
    if not stat.S_ISCHR(dev.st_mode) or (os.major(dev.st_rdev), os.minor(dev.st_rdev)) != (10, 229):
        raise Rejected("the original kernel FUSE character device is unavailable")
    helper = _InstalledFuse(proc, fd, (m.st_dev, m.st_ino, m.st_mode, m.st_size, m.st_mtime_ns, m.st_ctime_ns), dev.st_rdev)
    helper.check_version()
    yield helper
