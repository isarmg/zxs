"""One proposed commit for the original worktree still held by this desktop window."""
from contextlib import ExitStack
import os
import stat

from .. import lifecycle, worktree
from ..archive import read_managed_document, stamp
from ..document import archive_sha, check_relative, native_identity, relative_document
from ..model import Commit, Rejected, check_id, new_id
from ..private_candidates import CandidateDirectory
from ..private_freeze import local_mount
from ..publish import Conflict, StoreReadback, require_native_writer


def _baseline(row):
    return (row["repo"], row["branch"], row["generation"], row["parent"], row["base_sha"])


class CommitChoice:
    purpose = "commit"

    def __init__(self, live):
        self._bind(live, new_id())

    def _bind(self, live, operation):
        self.operation = check_id(operation)
        self.live = live.borrow()
        self.stack = ExitStack()
        self.base = self.original = self.marker = self.digest = None
        self.used = self.closed = self.ready = self.pending_seen = False
        self.conflict = self.conflict_pending_seen = False
        self.ack_fd = None

    def close(self):
        if self.closed: return
        self.closed = True
        self.stack.close()

    def release(self):
        try: self.close()
        finally: self.live.close()

    def qualify(self):
        require_native_writer()
        if self.purpose == "commit" and not getattr(os, "O_TMPFILE", 0): raise Rejected("本机没有所需匿名冻结能力")
        self.live.check_catalog(self.live.state / "worktree.sqlite")
        if local_mount(self.live.work_fd) != local_mount(self.live.state_fd):
            raise Rejected("原工作树与私有 STATE 不再处于同一已准入挂载")
        local_mount(self.live.store_fd)  # This UI does not start writes on a network/unknown STORE.
        self.store.check_store()

    def prepare(self):
        if self.closed or self.ready: raise Rejected("不能重用旧的提交确认")
        try:
            detail = self._prepare()
            self.ready = True
            return detail
        except BaseException:
            self.close()  # A failed admission never leaves authority to commit or try this decision again.
            raise

    def _idle_base(self):
        with worktree.locked(self.live.state, read_only=True, live=self.live) as (con, row, _):
            self.live.check_instance(row, lifecycle.instance(con))
            lifecycle.require_visible(con, row)
            if row["pending"] is not None: raise Rejected("有原提交待决；不能另发操作")
            return _baseline(row)

    def _prepare_base(self, *, required=None):
        self.store = StoreReadback(self.live.store, self.live.store_fd)
        self.area = CandidateDirectory(self.live.state, self.live.state_fd,
                                      native_identity(os.fstat(self.live.state_fd), stat.S_IFDIR))
        self.qualify()
        self.base = self._idle_base()
        if required is not None and self.base != required:
            raise Rejected("本工作树的当前基线不再是窗口所选原节点；不打开其他代")
        self._open_base()

    def _prepare(self):
        self._prepare_base()
        _, branch, gen, node, digest = self.base
        return (f"工作树：{self.live.work}\n命令行 STATE：{self.live.state}\n原 STORE：{self.live.store}\n"
                f"分支：{branch}\n基线：第 {gen} 代 · {node}\n原包 SHA-256：{digest}\n"
                f"本次操作号：{self.operation}\n下一代：{gen+1}\n\n"
                "提交整个工作目录，生成新的不可变归档。不改写原代，不自动解决同代冲突。"
                "STATE 的冻结和新整包会同时占空间，STORE 还需一份整包；这里不保证容量。")

    def _open_base(self):
        repo, branch, gen, node, digest = self.base
        check_id(repo); check_id(branch); check_id(node)
        if (type(gen) is not int or not 0 < gen < (1 << 63)
                or self.purpose == "commit" and gen == (1 << 63) - 1):
            raise Rejected("工作树没有可继续的原基线")
        self.name = f"{repo}.{branch}.{gen:016x}.zxs"
        # No SQLite writer or lock persists across this potentially long read / consent dialog.
        try:
            doc = self.stack.enter_context(relative_document(self.live.store_fd, self.name))
        except FileNotFoundError as exc:
            raise Rejected("所选 STORE 中缺少原基线，不用窗口打开的散包代替，也不自动补发") from exc
        observed = stamp(doc)
        marker = read_managed_document(doc, display_name=self.name)[0]
        if ((marker.repo, marker.branch, marker.generation, marker.operation) != self.base[:4]
                or archive_sha(doc) != digest):
            raise Rejected("STORE 中原基线的节点或整包 SHA-256 不符")
        self.qualify()
        check_relative(self.live.store_fd, self.name, doc, observed)
        self.base_marker, self.observed, self.original = marker, observed, doc  # Only after both checks.

    def check_base(self):
        if self.closed or not self.ready or self.original is None: raise Rejected("未持有本次已核验的原基线")
        self.qualify()
        check_relative(self.live.store_fd, self.name, self.original, self.observed)

    def check_row(self, con, row):
        self.live.check_instance(row, lifecycle.instance(con))
        if _baseline(row) != self.base:
            raise Rejected("原工作树基线已改变；本次确认失效，不追随新的 HEAD")
        lifecycle.require_visible(con, row)
        self.check_base()

    def check_marker(self, marker, sha):
        if self.marker is None or marker != self.marker or sha != self.digest or marker.operation != self.operation:
            raise Rejected("记录并非本次已核验的冻结候选")

    def candidate_source(self, marker):
        return relative_document(self.live.state_fd, marker.operation+".zxs", single=True)

    def result(self):
        # Do not hand out a replay decision unless this particular original's read txn actually returned.
        self.pending_seen = self.conflict_pending_seen = False
        outcome = self._result()
        if self.conflict and outcome[0] == "pending":
            self.conflict_pending_seen = self.purpose == "commit"
            action = ("可另行核对竞争者及原冻结归档。" if self.conflict_pending_seen else "请保留原操作号。")
            return "pending", ("此操作仍待决，原槽核对曾发现冲突；不是已核实的竞争节点。"
                               "不会覆盖或续传。" + action)
        self.pending_seen = outcome[0] == "pending"
        return outcome

    def _result(self):
        # Metadata only. Never run finish(), read user bodies or publish while querying an outcome.
        with worktree.locked(self.live.state, read_only=True, live=self.live) as (con, row, _):
            self.live.check_instance(row, lifecycle.instance(con))
            done = con.execute("SELECT marker,digest FROM completed WHERE operation=?", (self.operation,)).fetchone()
            if done is not None:
                self.check_marker(Commit.decode(done["marker"]), done["digest"])
                return "completed", "原操作已记为完成；这是历史回执，未再次验证共享中当前是否仍有此文件。"
            forked = con.execute("SELECT successor FROM forked_original WHERE operation=?", (self.operation,)).fetchone()
            if forked is not None:
                if self.marker is None: raise Rejected("未持有本次冻结候选，不能确认分叉来源")
                return "forked", "原冲突已登记后继分叉操作：" + check_id(forked["successor"]) + "。这不表示新分支已发布。"
            if row["pending"] is not None:
                pending = Commit.decode(row["pending"])
                if pending.operation == self.operation:
                    self.check_marker(pending, row["pending_sha"])
                    return "pending", "本次原提交仍待决。不重新冻结；可另行核对并确认续作，也可用 CLI resume STATE OPERATION。"
                return "other", "当前有另一笔待决提交；不能作为本次回执。"
            return "unrecorded", "此原登记中未查到本次受理；可能有未登记的私有候选，不另发、不清除。"

    def run(self):
        if self.closed or not self.ready or self.used: raise Rejected("不能重复提交本次决定")
        self.used = True
        try:
            self.check_base()
            return worktree.commit(self.live.state, selected=self)
        except Conflict:
            self.conflict = True
            raise
        finally:
            self.close()
