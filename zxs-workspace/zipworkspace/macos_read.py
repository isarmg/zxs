"""Darwin-only safeguards for reading an explicitly chosen folder, not for publishing archives."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack, contextmanager
import ctypes as C
import errno
from functools import lru_cache
import os
import stat
import struct
import sys

from .document import native_identity
from .model import Rejected

# Public Darwin sys/resource.h, sys/attr.h, sys/mount.h and sys/stat.h constants.
_MATERIALIZE, _THREAD, _OFF = 3, 1, 1
_RETURNED = _VOL_INFO = 0x80000000
_VOL_MOUNTFLAGS, _MNT_LOCAL, _DATALESS = 0x4000, 0x1000, 0x40000000
_VOL_UUID, _VOL_FSTYPE = 0x40000, 0x100000
_O_EVTONLY = 0x8000  # Darwin fcntl.h; used for name witnesses, never for payload reads.


class _Attributes(C.Structure):
    _fields_ = (("count", C.c_uint16), ("reserved", C.c_uint16), ("common", C.c_uint32),
                ("volume", C.c_uint32), ("directory", C.c_uint32), ("file", C.c_uint32),
                ("fork", C.c_uint32))


@lru_cache(maxsize=1)
def _api():
    if sys.platform != "darwin": return None
    try:
        lib = C.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
        get, set_, attrs = lib.getiopolicy_np, lib.setiopolicy_np, lib.fgetattrlist
    except (OSError, AttributeError):
        return None
    get.argtypes = (C.c_int, C.c_int)
    set_.argtypes = (C.c_int, C.c_int, C.c_int)
    attrs.argtypes = (C.c_int, C.POINTER(_Attributes), C.POINTER(C.c_uint32), C.c_size_t, C.c_uint)
    get.restype = set_.restype = attrs.restype = C.c_int
    return get, set_, attrs


def available():
    return _api() is not None


def _require():
    functions = _api()
    if functions is None: raise Rejected("macOS 未提供比较所需的线程读取策略和原卷查询；未改用普通路径读取")
    return functions


def in_disposable_thread(work, *args, **kwargs):
    # XNU's setting is per native thread. If restoring it ever fails, the
    # affected thread must not do another job; never change the Qt/shared
    # worker policy here. Executor exit joins even on a failed result.
    with ThreadPoolExecutor(max_workers=1, thread_name_prefix="macos-original-reader") as pool:
        return pool.submit(work, *args, **kwargs).result()


@contextmanager
def no_materialization():
    """Apple TN3150: opt out on this calling thread only, including intermediate path lookups."""
    get, set_, _ = _require()
    original = get(_MATERIALIZE, _THREAD)
    if original not in (0, _OFF, 2):
        raise Rejected("无法核对 macOS 当前线程的原读取策略；没有开始操作")
    try:
        if original != _OFF:
            # The native change can succeed before Python observes an interruption.
            if set_(_MATERIALIZE, _THREAD, _OFF) != 0 or get(_MATERIALIZE, _THREAD) != _OFF:
                raise Rejected("无法阻止 macOS 自动下载占位内容；没有开始操作")
        try:
            yield
        except OSError as error:
            if error.errno != errno.EDEADLK: raise
            raise Rejected("macOS 拒绝读取，内容可能只在云端；请先在系统中下载所需内容后重新选择") from error
    finally:
        if original != _OFF:
            if set_(_MATERIALIZE, _THREAD, original) != 0 or get(_MATERIALIZE, _THREAD) != original:
                raise Rejected("无法恢复 macOS 当前线程原有的读取策略；不返回对照结果")


def resident(name, fd):
    resident_stat(name, os.fstat(fd))


def resident_stat(name, info):
    flags = getattr(info, "st_flags", None)
    if flags is None or flags & _DATALESS:
        raise Rejected(f"无法确认本机已有完整内容：{name}。未将占位文件或目录当作空内容")


def local_object(name, fd):
    local_volume(fd)
    resident(name, fd)


def local_volume(fd):
    # fgetattrlist asks the actual open vnode for its volume. The fixed-size reply
    # is length + 5 returned attribute masks + the requested uint32 mount flags.
    _, _, attrs = _require()
    request = _Attributes(5, 0, _RETURNED, _VOL_INFO | _VOL_MOUNTFLAGS)
    result = (C.c_uint32 * 7)()
    if attrs(fd, C.byref(request), result, C.sizeof(result), 4) != 0:  # FSOPT_REPORT_FULLSIZE
        err = C.get_errno()
        raise OSError(err, "无法从所持句柄核对 macOS 卷")
    size, common, volume, dirs, files, forks, flags = result
    if (size != C.sizeof(result) or common != _RETURNED or volume not in (_VOL_MOUNTFLAGS, _VOL_INFO | _VOL_MOUNTFLAGS)
            or dirs or files or forks or not flags & _MNT_LOCAL):
        raise Rejected("此对照只读取 macOS 能从原句柄确认的本地卷；远程或未知卷未比较")


def apfs_volume(fd):
    """Read the actual local APFS identity; writing policy is deliberately separate."""
    resident("所选归档或目录", fd)
    _, _, attrs = _require()
    need = _VOL_MOUNTFLAGS | _VOL_UUID | _VOL_FSTYPE
    request = _Attributes(5, 0, _RETURNED, _VOL_INFO | need)
    buf = (C.c_uint32 * 32)()
    if attrs(fd, C.byref(request), buf, C.sizeof(buf), 4):
        raise OSError(C.get_errno(), "无法核实原 APFS 卷")
    raw = bytes(buf)
    size, ret, vol, dirs, files, forks = struct.unpack_from("=6I", raw)
    if not 52 <= size <= len(raw) or ret != _RETURNED or vol not in (need, _VOL_INFO | need) or dirs or files or forks:
        raise Rejected("系统未返回完整的原卷类型和身份")
    flags, = struct.unpack_from("=I", raw, 24)
    volume = raw[28:44]
    off, n = struct.unpack_from("=iI", raw, 44)
    if (not flags & _MNT_LOCAL or not any(volume) or off < 8 or n != 5 or
            44 + off + n > size or raw[44+off:44+off+n] != b"apfs\0"):
        raise Rejected("只支持可确证的本地 APFS 卷")
    return (os.fstat(fd).st_dev, volume), flags


def check_literal_path(path, fd):
    # XNU fcntl(F_GETPATH) takes at least MAXPATHLEN bytes (1024).
    import fcntl
    result = fcntl.fcntl(fd, 50, bytes(1024))
    if result.partition(b"\0")[0] != os.fsencode(path):
        raise Rejected("原句柄的系统路径与所选的准确名称不一致")


def check_spelling(fd, leaf):
    # APFS may accept different case or normalization. Require the actual leaf.
    _, _, attrs = _require()
    request = _Attributes(5, 0, _RETURNED | 1)
    buf = (C.c_uint32 * 160)()
    if attrs(fd, C.byref(request), buf, C.sizeof(buf), 4):
        raise OSError(C.get_errno(), "无法核实实际文件名")
    raw = bytes(buf)
    size, common, vol, dirs, files, forks, off, n = struct.unpack_from("=6IiI", raw)
    expected = os.fsencode(leaf) + b"\0"
    if (not 32 <= size <= len(raw) or common != (_RETURNED | 1) or vol or dirs or files or forks or
            off < 8 or n != len(expected) or 24+off+n > size or raw[24+off:24+off+n] != expected):
        raise Rejected("实际文件名与所选原名的拼写不一致")


def named_flags(*, directory=False, metadata=False):
    mode = _O_EVTONLY if metadata else os.O_RDONLY
    return mode | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK | (os.O_DIRECTORY if directory else 0)


@contextmanager
def literal_name(path, *, directory):
    """Under the caller's opt-out, prove a name one component at a time without following a link."""
    if (path.anchor != "/" or ".." in path.parts or len(path.parts) > 65 or len(os.fsencode(path)) >= 1024 or
            not directory and len(path.parts) < 2):
        raise Rejected("需要能逐级核验的原本机路径")
    with ExitStack() as held:
        current = None
        for n, part in enumerate(path.parts, 1):
            is_dir = directory or n < len(path.parts)
            if not is_dir:
                before = os.stat(part, dir_fd=current, follow_symlinks=False)
                native_identity(before, stat.S_IFREG)
                resident_stat(part, before)
            current = os.open(part, named_flags(directory=is_dir, metadata=not is_dir), dir_fd=current)
            held.callback(os.close, current)
            native_identity(os.fstat(current), stat.S_IFDIR if is_dir else stat.S_IFREG)
            local_object(part, current)
        check_literal_path(path, current)
        if len(path.parts) > 1: check_spelling(current, path.name)
        yield current
