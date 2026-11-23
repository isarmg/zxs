"""Held local WORK editing and a one-way cut for exact individual file images.

No mount entry, archive packaging, network, STORE write or automatic submit belongs here.
"""
from collections.abc import Mapping
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass, field
import errno
import fcntl
from itertools import count
import os
import stat
from threading import RLock
from types import MappingProxyType

from . import lifecycle, worktree
from .document import native_identity
from .linux_authority import ProcView, owned_directory
from .linux_edit_state import exchange_closed_at
from .linux_mount import _ordinary_user
from .linux_upload_ignore import RULE_FILE, UploadIgnore, held_upload_ignore
from .model import MAX_FILE, MAX_MEMBERS, Rejected, member_name
from .private_freeze import local_mount, mount_id
from .publish import rename_new_at

ROOT = 1
MAX_IO = 16 << 20  # Same per-request byte budget as the standalone snapshot mount.
_DIR = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
_PIN = os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC
_OPEN_SYNC = os.O_SYNC | os.O_DSYNC | getattr(os, "O_NOATIME", 0)
# libc's LP64 O_LARGEFILE can be zero. FUSE receives the native kernel bit.
_LARGEFILE = {"x86_64": 0o100000, "aarch64": 0o400000}.get(os.uname().machine, 0)
_OPEN_HINTS = os.O_CLOEXEC | os.O_NONBLOCK | os.O_NOCTTY | os.O_NOFOLLOW | _LARGEFILE


def _refuse(number, message):
    raise OSError(number, message)


def _path_member(parts):
    try:
        name = "/".join(parts)
        if member_name(name) != parts:
            raise Rejected("not a single component")
    except (UnicodeError, Rejected) as err:
        raise OSError(errno.EINVAL, "not a portable working-tree name") from err


def _child_path(parent, name):
    try: path = parent.path + (name.decode("utf-8"),)
    except UnicodeError as err: raise OSError(errno.EILSEQ, "not a UTF-8 working name") from err
    _path_member(path)
    return path


def _open_mode(flags, *, creating=False):
    access = flags & os.O_ACCMODE
    allowed = os.O_ACCMODE | os.O_TRUNC | os.O_APPEND | _OPEN_SYNC | _OPEN_HINTS
    if creating: allowed |= os.O_CREAT | os.O_EXCL
    if access not in (os.O_RDONLY, os.O_WRONLY, os.O_RDWR) or flags & ~allowed:
        _refuse(errno.EOPNOTSUPP, "unsupported editable open flags")
    writer = access != os.O_RDONLY
    if not writer and (creating or flags & (os.O_APPEND | os.O_TRUNC)):
        _refuse(errno.EACCES, "a writable open is required")
    return access | (flags & _OPEN_SYNC), access != os.O_WRONLY, writer


@dataclass(slots=True)
class _Node:
    ino: int
    path: tuple | None  # None only for a file explicitly unlinked in this session.
    pin: int
    kind: int
    lookups: int = 0
    opens: int = 0


@dataclass(slots=True)
class _File:
    node: _Node
    fd: int
    readable: bool
    writer: bool
    append: bool
    dirty: bool = False


@dataclass(slots=True)
class _Directory:
    node: _Node
    names: tuple  # one stable cookie sequence for this directory open
    retired: set[bytes] = field(default_factory=set)


@dataclass(frozen=True, slots=True)
class _BoundFile:
    path: str
    local_identity: tuple
    _node: _Node


@dataclass(slots=True)
class _Tracked:
    bound: _BoundFile
    stamp: tuple
    changed: bool = False  # File data, not a rename.
    namespace_changed: bool = False
    born_here: bool = False
    original_binding: bool = False
    retained: tuple | None = None  # exact replacement intent/completion proof for a retired inode


@dataclass(frozen=True, slots=True)
class NamespaceEvent:
    """One confirmed operation; original_path is the name immediately before it."""
    kind: str
    local_identity: tuple
    original_path: str
    destination_path: str | None
    replacement_identity: tuple | None = None
    local_birth: bool = False


@dataclass(frozen=True, slots=True)
class _RetiredMeta:
    metadata: os.stat_result
    st_nlink = 0  # The protected old inode has no remaining link in the virtual disk.

    def __getattr__(self, attr):
        return getattr(self.metadata, attr)


@dataclass(slots=True)
class _FileImage:
    fd: int
    bound: _BoundFile
    size: int
    _stamp: tuple
    _cut: object
    _active: bool = True

    def check_file(self):
        self._cut._owner._check_image(self)


@dataclass(frozen=True, slots=True)
class _FrozenWork:
    _work_fd: int
    _changed: Mapping[int, tuple[_BoundFile, tuple]]
    namespace_events: tuple[NamespaceEvent, ...]
    created_files: tuple[_BoundFile, ...]
    upload_ignore: UploadIgnore
    _owner: object

    @property
    def changed_files(self):
        return tuple(bound for bound, _ in self._changed.values())

    def check_work(self):
        self._owner._check_cut(self)

    def open_file(self, bound):
        return self._owner._open_image(self, bound)


