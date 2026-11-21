"""Native macOS evidence for whole-archive writes. Never creates or changes a label."""
from contextlib import ExitStack, contextmanager
import ctypes as C
import errno
from functools import lru_cache
import os
import stat
import sys

from . import macos_read
from .document import native_identity
from .macos_read import check_literal_path, check_spelling
from .model import Rejected

# sys/attr.h, sys/fcntl.h, sys/clonefile.h and CoreFoundation/CFURL.h.
_RET = 0x80000000
_CAPABILITIES, _EXCLUSIVE_RENAME = 0x20000, 0x80000  # sys/attr.h
_MNT_QUARANTINE = 0x400  # sys/mount.h: volume-wide quarantine
_ZONE = b"com.apple.quarantine"
_WHERE = b"com.apple.metadata:kMDItemWhereFroms"
_RESOURCE = b"com.apple.ResourceFork"
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
    def _quarantine_property(self, path, fd, directory):
        """One fresh system query on a reference to this exact held vnode."""
        bpath = os.fsencode(path)
        kind = stat.S_IFDIR if directory else stat.S_IFREG
        def match(raw):
            held = native_identity(os.fstat(fd), kind)
            if native_identity(os.stat(raw, follow_symlinks=False), kind) != held:
                raise Rejected("系统查询的文件不是选定原件")
        match(bpath)
        with ExitStack() as stack:
            def own(value):
                if not value: raise Rejected("系统无法将来源查询绑定到现有文件")
                stack.callback(self.release, value)
                return value
            uri = own(self.make_url(None, bpath, len(bpath), directory))
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
            if not ok: raise Rejected("macOS 无法确定原件的隔离来源；不会当成无标记")
            check_ref()
            yield prop.value
            check_ref()

    @contextmanager
    def quarantine(self, path, fd):
        """Read a positive quarantine value; never serialize, log or set it."""
        with self._quarantine_property(path, fd, False) as prop:
            if not prop or self.type(prop) != self.dict_type() or not 0 < self.count(prop) <= 64:
                raise Rejected("macOS 未返回可核实的隔离来源；不会补造来源")
            yield prop

    def no_payload_metadata(self, path, fd, *, directory=False):
        # A positive null from the public CFURL query is distinct from an
        # error. Also require a successful empty visible FD attribute list;
        # never inspect/copy the values or treat an unsupported query as empty.
        self.no_acl(fd)
        def empty():
            n = self.names(fd, None, 0, 0)
            if n < 0: raise _error("无法从原句柄读取扩展属性清单")
            if n: raise Rejected("所选目录内容带扩展属性、隔离标记或资源叉，不生成省略这些信息的快照")
        empty()
        if not directory:
            # A size probe reads no fork payload. A zero-length present fork
            # is still metadata. Only Darwin's explicit not-present is clean.
            n = self.value(fd, _RESOURCE, None, 0, 0, 0)
            if n >= 0: raise Rejected("原件有资源叉，不生成省略这些信息的快照")
            if C.get_errno() != getattr(errno, "ENOATTR", None):
                raise _error("无法从原句柄排除资源叉")
        with self._quarantine_property(path, fd, directory) as prop:
            if prop is not None: raise Rejected("原件有系统隔离来源，不生成独立快照")
            empty()


@lru_cache(maxsize=1)
def native():
    try: return Native()
    except (OSError, AttributeError, ValueError) as err:
        raise Rejected("本系统未提供全部原生来源和权限查询；未启用备用复制方式") from err


def apfs(fd, *, fresh_snapshot=False):
    """The existing writable APFS qualifier, including its origin restrictions."""
    volume, flags = macos_read.apfs_volume(fd)
    if flags & (0x00200000 | 0x1):  # no ignored ownership/read-only mount
        raise Rejected("需要启用真实所有权的本机可写卷")
    if fresh_snapshot and flags & _MNT_QUARANTINE:
        raise Rejected("原卷有系统卷级隔离，不能制作新的独立快照")
    return volume


def require_exclusive_rename(fd):
    # Only the actual held volume can qualify RENAME_EXCL; requesting a
    # capability, or seeing an unknown bit as zero, does not prove support.
    _, _, attrs = macos_read._require()
    req = macos_read._Attributes(5, 0, _RET, _RET | _CAPABILITIES)
    result = (C.c_uint32 * 14)()
    if attrs(fd, C.byref(req), result, C.sizeof(result), 4):
        raise _error("无法核实原卷的无覆盖改名能力")
    size, common, vol, dirs, files, forks = result[:6]
    if (size != C.sizeof(result) or common != _RET or vol not in (_CAPABILITIES, _RET | _CAPABILITIES)
            or dirs or files or forks or not (result[7] & result[11] & _EXCLUSIVE_RENAME)):
        raise Rejected("原卷未明确返回无覆盖改名能力")
