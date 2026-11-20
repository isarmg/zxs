"""One client-owned SQLite catalog per checkout, outside both its tree and share."""
from __future__ import annotations
from contextlib import contextmanager
import json
import os
from pathlib import Path
import sqlite3
import tempfile
from . import lifecycle
from .archive import read_archive
from .capacity import note
from .freeze import frozen_tree, pack
from .model import Commit, Rejected, check_id, new_id
from .private_candidates import held_candidates
from .private_freeze import anonymous_capture
from .publish import _publish, confirm_final, digest_file, publish, rename_new, rename_new_at, require_native_writer, sync_dir


def paths(work, store, state):
    work, store, state = (Path(p).absolute() for p in (work, store, state))
    normalized = [p.resolve() for p in (work, store, state)]
    if any(a.is_relative_to(b) or b.is_relative_to(a)
           for i, a in enumerate(normalized) for b in normalized[i + 1:]):
        raise Rejected("worktree, archive store and private state must be independent directories")
    if not store.is_dir() or store.is_symlink(): raise Rejected("select an existing archive-store directory")
    return work, store, state


def enroll(work, store, state, base=None):
    require_native_writer()
    work, store, state = paths(work, store, state)
    if not work.is_dir() or work.is_symlink(): raise Rejected("select a regular local working directory")
    state.mkdir(mode=0o700, exist_ok=False)
    create_catalog(work, store, state, base, new_id())
    sync_dir(state.parent)


def create_catalog(work, store, state, base, instance_id, *, checked_root=None, live=None):
    if live is not None and checked_root is None: raise Rejected("live catalog requires the actual checked checkout root")
    if live is None:
        fd, tmp = tempfile.mkstemp(prefix=".catalog-", dir=state)
        temp = Path(tmp)
    else:
        live.check_dirs()
        temp = state / (".catalog-" + new_id())
        fd = os.open(temp.name, os.O_CREAT | os.O_EXCL | os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC,
                     0o600, dir_fd=live.state_fd)
    try:
        if live is not None: live.new_catalog(temp, fd)  # Simultaneous with the exclusive creator's fd.
    finally: os.close(fd)
    if live is not None: live.check_catalog(temp)
    con = sqlite3.connect(temp) if live is None else sqlite3.connect(temp.as_uri()+"?mode=rw", uri=True)
    try:
        con.execute("PRAGMA synchronous=FULL")
        con.execute("""CREATE TABLE checkout(singleton INTEGER PRIMARY KEY CHECK(singleton=1),
          worktree TEXT NOT NULL, location TEXT NOT NULL, store TEXT NOT NULL, repo TEXT NOT NULL,
          branch TEXT NOT NULL, generation INTEGER NOT NULL, parent TEXT, base_sha TEXT,
          pending BLOB, pending_sha TEXT)""")
        con.execute("CREATE TABLE completed(operation TEXT PRIMARY KEY, marker BLOB NOT NULL, digest TEXT NOT NULL)")
        lifecycle.install(con, work, original=checked_root() if checked_root is not None else None)
        con.execute("CREATE TABLE forked_original(operation TEXT PRIMARY KEY, successor TEXT NOT NULL)")
        root, source_sha = base if base else (None, None)
        con.execute("INSERT INTO checkout VALUES(1,?,?,?,?,?,?,?,?,NULL,NULL)", (
            instance_id, str(work), str(store), root.repo if root else new_id(),
            root.branch if root else new_id(), root.generation if root else 0,
            root.operation if root else None, source_sha))
        con.commit()
    finally: con.close()
    name = state / "worktree.sqlite"
    if live is None:
        with temp.open("rb") as f: os.fsync(f.fileno())
        if checked_root is not None: checked_root()
        rename_new(temp, name)
        sync_dir(state)
    else:
        # The ordinary fd is only for the closed, still-private DB. Close it BEFORE
        # naming worktree.sqlite, so it cannot cancel another live SQLite connection's locks.
        live.sync_new_catalog(temp)
        checked_root()
        live.check_catalog(temp)
        rename_new_at(live.state_fd, temp.name, name.name)
        live.check_catalog(name)
        os.fsync(live.state_fd)
        live.check_catalog(name)


