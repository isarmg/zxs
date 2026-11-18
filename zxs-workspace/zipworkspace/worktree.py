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
from .freeze import frozen_tree, pack
from .model import Commit, Rejected, check_id, new_id
from .publish import confirm_final, digest_file, publish, rename_new, require_native_writer, sync_dir


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


def create_catalog(work, store, state, base, instance_id):
    fd, tmp = tempfile.mkstemp(prefix=".catalog-", dir=state)
    os.close(fd)
    temp = Path(tmp)
    con = sqlite3.connect(temp)
    try:
        con.execute("PRAGMA synchronous=FULL")
        con.execute("""CREATE TABLE checkout(singleton INTEGER PRIMARY KEY CHECK(singleton=1),
          worktree TEXT NOT NULL, location TEXT NOT NULL, store TEXT NOT NULL, repo TEXT NOT NULL,
          branch TEXT NOT NULL, generation INTEGER NOT NULL, parent TEXT, base_sha TEXT,
          pending BLOB, pending_sha TEXT)""")
        con.execute("CREATE TABLE completed(operation TEXT PRIMARY KEY, marker BLOB NOT NULL, digest TEXT NOT NULL)")
        lifecycle.install(con, work)
        con.execute("CREATE TABLE forked_original(operation TEXT PRIMARY KEY, successor TEXT NOT NULL)")
        root, source_sha = base if base else (None, None)
        con.execute("INSERT INTO checkout VALUES(1,?,?,?,?,?,?,?,?,NULL,NULL)", (
            instance_id, str(work), str(store), root.repo if root else new_id(),
            root.branch if root else new_id(), root.generation if root else 0,
            root.operation if root else None, source_sha))
        con.commit()
    finally: con.close()
    with temp.open("rb") as f: os.fsync(f.fileno())
    rename_new(temp, state / "worktree.sqlite")
    sync_dir(state)


@contextmanager
def locked(state):
    state = Path(state).absolute()
    if not state.is_dir() or state.is_symlink() or (state.stat().st_mode & 0o077):
        raise Rejected("use the existing private worktree state directory")
    # mode=rw will not create a blank authority if an original catalog goes missing.
    con = sqlite3.connect((state / "worktree.sqlite").as_uri() + "?mode=rw", uri=True, isolation_level=None)
    con.row_factory = sqlite3.Row
    try:
        con.execute("PRAGMA synchronous=FULL")
        con.execute("BEGIN IMMEDIATE")
        row = con.execute("SELECT * FROM checkout WHERE singleton=1").fetchone()
        if row is None: raise Rejected("worktree record missing")
        yield con, row, state
        con.commit()
    except BaseException:
        con.rollback()
        raise
    finally: con.close()


def import_zip(archive, dest):
    require_native_writer()
    dest = Path(dest).absolute()
    if dest.exists() or dest.is_symlink(): raise Rejected("checkout needs a fresh destination")
    # On failure a private incomplete stage may survive; do not remove or
    # recursively clean an unknown name in a user-selected parent directory.
    stage = Path(tempfile.mkdtemp(prefix=".zip-checkout-", dir=dest.parent))
    _, manifest = read_archive(archive, managed=False, sink=stage)
    rename_new(stage, dest)
    sync_dir(dest.parent)
    return manifest


def checkout(archive, dest, store, state, *, expected=None):
    from . import opening
    dest, store, state = paths(dest, store, state)
    _, marker = opening.begin(archive, dest.parent, dest.name, store, state, automatic=False, expected=expected)
    return marker


def finish(state, operation):
    operation = check_id(operation)
    with locked(state) as (con, row, state):
        done = con.execute("SELECT marker,digest FROM completed WHERE operation=?", (operation,)).fetchone()
        if done is not None:
            marker = Commit.decode(done["marker"])
            target = Path(row["store"]) / marker.filename
            if marker.operation != operation or not confirm_final(target, marker, done["digest"]):
                raise Rejected("completed original archive is unavailable; no replacement was published")
            return target, marker
        superseded = con.execute("SELECT successor FROM forked_original WHERE operation=?", (operation,)).fetchone()
        if superseded is not None: raise Rejected("original conflict remains preserved; separate explicit fork: " + superseded[0])
        if row["pending"] is None: raise Rejected("no original operation recorded here; not reconstructing it")
        marker = Commit.decode(row["pending"])
        if marker.operation != operation: raise Rejected("another operation is pending; no worktree was recaptured")
        local = state / (marker.operation + ".zxs")
        target = publish(local, Path(row["store"]), marker, row["pending_sha"])
        con.execute("INSERT INTO completed(operation,marker,digest) VALUES(?,?,?)", (operation, marker.encode(), row["pending_sha"]))
        con.execute("""UPDATE checkout SET branch=?,generation=?,parent=?,base_sha=?,pending=NULL,pending_sha=NULL
                       WHERE singleton=1""", (marker.branch, marker.generation, marker.operation, row["pending_sha"]))
        # The immutable private spool remains recoverable; cleanup is not part of this prototype.
    return target, marker


