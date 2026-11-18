"""A closed matching temporary sibling is the ONLY file eligible to publish."""
from __future__ import annotations
import ctypes
import errno
import hashlib
import os
from pathlib import Path
import stat
import sys
import tempfile
from .archive import read_archive, stamp
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


def rename_new(source: Path, target: Path):
    require_native_writer()
    if source.parent != target.parent:
        raise Rejected("atomic publication requires a same-directory temporary sibling")
    fd = os.open(source.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        libc = ctypes.CDLL(None, use_errno=True)
        if sys.platform.startswith("linux") and hasattr(libc, "renameat2"):
            fn, flags = libc.renameat2, 1  # Linux RENAME_NOREPLACE
        elif sys.platform == "darwin" and hasattr(libc, "renameatx_np"):
            fn, flags = libc.renameatx_np, 4  # Darwin RENAME_EXCL
        else: raise Rejected("native no-replace rename not available; the old version is untouched")
        fn.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
        fn.restype = ctypes.c_int
        if fn(fd, os.fsencode(source.name), fd, os.fsencode(target.name), flags) != 0:
            code = ctypes.get_errno()
            raise OSError(code, os.strerror(code))
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


def confirm_final(target: Path, marker: Commit, archive_sha: str):
    try:
        if digest_file(target) != archive_sha: raise Conflict("branch slot already contains a different archive; candidate retained locally")
    except FileNotFoundError: return False
    found, _ = read_archive(target)
    if found != marker or digest_file(target) != archive_sha:
        raise Conflict("a different archive occupies this immutable commit slot")
    sync_dir(target.parent)
    return True


def publish(local: Path, store: Path, marker: Commit, archive_sha: str):
    target = store / marker.filename
    if confirm_final(target, marker, archive_sha): return target
    if digest_file(local) != archive_sha or read_archive(local, internal=True)[0] != marker:
        raise Rejected("saved local candidate no longer matches the original operation")
    fd, name = tempfile.mkstemp(prefix=".zip-workspace-", suffix=".partial", dir=store)
    stage = Path(name)
    # No speculative path deletion on failure: if an outside writer replaced
    # a temporary name it must not be removed as if it were ours.
    with os.fdopen(fd, "wb") as out, local.open("rb") as src:
        original = os.fstat(out.fileno())
        while block := src.read(CHUNK): out.write(block)
        out.flush(); os.fsync(out.fileno())
    # Reopen after the uploader has completely closed the file. Keep the exact
    # uploaded inode alive throughout the no-replace syscall and final check.
    heldfd = os.open(stage, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        held = os.fstat(heldfd)
        if not os.path.samestat(original, held):
            raise Rejected("temporary upload name was replaced before verification")
        if digest_file(stage) != archive_sha or read_archive(stage, internal=True)[0] != marker:
            raise Rejected("temporary upload does not reproduce the frozen snapshot")
        sync_dir(store)
        if not os.path.samestat(held, os.stat(stage, follow_symlinks=False)):
            raise Rejected("temporary name no longer identifies the uploaded file")
        try:
            rename_new(stage, target)
        except OSError as err:
            # A lost reply is not evidence of failure. Durable logical identity
            # is the original operation/marker and the exact entire ZIP digest.
            if confirm_final(target, marker, archive_sha): return target
            if err.errno in (errno.EOPNOTSUPP, errno.ENOSYS, errno.EINVAL):
                raise Rejected("destination does not support native atomic no-replace rename") from err
            raise Uncertain("publication outcome unknown; retry only the recorded local operation") from err
        if not os.path.samestat(held, os.stat(target, follow_symlinks=False)) or not confirm_final(target, marker, archive_sha):
            raise Uncertain("published name changed; inspect only the original operation")
        return target
    finally:
        os.close(heldfd)
