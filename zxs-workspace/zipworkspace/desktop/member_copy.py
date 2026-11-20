"""Linux Qt exports a selected snapshot or archive entry without overwriting."""
from contextlib import ExitStack, closing, contextmanager
import hashlib
import os
from pathlib import Path
from types import SimpleNamespace
import stat
import sys
from threading import Event, Lock

from .. import private_stage
from ..archive import copy_body, read_managed_document, stored_member, stored_plain, stored_subtree
from ..capacity import observed_available
from ..comparison import Folder
from ..document import archive_sha, native_identity, relative_document
from ..freeze import _distinct_roots, observe_held
from ..linux_authority import ProcView as _ProcView, owned_directory as _directory
from ..linux_mount import _ordinary_user
from ..model import CHUNK, MAX_CD, MAX_FILE, MAX_TOTAL, Rejected, member_name, new_id
from ..opening import _read_selected
from ..private_candidates import held_candidates
from ..private_freeze import local_mount, mount_id
from ..publish import Uncertain, held_store, rename_new_at, require_native_writer


def _generation(m):
    return (*native_identity(m, stat.S_IFREG), m.st_size, m.st_mtime_ns, m.st_ctime_ns)


class _Positional:
    """Never change the offset shared by the displayed FD and the FUSE helper."""
    def __init__(self, fd, end, check=None):
        self.fd, self.end, self.pos, self.check = fd, end, 0, check

    def tell(self): return self.pos

    def seek(self, n, whence=os.SEEK_SET):
        origin = {os.SEEK_SET: 0, os.SEEK_CUR: self.pos, os.SEEK_END: self.end}[whence]
        if origin+n < 0: raise Rejected("negative ZIP offset")
        self.pos = origin+n
        return self.pos

    def read(self, n):
        if not 0 <= n <= MAX_CD or self.pos+n > self.end:
            raise Rejected("ZIP read exceeds its selected byte window")
        if self.check is not None: self.check()
        data = os.pread(self.fd, n, self.pos)
        if self.check is not None: self.check()
        self.pos += len(data)
        return data


def _new_path(spelling, *, scratch=False):
    path = Path(spelling)
    if (path.anchor != "/" or str(path) != spelling or ".." in path.parts or
            not (2 if scratch else 3) <= len(path.parts) <= 66 or path.parts[1] in ("dev", "proc", "sys") or
            (not scratch and len(member_name(path.name)) != 1)):
        raise Rejected("请选择明确的现有本机目录或其下一个新名称")
    return path


