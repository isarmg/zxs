"""A closed matching temporary sibling is the ONLY file eligible to publish."""
from __future__ import annotations
from contextlib import ExitStack, contextmanager
import ctypes
import errno
import hashlib
import os
from pathlib import Path
import stat
import sys
import tempfile
from .archive import read_archive, read_managed_document, stamp
from .document import archive_sha as hash_document, native_identity
from .model import CHUNK, MAX_TOTAL, MAX_CD, Commit, Rejected


class Conflict(Rejected): pass
class Uncertain(Rejected): pass


def sync_dir(directory: Path):
    if os.name != "posix": raise Rejected("provider directory durability is not verified on this platform")
    fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try: os.fsync(fd)
    finally: os.close(fd)


def require_native_writer():
    if os.name != "posix" or not hasattr(os, "O_NOFOLLOW") or not hasattr(os, "O_DIRECTORY") or os.open not in os.supports_dir_fd:
        raise Rejected("this platform has no admitted native workspace writer; no files or settings were changed")
    libc = ctypes.CDLL(None, use_errno=True)
    if sys.platform.startswith("linux") and hasattr(libc,"renameat2"): return
    if sys.platform == "darwin" and hasattr(libc,"renameatx_np"): return
    raise Rejected("native no-replace rename unavailable; no files or settings were changed")


def rename_new_at(fd, source, target, *, expected_source=None, target_fd=None):
    require_native_writer()
    if any(not n or n in (".","..") or Path(n).name != n for n in (source,target)):
        raise Rejected("expected single directory entry names")
    if expected_source is not None:
        actual = os.stat(source,dir_fd=fd,follow_symlinks=False)
        if not stat.S_ISDIR(actual.st_mode) or not os.path.samestat(actual,expected_source):
            raise Rejected("original staging directory was replaced; nothing was moved")
    libc = ctypes.CDLL(None, use_errno=True)
    if sys.platform.startswith("linux") and hasattr(libc, "renameat2"):
        fn, flags = libc.renameat2, 1  # Linux RENAME_NOREPLACE
    elif sys.platform == "darwin" and hasattr(libc, "renameatx_np"):
        fn, flags = libc.renameatx_np, 4  # Darwin RENAME_EXCL
    else: raise Rejected("native no-replace rename not available; the old version is untouched")
    fn.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    fn.restype = ctypes.c_int
    if fn(fd, os.fsencode(source), fd if target_fd is None else target_fd, os.fsencode(target), flags) != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code))


