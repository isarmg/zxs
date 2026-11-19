"""macOS evidence for copying a complete, marked archive. Never creates or changes a label."""
from contextlib import ExitStack, contextmanager
import ctypes as C
from functools import lru_cache
import os
import stat
import struct
import sys

from . import macos_read
from .document import native_identity
from .model import Rejected

# sys/attr.h, sys/fcntl.h, sys/clonefile.h and CoreFoundation/CFURL.h.
_UUID, _FSTYPE, _RET = 0x40000, 0x100000, 0x80000000
_ZONE = b"com.apple.quarantine"
_WHERE = b"com.apple.metadata:kMDItemWhereFroms"
_LIMIT = {_ZONE: 4096, _WHERE: 16384}


def _bind(lib, name, result, *args):
    f = getattr(lib, name)
    f.argtypes, f.restype = args, result
    return f


def _error(what):
    return OSError(C.get_errno(), what)


class Native:
    def __init__(self):
        if sys.platform != "darwin" or C.sizeof(C.c_void_p) != 8:
            raise Rejected("仅限具有原生文件证据的 64 位 macOS")
        lib = C.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
        cf = C.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
        I, P, L, B = C.c_int, C.c_void_p, C.c_long, C.c_ubyte
        self.init_sec = _bind(lib, "filesec_init", P)
        self.free_sec = _bind(lib, "filesec_free", None, P)
        self.fstatx = _bind(lib, "fstatx_np$INODE64" if hasattr(lib, "fstatx_np$INODE64") else "fstatx_np",
                            I, I, P, P)
        self.query = _bind(lib, "filesec_query_property", I, P, I, C.POINTER(I))
        self.names = _bind(lib, "flistxattr", C.c_ssize_t, I, P, C.c_size_t, I)
        self.value = _bind(lib, "fgetxattr", C.c_ssize_t, I, C.c_char_p, P, C.c_size_t, C.c_uint32, I)
        self.clone = _bind(lib, "fclonefileat", I, I, I, C.c_char_p, C.c_uint32)
        self.make_url = _bind(cf, "CFURLCreateFromFileSystemRepresentation", P, P, C.c_char_p, L, B)
        self.make_ref = _bind(cf, "CFURLCreateFileReferenceURL", P, P, P, P)
        self.make_path = _bind(cf, "CFURLCreateFilePathURL", P, P, P, P)
        self.fsname = _bind(cf, "CFURLGetFileSystemRepresentation", B, P, B, P, L)
        self.property = _bind(cf, "CFURLCopyResourcePropertyForKey", B, P, P, C.POINTER(P), P)
        self.key = P.in_dll(cf, "kCFURLQuarantinePropertiesKey").value
        self.type = _bind(cf, "CFGetTypeID", C.c_ulong, P)
        self.dict_type = _bind(cf, "CFDictionaryGetTypeID", C.c_ulong)
        self.count = _bind(cf, "CFDictionaryGetCount", L, P)
        self.equal = _bind(cf, "CFEqual", B, P, P)
        self.retain = _bind(cf, "CFRetain", P, P)
        self.release = _bind(cf, "CFRelease", None, P)
        if not self.key: raise Rejected("系统未提供文件隔离来源查询")

    def no_acl(self, fd):
        # Do not interpret a failed acl_get_fd / filesystem lookup as 'private'.
        # Query the security object only after a successful fstatx on the actual FD.
        sec = self.init_sec()
        if not sec: raise _error("无法读取原件访问权限")
        try:
            # An aligned, oversized output buffer for Darwin LP64 struct stat;
            # this API requires it, but all regular stat fields use os.fstat.
            sb = (C.c_uint64 * 128)()
            present = C.c_int(-1)
            if self.fstatx(fd, sb, sec) or self.query(sec, 5, C.byref(present)):  # FILESEC_ACL
                raise _error("无法核实原件的扩展 ACL")
            if present.value != 0: raise Rejected("此操作不接受带扩展 ACL 的原件或目的目录")
        finally:
            self.free_sec(sec)

    def attrs(self, fd):
        # Bound _first_, not after calling listxattr/getxattr on hostile input.
        buf = C.create_string_buffer(256)
        n = self.names(fd, buf, len(buf), 0)
        if not 0 <= n <= len(buf): raise _error("无法完整取得原件的有限扩展属性清单")
        raw = buf.raw[:n]
        names = raw[:-1].split(b"\0") if raw.endswith(b"\0") else []
        if (len(names) not in (1, 2) or len(set(names)) != len(names) or
                _ZONE not in names or not set(names).issubset(_LIMIT)):
            raise Rejected("只接受有原隔离来源的归档；额外元数据或资源叉不在本次另存范围")
        values = {}
        for name in names:
            buf = C.create_string_buffer(_LIMIT[name])
            n = self.value(fd, name, buf, len(buf), 0, 0)
            if not 0 < n <= len(buf): raise Rejected("原来源属性无效、不可完整读出或超限")
            values[name] = buf.raw[:n]
        return values

    @contextmanager
    def quarantine(self, path, fd):
        """Read a fresh file-reference URL. Never serialize, log or set its dictionary."""
        bpath = os.fsencode(path)
        def match(raw):
            held = native_identity(os.fstat(fd), stat.S_IFREG)
            if native_identity(os.stat(raw, follow_symlinks=False), stat.S_IFREG) != held:
                raise Rejected("系统查询的文件不是选定原件")
        match(bpath)
        with ExitStack() as stack:
            def own(value):
                if not value: raise Rejected("系统无法将来源查询绑定到现有文件")
                stack.callback(self.release, value)
                return value
            uri = own(self.make_url(None, bpath, len(bpath), False))
            ref = own(self.make_ref(None, uri, None))
            def check_ref():
                with ExitStack() as paths:
                    named = self.make_path(None, ref, None)
                    if not named: raise Rejected("无法解析已绑定的系统文件引用")
                    paths.callback(self.release, named)
                    buf = C.create_string_buffer(4096)
                    if not self.fsname(named, True, buf, len(buf)) or not buf.value.startswith(b"/"):
                        raise Rejected("系统未返回已绑定的本地文件名")
                    match(buf.value)
                match(bpath)
            check_ref()
            prop = C.c_void_p()
            ok = self.property(ref, self.key, C.byref(prop), None)
            if prop.value: stack.callback(self.release, prop.value)
            if not ok or not prop.value or self.type(prop.value) != self.dict_type() or not 0 < self.count(prop.value) <= 64:
                raise Rejected("macOS 未返回可核实的隔离来源；不会补造来源")
            check_ref()
            yield prop.value
            check_ref()


