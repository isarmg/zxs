"""Durable local checkout creation from one archived original. No server locks."""
from __future__ import annotations
from contextlib import ExitStack, closing, contextmanager
import hashlib
import os
from pathlib import Path
import sqlite3
import stat
import tempfile
from .archive import read_archive, read_managed_document, stamp
from .document import archive_sha, named_document
from .freeze import observe_tree
from .lifecycle import instance, locations, who
from .model import CHUNK, MAX_TOTAL, MAX_CD, Commit, Rejected, check_id, member_name, new_id
from .private_candidates import CandidateDirectory, generation, held_candidates
from .publish import digest_file, rename_new, require_native_writer, sync_dir

MAX_AUTO = 256
MAX_INITIAL_DB = 64 << 10  # One fresh PREPARED row; never serialize a live catalog.


def _target(row):
    n = row["ordinal"]
    if type(n) is not int or not 1 <= n <= MAX_AUTO or row["allocate"] not in (0,1) or not row["allocate"] and n != 1:
        raise Rejected("invalid original checkout name decision")
    leaf = row["stem"] if n == 1 else f"{row['stem']} ({n})"
    if len(member_name(leaf)) != 1: raise Rejected("invalid retained checkout name")
    return Path(row["parent"]) / leaf


def _parent(path):
    entry = who(path)
    if entry is None or entry[2] != stat.S_IFDIR: raise Rejected("checkout parent must be a real directory")
    return entry


@contextmanager
def _save_original(inp, size, area, expected):
    """Copy to one exclusive leaf in the already selected private seed directory."""
    name = "original.zxs"
    area.check_state()
    fd = os.open(name, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                 0o600, dir_fd=area.parent)
    try:
        with os.fdopen(fd, "w+b", closefd=False) as out:
            if area.check_file(name, fd)[2] != 0: raise Rejected("checkout copy is not new")
            left, h = size, hashlib.sha256()
            while left:
                data = inp.read(min(CHUNK, left))
                if not data or out.write(data) != len(data):
                    raise Rejected("incomplete copy of the selected checkout source")
                h.update(data); left -= len(data)
            if inp.read(1): raise Rejected("selected source grew")
            digest = h.hexdigest()
            if expected is not None and digest != expected.sha256: raise Rejected("selected original was replaced")
            out.flush(); os.fsync(fd)
            observed = area.check_file(name, fd)
            if observed[2] != size: raise Rejected("checkout copy length changed")
            marker = read_managed_document(out, display_name=name)[0]
            if archive_sha(out) != digest or expected is not None and marker != expected.marker:
                raise Rejected("checkout copy does not represent the selected original")
            area.check_file(name, fd, observed)
            os.fsync(area.parent)
            area.check_file(name, fd, observed)
        yield marker, digest, (fd, observed)
    finally:
        os.close(fd)


@contextmanager
def _source(source, seed_parent, expected):
    source = Path(source).absolute()  # A different spelling isn't the selected catalog slot.
    if source.suffix.lower() != ".zxs": raise Rejected("repository checkout requires the .zxs extension")
    seed = None
    try:
        with ExitStack() as held:
            with named_document(source) as inp:
                observed = stamp(inp)
                if expected is not None and (str(source) != str(expected.path) or observed != expected.observed):
                    raise Rejected("checkout source is not the selected catalog entry")
                if observed[2] > MAX_TOTAL + 2 * MAX_CD + (1 << 20):
                    raise Rejected("checkout source is outside the archive size budget")
                seed = Path(tempfile.mkdtemp(prefix=".zip-opening-", dir=seed_parent))
                area = held.enter_context(held_candidates(seed))
                marker, digest, copy = held.enter_context(_save_original(inp, observed[2], area, expected))
            yield area, marker, digest, copy
    except Exception as error:
        if seed is not None:
            error.add_note(f"签出准备位置是 {seed}；若已改名，请检查原 STATE。结果不明时不清理、不另建。")
        raise


@contextmanager
def _record(state, *, write):
    state = Path(state).absolute()
    if state.is_symlink() or state.stat().st_mode & 0o077: raise Rejected("checkout state is not private")
    con = sqlite3.connect((state/"opening.sqlite").as_uri()+("?mode=rw" if write else "?mode=ro"), uri=True, isolation_level=None)
    con.row_factory = sqlite3.Row
    try:
        if write:
            con.execute("PRAGMA synchronous=FULL"); con.execute("BEGIN IMMEDIATE")
        row = con.execute("SELECT * FROM opening WHERE singleton=1").fetchone()
        if row is None: raise Rejected("original checkout operation not available")
        yield con,row,state
        if write: con.commit()
    except BaseException:
        if write: con.rollback()
        raise
    finally: con.close()


def status(state):
    with _record(state,write=False) as (_,row,_):
        marker = Commit.decode(row["marker"])
        return {"operation":row["operation"], "worktree":row["worktree"], "source_node":marker.operation,
                "chosen_destination":str(_target(row)),
                "recorded_stage":"attached" if row["done"] else "verified_stage" if row["stage"] else "original_recorded"}