@contextmanager
def locked(state, *, read_only=False, live=None, operation=None, phase="更新原工作树登记"):
    """Open the existing catalog with either a read snapshot or a reserved writer."""
    state = Path(state).absolute()
    db = state / "worktree.sqlite"
    if live is not None: live.check_catalog(db)
    if not state.is_dir() or state.is_symlink() or (state.stat().st_mode & 0o077):
        raise Rejected("use the existing private worktree state directory")
    mode = "ro" if read_only else "rw"  # Neither may create a missing catalog.
    def database_error(exc):
        if not read_only:
            note(exc,"STATE",db,phase,operation,None if live is None else live.state_fd,database=True)
    try: con = sqlite3.connect(db.as_uri() + "?mode=" + mode, uri=True, isolation_level=None)
    except sqlite3.Error as error:
        database_error(error); raise
    con.row_factory = sqlite3.Row
    try:
        if live is not None: live.check_catalog(db)
        if read_only:
            con.execute("BEGIN")  # Both checkout and instance reads see the same committed state.
        else:
            con.execute("PRAGMA synchronous=FULL")
            con.execute("BEGIN IMMEDIATE")
        row = con.execute("SELECT * FROM checkout WHERE singleton=1").fetchone()
        if row is None: raise Rejected("worktree record missing")
        yield con, row, state
        if live is not None: live.check_catalog(db)
        con.commit()
    except BaseException as error:
        database_error(error)
        con.rollback()
        raise
    finally: con.close()


def import_zip(archive, dest):
    from .plain_import import require_writer
    require_writer()  # No destination is admitted. A preview is never a write request.


def checkout(archive, dest, store, state, *, expected=None, private_stage=False):
    from . import opening
    dest, store, state = paths(dest, store, state)
    _, marker = opening.begin(archive, dest.parent, dest.name, store, state, automatic=False,
                              expected=expected, private_stage=private_stage)
    return marker


def finish(state, operation, *, selected=None):
    operation = check_id(operation)
    live = None if selected is None else selected.live
    if selected is not None and (operation != selected.operation or selected.closed
                                 or not selected.used or not selected.ready):
        raise Rejected("not the reviewed original operation")
    with locked(state, live=live, operation=operation,
                phase="记原提交回执；正式归档可能已出现，不能按返回失败判断是否发布") as (con, row, state):
        if live is not None: live.check_instance(row, lifecycle.instance(con))
        if selected is not None and selected.purpose == "conflict-fork":
            selected.require_link(con)
        done = con.execute("SELECT marker,digest FROM completed WHERE operation=?", (operation,)).fetchone()
        if done is not None:
            marker = Commit.decode(done["marker"])
            target = Path(row["store"]) / marker.filename
            if marker.operation != operation: raise Rejected("completed record does not name the original operation")
            if selected is not None:
                selected.check_marker(marker, done["digest"])
                if selected.purpose in ("retry", "conflict-fork"):
                    raise Rejected("原操作已登记完成；本次没有上传，请只读查询其历史回执")
            matched = (confirm_final(target, marker, done["digest"]) if selected is None else
                       selected.store.confirm(marker, done["digest"]))
            if not matched:
                raise Rejected("completed original archive is unavailable; no replacement was published")
            return target, marker
        superseded = con.execute("SELECT successor FROM forked_original WHERE operation=?", (operation,)).fetchone()
        if superseded is not None: raise Rejected("original conflict remains preserved; separate explicit fork: " + superseded[0])
        if row["pending"] is None: raise Rejected("no original operation recorded here; not reconstructing it")
        marker = Commit.decode(row["pending"])
        if marker.operation != operation: raise Rejected("another operation is pending; no worktree was recaptured")
        local = state / (marker.operation + ".zxs")
        if selected is None:
            target = publish(local, Path(row["store"]), marker, row["pending_sha"])
        else:
            selected.check_row(con, row)
            selected.check_marker(marker, row["pending_sha"])
            target = _publish(local, selected.store, marker, row["pending_sha"],
                              source=selected.candidate_source(marker), confirm_only_fd=selected.ack_fd)
            selected.check_row(con, row)
        con.execute("INSERT INTO completed(operation,marker,digest) VALUES(?,?,?)", (operation, marker.encode(), row["pending_sha"]))
        con.execute("""UPDATE checkout SET branch=?,generation=?,parent=?,base_sha=?,pending=NULL,pending_sha=NULL
                       WHERE singleton=1""", (marker.branch, marker.generation, marker.operation, row["pending_sha"]))
        # The immutable private spool remains recoverable; cleanup is not part of this prototype.
    return target, marker


