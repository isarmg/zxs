"""Linux checkout staging in the operator's exclusive local CLI state. Never a picker for GUI state."""
from contextlib import contextmanager, nullcontext
import os
import stat
import sys

from .archive import _read_checked
from .document import native_identity
from .freeze import _distinct_roots, observe_held
from .model import Rejected
from .private_candidates import CandidateDirectory
from .private_freeze import local_mount, mount_id

LEAF = "prepared-worktree"


def same_mount(private, target):
    if not sys.platform.startswith("linux"):
        raise Rejected("private checkout staging needs one admitted Linux ext4/XFS mount")
    mount = local_mount(private)
    if mount_id(target) != mount or os.fstat(private).st_dev != os.fstat(target).st_dev:
        raise Rejected("private STATE and checkout target are not on the same original mount")
    return mount


def separate_roots(private, target, store, mount):
    store.check_store()
    # Walking ".." from a bind root follows its presented mount, not its hidden
    # backing parent. Don't use that walk to prove separation on the same device.
    if os.fstat(store.fd).st_dev == os.fstat(private).st_dev and mount_id(store.fd) != mount:
        raise Rejected("ambiguous archive-store mount on the private checkout device")
    _distinct_roots((private, target, store.fd))
    store.check_store()


def _flags():
    return os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC


def _private_id(m):
    if m.st_uid != os.geteuid() or m.st_mode & 0o077:
        raise Rejected("private checkout directory is no longer exclusive to its owner")
    return native_identity(m, stat.S_IFDIR)


def check_private(area):
    if (_private_id(os.fstat(area.parent)) != area.identity or
            _private_id(area.path.stat(follow_symlinks=False)) != area.identity):
        raise Rejected("the named private directory was replaced")


def check_named(parent, name, fd):
    identity = _private_id(os.fstat(fd))
    if identity != _private_id(os.stat(name, dir_fd=parent, follow_symlinks=False)):
        raise Rejected("private checkout child was replaced")
    return identity


@contextmanager
def new_child(area, name, mount, *, read_proc=None):
    # mkdirat does not return a handle. Use only an already-held, explicitly
    # exclusive private parent, not a shared destination or sync folder.
    check_private(area)
    os.mkdir(name, 0o700, dir_fd=area.parent)
    fd = os.open(name, _flags(), dir_fd=area.parent)
    try:
        identity = check_named(area.parent, name, fd)
        if (mount_id(fd) if read_proc is None else mount_id(fd, read_proc=read_proc)) != mount:
            raise Rejected("new private checkout directory crossed a mount")
        with os.scandir(fd) as entries:
            if next(entries, None) is not None: raise Rejected("new private checkout directory is not empty")
        check_private(area)
        yield CandidateDirectory(area.path / name, fd, identity)
    finally:
        os.close(fd)


def member_identity(fd, mount, *, read_proc=None):
    m = os.fstat(fd)
    actual_mount = mount_id(fd) if read_proc is None else mount_id(fd, read_proc=read_proc)
    if (not m.st_ino or m.st_uid != os.geteuid() or m.st_mode & 0o077 or
            not (stat.S_ISDIR(m.st_mode) or stat.S_ISREG(m.st_mode) and m.st_nlink == 1) or
            actual_mount != mount):
        raise Rejected("unsafe member in the original private stage")
    return m.st_dev, m.st_ino, stat.S_IFMT(m.st_mode)


class _Output:
    """Create each admitted member relative to checked directory FDs, at most one path deep at a time."""
    def __init__(self, stage, mount, *, read_proc=None):
        self.stage, self.mount, self.read_proc, self.ids = stage, mount, read_proc, {}

    def identity(self, fd):
        return member_identity(fd, self.mount, read_proc=self.read_proc)

    def check(self, name, fd):
        if self.ids.get(name) != self.identity(fd):
            raise Rejected("a different object occupies a private checkout member name")

    @contextmanager
    def _directory(self, path):
        check_private(self.stage)
        fd, prefix = os.dup(self.stage.parent), ""
        try:
            for part in (path.split("/") if path else ()):
                prefix = f"{prefix}/{part}" if prefix else part
                child = os.open(part, _flags(), dir_fd=fd)
                try: self.check(prefix, child)
                except BaseException:
                    os.close(child)
                    raise
                os.close(fd)
                fd = child
            yield fd
        finally:
            os.close(fd)

    def mkdir(self, path):
        parent, _, leaf = path.rpartition("/")
        with self._directory(parent) as fd:
            os.mkdir(leaf, 0o700, dir_fd=fd)
            child = os.open(leaf, _flags(), dir_fd=fd)
            try:
                check_named(fd, leaf, child)
                self.ids[path] = self.identity(child)
            finally: os.close(child)

    @contextmanager
    def file(self, path):
        parent, _, leaf = path.rpartition("/")
        with self._directory(parent) as fd:
            child = os.open(leaf, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW | os.O_CLOEXEC,
                            0o600, dir_fd=fd)
            try:
                self.ids[path] = self.identity(child)
                with os.fdopen(child, "wb", closefd=False) as output:
                    yield output
            finally: os.close(child)

    def sync(self, rows, *, checkpoint=None):
        for name in sorted((p for p, (d, _, _) in rows.items() if d), key=lambda p: p.count("/"), reverse=True):
            if checkpoint is not None: checkpoint()
            with self._directory(name) as fd: os.fsync(fd)
        if checkpoint is not None: checkpoint()
        os.fsync(self.stage.parent)


@contextmanager
def materialize(area, copy, marker, mount, *, observer=None):
    if observer is not None:
        observer.section("私有工作目录")
        observer.begin("layout")
    area.check_file("original.zxs", *copy)
    with new_child(area, LEAF, mount) as stage:
        output = _Output(stage, mount)
        # Same original FD whose immutable generation, marker and whole-ZIP SHA
        # _save_original verified. No reopen of the source or output by a full path.
        with os.fdopen(copy[0], "rb", closefd=False) as package:
            actual, _ = _read_checked(nullcontext(package), managed=True, sink=output, observer=observer)
        area.check_file("original.zxs", *copy)
        if observer is not None: observer.begin("tree")
        if (actual != marker or
                observe_held(stage.parent, stage.identity, require_ids=True, check_held=output.check,
                             checkpoint=observer.check if observer is not None else None).digest() != marker.tree):
            raise Rejected("private checkout stage differs from the selected original archive")
        if observer is not None: observer.begin("sync")
        check_named(area.parent, LEAF, stage.parent)
        os.fsync(area.parent)
        check_named(area.parent, LEAF, stage.parent)
        yield stage