def _initial_image(fields):
    # No SQLite pathname, journal or second receipt: this database has never been opened on disk.
    budget = MAX_INITIAL_DB // 2
    if any(len(x) > budget for x in fields if isinstance(x, (str, bytes))):
        raise Rejected("initial checkout record exceeds the size budget")
    if sum(len(x) if isinstance(x, bytes) else len(str(x).encode()) for x in fields) > budget:
        raise Rejected("initial checkout record exceeds the size budget")
    with closing(sqlite3.connect(":memory:")) as con:
        page = con.execute("PRAGMA page_size").fetchone()[0]
        pages = MAX_INITIAL_DB // page
        if pages < 2 or con.execute(f"PRAGMA max_page_count={pages}").fetchone()[0] != pages:
            raise Rejected("cannot bound the initial checkout database")
        con.setlimit(sqlite3.SQLITE_LIMIT_LENGTH, MAX_INITIAL_DB)
        con.execute("""CREATE TABLE opening(singleton INTEGER PRIMARY KEY CHECK(singleton=1), operation TEXT NOT NULL,
        worktree TEXT NOT NULL, marker BLOB NOT NULL, sha TEXT NOT NULL, parent TEXT NOT NULL, device TEXT NOT NULL,
        inode TEXT NOT NULL, store TEXT NOT NULL, stem TEXT NOT NULL, allocate INTEGER NOT NULL,ordinal INTEGER NOT NULL,
        stage TEXT, stage_dev TEXT, stage_ino TEXT, done INTEGER NOT NULL)""")
        con.execute("INSERT INTO opening VALUES(1,?,?,?,?,?,?,?,?,?,?,1,NULL,NULL,NULL,0)", fields)
        con.commit()
        size = page * con.execute("PRAGMA page_count").fetchone()[0]
        if not 0 < size <= MAX_INITIAL_DB: raise Rejected("initial checkout database exceeds the size budget")
        data = con.serialize()
        if len(data) != size: raise Rejected("incomplete initial checkout database")
        return data


def _seed_names(area, *names):
    missing = set(names)
    with os.scandir(area.parent) as entries:
        for entry in entries:
            if entry.name not in missing: raise Rejected("unexpected entry in the private checkout seed")
            missing.remove(entry.name)
    if missing: raise Rejected("incomplete private checkout seed")


def _install_initial(area, copy, image, state):
    area.check_file("original.zxs", *copy)
    _seed_names(area, "original.zxs")
    name = "opening.sqlite"
    fd = os.open(name, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                 0o600, dir_fd=area.parent)
    try:
        with os.fdopen(fd, "w+b", closefd=False) as out:
            if area.check_file(name, fd)[2] or out.write(image) != len(image):
                raise Rejected("incomplete initial checkout database write")
            out.flush(); os.fsync(fd)
            observed = area.check_file(name, fd)
            out.seek(0)
            if observed[2] != len(image) or out.read(len(image)) != image or out.read(1):
                raise Rejected("initial checkout database readback differs")

        def check_pair(held, *, db=None):
            _seed_names(held, "original.zxs", name)
            held.check_file("original.zxs", *copy)
            if db is not None:
                held.check_file(name, db, observed)
            else:
                m = os.stat(name, dir_fd=held.parent, follow_symlinks=False)
                if not stat.S_ISREG(m.st_mode) or m.st_nlink != 1 or generation(m) != observed:
                    raise Rejected("installed checkout database no longer matches its prepared generation")

        check_pair(area, db=fd)
        os.fsync(area.parent)
        check_pair(area, db=fd)
    finally:
        # Closing a non-SQLite fd while SQLite has this inode open in the same
        # process can drop its POSIX locks. Close before exposing the STATE name.
        os.close(fd)

    rename_new(area.path, state)  # Existing CLI requires a stable private STATE parent.
    installed = CandidateDirectory(state, area.parent, area.identity)
    check_pair(installed)
    sync_dir(state.parent)
    check_pair(installed)


def begin(source, parent, stem, store, state, *, automatic, expected=None):
    from .worktree import paths
    require_native_writer()
    if not callable(getattr(sqlite3.Connection, "serialize", None)) or os.scandir not in os.supports_fd:
        raise Rejected("fresh checkout needs SQLite serialization and held-directory inventory")
    if len(member_name(stem)) != 1 or automatic and len(stem.encode()) > 230:
        raise Rejected("choose one portable checkout name with room for a collision suffix")
    parent = Path(parent).resolve(strict=True)
    dest, store, state = paths(parent/stem,store,state)
    if os.path.lexists(state) or not automatic and os.path.lexists(dest):
        raise Rejected("checkout state or requested destination is occupied")
    owner = _parent(parent)
    operation, worktree = new_id(), new_id()
    with _source(source, state.parent, expected) as (area, marker, sha, copy):
        fields = (operation, worktree, marker.encode(), sha, str(parent), owner[0], owner[1],
                  str(store), stem, int(automatic))
        _install_initial(area, copy, _initial_image(fields), state)
    return finish(state,operation)


