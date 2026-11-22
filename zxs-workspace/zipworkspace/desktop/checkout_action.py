"""One explicitly selected Linux CLI-format checkout. No registry, shell or Qt dependency."""
from contextlib import ExitStack
import os
import sqlite3
import stat
import sys

from .. import opening, private_stage
from ..comparison import Folder
from ..capacity import describe, observed_available
from ..document import native_identity
from ..lifecycle import who
from ..model import Rejected, check_id, member_name, new_id
from ..private_candidates import generation, held_candidates
from ..publish import held_store, require_native_writer

from .selection import Verification


class CancelledCheckout(Rejected):
    """A preparation stopped before making the new STATE name."""


class _Preparation(Verification):
    PHASE = dict(Verification.PHASE,
                 capture="正在把所选原包写入私有准备目录",
                 sync="正在同步私有准备材料",
                 tree="正在读回并核验私有工作目录",
                 record="正在准备新 STATE 的原记录",
                 install="正在安装原 STATE 并完成这笔签出；现在不能撤销")

    def __init__(self):
        super().__init__()
        self._irreversible = False
        self._section = ""
        self.begin("identity")

    def check(self):
        if self.cancelled:
            raise CancelledCheckout("已取消本次签出。没有安装本次 STATE，也没有发布工作目录。"
                                    "私有准备材料可能保留，不能按续作回执使用。")

    def cancel(self):
        with self._lock:
            if self._irreversible: return False
            self._stop.set()
            return True

    def section(self, label):
        # Only the preparation worker changes phases or sections.
        self._section = label + " · " if label else ""

    def begin(self, phase, total=None):
        self.check()
        with self._lock:
            self._progress = (self._section + self.PHASE[phase], 0, total)

    def before_install(self):
        # Only the cancellation decision is locked, not any syscall.
        with self._lock:
            self.check()
            self._irreversible = True
            self._progress = (self.PHASE["install"], 0, None)

    def finish(self):
        with self._lock: self._irreversible = True


def available():
    return (sys.platform.startswith("linux") and os.name == "posix"
            and hasattr(os, "O_PATH") and callable(getattr(os, "pread", None))
            and callable(getattr(sqlite3.Connection, "serialize", None))
            and os.scandir in os.supports_fd)


def _absent(parent, leaf):
    try: os.stat(leaf, dir_fd=parent, follow_symlinks=False)
    except FileNotFoundError: return True
    return False

def _named_mount(path, original):
    # A bind of the same inode can pass samestat while naming a different mount.
    current = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW)
    try:
        if (not os.path.samestat(os.fstat(current), os.fstat(original)) or
                private_stage.mount_id(current) != private_stage.mount_id(original)):
            raise Rejected("所选目录名已不在原挂载或原目录上")
    finally: os.close(current)


