"""Move an existing *local* worktree only: no archive/member mapping or deletion."""
from __future__ import annotations
import os
from pathlib import Path
import stat
from .archive import read_archive
from .freeze import observe_tree
from .model import Rejected, check_id, new_id
from .publish import digest_file, rename_new, sync_dir


def install(con, root: Path):
    m = root.stat(follow_symlinks=False)
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


def who(path: Path):
    try:
        m = path.stat(follow_symlinks=False)
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
