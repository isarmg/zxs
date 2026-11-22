"""Optional Linux FUSE3 adapter for the explicit foreground and desktop mount paths."""
from contextlib import contextmanager
import errno
import os
import stat

import pyfuse3
import trio

from .linux_mount import mount_source
from .snapshot_view import ROOT


@contextmanager
def kernel_errors():
    try:
        yield
    except OSError as err:
        raise pyfuse3.FUSEError(err.errno or errno.EIO) from None


class Operations(pyfuse3.Operations):
    enable_writeback_cache = False
    enable_acl = False
    supports_dot_lookup = True

    def __init__(self, view):
        super().__init__()
        self.view = view
        self.uid, self.gid = os.getuid(), os.getgid()

    def _attr(self, ino):
        node = self.view.node(ino)
        d = node.children is not None
        a = pyfuse3.EntryAttributes()
        a.st_ino, a.st_uid, a.st_gid = ino, self.uid, self.gid
        a.st_mode = (stat.S_IFDIR | 0o500) if d else (stat.S_IFREG | 0o400)
        a.st_size, a.st_nlink = node.size, node.nlink
        a.st_blksize, a.st_blocks = 4096, (a.st_size+511)//512
        a.st_atime_ns = a.st_mtime_ns = a.st_ctime_ns = 0
        a.entry_timeout = a.attr_timeout = 0
        return a

    async def lookup(self, parent_inode, name, ctx):
        with kernel_errors():
            self.view.check()
            return self._attr(self.view.lookup(parent_inode, name))

    async def getattr(self, inode, ctx=None):
        with kernel_errors():
            self.view.check()
            return self._attr(inode)

    async def opendir(self, inode, ctx):
        with kernel_errors():
            self.view.check()
            if self.view.node(inode).children is None: raise pyfuse3.FUSEError(errno.ENOTDIR)
            return inode

    async def readdir(self, fh, start_id, token):
        with kernel_errors():
            self.view.check()
            children = self.view.node(fh).children
            if children is None: raise pyfuse3.FUSEError(errno.ENOTDIR)
            if not 0 <= start_id <= len(children): raise pyfuse3.FUSEError(errno.EINVAL)
            for i in range(start_id, len(children)):
                name, ino = children[i]
                if not pyfuse3.readdir_reply(token, name, self._attr(ino), i + 1):
                    break

    async def open(self, inode, flags, ctx):
        if flags & os.O_ACCMODE != os.O_RDONLY or flags & (os.O_CREAT | os.O_TRUNC | os.O_APPEND):
            raise pyfuse3.FUSEError(errno.EROFS)
        with kernel_errors():
            self.view.check()
            if self.view.node(inode).children is not None: raise pyfuse3.FUSEError(errno.EISDIR)
        info = pyfuse3.FileInfo(fh=inode)
        info.direct_io, info.keep_cache = True, False
        return info

    async def read(self, fh, off, size):
        with kernel_errors():
            # run_sync waits for its worker even on cancellation. Dispatch is bounded by main(max_tasks).
            return await trio.to_thread.run_sync(self.view.read, fh, off, size)

    async def statfs(self, ctx):
        with kernel_errors(): self.view.check()
        s = pyfuse3.StatvfsData()
        s.f_bsize = s.f_frsize = 4096
        s.f_blocks = (self.view.total+4095)//4096
        s.f_bfree = s.f_bavail = s.f_ffree = s.f_favail = 0
        s.f_files, s.f_namemax = len(self.view.nodes), 240
        return s

    # Every inode and directory cookie lives for this fixed snapshot. Opens/lookup references
    # acquire no host FDs; nothing may be reclaimed/reused before the whole session ends.
    async def forget(self, inode_list): pass
    async def release(self, fh): pass
    async def releasedir(self, fh): pass
    async def flush(self, fh): pass

    async def _readonly(self, *args, **kwargs):
        raise pyfuse3.FUSEError(errno.EROFS)

    write = setattr = create = mknod = mkdir = unlink = rmdir = symlink = rename = link = _readonly
    setxattr = removexattr = fallocate = _readonly


def serve(view, point, last_check, *, token, accepted):
    if pyfuse3.ROOT_INODE != ROOT:
        raise RuntimeError("unsupported FUSE root inode")
    ops = Operations(view)
    options = {"ro", "nodev", "nosuid", "noexec", "default_permissions", "fsname=" + mount_source(token)}
    last_check()
    pyfuse3.init(ops, str(point), options)
    try:
        accepted()  # pyfuse3.init has returned; only the caller's table observation can identify the mount.
        trio.run(pyfuse3.main, 1, 4)
    finally:
        # Never unmount by a possibly reused pathname. A normal, external unmount ends main().
        pyfuse3.close(unmount=False)
