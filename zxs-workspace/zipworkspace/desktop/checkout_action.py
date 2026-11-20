"""One explicitly selected Linux CLI-format checkout. No registry, shell or Qt dependency."""
from contextlib import ExitStack
import os
import sqlite3
import stat
import sys

from .. import opening, private_stage
from ..comparison import Folder
from ..capacity import describe
from ..document import native_identity
from ..model import Rejected, member_name, new_id
from ..private_candidates import generation, held_candidates
from ..publish import held_store, require_native_writer


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
        self.publication = None  # Only the actual private STATE installation boundary sets this.
        self.used = self.closed = self.initial_opened = False
        try:
            # Transfer the exact input already verified for display. Do not
            # access a slow/rebound archive name during this GUI-thread handoff.
            self.source_fd = snapshot.held.duplicate()
            self.stack.callback(os.close, self.source_fd)
        except BaseException:
            self.close()
            raise

    def _close_picks(self):
        if not self.closed:
            self.closed = True
            self.stack.close()

    def close(self):
        # Explicit discard: any successfully created but not handed-off session is still ours.
        if self.session is not None:
            self.session.close()
            self.session = None
        self._close_picks()

    def check_source(self):
        s = self.snapshot
        if self.closed: raise Rejected("原归档读取能力已经关闭")
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
        available = None
        try:
            v = os.fstatvfs(self.base.parent)
            unit, blocks, free, usable = v.f_frsize, v.f_blocks, v.f_bfree, v.f_bavail
            if (all(type(x) is int for x in (unit, blocks, free, usable))
                    and unit > 0 and blocks > 0 and 0 <= usable <= free <= blocks):
                available = unit * usable
        except (OSError, AttributeError):
            pass  # Unavailable is neither zero nor permission to proceed on another volume.
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
                "两个新名称都不能已存在。请保留 STATE 与操作号；不覆盖、不自动换名或登记已有工作树。")

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
        fd = os.dup(area.parent)
        self.stack.callback(os.close, fd)
        self.publication = (fd, area.identity, db_generation)

    def check_registered(self, state):
        if self.closed or self.publication is None or state != self.state:
            raise Rejected("未持有本次安装的 CLI STATE")
        private_stage.check_private(self.base)
        self.target.check()
        self.store.check_store()
        fd, identity, db_generation = self.publication
        _named_mount(state, fd)
        if (private_stage.check_named(self.base.parent, state.name, fd) != identity or
                native_identity(state.stat(follow_symlinks=False), stat.S_IFDIR) != identity):
            raise Rejected("本次安装的 STATE 已移走")
        m = os.stat("opening.sqlite", dir_fd=fd, follow_symlinks=False)
        if (native_identity(m, stat.S_IFREG) != db_generation[:2] or m.st_nlink != 1 or
                not self.initial_opened and generation(m) != db_generation):
            raise Rejected("本次签出的原登记数据库已变化")

    def new_session(self, row, work_fd, state_fd, store_fd):
        if not self.keep_session: return None
        from .live_checkout import LiveCheckout
        snapshot = self.snapshot
        self.provisional = LiveCheckout(self.destination, self.state, self.store.path, row["worktree"],
                                        snapshot.browser.commit.repo, work_fd, state_fd, store_fd,
                                        source=(self.operation, snapshot.browser.commit, self.verified_sha,
                                                snapshot.path, snapshot.observed))
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
                        outcome = "原操作回执记为已完成；这不是对目录现在是否仍存在的保证。可用 checkout-status STATE 查询。"
                    else:
                        outcome = ("已读到本次原登记，尚无完成回执。请先用 checkout-status STATE 核对；"
                                   "如需继续，再使用 resume-checkout STATE OPERATION，限下列原 STATE 和操作号。")
            except (OSError, ValueError, sqlite3.Error):
                pass  # Unknown is not absence. Never select another database or issue a retry.
        return f"{describe(error)}\n{outcome}\nSTATE：{self.state}\n操作号：{self.operation}"

    def run(self):
        if self.closed or self.used or self.state is None: raise Rejected("本次签出决定不能重复提交")
        self.used = True
        try:
            self.check_new()
            result = opening.begin(self.snapshot.path, self.target.evidence.path, self.destination.name,
                                   self.store.path, self.state, automatic=False, expected=self.snapshot.exact,
                                   private_stage=True, selected=self)
            self.session, self.provisional = self.provisional, None  # Original transaction has returned.
            return result
        except Exception as error:
            raise Rejected(self._failure(error)) from error
        finally:
            if self.provisional is not None: self.provisional.close()
            self._close_picks()  # The successful session, if any, awaits take_session() or close().
