"""Whole archives on qualified macOS. No extraction or worktree registration."""
from contextlib import ExitStack, closing
import ctypes
import errno
import os
from pathlib import Path
import stat
import sys

from . import macos_read
from ._macos_copy_evidence import (_absent, _attributes, apfs, check_literal_path, check_spelling,
                                  native, require_exclusive_rename)
from ._transfer import _BeforePublication, _CheckedReads
from .archive import read_managed_document, stamp
from .comparison import Folder
from .document import PositionalDocument, archive_sha, check_relative, native_identity
from .freeze import _dir_flags, pack_chosen_folder
from .model import Rejected, member_name, new_id
from .publish import Uncertain, rename_new_at, require_native_writer


def _outside_source(parent, source):
    """Prove the held destination isn't the source or inside it."""
    current = os.dup(parent)
    try:
        for _ in range(256):
            here = native_identity(os.fstat(current), stat.S_IFDIR)
            if here == source: raise Rejected("新快照须保存在所选原目录之外")
            above = os.open("..", _dir_flags(), dir_fd=current)
            try:
                if native_identity(os.fstat(above), stat.S_IFDIR) == here: return
                old, current, above = current, above, None
                os.close(old)
            finally:
                if above is not None: os.close(above)
        raise Rejected("无法证明所选目标在原目录之外")
    finally:
        os.close(current)


