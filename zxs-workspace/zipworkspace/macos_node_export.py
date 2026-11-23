"""Publish one exact R3 file node on a qualified local APFS volume, not a ZIP or checkout."""
from contextlib import ExitStack, contextmanager
import ctypes as C
from functools import lru_cache
import hashlib
import os
from pathlib import Path
import stat
import sys
from threading import Lock

from . import macos_read as mac
from ._macos_copy_evidence import _absent, _attributes, _bind, apfs, native, require_exclusive_rename
from .container_https import HttpsExchange
from .document import archive_observation, native_identity
from .model import CHUNK, Rejected, member_name, new_id
from .publish import Uncertain, rename_new_at, require_native_writer

_CF = "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation"
_LS = "/System/Library/Frameworks/CoreServices.framework/CoreServices"
_STAGE_PREFIX = ".zipworkspace-node-"


class _DownloadQuarantine:
    """Set only a new download's Apple resource property; never touch raw xattrs."""
    def __init__(self):
        cf, ls = C.CDLL(_CF), C.CDLL(_LS)
        p, l = C.c_void_p, C.c_long
        self._url = _bind(cf, "CFURLCreateWithBytes", p, p, C.c_char_p, l, C.c_uint32, p)
        self._url_string = _bind(cf, "CFURLGetString", p, p)
        self._url_type = _bind(cf, "CFURLGetTypeID", C.c_ulong)
        self._string_type = _bind(cf, "CFStringGetTypeID", C.c_ulong)
        self._dictionary = _bind(cf, "CFDictionaryCreate", p, p, C.POINTER(p), C.POINTER(p), l, p, p)
        self._get = _bind(cf, "CFDictionaryGetValue", p, p, p)
        self._set = _bind(cf, "CFURLSetResourcePropertyForKey", C.c_ubyte, p, p, p, p)
        self._keys = C.addressof(C.c_byte.in_dll(cf, "kCFTypeDictionaryKeyCallBacks"))
        self._values = C.addressof(C.c_byte.in_dll(cf, "kCFTypeDictionaryValueCallBacks"))
        self._kind, self._data, self._download = (
            p.in_dll(ls, symbol).value for symbol in (
                "kLSQuarantineTypeKey", "kLSQuarantineDataURLKey", "kLSQuarantineTypeOtherDownload"))
        if not all((self._kind, self._data, self._download)):
            raise Rejected("macOS 未提供下载来源标记；未启用备用写入。")

    @contextmanager
    def url(self, exact):
        encoded = exact.encode("ascii")  # Canonical, credential-free URL from the same HTTPS exchange.
        value = self._url(None, encoded, len(encoded), 0x08000100, None)  # kCFStringEncodingUTF8
        if not value: raise Rejected("无法表示这次已选节点的下载来源。")
        try: yield value
        finally: native().release(value)

    def mark(self, path, fd, url):
        api = native()
        with ExitStack() as held:
            reference, check = held.enter_context(api._file_reference(path, fd, False, literal=True))
            keys = (C.c_void_p * 2)(self._kind, self._data)
            values = (C.c_void_p * 2)(self._download, url)
            attrs = self._dictionary(None, keys, values, 2, self._keys, self._values)
            if not attrs: raise Rejected("无法建立原节点的系统下载来源。")
            held.callback(api.release, attrs)
            check()
            if not self._set(reference, api.key, attrs, None):
                raise Rejected("macOS 未能为这次新文件记录下载来源。")
            check()
        # A setter may ignore unsupported properties. Only a fresh positive query
        # of the same vnode and actual supported FD attributes admits publication.
        return self.verify(path, fd, url)

    def verify(self, path, fd, url):
        api = native()
        attrs = api.attrs(fd)
        with api.quarantine(path, fd, literal=True) as observed:
            kind, value = self._get(observed, self._kind), self._get(observed, self._data)
            string_type = self._string_type()
            if not kind:
                raise Rejected("macOS 未返回下载隔离类型。")
            if api.type(kind) != string_type:
                raise Rejected("macOS 返回的下载隔离类型不是文本。")
            if not api.equal(kind, self._download):
                raise Rejected("macOS 返回了不同的下载隔离类型。")
            if not value:
                raise Rejected("macOS 保留了下载隔离类型，但未返回下载 URL。")
            if api.type(value) == self._url_type():
                value = self._url_string(value)
                if not value:
                    raise Rejected("无法读取 macOS 返回的下载 URL。")
            if api.type(value) != string_type:
                raise Rejected("macOS 返回了无法识别的下载 URL 类型。")
            expected = self._url_string(url)
            if not expected:
                raise Rejected("无法读取本次请求的下载 URL。")
            if not api.equal(value, expected):
                raise Rejected("macOS 返回的下载 URL 与本次请求不同。")
        if api.attrs(fd) != attrs:
            raise Rejected("原文件的下载来源在核验时改变。")
        return attrs