def commit(state, *, fork=False, selected=None):
    require_native_writer()
    live = None if selected is None else selected.live
    if selected is not None:
        if selected.purpose != "commit" or not selected.used or fork or Path(state).absolute() != live.state:
            raise Rejected("not the reviewed ordinary fresh commit")
        selected.check_base()
    # Unsupported STATE volumes refuse before SQLite can open/recover the catalog.
    proposal = selected.operation if selected is not None else None
    with anonymous_capture(state, held=None if selected is None else selected.area, operation=proposal) as capture, \
         locked(state, live=live, operation=lambda: capture.operation or proposal,
                phase="登记原 pending；冻结候选可能已存在，不能据此认定已受理") as (con, row, state):
        capture.check_state()
        if row["pending"] is not None:
            raise Rejected("an original operation is pending; resume it rather than recapturing")
        lifecycle.require_visible(con, row)
        work, store, state = paths(Path(row["location"]), Path(row["store"]), state)
        gen, branch, repo = row["generation"], row["branch"], row["repo"]
        if selected is not None:
            selected.check_row(con, row)  # Baseline CAS under the actual SQLite writer; never reselect HEAD.
        elif gen:
            base_path = store / f"{repo}.{branch}.{gen:016x}.zxs"
            saved, _ = read_archive(base_path)
            if (saved.repo, saved.branch, saved.generation, saved.operation) != (repo, branch, gen, row["parent"]) or digest_file(base_path) != row["base_sha"]:
                raise Rejected("exact original branch base is not available in the selected store")
        operation = new_id() if selected is None else selected.operation
        capture.operation = operation
        registered = lifecycle.instance(con)
        expected = (int(registered["device"]),int(registered["inode"]))
        with frozen_tree(work, capture, expected, store, selected=live) as (frozen, manifest):
            marker = Commit(repo, new_id() if fork else branch, 1 if fork else gen + 1,
                            operation, row["parent"], manifest.digest())
            with capture.candidate(operation) as (name, original, stream):
                pack(frozen, manifest, marker, stream)
                stream.close()
                digest = capture.verify_candidate(name, original, marker)
        if selected is not None:
            selected.check_row(con, row)
            selected.marker, selected.digest = marker, digest
        con.execute("UPDATE checkout SET pending=?,pending_sha=? WHERE singleton=1", (marker.encode(), digest))
    # Durable client intent precedes any remote bytes. A retry uses this exact
    # candidate, regardless of later writes to the local working directory.
    if selected is not None: return finish(state, marker.operation, selected=selected)
    return finish(state, marker.operation)


def branch_from_snapshot(state, *, selected):
    """An existing desktop decision copies its selected snapshot; it cannot capture live edits."""
    if selected.purpose != "branch" or not selected.used or Path(state).absolute() != selected.live.state:
        raise Rejected("not a confirmed original-node branch decision")
    selected.check_base()  # Also rejects a closed or unprepared decision before opening a writer.
    op, new, old = selected.operation, selected.proposed, selected.live.checkout_marker
    if new != Commit(old.repo, selected.branch_id, 1, op, old.operation, old.tree) or new.branch == old.branch:
        raise Rejected("branch marker does not represent the original reviewed node")
    with locked(state, live=selected.live, operation=op, phase="登记从原节点创建的新分支待决") as (con, row, _):
        selected.check_row(con, row)
        if row["pending"] is not None or con.execute(
                "SELECT 1 FROM completed WHERE operation=? UNION ALL "
                "SELECT 1 FROM forked_original WHERE operation=? OR successor=?", (op,op,op)).fetchone():
            raise Rejected("已有待决或该操作号已使用；不另建分支")
        selected.check_empty_slot()
        sha = selected.area.copy_with_marker(selected.source, selected.live.checkout_sha, new, selected.check_base)
        selected.check_row(con, row)
        selected.check_empty_slot()
        selected.marker, selected.digest = new, sha
        con.execute("UPDATE checkout SET pending=?,pending_sha=? WHERE singleton=1", (new.encode(), sha))
    # Exactly the existing pending/completed protocol. Only a confirmed result switches this base.
    return finish(state, op, selected=selected)


