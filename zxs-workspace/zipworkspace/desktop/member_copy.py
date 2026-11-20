"""Linux Qt copies one selected archive member, with native non-overwrite publication."""
from contextlib import ExitStack, closing
import hashlib
import os
from pathlib import Path
import stat
import sys

from .. import private_stage
from ..archive import copy_body, stored_member, stored_subtree
from ..comparison import Folder
from ..document import native_identity
from ..freeze import _distinct_roots, observe_held
from ..linux_authority import ProcView as _ProcView, owned_directory as _directory
from ..linux_mount import _ordinary_user
from ..model import CHUNK, MAX_CD, MAX_FILE, Rejected, member_name, new_id
from ..private_candidates import held_candidates
from ..private_freeze import local_mount, mount_id
from ..publish import Uncertain, rename_new_at, require_native_writer


def _generation(m):
    return (*native_identity(m, stat.S_IFREG), m.st_size, m.st_mtime_ns, m.st_ctime_ns)


class _Positional:
    """Never change the offset shared by the displayed FD and the FUSE helper."""
    def __init__(self, fd, end):
        self.fd, self.end, self.pos = fd, end, 0

    def tell(self): return self.pos

    def seek(self, n, whence=os.SEEK_SET):
        origin = {os.SEEK_SET: 0, os.SEEK_CUR: self.pos, os.SEEK_END: self.end}[whence]
        if origin+n < 0: raise Rejected("negative ZIP offset")
        self.pos = origin+n
        return self.pos

    def read(self, n):
        if not 0 <= n <= MAX_CD or self.pos+n > self.end:
            raise Rejected("ZIP read exceeds its selected byte window")
        data = os.pread(self.fd, n, self.pos)
        self.pos += len(data)
        return data


def _new_path(spelling, *, scratch=False):
    path = Path(spelling)
    if (path.anchor != "/" or str(path) != spelling or ".." in path.parts or
            not (2 if scratch else 3) <= len(path.parts) <= 66 or path.parts[1] in ("dev", "proc", "sys") or
            (not scratch and len(member_name(path.name)) != 1)):
        raise Rejected("请选择明确的现有本机目录或其下一个新名称")
    return path


def _check_source(selected, source):
    if (_generation(os.fstat(source)) != selected.observed or
            _generation(selected.path.stat(follow_symlinks=False)) != selected.observed):
        raise Rejected("选中的原归档已被移走、替换或修改；不会打开同名文件或改选分支头")


def _unoccupied(parent, name):
    try: os.stat(name, dir_fd=parent, follow_symlinks=False)
    except FileNotFoundError: return
    raise Rejected("目的名称已有文件、目录或链接；不覆盖")


