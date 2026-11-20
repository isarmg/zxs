"""Held native Linux mount evidence and literal directory chains for name publication."""
import ctypes
import os
import stat
from threading import get_native_id

from .document import native_identity
from .model import Rejected
from .private_freeze import mount_id, mount_record


def owned_directory(stack, path, uid, read_proc):
    # Hold each ancestor of the literal path. We never resolve a symlink or
    # continue under a newly selected directory after approval.
    descriptors, identities = [], []
    for i, part in enumerate(path.parts):
        mode = os.O_RDONLY if i == len(path.parts)-1 else os.O_PATH
        fd = os.open(part, mode | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                     dir_fd=descriptors[-1] if descriptors else None)
        stack.callback(os.close, fd)
        m = os.fstat(fd)
        identities.append((*native_identity(m, stat.S_IFDIR), mount_id(fd, read_proc=read_proc)))
        if m.st_uid not in (0, uid) or m.st_mode & 0o022:
            raise Rejected("所选目录和上级目录必须属于本人或 root，且不能让组或其他人写入")
        descriptors.append(fd)
    parent = descriptors[-1]
    if os.fstat(parent).st_uid != uid:
        raise Rejected("需要本人所有的现有目录")
    return parent, identities


class _OpenHow(ctypes.Structure):
    _fields_ = [(field, ctypes.c_uint64) for field in ("flags", "mode", "resolve")]


class ProcView:
    """Use genuine kernel procfs files, never the ambient proc spelling for evidence."""
    def __init__(self, stack):
        # The Linux native LP64 x86-64/AArch64 UAPI is used for both syscalls.
        uname = os.uname()
        fstatfs = {"x86_64": 138, "aarch64": 44}.get(uname.machine)
        if (uname.sysname != "Linux" or not fstatfs or ctypes.sizeof(ctypes.c_long) != 8 or
                ctypes.sizeof(ctypes.c_void_p) != 8):
            raise Rejected("本操作需要 Linux x86-64 或 AArch64 原生 64 位接口")
        self.syscall = ctypes.CDLL(None, use_errno=True).syscall
        self.syscall.argtypes = (ctypes.c_long,)
        self.syscall.restype = ctypes.c_long
        flags = os.O_PATH | os.O_DIRECTORY | os.O_CLOEXEC
        root = os.open("/proc", flags | os.O_NOFOLLOW)
        stack.callback(os.close, root)
        # asm-generic/statfs.h: 120-byte struct, first native word is f_type.
        fields = (ctypes.c_long * 15)()
        if self.syscall(fstatfs, ctypes.c_int(root), ctypes.byref(fields)) != 0 or fields[0] != 0x9fa0:
            raise Rejected("不能从原目录句柄核实真正的 Linux procfs")
        self.pid = os.getpid()
        if os.readlink("self", dir_fd=root) != str(self.pid):
            raise Rejected("procfs 不对应本进程的 PID")
        self.task = self._open(root, str(self.pid), flags)
        stack.callback(os.close, self.task)
        self.fds = self._open(self.task, "fd", flags)
        stack.callback(os.close, self.fds)
        _, base, path, kind = mount_record(root, read_proc=self.read)
        if (base, path, kind) != (b"/", b"/proc", b"proc"):
            raise Rejected("不是原 procfs 根挂载")

    def _open(self, parent, name, flags):
        # Linux openat2: NO_XDEV | NO_SYMLINKS | BENEATH. This excludes a
        # separately mounted lookalike even before opening/reading its files.
        how = _OpenHow(flags, 0, 0x0d)
        fd = self.syscall(437, ctypes.c_int(parent), ctypes.c_char_p(os.fsencode(name)),
                          ctypes.byref(how), ctypes.c_size_t(ctypes.sizeof(how)))
        if fd < 0:
            code = ctypes.get_errno()
            raise Rejected(f"无法在原 procfs 挂载内取得必要句柄（openat2: {os.strerror(code)}）")
        return fd

    def read(self, name, limit):
        if os.getpid() != self.pid: raise Rejected("原进程的 procfs 证据不能移交")
        if name == "status": name = f"task/{get_native_id()}/status"
        fd = self._open(self.task, name, os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC)
        try:
            native_identity(os.fstat(fd), stat.S_IFREG)
            with os.fdopen(fd, "rb", closefd=False) as actual:
                return actual.read(limit + 1)
        finally: os.close(fd)
