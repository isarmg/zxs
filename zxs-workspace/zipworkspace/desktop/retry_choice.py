"""A fresh consent to settle the *existing* frozen operation, never a second capture."""
from contextlib import contextmanager

from .. import lifecycle, worktree
from ..archive import read_managed_document, stamp
from ..document import archive_sha, check_relative, relative_document
from ..model import Commit, Rejected
from ..publish import Conflict, StoreReadback
from .commit_choice import CommitChoice, _baseline


class PendingRetry(CommitChoice):
    replay = True

    def __init__(self, submitted):
        if (submitted.replay or not submitted.used or not submitted.pending_seen or submitted.conflict
                or submitted.marker is None or submitted.digest is None):
            raise Rejected("需要先只读查到本窗口这笔无已知冲突的待决原操作")
        submitted.pending_seen = False  # A cancellation, refusal or loss requires another explicit query.
        self._bind(submitted.live, submitted.operation)  # No new operation ID, no reopening of the old decision.
        self.base, self.marker, self.digest = submitted.base, submitted.marker, submitted.digest
        self.origin, self.candidate = submitted, None
        self.leaf = self.operation + ".zxs"

    def _pending(self):
        with worktree.locked(self.live.state, read_only=True, live=self.live) as (con, row, _):
            self.live.check_instance(row, lifecycle.instance(con))
            lifecycle.require_visible(con, row)
            if (_baseline(row) != self.base or row["pending"] is None or
                    con.execute("SELECT 1 FROM completed WHERE operation=? UNION ALL "
                                "SELECT 1 FROM forked_original WHERE operation=?", (self.operation,self.operation)).fetchone()):
                raise Rejected("当前不再是这笔原待决或原基线；请只读查询，不另发")
            self.check_marker(Commit.decode(row["pending"]), row["pending_sha"])

    def check_candidate(self):
        check_relative(self.live.state_fd, self.leaf, self.candidate, self.candidate_stamp, single=True)
        if self.ack_fd is not None: self._check_ack()

    def _prepare(self):
        self.store = StoreReadback(self.live.store, self.live.store_fd)
        self.qualify()
        self._pending()
        self._open_base()
        src = self.stack.enter_context(relative_document(self.live.state_fd, self.leaf, single=True))
        observed = stamp(src)
        record, _ = read_managed_document(src, display_name=self.leaf)
        self.check_marker(record, archive_sha(src))
        check_relative(self.live.state_fd, self.leaf, src, observed, single=True)
        self.candidate_stamp, self.candidate = observed, src
        try:
            original_slot = self.stack.enter_context(self.store.verified_slot(self.marker,self.digest))
            if original_slot is not None: self.ack_fd, self._check_ack = original_slot
        except Conflict:
            self.origin.conflict = True
            raise
        self._pending()  # Expensive archive reads never run with a SQLite lock held.
        self.check_candidate()
        check_relative(self.live.store_fd, self.name, self.original, self.observed)
        action = ("该正式槽已是核验过的原归档。本次只确认这个原文件的回执，不上传；它若消失或被换就停止。"
                  if self.ack_fd is not None else
                  "核验时正式槽不存在。只用这份已冻结的原候选尝试不可覆盖发布；相同结果只确认，不同结果不覆盖。")
        return (f"原工作树：{self.live.work}\n原 STATE：{self.live.state}\n原 STORE：{self.live.store}\n"
                f"原操作：{self.operation}\n冻结归档 SHA-256：{self.digest}\n"
                f"原分支：{self.marker.branch} · 第 {self.marker.generation} 代\n正式槽：{self.marker.filename}\n\n"
                f"{action}\n不读取或冻结当前工作文件，后来保存的编辑不会进入这笔原操作。")

    @contextmanager
    def candidate_source(self, marker):
        self.check_marker(marker, self.digest)
        self.check_candidate()
        yield self.candidate  # The very FD verified before the user consented.
        self.check_candidate()

    def check_row(self, con, row):
        super().check_row(con, row)
        self.check_candidate()

    def run(self):
        if self.closed or not self.ready or self.used: raise Rejected("本次原操作续作决定不可复用")
        self.used = True
        try:
            self.check_base()
            self.check_candidate()
            return worktree.finish(self.live.state, self.operation, selected=self)
        except Conflict:
            self.origin.conflict = True
            raise
        finally:
            self.close()