@lru_cache(maxsize=1)
def _download_quarantine():
    try: return _DownloadQuarantine()
    except (OSError, AttributeError, ValueError) as error:
        raise Rejected("macOS 缺少公开下载来源接口；未启用备用写入。") from error


class MacNodeOutput:
    """One held destination, then one explicit, exact remote-node export.

    The caller confirms the directory is private, not synced, and quiescent.
    Darwin's exclusive rename protects the target, not a hostile same-UID
    replacement of the staging name. Cancellation uses the caller's checkpoint.
    """
    def __init__(self, destination_parent, output_name):
        if sys.platform != "darwin": raise Rejected("只支持已核验的本机 macOS APFS 目的目录。")
        spelling = os.fspath(destination_parent)
        if type(spelling) is not str: raise Rejected("请选择原目的目录的准确本机名称。")
        parent = Path(spelling)
        if (str(parent) != spelling or parent.anchor != "/" or ".." in parent.parts or
                len(member_name(output_name)) != 1 or output_name.casefold().startswith(_STAGE_PREFIX)):
            raise Rejected("请选择确切现有目录和一个新的普通文件名。")
        self._path = parent / output_name
        self._stage = _STAGE_PREFIX + new_id() + ".partial"
        if len(parent.parts) >= 65 or any(len(os.fsencode(leaf)) >= 1024 for leaf in (self.path, parent / self._stage)):
            raise Rejected("目标名称无法由 macOS 原句柄逐级核对。")
        self._lock = Lock()
        self._parent, self._identity, self._volume = mac.in_disposable_thread(self._pin)

    @property
    def path(self): return self._path

    def __enter__(self): return self

    def __exit__(self, *_): self.close()

    def _pin(self):
        fd = None
        try:
            with mac.no_materialization(), mac.literal_name(self.path.parent, directory=True) as original:
                require_native_writer()
                _attributes(original, directory=True)
                volume = apfs(original, fresh_snapshot=True)
                require_exclusive_rename(original)
                _absent(original, self.path.name)
                identity = native_identity(os.fstat(original), stat.S_IFDIR)
                fd = os.dup(original)
            return fd, identity, volume
        except BaseException:
            if fd is not None: os.close(fd)
            raise

    def _take_parent(self):
        with self._lock:
            fd, self._parent = self._parent, None
        return fd

    def close(self):
        fd = self._take_parent()
        if fd is not None: os.close(fd)

    def _original_parent(self, fd):
        _attributes(fd, directory=True)
        mac.check_literal_path(self.path.parent, fd)
        if (native_identity(os.fstat(fd), stat.S_IFDIR) != self._identity
                or apfs(fd, fresh_snapshot=True) != self._volume):
            raise Rejected("原目的目录或实际 APFS 卷已改变。")
        with mac.literal_name(self.path.parent, directory=True) as named:
            if (native_identity(os.fstat(named), stat.S_IFDIR) != self._identity
                    or apfs(named, fresh_snapshot=True) != self._volume):
                raise Rejected("原目的目录名称已改变；不改用替件。")
        return fd

    def _output(self, parent, name, fd, length):
        _attributes(fd)
        mac.check_literal_path(self.path.parent / name, fd)
        mac.check_spelling(fd, name)
        observed = archive_observation(os.fstat(fd))  # Regular file identity + length + modification stamps.
        named = os.stat(name, dir_fd=parent, follow_symlinks=False)
        if (observed[2] != length or archive_observation(named) != observed or named.st_nlink != 1
                or apfs(fd, fresh_snapshot=True) != self._volume):
            raise Rejected("新文件不是所持目的卷上的原普通文件。")
        return observed

    def publish(self, exchange, container, node, *, length, sha256, confirmed=False,
                checkpoint=lambda: None, begin_publication=lambda: None,
                progress=lambda n: None, phase=lambda text, total: None):
        """Return the new path after durability and identity checks, or raise without retrying."""
        if (confirmed is not True or type(exchange) is not HttpsExchange
                or not callable(getattr(exchange._authorization, "require_live", None))):
            raise Rejected("需明确确认本次原认证连接和目的文件。")
        target = exchange._node_target(container, node, length, sha256)
        if length >= 1 << 63: raise Rejected("节点长度超出 macOS 可核验的文件偏移。")
        location, identity, authority = exchange._location, exchange.destination_identity, exchange._authorization

        def original_source():
            if (exchange._location != location or exchange.destination_identity != identity
                    or exchange._authorization is not authority):
                raise Rejected("原 HTTPS 服务器连接已改变。")
            authority.require_live()

        with ExitStack() as owned:
            parent = self._take_parent()
            if parent is None: raise Rejected("原目的目录的单次另存已结束。")
            owned.callback(os.close, parent)  # Also covers failure to start the worker.
            return mac.in_disposable_thread(self._publish, parent, owned.close, exchange, container, node,
                                            length, sha256, location + target, original_source, checkpoint,
                                            begin_publication, progress, phase)

    def _publish(self, parent, close_parent, exchange, container, node, length, sha256,
                 url, original_source, checkpoint, begin_publication, progress, phase):
        state = "uncreated"
        partial = self.path.parent / self._stage
        try:
            with ExitStack() as held:
                held.callback(close_parent)
                held.enter_context(mac.no_materialization())
                checkpoint()
                original_source()
                self._original_parent(parent)
                require_exclusive_rename(parent)
                _absent(parent, self.path.name)
                os.fsync(parent)  # Reject an unavailable parent barrier before creating anything.
                self._original_parent(parent)
                origin = _download_quarantine()
                data_url = held.enter_context(origin.url(url))
                flags = os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
                fd = os.open(self._stage, flags, 0o600, dir_fd=parent)
                held.callback(os.close, fd)
                state = "partial"
                self._output(parent, self._stage, fd, 0)
                native().no_payload_metadata(partial, fd, literal=True)
                properties = None

                def marked():
                    # Before the first body write, or after successful empty GET.
                    nonlocal properties
                    if properties is None:
                        self._output(parent, self._stage, fd, 0)
                        properties = origin.mark(partial, fd, data_url)

                def write(block):
                    checkpoint()
                    original_source()
                    marked()
                    view = memoryview(block)
                    while view:
                        checkpoint()
                        n = os.write(fd, view)
                        if not 0 < n <= len(view): raise OSError("短写；原节点没有发布。")
                        view = view[n:]
                        progress(n)
                    return len(block)

                phase("正在下载这次原节点到私有暂存文件", length)
                exchange.read_node_into(container, node, length=length, sha256=sha256, write=write)
                checkpoint()
                original_source()
                marked()
                sealed = self._output(parent, self._stage, fd, length)
                os.fsync(fd)  # Body and its actual native download mark.
                phase("正在读回私有文件，尚未发布", length)
                digest, at = hashlib.sha256(), 0
                while at < length:
                    checkpoint()
                    block = os.pread(fd, min(CHUNK, length - at), at)
                    if not block: raise Rejected("原暂存内容读回提前结束。")
                    at += len(block)
                    digest.update(block)
                    progress(len(block))
                if os.pread(fd, 1, length) or digest.hexdigest() != sha256:
                    raise Rejected("私有文件读回不等于所选原节点。")
                os.fsync(parent)
                self._original_parent(parent)
                require_exclusive_rename(parent)
                _absent(parent, self.path.name)
                if (origin.verify(partial, fd, data_url) != properties
                        or self._output(parent, self._stage, fd, length) != sealed):
                    raise Rejected("原暂存文件或实际下载来源已改变。")
                checkpoint()
                phase("正在原生发布并同步原目的目录", None)
                begin_publication()
                original_source()  # Revocation before this final admission cannot publish.
                # Caller callbacks may have taken time. Witness the original
                # parent and sealed stage again after the last callback.
                self._original_parent(parent)
                if self._output(parent, self._stage, fd, length) != sealed:
                    raise Rejected("开始发布前，原暂存文件已改变。")
                state = "rename"
                try: rename_new_at(parent, self._stage, self.path.name)
                except FileExistsError as error:
                    state = "partial"
                    raise Rejected("正式名称后来被占用，没有覆盖。") from error
                published = self._output(parent, self.path.name, fd, length)
                if published[:4] != sealed[:4]:  # The original file's ctime can change on rename.
                    raise Rejected("正式名称不是原核验文件。")
                os.fsync(parent)
                self._original_parent(parent)
                if (origin.verify(self.path, fd, data_url) != properties
                        or self._output(parent, self.path.name, fd, length) != published):
                    raise Rejected("正式文件或实际下载来源无法确认。")
                _absent(parent, self._stage)
            return self.path
        except Exception as error:
            if state == "rename":
                raise Uncertain(f"正式名称可能已出现，无法确认：{str(self.path)!r}。先检查原位置，不重试或删除。") from error
            if state == "partial":
                raise Rejected(f"没有发布；私有暂存可能保留：{str(partial)!r}。未自动删除。{error}") from error
            raise
