"""A live checkout receipt, not a path-based opener or a durable desktop registry."""
from contextlib import ExitStack
import os
import stat

from ..document import native_identity
from ..lifecycle import locations
from ..model import Rejected, check_id
from ..private_stage import mount_id
from .checkout_action import _named_mount


class LiveCheckout:
    """Only the actual creator of a new catalog can supply a session."""
    def __init__(self, choice, row, work_fd, private_fd, store_fd):
        self.work, self.state, self.store = choice.destination, choice.state, choice.store.path
        self.instance, self.repo = row["worktree"], choice.snapshot.browser.commit.repo
        self.checkout_operation = choice.operation
        self.stack, self.db, self.closed = ExitStack(), None, False
        try:
            self.work_fd, self.state_fd, self.store_fd = (
                self.keep(fd) for fd in (work_fd, private_fd, store_fd))
            self.check_dirs()
        except BaseException:
            self.close()
            raise

    def keep(self, original):
        fd = os.dup(original)
        self.stack.callback(os.close, fd)
        return fd

    def close(self):
        if not self.closed:
            self.closed = True
            self.stack.close()

    def check_dirs(self):
        if self.closed: raise Rejected("已关闭本窗口的原工作树会话")
        _named_mount(self.state, self.state_fd)
        _named_mount(self.store, self.store_fd)
        m = os.fstat(self.state_fd)
        if m.st_uid != os.geteuid() or m.st_mode & 0o077:
            raise Rejected("原 CLI 状态目录不再由当前用户独占")

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

    def check_catalog(self, path):
        self.check_dirs()
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

    def check_instance(self, row, original):
        expected = tuple(map(str, native_identity(os.fstat(self.work_fd), stat.S_IFDIR)))
        if ((row["worktree"], row["repo"], row["location"], row["store"]) !=
                (self.instance, self.repo, str(self.work), str(self.store)) or
                (original["device"], original["inode"]) != expected):
            raise Rejected("这不是本窗口创建的原工作树登记")
        phase = original["phase"]
        if phase in ("visible", "hidden"):
            if original["active"] is not None: raise Rejected("原工作树状态与待决可见操作冲突")
            visible, hidden = locations(row)
            _named_mount(visible if phase == "visible" else hidden, self.work_fd)
        elif phase in ("hide-pending", "restore-pending"):
            check_id(original["active"])
        else: raise Rejected("原工作树可见状态无法核证")
        # An actual in-progress phase is a record, not a promise about either name.

    def status(self):
        from ..worktree import status
        return status(self.state, live=self)
