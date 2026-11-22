"""Handles delivered by the actual CLI-format creator, never a path-based opener."""
from contextlib import ExitStack
from copy import copy
import os
import stat
from threading import Event

from ..document import native_identity
from ..lifecycle import locations
from ..model import Rejected, check_id
from ..private_stage import mount_id
from .checkout_action import _named_mount


class LiveCheckout:
    """Only the actual creator of a new catalog can supply a session."""
    def __init__(self, work, state, store, instance, repo, work_fd, private_fd, store_fd,
                 *, source=None, initial_branch=None, initial_index=None, initial_seed=None):
        self.work, self.state, self.store = work, state, store
        self.instance, self.repo = check_id(instance), check_id(repo)
        self.review_origin = object()  # The creator and its borrowed views share only this in-memory identity.
        self.initial_base = ((self.repo, check_id(initial_branch), 0, None, None)
                             if initial_branch is not None else None)
        # Only a checkout has a displayed source node. A fresh registration is
        # defined by its own actual, newly created catalog and empty baseline.
        if (source is None) == (self.initial_base is None):
            raise Rejected("必须明确交接已签出的原节点或本次新登记的空基线")
        (self.checkout_operation, self.checkout_marker, self.checkout_sha,
         self.source_path, self.source_stamp) = source if source is not None else (None,)*5
        if initial_index is not None and (source is None or initial_index.commit != self.checkout_marker):
            raise Rejected("首次签出索引不属于原节点")
        # Verified names/digests only, independent of the display selection and
        # its remote file descriptor. Never an authority for writing to STORE.
        self.initial_index = initial_index
        # The revocation is shared by borrowed views; none holds the old display FD.
        self._branch_source_revoked = Event()
        self.stack, self.db, self.closed = ExitStack(), None, False
        try:
            self.work_fd, self.state_fd, self.store_fd = (
                self.keep(fd) for fd in (work_fd, private_fd, store_fd))
            self.check_dirs()
            self.store_identity = native_identity(os.fstat(self.store_fd), stat.S_IFDIR)
            self.initial_seed = None
            if initial_seed is not None and initial_index is not None:
                try: self.initial_seed = self.keep(initial_seed[0]), initial_seed[1]
                except OSError: pass  # Listing and ordinary worktree custody do not require a text-read loan.
        except BaseException:
            self.close()
            raise

    @property
    def branch_source_available(self):
        return self.checkout_marker is not None and not self.closed and not self._branch_source_revoked.is_set()

    def revoke_branch_source(self):
        self._branch_source_revoked.set()

    def require_branch_source(self):
        if not self.branch_source_available:
            raise Rejected("已离开签出时的原浏览快照；不能从同名或重新选择的归档新建分支")

    def borrow(self, *, seed=False):
        """One in-flight job owns independent references, even if a stale UI drops its selection."""
        if self.closed or self.db is None: raise Rejected("没有可交接的本窗口原会话")
        other = copy(self)
        other.stack = ExitStack()
        other.initial_seed = None
        try:
            for attr in ("work_fd", "state_fd", "store_fd", "db"):
                setattr(other, attr, other.keep(getattr(self, attr)))
            if seed and self.initial_seed is not None:
                try: other.initial_seed = other.keep(self.initial_seed[0]), self.initial_seed[1]
                except OSError: pass
            if self.closed: raise Rejected("原工作树会话已关闭")
            return other  # The background reader qualifies original names, mounts and SQLite before use.
        except BaseException:
            other.close()
            raise

    def keep(self, original):
        fd = os.dup(original)
        self.stack.callback(os.close, fd)
        return fd

    def close(self):
        if not self.closed:
            self.closed = True
            self.initial_index = self.initial_seed = None
            self.stack.close()

    def check_state(self):
        if self.closed: raise Rejected("已关闭本窗口的原工作树会话")
        _named_mount(self.state, self.state_fd)
        m = os.fstat(self.state_fd)
        if m.st_uid != os.geteuid() or m.st_mode & 0o077:
            raise Rejected("原 CLI 状态目录不再由当前用户独占")

    def check_dirs(self):
        self.check_state()
        _named_mount(self.store, self.store_fd)

    def new_catalog(self, path, original):
        self.check_dirs()
        if self.db is not None or path.parent != self.state:
            raise Rejected("只接收本次状态目录中独占新建的库")
        # O_PATH pins the inode, not the contents, without opening an additional
        # SQLite data fd whose close might cancel this process's POSIX locks.
        db = os.open(path.name, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.state_fd)
        self.stack.callback(os.close, db)
        self.db = db
        if not os.path.samestat(os.fstat(original), os.fstat(db)):
            raise Rejected("新建的原 catalog 文件在交接前已被替换")
        self.check_catalog(path)

    def check_catalog(self, path, *, local_only=False):
        if local_only: self.check_state()
        else: self.check_dirs()
        if self.db is None or path.parent != self.state:
            raise Rejected("无本窗口实际创建的库，不按路径接管")
        named_fd = os.open(path.name, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.state_fd)
        try:
            held, named = os.fstat(self.db), os.fstat(named_fd)
            if (native_identity(held, stat.S_IFREG) != native_identity(named, stat.S_IFREG) or
                    held.st_nlink != 1 or named.st_nlink != 1 or held.st_uid != os.geteuid()
                    or held.st_mode & 0o077 or not mount_id(named_fd) == mount_id(self.db) == mount_id(self.state_fd)):
                raise Rejected("本窗口原 catalog 的实例、链接、权限或挂载已变化")
        finally: os.close(named_fd)

    def sync_new_catalog(self, temp):
        self.check_catalog(temp)
        fd = os.open(temp.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK, dir_fd=self.state_fd)
        try:
            if not os.path.samestat(os.fstat(self.db), os.fstat(fd)):
                raise Rejected("库在文件同步前被替换")
            os.fsync(fd)
            self.check_catalog(temp)
        finally: os.close(fd)

    def check_instance(self, row, original, *, check_name=True):
        expected = tuple(map(str, native_identity(os.fstat(self.work_fd), stat.S_IFDIR)))
        if ((row["worktree"], row["repo"], row["location"], row["store"]) !=
                (self.instance, self.repo, str(self.work), str(self.store)) or
                (original["device"], original["inode"]) != expected):
            raise Rejected("这不是本窗口创建的原工作树登记")
        phase = original["phase"]
        if phase in ("visible", "hidden"):
            if original["active"] is not None: raise Rejected("原工作树状态与待决可见操作冲突")
            if check_name:
                visible, hidden = locations(row)
                _named_mount(visible if phase == "visible" else hidden, self.work_fd)
        elif phase in ("hide-pending", "restore-pending"):
            check_id(original["active"])
        else: raise Rejected("原工作树可见状态无法核证")
        # An actual in-progress phase is a record, not a promise about either name.

    def status(self):
        from ..worktree import status
        return status(self.state, live=self)