@lru_cache(maxsize=1)
def native():
    try: return Native()
    except (OSError, AttributeError, ValueError) as err:
        raise Rejected("本系统未提供全部原生来源和权限查询；未启用备用复制方式") from err


def apfs(fd):
    """Ask the open vnode, not a mount-name lookup. Require the actual volume UUID and APFS name."""
    macos_read.resident("所选归档或目录", fd)
    _, _, attrs = macos_read._require()
    req = macos_read._Attributes(5, 0, _RET, _RET | 0x4000 | _UUID | _FSTYPE)
    buf = (C.c_uint32 * 32)()
    if attrs(fd, C.byref(req), buf, C.sizeof(buf), 4): raise _error("无法核实原 APFS 卷")
    raw = bytes(buf)
    size, ret, vol, dirs, files, forks = struct.unpack_from("=6I", raw)
    requested = 0x4000 | _UUID | _FSTYPE
    if not 52 <= size <= len(raw) or ret != _RET or vol not in (requested, _RET | requested) or dirs or files or forks:
        raise Rejected("系统未返回完整的原卷类型和身份")
    flags, = struct.unpack_from("=I", raw, 24)
    if not flags & 0x1000 or flags & (0x00200000 | 0x1):  # MNT_LOCAL; no ignored ownership/read-only mount
        raise Rejected("需要启用真实所有权的本机可写卷")
    volume = raw[28:44]
    off, n = struct.unpack_from("=iI", raw, 44)
    if not any(volume) or off < 8 or n != 5 or 44 + off + n > size or raw[44+off:44+off+n] != b"apfs\0":
        raise Rejected("只支持可确证的本地 APFS 卷")
    return os.fstat(fd).st_dev, volume


def check_spelling(fd, leaf):
    # APFS may accept different case or normalization. Keep the actual original leaf,
    # not just a spelling which currently opens that inode.
    _, _, attrs = macos_read._require()
    req = macos_read._Attributes(5, 0, _RET | 1)
    buf = (C.c_uint32 * 160)()
    if attrs(fd, C.byref(req), buf, C.sizeof(buf), 4): raise _error("无法核实实际文件名")
    raw = bytes(buf)
    size, common, vol, dirs, files, forks, off, n = struct.unpack_from("=6IiI", raw)
    expected = os.fsencode(leaf) + b"\0"
    if (not 32 <= size <= len(raw) or common != (_RET | 1) or vol or dirs or files or forks or
            off < 8 or n != len(expected) or 24+off+n > size or raw[24+off:24+off+n] != expected):
        raise Rejected("实际文件名与所选原名的拼写不一致；不更名发布")
