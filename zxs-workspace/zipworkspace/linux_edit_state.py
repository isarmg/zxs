"""One held Linux edit-session crash fence and exact replacement receipts.

An existing RUNNING name always quarantines the session; no cold resume or cleanup.
"""
from contextlib import ExitStack, contextmanager
import ctypes
import errno
import os
import stat

from .document import native_identity
from .freeze import _distinct_roots
from .linux_authority import owned_directory
from .model import Rejected, canonical, check_id, new_id
from .private_freeze import mount_id
from .publish import rename_new_at

_RUNNING = ".workspace-edit-running"
_MARK = "session"
_DIR = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
_PIN = os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC
_NEW = os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC


def exchange_closed_at(parent, source, target):
    """Exact single-leaf Linux RENAME_EXCHANGE. Neither operand is discarded."""
    if (source == target or any(not n or n in (".", "..") or "/" in n or "\0" in n for n in (source, target))):
        raise Rejected("invalid literal editor exchange operands")
    call = getattr(ctypes.CDLL(None, use_errno=True), "renameat2", None)
    if call is None:
        raise OSError(errno.EOPNOTSUPP, "no native atomic editor exchange")
    call.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
    call.restype = ctypes.c_int
    if call(parent, os.fsencode(source), parent, os.fsencode(target), 2) != 0:  # RENAME_EXCHANGE
        num = ctypes.get_errno()
        raise OSError(num, os.strerror(num))


def _stamp(m):
    return m.st_dev, m.st_ino, m.st_mode, m.st_size, m.st_mtime_ns, m.st_ctime_ns


