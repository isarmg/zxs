"""One proposed commit for the original worktree still held by this desktop window."""
from contextlib import ExitStack
import os
import stat

from .. import lifecycle, worktree
from ..archive import read_managed_document, stamp
from ..document import archive_sha, check_relative, native_identity, relative_document
from ..model import Commit, Rejected, check_id, new_id
from ..private_candidates import CandidateDirectory
from ..private_freeze import local_mount, mount_id
from ..publish import Conflict, StoreReadback, require_native_writer

from .selection import Verification


def _baseline(row):
    return (row["repo"], row["branch"], row["generation"], row["parent"], row["base_sha"])


class _Receipt:
    """Process-local evidence for one operation, shared with its exact retries."""
    def __init__(self):
        self.completion_started = self.directory_remaining = False

    def completed_barrier(self):
        self.completion_started = self.directory_remaining = False


class CommitStopped(Rejected):
    """The user's stop request won before registering this operation."""


class _CommitPreparation(Verification):
    PHASE = dict(Verification.PHASE,
                 freeze="正在从原工作目录写入一份未命名的私有 Store ZIP",
                 sync="正在同步私有候选并核对原工作树")

    def __init__(self):
        super().__init__()
        self._past_stop = False

    def cancel(self):
        with self._lock:
            if self._past_stop: return False
            self._stop.set()
            return True

    def check(self):
        if self.cancelled:
            raise CommitStopped("已停止本次提交。没有登记本次待决或向 STORE 发布；"
                                "可能保留私有候选，不自动删除，也不能当作已受理的回执。")

    def before_intent(self):
        with self._lock:
            self.check()
            self._past_stop = True
            self._progress = ("正在登记本次原待决并继续同号发布；现在不能撤销", 0, None)

    def finish(self):
        with self._lock: self._past_stop = True


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
        self.receipt = _Receipt()
        self.local_receipt_seen = False
        self.preparation = _CommitPreparation() if self.purpose == "commit" else None
        self.ack_fd = None
        self.anonymous_candidate = None

    def close(self):
        if self.closed: return
        self.closed = True
        if self.preparation is not None: self.preparation.finish()
        self.anonymous_candidate = None
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
            if self.preparation is not None: self.preparation.check()
            detail = self._prepare()
            if self.preparation is not None: self.preparation.check()
            self.ready = True
            return detail
        except BaseException:
            self.close()  # A failed admission never leaves authority to commit or try this decision again.
            raise

    def _idle_base(self):
        with worktree.locked(self.live.state, read_only=True, live=self.live) as (con, row, _):
            self.live.check_instance(row, lifecycle.instance(con))
            lifecycle.require_visible(con, row)
            if row["pending"] is not None or row["pending_sha"] is not None:
                raise Rejected("有待决提交或不一致的原登记；不能另发操作")
            return _baseline(row)

    def _prepare_base(self, *, required=None, displayed=None):
        self.store = StoreReadback(self.live.store, self.live.store_fd)
        self.area = CandidateDirectory(self.live.state, self.live.state_fd,
                                      native_identity(os.fstat(self.live.state_fd), stat.S_IFDIR))
        self.qualify()
        self.base = self._idle_base()
        if required is not None and self.base != required:
            raise Rejected("本工作树的当前基线不再是窗口所选原节点；不打开其他代")
        if displayed is not None and self.base[:4] != displayed:
            raise Rejected("登记基线与窗口状态不同；请刷新，不会选另一代")
        if displayed is None: self._open_base()
        else: self._open_base(held_mount=True)

    def _prepare(self):
        self._prepare_base()
        _, branch, gen, node, digest = self.base
        if gen == 0:
            self.check_first_slot()
            base_detail = f"尚无已提交的快照。此次将建立第 1 代，没有父节点。\n新槽：{self.live.store / self.first_slot}"
        else:
            base_detail = f"基线：第 {gen} 代 · {node}\n原包 SHA-256：{digest}"
        return (f"工作树：{self.live.work}\n命令行 STATE：{self.live.state}\n原 STORE：{self.live.store}\n"
                f"分支：{branch}\n{base_detail}\n"
                f"本次操作号：{self.operation}\n下一代：{gen+1}\n\n"
                "提交整个工作目录，生成新的不可变归档。不改写原代，不自动解决同代冲突。"
                "登记本次原待决前可请求停止；可能保留私有候选。开始登记后不能用取消撤回，只查原号。"
                "捕获在原 STATE 中只写一份匿名整包，核验及同步后才用正式候选名登记；"
                "上传时原候选与 STORE 暂存仍同时存在。请确认足够空间，不保证容量。"
                "此路径须为普通用户并能从本机内核 procfs 核验匿名文件，不使用具名兜底。")

    def _open_base(self, *, held_mount=False):
        repo, branch, gen, node, digest = self.base
        check_id(repo); check_id(branch)
        if type(gen) is int and gen == 0:
            self._check_base_evidence()
            self.first_slot = f"{repo}.{branch}.{1:016x}.zxs"
            return
        check_id(node)
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
        if held_mount and mount_id(doc.fileno()) != mount_id(self.live.store_fd):
            raise Rejected("登记基线归档不在原 STORE 挂载")
        marker = read_managed_document(doc, display_name=self.name, observer=self.preparation)[0]
        if ((marker.repo, marker.branch, marker.generation, marker.operation) != self.base[:4]
                or archive_sha(doc, observer=self.preparation) != digest):
            raise Rejected("STORE 中原基线的节点或整包 SHA-256 不符")
        self.qualify()
        check_relative(self.live.store_fd, self.name, doc, observed)
        self.base_marker, self.observed, self.original = marker, observed, doc  # Only after both checks.

    def _check_base_evidence(self):
        if (self.base is not None and self.live.initial_base is not None and self.base == self.live.initial_base
                and self.purpose in ("commit", "retry")):
            return  # Only the actual creator of this original catalog carries this evidence.
        if self.original is None: raise Rejected("未持有本次已核验的原基线")
        check_relative(self.live.store_fd, self.name, self.original, self.observed)

    def _check_registered_mount(self):
        named = os.open(self.name, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.live.store_fd)
        try:
            m = os.fstat(named)
            if ((*native_identity(m, stat.S_IFREG), m.st_size, m.st_mtime_ns, m.st_ctime_ns) != self.observed
                    or not mount_id(named) == mount_id(self.original.fileno()) == mount_id(self.live.store_fd)):
                raise Rejected("登记基线的正式名称或实际挂载已变化")
        finally: os.close(named)

    def check_base(self):
        if self.closed or not self.ready: raise Rejected("未持有本次已核验的原基线")
        self.qualify()
        self._check_base_evidence()

    def check_first_slot(self):
        if self.purpose != "commit" or self.base != self.live.initial_base:
            raise Rejected("只能检查本次新登记的原第 1 代")
        self.store.check_store()
        try: os.stat(self.first_slot, dir_fd=self.live.store_fd, follow_symlinks=False)
        except FileNotFoundError:
            self.store.check_store()
            return  # Not a reservation. The final publisher still refuses overwrite.
        raise Rejected("所选 STORE 中的第 1 代名称已占用；不覆盖或更换标识")

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
        if self.purpose == "commit":
            if self.anonymous_candidate is None: raise Rejected("未持有本次交接的原候选")
            return self.anonymous_candidate.source(marker)
        return relative_document(self.live.state_fd, marker.operation+".zxs", single=True)

    def result(self):
        # Do not hand out a replay decision unless this particular original's read txn actually returned.
        self.pending_seen = self.conflict_pending_seen = self.local_receipt_seen = False
        outcome = self._result()
        if self.conflict and outcome[0] == "pending":
            self.conflict_pending_seen = self.purpose == "commit" and self.base[2] > 0
            if self.purpose == "commit" and self.base[2] == 0:
                action = "请保留原 STATE 和操作号；第 1 代冲突不在窗口另造另一仓库起点。"
            else:
                action = "可另行核对竞争者及原冻结归档。" if self.conflict_pending_seen else "请保留原操作号。"
            return "pending", ("此操作仍待决，原槽核对曾发现冲突；不是已核实的竞争节点。"
                               "不会覆盖或续传。" + action)
        self.pending_seen = outcome[0] == "pending"
        self.local_receipt_seen = outcome[0] == "receipt_dir"
        return outcome

    def completion_result(self, history):
        if not self.receipt.completion_started: return "completed", history
        if self.receipt.directory_remaining:
            return ("receipt_dir", "原完成行可读，SQLite 提交已返回，但本机目录同步未确认。"
                    "可单独确认仅完成这道本机屏障，不重传。")
        return "completion_unsynced", "原完成行可读，但这次 SQLite 提交未确认；不重新发布或当作成功。"

    def confirm_local_receipt(self, *, confirmed=False):
        if (not confirmed or not self.local_receipt_seen or not self.receipt.completion_started
                or not self.receipt.directory_remaining):
            raise Rejected("缺少本窗口已核到的原完成行和仅同步本机回执的明确确认")
        self.local_receipt_seen = False  # Any next attempt needs a new observation and consent.

        def check():
            with worktree.locked(self.live.state, read_only=True, live=self.live, local_only=True) as (con, row, _):
                self.live.check_instance(row, lifecycle.instance(con), check_name=False)
                if con.execute("PRAGMA main.journal_mode").fetchone()[0] != "delete":
                    raise Rejected("原库日志模式已改变，不按原方式同步")
                if self.purpose == "conflict-fork": self.require_link(con)
                saved = con.execute("SELECT marker,digest FROM completed WHERE operation=?",
                                    (self.operation,)).fetchone()
                if saved is None: raise Rejected("原完成行不再可核对，不同步或重发")
                self.check_marker(Commit.decode(saved["marker"]), saved["digest"])

        check()
        worktree.sync_live_state(self.live, self.operation, "仅同步本机原完成回执", local_only=True)
        check()
        self.receipt.completed_barrier()
        return "原完成回执已在本机确认；没有接触 STORE，不据此判断共享文件当前是否仍在。"

    def _result(self):
        # Metadata only. Never run finish(), read user bodies or publish while querying an outcome.
        with worktree.locked(self.live.state, read_only=True, live=self.live, local_only=True) as (con, row, _):
            self.live.check_instance(row, lifecycle.instance(con))
            done = con.execute("SELECT marker,digest FROM completed WHERE operation=?", (self.operation,)).fetchone()
            if done is not None:
                self.check_marker(Commit.decode(done["marker"]), done["digest"])
                return self.completion_result("原操作已记为完成；这是历史回执，未再次验证共享中当前是否仍有此文件。")
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
            if self.preparation is not None: self.preparation.check()
            self.check_base()
            return worktree.commit(self.live.state, selected=self)
        except CommitStopped as error:
            error.add_note(f"本次未登记的可能私有候选：{self.live.state / (self.operation + '.zxs')}")
            raise
        except Conflict:
            self.conflict = True
            raise
        finally:
            self.close()