def _verify_record(row,operation):
    if row["operation"] != check_id(operation): raise Rejected("checkout retry must name the original operation")
    parent = Path(row["parent"])
    if _parent(parent) != (row["device"],row["inode"],stat.S_IFDIR):
        raise Rejected("registered checkout parent directory changed")
    return parent, Commit.decode(row["marker"])


def _catalog_matches(state,row,dest):
    from .worktree import locked
    with locked(state) as (con,attached,_):
        original = instance(con)
        if (attached["worktree"],attached["location"],attached["store"],attached["repo"]) != (
            row["worktree"],str(dest),row["store"],Commit.decode(row["marker"]).repo
        ) or (original["device"],original["inode"]) != (row["stage_dev"],row["stage_ino"]):
            raise Rejected("an unrelated catalog occupies the local registration; no metadata was overwritten")
        visible, hidden = locations(attached)
        phase = original["phase"]
        path = visible if phase == "visible" else hidden if phase == "hidden" else None
        return path is not None and who(path) == (original["device"], original["inode"], stat.S_IFDIR)


def finish(state, operation):
    from .worktree import create_catalog, paths
    require_native_writer()
    with _record(state,write=True) as (con,row,state):
        parent, marker = _verify_record(row,operation)
        dest = _target(row)
        if row["done"]:
            _catalog_matches(state,row,dest)
            return dest,marker  # Historical receipt, never reattach or move a later checkout.
        if row["stage"] is None:
            local = state/"original.zxs"
            if digest_file(local) != row["sha"]: raise Rejected("original checkout archive is unavailable")
            stage = Path(tempfile.mkdtemp(prefix=".zip-checkout-",dir=parent))
            held = os.open(stage, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                m = os.fstat(held); sid = str(m.st_dev), str(m.st_ino), stat.S_IFDIR
                represented,_ = read_archive(local,sink=stage)
                if (represented != marker or digest_file(local) != row["sha"]
                        or observe_tree(stage).digest() != marker.tree or _parent(stage) != sid):
                    raise Rejected("private checkout stage is not the original verified directory")
                sync_dir(parent)
            finally: os.close(held)
            con.execute("UPDATE opening SET stage=?,stage_dev=?,stage_ino=? WHERE singleton=1", (stage.name,sid[0],sid[1]))
    # Verified stage identity is durable before ANY attempted visible rename.
    while True:
        with _record(state,write=True) as (con,row,state):
            parent,marker = _verify_record(row,operation)
            dest = _target(row)
            paths(dest,Path(row["store"]),state)
            stage = parent / row["stage"]
            if stage.parent != parent or not row["stage"].startswith(".zip-checkout-"):
                raise Rejected("invalid registered checkout stage")
            sid = (row["stage_dev"],row["stage_ino"],stat.S_IFDIR)
            if row["done"] or os.path.lexists(state/"worktree.sqlite"):
                named = _catalog_matches(state,row,dest)
                if not row["done"] and not named:
                    raise Rejected("original checkout registration exists without its exact named worktree; not finalizing")
                con.execute("UPDATE opening SET done=1 WHERE singleton=1")
                return dest,marker
            src, target = who(stage), who(dest)
            if src == sid and target is not None:
                if target == sid: raise Rejected("both names refer to the prepared worktree; preserving the ambiguous pair")
                if not row["allocate"] or row["ordinal"] >= MAX_AUTO:
                    raise Rejected("requested checkout name or all automatic candidates are occupied; original stage preserved")
                con.execute("UPDATE opening SET ordinal=ordinal+1 WHERE singleton=1")
                continue
            if (src,target) not in ((sid,None),(None,sid)):
                raise Rejected("neither complete original stage/destination pair is present")
            current = stage if src == sid else dest
            if observe_tree(current).digest() != marker.tree:
                raise Rejected("original prepared checkout contents changed; no existing data was adopted")
            if src == sid:
                try: rename_new(stage,dest)
                except FileExistsError:
                    # Reobserve both names before selecting another suffix.
                    if not row["allocate"] or who(stage) != sid or who(dest) in (None,sid):
                        raise Rejected("checkout name collision has an uncertain original state")
                    if row["ordinal"] >= MAX_AUTO: raise Rejected("all automatic checkout names are occupied")
                    con.execute("UPDATE opening SET ordinal=ordinal+1 WHERE singleton=1")
                    continue
            sync_dir(parent)
            if who(dest) != sid or who(stage) is not None: raise Rejected("checkout rename outcome is unknown")
            create_catalog(dest,Path(row["store"]),state,(marker,row["sha"]),row["worktree"])
            if not _catalog_matches(state,row,dest):
                raise Rejected("original directory absent after new catalog publication")
            con.execute("UPDATE opening SET done=1 WHERE singleton=1")
            return dest,marker
