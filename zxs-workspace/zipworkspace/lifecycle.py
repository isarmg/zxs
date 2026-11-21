"""Observe a live worktree's exact baseline or move its original local directory."""
from __future__ import annotations
from contextlib import ExitStack, closing, contextmanager
import os
from pathlib import Path
import stat
import sys
from .archive import read_archive, read_managed_document, stamp
from .capacity import writing
from .document import archive_sha, check_relative, native_identity, relative_document
from .freeze import _dir_flags, _distinct_roots, _sig, observe_held, observe_tree
from .model import Rejected, check_id, new_id
from .private_freeze import local_mount, mount_id
from .publish import digest_file, rename_new, rename_new_at, require_native_writer, sync_dir


def install(con, root: Path, *, original=None):
    m = root.stat(follow_symlinks=False) if original is None else original
    if not stat.S_ISDIR(m.st_mode): raise Rejected("cannot register a non-directory worktree")
    con.execute("""CREATE TABLE instance(singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                device TEXT NOT NULL, inode TEXT NOT NULL, phase TEXT NOT NULL, active TEXT)""")
    con.execute("INSERT INTO instance VALUES(1,?,?, 'visible',NULL)", (str(m.st_dev), str(m.st_ino)))
    con.execute("""CREATE TABLE visibility_log(operation TEXT PRIMARY KEY, action TEXT NOT NULL,
                original_tree TEXT NOT NULL, result TEXT)""")


def instance(con):
    row = con.execute("SELECT * FROM instance WHERE singleton=1").fetchone()
    if row is None: raise Rejected("missing original worktree identity")
    return row


def locations(row):
    visible = Path(row["location"])
    return visible, visible.with_name(".zip-workspace-hidden-" + check_id(row["worktree"]))


def who(path, *, dir_fd=None):
    try:
        m = os.stat(path, dir_fd=dir_fd, follow_symlinks=False)
    except FileNotFoundError: return None
    return str(m.st_dev), str(m.st_ino), stat.S_IFMT(m.st_mode)


def identity(rec): return rec["device"], rec["inode"], stat.S_IFDIR


def require_visible(con, row):
    rec = instance(con)
    if rec["phase"] != "visible" or rec["active"] is not None or who(locations(row)[0]) != identity(rec):
        raise Rejected("visibility operation pending, checkout hidden, or original directory identity changed")


def base_tree(row):
    if not row["generation"]: raise Rejected("worktree has no confirmed directory snapshot")
    p = Path(row["store"]) / f"{row['repo']}.{row['branch']}.{row['generation']:016x}.zxs"
    marker, tree = read_archive(p)
    if (marker.repo, marker.branch, marker.generation, marker.operation) != (
            row["repo"], row["branch"], row["generation"], row["parent"]) or digest_file(p) != row["base_sha"]:
        raise Rejected("cannot prove this working tree's original committed base")
    return tree


def clean_at(path, state, tree_id, *, restore=False):
    if observe_tree(path).digest() != tree_id:
        message = "worktree is dirty; its existing directory and contents were preserved"
        if restore:
            message += (". CLI restore requires the committed tree. Close editors and pause sync. "
                        "If the original window still holds this worktree, request restore there to keep changes. "
                        "Do not replace either directory name or edit STATE.")
        raise Rejected(message)


def changes(state):
    from .worktree import locked
    with locked(state) as (con, row, state):
        require_visible(con, row)
        base = base_tree(row).rows if row["generation"] else {}
        cur = observe_tree(locations(row)[0]).rows
        def report(keys):
            keys = sorted(keys)
            return {"count": len(keys), "first_paths": keys[:100]}
        return {"added":report(cur.keys() - base.keys()), "removed":report(base.keys() - cur.keys()),
                "changed":report(k for k in cur.keys() & base.keys() if cur[k] != base[k])}


def _base_values(row):
    return row["repo"], row["branch"], row["generation"], row["parent"], row["base_sha"]


