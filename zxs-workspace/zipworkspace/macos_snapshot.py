"""macFUSE read callbacks on an already selected .zxs. This module cannot create a mount."""
from contextlib import contextmanager
import errno
import io
import os
import stat
import sys
from threading import RLock
from unicodedata import normalize

from . import macos_read
from .archive import stamp
from .document import archive_sha, native_identity
from .model import MAX_DEPTH, Rejected
from .snapshot_view import ROOT, SnapshotView


class _ReadCursor(io.RawIOBase):
    """Own position, not the archive FD: dup() shares the display's kernel file offset."""
    def __init__(self, fd, end):
        super().__init__()
        self.fd, self.end, self.pos = fd, end, 0

    def fileno(self): return self.fd
    def readable(self): return True
    def seekable(self): return True
    def tell(self): return self.pos

    def seek(self, offset, whence=os.SEEK_SET):
        if whence == os.SEEK_CUR: offset += self.pos
        elif whence == os.SEEK_END: offset += self.end
        elif whence != os.SEEK_SET: raise OSError(errno.EINVAL, "invalid seek")
        if offset < 0: raise OSError(errno.EINVAL, "negative archive position")
        self.pos = offset
        return offset

    def readinto(self, buf):
        while True:
            try:
                part = os.pread(self.fd, len(buf), self.pos)
                break
            except InterruptedError:
                pass
        buf[:len(part)] = part
        self.pos += len(part)
        return len(part)


def _components(path):
    # Finder exchanges decomposed UTF-8. ZIP spelling remains canonical NFC and case sensitive.
    if not isinstance(path, str) or not path.startswith("/") or len(path) > 1024 * MAX_DEPTH + MAX_DEPTH:
        raise OSError(errno.EINVAL, "not a bounded absolute virtual path")
    if path == "/": return ()
    parts = path[1:].split("/", MAX_DEPTH)
    if len(parts) > MAX_DEPTH: raise OSError(errno.ENAMETOOLONG, "virtual path too deep")
    result = []
    for part in parts:
        if part in ("", ".", ".."): raise OSError(errno.EINVAL, "not a virtual member")
        try:
            if len(part.encode("utf-8")) > 1024: raise OSError(errno.ENAMETOOLONG, "long Finder component")
            name = normalize("NFC", part).encode("utf-8")
        except UnicodeError:
            raise OSError(errno.EILSEQ, "invalid UTF-8 name") from None
        if len(name) > 255: raise OSError(errno.ENAMETOOLONG, "long normalized component")
        result.append(name)
    if sum(map(len, result)) + len(result) - 1 > 1024: raise OSError(errno.ENAMETOOLONG, "long archive name")
    return result