def rename_new(source: Path, target: Path):
    require_native_writer()
    if source.parent != target.parent:
        raise Rejected("atomic publication requires a same-directory temporary sibling")
    fd = os.open(source.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try: rename_new_at(fd, source.name, target.name)
    finally: os.close(fd)


def digest_file(path: Path):
    if not hasattr(os, "O_NOFOLLOW") or not hasattr(os, "O_NONBLOCK"):
        raise Rejected("exact named archive readback is not available on this platform")
    h = hashlib.sha256()
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
    with os.fdopen(fd, "rb") as f:
        meta = os.fstat(f.fileno())
        if not stat.S_ISREG(meta.st_mode) or meta.st_size > MAX_TOTAL + 2 * MAX_CD + (1 << 20):
            raise Rejected("expected a bounded ordinary archive file")
        before = stamp(f)
        while data := f.read(CHUNK): h.update(data)
        if before != stamp(f): raise Rejected("archive changed during digest")
        if not os.path.samestat(os.fstat(f.fileno()), os.stat(path, follow_symlinks=False)):
            raise Rejected("named archive rebound")
    return h.hexdigest()


class StoreReadback:
    """An observation of one currently selected store. Not a persistent store registration."""

    def __init__(self, path, fd):
        self.path, self.fd = path, fd
        self.identity = native_identity(os.fstat(fd), stat.S_IFDIR)

    def check_store(self):
        try:
            named = self.path.stat(follow_symlinks=False)
        except FileNotFoundError as err:
            raise Uncertain("the selected archive store disappeared; original outcome unknown") from err
        if native_identity(named, stat.S_IFDIR) != self.identity:
            raise Uncertain("the selected archive store changed; original outcome unknown")

    def confirm(self, marker, expected_sha, *, uploaded_fd=None):
        self.check_store()
        name = marker.filename
        flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
        try:
            fd = os.open(name, flags, dir_fd=self.fd)
        except FileNotFoundError as err:
            # Only this openat can report an absent slot, never a late missing
            # name or an absent STORE. Absence does not reserve the slot.
            self.check_store()
            if uploaded_fd is not None:
                raise Uncertain("published slot is unavailable after the original rename") from err
            return False
        try:
            with os.fdopen(fd, "rb", closefd=False) as doc:
                identity = native_identity(os.fstat(fd), stat.S_IFREG)
                if uploaded_fd is not None and native_identity(os.fstat(uploaded_fd), stat.S_IFREG) != identity:
                    raise Uncertain("published slot is not the inode whose upload just completed")
                observed = stamp(doc)  # Post-rename: rename itself can change ctime.

                def check_slot():
                    try:
                        named = os.stat(name, dir_fd=self.fd, follow_symlinks=False)
                    except FileNotFoundError as err:
                        raise Uncertain("the original archive slot disappeared during readback") from err
                    if (native_identity(named, stat.S_IFREG) != identity
                            or (named.st_size, named.st_mtime_ns, named.st_ctime_ns) != observed[2:]
                            or stamp(doc) != observed):
                        raise Uncertain("different archive or generation in the original slot")
                    self.check_store()

                check_slot()
                actual_sha = hash_document(doc)
                check_slot()
                if actual_sha != expected_sha:
                    raise Conflict("branch slot contains different bytes; candidate retained locally")
                actual, _ = read_managed_document(doc, display_name=name)
                check_slot()
                if actual != marker:
                    raise Conflict("another commit occupies this immutable slot")
                # Flush the actual verified file and its actual directory. A
                # failure (including unsupported provider durability) is not success.
                os.fsync(fd)
                os.fsync(self.fd)
                check_slot()
                return True
        finally:
            os.close(fd)


@contextmanager
def held_store(path):
    if (os.name != "posix" or not all(hasattr(os, flag) for flag in
            ("O_DIRECTORY", "O_NOFOLLOW", "O_NONBLOCK", "O_CLOEXEC"))
            or os.open not in os.supports_dir_fd or os.stat not in os.supports_dir_fd):
        raise Rejected("cannot verify a named archive beneath its original store on this platform")
    path = Path(path).absolute()
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        store = StoreReadback(path, fd)
        store.check_store()
        yield store
    finally:
        os.close(fd)


def confirm_final(target: Path, marker: Commit, archive_sha: str):
    if target.name != marker.filename:
        raise Rejected("the requested name is not the original branch slot")
    with held_store(target.parent) as store:
        return store.confirm(marker, archive_sha)


def publish(local: Path, store: Path, marker: Commit, archive_sha: str):
    with held_store(store) as scope:
        return _publish(local, scope, marker, archive_sha)


@contextmanager
def _pin_upload(scope, name, writer):
    """Transfer identity from the live writable upload to an independent read-only handle."""
    scope.check_store()
    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=scope.fd)
    try:
        if native_identity(os.fstat(fd), stat.S_IFREG) != native_identity(os.fstat(writer.fileno()), stat.S_IFREG):
            raise Uncertain("temporary upload name changed before its writer closed")
        yield fd
    finally:
        os.close(fd)


def _publish(local, scope, marker, archive_sha):
    store = scope.path
    target = store / marker.filename
    if scope.confirm(marker, archive_sha): return target
    if digest_file(local) != archive_sha or read_archive(local, internal=True)[0] != marker:
        raise Rejected("saved local candidate no longer matches the original operation")
    # No path deletion or replacement if a temporary entry is lost. Pin the
    # independent read FD while the actual writer is still alive, then close
    # ALL writable handles before checking even the first ZIP byte for publication.
    with ExitStack() as reader:
        with ExitStack() as writer:
            fd, name = tempfile.mkstemp(prefix=".zip-workspace-", suffix=".partial", dir=store)
            writer.callback(os.close, fd)
            stage = Path(name)
            out = writer.enter_context(os.fdopen(fd, "wb", closefd=False))
            with local.open("rb") as src:
                while block := src.read(CHUNK): out.write(block)
            out.flush(); os.fsync(fd)
            heldfd = reader.enter_context(_pin_upload(scope, stage.name, out))
        with os.fdopen(heldfd, "rb", closefd=False) as uploaded:
            observed = stamp(uploaded)
            if (hash_document(uploaded) != archive_sha
                    or read_managed_document(uploaded, display_name=marker.filename)[0] != marker
                    or stamp(uploaded) != observed):
                raise Rejected("temporary upload does not reproduce the frozen snapshot")
        scope.check_store()
        os.fsync(scope.fd)
        scope.check_store()
        named = os.stat(stage.name, dir_fd=scope.fd, follow_symlinks=False)
        if (native_identity(named, stat.S_IFREG) + (named.st_size, named.st_mtime_ns, named.st_ctime_ns)
                != observed):
            raise Rejected("temporary name no longer identifies the uploaded file")
        try:
            rename_new(stage, target)
        except OSError as err:
            # A lost reply is not evidence of failure. Durable logical identity
            # is the original operation/marker and the exact entire ZIP digest.
            if scope.confirm(marker, archive_sha): return target
            if err.errno in (errno.EOPNOTSUPP, errno.ENOSYS, errno.EINVAL):
                raise Rejected("destination does not support native atomic no-replace rename") from err
            raise Uncertain("publication outcome unknown; retry only the recorded local operation") from err
        scope.confirm(marker, archive_sha, uploaded_fd=heldfd)
        return target