def _base_leaf(expected):
    repo, branch, generation, node, digest = expected
    if type(generation) is not int or not 0 < generation < (1 << 63) or not digest:
        raise Rejected("没有可核验的已提交基线")
    check_id(repo); check_id(branch); check_id(node)
    return f"{repo}.{branch}.{generation:016x}.zxs"


def _verify_base(doc, name, expected, *, observer=None):
    options = {} if observer is None else {"observer": observer}
    marker, base = read_managed_document(doc, display_name=name, **options)
    if ((marker.repo, marker.branch, marker.generation, marker.operation) != expected[:4]
            or archive_sha(doc, **options) != expected[4]):
        raise Rejected("STORE 原基线的节点或整包 SHA-256 不符")
    return marker, base


@contextmanager
def _held_idle_live_base(live, *, local_only=False, rollback_only=False):
    from .worktree import locked
    with locked(live.state, read_only=True, live=live, local_only=local_only) as (con, row, _):
        rec = instance(con)
        live.check_instance(row, rec)
        if (rec["phase"] != "visible" or rec["active"] is not None
                or row["pending"] is not None or row["pending_sha"] is not None):
            raise Rejected("原工作树必须可见且无待决操作；不提供本次对照")
        if rollback_only and con.execute("PRAGMA main.journal_mode").fetchone()[0] != "delete":
            raise Rejected("不能核验原登记的 DELETE 日志模式；未补回文件")
        yield _base_values(row)


def _idle_live_base(live, *, local_only=False):
    with _held_idle_live_base(live, local_only=local_only) as before:
        return before


def _observe_local_work(live, *, with_store, observer=None):
    from .private_stage import same_mount
    working_mount = same_mount(live.state_fd, live.work_fd)
    outside = (live.state_fd, live.store_fd) if with_store else (live.state_fd,)
    _distinct_roots((live.work_fd, *outside))
    work = native_identity(os.fstat(live.work_fd), stat.S_IFDIR)
    excluded = {native_identity(os.fstat(fd), stat.S_IFDIR) for fd in outside}
    excluded.add(live.store_identity)  # Originally observed, not a lookup in the offline STORE.

    def work_member(_, child):
        m = os.fstat(child)
        if (m.st_dev, m.st_ino) in excluded or mount_id(child) != working_mount:
            raise Rejected("工作目录含子挂载或 STATE/STORE 别名；没有完整观察")

    options = {} if observer is None else {"observer": observer}
    return observe_held(live.work_fd, work, require_ids=True, check_held=work_member, **options)


def observe_initial_changes(live, *, observer=None):
    """Local observation only, against the exact initial checkout this window created."""
    from .comparison import compare_to_index
    if observer is not None: observer.begin("identity")
    index = live.initial_index
    if index is None: raise Rejected("本窗口没有从原签出交接的初始核验索引")
    marker = index.commit
    expected = (marker.repo, marker.branch, marker.generation, marker.operation, live.checkout_sha)
    if _idle_live_base(live, local_only=True) != expected:
        raise Rejected("原登记已不是首次签出的确切基线；请核验 STORE 中的当前版本")
    current = _observe_local_work(live, with_store=False, observer=observer)
    if observer is not None: observer.begin("compare")
    result = compare_to_index(index, current)
    if observer is not None: observer.begin("identity")
    if _idle_live_base(live, local_only=True) != expected:
        raise Rejected("离线观察期间原基线改变；不显示旧结果")
    if observer is not None: observer.check()
    return marker, result


class MissingBaseline:
    """Short-lived, exact STORE evidence for one online comparison dialog."""
    def __init__(self, fd, path, observed, basis, marker, missing, instance_id):
        self.path, self.observed, self.basis, self.marker = path, observed, basis, marker
        self.instance = instance_id
        self.missing, self._fd = missing, os.dup(fd)

    @property
    def held(self): return self if self._fd is not None else None

    def duplicate(self):
        if self._fd is None: raise Rejected("原基线已随对照关闭；请重新查看")
        return os.dup(self._fd)

    def file(self, row):
        actual = self.missing.get(row.path)
        if actual is None or actual[0] is not row:
            raise Rejected("只能补回本次基线对照中的一条缺失普通文件")
        return actual[1:]

    def close(self):
        fd, self._fd = self._fd, None
        self.missing.clear()
        if fd is not None: os.close(fd)


