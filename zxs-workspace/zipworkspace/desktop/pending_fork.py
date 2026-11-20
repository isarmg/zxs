"""One explicit new branch from this live window's already-frozen losing commit."""
import os
import stat

from .. import lifecycle, worktree
from ..archive import read_managed_document, stamp
from ..document import archive_sha, check_relative, native_identity, relative_document
from ..model import Commit, Rejected, new_id
from ..private_candidates import CandidateDirectory
from ..private_freeze import mount_id
from ..publish import Conflict, StoreReadback
from .commit_choice import CommitChoice, _baseline


class PendingFork(CommitChoice):
    purpose = "conflict-fork"

    def __init__(self, original):
        old = original.marker
        if (original.purpose != "commit" or not original.closed or not original.used
                or not original.conflict_pending_seen or not original.conflict or old is None or not original.digest):
            raise Rejected("请先只读查询本窗口发生冲突的原待决提交")
        repo, branch, generation, parent, _ = original.base
        if generation == 0:
            raise Rejected("第 1 代冲突不另造同一仓库的第二个起点；保留原待决")
        if (old.repo, old.branch, old.generation, old.parent) != (repo, branch, generation + 1, parent):
            raise Rejected("待决不是原分支的下一代普通提交")
        successor, branch_id = new_id(), new_id()
        if successor in (old.operation, parent) or branch_id == old.branch:
            raise Rejected("新分支或操作号冲突，未制作候选")
        self._bind(original.live, successor)
        original.conflict_pending_seen = False
        self.base, self.old, self.old_sha = original.base, old, original.digest
        self.proposed = Commit(old.repo, branch_id, 1, successor, old.parent, old.tree)

    def check_pending(self, con, row):
        self.live.check_instance(row, lifecycle.instance(con))
        lifecycle.require_visible(con, row)
        if (_baseline(row) != self.base or row["pending"] is None
                or Commit.decode(row["pending"]) != self.old or row["pending_sha"] != self.old_sha
                or con.execute("""SELECT 1 FROM completed WHERE operation IN (?,?)
                                  UNION ALL SELECT 1 FROM forked_original
                                  WHERE operation IN (?,?) OR successor IN (?,?)""",
                               (self.old.operation, self.operation) * 3).fetchone() is not None):
            raise Rejected("原待决、基线或操作收据已改变；没有另发")

    def _read_pending(self):
        with worktree.locked(self.live.state, read_only=True, live=self.live) as (con, row, _):
            self.check_pending(con, row)

    def _hold(self, parent, name, *, single=False):
        # Complete the checked named handoff with both FDs alive. Thereafter
        # _check_material is the final name barrier before recording the successor;
        # releasing this duplicate later does not re-open or re-check the old name.
        with relative_document(parent, name, single=single) as original:
            fd = os.dup(original.fileno())
            self.stack.callback(os.close, fd)
            doc = self.stack.enter_context(os.fdopen(fd, "rb", closefd=False))
            if mount_id(fd) != mount_id(parent):
                raise Rejected("归档不在原选定的实际挂载")
            return parent, name, doc, stamp(doc), single

    def _check_material(self):
        for parent, name, doc, observed, single in self.material:
            check_relative(parent, name, doc, observed, single=single)
            if mount_id(doc.fileno()) != mount_id(parent):
                raise Rejected("争用槽或原冻结候选已离开原挂载")

    def check_inputs(self):
        self.check_base()
        self._check_material()

    def check_empty_slot(self):
        self.store.check_store()
        try:
            os.stat(self.proposed.filename, dir_fd=self.store.fd, follow_symlinks=False)
        except FileNotFoundError:
            self.store.check_store()
            return  # Observation only. The publish step still uses an atomic no-replace rename.
        raise Conflict("选定新分支第 1 代名称已被占用；不覆盖或另换号码")

    def _prepare(self):
        statefd, storefd = self.live.state_fd, self.live.store_fd
        self.area = CandidateDirectory(self.live.state, statefd, native_identity(os.fstat(statefd), stat.S_IFDIR))
        self.store = StoreReadback(self.live.store, storefd)
        self.qualify()
        self._read_pending()
        self._open_base()
        name = self.old.operation + ".zxs"
        frozen = self._hold(statefd, name, single=True)
        self.candidate = frozen[2]
        if (read_managed_document(self.candidate, display_name=name)[0] != self.old
                or archive_sha(self.candidate) != self.old_sha):
            raise Rejected("持有的原冻结归档与待决原件不符")
        occupied = self._hold(storefd, self.old.filename)
        winner, _ = read_managed_document(occupied[2], display_name=self.old.filename)
        if ((winner.repo, winner.branch, winner.generation, winner.parent) !=
                (self.old.repo, self.old.branch, self.old.generation, self.old.parent)
                or winner.operation in (self.old.operation, self.operation)):
            raise Rejected("此正式槽不是同一父节点的有效同代竞争者；不制作候选")
        self.material = frozen, occupied
        self._read_pending()
        self.qualify()
        check_relative(storefd, self.name, self.original, self.observed)
        self._check_material()
        self.check_empty_slot()
        return (f"原工作目录：{self.live.work}\n原 STATE：{self.live.state}\n原 STORE：{self.live.store}\n"
                f"原提交：{self.old.operation}\n原冻结 ZIP SHA-256：{self.old_sha}\n"
                f"已核验占据原分支第 {self.old.generation} 代的竞争节点：{winner.operation}\n"
                f"新分支：{self.proposed.branch} · 第 1 代\n父节点仍为：{self.old.parent}\n"
                f"指定后继操作号：{self.operation}\n新正式槽：{self.live.store / self.proposed.filename}\n\n"
                "仅把这份已冻结的原提交另存为新分支，不扫描或写入当前工作文件，也不改原分支槽。"
                "后继发布且登记完成才切换本工作树的基线，后来编辑不会进入该包。")

    def _successor(self, con):
        record = con.execute("SELECT successor FROM forked_original WHERE operation=?", (self.old.operation,)).fetchone()
        return record["successor"] if record is not None else None

    def require_link(self, con):
        if self._successor(con) != self.operation:
            raise Rejected("未核到本次确认的旧操作和指定后继的原登记；不发布")

    def result(self):
        # A link is acceptance, not publication. Do not recheck STORE or write during inquiry.
        self.local_receipt_seen = False
        outcome = self._read_successor()
        self.local_receipt_seen = outcome[0] == "receipt_dir"
        return outcome

    def _read_successor(self):
        with worktree.locked(self.live.state, read_only=True, live=self.live, local_only=True) as (con, row, _):
            self.live.check_instance(row, lifecycle.instance(con))
            linked = self._successor(con)
            if linked is None:
                if (row["pending"] is not None and Commit.decode(row["pending"]) == self.old
                        and row["pending_sha"] == self.old_sha):
                    return "unaccepted", "旧操作仍待决，未查到本次后继受理；私有副本可能留存。不重发。"
                return "unknown", "未查到本次后继受理，旧操作也不再是原待决；不要重发。"
            if linked != self.operation:
                return "other", "原操作记录了另一个后继；不会将它当作本次结果或继续。"
            done = con.execute("SELECT marker,digest FROM completed WHERE operation=?", (self.operation,)).fetchone()
            if done is not None:
                self.check_marker(Commit.decode(done["marker"]), done["digest"])
                return self.completion_result("已核到这笔后继的完成历史回执；未重新验证 STORE 当前仍有此归档。")
            if row["pending"] is not None and Commit.decode(row["pending"]).operation == self.operation:
                self.check_marker(Commit.decode(row["pending"]), row["pending_sha"])
                return "pending", ("原登记确有这笔后继待决；正式槽可能已出现，不能当作完成。"
                                   f"此窗口仅查询；如需继续，用原 STATE 显式运行 resume STATE {self.operation}。")
            return "unknown", "原登记有此后继关系，但没有可核实的待决或完成回执；保留现场、不重发。"

    def run(self):
        if self.closed or not self.ready or self.used:
            raise Rejected("本次冲突分叉确认不可复用")
        self.used = True
        try:
            return worktree.fork_pending(self.live.state, self.old.operation, selected=self)
        finally:
            self.close()