class FolderSnapshot(_BeforePublication):
    """The one explicitly selected on-volume folder → fresh private Store ZIP."""
    _cancel_note = "已停止制作快照；未发布正式名称。已出现的私有暂存不会自动清理。"

    def __init__(self, folder, spelling):
        if sys.platform != "darwin" or not isinstance(folder, Folder) or folder._fd is None:
            raise Rejected("只接受当前实际选择的 macOS 目录")
        self.path = Path(spelling)
        if (self.path.anchor != "/" or str(self.path) != spelling or ".." in self.path.parts
                or self.path.name.startswith(".") or self.path.suffix.lower() != ".zxs"
                or len(member_name(self.path.name)) != 1):
            raise Rejected("选择目录之外的准确新 .zxs 名称")
        self.stack = ExitStack()
        self.ready = self.used = self.closed = False
        self._init_cancellation()
        self.stage = ".zipworkspace-folder-" + new_id() + ".partial"
        self.repo, self.branch, self.operation = (new_id() for _ in range(3))
        try:
            self.dest = self.stack.enter_context(closing(Folder.choose(self.path.parent)))
            self.source = self.stack.enter_context(closing(folder))
        except BaseException:
            self.close()
            raise

    def close(self):
        if not self.closed:
            self.closed = True
            self.stack.close()

    def _source_member(self, path, fd):
        self._checkpoint()
        meta = os.fstat(fd)
        directory = stat.S_ISDIR(meta.st_mode)
        native_identity(meta, stat.S_IFDIR if directory else stat.S_IFREG)
        if (meta.st_uid != os.geteuid() or meta.st_flags != 0 or meta.st_mode & 0o7022
                or not directory and meta.st_nlink != 1 or apfs(fd, fresh_snapshot=True) != self.volume):
            raise Rejected("源目录及每个成员须驻留、由本人所有、无特殊标志或链接，并位于同一实际 APFS 卷")
        check_literal_path(path, fd)
        native().no_payload_metadata(path, fd, directory=directory)
        self._checkpoint()

    def _inputs(self):
        self._checkpoint()
        p, source = self.dest.check(), self.source.check()
        _attributes(p, directory=True)
        check_literal_path(self.dest.evidence.path, p)
        if apfs(p, fresh_snapshot=True) != self.volume: raise Rejected("原目的 APFS 卷或目录已改变")
        _outside_source(p, self.source.evidence.identity)
        self._source_member(self.source.evidence.path, source)
        return p, source

    def review(self):
        if self.closed or self.used or self.ready: raise Rejected("请重新选择这个来源与新的输出名称")
        try:
            with macos_read.no_materialization():
                require_native_writer()
                self.volume = apfs(self.dest.check(), fresh_snapshot=True)
                p, _ = self._inputs()
                require_exclusive_rename(p)
                _absent(p, self.stage)
                _absent(p, self.path.name)
            self.ready = True
            return (f"原目录（只读）：{str(self.source.evidence.path)!r}\n新文件：{str(self.path)!r}\n"
                    f"新仓库：{self.repo}\n新分支：{self.branch}\n第 1 代 · 节点：{self.operation}\n"
                    f"独占目的目录中的暂存名：{self.stage}\n\n"
                    "两处须在同一已核本机 APFS 卷；不接网络卷或仅在云端的文件。"
                    "只保存普通文件正文和目录，原权限与时间不作为存档元数据。"
                    "遇到可见扩展属性、资源叉、ACL、卷或原件的系统隔离标记，或者无法证实的成员，整次拒绝，不剥除或略过。"
                    "在所选 0700 私有目录内逐项读取、完整校验和同步后才一次无覆盖发布。"
                    "可在正式改名前请求取消；失败或取消可能留有上列私有文件，不自动删除或重试。"
                    "不创建 STATE、不写现有版本、不打开或挂载新包。")
        except BaseException:
            self.close()
            raise

    def _output(self, p, name, out, before=None):
        seen = stamp(out)
        if before is not None and seen != before:
            raise Rejected("原暂存文件的内容代已改变")
        fd = out.fileno()
        _attributes(fd)
        if apfs(fd, fresh_snapshot=True) != self.volume: raise Rejected("输出已离开所选实际 APFS 卷")
        path = self.dest.evidence.path / name
        check_literal_path(path, fd)
        check_spelling(fd, name)
        check_relative(p, name, out, seen, single=True)
        native().no_payload_metadata(path, fd)
        check_relative(p, name, out, seen, single=True)
        return seen

    def run(self, *, confirmed=False):
        if confirmed is not True or not self.ready or self.used or self.closed:
            raise Rejected("只允许明确确认一次已核查的原目录和新位置")
        self.used = True
        phase = "none"
        try:
            # Include closing the held directories, the output, and the
            # policy restore in the same classification by actual phase.
            with closing(self), macos_read.no_materialization(), ExitStack() as output:
                p, source = self._inputs()
                require_exclusive_rename(p)
                _absent(p, self.path.name)
                os.fsync(p)  # Prove the original held parent's barrier before creating anything.
                self._inputs()
                flags = os.O_CREAT | os.O_EXCL | os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
                try: fd = os.open(self.stage, flags, 0o600, dir_fd=p)
                except FileExistsError as error:
                    raise Rejected("私有暂存名后来被占用；不会打开、覆盖或换名") from error
                output.callback(os.close, fd)
                phase = "stage"
                out = output.enter_context(os.fdopen(fd, "w+b", closefd=False))
                self._output(p, self.stage, out)
                # Source data is read once into this file; the second pass is
                # a directory/identity/metadata recheck, not another body read.
                self._phase("正在读取原目录正文并写入私有 Store ZIP")
                marker, length = pack_chosen_folder(
                    source, self.source.evidence.identity, out, self.repo, self.branch, self.operation,
                    check_held=lambda path, child: self._source_member(self.source.evidence.path/path, child),
                    checkpoint=self._checkpoint, on_read=self._bytes_processed,
                    before_inventory=lambda: self._phase("正在复核原目录清单与成员元数据"))
                out.flush()
                before = self._output(p, self.stage, out)
                self._phase("正在同步并读回私有快照，尚未发布")
                os.fsync(fd)
                out.seek(0)
                actual, tree = read_managed_document(_CheckedReads(out, self._checkpoint, self._bytes_processed),
                                                     display_name=self.path.name)
                if marker != actual or tree.total != length:
                    raise Rejected("私有快照读回不等于这次从原目录生成的整树")
                os.fsync(p)
                self._inputs()
                self._output(p, self.stage, out, before)
                require_exclusive_rename(p)
                _absent(p, self.path.name)
                self._begin_publication()
                self._phase("正在原生发布并同步原目的目录")
                phase = "rename"
                try: rename_new_at(p, self.stage, self.path.name)
                except FileExistsError as error:
                    phase = "stage"
                    raise Rejected("正式名称后来被占用，没有覆盖") from error
                published = self._output(p, self.path.name, out)
                if published[:4] != before[:4]:
                    raise Rejected("正式名称不是完整核验后的原暂存文件")
                os.fsync(p)
                self._inputs()
                self._output(p, self.path.name, out, published)
                _absent(p, self.stage)
            return f"已核验并另存新快照：{str(self.path)!r}\n第 1 代节点：{marker.operation}。未登记或切换工作树。"
        except Exception as error:
            if phase == "rename":
                raise Uncertain(f"正式名称可能已出现，无法确认：{str(self.path)!r}。先核对原位置，不覆盖或重试。") from error
            if phase == "stage":
                raise Rejected(f"{error}。未发布；私有暂存 {self.stage} 可能保留，不自动删除。") from error
            raise