def commit(state, *, fork=False):
    require_native_writer()
    with locked(state) as (con, row, state):
        if row["pending"] is not None:
            raise Rejected("an original operation is pending; resume it rather than recapturing")
        lifecycle.require_visible(con, row)
        work, store, state = paths(Path(row["location"]), Path(row["store"]), state)
        gen, branch, repo = row["generation"], row["branch"], row["repo"]
        if gen:
            base_path = store / f"{repo}.{branch}.{gen:016x}.zxs"
            saved, _ = read_archive(base_path)
            if (saved.repo, saved.branch, saved.generation, saved.operation) != (repo, branch, gen, row["parent"]) or digest_file(base_path) != row["base_sha"]:
                raise Rejected("exact original branch base is not available in the selected store")
        operation = new_id()
        with frozen_tree(work, state) as (frozen, manifest):
            marker = Commit(repo, new_id() if fork else branch, 1 if fork else gen + 1,
                            operation, row["parent"], manifest.digest())
            local = state / (operation + ".zxs")
            pack(frozen, manifest, marker, local)
        if read_archive(local)[0] != marker: raise Rejected("local closed archive verification failed")
        digest = digest_file(local)
        sync_dir(state)
        con.execute("UPDATE checkout SET pending=?,pending_sha=? WHERE singleton=1", (marker.encode(), digest))
    # Durable client intent precedes any remote bytes. A retry uses this exact
    # candidate, regardless of later writes to the local working directory.
    return finish(state, marker.operation)


def status(state):
    with locked(state) as (con, row, _):
        rec = lifecycle.instance(con)
        return {"visibility":rec["phase"], "pending_visibility_operation":rec["active"], "worktree": row["worktree"], "repo": row["repo"], "branch": row["branch"],
                "base_generation": row["generation"], "base_operation": row["parent"],
                "pending_operation": Commit.decode(row["pending"]).operation if row["pending"] is not None else None}


def history(archive, store):
    """Follow original parent operations only. Missing history is not a synthetic root."""
    from .refs import Catalog
    catalog = Catalog(archive, store)
    return catalog.history(catalog.anchor.marker.operation)


def fork_pending(state, operation):
    """Explicitly rebranch the actual original frozen candidate, never today's worktree."""
    import shutil
    import zipfile
    check_id(operation)
    with locked(state) as (con, row, state):
        prior = con.execute("SELECT successor FROM forked_original WHERE operation=?", (operation,)).fetchone()
        if prior is not None:
            successor = prior[0]
        else:
            lifecycle.require_visible(con, row)
            if row["pending"] is None: raise Rejected("no pending directory snapshot")
            old = Commit.decode(row["pending"])
            if old.operation != operation:
                raise Rejected("refusing to replace a different pending operation")
            oldfile = state / (operation + ".zxs")
            if digest_file(oldfile) != row["pending_sha"] or read_archive(oldfile)[0] != old:
                raise Rejected("original frozen candidate is unavailable")
            winner, _ = read_archive(Path(row["store"]) / old.filename)
            if (winner.repo, winner.branch, winner.generation) != (old.repo, old.branch, old.generation) or winner.operation == old.operation:
                raise Rejected("not a verified conflicting commit in the original branch slot")
            marker = Commit(old.repo, new_id(), 1, new_id(), old.parent, old.tree)
            successor = marker.operation
            output = state / (successor + ".zxs")
            with oldfile.open("rb") as src, output.open("xb") as dst:
                shutil.copyfileobj(src, dst, length=1 << 20)
            # Only modify the new private copy. No archive member is changed.
            with output.open("r+b") as io:
                with zipfile.ZipFile(io, "a", allowZip64=True) as z: z.comment = marker.encode()
                io.flush(); os.fsync(io.fileno())
            if read_archive(output)[0] != marker: raise Rejected("rebranched candidate failed readback")
            digest = digest_file(output)
            sync_dir(state)
            con.execute("INSERT INTO forked_original VALUES(?,?)", (operation, successor))
            con.execute("UPDATE checkout SET pending=?,pending_sha=? WHERE singleton=1", (marker.encode(), digest))
    return finish(state, successor)
