"""Move an existing *local* worktree only: no archive/member mapping or deletion."""
from __future__ import annotations
import os
from pathlib import Path
import stat
from .archive import read_archive, read_managed_document, stamp
from .document import archive_sha, check_relative, native_identity, relative_document
from .freeze import _distinct_roots, observe_held, observe_tree
from .model import Rejected, check_id, new_id
from .publish import digest_file, rename_new, sync_dir


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


def clean_at(path, state, tree_id):
    if observe_tree(path).digest() != tree_id:
        raise Rejected("worktree is dirty; its existing directory and contents were preserved")


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


def _idle_live_base(live):
    from .worktree import locked
    with locked(live.state, read_only=True, live=live) as (con, row, _):
        rec = instance(con)
        live.check_instance(row, rec)
        if (rec["phase"] != "visible" or rec["active"] is not None
                or row["pending"] is not None or row["pending_sha"] is not None):
            raise Rejected("原工作树必须可见且无待决操作；不提供本次对照")
        return row["repo"], row["branch"], row["generation"], row["parent"], row["base_sha"]


def observe_live_changes(live):
    """One observation against this still-held CLI checkout's last recorded base."""
    from .comparison import compare_manifests
    from .private_freeze import mount_id
    from .private_stage import same_mount

    expected = _idle_live_base(live)
    repo, branch, generation, node, digest = expected
    if type(generation) is not int or not 0 < generation < (1 << 63) or not digest:
        raise Rejected("没有可核验的已提交基线")
    check_id(repo); check_id(branch); check_id(node)
    name = f"{repo}.{branch}.{generation:016x}.zxs"
    working_mount = same_mount(live.state_fd, live.work_fd)
    store_mount = mount_id(live.store_fd)
    _distinct_roots((live.work_fd, live.state_fd, live.store_fd))
    work = native_identity(os.fstat(live.work_fd), stat.S_IFDIR)
    excluded = {native_identity(os.fstat(fd), stat.S_IFDIR) for fd in (live.state_fd, live.store_fd)}

    def work_member(_, child):
        m = os.fstat(child)
        if (m.st_dev, m.st_ino) in excluded or mount_id(child) != working_mount:
            raise Rejected("工作目录含子挂载或 STATE/STORE 别名；没有完整观察")

    # Both SQLite snapshots are short. Only original archive/work descriptors,
    # not a SQLite transaction, stay held across the potentially large reads.
    with relative_document(live.store_fd, name) as doc:
        observed = stamp(doc)
        if mount_id(doc.fileno()) != store_mount:
            raise Rejected("基线归档不在原 STORE 挂载")
        marker, base = read_managed_document(doc, display_name=name)
        if ((marker.repo, marker.branch, marker.generation, marker.operation) != expected[:4]
                or archive_sha(doc) != digest):
            raise Rejected("STORE 原基线的节点或整包 SHA-256 不符")
        check_relative(live.store_fd, name, doc, observed)
        current = observe_held(live.work_fd, work, require_ids=True, check_held=work_member)
        result = compare_manifests(base, current)
        if _idle_live_base(live) != expected:
            raise Rejected("观察期间原登记基线改变；请重新查看")
    return marker, result


def start(state, action):
    from .worktree import locked
    if action not in ("hide", "restore"): raise Rejected("unknown lifecycle action")
    with locked(state) as (con, row, state):
        rec = instance(con)
        phase = "visible" if action == "hide" else "hidden"
        if rec["phase"] != phase or rec["active"] is not None or row["pending"] is not None:
            raise Rejected("not an idle worktree in the required visible/hidden state")
        visible, hidden = locations(row)
        src, dst = (visible, hidden) if action == "hide" else (hidden, visible)
        if who(src) != identity(rec) or who(dst) is not None:
            raise Rejected("original worktree unavailable or destination occupied; nothing was moved")
        tree = base_tree(row).digest()
        clean_at(src, state, tree)
        operation = new_id()
        con.execute("INSERT INTO visibility_log VALUES(?,?,?,NULL)", (operation, action, tree))
        con.execute("UPDATE instance SET phase=?,active=? WHERE singleton=1", (action+"-pending", operation))
    # This is a separate transaction: a failure after the rename cannot lose the intent.
    return finish(state, operation)


def finish(state, operation):
    from .worktree import locked
    check_id(operation)
    with locked(state) as (con, row, state):
        item = con.execute("SELECT * FROM visibility_log WHERE operation=?", (operation,)).fetchone()
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
        if rec["active"] != operation or rec["phase"] != action+"-pending":
            raise Rejected("incomplete visibility evidence belongs to a different original operation")
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
            clean_at(current, state, item["original_tree"])
            if needs_move:
                if who(src) != identity(rec) or who(dst) is not None: raise Rejected("worktree names changed")
                rename_new(src, dst)
            sync_dir(dst.parent)
            if who(src) is not None or who(dst) != identity(rec):
                raise Rejected("move outcome unknown; preserve both names and the original operation")
        finally: os.close(fd)
        result = "hidden" if action == "hide" else "visible"
        con.execute("UPDATE instance SET phase=?,active=NULL WHERE singleton=1", (result,))
        con.execute("UPDATE visibility_log SET result=? WHERE operation=?", (result, operation))
        return {"operation":operation, "original_outcome":result, "recorded_phase":result, "current_original_named":True}