class _Running:
    def __init__(self, live, owner, state):
        self.live, self.owner, self.state = live, owner, state
        self.proc, self.mount, self.uid = owner.proc, owner.mount, owner.uid
        self.operation = new_id()
        self.state_identity = native_identity(os.fstat(state), stat.S_IFDIR)
        self._check_root()

    def _check_root(self):
        m = os.fstat(self.state)
        if (self.live.closed or native_identity(m, stat.S_IFDIR) != self.state_identity
                or not os.path.samestat(m, os.fstat(self.live.state_fd))
                or self.state_identity == self.owner.identity
                or m.st_uid != self.uid or stat.S_IMODE(m.st_mode) != 0o700
                or mount_id(self.state, read_proc=self.proc.read) != self.mount
                or os.readlink(str(self.state), dir_fd=self.proc.fds) != str(self.live.state)):
            raise Rejected("需要原 WORK 挂载上的原有私有 STATE，不改用其他目录")

    def _check_name(self, name):
        self._check_root()
        m = os.fstat(self.fd)
        found = os.stat(name, dir_fd=self.state, follow_symlinks=False)
        if (not os.path.samestat(m, self.created) or not os.path.samestat(m, found)
                or found.st_uid != self.uid or stat.S_IMODE(found.st_mode) != 0o700
                or mount_id(self.fd, read_proc=self.proc.read) != self.mount
                or os.readlink(str(self.fd), dir_fd=self.proc.fds) != str(self.live.state / name)):
            raise Rejected("原本机会话隔离目录已变化")

    def _metadata(self, name):
        m = os.stat(name, dir_fd=self.fd, follow_symlinks=False)
        native_identity(m, stat.S_IFREG)
        if m.st_nlink != 1 or m.st_uid != self.uid or stat.S_IMODE(m.st_mode) != 0o400:
            raise Rejected("原本地记录不再是独占的只读文件")
        return _stamp(m)

    def _record(self, name, fields):
        data = canonical(fields)
        fd = os.open(name, _NEW, 0o600, dir_fd=self.fd)
        try:
            tail = memoryview(data)
            while tail:
                n = os.write(fd, tail)
                if n <= 0: raise OSError(errno.EIO, "incomplete private edit record")
                tail = tail[n:]
            os.fchmod(fd, 0o400)
            os.fsync(fd)
            receipt = _stamp(os.fstat(fd))
            if (os.pread(fd, len(data) + 1, 0) != data
                    or mount_id(fd, read_proc=self.proc.read) != self.mount
                    or self._metadata(name) != receipt or _stamp(os.fstat(fd)) != receipt):
                raise Rejected("未核到原私有记录的写后读回")
        finally:
            os.close(fd)
        os.fsync(self.fd)
        if self._metadata(name) != receipt: raise Rejected("本地记录在同步时变化")
        return receipt

    def check(self):
        self._check_name(_RUNNING)
        if self._metadata(_MARK) != self.marker:
            raise Rejected("原 RUNNING 记录已变化，保留现场")

    def prepare_replace(self, old, target, incoming, original):
        self.check()
        operation = new_id()
        stamp = self._record(operation + ".intent", {"temporary": old, "target": target,
                             "incoming": incoming, "original": original, "session": self.operation})
        intent = operation, stamp
        self.check_intent(intent)
        return intent

    def check_intent(self, intent):
        self.check()
        operation, stamp = intent
        if self._metadata(operation + ".intent") != stamp:
            raise Rejected("原替换意图的持久记录已变化")
        return operation

    def hide_original(self, parent, name, intent):
        operation = self.check_intent(intent)
        rename_new_at(parent, name, operation + ".old", target_fd=self.fd)

    def check_old(self, intent, expected):
        leaf = self.check_intent(intent) + ".old"
        fd = self.proc.open_beneath(self.fd, leaf, _PIN)
        try:
            m = os.fstat(fd)
            native_identity(m, stat.S_IFREG)
            if (m.st_nlink != 1 or m.st_uid != self.uid or _stamp(m) != expected
                    or mount_id(fd, read_proc=self.proc.read) != self.mount
                    or os.readlink(str(fd), dir_fd=self.proc.fds) != str(self.live.state / _RUNNING / leaf)):
                raise Rejected("私处未保存原来被替换的实物")
        finally:
            os.close(fd)

    def replaced(self, intent, incoming, original):
        self.check_old(intent, original)
        done = self._record(intent[0] + ".done", {"incoming": incoming, "original": original,
                                               "session": self.operation})
        self.check_committed((intent, done), original)
        return intent, done

    def check_committed(self, retained, original):
        intent, done = retained
        self.check_old(intent, original)
        if self._metadata(intent[0] + ".done") != done:
            raise Rejected("原替换完成回执已变化")

    def finish_empty(self):
        """Normal stopped, unchanged session only; never discard an edited sentinel."""
        self.check()
        with os.scandir(self.fd) as items:
            if {entry.name for entry in items} != {_MARK}:
                raise Rejected("原会话仍有操作材料，保留 RUNNING")
        self._retire(".workspace-edit-clean-" + self.operation)

    def pending_identity(self, submission, area):
        self._check_root()
        leaf = check_id(submission) + ".pending"
        fd = self.proc.open_beneath(self.state, leaf, _PIN | os.O_DIRECTORY)
        try:
            m = os.fstat(fd)
            actual = native_identity(m, stat.S_IFDIR)
            if (actual != area.identity or not os.path.samestat(m, os.fstat(area.parent))
                    or mount_id(fd, read_proc=self.proc.read) != self.mount
                    or m.st_uid != self.uid or stat.S_IMODE(m.st_mode) != 0o700
                    or os.readlink(str(fd), dir_fd=self.proc.fds) != str(self.live.state / leaf)):
                raise Rejected("待决提交目录不是原本机会话中核对的同一实物")
            return actual
        finally:
            os.close(fd)

    def finish_pending(self, proof, *, verify_before, verify_after):
        """Retire the active name, not the retained plan or original file material."""
        self.check()
        verify_before()
        stamp = self._record("settled", {"format": 1, "session": self.operation, **proof})
        self.check()
        verify_before()
        self._retire(".workspace-edit-settled-" + self.operation)
        if self._metadata("settled") != stamp:
            raise Rejected("本次本地结清记录已经变化")
        verify_after()

    def _retire(self, name):
        self.check()
        rename_new_at(self.state, _RUNNING, name, expected_source=self.created)
        os.fsync(self.state)
        self._check_name(name)
        if self._metadata(_MARK) != self.marker:
            raise Rejected("原会话回执变化")


@contextmanager
def running_state(live, owner):
    """Only the admitted broker invokes this before exposing any mounted writer."""
    with ExitStack() as held:
        state = os.open(".", _DIR, dir_fd=live.state_fd)
        held.callback(os.close, state)
        run = _Running(live, owner, state)
        named, _ = owned_directory(held, live.state, run.uid, run.proc.read, metadata_only=True)
        if not os.path.samestat(os.fstat(state), os.fstat(named)):
            raise Rejected("原 STATE 的私有路径与保留的目录不一致")
        _distinct_roots((owner.root, state))
        try: os.mkdir(_RUNNING, 0o700, dir_fd=state)
        except FileExistsError as err:
            raise Rejected("原 WORK 仍有 RUNNING 或未知编辑记录；保留，不能重挂或按路径推断更改") from err
        run.created = os.stat(_RUNNING, dir_fd=state, follow_symlinks=False)
        run.fd = run.proc.open_beneath(state, _RUNNING, _DIR)
        held.callback(os.close, run.fd)
        run._check_name(_RUNNING)
        run.marker = run._record(_MARK, {"format": 1, "instance": live.instance,
                                 "operation": run.operation, "state": run.state_identity,
                                 "work": owner.identity})
        os.fsync(state)
        run.check()
        yield run
        # An edited or abnormal session remains RUNNING. The caller may record a
        # proven clean stop explicitly, but there is no implicit rollback or retry.
