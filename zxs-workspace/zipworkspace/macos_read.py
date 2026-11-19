"""Darwin-only safeguards for reading an explicitly chosen folder, not for publishing archives."""
from contextlib import contextmanager
import ctypes as C
import errno
from functools import lru_cache
import os
import sys

from .model import Rejected

# Public Darwin sys/resource.h, sys/attr.h, sys/mount.h and sys/stat.h constants.
_MATERIALIZE, _THREAD, _OFF = 3, 1, 1
_RETURNED = _VOL_INFO = 0x80000000
_VOL_MOUNTFLAGS, _MNT_LOCAL, _DATALESS = 0x4000, 0x1000, 0x40000000


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
    flags = getattr(os.fstat(fd), "st_flags", None)
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