@contextmanager
def edit_live_worktree(live, *, exclusive=False):
    """Borrow the actual creating window's WORK. Never adopt a pathname or a cold STATE."""
    if exclusive is not True:
        raise Rejected("explicit exclusive custody of the local backing directory is required")
    with ExitStack() as owned:
        proc = ProcView(owned)
        uid = _ordinary_user(read_proc=proc.read)
        original = live.borrow()
        owned.callback(original.close)
        with worktree.locked(original.state, read_only=True, live=original, local_only=True) as (con, row, _):
            instance = lifecycle.instance(con)
            original.check_instance(row, instance)
            lifecycle.require_visible(con, row)

        named, chain = owned_directory(owned, original.work, uid, proc.read)
        initial = os.fstat(original.work_fd)
        identity = native_identity(initial, stat.S_IFDIR)
        selected = local_mount(original.work_fd, read_proc=proc.read)
        state_identity = native_identity(os.fstat(original.state_fd), stat.S_IFDIR)

        def check_root():
            with ExitStack() as checking:
                current, witnesses = owned_directory(checking, original.work, uid, proc.read)
                here = os.fstat(current)
                if (witnesses != chain or native_identity(here, stat.S_IFDIR) != identity or
                        state_identity in tuple(witness[:2] for witness in witnesses) or
                        os.readlink(str(current), dir_fd=proc.fds) != str(original.work) or
                        here.st_uid != uid or stat.S_IMODE(here.st_mode) != 0o700 or
                        local_mount(current, read_proc=proc.read) != selected):
                    raise Rejected("the original private WORK directory or its mount changed")

        # A new description of the *held* directory takes an advisory lock. dup()
        # would share this lock with the long-lived GUI FD past the edit session.
        root = os.open(".", _DIR, dir_fd=original.work_fd)
        owned.callback(os.close, root)
        if (native_identity(os.fstat(named), stat.S_IFDIR) != identity or
                native_identity(os.fstat(root), stat.S_IFDIR) != identity or
                mount_id(root, read_proc=proc.read) != selected):
            raise Rejected("the retained WORK and its original name do not match")
        fcntl.flock(root, fcntl.LOCK_EX | fcntl.LOCK_NB)
        check_root()
        provider = _LocalEdit(root, proc, selected, uid, os.getgid(), check_root,
                              excluded=state_identity)
        try:
            yield provider
        finally:
            provider.close()