def _check_source(selected, source, read_proc):
    named = os.open(selected.path, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        if (_generation(os.fstat(source)) != selected.observed or
                _generation(os.fstat(named)) != selected.observed or
                mount_id(source, read_proc=read_proc) != mount_id(named, read_proc=read_proc)):
            raise Rejected("选中的原归档已被移走、替换或修改了挂载；不重开正文")
    finally: os.close(named)


def _unoccupied(parent, name):
    try: os.stat(name, dir_fd=parent, follow_symlinks=False)
    except FileNotFoundError: return
    raise Rejected("目的名称已有文件、目录或链接；不覆盖")


def _copy_capacity(parent, size):
    available = observed_available(parent)
    line = f"本次新增的文件逻辑量：{size:,} B；正式发布不再复制一份。"
    if available is None:
        line += "\n所选本机挂载当前可用量未知。"
    else:
        line += f"\n所选本机挂载当前报告普通用户可用 {available:,} B。"
        if size > available: line += "按当前报告可能不足。"
    return line + "未计目录、inode 等开销；不预留空间，也不保证配额或成功。"


class CancelledCopy(Rejected):
    """This copy stopped before making any public name."""


class _CheckedReads:
    def __init__(self, raw, check, on_read=None): self.raw, self.check, self.on_read = raw, check, on_read

    def __getattr__(self, name): return getattr(self.raw, name)

    def read(self, *args):
        self.check()
        data = self.raw.read(*args)
        self.check()
        if self.on_read is not None: self.on_read(len(data))
        return data


class _BeforePublication:
    _cancel_note = "已取消这次目录复制，未发布新的正式名称。"

    def _init_cancellation(self):
        self.cancelled, self._publication_lock, self.publishing = Event(), Lock(), False
        self._progress = ("", 0, None)

    def _phase(self, name, total=None):
        with self._publication_lock: self._progress = (name, 0, total)

    def _bytes_processed(self, amount):
        with self._publication_lock:
            label, done, total = self._progress
            self._progress = label, done + amount, total

    def progress(self):
        with self._publication_lock: return self._progress

    def cancel(self):
        """Return true only if cancellation won before the unique native publication."""
        with self._publication_lock:
            if self.closed or self.publishing: return False
            self.cancelled.set()
            return True

    def _checkpoint(self):
        if self.cancelled.is_set(): raise CancelledCopy(self._cancel_note)

    def _begin_publication(self):
        # No lock spans system I/O. This is the irrevocable commitment to
        # perform the one allowed native publication, not its success.
        with self._publication_lock:
            self._checkpoint()
            self.publishing = True


class _AnonymousCopy(_BeforePublication):
    """One genuine unnamed inode until the fully verified result can be linked."""
    _cancel_note = "已取消复制，未发布该文件。无名称副本在句柄关闭后释放。"
    def __init__(self, selected, spelling, size):
        if sys.platform != "linux" or selected.held is None:
            raise Rejected("另存需要 Linux 上当前仍持有原件的快照")
        self.path = _new_path(spelling)
        self.selected, self.size = selected, size
        self.stack = ExitStack()
        self.ready = self.used = self.closed = False
        self._init_cancellation()
        self.source = selected.held.duplicate()
        self.stack.callback(os.close, self.source)

    def close(self):
        if not self.closed:
            self.closed = True
            self.stack.close()

    def _source(self):
        _check_source(self.selected, self.source, self.proc.read)

    def _parent(self):
        with ExitStack() as check:
            _, ids = _directory(check, self.path.parent, self.uid, self.proc.read)
        if ids != self.parents or local_mount(self.parent, read_proc=self.proc.read) != self.parents[-1][-1]:
            raise Rejected("选中的目的目录或上级目录不再是原件")

    def _absent(self):
        _unoccupied(self.parent, self.path.name)

    def review(self):
        if self.closed or self.ready or self.used: raise Rejected("请重新选择一次快照内容和目的文件")
        try:
            if (not getattr(os, "O_TMPFILE", 0) or not hasattr(os, "pread") or
                    os.link not in os.supports_dir_fd or os.link not in os.supports_follow_symlinks):
                raise Rejected("当前 Python/Linux 缺少匿名文件、偏移读取或原生无覆盖链接能力")
            self.proc = _ProcView(self.stack)
            self.uid = _ordinary_user(read_proc=self.proc.read)
            try: self.parent, self.parents = _directory(self.stack, self.path.parent, self.uid, self.proc.read)
            except OSError as err: raise Rejected("无法持有无符号链接的原目的目录") from err
            try: mount = local_mount(self.parent, read_proc=self.proc.read)
            except Rejected as err: raise Rejected("目的目录须位于已核验的本机 ext4/XFS 挂载") from err
            if mount != self.parents[-1][-1]:
                raise Rejected("目的目录在选择期间改变了挂载")
            self._source()
            self._absent()
            capacity = _copy_capacity(self.parent, self.size)
            self._parent()
            self.ready = True
            return (f"原归档：{str(self.selected.path)!r}\n节点：{self.selected.browser.commit.operation}\n"
                    + self._selection_details() +
                    f"新文件：{str(self.path)!r}\n{capacity}\n\n"
                    "完整核验及读回后才公开名称；不覆盖、不创建工作树、不打开结果。"
                    "可在正式发布前请求取消；正在进行的系统调用须先返回，发布开始后不能撤销。"
                    "不会复制权限、时间或来源元数据；内容校验不代表来源可信。")
        except BaseException:
            self.close()
            raise

    def _output(self, fd, links, size, name=None, *, follow=False):
        m = os.fstat(fd) if name is None else os.stat(name[1], dir_fd=name[0], follow_symlinks=follow)
        generation = _generation(m)
        if (generation[0] != self.parents[-1][0] or m.st_uid != self.uid or stat.S_IMODE(m.st_mode) != 0o600 or
                m.st_nlink != links or m.st_size != size or (name is None and mount_id(fd, read_proc=self.proc.read) != self.parents[-1][-1])):
            raise Rejected("未核到原目的卷上本人所有、0600 的普通新文件")
        return generation

    def _copy(self, read, fd):
        self._phase("正在写入未发布的匿名副本", self.size)
        digest = hashlib.sha256()
        left = self.size
        while left:
            self._checkpoint()
            count = min(left, CHUNK)
            data = read(count)
            self._checkpoint()
            if not 0 < len(data) <= count: raise Rejected("没有达到这次选择的原核验长度")
            digest.update(data)
            left -= len(data)
            view = memoryview(data)
            while view:
                self._checkpoint()
                written = os.write(fd, view)
                if not 0 < written <= len(view): raise Rejected("未能写完匿名文件")
                self._bytes_processed(written)
                view = view[written:]
        self._checkpoint()
        if read(1): raise Rejected("实际 EOF 与原长度不符")
        self._checkpoint()
        return digest.hexdigest()

    def _verify(self, fd, sha):
        size = self.size
        self._checkpoint()
        before = self._output(fd, 0, size)
        self._phase("正在同步未发布的匿名副本")
        os.fsync(fd)
        self._checkpoint()
        self._phase("正在读回未发布的匿名副本", size)
        digest, read = hashlib.sha256(), 0
        while read < size:
            self._checkpoint()
            data = os.pread(fd, min(CHUNK, size-read), read)
            self._checkpoint()
            if not data: raise Rejected("匿名副本读回提前结束")
            read += len(data)
            self._bytes_processed(len(data))
            digest.update(data)
        if os.pread(fd, 1, size) or digest.hexdigest() != sha or self._output(fd, 0, size) != before:
            raise Rejected("匿名副本读回不等于原数据")
        return before

    def run(self, *, confirmed=False):
        if confirmed is not True or not self.ready or self.closed or self.used:
            raise Rejected("只允许明确确认一次已核验的快照内容和目的文件")
        self.used = True
        may_be_linked = False
        try:
            with self.stack, ExitStack() as temp:
                self._checkpoint()
                if _ordinary_user(read_proc=self.proc.read) != self.uid:
                    raise Rejected("不再是确认这次另存的普通用户")
                self._source()
                self._parent()
                self._absent()
                def create():
                    self._checkpoint()
                    # O_EXCL would forbid linking this O_TMPFILE; no named fallback.
                    out = os.open(".", os.O_TMPFILE | os.O_RDWR | os.O_CLOEXEC, 0o600, dir_fd=self.parent)
                    temp.callback(os.close, out)
                    self._output(out, 0, 0)
                    return out

                out, sha = self._payload(create)
                size = self.size
                sealed = self._verify(out, sha)
                self._source()
                self._parent()
                name = str(out)
                if (not stat.S_ISLNK(os.stat(name, dir_fd=self.proc.fds, follow_symlinks=False).st_mode) or
                        self._output(out, 0, size) != sealed or
                        self._output(out, 0, size, (self.proc.fds, name), follow=True) != sealed):
                    raise Rejected("procfs 中没有核验通过的原匿名文件")
                self._begin_publication()
                self._phase("正在正式发布并同步所选父目录")
                may_be_linked = True
                try:
                    os.link(name, self.path.name, src_dir_fd=self.proc.fds, dst_dir_fd=self.parent, follow_symlinks=True)
                except FileExistsError as err:
                    may_be_linked = False
                    raise Rejected("目的名称已被占用；没有覆盖") from err
                published = self._output(out, 1, size)
                if published[:4] != sealed[:4]:  # link changes ctime, not identity/size/mtime
                    raise Rejected("新名称与已核验的文件不符")

                def recheck():
                    self._parent()
                    named = os.open(self.path.name, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.parent)
                    try:
                        if self._output(named, 1, size) != published or self._output(out, 1, size) != published:
                            raise Rejected("正式名称或原副本在确认时改变")
                    finally: os.close(named)

                recheck()
                os.fsync(self.parent)
                recheck()
            return f"已另存并核验：{str(self.path)!r}（{size:,} 字节）"
        except Exception as err:
            if may_be_linked:
                raise Uncertain(f"无法确认原目的文件是否完整登记：{str(self.path)!r}。完整副本可能已经出现；"
                                f"请先检查原位置，不自动重试或删除。\n{err}") from err
            raise
        finally:
            self.closed = True


class MemberCopy(_AnonymousCopy):
    """Only the selected ordinary member, not its siblings or the whole tree."""
    def __init__(self, selected, directory, entry, spelling):
        if (entry.kind != "file" or selected.browser.entry(directory, entry.name) is not entry or
                type(entry.size_bytes) is not int or not 0 <= entry.size_bytes <= MAX_FILE):
            raise Rejected("只能另存本次已核验的一条普通成员")
        self.entry = entry
        self.member = f"{directory}/{entry.name}" if directory else entry.name
        super().__init__(selected, spelling, entry.size_bytes)

    def _selection_details(self):
        return f"成员：{self.member!r}\n正文：{self.size:,} 字节\nSHA-256：{self.entry.sha256}\n"

    def _payload(self, create):
        reader = _Positional(self.source, self.selected.observed[2], self._checkpoint)
        with stored_member(reader, reader.end, self.selected.browser.commit, self.member, self.size) as body:
            fd = create()
            sha = self._copy(body.read, fd)  # ZipExtFile verifies the member's CRC at EOF.
            if sha != self.entry.sha256: raise Rejected("成员与原核验的 SHA-256 不符")
        return fd, sha


class ZipCopy(_AnonymousCopy):
    """An unchanged ordinary Store/ZIP64 file with a .zip name for other extractors."""
    def __init__(self, selected, spelling):
        path = _new_path(spelling)
        size = selected.observed[2]
        if path.suffix.lower() != ".zip" or not 0 < size <= MAX_TOTAL + 2*MAX_CD + (1 << 20):
            raise Rejected("请选择新的 .zip 文件名；原 ZIP 长度必须在归档上限内")
        super().__init__(selected, spelling, size)

    def _selection_details(self):
        exact = self.selected.exact
        return (f"完整原包：{self.size:,} 字节\n"
                + (f"原整包 SHA-256：{exact.sha256}\n" if exact is not None else "")
                + "逐字节另存原 Store ZIP，保留所有目录，以及包内版本标识等标准 ZIP 注释；可用普通 ZIP 工具解压。"
                  "不解包、不改原归档。\n")

    def _payload(self, create):
        fd = create()
        sha = self._copy(_read_selected(self.source), fd)
        if self.selected.exact is not None and sha != self.selected.exact.sha256:
            raise Rejected("复制的原始字节不等于列表中已核验的节点")
        self._output(fd, 0, self.size)
        # The private readback uses the one new inode, before publishing the .zip name.
        self._phase("正在核验匿名 ZIP 里的完整目录和成员")
        with os.fdopen(fd, "rb", closefd=False) as actual:
            marker, _ = read_managed_document(_CheckedReads(actual, self._checkpoint), display_name=self.selected.path.name)
        if marker != self.selected.browser.commit:
            raise Rejected("原始字节不再表示选中的目录快照")
        return fd, sha


class _AtomicDirectoryCopy(_BeforePublication):
    """The destination becomes visible only as one complete verified directory."""
    def __init__(self, selected, spelling):
        if sys.platform != "linux" or selected.held is None:
            raise Rejected("提取目录需要 Linux 窗口中仍持有核验过的原归档")
        self.path = _new_path(spelling)
        self.selected = selected
        self.stage_name = ".zip-workspace-export-" + new_id()
        self.stack = ExitStack()
        self.private = None
        self.ready = self.used = self.closed = False
        self._init_cancellation()
        try:
            self.source = selected.held.duplicate()
            self.stack.callback(os.close, self.source)
            # Pin the target before the next modal picker. Qualify its ancestor
            # chain against this same live object when both choices are ready.
            self.target = self.stack.enter_context(closing(Folder.choose(self.path.parent)))
        except BaseException:
            self.close()
            raise

    def pick_private(self, spelling):
        if self.closed or self.used or self.private is not None:
            raise Rejected("请重新选择一次私有暂存目录")
        self.private = self.stack.enter_context(held_candidates(_new_path(spelling, scratch=True)))

    def close(self):
        if not self.closed:
            self.closed = True
            self.stack.close()

    def _bind_path(self, path, original):
        fd, chain = _directory(self.stack, path, self.uid, self.proc.read)
        if (not os.path.samestat(os.fstat(fd), os.fstat(original)) or
                chain[-1][-1] != mount_id(original, read_proc=self.proc.read)):
            raise Rejected("所选目录在核验前已换成另一个原件或挂载")
        return fd, chain

    def _parents(self):
        read = self.proc.read
        private_stage.check_private(self.private)
        if stat.S_IMODE(os.fstat(self.private.parent).st_mode) != 0o700:
            raise Rejected("暂存目录必须是本人独占的现有 0700 目录")
        with ExitStack() as current:
            for path, held, ids in ((self.private.path, self.private.parent, self.scratch_chain),
                                    (self.path.parent, self.parent, self.target_chain)):
                _, actual = _directory(current, path, self.uid, read)
                if actual != ids or local_mount(held, read_proc=read) != self.mount:
                    raise Rejected("原目的目录、私有暂存目录或上级路径已改变")
        _distinct_roots((self.private.parent, self.parent))

    def _named_stage(self, parent, name, stage):
        if private_stage.check_named(parent, name, stage.parent) != stage.identity:
            raise Rejected("目录名不再指向原暂存实体")
        # samestat alone can agree through a same-inode bind. Qualify the
        # actual name's native handle as well as the held original object.
        flags = os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
        fd = os.open(name, flags, dir_fd=parent)
        try:
            if (not os.path.samestat(os.fstat(fd), os.fstat(stage.parent)) or
                    mount_id(fd, read_proc=self.proc.read) != self.mount or
                    mount_id(stage.parent, read_proc=self.proc.read) != self.mount):
                raise Rejected("目录名或原暂存不在选定的实际挂载")
        finally:
            os.close(fd)

    def review(self):
        if self.closed or self.used or self.ready or self.private is None:
            raise Rejected("须明确选择本次目录、目的新名称及专用暂存位置")
        try:
            require_native_writer()
            if not hasattr(os, "pread"): raise Rejected("本机不支持原归档的偏移读取")
            self.proc = _ProcView(self.stack)
            self.uid = _ordinary_user(read_proc=self.proc.read)
            self.parent, self.target_chain = self._bind_path(self.path.parent, self.target.check())
            _, self.scratch_chain = self._bind_path(self.private.path, self.private.parent)
            self.mount = local_mount(self.parent, read_proc=self.proc.read)
            self._parents()
            _unoccupied(self.parent, self.path.name)
            _unoccupied(self.private.parent, self.stage_name)
            self._bound_source()
            self.rows = self._approved_rows()
            self.size = sum(row[1] for row in self.rows.values())
            capacity = _copy_capacity(self.private.parent, self.size)
            self._parents()
            self.ready = True
            return (f"原归档：{str(self.selected.path)!r}\n{self._describe()}\n"
                    f"内容：{len(self.rows):,} 项，{self.size:,} 字节（包括空目录）\n"
                    f"新的完整目录：{str(self.path)!r}\n私有暂存：{str(self.private.path / self.stage_name)!r}\n{capacity}\n\n"
                    "不登记或签出工作树。在独占私处复核全部内容后，只改名一次；同名不覆盖。"
                    "不复制来源元数据，也不打开或执行结果。"
                    "可在正式发布前请求取消；正在进行的系统调用须先返回，发布开始后不能撤销。"
                    "取消或失败可能在私处留材料，不删除或另发。")
        except BaseException:
            self.close()
            raise

    def _bound_source(self):
        _check_source(self.selected, self.source, self.proc.read)

    def _sealed_tree(self, stage, sink):
        return _observe_subtree(stage, sink, self.rows, checkpoint=self._checkpoint)

    def run(self, *, confirmed=False):
        if confirmed is not True or not self.ready or self.used or self.closed:
            raise Rejected("只允许明确确认一次已核验的目录内容和新位置")
        self.used = True
        stage_attempted = may_have_moved = False
        try:
            with self.stack:
                read = self.proc.read
                self._checkpoint()
                if _ordinary_user(read_proc=read) != self.uid: raise Rejected("普通用户已改变，不能另存")
                self._bound_source()
                self._parents()
                _unoccupied(self.parent, self.path.name)
                reader = _Positional(self.source, self.selected.observed[2], self._checkpoint)
                with self._entries(reader) as (archive, files):
                    self._bound_source()
                    self._parents()
                    _unoccupied(self.parent, self.path.name)
                    self._checkpoint()
                    stage_attempted = True
                    with private_stage.new_child(self.private, self.stage_name, self.mount, read_proc=read) as stage:
                        sink = private_stage._Output(stage, self.mount, read_proc=read)
                        for name, (directory, _, _) in sorted(self.rows.items()):
                            self._checkpoint()
                            if directory: sink.mkdir(name)
                        self._phase("正在读取将复制的文件内容，尚未发布", self.size)
                        for name, original in files:
                            self._checkpoint()
                            _, size, sha = self.rows[name]
                            with archive.open(original) as body, sink.file(name) as dest:
                                source = _CheckedReads(body, self._checkpoint, self._bytes_processed)
                                if copy_body(source, size, dest) != sha:
                                    raise Rejected("私有副本与原预览中成员的 SHA-256 不符")
                        self._phase("正在同步并完整读回私处，尚未发布")
                        sink.sync(self.rows, checkpoint=self._checkpoint)
                        sealed = self._sealed_tree(stage, sink)
                        self._bound_source()
                        self._parents()
                        self._named_stage(self.private.parent, self.stage_name, stage)
                        if _directory_generation(stage.parent) != sealed:
                            raise Rejected("原私有暂存目录已改变")
                        self._begin_publication()
                        self._phase("正在正式发布完整目录并同步两个原父")
                        may_have_moved = True
                        try:
                            rename_new_at(self.private.parent, self.stage_name, self.path.name,
                                          expected_source=os.fstat(stage.parent), target_fd=self.parent)
                        except FileExistsError as err:
                            may_have_moved = False
                            raise Rejected("目的名称已被占用，没有覆盖") from err

                        published = _directory_generation(stage.parent)
                        # ext4/XFS update the moved inode's ctime on rename.
                        if published[:-1] != sealed[:-1]:
                            raise Rejected("正式名称的目录已不是刚刚核验的内容代")
                        def recheck():
                            self._parents()
                            self._named_stage(self.parent, self.path.name, stage)
                            if _directory_generation(stage.parent) != published:
                                raise Rejected("正式名称不是原来完整核验的暂存目录")
                            _unoccupied(self.private.parent, self.stage_name)

                        recheck()
                        os.fsync(self.parent)
                        os.fsync(self.private.parent)
                        recheck()
            return f"已另存并核验目录：{str(self.path)!r}（{len(self.rows):,} 项，{self.size:,} 字节）"
        except Exception as err:
            scratch = str(self.private.path / self.stage_name)
            if may_have_moved:
                raise Uncertain(f"无法确认原目录是否完成持久发布：{str(self.path)!r}。完整副本可能已经出现；"
                                f"请先核对该位置及原私处 {scratch!r}，不自动删除或重试。\n{err}") from err
            if stage_attempted:
                failure = CancelledCopy if isinstance(err, CancelledCopy) else Rejected
                raise failure(f"{err}\n原私处 {scratch!r} 可能仍有准备材料；未覆盖目的地，不自动清理或重试。") from err
            raise
        finally:
            self.closed = True


class DirectoryCopy(_AtomicDirectoryCopy):
    """One genuine displayed managed subtree, not the rest of the archive."""
    def __init__(self, selected, directory, entry, spelling):
        if entry.kind != "directory" or selected.browser.entry(directory, entry.name) is not entry:
            raise Rejected("只能另存本次窗口内的一条已核验目录")
        self.root = f"{directory}/{entry.name}" if directory else entry.name
        super().__init__(selected, spelling)

    def _approved_rows(self):
        prefix = self.root + "/"
        return {path[len(prefix):]: (entry.kind == "directory", entry.size_bytes or 0, entry.sha256)
                for path, entry in self.selected.browser.verified_entries() if path.startswith(prefix)}

    def _describe(self):
        return f"节点：{self.selected.browser.commit.operation}\n仅另存所选子目录：{self.root!r}"

    def _entries(self, reader):
        return stored_subtree(reader, reader.end, self.selected.browser.commit, self.root, self.rows)


class PlainZipCopy(_AtomicDirectoryCopy):
    """Only a preview-held ordinary ZIP; no marker is trusted or created."""
    def __init__(self, selected, spelling):
        from .selection import PlainSelection
        if not isinstance(selected, PlainSelection) or selected.held is None or selected.held.closed:
            raise Rejected("须持有本次普通 ZIP 核验时的原文件")
        super().__init__(selected, spelling)

    def _approved_rows(self): return self.selected.rows

    def _describe(self):
        return f"提取完整普通 Store ZIP\n原预览整树 SHA-256：{self.selected.preview.tree}"

    def _entries(self, reader):
        return stored_plain(reader, reader.end, self.rows)


class HistoryCopy(_AtomicDirectoryCopy):
    """Copy exactly one previously verified, complete ancestry to a new local STORE."""

    def __init__(self, selected, choice, catalog, spelling):
        marker = selected.browser.commit
        tip = catalog.nodes.get(marker.operation)
        chain, missing = catalog.history(marker.operation)
        if (choice.path != catalog.store or tip is None or missing is not None or tip.marker != marker or
                str(tip.path) != str(selected.path) or tip.observed != selected.observed or
                (selected.exact is not None and selected.exact != tip)):
            raise Rejected("需选择当前 STORE 中已经核验到起点的规范版本；不能用散包、旧列表或缺失父节点")
        originals = []
        for parent in reversed(chain):  # never select an updated branch tip
            node = catalog.nodes.get(parent.operation)
            if (node is None or node.marker != parent or
                    str(node.path) != str(choice.path / parent.filename)):
                raise Rejected("每一代都须来自同一次列表核验的原规范槽")
            originals.append(node)
        if not originals or originals[-1] is not tip:
            raise Rejected("没有当前版本的完整原历史")
        self.tip, self.originals, self.choice = tip, tuple(originals), choice
        self.store = None
        super().__init__(selected, spelling)

    def _approved_rows(self):
        self.store = self.stack.enter_context(held_store(self.choice.path))
        if self.store.identity != self.choice.identity:
            raise Rejected("原归档存储被替换，不能复制历史")
        self.store_mount = mount_id(self.store.fd, read_proc=self.proc.read)
        self._bound_source()
        return {node.marker.filename: (False, node.observed[2], node.sha256) for node in self.originals}

    def _bound_source(self):
        super()._bound_source()
        if self.store is None: return
        self.store.check_store()
        read = self.proc.read
        fd = os.open(self.store.path, os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            if (native_identity(os.fstat(fd), stat.S_IFDIR) != self.choice.identity or
                    mount_id(fd, read_proc=read) != self.store_mount or
                    mount_id(self.store.fd, read_proc=read) != self.store_mount or
                    mount_id(self.source, read_proc=read) != self.store_mount):
                raise Rejected("原 STORE 已更换挂载或所选归档不在这个原 STORE 上")
        finally: os.close(fd)
        if self.choice.identity[0] == self.target_chain[-1][0] and self.store_mount != self.mount:
            raise Rejected("源 STORE 和本机目的藏在同设备的不同挂载，不能证明互相独立")
        _distinct_roots((self.store.fd, self.private.parent, self.parent))

    def _describe(self):
        return (f"原 STORE（只读）：{str(self.store.path)!r}\n"
                f"固定的末代节点：{self.tip.marker.operation} · 第 {self.tip.marker.generation} 代\n"
                f"原样复制已核的 {len(self.originals):,} 代及其规范文件名，不追分支头或复制旁支。"
                "新目录是独立的本机 STORE，可稍后明确选择。不会修改或上传到原 STORE。")

    def _named_original(self, node, fd):
        if (_generation(os.fstat(fd)) != node.observed or
                mount_id(fd, read_proc=self.proc.read) != self.store_mount):
            raise Rejected("原历史归档已替换或迁出所选 STORE 挂载")

    @contextmanager
    def _entries(self, _reader):
        # Check original relative names and physical mounts before creating
        # the private directory. Only one ancestor data FD is open at a time.
        for node in self.originals:
            self._checkpoint()
            self._exact_leaf(node)
        yield self, [(node.marker.filename, node) for node in self.originals]

    def _exact_leaf(self, node):
        fd = os.open(node.marker.filename, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.store.fd)
        try: self._named_original(node, fd)
        finally: os.close(fd)

    @contextmanager
    def open(self, node):
        if node is self.tip:
            # The displayed file's original data capability is already held.
            self._named_original(node, self.source)
            self._exact_leaf(node)
            yield SimpleNamespace(read=_read_selected(self.source))
            self._named_original(node, self.source)
            self._exact_leaf(node)
        else:
            name = node.marker.filename
            with relative_document(self.store.fd, name) as doc:
                self._named_original(node, doc.fileno())
                self._exact_leaf(node)
                yield doc
                self._named_original(node, doc.fileno())
                self._exact_leaf(node)

    def _sealed_tree(self, stage, sink):
        # These are raw archive files, not a worktree manifest. Several valid
        # ZIP snapshots can exceed the per-tree logical body budget in total.
        original = _directory_generation(stage.parent)
        def names():
            with os.scandir(stage.parent) as found:
                got = {entry.name for _, entry in zip(range(len(self.rows) + 1), found)}
            if got != self.rows.keys(): raise Rejected("本机暂存包含缺失或额外归档")
        names()
        for leaf, (_, size, expected) in self.rows.items():
            self._checkpoint()
            with relative_document(stage.parent, leaf, single=True) as doc:
                def still_named():
                    fd = os.open(leaf, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=stage.parent)
                    try:
                        sink.check(leaf, fd)
                        sink.check(leaf, doc.fileno())
                    finally: os.close(fd)
                still_named()
                if (os.fstat(doc.fileno()).st_size != size or
                        archive_sha(_CheckedReads(doc, self._checkpoint)) != expected):
                    raise Rejected("本机暂存未能逐字节读回原历史归档")
                still_named()
        names()
        if _directory_generation(stage.parent) != original:
            raise Rejected("读回时原本机暂存目录已改变")
        return original


def _directory_generation(fd):
    meta = os.fstat(fd)
    return (*native_identity(meta, stat.S_IFDIR), meta.st_mtime_ns, meta.st_ctime_ns)


def _observe_subtree(stage, sink, rows, *, checkpoint=None):
    original = _directory_generation(stage.parent)
    if observe_held(stage.parent, stage.identity, require_ids=True, check_held=sink.check,
                    checkpoint=checkpoint).rows != rows:
        raise Rejected("原私有暂存目录读回不等于所选子目录")
    if _directory_generation(stage.parent) != original:
        raise Rejected("读回时原私有暂存目录已改变")
    return original
