"""Directories and exclusive member creation anchored to one selected native parent."""
from contextlib import contextmanager
import os
from pathlib import Path
import stat

from .document import native_identity
from .model import Rejected, member_name


def _flags():
    return os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC


def _leaf(name):
    if not name or name in (".", "..") or Path(name).name != name:
        raise Rejected("expected one native directory entry")
    return name


class Parent:
    def __init__(self, path, fd, original):
        self.path, self.fd, self.original = path, fd, original

    def check(self):
        if native_identity(self.path.stat(follow_symlinks=False), stat.S_IFDIR) != self.original:
            raise Rejected("selected parent no longer names the original directory")

    def child(self, name):
        try: meta = os.stat(_leaf(name), dir_fd=self.fd, follow_symlinks=False)
        except FileNotFoundError: return None
        return native_identity(meta, stat.S_IFDIR)

    def occupied(self, name):
        try: os.stat(_leaf(name), dir_fd=self.fd, follow_symlinks=False)
        except FileNotFoundError: return False
        return True

    @contextmanager
    def hold(self, name, expected):
        fd = os.open(_leaf(name), _flags(), dir_fd=self.fd)
        try:
            identity = native_identity(os.fstat(fd), stat.S_IFDIR)
            if identity != expected or self.child(name) != identity:
                raise Rejected("held child is not the original directory")
            yield fd
        finally: os.close(fd)


@contextmanager
def held_parent(path, expected=None):
    path = Path(path)
    fd = os.open(path, _flags())
    try:
        identity = native_identity(os.fstat(fd), stat.S_IFDIR)
        if expected is not None and identity != expected:
            raise Rejected("reopened parent is not the original selection")
        parent = Parent(path, fd, identity)
        parent.check()
        yield parent
        parent.check()
    finally: os.close(fd)


class HeldOutput:
    """New scratch only. Never open a ZIP member by joining it to a mutable OS path."""
    def __init__(self, root_fd):
        self.root = root_fd
        self.directories = {(): native_identity(os.fstat(root_fd), stat.S_IFDIR)}

    @contextmanager
    def _at(self, parts):
        fd = os.dup(self.root)
        try:
            for end, name in enumerate(parts, 1):
                child = os.open(name, _flags(), dir_fd=fd)
                os.close(fd)
                fd = child
                if native_identity(os.fstat(fd), stat.S_IFDIR) != self.directories.get(parts[:end]):
                    raise Rejected("private extraction directory was replaced")
            yield fd
        finally: os.close(fd)

    def mkdir(self, name):
        parts = member_name(name)
        with self._at(parts[:-1]) as parent:
            os.mkdir(parts[-1], 0o700, dir_fd=parent)
            self.directories[parts] = native_identity(
                os.stat(parts[-1], dir_fd=parent, follow_symlinks=False), stat.S_IFDIR)

    def file(self, name):
        parts = member_name(name)
        with self._at(parts[:-1]) as parent:
            fd = os.open(parts[-1], os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=parent)
            return os.fdopen(fd, "wb")

    def sync(self, _rows):
        for parts in sorted(self.directories, key=len, reverse=True):
            with self._at(parts) as fd: os.fsync(fd)
