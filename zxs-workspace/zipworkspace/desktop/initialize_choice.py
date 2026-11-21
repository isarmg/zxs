"""Explicit registration of an existing local folder as a new CLI worktree, without committing."""
from contextlib import ExitStack, closing
import os
import sys

from .. import lifecycle, private_stage, worktree
from ..capacity import describe, writing
from ..comparison import Folder
from ..model import Rejected, member_name, new_id
from ..private_candidates import held_candidates
from ..private_freeze import local_mount
from ..publish import held_store, require_native_writer
from .checkout_action import _absent, _named_mount
from .live_checkout import LiveCheckout


def available():
    return (sys.platform == "linux" and bool(getattr(os, "O_PATH", 0) and getattr(os, "O_TMPFILE", 0))
            and os.scandir in os.supports_fd)


class InitializeChoice:
    """Keep all three selected original locations; no catalog or STORE write in review."""
    def __init__(self):
        if not available(): raise Rejected("此平台不能在窗口新建可提交的 Linux CLI 工作树")
        require_native_writer()
        self.instance, self.repo, self.branch = (new_id() for _ in range(3))
        self.stack = ExitStack()
        self.work = self.store = self.private = self.state = self.session = self.publication = self.extracted = None
        self.ready = self.used = self.closed = False

    def close(self):
        try:
            if self.session is not None: self.take_session().close()
        finally:
            if not self.closed:
                self.closed = True
                self.stack.close()

    def _pick(self, value):
        if self.closed or self.used or self.state is not None or value is not None:
            raise Rejected("请重新明确选择新仓库的各个位置")

    def pick_work(self, path):
        self._pick(self.work)
        self.work = self.stack.enter_context(closing(Folder.choose(path)))

    def pick_extracted(self, original):
        from .member_copy import ExtractedWork
        self._pick(self.work)
        if not isinstance(original, ExtractedWork): raise Rejected("没有这次提取完成后的原目录交接")
        self.work = self.stack.enter_context(closing(original.borrow()))
        self.extracted = original

    def pick_store(self, path):
        self._pick(self.store)
        self.store = self.stack.enter_context(held_store(path))

    def pick_private(self, path):
        self._pick(self.private)
        self.private = self.stack.enter_context(held_candidates(path))

    def _roots(self):
        if self.closed: raise Rejected("本次目录选择已关闭")
        p, w, s = self.private, self.work, self.store
        work_fd = w.check()
        private_stage.check_private(p)
        for path, held in ((w.evidence.path, work_fd), (p.path, p.parent), (s.path, s.fd)):
            _named_mount(path, held)
        mount = private_stage.same_mount(p.parent, work_fd)
        local_mount(s.fd)
        private_stage.separate_roots(p.parent, work_fd, s, mount)
        return work_fd, mount

    def _new(self):
        picked = self._roots()
        if not _absent(self.private.parent, self.state.name):
            raise Rejected("新 STATE 名称已被占用；不会接管或覆盖")
        return picked

    def review(self, leaf):
        self._pick(None)
        if self.work is None or self.store is None or self.private is None or len(member_name(leaf)) != 1:
            raise Rejected("必须选择三处原目录并输入一个新的 STATE 名称")
        self.state = self.private.path / leaf
        self._new()
        self.ready = True
        from_zip = (f"来自刚完成提取的原目录；原 ZIP SHA-256：{self.extracted.archive_sha256}\n"
                    "只登记原目录，不把归档注释或内容当作已提交历史。\n" if self.extracted is not None else "")
        return (f"现有工作目录：{str(self.work.evidence.path)!r}\n{from_zip}原 STORE：{str(self.store.path)!r}\n"
                f"新 CLI STATE：{str(self.state)!r}\n新实例：{self.instance}\n仓库：{self.repo}\n分支：{self.branch}\n\n"
                "只在新 STATE 中登记这个工作目录。不读工作文件正文，不移动或改写工作目录，"
                "不在 STORE 创建归档。不使用已有 STATE；此时没有已提交快照。"
                "成功后还须在本窗口另行停止写入并确认提交，才会生成第 1 代。")

    def take_session(self):
        result, self.session = self.session, None
        return result

    def run(self, *, confirmed=False):
        if confirmed is not True or not self.ready or self.closed or self.used:
            raise Rejected("只允许明确确认一次的新工作树登记")
        self.used = True
        incoming = None
        try:
            work_fd, mount = self._new()
            if self.extracted is not None: self.extracted.use()
            p = self.private
            self.publication = "state-mkdir-attempted"  # From here do not retry/adopt/clean a pathname.
            with private_stage.new_child(p, self.state.name, mount) as actual:
                incoming = LiveCheckout(self.work.evidence.path, self.state, self.store.path,
                                        self.instance, self.repo, work_fd, actual.parent, self.store.fd,
                                        initial_branch=self.branch)

                def checked_root():
                    self._roots()
                    _named_mount(self.state, actual.parent)
                    if private_stage.check_named(p.parent, self.state.name, actual.parent) != actual.identity:
                        raise Rejected("新 STATE 不再是本次实际创建的目录")
                    return os.fstat(work_fd)

                worktree.create_catalog(incoming.work, incoming.store, self.state, None, self.instance,
                                        checked_root=checked_root, live=incoming)
                with writing("STATE 上级目录", p.path, "同步本次新 STATE 名称；即使可见也不能视为已确认", fd=p.parent):
                    os.fsync(p.parent)
                checked_root()
                with worktree.locked(self.state, read_only=True, live=incoming) as (con, row, _):
                    incoming.check_instance(row, lifecycle.instance(con))
                    lifecycle.require_visible(con, row)
                    if ((row["repo"], row["branch"], row["generation"], row["parent"], row["base_sha"])
                            != incoming.initial_base or row["pending"] is not None or row["pending_sha"] is not None):
                        raise Rejected("没有核到本次新登记的空基线")
                checked_root()
                incoming.check_catalog(self.state / "worktree.sqlite")
            self.session, incoming = incoming, None
        except Exception as error:
            if self.publication is not None:
                raise Rejected(f"{describe(error)}\n原 STATE 可能已出现，无法确认本次登记完成。"
                               f"请保留现场；不接管、不删除、不重试或上传。\n原 STATE：{self.state}\n拟登记实例：{self.instance}") from error
            raise
        finally:
            self.closed = True
            try:
                if incoming is not None: incoming.close()
            finally:
                self.stack.close()