def status(state, *, live=None):
    with locked(state, read_only=True, live=live) as (con, row, _):
        rec = lifecycle.instance(con)
        if live is not None:
            live.check_instance(row, rec)
        return {"visibility":rec["phase"], "pending_visibility_operation":rec["active"], "worktree": row["worktree"], "repo": row["repo"], "branch": row["branch"],
                "base_generation": row["generation"], "base_operation": row["parent"],
                "pending_operation": Commit.decode(row["pending"]).operation if row["pending"] is not None else None}


def history(archive, store):
    """Follow original parent operations only. Missing history is not a synthetic root."""
    from .refs import Catalog
    catalog = Catalog(archive, store)
    return catalog.history(catalog.anchor.marker.operation)


def _record_fork(con, operation, marker, digest):
    con.execute("INSERT INTO forked_original VALUES(?,?)", (operation, marker.operation))
    con.execute("UPDATE checkout SET pending=?,pending_sha=? WHERE singleton=1", (marker.encode(), digest))


def _fork_reviewed(state, operation, selected):
    if (selected.purpose != "conflict-fork" or operation != selected.old.operation
            or Path(state).absolute() != selected.live.state or not selected.used):
        raise Rejected("not the confirmed conflict-fork decision for this original operation")
    selected.check_inputs()  # Refuses a closed/unprepared choice before opening a writer.
    old, proposed = selected.old, selected.proposed
    if (proposed != Commit(old.repo, proposed.branch, 1, selected.operation, old.parent, old.tree)
            or proposed.branch == old.branch or proposed.operation == old.operation):
        raise Rejected("new marker does not preserve the reviewed frozen candidate's ancestry")
    with locked(state, live=selected.live, operation=proposed.operation,
                phase="登记原冲突操作的指定后继；不能仅从私有副本判断是否已受理") as (con, row, _):
        selected.check_pending(con, row)
        selected.check_inputs()
        selected.check_empty_slot()
        sha = selected.area.copy_with_marker(selected.candidate, selected.old_sha, proposed, selected.check_inputs)
        selected.check_pending(con, row)
        selected.check_empty_slot()
        selected.check_inputs()
        selected.marker, selected.digest = proposed, sha
        _record_fork(con, operation, proposed, sha)
    # Same durable successor and held non-overwrite publication path, never the old branch slot.
    return finish(state, proposed.operation, selected=selected)


def fork_pending(state, operation, *, selected=None):
    """Explicitly rebranch the actual original frozen candidate, never today's worktree."""
    check_id(operation)
    require_native_writer()  # Reject an unavailable native primitive before opening/recovering SQLite.
    if selected is not None:
        return _fork_reviewed(state, operation, selected)
    with held_candidates(state) as area, locked(state) as (con, row, state):
        area.check_state()
        prior = con.execute("SELECT successor FROM forked_original WHERE operation=?", (operation,)).fetchone()
        if prior is not None:
            successor = prior[0]
        else:
            lifecycle.require_visible(con, row)
            if row["pending"] is None: raise Rejected("no pending directory snapshot")
            old = Commit.decode(row["pending"])
            if old.operation != operation:
                raise Rejected("refusing to replace a different pending operation")
            winner, _ = read_archive(Path(row["store"]) / old.filename)
            if (winner.repo, winner.branch, winner.generation) != (old.repo, old.branch, old.generation) or winner.operation == old.operation:
                raise Rejected("not a verified conflicting commit in the original branch slot")
            marker = Commit(old.repo, new_id(), 1, new_id(), old.parent, old.tree)
            successor = marker.operation
            digest = area.fork(old, row["pending_sha"], marker)
            _record_fork(con, operation, marker, digest)
    return finish(state, successor)