class _Callbacks:
    """mfusepy 3.1.1, Darwin libfuse2, raw_fi=True. No mount, write or native loader."""
    use_ns = True

    def __init__(self, view, uid, gid):
        self.view, self.uid, self.gid = view, uid, gid
        self.lock = RLock()
        self.closed = False

    def close(self):
        with self.lock: self.closed = True

    def _health(self):
        try:
            self.view.check()
            macos_read.resident("原归档", self.view.document.fileno())
        except (OSError, Rejected) as err:
            self.view.failed = True
            raise OSError(errno.EIO, "the selected resident archive is unavailable") from err

    @contextmanager
    def _request(self):
        with self.lock:  # never close/reuse the FD while a callback is reading it
            if self.closed or self.view.failed: raise OSError(errno.EIO, "snapshot closed or changed")
            try:
                with macos_read.no_materialization():  # policy belongs to the calling native thread
                    self._health()
                    yield
                    self._health()
            except Rejected as err:
                self.view.failed = True
                raise OSError(errno.EIO, "cannot preserve the archive read policy") from err

    def _inode(self, path, handle=None):
        ino = ROOT
        if path is not None:
            for name in _components(path): ino = self.view.lookup(ino, name)
        if handle is not None:
            self.view.node(handle)
            if path is not None and ino != handle: raise OSError(errno.EBADF, "handle and virtual name differ")
            return handle
        if path is None: raise OSError(errno.EBADF, "missing name and handle")
        return ino

    def _stat(self, ino):
        n = self.view.node(ino)
        directory = n.children is not None
        return {"st_ino": ino, "st_mode": stat.S_IFDIR | 0o500 if directory else stat.S_IFREG | 0o400,
                "st_uid": self.uid, "st_gid": self.gid, "st_size": n.size,
                "st_nlink": 2 if directory else 1, "st_blksize": 4096}

    def getattr(self, path, fh=None):
        with self._request(): return self._stat(self._inode(path, None if fh is None else fh.fh))

    def access(self, path, mode):
        with self._request():
            node = self.view.node(self._inode(path))
            if mode & os.W_OK: raise OSError(errno.EROFS, "read-only snapshot")
            if mode & ~(os.R_OK | os.X_OK) or mode & os.X_OK and node.children is None:
                raise OSError(errno.EACCES, "regular members are not executable")
            return 0

    def open(self, path, fi):
        with self._request():
            if fi.flags & os.O_ACCMODE != os.O_RDONLY or fi.flags & (os.O_APPEND | os.O_TRUNC | os.O_CREAT):
                raise OSError(errno.EROFS, "read-only snapshot")
            if fi.flags & (getattr(os, "O_EXEC", 0) | getattr(os, "O_EXLOCK", 0)):
                raise OSError(errno.EACCES, "execute/lock opens are unsupported")
            ino = self._inode(path)
            if self.view.node(ino).children is not None: raise OSError(errno.EISDIR, "not a regular file")
            if fi.flags & getattr(os, "O_DIRECTORY", 0): raise OSError(errno.ENOTDIR, "not a directory")
        fi.fh, fi.direct_io, fi.keep_cache = ino, 1, 0
        return 0

    def read(self, path, size, offset, fi):
        with self._request():
            # Only bytes sliced from the same fully rehashed blocks may leave this callback.
            return self.view.read(self._inode(path, fi.fh), offset, size)

    def opendir(self, path):
        with self._request():
            ino = self._inode(path)
            if self.view.node(ino).children is None: raise OSError(errno.ENOTDIR, "not a directory")
            return ino  # mfusepy always passes integer directory handles, even in raw_fi mode.

    def readdir_with_offset(self, path, offset, fh):
        with self._request():
            node = self.view.node(self._inode(path, fh))
            if node.children is None: raise OSError(errno.ENOTDIR, "not a directory")
            children = node.children
            if not 0 <= offset <= len(children) + 2: raise OSError(errno.EINVAL, "invalid continuation")
        # Only immutable, admitted metadata escapes the FD lock; never leave the thread's I/O
        # policy changed while yielding to the native filler. A full page retries its rejected row.
        for pos in range(offset, len(children) + 2):
            if self.closed or self.view.failed: raise OSError(errno.EIO, "snapshot closed or changed")
            if pos == 0: name, ino = ".", fh
            elif pos == 1: name, ino = "..", node.parent
            else:
                raw, ino = children[pos-2]
                name = normalize("NFD", raw.decode("utf-8"))
            yield name, self._stat(ino), pos + 1

    def statfs(self, path):
        with self._request():
            self._inode(path)
            return {"f_bsize": 4096, "f_frsize": 4096, "f_blocks": (self.view.total + 4095)//4096,
                    "f_bfree": 0, "f_bavail": 0, "f_files": len(self.view.nodes), "f_ffree": 0,
                    "f_favail": 0, "f_namemax": 255}

    def release(self, path, fi): return 0   # immutable inode IDs acquire no per-open resources
    def releasedir(self, path, fh): return 0
    def _readonly(self, *args): raise OSError(errno.EROFS, "read-only snapshot")
    write = truncate = create = unlink = rename = chmod = chown = utimens = _readonly
    mknod = mkdir = rmdir = link = symlink = setxattr = removexattr = fallocate = _readonly


@contextmanager
def selected_callbacks(selection):
    """Borrow a live Selection made with retain=True; never discover a source by its pathname."""
    if sys.platform != "darwin": raise Rejected("this read adapter requires macOS")
    import fcntl
    uid, gid = os.getuid(), os.getgid()
    if not uid or uid != os.geteuid() or gid != os.getegid(): raise Rejected("ordinary user context required")
    original = selection.held
    if original is None: raise Rejected("the selected file was not retained; reselect it")
    try: fd = original.duplicate()  # atomic with display release; this duplicate then owns the actual inode
    except ValueError as err: raise Rejected("the selected original was already released") from err
    try:
        with io.BufferedReader(_ReadCursor(fd, selection.observed[2])) as doc:
            with macos_read.no_materialization():
                if fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_ACCMODE != os.O_RDONLY:
                    raise Rejected("the retained original must be read-only")
                native_identity(os.fstat(fd), stat.S_IFREG)
                if stamp(doc) != selection.observed: raise Rejected("the selected original changed")
                macos_read.local_object("原归档", fd)
                view = SnapshotView(doc, display_name=selection.path.name)
                exact = selection.exact
                if view.original != selection.observed or view.commit != selection.browser.commit or (exact is not None and
                        (exact.path != selection.path or exact.observed != selection.observed or
                         exact.marker != view.commit or archive_sha(doc) != exact.sha256)):
                    raise Rejected("the retained original does not match the selected snapshot")
                macos_read.resident("原归档", fd)
                view.check()
            callbacks = _Callbacks(view, uid, gid)
            try: yield callbacks
            finally: callbacks.close()
    finally: os.close(fd)