class _LocalEdit:
    """Edit ordinary files within one held local working directory."""

    def __init__(self, root, proc, selected, uid, gid, check_root, *, excluded):
        self.root, self.proc, self.mount, self.uid, self.gid = root, proc, selected, uid, gid
        self.excluded = excluded
        self.identity = native_identity(os.fstat(root), stat.S_IFDIR)
        self._root_name = os.readlink(str(root), dir_fd=proc.fds).rstrip("/")
        self.check_root = check_root
        self._lock = RLock()
        self._ino, self._fh = count(ROOT + 1), count(1)
        core = _Node(ROOT, (), root, stat.S_IFDIR, lookups=1)
        self._nodes, self._paths = {ROOT: core}, {(): core}
        self._opened = {}
        self._tracked = {}
        self._namespace, self._namespace_parents = [], set()
        self._stopped = self._detaching = self._closed = False
        self._failure = None
        self._journal = self._cut = None

    def _check(self):
        if self._closed: _refuse(errno.EIO, "the original editing session ended")
        try: self.check_root()
        except (OSError, Rejected) as err:
            self._failure = err
            raise

    def _gate(self):
        self._check()
        if self._failure is not None:
            raise OSError(errno.EIO, "original edit writeback is unconfirmed") from self._failure
        if self._stopped: _refuse(errno.EBUSY, "the original session stopped admitting writers")
        self._check_journal()

    def _check_journal(self):
        if self._journal is not None:
            try: self._journal.check()
            except (OSError, Rejected) as err:
                self._failure = err
                raise

    def attach_running_state(self, journal):
        with self._lock:
            self._gate()
            if (journal.owner is not self or self._journal is not None or self._opened
                    or self._namespace or any(record.changed for record in self._tracked.values())):
                _refuse(errno.EBUSY, "the original journal must precede every mounted edit")
            journal.check()
            self._journal = journal

    def _member(self, fd, *, links=1):
        m = os.fstat(fd)
        kind = stat.S_IFMT(m.st_mode)
        if (kind not in (stat.S_IFDIR, stat.S_IFREG) or not m.st_ino or
                (m.st_dev, m.st_ino) == self.excluded or m.st_dev != self.identity[0] or
                mount_id(fd, read_proc=self.proc.read) != self.mount or
                m.st_uid != self.uid or m.st_mode & 0o7022 or
                kind == stat.S_IFREG and (m.st_nlink != links or m.st_size > MAX_FILE)):
            _refuse(errno.EACCES, "not an owned plain member on the original WORK mount")
        return m

    def _node(self, ino):
        if ino not in self._nodes: _refuse(errno.ESTALE, "expired working-tree inode")
        return self._nodes[ino]

    def _literal(self, fd, path):
        # On a casefolding backing directory an ordinary open can succeed under
        # another spelling. Only the kernel dentry for the exact name is admitted.
        if os.readlink(str(fd), dir_fd=self.proc.fds) != self._root_name + "/" + "/".join(path):
            _refuse(errno.ENOENT, "not the original working spelling")

    def _observe(self, node):
        try:
            if node.path is None:
                record = self._tracked[node.ino]
                m = self._member(node.pin, links=0 if record.retained is None else 1)
                if self._image_stamp(m) != record.stamp:
                    self._lost_file("the original retired file changed")
                if record.retained is not None:
                    self._journal.check_committed(record.retained, record.stamp)
                return m
            m = self._member(node.pin)
            actual = native_identity(m, node.kind)
            if node.path:
                with self._pin(node.path) as fd:
                    self._literal(fd, node.path)
                    if native_identity(self._member(fd), node.kind) != actual:
                        _refuse(errno.ESTALE, "the original member name changed")
            elif actual != self.identity:
                _refuse(errno.ESTALE, "the original working root changed")
            return m
        except (OSError, Rejected) as err:
            self._failure = err  # No future cut can attest this provider's known name loss.
            raise

    @contextmanager
    def _pin(self, path):
        fd = self.proc.open_beneath(self.root, "/".join(path), _PIN)
        try:
            yield fd
        finally:
            os.close(fd)

    def _access(self, node, flags):
        original = native_identity(self._observe(node), node.kind)
        # Reopen only our own retained ordinary inode via the proven kernel procfs.
        # No file body or write open is done through the user-supplied name.
        slot = str(node.pin)
        link = os.stat(slot, dir_fd=self.proc.fds, follow_symlinks=False)
        same = os.stat(slot, dir_fd=self.proc.fds, follow_symlinks=True)
        if not stat.S_ISLNK(link.st_mode) or native_identity(same, node.kind) != original:
            _refuse(errno.ESTALE, "the original held inode cannot be reopened")
        fd = os.open(slot, flags | os.O_CLOEXEC | os.O_NONBLOCK, dir_fd=self.proc.fds)
        try:
            if (native_identity(self._member(fd), node.kind) != original or
                    native_identity(self._observe(node), node.kind) != original):
                _refuse(errno.ESTALE, "the opened member does not match its original")
            return fd
        except BaseException:
            os.close(fd)
            raise

    def _remember(self, path, *, expected=None):
        if not path: return self._node(ROOT)
        with self._pin(path) as fd:
            self._literal(fd, path)
            m = self._member(fd)
            kind = stat.S_IFMT(m.st_mode)
            actual = native_identity(m, kind)
            if expected is not None and actual != expected:
                _refuse(errno.ESTALE, "the exclusive creation no longer names its original file")
            old = self._paths.get(path)
            if old is not None:
                if kind != old.kind or native_identity(self._member(old.pin), old.kind) != actual:
                    error = OSError(errno.ESTALE, "the original inode at this name changed")
                    self._failure = error
                    raise error
                return old
            if len(self._nodes) > MAX_MEMBERS:
                _refuse(errno.ENOSPC, "working-tree index exceeds the portable member budget")
            node = _Node(next(self._ino), path, os.dup(fd), kind)
            self._nodes[node.ino] = self._paths[path] = node
            return node

    def _drop(self, node):
        if (node.ino != ROOT and node.ino not in self._tracked
                and node.ino not in self._namespace_parents and not (node.lookups or node.opens)):
            del self._nodes[node.ino], self._paths[node.path]
            self._close_pin(node)

    def _close_pin(self, node):
        try: os.close(node.pin)
        except OSError as err:
            # Forget has no error reply. Do not give a later successful write cut.
            if self._failure is None: self._failure = err

    def forget(self, names):
        with self._lock:
            for ino, n in names:
                node = self._nodes.get(ino)
                if node is not None:
                    node.lookups = max(0, node.lookups - n)
                    self._drop(node)

    def _dir(self, ino):
        node = self._node(ino)
        if node.kind != stat.S_IFDIR: _refuse(errno.ENOTDIR, "not an original directory")
        self._observe(node)
        return node

    def _named(self, parent, name):
        if name == b".": return parent
        if name == b"..": return self._remember(parent.path[:-1])
        path = _child_path(parent, name)
        os.stat(path[-1], dir_fd=parent.pin, follow_symlinks=False)  # true missing name => ENOENT
        return self._remember(path)

    def _entry(self, node):
        m = self._observe(node)
        return node.ino, _RetiredMeta(m) if node.path is None and m.st_nlink else m

    def lookup(self, ino, name):
        with self._lock:
            self._check()
            node = self._named(self._dir(ino), name)
            try: entry = self._entry(node)
            except BaseException:
                self._drop(node)
                raise
            node.lookups += 1
            return entry

    def getattr(self, ino):
        with self._lock:
            self._check()
            return self._entry(self._node(ino))

    def _names(self, parent, fd):
        names, folds = [], set()
        with os.scandir(fd) as entries:
            for item in entries:
                name = item.name
                _path_member(parent.path + (name,))
                key = name.casefold()
                if key in folds or len(names) == MAX_MEMBERS:
                    _refuse(errno.EIO, "ambiguous or over-budget working directory")
                folds.add(key)
                names.append(name.encode("utf-8"))
        self._observe(parent)
        return tuple(sorted(names)), folds

    def _hold(self, item):
        fh = next(self._fh)
        self._opened[fh] = item
        item.node.opens += 1
        return fh

    def _handle(self, fh, kind):
        held = self._opened.get(fh)
        if not isinstance(held, kind): _refuse(errno.EBADF, "not an original working handle")
        return held

    def opendir(self, ino):
        with self._lock:
            self._check()
            if self._detaching: _refuse(errno.EBUSY, "the original volume is preparing to detach")
            parent = self._dir(ino)
            fd = self._access(parent, os.O_RDONLY | os.O_DIRECTORY)
            try: names, _ = self._names(parent, fd)
            finally: os.close(fd)
            return self._hold(_Directory(parent, names))

    def readdir(self, fh, offset, reply):
        with self._lock:
            self._check()
            held = self._handle(fh, _Directory)
            self._observe(held.node)
            if not 0 <= offset <= len(held.names):
                _refuse(errno.EINVAL, "not an original directory cookie")
            for i in range(offset, len(held.names)):
                name = held.names[i]
                if name in held.retired: continue
                node = self._named(held.node, name)
                try:
                    if not reply(name, self._entry(node), i + 1): break
                    node.lookups += 1
                finally: self._drop(node)

    def releasedir(self, fh):
        with self._lock:
            held = self._handle(fh, _Directory)
            del self._opened[fh]
            held.node.opens -= 1
            self._drop(held.node)

    def _file(self, ino):
        node = self._node(ino)
        if node.kind != stat.S_IFREG: _refuse(errno.EISDIR, "not an original regular file")
        if node.path is None: _refuse(errno.ENOENT, "the original file was unlinked")
        return node

    def _track(self, node, metadata):
        record = self._tracked.get(node.ino)
        identity = native_identity(metadata, stat.S_IFREG)
        if record is None:
            bound = _BoundFile("/".join(node.path), identity, node)
            record = self._tracked[node.ino] = _Tracked(bound, self._image_stamp(metadata))
        elif record.bound.local_identity != identity:
            self._lost_file("the tracked file lost its original identity")
        return record

    def _lost_file(self, message):
        error = OSError(errno.ESTALE, message)
        self._failure = error
        raise error

    def _known_file(self, node, metadata):
        record = self._track(node, metadata)
        if self._image_stamp(metadata) != record.stamp:
            self._lost_file("the tracked file changed outside the held editing operation")
        return record

    def bind_file(self, ino):
        """Keep a before-edit local identity for the caller's separately attested container."""
        with self._lock:
            self._gate()
            node = self._file(ino)
            if node.path == (RULE_FILE,):
                _refuse(errno.EINVAL, ".zxsignore is reserved for local upload rules")
            record = self._known_file(node, self._observe(node))
            if not record.original_binding and (record.born_here or record.changed or record.namespace_changed):
                _refuse(errno.EINVAL, "bind the original container before any local edit")
            record.original_binding = True
            return record.bound

    def bind_retained(self, path, fd):
        """Bind the exact pre-verified collector FD, never a path alone."""
        with self._lock:
            self._gate()
            flags = fcntl.fcntl(fd, fcntl.F_GETFL)
            if flags & (os.O_ACCMODE | os.O_PATH) != os.O_RDONLY:
                _refuse(errno.EACCES, "an original read-only file descriptor is required")
            before = self._member(fd)
            node = self._remember(member_name(path), expected=native_identity(before, stat.S_IFREG))
            try:
                if self._image_stamp(before) != self._image_stamp(self._observe(node)):
                    self._lost_file("the exact pre-edit file did not match the original retained descriptor")
                binding = self.bind_file(node.ino)
                if self._image_stamp(before) != self._image_stamp(self._member(fd)):
                    self._lost_file("the original retained binding changed")
                return binding
            finally:
                self._drop(node)

    def _changed_file(self, node, metadata):
        record = self._track(node, metadata)
        record.stamp = self._image_stamp(metadata)
        record.changed = True
        return record

    def open(self, ino, flags):
        with self._lock:
            self._check()
            if self._detaching: _refuse(errno.EBUSY, "the original volume is preparing to detach")
            actual, readable, writer = _open_mode(flags)
            if writer: self._gate()
            node = self._file(ino)
            file = _File(node, self._access(node, actual), readable, writer, bool(flags & os.O_APPEND))
            try:
                if writer: self._known_file(node, self._held_file(file))
                if flags & os.O_TRUNC: self._resize(file, 0)
                return self._hold(file)
            except BaseException:
                self._finish(file)
                raise

    def create(self, parent, name, mode, flags):
        with self._lock:
            self._gate()
            actual, readable, writer = _open_mode(flags, creating=True)
            # No requested set-id, execute, ACL or group/other permissions are projected.
            folder = self._dir(parent)
            path = _child_path(folder, name)
            part = path[-1]
            dirfd = self._access(folder, os.O_RDONLY | os.O_DIRECTORY)
            fd, node = None, None
            try:
                _, folds = self._names(folder, dirfd)
                if part.casefold() in folds: _refuse(errno.EEXIST, "the original directory already has that name")
                if len(folds) == MAX_MEMBERS or len(self._nodes) > MAX_MEMBERS:
                    _refuse(errno.ENOSPC, "portable working-tree index is full")
                fd = os.open(part, actual | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW |
                             os.O_CLOEXEC | os.O_NONBLOCK, mode & 0o600, dir_fd=dirfd)
                original = native_identity(self._member(fd), stat.S_IFREG)
                node = self._remember(path, expected=original)
                entry = self._entry(node)
                self._sync_fd(dirfd)
                closed, dirfd = dirfd, None
                os.close(closed)
                self._changed_file(node, entry[1]).born_here = True
                fh = self._hold(_File(node, fd, readable, writer, bool(flags & os.O_APPEND), dirty=True))
                node.lookups += 1
                return fh, entry
            except BaseException as err:
                if fd is not None:
                    self._failure = err  # A name already appeared: never silently clean or submit.
                    try: os.close(fd)
                    finally:
                        if node is not None: self._drop(node)
                raise
            finally:
                if dirfd is not None: os.close(dirfd)

    @staticmethod
    def _vacant(parent_fd, part):
        try: os.stat(part, dir_fd=parent_fd, follow_symlinks=False)
        except FileNotFoundError: return
        _refuse(errno.EEXIST, "the working name is occupied")

    def _expect_name(self, parent_fd, path, stamp):
        try:
            fd = self.proc.open_beneath(parent_fd, path[-1], _PIN)
            try:
                self._literal(fd, path)
                if self._image_stamp(self._member(fd)) != stamp:
                    self._lost_file("the actual directory entry is not the original held file")
            finally:
                os.close(fd)
        except (OSError, Rejected) as err:
            self._failure = err
            raise

    def rename(self, parent, name, target, new_name, *, no_replace=False):
        return self._change_name(parent, name, target=target, new_name=new_name, no_replace=no_replace)

    def unlink(self, parent, name):
        return self._change_name(parent, name)

    def _editor_replacement(self, node, new):
        # Only a genuinely new local file may replace a known existing binding.
        # Binding authority is supplied by the controller, not guessed from a filename.
        temp = self._tracked.get(node.ino)
        target = self._paths.get(new)
        if (node.path == (RULE_FILE,) or self._journal is None or temp is None
                or not temp.born_here or temp.original_binding or target is None):
            return None
        kept = self._tracked.get(target.ino)
        if kept is not None and kept.original_binding:
            return temp, target, kept
        return None

    def _replace_exact(self, parent, node, old, new, entry):
        temp, target, kept = entry
        if target.opens:
            _refuse(errno.EBUSY, "close the original target before replacing it")
        journal = self._journal
        armed = False
        try:
            with ExitStack() as files:
                left = self._access(parent, os.O_RDONLY | os.O_DIRECTORY)
                files.callback(os.close, left)
                incoming = self._known_file(node, self._observe(node)).stamp
                original = self._known_file(target, self._observe(target)).stamp
                if incoming[:2] == original[:2]:
                    self._lost_file("editor replacement did not retain two different original files")
                with ExitStack() as fresh_closed:
                    fresh = self._access(node, os.O_RDONLY)
                    fresh_closed.callback(os.close, fresh)
                    self._sync_fd(fresh)
                self._gate()
                intent_old, target_path = "/".join(old), "/".join(new)
                armed = True  # Even an incomplete durable intent is not discarded or reused.
                intent = journal.prepare_replace(intent_old, target_path, incoming, original)
                self._observe(parent)
                self._expect_name(left, old, incoming)
                self._expect_name(left, new, original)
                journal.check_intent(intent)
                exchange_closed_at(left, old[-1], new[-1])

                # Both old inodes must still exist. The exchanged target is
                # preserved privately; do not unlink it or rely on a guessed temp name.
                first = self._image_stamp(self._member(node.pin))
                second = self._image_stamp(self._member(target.pin))
                if first[:-1] != incoming[:-1] or second[:-1] != original[:-1]:
                    self._lost_file("original editor operands changed during exchange")
                self._expect_name(left, new, first)
                self._expect_name(left, old, second)
                journal.hide_original(left, old[-1], intent)
                self._sync_fd(left)
                self._sync_fd(journal.fd)
                self._check()
                self._observe(parent)
                self._vacant(left, old[-1])
                after = self._image_stamp(self._member(target.pin))
                if after[:-1] != original[:-1]:
                    self._lost_file("original replaced inode changed before quarantine")
                journal.check_old(intent, after)
                self._expect_name(left, new, first)
            retained = journal.replaced(intent, first, after)
            self._check()
            self._observe(parent)
            with self._pin(new) as named:
                self._literal(named, new)
                if self._image_stamp(self._member(named)) != first:
                    self._lost_file("the completed replacement no longer names the exact new file")
            # Directories are closed and both native identities still pinned.
            del self._paths[old]
            self._paths[new] = node
            node.path, target.path = new, None
            kept.stamp, kept.namespace_changed, kept.retained = after, True, retained
            temp.stamp, temp.namespace_changed, temp.original_binding = first, True, True
            temp.bound = _BoundFile(target_path, temp.bound.local_identity, node)
            self._retire_cookie(parent, os.fsencode(old[-1]))
            self._namespace.append(NamespaceEvent("replace", kept.bound.local_identity, target_path,
                                                  target_path, temp.bound.local_identity))
        except BaseException as err:
            if armed:
                self._failure = err
                raise OSError(errno.EIO, "editor replacement is unconfirmed; retain RUNNING") from err
            raise

    def _retire_cookie(self, parent, name):
        self._namespace_parents.add(parent.ino)
        for held in self._opened.values():
            if isinstance(held, _Directory) and held.node is parent and name in held.names:
                held.retired.add(name)

    def _change_name(self, parent, name, *, target=None, new_name=None, no_replace=False):
        with self._lock:
            self._gate()
            source = self._dir(parent)
            old = _child_path(source, name)  # "." and ".." are never names to move or remove.
            node = self._named(source, name)
            attempted, visible = False, False
            try:
                self._file(node.ino)
                dest = self._dir(target) if target is not None else None
                new = _child_path(dest, new_name) if dest is not None else None
                if new == old:
                    if no_replace: _refuse(errno.EEXIST, "the working name is occupied")
                    return  # Ordinary exact same-name rename is not an edit.
                if node.opens:
                    _refuse(errno.EBUSY, "close the original file before moving or removing it")
                record = self._tracked.get(node.ino)
                if new == (RULE_FILE,) and record is not None and record.original_binding:
                    _refuse(errno.EINVAL, "an original container cannot become .zxsignore")
                if dest is source and not no_replace:
                    replacement = self._editor_replacement(node, new)
                    if replacement is not None:
                        return self._replace_exact(source, node, old, new, replacement)

                with ExitStack() as directories:
                    def open_parent(folder):
                        fd = self._access(folder, os.O_RDONLY | os.O_DIRECTORY)
                        directories.callback(os.close, fd)
                        return fd

                    left = open_parent(source)
                    right = None
                    if dest is not None:
                        right = left if dest is source else open_parent(dest)
                        _, folds = self._names(dest, right)
                        if dest is source: folds.discard(old[-1].casefold())
                        if new[-1].casefold() in folds:
                            _refuse(errno.EEXIST, "the destination already has that working name")
                        if dest is not source and len(folds) == MAX_MEMBERS:
                            _refuse(errno.ENOSPC, "the destination directory is full")
                        if new in self._paths:
                            self._lost_file("the indexed destination name changed outside the edit")
                        self._vacant(right, new[-1])

                    self._gate()
                    if dest is not None and dest is not source: self._observe(dest)
                    self._observe(source)
                    record = self._known_file(node, self._observe(node))
                    before = record.stamp
                    self._expect_name(left, old, before)
                    # Linux has no compare-unlink-by-inode here. Both parent
                    # names remain protected by this session's exclusive custody.
                    attempted = True
                    if new is None:
                        os.unlink(old[-1], dir_fd=left)
                    else:
                        rename_new_at(left, old[-1], new[-1], target_fd=right)
                    visible = True
                    self._sync_fd(left)
                    if right is not None and right != left: self._sync_fd(right)
                    self._check()
                    self._observe(source)
                    if dest is not None and dest is not source: self._observe(dest)
                    self._vacant(left, old[-1])
                    after = self._image_stamp(self._member(node.pin, links=0 if new is None else 1))
                    if after[:-1] != before[:-1]:  # rename/unlink can change ctime, not the body stamp.
                        self._lost_file("the original file changed during its name edit")
                    if new is not None: self._expect_name(right, new, after)
                # Close both real directory FDs before advancing the observed
                # namespace. The inode pin survives even an unlink and a FUSE forget.
                del self._paths[old]
                node.path = new
                record.stamp, record.namespace_changed = after, True
                end = None if new is None else "/".join(new)
                identity = record.bound.local_identity
                if new is not None:
                    self._paths[new] = node
                    record.bound = _BoundFile(end, identity, node)
                    self._namespace_parents.add(dest.ino)
                self._retire_cookie(source, name)
                kind = "unlink" if new is None else "rename"
                self._namespace.append(NamespaceEvent(kind, identity, "/".join(old), end,
                                                      local_birth=record.born_here and not record.original_binding))
            except BaseException as err:
                if attempted: self._failure = err
                if visible:
                    raise OSError(errno.EIO, "name edit may already be visible; no handoff") from err
                raise
            finally:
                self._drop(node)

    def _held_file(self, file):
        current = self._member(file.fd)
        if native_identity(current, stat.S_IFREG) != native_identity(self._observe(file.node), stat.S_IFREG):
            _refuse(errno.ESTALE, "not the original held file")
        return current

    def read(self, fh, offset, size):
        with self._lock:
            self._check()
            file = self._handle(fh, _File)
            if not file.readable: _refuse(errno.EBADF, "that handle has no read access")
            if not 0 <= size <= MAX_IO or offset < 0: _refuse(errno.EINVAL, "invalid local read")
            before = self._held_file(file)
            wanted = min(size, max(0, before.st_size - offset))
            data = bytearray()
            while len(data) < wanted:
                some = os.pread(file.fd, wanted - len(data), offset + len(data))
                if not some: _refuse(errno.EIO, "unexpected short read of the original local file")
                data.extend(some)
            after = self._held_file(file)
            if (after.st_size, after.st_mtime_ns, after.st_ctime_ns) != (
                    before.st_size, before.st_mtime_ns, before.st_ctime_ns):
                _refuse(errno.EIO, "the original local file changed during read")
            return bytes(data)

    def _check_writer(self, file):
        if self._failure is not None:
            raise OSError(errno.EIO, "original edit writeback is unconfirmed") from self._failure
        if not file.writer: _refuse(errno.EBADF, "that original handle has no write access")
        self._check_journal()

    def write(self, fh, offset, data):
        with self._lock:
            self._check()
            file = self._handle(fh, _File)
            self._check_writer(file)  # Already held writers may finish after stop_writes().
            before = self._held_file(file)
            self._known_file(file.node, before)
            if file.append: offset = before.st_size  # serialized with all true writers
            if not 0 <= len(data) <= MAX_IO or not 0 <= offset <= MAX_FILE - len(data):
                _refuse(errno.EFBIG, "local write outside portable file byte budget")
            if not data: return 0
            file.dirty = True
            try:
                written = os.pwrite(file.fd, data, offset)
                if not 0 < written <= len(data): _refuse(errno.EIO, "invalid local write count")
                self._changed_file(file.node, self._held_file(file))
                return written  # a short count is not reported as a complete edit
            except (OSError, Rejected) as err:
                self._failure = err
                raise

    def _resize(self, file, size):
        self._check_writer(file)
        self._known_file(file.node, self._held_file(file))
        if not 0 <= size <= MAX_FILE: _refuse(errno.EFBIG, "invalid portable file length")
        file.dirty = True
        try:
            os.ftruncate(file.fd, size)
            self._changed_file(file.node, self._held_file(file))
        except (OSError, Rejected) as err:
            self._failure = err
            raise

    def truncate(self, ino, size, fh=None):
        with self._lock:
            self._check()
            node = self._file(ino)
            if fh is None:
                self._gate()
                file = _File(node, self._access(node, os.O_WRONLY), False, True, False)
                try: self._resize(file, size)
                finally: self._finish(file)
            else:
                file = self._handle(fh, _File)
                if file.node is not node: _refuse(errno.EBADF, "truncate does not use this inode's own handle")
                self._resize(file, size)
            return self._entry(node)

    def _sync_fd(self, fd):
        try: os.fsync(fd)
        except OSError as err:
            self._failure = err
            raise

    def _flush(self, file):
        if file.writer and file.dirty:
            self._sync_fd(file.fd)
            file.dirty = False

    def flush(self, fh):
        with self._lock:
            self._flush(self._handle(fh, _File))  # sync the original, not its current name

    def fsync(self, fh):
        with self._lock:
            file = self._handle(fh, _File)
            self._sync_fd(file.fd)  # Never report explicit fsync without the native barrier.
            file.dirty = False

    def _finish(self, file):
        try: self._flush(file)
        finally:
            try: os.close(file.fd)
            except OSError as err:
                if file.writer: self._failure = err
                raise  # Never retry close(2): the descriptor number may be reused.

    def release(self, fh):
        with self._lock:
            file = self._handle(fh, _File)
            del self._opened[fh]
            try: self._finish(file)
            finally:
                file.node.opens -= 1
                self._drop(file.node)

    def stop_writes(self):
        """Close the admission gate; never force-close an existing writer's handle."""
        with self._lock:
            self._check()
            self._stopped = True
            return self._writers()

    def check_mount_start(self):
        with self._lock:
            self._gate()
            if self._journal is None or self._opened or self._cut is not None:
                _refuse(errno.EBUSY, "original mounted editing requires a durable RUNNING state and no old handles")

    def quiesce_for_detach(self):
        with self._lock:
            self.require_dispatch_drained()
            self._detaching = True  # No subsequent opens, including read/directory handles.

    def require_dispatch_drained(self):
        # Physical unmount proof is separately required by the broker. Never
        # force-close a remaining handle to make this assertion succeed.
        with self._lock:
            self._check()
            if self._failure is not None:
                raise OSError(errno.EIO, "original edit writeback is unconfirmed") from self._failure
            if self._journal is None or not self._stopped or self._opened or self._cut is not None:
                _refuse(errno.EBUSY, "original dispatch did not drain all file and directory handles")
            self._check_journal()

    def finish_unchanged_mount(self):
        """Release only a proven unchanged, normally detached mounted session."""
        with self._lock:
            self.require_dispatch_drained()
            if self._namespace or any(record.changed for record in self._tracked.values()):
                return False
            self._journal.finish_empty()
            return True

    def check_settled_effects(self):
        with self._lock:
            self.require_dispatch_drained()
            self._check_namespace()
            for record in self._tracked.values():
                node = record.bound._node
                if record.changed and not record.namespace_changed and node.path is not None:
                    self._known_file(node, self._observe(node))

    def _writers(self):
        return sum(isinstance(item, _File) and item.writer for item in self._opened.values())

    def _check_cut(self, cut):
        with self._lock:
            self._check()
            if cut is not self._cut or not self._stopped or self._writers() or self._failure is not None:
                _refuse(errno.EBUSY, "original write-stop proof is absent or unconfirmed")
            self._check_journal()
            m = self._member(cut._work_fd)
            if native_identity(m, stat.S_IFDIR) != self.identity:
                _refuse(errno.ESTALE, "capture did not hold the original working root")

    @staticmethod
    def _image_stamp(meta):
        return (meta.st_dev, meta.st_ino, meta.st_mode, meta.st_size, meta.st_mtime_ns, meta.st_ctime_ns)

    def _check_image(self, image):
        with self._lock:
            if not image._active: _refuse(errno.ESTALE, "the individual file loan already ended")
            self._check_cut(image._cut)
            f = self._member(image.fd)
            n = self._observe(image.bound._node)
            if self._image_stamp(f) != image._stamp or self._image_stamp(n) != image._stamp:
                self._lost_file("the original file image differs from the last admitted edit")

    @contextmanager
    def _open_image(self, cut, bound):
        with ExitStack() as loan:
            with self._lock:
                self._check_cut(cut)
                selected = cut._changed.get(bound._node.ino) if isinstance(bound, _BoundFile) else None
                if selected is None or selected[0] is not bound:
                    _refuse(errno.ESTALE, "file is not in this exact affected-file cut")
                fd = self._access(bound._node, os.O_RDONLY)
                loan.callback(os.close, fd)
                original = self._member(fd)
                if native_identity(original, stat.S_IFREG) != bound.local_identity:
                    self._lost_file("the original individual file was lost")
                image = _FileImage(fd, bound, original.st_size, selected[1], cut)
                image.check_file()
            try:
                yield image
                image.check_file()
            finally:
                with self._lock: image._active = False

    def _check_namespace(self):
        # Final live names and original unlinked inode pins, not missing paths
        # from a later scan. Directories may themselves have no remaining open handle.
        for ino in self._namespace_parents:
            self._observe(self._node(ino))
        for record in self._tracked.values():
            if record.namespace_changed:
                node = record.bound._node
                self._known_file(node, self._observe(node))

    @contextmanager
    def freeze(self):
        """Lend proved individual files from the stopped WORK, never across transport."""
        with ExitStack() as loans:
            with self._lock:
                self._check()
                if self._failure is not None:
                    raise OSError(errno.EIO, "failed write/close barrier; keep the original WORK") from self._failure
                if not self._stopped or self._writers() or self._cut is not None:
                    _refuse(errno.EBUSY, "stop new writes and wait for the original writers to release")
                self._sync_fd(self.root)
                # A capture has an independent directory cursor and keeps the
                # original session's lock even if its window/dispatcher exits.
                loans.callback(os.close, os.dup(self.root))
                fd = os.open(".", _DIR, dir_fd=self.root)
                loans.callback(os.close, fd)
                changed = sorted((r for r in self._tracked.values()
                                  if r.changed and r.bound._node.path is not None),
                                 key=lambda record: record.bound.path)
                entries = {record.bound._node.ino: (record.bound, record.stamp) for record in changed}
                births = tuple(r.bound for r in changed if r.born_here and not r.original_binding)
                ignore = loans.enter_context(held_upload_ignore(fd, self.proc, self.mount, self.uid))
                cut = _FrozenWork(fd, MappingProxyType(entries), tuple(self._namespace), births, ignore, self)
                self._cut = cut
                try:
                    cut.check_work()
                    self._check_namespace()
                except BaseException:
                    self._cut = None
                    raise
            try:
                yield cut
                with self._lock:
                    cut.check_work()
                    self._check_namespace()
            finally:
                with self._lock: self._cut = None  # admission remains stopped; no automatic resume

    def close(self):
        """Retire this owner. Never report a successful freeze after teardown."""
        with self._lock:
            if self._closed: return
            self._closed = self._stopped = True
            failure = None
            for fh, item in tuple(self._opened.items()):
                try:
                    if isinstance(item, _File): self.release(fh)
                    else: self.releasedir(fh)
                except OSError as err:
                    if failure is None: failure = err
            for node in self._nodes.values():
                if node.ino != ROOT: self._close_pin(node)
            self._nodes.clear()
            self._paths.clear()
            self._tracked.clear()
            self._namespace.clear()
            self._namespace_parents.clear()
            if failure is not None: raise failure