class CheckoutChoice:
    """Handles live from the individual picks through confirmation, job and original result."""
    def __init__(self, snapshot, *, keep_session=False):
        if not available(): raise Rejected("此平台不能从桌面创建 Linux CLI 工作树")
        if snapshot.held is None or snapshot.held.closed:
            raise Rejected("本窗口未保留首次核验的原归档句柄；请重新明确选择快照，不按旧名重开")
        require_native_writer()
        self.stack = ExitStack()
        self.snapshot = snapshot
        self.operation = new_id()
        self.store = self.target = self.base = self.state = None
        self.keep_session = keep_session
        self.provisional = self.session = self.verified_sha = None
        self.initial_seed = None
        self.publication = None  # Only the actual private STATE installation boundary sets this.
        self.prepared = self.resume_offer = self.resume_mode = None
        self.registered = self.receipt_synced = self.initial_run = False
        self.source_fd = None
        self.used = self.closed = self.initial_opened = False
        self.preparation = _Preparation()
        try:
            # Transfer the exact input already verified for display. Do not
            # access a slow/rebound archive name during this GUI-thread handoff.
            self.source_fd = snapshot.held.duplicate()
        except BaseException:
            self.close()
            raise

    def _close_picks(self):
        if not self.closed:
            self.preparation.finish()
            self.closed = True
            self.initial_seed = None
            try: self._close_source()
            finally: self.stack.close()

    def _close_source(self):
        fd, self.source_fd = self.source_fd, None
        if fd is not None: os.close(fd)

    def cancel(self):
        return self.preparation.cancel()

    def close(self):
        # Explicit discard: any successfully created but not handed-off session is still ours.
        if self.session is not None:
            self.session.close()
            self.session = None
        self._close_picks()

    def check_source(self):
        s = self.snapshot
        if self.source_fd is None: raise Rejected("原归档读取能力已经关闭")
        # O_PATH compares the actual live mount and final name without opening
        # a second data stream (or following a leaf symlink).
        named = os.open(s.path, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            for fd in (self.source_fd, named):
                m = os.fstat(fd)
                if native_identity(m, stat.S_IFREG) != s.observed[:2] or generation(m) != s.observed:
                    raise Rejected("所选节点的原归档已变化；未重新选择分支头")
            if (private_stage.mount_id(named) != private_stage.mount_id(self.source_fd)
                    or s.exact is not None and (str(s.exact.path) != str(s.path) or s.exact.observed != s.observed
                                                or s.exact.marker != s.browser.commit)):
                raise Rejected("所选节点已不在原挂载或原归档中；不会重新打开正文")
        finally: os.close(named)
        return s.observed

    def _select(self, old):
        if self.closed or self.used or old is not None: raise Rejected("请重新开始一次明确的目录选择")

    def pick_store(self, path):
        self._select(self.store)
        self.store = self.stack.enter_context(held_store(path))

    def pick_target(self, path):
        self._select(self.target)
        self.target = Folder.choose(path)
        self.stack.callback(self.target.close)

    def pick_private(self, path):
        self._select(self.base)
        self.base = self.stack.enter_context(held_candidates(path))

    def _base_hint(self, marker):
        # Advisory only: neither read a ZIP nor change the named loose snapshot into a branch head.
        _named_mount(self.store.path, self.store.fd)
        try:
            meta = os.stat(marker.filename, dir_fd=self.store.fd, follow_symlinks=False)
        except FileNotFoundError:
            result = "未看到原分支基线槽。仍可签出；如要继续提交，所选 STORE 须有这个原节点的精确归档。"
        except OSError:
            result = "无法核对原分支基线槽的属性。仍可签出；不能据此判断是否能够继续提交。"
        else:
            if stat.S_ISREG(meta.st_mode):
                result = "看到同名普通文件，但未在这里核验它的内容、节点或整包摘要。"
            else:
                result = "该名称被非普通文件占用，不跟随链接。仍可签出，但不能当作可提交的原基线。"
        _named_mount(self.store.path, self.store.fd)
        return f"所选 STORE 的基线槽：{marker.filename}\n{result}"

    def _space_hint(self):
        # Both copies originate below the same admitted private parent. Publication
        # renames the tree on that mount; neither the STORE nor the output parent gets a second copy.
        raw, body = self.snapshot.observed[2], self.snapshot.browser.data_bytes
        needed = raw + body
        def size(n):
            return f"{n:,} B" if n < (1 << 30) else f"{n:,} B（约 {n / (1 << 30):.2f} GiB）"
        detail = (f"本次签出新增逻辑字节：{size(needed)}（STATE 原包副本 {size(raw)}，解包正文 {size(body)}）。"
                  "工作目录同挂载改名，不再复制；已有归档和候选不重复计入。")
        available = observed_available(self.base.parent)
        if available is None:
            observed = "当前无法估算该挂载普通用户的可用字节。"
        else:
            observed = f"该挂载当前报告普通用户可用 {size(available)}。"
            if available < needed: observed += "按当前报告可能不足，建议释放空间或另选合格位置。"
        return detail + "\n" + observed + "未计目录、inode 和额外元数据；未预留空间，不保证配额或写入成功。"

    def review(self, leaf):
        self._select(self.state)
        if self.store is None or self.target is None or self.base is None:
            raise Rejected("必须明确选择归档存储、工作树父及专用 CLI 状态父")
        if len(member_name(leaf)) != 1: raise Rejected("请输入一个新的目录名，不是路径")
        self.state = self.base.path / leaf
        self.destination = self.target.evidence.path / leaf
        self.check_new()
        m = self.snapshot.browser.commit
        exact = (f"原 ZIP SHA-256：{self.snapshot.exact.sha256}" if self.snapshot.exact is not None
                 else "原 ZIP 字节将在确认后从这同一个已选文件核对。")
        hint = self._base_hint(m)
        space = self._space_hint()
        self.check_new()  # Observations must still belong to the selected source, directories and new names.
        return (f"归档：{self.snapshot.path}\n仓库：{m.repo}\n分支：{m.branch} · 第 {m.generation} 代\n"
                f"原节点：{m.operation}\n整树 SHA-256：{m.tree}\n{exact}\n\n新工作目录：{self.destination}\n"
                f"归档存储：{self.store.path}\n{hint}\n新 CLI 状态目录（STATE）：{self.state}\n{space}\n本次操作号：{self.operation}\n\n"
                "只从首次核验时持有的原归档读取正文；本次仅写本机工作目录与私有 STATE，STORE 只读。"
                "远端 STORE 可以作为本次读取来源；这不授予远端提交或同步资格。后续窗口提交仍要求已准入的本机 STORE。"
                "两个新名称都不能已存在，不覆盖或自动换名。可以在安装新 STATE 前请求停止；"
                "停下可能保留私有材料，不公开工作目录。"
                "开始安装后只完成或查询原操作，不撤销；请保留 STATE 与操作号。")

    def check_roots(self):
        if self.closed: raise Rejected("原目录选择已经关闭")
        fd = self.target.check()
        private_stage.check_private(self.base)
        for path, held in ((self.base.path, self.base.parent), (self.target.evidence.path, fd), (self.store.path, self.store.fd)):
            _named_mount(path, held)
        mount = private_stage.same_mount(self.base.parent, fd)
        private_stage.separate_roots(self.base.parent, fd, self.store, mount)
        return mount

    def check_reopened(self, private, target, store):
        for held, opened in ((self.base.parent, private), (self.target.check(), target), (self.store.fd, store)):
            if (not os.path.samestat(os.fstat(held), os.fstat(opened)) or
                    private_stage.mount_id(held) != private_stage.mount_id(opened)):
                raise Rejected("原操作重新打开的目录不再是本次选择的实际挂载与目录")

    def check_new(self):
        self.check_source()
        self.check_roots()
        if not _absent(self.target.check(), self.destination.name) or not _absent(self.base.parent, self.state.name):
            raise Rejected("新工作目录或新 STATE 的名称已被占用；不会打开或替换它")

    def installing(self, area, db_generation):
        # Only called after verified preparation. Keep the SEED DIR, never a raw SQLite file
        # descriptor: closing a second DB fd can cancel native POSIX locks in this process.
        self.check_new()
        self.preparation.check()
        fd = os.dup(area.parent)
        self.stack.callback(os.close, fd)
        self.preparation.before_install()
        self.publication = (fd, area.identity, db_generation)

    def witness_prepared(self, area, stage, writer, observed, worktree):
        # The fresh DB writer and extracted root are both still held by their
        # producers. An O_PATH witness does not acquire/drop SQLite POSIX locks.
        if self.prepared is not None or stage is None:
            raise Rejected("缺少此次签出的原始准备材料")
        db = os.open("opening.sqlite", os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=area.parent)
        self.stack.callback(os.close, db)
        area.check_file("opening.sqlite", db, observed)
        if (not os.path.samestat(os.fstat(writer), os.fstat(db)) or
                private_stage.mount_id(db) != private_stage.mount_id(area.parent)):
            raise Rejected("准备签出记录时，原库实体或挂载已改变")
        work = os.dup(stage.parent)
        self.stack.callback(os.close, work)
        self.prepared = check_id(worktree), work, db, stage.identity

    def check_registered(self, state):
        if self.closed or self.publication is None or self.prepared is None or state != self.state:
            raise Rejected("未持有本次安装的 CLI STATE")
        private_stage.check_private(self.base)
        self.target.check()
        self.store.check_store()
        fd, identity, db_generation = self.publication
        _named_mount(state, fd)
        if (private_stage.check_named(self.base.parent, state.name, fd) != identity or
                native_identity(state.stat(follow_symlinks=False), stat.S_IFDIR) != identity):
            raise Rejected("本次安装的 STATE 已移走")
        db = self.prepared[2]
        named = os.open("opening.sqlite", os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=fd)
        try:
            for m in (os.fstat(db), os.fstat(named)):
                if (native_identity(m, stat.S_IFREG) != db_generation[:2] or m.st_nlink != 1 or
                        m.st_uid != os.geteuid() or m.st_mode & 0o077 or
                        not self.initial_opened and generation(m) != db_generation):
                    raise Rejected("本次签出的原登记数据库已变化")
            if not private_stage.mount_id(db) == private_stage.mount_id(named) == private_stage.mount_id(fd):
                raise Rejected("本次签出的原登记数据库已离开原挂载")
        finally: os.close(named)

    def check_prepared_row(self, row):
        worktree, _, _, tree = self.prepared
        state_id = self.publication[1]
        target = os.fstat(self.target.check())
        expected = (worktree, str(self.target.evidence.path), str(target.st_dev), str(target.st_ino),
                    private_stage.LEAF, *map(str, tree), *map(str, state_id), str(self.base.identity[1]))
        actual = tuple(row[k] for k in ("worktree", "parent", "device", "inode", "stage", "stage_dev", "stage_ino",
                                        "private_device", "private_inode", "private_base_inode"))
        if actual != expected or row["done"] not in (0, 1):
            raise Rejected("根目录或原签出记录与本窗口实际准备的内容不符")

    @property
    def can_review_recovery(self):
        return self.keep_session and self.used and self.registered and not self.closed

    def _recovery_place(self):
        # Classify the exact same directory, never a path with equivalent content.
        self.check_registered(self.state)
        mount = self.check_roots()
        _, work, _, identity = self.prepared
        state_fd = self.publication[0]
        target_fd = self.target.check()
        if private_stage.member_identity(work, mount)[:2] != identity:
            raise Rejected("无实际原工作目录，不接管同名目录")
        if not _absent(state_fd, "worktree.sqlite"):
            raise Rejected("STATE 已有登记库，本窗口不打开或接管它。请只核对原 CLI 回执。")
        expected = (*map(str, identity), stat.S_IFDIR)
        staged = who(private_stage.LEAF, dir_fd=state_fd)
        final = who(self.destination.name, dir_fd=target_fd)
        if (staged, final) == (expected, None):
            private_stage.check_mount_name(state_fd, private_stage.LEAF, identity, mount)
            place = "move"
        elif (staged, final) == (None, expected):
            private_stage.check_mount_name(target_fd, self.destination.name, identity, mount)
            place = "ack"
        else:
            raise Rejected("原暂存或正式名称已变化或被占用；未选择其他目录")
        self.check_registered(self.state)
        return place

    def review_recovery(self):
        """Read only the actual original operation; never prepare a fresh checkout."""
        if not self.can_review_recovery or self.resume_offer is not None or self.resume_mode is not None:
            raise Rejected("没有本窗口仍持有且已同步的原签出记录")
        with opening._record(self.state, write=False, selected=self) as (_, row, _):
            if row["done"]:
                return None, ("可读到这笔签出的历史完成行。没有写入或接管工作树；"
                              "这不证明目录现在仍在或已通过本窗口的耐久确认。"
                              f"\n原 STATE：{self.state}\n签出号：{self.operation}")
            place = self._recovery_place()
        offer = self.resume_offer = object(), place
        direction = ("仍在原私处。只把这棵原树以无覆盖方式移到原工作目录名，然后同步并写入原回执。"
                     if place == "move" else
                     "已经在原正式路径。只核验、同步并补全同一个原登记；不会再次移动或重做原树。")
        return offer, (f"签出号：{self.operation}\n原节点：{self.snapshot.browser.commit.operation}\n"
                       f"原 STATE：{self.state}\n原工作目录：{self.destination}\n原 STORE（只读）：{self.store.path}\n\n"
                       f"{direction}\n会从原工作目录实体读回核验，不重读归档、不选择新版本或覆盖任何目标。")

    def cancel_recovery(self, offer):
        if self.resume_offer is offer: self.resume_offer = None

    def check_recovery_row(self, row):
        if self.resume_mode is not None and (row["done"] or self._recovery_place() != self.resume_mode):
            raise Rejected("原签出状态已改变；不会转换成另一种恢复方式")

    def authorize_finish(self, state, operation):
        if (not self.registered or self.closed or state != self.state or operation != self.operation or
                not (self.initial_run or self.resume_mode is not None)):
            raise Rejected("缺少本次签出或明确继续原操作的授权")

    def verify_recovery_child(self, child, from_stage):
        if self.resume_mode is None: return
        root = self.prepared[1]
        if (from_stage != (self.resume_mode == "move") or
                not os.path.samestat(os.fstat(child), os.fstat(root)) or
                private_stage.mount_id(child) != private_stage.mount_id(root)):
            raise Rejected("原工作目录或确认的恢复方式不再相符")

    def retain_initial_seed(self, installed, copy):
        """Keep only a read-only handle to the producer's actual verified local copy."""
        if not self.keep_session: return
        original, observed = copy
        fd = None
        try:
            installed.check_file("original.zxs", original, observed)
            fd = os.open("original.zxs", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,
                         dir_fd=installed.parent)
            installed.check_file("original.zxs", fd, observed)
            installed.check_file("original.zxs", original, observed)
            if not private_stage.mount_id(fd) == private_stage.mount_id(original) == private_stage.mount_id(installed.parent):
                raise Rejected("私有原包不在本次实际 STATE 挂载")
            self.stack.callback(os.close, fd)
            self.initial_seed, fd = (fd, observed), None
        except (OSError, Rejected):
            pass  # Only the optional local text action loses its witness.
        finally:
            if fd is not None: os.close(fd)

    def resume(self, offer, *, confirmed=False):
        if (not confirmed or not self.can_review_recovery or self.resume_offer is not offer or offer is None or
                self.resume_mode is not None):
            raise Rejected("缺少本窗口对当前原回执的一次明确确认")
        self.resume_offer, self.resume_mode = None, offer[1]
        try:
            result = opening.finish(self.state, self.operation, selected=self)
            self.session, self.provisional = self.provisional, None
            self._close_picks()
            return result
        except Exception as error:
            raise Rejected(self._failure(error)) from error
        finally:
            self.resume_mode = None
            if self.provisional is not None:
                self.provisional.close()
                self.provisional = None

    def new_session(self, row, work_fd, state_fd, store_fd):
        if not self.keep_session: return None
        from .live_checkout import LiveCheckout
        snapshot = self.snapshot
        self.provisional = LiveCheckout(self.destination, self.state, self.store.path, row["worktree"],
                                        snapshot.browser.commit.repo, work_fd, state_fd, store_fd,
                                        source=(self.operation, snapshot.browser.commit, self.verified_sha,
                                                snapshot.path, snapshot.observed), initial_index=snapshot.browser,
                                        initial_seed=self.initial_seed)
        return self.provisional

    def take_session(self):
        session, self.session = self.session, None
        return session

    def _failure(self, error):
        if self.publication is None:
            outcome = "本次尚未安装原登记，没有可续作的回执。请保留可能留下的私有准备材料，不扫描、认领或清除。"
        else:
            outcome = "原登记或签出结果未知；不能据此续作或重新发起。请保留现场，先核实原 STATE。"
            try:
                self.check_registered(self.state)
                original = opening.status(self.state, selected=self)  # mode=ro; never opens an unproved STATE.
                self.check_registered(self.state)
                if (original["operation"] == self.operation and
                        original["source_node"] == self.snapshot.browser.commit.operation and
                        original["chosen_destination"] == str(self.destination)):
                    if original["recorded_stage"] == "attached":
                        outcome = ("读到这笔原操作的完成行。" if self.receipt_synced else
                                   "可读到原操作完成行，但本窗口未确认其耐久。")
                        outcome += "不据此判断原目录现在是否仍在；可用 checkout-status STATE 查询。"
                    elif self.registered:
                        outcome = ("已读到本次原登记，尚无完成回执。请先用 checkout-status STATE 核对；"
                                   "如需继续，再使用 resume-checkout STATE OPERATION，限下列原 STATE 和操作号。")
                    else:
                        outcome = ("能读到本次原登记，但未确认准备与父目录屏障全部完成。"
                                   "请保留现场，先用 checkout-status STATE 查原号；不能据此开始另一笔。")
            except (OSError, ValueError, sqlite3.Error):
                pass  # Unknown is not absence. Never select another database or issue a retry.
        return f"{describe(error)}\n{outcome}\nSTATE：{self.state}\n操作号：{self.operation}"

    def run(self):
        if self.closed or self.used or self.state is None: raise Rejected("本次签出决定不能重复提交")
        self.used = self.initial_run = True
        retained = False
        try:
            self.preparation.check()
            self.check_new()
            result = opening.begin(self.snapshot.path, self.target.evidence.path, self.destination.name,
                                   self.store.path, self.state, automatic=False, expected=self.snapshot.exact,
                                   private_stage=True, selected=self)
            self.session, self.provisional = self.provisional, None  # Original transaction has returned.
            return result
        except CancelledCheckout as error:
            raise Rejected(describe(error)) from error  # Preserve the exact private seed note, if there is one.
        except Exception as error:
            retained = self.can_review_recovery
            raise Rejected(self._failure(error)) from error
        finally:
            self.initial_run = False
            if self.provisional is not None:
                self.provisional.close()
                self.provisional = None
            self.preparation.finish()
            # A later review only needs the original directories and O_PATH
            # receipt evidence, never a duplicate of a possibly large SMB ZIP.
            try: self._close_source()
            finally:
                if not retained: self._close_picks()
