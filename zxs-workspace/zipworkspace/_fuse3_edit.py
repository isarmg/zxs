"""Linux editable-WORK FUSE3 callbacks and broker-only dispatch."""
from contextlib import contextmanager
import ctypes
import errno
import os
import stat
from threading import Lock

import pyfuse3
import trio

from .linux_edit import ROOT
from .linux_edit_mount import FUSE_VERSION
from .model import Rejected


_NATIVE = Lock()
_NATIVE_UNCERTAIN = False


@contextmanager
def _kernel_errors():
    try:
        yield
    except OSError as err:
        raise pyfuse3.FUSEError(err.errno or errno.EIO) from None
    except Rejected:
        raise pyfuse3.FUSEError(errno.EIO) from None


class EditOperations(pyfuse3.Operations):
    # Never rely on these switches alone to prove an actual native mount drained.
    enable_writeback_cache = False
    enable_acl = False
    supports_dot_lookup = True

    def __init__(self, working):
        super().__init__()
        if pyfuse3.ROOT_INODE != ROOT:
            raise Rejected("unexpected FUSE root identity")
        self.working = working

    def _caller(self, ctx):
        if ctx is None or ctx.uid != self.working.uid:
            raise pyfuse3.FUSEError(errno.EACCES)

    def _attr(self, entry):
        ino, meta = entry
        result = pyfuse3.EntryAttributes()
        result.st_ino = ino
        result.st_uid, result.st_gid = self.working.uid, self.working.gid
        permitted = 0o700 if stat.S_ISDIR(meta.st_mode) else 0o600
        result.st_mode = stat.S_IFMT(meta.st_mode) | (meta.st_mode & permitted)
        result.st_size, result.st_nlink = meta.st_size, meta.st_nlink
        result.st_atime_ns, result.st_mtime_ns, result.st_ctime_ns = meta.st_atime_ns, meta.st_mtime_ns, meta.st_ctime_ns
        result.st_blksize, result.st_blocks = meta.st_blksize, meta.st_blocks
        result.entry_timeout = result.attr_timeout = 0
        return result

    @staticmethod
    def _file_info(fh):
        result = pyfuse3.FileInfo(fh=fh)
        result.direct_io, result.keep_cache = True, False
        return result

    async def lookup(self, parent_inode, name, ctx):
        self._caller(ctx)
        with _kernel_errors(): return self._attr(self.working.lookup(parent_inode, name))

    async def getattr(self, inode, ctx=None):
        self._caller(ctx)
        with _kernel_errors(): return self._attr(self.working.getattr(inode))

    async def forget(self, inode_list):
        self.working.forget(inode_list)  # A kernel lifetime notification, not a new admission.

    async def opendir(self, inode, ctx):
        self._caller(ctx)
        with _kernel_errors(): return self.working.opendir(inode)

    async def readdir(self, fh, start_id, token):
        def reply(name, entry, cookie):
            return pyfuse3.readdir_reply(token, name, self._attr(entry), cookie)
        with _kernel_errors(): self.working.readdir(fh, start_id, reply)

    async def releasedir(self, fh):
        with _kernel_errors(): self.working.releasedir(fh)

    async def open(self, inode, flags, ctx):
        self._caller(ctx)
        with _kernel_errors(): return self._file_info(self.working.open(inode, flags))

    async def create(self, parent_inode, name, mode, flags, ctx):
        self._caller(ctx)
        with _kernel_errors():
            fh, entry = self.working.create(parent_inode, name, mode, flags)
            return self._file_info(fh), self._attr(entry)

    async def rename(self, parent_inode_old, name_old, parent_inode_new, name_new, flags, ctx):
        self._caller(ctx)
        if flags not in (0, pyfuse3.RENAME_NOREPLACE):
            raise pyfuse3.FUSEError(errno.EOPNOTSUPP)
        with _kernel_errors():
            self.working.rename(parent_inode_old, name_old, parent_inode_new, name_new,
                                no_replace=flags == pyfuse3.RENAME_NOREPLACE)

    async def unlink(self, parent_inode, name, ctx):
        self._caller(ctx)
        with _kernel_errors(): self.working.unlink(parent_inode, name)

    async def read(self, fh, off, size):
        with _kernel_errors(): return self.working.read(fh, off, size)

    async def write(self, fh, off, buf):
        with _kernel_errors(): return self.working.write(fh, off, buf)

    async def setattr(self, inode, attr, fields, fh, ctx):
        self._caller(ctx)
        if (not fields.update_size or any(getattr(fields, field, False) for field in
                ("update_mode", "update_uid", "update_gid", "update_atime", "update_mtime", "update_ctime"))):
            raise pyfuse3.FUSEError(errno.EOPNOTSUPP)
        with _kernel_errors(): return self._attr(self.working.truncate(inode, attr.st_size, fh))

    async def flush(self, fh):
        with _kernel_errors(): self.working.flush(fh)

    async def fsync(self, fh, datasync):
        with _kernel_errors(): self.working.fsync(fh)  # The full native barrier also satisfies datasync.

    async def release(self, fh):
        with _kernel_errors(): self.working.release(fh)

    async def _not_supported(self, *args, **kwargs):
        raise pyfuse3.FUSEError(errno.EOPNOTSUPP)

    mkdir = rmdir = mknod = symlink = link = _not_supported
    setxattr = removexattr = fallocate = _not_supported


def qualify_edit_driver():
    # For this profile libfuse has no default direct-I/O shared-mmap or
    # writeback flag and pyfuse3 opts in to neither. No unknown driver fallback.
    if pyfuse3.__version__ != "3.5.0":
        raise Rejected("尚未核准此可编辑卷的 pyfuse3 版本")
    try:
        version = ctypes.CDLL(pyfuse3.__file__, mode=os.RTLD_NOLOAD).fuse_pkgversion
        version.argtypes = ()
        version.restype = ctypes.c_char_p
        if version() != FUSE_VERSION:
            raise Rejected("实际绑定的 libfuse 不属于核准的 I/O 接口")
    except (AttributeError, OSError) as err:
        raise Rejected("无法核对实际绑定的 libfuse") from err


def serve_edit(working, mount_original, source, before, accepted, ended):
    """Blocking single-session dispatch, borrowing exactly the helper's FUSE FD."""

    global _NATIVE_UNCERTAIN
    if not _NATIVE.acquire(blocking=False):
        raise Rejected("another editable dispatcher already owns this process")
    try:
        if _NATIVE_UNCERTAIN:
            raise Rejected("the previous native attempt is unconfirmed; no retry")
        ops = EditOperations(working)
        before()
        _NATIVE_UNCERTAIN = True  # Before the one authorized native mount request.
        try:
            fd = mount_original()
            try:
                # Qualified libfuse adopts this descriptor; it does not resolve
                # a mountpoint again. Returned init transfers closing to libfuse.
                pyfuse3.init(ops, f"/dev/fd/{fd}")
            except Exception:
                os.close(fd)
                raise
            try:
                accepted()  # Prove exact native mount and restrictions before serving requests.
                trio.run(pyfuse3.main, 1, 4)
            finally:
                pyfuse3.close(unmount=False)
            result = ended()  # Main returned, workers joined, native close returned; still prove detach.
        except Exception as err:
            raise Rejected(f"挂载已尝试，原卷 {source} 的收尾未确认；不会自动重挂或卸载：{err}") from err
        _NATIVE_UNCERTAIN = False
        return result
    finally:
        _NATIVE.release()