def observe_live_changes(live, *, observer=None, retain_missing=False):
    """One observation against this still-held CLI checkout's last recorded base."""
    from .comparison import compare_manifests
    if observer is not None: observer.begin("identity")
    expected = _idle_live_base(live)
    name = _base_leaf(expected)
    store_mount = mount_id(live.store_fd)
    # Both SQLite snapshots are short. Only original archive/work descriptors,
    # not a SQLite transaction, stay held across the potentially large reads.
    with ExitStack() as handoff:
        missing = None
        with relative_document(live.store_fd, name) as doc:
            observed = stamp(doc)
            if mount_id(doc.fileno()) != store_mount:
                raise Rejected("基线归档不在原 STORE 挂载")
            marker, base = _verify_base(doc, name, expected, observer=observer)
            check_relative(live.store_fd, name, doc, observed)
            current = _observe_local_work(live, with_store=True, observer=observer)
            if observer is not None: observer.begin("compare")
            result = compare_manifests(base, current)
            if retain_missing:
                files = {}
                for row in result.rows:
                    if observer is not None: observer.check()
                    if row.change == "missing" and row.before == "file":
                        files[row.path] = (row, *base.rows[row.path][1:])
                if files:
                    missing = MissingBaseline(doc.fileno(), live.store/name, observed, expected, marker, files, live.instance)
                    handoff.callback(missing.close)
            if observer is not None: observer.begin("identity")
            if _idle_live_base(live) != expected:
                raise Rejected("观察期间原登记基线改变；请重新查看")
        if observer is not None: observer.check()
        handoff.pop_all()
        return (marker, result, missing) if retain_missing else (marker, result)


def _phases(action):
    if action == "hide": return "visible", "hidden"
    if action == "restore": return "hidden", "visible"
    raise Rejected("unknown lifecycle action")


def _start_visibility(con, operation, action, tree):
    con.execute("INSERT INTO visibility_log VALUES(?,?,?,NULL)", (operation, action, tree))
    con.execute("UPDATE instance SET phase=?,active=? WHERE singleton=1", (action+"-pending", operation))


def _finish_visibility(con, operation, result):
    con.execute("UPDATE instance SET phase=?,active=NULL WHERE singleton=1", (result,))
    con.execute("UPDATE visibility_log SET result=? WHERE operation=?", (result, operation))


def _visibility_row(con, operation):
    return con.execute("SELECT * FROM visibility_log WHERE operation=?", (operation,)).fetchone()


