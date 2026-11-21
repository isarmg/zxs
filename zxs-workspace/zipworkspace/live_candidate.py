"""One live Linux worktree capture, using the private ZIP as the only data copy."""
from contextlib import ExitStack, contextmanager
import errno
import os
import stat
import sys

from .archive import read_managed_document
from .capacity import buffered_output, close_output_fd, writing
from .document import PositionalDocument, archive_sha, native_identity
from .freeze import _distinct_roots, _sig, pack_observed_folder
from .linux_authority import ProcView
from .model import Rejected, check_id
from .private_candidates import generation
from .private_freeze import local_mount, mount_id


class _StateWrites:
    def __init__(self, stream, scope): self.stream, self.scope = stream, scope

    def __getattr__(self, name): return getattr(self.stream, name)

    def write(self, data):
        with self.scope("写入未命名的 Store ZIP 捕获件"):
            length = self.stream.write(data)
            if length != len(data): raise OSError(errno.EIO, "short private ZIP write")
            return length

    def flush(self):
        with self.scope("刷新未命名的 Store ZIP 捕获件"): self.stream.flush()


class LiveArchiveCandidate:
    def __init__(self, choice, proc, uid):
        self.choice, self.live, self.area = choice, choice.live, choice.area
        self.proc, self.uid, self.observer = proc, uid, choice.preparation
        self.operation = check_id(choice.operation)
        self.name = self.operation + ".zxs"
        self.path, self.writer, self.reader = self.area.path/self.name, None, None
        if (self.live.state, self.live.state_fd) != (self.area.path, self.area.parent):
            raise Rejected("不是本窗口持有的原 STATE")
        self.mount = local_mount(self.area.parent, read_proc=proc.read)

    def writing(self, phase):
        return writing("STATE", self.path, phase, self.operation, self.area.parent)

    def check_state(self):
        self.live.check_state()
        self.area.check_state()
        if local_mount(self.area.parent, read_proc=self.proc.read) != self.mount:
            raise Rejected("原 STATE 的实际挂载已经改变")

    def _absent(self):
        try: os.stat(self.name, dir_fd=self.area.parent, follow_symlinks=False)
        except FileNotFoundError: return
        raise Rejected("原操作候选名已被占用，不打开、覆盖或另选候选")

    def _file(self, fd, links):
        meta = os.fstat(fd)
        if (native_identity(meta, stat.S_IFREG)[0] != self.area.identity[0]
                or meta.st_uid != self.uid or stat.S_IMODE(meta.st_mode) != 0o600
                or meta.st_nlink != links or mount_id(fd, read_proc=self.proc.read) != self.mount):
            raise Rejected("候选不再是原 STATE 挂载上的本人私有普通文件")
        return generation(meta)

    def _check_named(self, fd, observed):
        self.check_state()
        named = os.open(self.name, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.area.parent)
        try:
            if self._file(fd, 1) != observed or self._file(named, 1) != observed:
                raise Rejected("原候选名称已移动、替换或改动；不受理替件")
        finally: os.close(named)

    def open_writer(self):
        self.check_state()
        self._absent()
        if self.writer is not None: raise Rejected("已有该原操作的匿名写句柄")
        with self.writing("取得唯一匿名 ZIP 句柄，未建立待决"):
            # O_EXCL here would make O_TMPFILE permanently un-linkable.
            self.writer = os.open(".", os.O_TMPFILE | os.O_RDWR | os.O_CLOEXEC, 0o600, dir_fd=self.area.parent)
        if self._file(self.writer, 0)[2] != 0: raise Rejected("匿名候选不是全新的空文件")

    def close_writer(self):
        fd, self.writer = self.writer, None
        if fd is not None: close_output_fd(fd, "STATE", self.path, self.operation, self.area.parent)

    def _link_private(self, proc_fd_name):
        os.link(proc_fd_name, self.name, src_dir_fd=self.proc.fds, dst_dir_fd=self.area.parent,
                follow_symlinks=True)

    def build(self, expected, marker_for_tree, check_registration):
        fd, observer = self.writer, self.observer
        if fd is None or self.reader is not None: raise Rejected("只能从本次独占的匿名捕获件建立候选")
        work, store = self.live.work_fd, self.live.store_fd
        _distinct_roots((work, self.area.parent, store))
        if local_mount(work, read_proc=self.proc.read) != self.mount:
            raise Rejected("原 WORK 不在原 STATE 的同一已准入挂载上")
        store_mount = local_mount(store, read_proc=self.proc.read)
        excluded = {self.area.identity, native_identity(os.fstat(store), stat.S_IFDIR)}
        work_stat = _sig(os.fstat(work))

        def check_work():
            check_registration()
            self.check_state()
            if (_sig(os.fstat(work)) != work_stat or
                    local_mount(work, read_proc=self.proc.read) != self.mount or
                    local_mount(store, read_proc=self.proc.read) != store_mount):
                raise Rejected("原工作目录或挂载在捕获时改变")

        def member(_, child):
            m = os.fstat(child)
            if not m.st_ino or (m.st_dev, m.st_ino) in excluded or mount_id(child, read_proc=self.proc.read) != self.mount:
                raise Rejected("不能捕获状态、存储别名或嵌套挂载")

        check_work()
        with buffered_output(os.fdopen(fd, "w+b", closefd=False), "STATE", self.path,
                             self.operation, self.area.parent) as stream:
            out = _StateWrites(stream, self.writing)
            observer.begin("freeze")
            marker, body_length = pack_observed_folder(work, expected, out, marker_for_tree, check_held=member,
                                                        checkpoint=observer.check, on_read=observer.advance,
                                                        before_inventory=lambda: observer.begin("inventory"))
            out.flush()
        sealed = self._file(fd, 0)
        doc = PositionalDocument(fd, sealed[2])
        actual, tree = read_managed_document(doc, display_name=self.name, observer=observer)
        raw = archive_sha(doc, observer=observer)
        if actual != marker or marker.operation != self.operation or tree.total != body_length:
            raise Rejected("未命名候选的整树与本次冻结内容不等")
        observer.begin("sync")
        with self.writing("同步完整且已读回的未命名候选"): os.fsync(fd)
        check_work()
        proc_fd_name = str(fd)
        if (self._file(fd, 0) != sealed or
                not stat.S_ISLNK(os.stat(proc_fd_name, dir_fd=self.proc.fds, follow_symlinks=False).st_mode)):
            raise Rejected("未命名候选在读回后改变")
        actual = os.stat(proc_fd_name, dir_fd=self.proc.fds)
        if (native_identity(actual, stat.S_IFREG) != sealed[:2] or
                generation(actual) != sealed or actual.st_nlink != 0):
            raise Rejected("内核 procfs 项没有指向原匿名候选")
        observer.check()
        try:
            with self.writing("无覆盖建立原私有候选名，尚未登记待决"):
                self._link_private(proc_fd_name)
        except FileExistsError as error:
            raise Rejected("原操作候选名后来被占用；没有覆盖") from error
        except OSError as error:
            self._preserve(error)
            raise
        try:
            observed = self._file(fd, 1)
            if observed[:4] != sealed[:4]:  # link changes ctime, not content generation.
                raise Rejected("新候选不是读回并同步的原匿名文件")
            self._check_named(fd, observed)
            with self.writing("同步本机原命名候选及其父目录，尚未登记待决"):
                os.fsync(fd)
                os.fsync(self.area.parent)
            reader = os.open(self.name, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK,
                             dir_fd=self.area.parent)
            self.choice.stack.callback(os.close, reader)
            self._check_named(reader, observed)
            self._check_named(fd, observed)
            self.close_writer()  # Do not record any pending intent after a failed final writer close.
            self._check_named(reader, observed)
            check_work()
            self.reader, self.observed, self.marker = reader, observed, marker
            return marker, raw
        except Exception as error:
            self._preserve(error)
            raise

    def _preserve(self, error):
        error.add_note(f"原候选名 {self.path} 可能保留；没有因此登记待决，不自动删除或重试。")

    @contextmanager
    def source(self, marker):
        if self.choice.closed or self.reader is None or marker != self.marker:
            raise Rejected("没有该操作从实际捕获件交接的原只读句柄")
        self._check_named(self.reader, self.observed)
        yield PositionalDocument(self.reader, self.observed[2])
        self._check_named(self.reader, self.observed)


@contextmanager
def capture_live(choice):
    if (sys.platform != "linux" or not getattr(os, "O_TMPFILE", 0) or not hasattr(os, "pread")
            or os.link not in os.supports_dir_fd or os.link not in os.supports_follow_symlinks):
        raise Rejected("当前窗口需要原生匿名文件及无覆盖相对链接，不用命名临时文件降级")
    from .linux_mount import _ordinary_user
    proc = ProcView(choice.stack)
    capture = LiveArchiveCandidate(choice, proc, _ordinary_user(read_proc=proc.read))
    with ExitStack() as owned:
        owned.callback(capture.close_writer)
        choice.preparation.check()
        capture.open_writer()  # Unsupported anonymous storage refuses before SQLite's first writer.
        choice.preparation.check()
        yield capture