class SnapshotCopy:
    """The picker pins source and destination. Review is read-only; run is explicit and one-shot."""
    def __init__(self, selected, directory, *, as_zip=False):
        if sys.platform != "darwin": raise Rejected("完整快照另存仅支持符合条件的本机 macOS 目录")
        self.stack = ExitStack()
        self.selected, self.sha, self.closed, self.used = selected, None, False, False
        self.original_name = selected.path.name
        if (self.original_name.startswith(".") or len(member_name(self.original_name)) != 1 or
                selected.path.suffix.lower() != ".zxs"):
            raise Rejected("必须有一个非隐藏的普通 .zxs 原文件名")
        if selected.held is None or selected.held.closed: raise Rejected("已失去原浏览句柄，请重新选择原快照")
        self.as_zip = as_zip
        self.final = selected.path.with_suffix(".zip").name if as_zip else self.original_name
        if len(member_name(self.final)) != 1: raise Rejected("目标文件名不是普通单一名称")
        self.stage = ".zipworkspace-" + new_id() + ".partial"
        try:
            with macos_read.no_materialization():
                require_native_writer()
                fd = selected.held.duplicate()  # never reopen a selected archive's content by name
                self.stack.callback(os.close, fd)
                self.src = PositionalDocument(fd, selected.observed[2])
                self.folder = Folder.choose(directory)
                self.stack.callback(self.folder.close)
                self.destination = self.folder.evidence.path / self.final
                self.check_names()
        except BaseException:
            self.close()
            raise

    def close(self):
        if not self.closed:
            self.closed = True
            self.stack.close()

    def check_names(self):
        if self.closed: raise Rejected("本次另存选择已关闭")
        chosen = self.selected
        named = chosen.path.stat(follow_symlinks=False)
        if (stamp(self.src) != chosen.observed or
                (*native_identity(named, stat.S_IFREG), named.st_size, named.st_mtime_ns, named.st_ctime_ns) != chosen.observed or
                chosen.exact is not None and (chosen.exact.path != chosen.path or chosen.exact.observed != chosen.observed
                                               or chosen.exact.marker != chosen.browser.commit)):
            raise Rejected("所选归档已改变；不能改用另一个同名文件或分支头")
        dest = self.folder.check()
        if apfs(self.src.fileno()) != apfs(dest): raise Rejected("源归档和目的目录必须在同一个本地 APFS 卷")
        check_spelling(self.src.fileno(), self.original_name)
        _attributes(self.src.fileno())
        _attributes(dest, directory=True)
        return dest

    def new_leaf(self, parent):
        try: os.stat(self.final, dir_fd=parent, follow_symlinks=False)
        except FileNotFoundError: return
        raise Rejected("目的名称已存在；不覆盖、不改名")

    def review(self):
        if self.closed or self.used or self.sha is not None: raise Rejected("必须重新选择一次归档与私有目录")
        try:
            with macos_read.no_materialization():
                fd = self.check_names()
                self.new_leaf(fd)
                # Pin exact original ZIP bytes to the same file and complete logical evidence.
                marker, _ = read_managed_document(self.src, display_name=self.original_name)
                digest = archive_sha(self.src)
                if marker != self.selected.browser.commit or self.selected.exact and digest != self.selected.exact.sha256:
                    raise Rejected("原节点或原 ZIP 字节与所选快照不符")
                api = native()
                attrs = api.attrs(self.src.fileno())
                with api.quarantine(self.selected.path, self.src.fileno()) as prop:
                    # Keep only immutable CF metadata, not a path-backed URL context through rename.
                    prop = api.retain(prop)
                    self.stack.callback(api.release, prop)
                    self.check_names()
                    self.new_leaf(fd)
            self.attrs, self.prop, self.sha = attrs, prop, digest
            format_note = ("仅把新副本的后缀改为 .zip；原包字节、归档注释及版本标识全部保留。"
                           if self.as_zip else "副本沿用原 .zxs 文件名。")
            return (f"已选原归档：{self.selected.path}\n节点：{marker.operation}\n完整 ZIP：{self.selected.observed[2]:,} B"
                    f"\nSHA-256：{digest}\n新文件：{self.destination}\n\n"
                    + format_note + "\n"
                    "只在当前同一 APFS 卷建立新文件，初期与原包共享数据块；不是另一块介质上的独立备份，"
                    "不适用于从 SMB 或另一外盘直接复制。不打开、不解包、不创建工作树、不覆盖。系统会在这个目录先克隆"
                    "整个原件及当时随原件复制的元数据，可能包括私有下载地址。核实系统隔离来源和完整 ZIP 后才"
                    f"产生正式名。即使拒绝发布，私有暂存 {self.stage} 也可能已经包含完整归档及当时的元数据。"
                    "不显示或发送来源地址，不自动清除暂存或重试。确认开始后须等待操作返回。")
        except BaseException:
            self.close()
            raise

    def _origin(self, path, fd):
        api = native()
        if api.attrs(fd) != self.attrs: raise Rejected("原文件或副本的可见来源元数据不符")
        with api.quarantine(path, fd) as current:
            if not api.equal(self.prop, current): raise Rejected("系统返回的隔离来源已经改变")

    def _target(self, parent, name, out):
        check_relative(parent, name, out, stamp(out), single=True)
        check_spelling(out.fileno(), name)
        _attributes(out.fileno())
        if apfs(out.fileno()) != apfs(parent): raise Rejected("暂存或正式文件不在原目的卷")
        return self.folder.evidence.path / name

    def run(self, *, confirmed=False):
        if confirmed is not True or self.closed or self.used or self.sha is None:
            raise Rejected("只允许另行确认一次已经核验的完整归档副本")
        self.used = True
        phase = "none"
        try:
            with macos_read.no_materialization(), ExitStack() as output:
                p = self.check_names()
                self.new_leaf(p)
                self._origin(self.selected.path, self.src.fileno())
                os.fsync(p)  # Confirm the actual parent's barrier before the first new file.
                self.check_names()
                self._origin(self.selected.path, self.src.fileno())
                # fclonefileat consumes this already-held source FD and creates a
                # complete new file, or fails. No raw xattr writes, metadata-only
                # bulk overwrite, ACL changes, truncating opens or copy fallback.
                api = native()
                if api.clone(self.src.fileno(), p, os.fsencode(self.stage), 0) != 0:
                    err = ctypes.get_errno()
                    if err == errno.EEXIST: raise Rejected("临时名称已被占用，不读取或清除它")
                    raise OSError(err, "APFS 未确认创建整个私有克隆；没有发布、回退或重试")
                phase = "stage"
                # The system returns no new FD. This first open relies on the
                # admitted private parent AND the explicit no-other-writers condition.
                self.check_names()
                fd = os.open(self.stage, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=p)
                output.callback(os.close, fd)
                out = output.enter_context(os.fdopen(fd, "rb", closefd=False))
                stage_path = self._target(p, self.stage, out)
                before = stamp(out)
                if before[:2] == self.selected.observed[:2] or before[2] != self.selected.observed[2]:
                    raise Rejected("暂存不是相同长度的独立普通文件")
                self._origin(stage_path, fd)
                if archive_sha(out) != self.sha: raise Rejected("私有克隆的完整 ZIP 字节读回不符")
                os.fsync(fd)
                self._origin(stage_path, fd)
                self._origin(self.selected.path, self.src.fileno())
                os.fsync(p)
                self.check_names()
                self._target(p, self.stage, out)
                check_relative(p, self.stage, out, before, single=True)
                self.new_leaf(p)
                # RENAME_EXCL protects the final name. The source name is under the
                # operator's private/quiescent parent, not an atomic FD/name comparison.
                phase = "rename"
                try: rename_new_at(p, self.stage, self.final)
                except FileExistsError:
                    phase = "stage"
                    raise
                os.fsync(p)
                final_path = self._target(p, self.final, out)
                after = stamp(out)
                if after[:4] != before[:4]: raise Rejected("改名后的文件代或长度与原暂存不符")
                self._origin(final_path, fd)
                self._origin(self.selected.path, self.src.fileno())
                self.check_names()
                check_relative(p, self.final, out, after, single=True)
                try: os.stat(self.stage, dir_fd=p, follow_symlinks=False)
                except FileNotFoundError: pass
                else: raise Rejected("改名后暂存名仍存在或被他人占用")
            # Releasing the original evidence or the thread policy may also fail after rename.
            self.close()
            kind = "原样完整 ZIP" if self.as_zip else "同名完整归档"
            return f"已另存{kind}：{self.destination}\n原 ZIP SHA-256：{self.sha}。未打开、提取或签出。"
        except Exception as error:
            if phase == "rename":
                raise Uncertain(f"正式名称可能已出现，无法确认本次结果：{self.destination}。请查看原位置；不覆盖或重试。") from error
            if phase == "stage":
                raise Rejected(f"{error}。未确认发布；私有暂存 {self.stage} 可能保留，不自动删除。") from error
            raise
        finally:
            self.close()