class LiveVisibility:
    """One original-window decision, on the existing CLI journal, never a cold opener."""

    def __init__(self, live, action):
        if sys.platform != "linux": raise Rejected("只在 Linux 本窗口的原工作树收起或恢复目录")
        self.action = action
        self.before, self.after = _phases(action)
        self.operation = new_id()
        self.stack = ExitStack()
        self.live = self.stack.enter_context(closing(live.borrow()))
        self.archive_handles = self.stack.enter_context(ExitStack())
        self.ready = self.used = self.started = self.closed = False
        self.intent_synced = self.writing_receipt = self.receipt_synced = False
        self.resume_review = None

    def close(self):
        if not self.closed:
            self.closed = True
            self.resume_review = None
            self.stack.close()

    def _key(self, fd):
        m = os.fstat(fd)
        return m.st_dev, m.st_ino, stat.S_IFMT(m.st_mode), mount_id(fd, read_proc=self.proc.read)

    def _name(self, name, *, parent=None):
        try: fd = os.open(name, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
        except FileNotFoundError: return None
        try: return self._key(fd)
        finally: os.close(fd)

    def _roots(self):
        from .linux_authority import owned_directory
        with ExitStack() as checked:
            _, chain = owned_directory(checked, self.live.work.parent, self.uid, self.proc.read)
        if chain != self.chain:
            raise Rejected("工作目录的原父或上级路径已变化；不会改用同名目录")
        for path, fd in ((self.live.state, self.live.state_fd), (self.live.store, self.live.store_fd),
                         (self.live.state/"worktree.sqlite", self.live.db)):
            if self._name(path) != self._key(fd):
                raise Rejected("原 STATE、目录库或 STORE 不再位于原名称与挂载")
        self.live.check_catalog(self.live.state/"worktree.sqlite")

    def _pair(self):
        self._roots()
        result = tuple(self._name(path.name, parent=self.parent) for path in (self.source, self.target))
        self._roots()
        return result

    def _expect_source(self):
        if self._pair() != (self.work_key, None):
            raise Rejected("原目录不可确认，或目的名称已被占用；不会覆盖")

    def _hidden_help(self):
        return (f"本次核到原隐藏目录：{str(self.source)!r}。如需先保住改动，请关闭编辑器、暂停后台同步；"
                "仅从仍能确认的原件把所需文件另存到独立的新目录，不覆盖任何文件。"
                "不要手工改名这棵登记树或修改 STATE。")

    def _note_hidden(self, error):
        if self.closed or self.action != "restore" or not hasattr(self, "source"): return
        try: source, _ = self._pair()
        except Exception: return  # Diagnostic only; keep the original error if this recheck fails.
        if source == self.work_key:
            error.add_note(self._hidden_help())

    def _base_name(self):
        check_relative(self.live.store_fd, self.leaf, self.archive, self.observed)
        if self._name(self.leaf, parent=self.live.store_fd) != self._key(self.archive.fileno()):
            raise Rejected("STORE 原基线的名称或实际挂载已变化")

    def _read_instance(self, con, row):
        original = instance(con)
        self.live.check_instance(row, original, check_name=False)
        return original

    def _idle(self, row, original):
        if (original["phase"] != self.before or original["active"] is not None
                or row["pending"] is not None or row["pending_sha"] is not None):
            raise Rejected("不是指定可见阶段的无待决工作树；不会移动或继续其他操作")
        return _base_values(row)

    def prepare(self):
        from .linux_authority import ProcView, owned_directory
        from .linux_mount import _ordinary_user
        from .worktree import locked
        if self.closed or self.ready or self.used: raise Rejected("不能重用收起或恢复决定")
        try:
            require_native_writer()
            path = self.live.work.parent
            if path.anchor != "/" or ".." in path.parts or not 2 <= len(path.parts) <= 65:
                raise Rejected("需要可核对的原工作目录父路径")
            self.proc = ProcView(self.stack)
            self.uid = _ordinary_user(read_proc=self.proc.read)
            self.parent, self.chain = owned_directory(self.stack, path, self.uid, self.proc.read)
            self.mount = local_mount(self.parent, read_proc=self.proc.read)
            work = self.live.work_fd
            actual_parent = os.open("..", _dir_flags(), dir_fd=work)
            try:
                if self._key(actual_parent) != self._key(self.parent):
                    raise Rejected("指定父目录不是原工作树的真实父")
            finally: os.close(actual_parent)
            self.work_key = self._key(work)
            private, store = self.live.state_fd, self.live.store_fd
            dev = self.work_key[0]
            store_mount = local_mount(store, read_proc=self.proc.read)
            if (self.work_key[3] != self.mount or self._key(private)[3] != self.mount
                    or self._key(self.live.db)[3] != self.mount
                    or os.fstat(store).st_dev == dev and store_mount != self.mount):
                raise Rejected("需要原 WORK 与 STATE 在同一已准入挂载，拒绝 STORE 挂载别名")
            _distinct_roots((work, private, store))
            self._roots()
            with locked(self.live.state, read_only=True, live=self.live) as (con, row, _):
                self.expected = self._idle(row, self._read_instance(con, row))
                visible, hidden = locations(row)
            self.source, self.target = (visible, hidden) if self.action == "hide" else (hidden, visible)
            self._expect_source()
            self.leaf = _base_leaf(self.expected)
            with relative_document(store, self.leaf) as doc:
                self.observed = stamp(doc)
                if self._key(doc.fileno())[3] != store_mount:
                    raise Rejected("基线归档不在原 STORE 挂载")
                self.marker, base = _verify_base(doc, self.leaf, self.expected)
                # The short context still verifies the original name on exit. Long
                # ownership only closes our FD; it cannot invalidate a receipt later.
                fd = os.dup(doc.fileno())
                self.archive_handles.callback(os.close, fd)
                self.archive = self.archive_handles.enter_context(os.fdopen(fd, "rb", closefd=False))
            with locked(self.live.state, read_only=True, live=self.live) as (con, row, _):
                if self._idle(row, self._read_instance(con, row)) != self.expected:
                    raise Rejected("原基线在准备时已改变；请重新选择")
            self._expect_source()
            self._base_name()
            self.ready = True
            if self.action == "hide":
                check = (f"确认后完整读取工作目录并核对基线（约 {base.total:,} B 文件正文）；"
                         "只在内容相符时收起。改名不会撤销已打开的写入句柄，也不是保密。")
            else:
                check = ("确认后完整读取并检查原隐藏目录，可能耗时。恢复的是同一目录；"
                         "已经写到这棵树的改动会一起带回，仍未提交，不会用基线覆盖文件。")
            return (f"从：{str(self.source)!r}\n到：{str(self.target)!r}\n"
                    f"原 STATE：{str(self.live.state)!r}\n原 STORE：{str(self.live.store)!r}\n"
                    f"基线：第 {self.marker.generation} 代 · {self.marker.operation}\n操作号：{self.operation}\n\n"
                    + check + "不会删除、覆盖、提交、释放数据空间或改变访问权限。"
                    "同目录改名后旧路径不再可用。确认后请等待，不自动重试。")
        except BaseException as error:
            if isinstance(error, Exception): self._note_hidden(error)
            self.close()
            raise

    def _observe(self, pair):
        fd = self.live.work_fd
        before = _sig(os.fstat(fd))
        _distinct_roots((fd, self.live.state_fd, self.live.store_fd))
        excluded = {self._key(self.live.state_fd)[:2], self._key(self.live.store_fd)[:2]}

        def member(_, child):
            obs = self._key(child)
            if obs[:2] in excluded or obs[3] != self.mount:
                raise Rejected("工作树含 STATE/STORE 别名或子挂载；不能移动")

        tree = observe_held(fd, self.work_key[:2], require_ids=True, check_held=member).digest()
        if self._pair() != pair:
            raise Rejected("原目录所在名称或空位已变化；不移动或接认")
        if _sig(os.fstat(fd)) != before: raise Rejected("原工作目录在核对时已变化")
        return before, tree

    def _log(self, con):
        item = _visibility_row(con, self.operation)
        if item is not None and (item["action"] != self.action or item["original_tree"] != self.original_tree
                                 or item["result"] not in (None, self.after)):
            raise Rejected("日志与本次确认的原操作不符；不接管其他结果")
        return item

    def _pending(self, row, original):
        if (original["phase"] != self.action+"-pending" or original["active"] != self.operation
                or _base_values(row) != self.expected or row["pending"] is not None or row["pending_sha"] is not None):
            raise Rejected("当前不是这笔原待决操作及基线；不会移动或补记")

    @staticmethod
    def _journal(con):
        mode = con.execute("PRAGMA main.journal_mode").fetchone()
        if mode is None or mode[0] != "delete":
            raise Rejected("原 CLI STATE 不再使用所需的 DELETE 回滚日志模式；不改模式或移动")

    def _sync_state(self, phase):
        self._roots()
        # With SQLite's default DELETE journal, FULL need not sync the
        # directory after removing the journal. Use the held STATE itself.
        with writing("STATE", self.live.state, phase, self.operation, self.live.state_fd):
            os.fsync(self.live.state_fd)
        self._roots()

    def run(self, *, confirmed=False):
        if confirmed is not True or self.closed or not self.ready or self.used:
            raise Rejected("只允许明确确认一次的原目录改名")
        self.used = True
        try: return self._run()
        except Exception as error:
            self._note_hidden(error)
            raise
        finally: self.archive_handles.close()  # Receipt lookup must not pin a large deleted STORE archive.

    def _run(self):
        from .linux_mount import _ordinary_user
        from .worktree import locked
        if _ordinary_user(read_proc=self.proc.read) != self.uid:
            raise Rejected("普通用户已变化，不移动工作目录")
        self._expect_source()
        self._base_name()
        with locked(self.live.state, live=self.live, operation=self.operation,
                    phase="记录原目录改名意图；在提交本次意图前不能移动") as (con, row, _):
            self._journal(con)
            if self._idle(row, self._read_instance(con, row)) != self.expected:
                raise Rejected("确认的原基线已改变；不给另一代改名")
            if _visibility_row(con, self.operation) is not None:
                raise Rejected("操作号已在原日志中使用，未登记或移动")
            before, tree = self._observe((self.work_key, None))  # Full read and two inventories after consent.
            self._base_name()
            if self.action == "hide" and tree != self.marker.tree:
                raise Rejected("工作目录有未提交改动；保留原路径与文件")
            self.original_tree = tree
            self.started = True  # From here an uncertain answer may have a durable intent.
            # A restore may move late edits, but exact-operation resume must
            # check the tree this decision actually accepted, not the ZIP base.
            _start_visibility(con, self.operation, self.action, self.original_tree)

        self._sync_state("同步原改名意图；失败不移动目录")
        self.intent_synced = True
        return self._complete("move", before=before)

    def _resume_allowed(self):
        if self.closed or not self.started or not self.intent_synced or self.receipt_synced:
            raise Rejected("没有本窗口已确认同步、可单独继续的原改名意图")

    def review_resume(self):
        """Read only: select either the exact remaining move or just its acknowledgement."""
        from .worktree import locked
        self.resume_review = None
        try:
            self._resume_allowed()
            with locked(self.live.state, read_only=True, live=self.live) as (con, row, _):
                self._journal(con)
                rec, item = self._read_instance(con, row), self._log(con)
                if item is None or item["result"] is not None:
                    raise Rejected("原号不是待决；请只读查本次回执")
                self._pending(row, rec)
                pair = self._pair()
            if pair == (self.work_key, None):
                mode = "move"
                effect = "仅尝试继续这笔无覆盖改名，不创建另一号。"
            elif pair == (None, self.work_key):
                mode = "ack"
                effect = "原目录已在目标名；仅核对并同步原回执，不再次改名。"
            else:
                raise Rejected("无法核实原目录和空位；不覆盖、不认领、不续作")
            decision = (mode,)  # This exact, single-use review must survive the confirmation.
            self.resume_review = decision
            return decision, (f"{effect}\n\n原操作：{self.operation}\n原 STATE：{str(self.live.state)!r}\n"
                              f"原 STORE：{str(self.live.store)!r}\n旧名：{str(self.source)!r}\n目标：{str(self.target)!r}\n\n"
                              "确认后会完整复核原操作已接受的文件内容，可能耗时。后续修改或名称变化即停止；"
                              "不会改动文件、覆盖、提交或自动重试。")
        except Exception as error:
            self._note_hidden(error)
            raise

    def cancel_resume(self, decision):
        if self.resume_review is decision: self.resume_review = None

    def resume(self, decision, *, confirmed=False):
        from .linux_mount import _ordinary_user
        self._resume_allowed()
        if confirmed is not True or decision is None or decision is not self.resume_review:
            raise Rejected("需要同一原号最新的一次继续确认")
        self.resume_review = None
        try:
            if _ordinary_user(read_proc=self.proc.read) != self.uid:
                raise Rejected("普通用户已变化，不继续原改名")
            return self._complete(decision[0])
        except Exception as error:
            self._note_hidden(error)
            raise

    def _complete(self, mode, *, before=None):
        from .worktree import locked
        pair = (self.work_key, None) if mode == "move" else (None, self.work_key)
        with locked(self.live.state, live=self.live, operation=self.operation,
                    phase="完成原目录改名回执；正式名称可能已变，失败须查原号") as (con, row, _):
            self._journal(con)
            rec = self._read_instance(con, row)
            item = self._log(con)
            if item is None: raise Rejected("未核到这笔原目录改名意图")
            if item["result"] is not None:
                if self.writing_receipt and not self.receipt_synced:
                    raise Rejected("原完成行可见，但未证原 STATE 完成屏障；不把它重当成功")
                return "historical"  # Never move it a second time.
            self._pending(row, rec)
            if before is not None:
                self._base_name()  # Fresh decision still holds the original verified STORE archive.
            if self._pair() != pair:
                raise Rejected("原目录与两名不再匹配这次明确确认；不切换到另一个动作")
            if before is None:
                before, tree = self._observe(pair)
                if tree != self.original_tree:
                    raise Rejected("当前内容已不等于原号接受的内容；保留原目录，不继续")
            fd = self.live.work_fd
            if _sig(os.fstat(fd)) != before:
                raise Rejected("原目录的代次在登记后改变；不尝试移动")
            if mode == "move":
                # Same parent: source-name check is not a kernel inode CAS. The
                # confirmation requires a controlled parent and stopped writers.
                with writing("工作目录父", self.source.parent, "原生不覆盖改名；碰撞原样保留", self.operation, self.parent):
                    try: rename_new_at(self.parent, self.source.name, self.target.name, expected_source=os.fstat(fd))
                    except FileExistsError as err:
                        raise Rejected("目的名称刚被占用；仍保留原操作，不覆盖双方") from err
            moved = _sig(os.fstat(fd))
            stable = ((moved[:3], moved[4]) == (before[:3], before[4])) if mode == "move" else moved == before
            if not stable or self._pair() != (None, self.work_key):
                raise Rejected("原目录可能已换名，但实际名称或挂载未确认；保留原操作")
            with writing("工作目录父", self.source.parent, "同步原改名；晚失败不能视为没移动", self.operation, self.parent):
                os.fsync(self.parent)
            if self._pair() != (None, self.work_key) or _sig(os.fstat(fd)) != moved:
                raise Rejected("目录同步后不能核实原名称与实体；保留原操作")
            self.writing_receipt = True
            _finish_visibility(con, self.operation, self.after)
        self._sync_state("同步原完成回执；失败不把可见完成行当作耐久")
        self.receipt_synced = True
        return self.after

    def result(self):
        from .worktree import locked
        if self.closed or not self.started: raise Rejected("没有这笔已尝试登记的窗口原操作")
        with locked(self.live.state, read_only=True, live=self.live) as (con, row, _):
            rec = self._read_instance(con, row)
            item = self._log(con)
            if item is None:
                if self._idle(row, rec) != self.expected:
                    raise Rejected("找不到这笔原回执，且原基线已变；不猜测")
                self._expect_source()
                return "unrecorded", "未查到本次受理记录，原目录仍在确认的旧名称。未重试。"
            if item["result"] is not None:
                if self.writing_receipt and not self.receipt_synced:
                    return "recorded", "读到原完成记录，但这次没有确认原 STATE 完成屏障；保留现场，不继续移动。"
                word = "收起" if self.after == "hidden" else "恢复"
                return "completed", f"原回执已记录{word}完成。它是历史结果，不代表此刻仍在该名称。"
            self._pending(row, rec)
            pair = self._pair()
        hidden_help = "\n" + self._hidden_help() if self.action == "restore" and pair[0] == self.work_key else ""
        if pair == (self.work_key, None):
            detail = "核到原目录仍在旧名，目标未见占用。"
        elif pair == (None, self.work_key):
            detail = "核到原目录已在目标名，但没有完成回执，不能据此确认耐久。"
        else:
            return "pending", "原记录待决，但两个名称无法构成准确原件与空位；保留现场，不继续。" + hidden_help
        detail += hidden_help + "\n"
        if not self.intent_synced:
            return "pending", detail + "只读见原记录待决，但没有确认原意图已同步；保留现场，不续作。"
        return "pending", (detail + "只读查询时原记录仍待决，没有继续。可在本窗口另行核对并明确确认原号续作。"
                           "\n只有原件、两名及本次核验的内容未变、关闭编辑器并停止同步后，才用 CLI 原号续作（会写入）：\n"
                           f"resume-visibility STATE {self.operation} --writers-stopped\n请使用下列原 STATE。")


def start(state, action):
    from .worktree import locked
    phase, _ = _phases(action)
    with locked(state) as (con, row, state):
        rec = instance(con)
        if (rec["phase"] != phase or rec["active"] is not None
                or row["pending"] is not None or row["pending_sha"] is not None):
            raise Rejected("not an idle worktree in the required visible/hidden state")
        visible, hidden = locations(row)
        src, dst = (visible, hidden) if action == "hide" else (hidden, visible)
        if who(src) != identity(rec) or who(dst) is not None:
            raise Rejected("original worktree unavailable or destination occupied; nothing was moved")
        tree = base_tree(row).digest()
        clean_at(src, state, tree, restore=action == "restore")
        operation = new_id()
        _start_visibility(con, operation, action, tree)
    # This is a separate transaction: a failure after the rename cannot lose the intent.
    return finish(state, operation)


def finish(state, operation):
    from .worktree import locked
    check_id(operation)
    with locked(state) as (con, row, state):
        item = _visibility_row(con, operation)
        rec = instance(con)
        if item is None: raise Rejected("no such original local visibility operation")
        if item["result"] is not None:
            # A historical result never reopens a later transition or claims its
            # retained file still exists. Report current evidence separately.
            visible, hidden = locations(row)
            loc = visible if rec["phase"] == "visible" else hidden if rec["phase"] == "hidden" else None
            return {"operation":operation, "original_outcome":item["result"], "recorded_phase":rec["phase"],
                    "current_original_named":loc is not None and who(loc) == identity(rec)}
        action = item["action"]
        if (rec["active"] != operation or rec["phase"] != action+"-pending"
                or row["pending"] is not None or row["pending_sha"] is not None):
            raise Rejected("incomplete visibility evidence does not match this original operation or conflicts with a pending commit")
        visible, hidden = locations(row)
        src, dst = (visible, hidden) if action == "hide" else (hidden, visible)
        pair = who(src), who(dst)
        if pair == (identity(rec), None): current, needs_move = src, True
        elif pair == (None, identity(rec)): current, needs_move = dst, False
        else: raise Rejected("cannot prove either complete original directory/name pair; nothing was overwritten")
        fd = os.open(current, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            held = os.fstat(fd)
            if (str(held.st_dev),str(held.st_ino),stat.S_IFMT(held.st_mode)) != identity(rec):
                raise Rejected("held checkout is not the registered directory")
            if observe_held(fd, (held.st_dev, held.st_ino)).digest() != item["original_tree"]:
                raise Rejected("worktree no longer matches the tree accepted by this exact visibility operation; "
                               "its directory and contents were preserved. Close editors and pause sync; do not overwrite either name.")
            if needs_move:
                if who(src) != identity(rec) or who(dst) is not None: raise Rejected("worktree names changed")
                rename_new(src, dst)
            sync_dir(dst.parent)
            if who(src) is not None or who(dst) != identity(rec):
                raise Rejected("move outcome unknown; preserve both names and the original operation")
        finally: os.close(fd)
        result = "hidden" if action == "hide" else "visible"
        _finish_visibility(con, operation, result)
        return {"operation":operation, "original_outcome":result, "recorded_phase":result, "current_original_named":True}