class MemberCopy:
    """A one-use choice: prepare only observes; run writes only with explicit consent."""
    def __init__(self, selected, directory, entry, spelling):
        if sys.platform != "linux" or selected.held is None:
            raise Rejected("另存单个成员需要 Linux 上当前仍持有原件的快照")
        if (entry.kind != "file" or selected.browser.entry(directory, entry.name) is not entry or
                type(entry.size_bytes) is not int or not 0 <= entry.size_bytes <= MAX_FILE):
            raise Rejected("只能另存本次已核验的一条普通成员")
        self.path = _new_path(spelling)
        self.selected, self.entry = selected, entry
        self.member = f"{directory}/{entry.name}" if directory else entry.name
        self.stack = ExitStack()
        self.ready = self.used = self.closed = False
        self.source = selected.held.duplicate()
        self.stack.callback(os.close, self.source)

    def close(self):
        if not self.closed:
            self.closed = True
            self.stack.close()

    def _source(self):
        _check_source(self.selected, self.source)

    def _parent(self):
        with ExitStack() as check:
            _, ids = _directory(check, self.path.parent, self.uid, self.proc.read)
        if ids != self.parents or local_mount(self.parent, read_proc=self.proc.read) != self.parents[-1][-1]:
            raise Rejected("选中的目的目录或上级目录不再是原件")

    def _absent(self):
        _unoccupied(self.parent, self.path.name)

    def review(self):
        if self.closed or self.ready or self.used: raise Rejected("请重新选择一次成员和目的文件")
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
            self._parent()
            self.ready = True
            return (f"原归档：{str(self.selected.path)!r}\n节点：{self.selected.browser.commit.operation}\n"
                    f"成员：{self.member!r}\n正文：{self.entry.size_bytes:,} 字节\nSHA-256：{self.entry.sha256}\n"
                    f"新文件：{str(self.path)!r}\n\n仅保存这一项的字节，需容纳一份正文。"
                    "核完这一成员并读回新文件后才公开名称；不覆盖、不创建工作树、不打开结果。"
                    "提交这次确认后须等待结果，不能从窗口中途取消。"
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

    def _copy(self, body, fd):
        size, sha = self.entry.size_bytes, self.entry.sha256
        digest = hashlib.sha256()
        left = size
        while left:
            data = body.read(min(left, CHUNK))
            if not data or len(data) > left: raise Rejected("成员没有达到原核验长度")
            digest.update(data)
            left -= len(data)
            view = memoryview(data)
            while view:
                written = os.write(fd, view)
                if not 0 < written <= len(view): raise Rejected("未能写完匿名文件")
                view = view[written:]
        # ZipExtFile checks CRC upon EOF, including empty members.
        if body.read(1) or digest.hexdigest() != sha: raise Rejected("成员与原核验的长度或 SHA-256 不符")

    def _verify(self, fd):
        size = self.entry.size_bytes
        before = self._output(fd, 0, size)
        os.fsync(fd)
        digest, read = hashlib.sha256(), 0
        while read < size:
            data = os.pread(fd, min(CHUNK, size-read), read)
            if not data: raise Rejected("匿名副本读回提前结束")
            read += len(data)
            digest.update(data)
        if os.pread(fd, 1, size) or digest.hexdigest() != self.entry.sha256 or self._output(fd, 0, size) != before:
            raise Rejected("匿名副本读回不等于原成员")
        return before

    def run(self, *, confirmed=False):
        if confirmed is not True or not self.ready or self.closed or self.used:
            raise Rejected("只允许明确确认一次已核验的成员和目的文件")
        self.used = True
        may_be_linked = False
        try:
            with self.stack, ExitStack() as temp:
                if _ordinary_user(read_proc=self.proc.read) != self.uid:
                    raise Rejected("不再是确认这次另存的普通用户")
                self._source()
                self._parent()
                self._absent()
                reader = _Positional(self.source, self.selected.observed[2])
                size = self.entry.size_bytes
                with stored_member(reader, reader.end, self.selected.browser.commit, self.member, size) as body:
                    # O_EXCL would forbid linking this O_TMPFILE; no named fallback.
                    out = os.open(".", os.O_TMPFILE | os.O_RDWR | os.O_CLOEXEC, 0o600, dir_fd=self.parent)
                    temp.callback(os.close, out)
                    self._output(out, 0, 0)
                    self._copy(body, out)
                sealed = self._verify(out)
                self._source()
                self._parent()
                name = str(out)
                if (not stat.S_ISLNK(os.stat(name, dir_fd=self.proc.fds, follow_symlinks=False).st_mode) or
                        self._output(out, 0, size) != sealed or
                        self._output(out, 0, size, (self.proc.fds, name), follow=True) != sealed):
                    raise Rejected("procfs 中没有核验通过的原匿名文件")
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
                    if (self._output(out, 1, size, (self.parent, self.path.name)) != published or
                            self._output(out, 1, size) != published):
                        raise Rejected("正式名称或原成员副本在确认时改变")

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


class DirectoryCopy:
    """A selected subtree becomes visible only as one complete directory."""
    def __init__(self, selected, directory, entry, spelling):
        if (sys.platform != "linux" or selected.held is None or entry.kind != "directory"
                or selected.browser.entry(directory, entry.name) is not entry):
            raise Rejected("只能另存 Linux 窗口中仍持原件的一条已核验目录")
        self.path = _new_path(spelling)
        self.selected = selected
        self.root = f"{directory}/{entry.name}" if directory else entry.name
        self.stage_name = ".zip-workspace-export-" + new_id()
        self.stack = ExitStack()
        self.private = None
        self.ready = self.used = self.closed = False
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
            _check_source(self.selected, self.source)
            prefix = self.root + "/"
            self.rows = {path[len(prefix):]: (entry.kind == "directory", entry.size_bytes or 0, entry.sha256)
                         for path, entry in self.selected.browser.verified_entries() if path.startswith(prefix)}
            self.size = sum(row[1] for row in self.rows.values())
            self.ready = True
            return (f"原归档：{str(self.selected.path)!r}\n节点：{self.selected.browser.commit.operation}\n"
                    f"所选子目录：{self.root!r}\n内容：{len(self.rows):,} 项，{self.size:,} 字节（包括空目录）\n"
                    f"新的完整目录：{str(self.path)!r}\n私有暂存：{str(self.private.path / self.stage_name)!r}\n\n"
                    "仅写这棵子目录，不提取其他文件，不登记或签出工作树。"
                    "在独占私处复核全部内容后，只改名一次；同名不覆盖。"
                    "只保存普通文件字节和目录结构，不复制来源元数据，也不打开或执行结果。"
                    "本版本确认后须等待，不能中途取消。失败可能在私处留材料，不删除或另发。")
        except BaseException:
            self.close()
            raise

    def run(self, *, confirmed=False):
        if confirmed is not True or not self.ready or self.used or self.closed:
            raise Rejected("只允许明确确认一次已核验的子目录和新位置")
        self.used = True
        stage_attempted = may_have_moved = False
        try:
            with self.stack:
                read = self.proc.read
                if _ordinary_user(read_proc=read) != self.uid: raise Rejected("普通用户已改变，不能另存")
                _check_source(self.selected, self.source)
                self._parents()
                _unoccupied(self.parent, self.path.name)
                reader = _Positional(self.source, self.selected.observed[2])
                with stored_subtree(reader, reader.end, self.selected.browser.commit, self.root, self.rows) as (archive, files):
                    _check_source(self.selected, self.source)
                    self._parents()
                    _unoccupied(self.parent, self.path.name)
                    stage_attempted = True
                    with private_stage.new_child(self.private, self.stage_name, self.mount, read_proc=read) as stage:
                        sink = private_stage._Output(stage, self.mount, read_proc=read)
                        for name, (directory, _, _) in sorted(self.rows.items()):
                            if directory: sink.mkdir(name)
                        for name, original in files:
                            _, size, sha = self.rows[name]
                            with archive.open(original) as body, sink.file(name) as dest:
                                if copy_body(body, size, dest) != sha:
                                    raise Rejected("私有副本与所选原成员的 SHA-256 不符")
                        sink.sync(self.rows)
                        sealed = _observe_subtree(stage, sink, self.rows)
                        _check_source(self.selected, self.source)
                        self._parents()
                        self._named_stage(self.private.parent, self.stage_name, stage)
                        if _directory_generation(stage.parent) != sealed:
                            raise Rejected("原私有暂存目录已改变")
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
                raise Rejected(f"{err}\n原私处 {scratch!r} 可能仍有准备材料；未覆盖目的地，不自动清理或重试。") from err
            raise
        finally:
            self.closed = True


def _directory_generation(fd):
    meta = os.fstat(fd)
    return (*native_identity(meta, stat.S_IFDIR), meta.st_mtime_ns, meta.st_ctime_ns)


def _observe_subtree(stage, sink, rows):
    original = _directory_generation(stage.parent)
    if observe_held(stage.parent, stage.identity, require_ids=True, check_held=sink.check).rows != rows:
        raise Rejected("原私有暂存目录读回不等于所选子目录")
    if _directory_generation(stage.parent) != original:
        raise Rejected("读回时原私有暂存目录已改变")
    return original
