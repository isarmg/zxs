"""Durable local checkout creation from one archived original. No server locks."""
from __future__ import annotations
from contextlib import ExitStack, closing, contextmanager, nullcontext
import hashlib
import os
from pathlib import Path
import sqlite3
import stat
import tempfile
from .archive import read_archive, read_managed_document, stamp
from .capacity import buffered_output, close_output_fd, note, writing
from .document import archive_sha, named_document
from .freeze import observe_held, observe_tree
from . import private_stage as staging
from .lifecycle import instance, locations, who
from .model import CHUNK, MAX_TOTAL, MAX_CD, Commit, Rejected, check_id, member_name, new_id
from .private_candidates import CandidateDirectory, generation, held_candidates
from .publish import digest_file, held_store, rename_new, rename_new_at, require_native_writer, sync_dir

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
def _save_original(read, size, area, expected, operation, *, observer=None):
    """Copy to one exclusive leaf in the already selected private seed directory."""
    name = "original.zxs"
    if observer is not None: observer.begin("capture", size)
    area.check_state()
    with writing("STATE", area.path/name, "独占建立原始包副本", operation, area.parent):
        fd = os.open(name, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                     0o600, dir_fd=area.parent)
    try:
        with buffered_output(os.fdopen(fd, "w+b", closefd=False), "STATE", area.path/name,
                             operation, area.parent) as out:
            if area.check_file(name, fd)[2] != 0: raise Rejected("checkout copy is not new")
            left, h = size, hashlib.sha256()
            while left:
                if observer is not None: observer.check()
                count = min(CHUNK, left)
                data = read(count)
                if observer is not None: observer.check()
                if not 0 < len(data) <= count: raise Rejected("incomplete copy of the selected checkout source")
                try: count = out.write(data)
                except OSError as error:
                    note(error, "STATE", area.path/name, "写入原始包副本", operation, area.parent)
                    raise
                if count != len(data): raise Rejected("incomplete copy of the selected checkout source")
                h.update(data); left -= len(data)
                if observer is not None: observer.advance(len(data))
            if observer is not None: observer.check()
            if read(1): raise Rejected("selected source grew")
            digest = h.hexdigest()
            if expected is not None and digest != expected.sha256: raise Rejected("selected original was replaced")
            if observer is not None: observer.begin("sync")
            with writing("STATE", area.path/name, "刷新并同步原始包副本", operation, area.parent):
                out.flush(); os.fsync(fd)
            observed = area.check_file(name, fd)
            if observed[2] != size: raise Rejected("checkout copy length changed")
            if observer is not None: observer.section("私有原包读回")
            watch = {} if observer is None else {"observer": observer}
            marker = read_managed_document(out, display_name=name, **watch)[0]
            if archive_sha(out, **watch) != digest or expected is not None and marker != expected.marker:
                raise Rejected("checkout copy does not represent the selected original")
            area.check_file(name, fd, observed)
            if observer is not None: observer.begin("sync")
            with writing("STATE", area.path, "同步原始包副本的目录项", operation, area.parent):
                os.fsync(area.parent)
            area.check_file(name, fd, observed)
        yield marker, digest, (fd, observed)
    finally:
        close_output_fd(fd, "STATE", area.path/name, operation, area.parent)


def _read_selected(fd):
    """An independent sequential cursor on a borrowed read FD (dup shares lseek state)."""
    offset = 0

    def read(size):
        nonlocal offset
        data = os.pread(fd, size, offset)
        offset += len(data)
        return data

    return read


@contextmanager
def _source(source, seed_parent, expected, *, exclusive=None, mount=None, selected=None, operation=None):
    source = Path(source).absolute()  # A different spelling isn't the selected catalog slot.
    if source.suffix.lower() != ".zxs": raise Rejected("repository checkout requires the .zxs extension")
    seed = None
    observer = None if selected is None else selected.preparation
    try:
        with ExitStack() as held:
            with (named_document(source) if selected is None else nullcontext()) as inp:
                if selected is None:
                    observed, read = stamp(inp), inp.read
                else:
                    observed = selected.check_source()
                    read = _read_selected(selected.source_fd)
                if expected is not None and (str(source) != str(expected.path) or observed != expected.observed):
                    raise Rejected("checkout source is not the selected catalog entry")
                if observed[2] > MAX_TOTAL + 2 * MAX_CD + (1 << 20):
                    raise Rejected("checkout source is outside the archive size budget")
                if observer is not None: observer.check()
                if exclusive is None:
                    with writing("STATE", seed_parent, "建立私有签出准备目录", operation):
                        seed = Path(tempfile.mkdtemp(prefix=".zip-opening-", dir=seed_parent))
                    area = held.enter_context(held_candidates(seed))
                else:
                    seed = exclusive.path / (".zip-opening-" + new_id())
                    with writing("STATE", seed_parent, "建立私有签出准备目录", operation, exclusive.parent):
                        area = held.enter_context(staging.new_child(exclusive, seed.name, mount))
                marker, digest, copy = held.enter_context(
                    _save_original(read, observed[2], area, expected, operation, observer=observer))
                if selected is not None:
                    selected.check_source()
                    if marker != selected.snapshot.browser.commit:
                        raise Rejected("copied archive is not the reviewed snapshot node")
                    selected.verified_sha = digest
            yield area, marker, digest, copy
    except Exception as error:
        if seed is not None:
            error.add_note(f"签出准备位置是 {seed}；若已改名，请检查原 STATE。结果不明时不清理、不另建。")
        raise


@contextmanager
def _record(state, *, write, selected=None, operation=None):
    state = Path(state).absolute()
    if selected is not None: selected.check_registered(state)
    if state.is_symlink() or state.stat().st_mode & 0o077: raise Rejected("checkout state is not private")
    db = state / "opening.sqlite"
    def database_error(error):
        if write:
            note(error, "STATE", db, "登记签出决策或回执；目录可能已安装", operation,
                 selected.publication[0] if selected is not None else None, database=True)
    try: con = sqlite3.connect(db.as_uri()+("?mode=rw" if write else "?mode=ro"), uri=True, isolation_level=None)
    except sqlite3.Error as error:
        database_error(error); raise
    con.row_factory = sqlite3.Row
    try:
        if selected is not None: selected.check_registered(state)
        if write:
            if selected is not None and con.execute("PRAGMA main.journal_mode").fetchone()[0] != "delete":
                raise Rejected("原签出登记不再使用默认 DELETE 回滚日志；不会更改日志模式")
            con.execute("PRAGMA synchronous=FULL"); con.execute("BEGIN IMMEDIATE")
        row = con.execute("SELECT * FROM opening WHERE singleton=1").fetchone()
        if row is None: raise Rejected("original checkout operation not available")
        if selected is not None:
            if (row["operation"] != selected.operation or row["sha"] != selected.verified_sha or
                    row["allocate"] != 0 or row["ordinal"] != 1 or _target(row) != selected.destination or
                    row["store"] != str(selected.store.path) or
                    Commit.decode(row["marker"]) != selected.snapshot.browser.commit):
                raise Rejected("CLI STATE does not contain the reviewed operation and node")
            selected.check_prepared_row(row)
            selected.initial_opened = True
            if write: selected.check_recovery_row(row)
        yield con,row,state
        if selected is not None: selected.check_registered(state)
        if write: con.commit()
    except BaseException as error:
        database_error(error)
        if write: con.rollback()
        raise
    finally: con.close()


def status(state, *, selected=None):
    with _record(state, write=False, selected=selected) as (_,row,_):
        marker = Commit.decode(row["marker"])
        _private_origin(row)
        return {"operation":row["operation"], "worktree":row["worktree"], "source_node":marker.operation,
                "chosen_destination":str(_target(row)),
                "recorded_stage":"attached" if row["done"] else "verified_stage" if row["stage"] else "original_recorded"}


def _private_origin(row):
    keys = {"private_device", "private_inode", "private_base_inode"}
    present = keys.intersection(row.keys())
    if not present and row["stage"] != staging.LEAF: return None  # Original target-parent protocol.
    if (present != keys or row["stage"] != staging.LEAF or
            not all(isinstance(row[k], str) and row[k].isdigit() and int(row[k]) > 0
                    for k in (*keys, "stage_dev", "stage_ino"))):
        raise Rejected("incomplete original private checkout decision; no alternate staging")
    return row["private_device"], row["private_inode"]


def _initial_image(fields, *, prepared=None):
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
        if prepared is not None:
            # One initial row, already fully verified before it is exposed. Older
            # clients reject this non-legacy stage name instead of unpacking it.
            con.execute("ALTER TABLE opening ADD COLUMN private_device TEXT")
            con.execute("ALTER TABLE opening ADD COLUMN private_inode TEXT")
            con.execute("ALTER TABLE opening ADD COLUMN private_base_inode TEXT")
            con.execute("UPDATE opening SET stage=?, stage_dev=?, stage_ino=?, private_device=?, private_inode=?, private_base_inode=?",
                        (staging.LEAF, *map(str, prepared)))
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


def _install_initial(area, copy, image, state, *, base=None, stage=None, selected=None, worktree=None):
    mount = staging.mount_id(base.parent) if base is not None else None
    if base is not None:
        staging.check_private(base)
        staging.check_private(area)
    area.check_file("original.zxs", *copy)
    extra = (staging.LEAF,) if stage is not None else ()
    _seed_names(area, "original.zxs", *extra)
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
            _seed_names(held, "original.zxs", name, *extra)
            if stage is not None:
                staging.check_private(base)
                staging.check_private(held)
                staging.check_named(held.parent, staging.LEAF, stage.parent)
                staging.check_mount_name(base.parent, held.path.name, held.identity, mount)
                staging.check_mount_name(held.parent, staging.LEAF, stage.identity, mount)
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
        if selected is not None:
            selected.witness_prepared(area, stage, fd, observed, worktree)
    finally:
        # Closing a non-SQLite fd while SQLite has this inode open in the same
        # process can drop its POSIX locks. Close before exposing the STATE name.
        os.close(fd)

    if base is None:
        rename_new(area.path, state)  # Legacy CLI requires a stable private STATE parent.
    else:
        staging.check_private(base)
        if selected is not None: selected.installing(area, observed)
        rename_new_at(base.parent, area.path.name, state.name, expected_source=os.fstat(area.parent))
    installed = CandidateDirectory(state, area.parent, area.identity)
    check_pair(installed)
    if base is None: sync_dir(state.parent)
    else:
        os.fsync(base.parent)
        staging.check_private(base)
    check_pair(installed)


def begin(source, parent, stem, store, state, *, automatic, expected=None, private_stage=False, selected=None):
    from .worktree import paths
    require_native_writer()
    if not callable(getattr(sqlite3.Connection, "serialize", None)) or os.scandir not in os.supports_fd:
        raise Rejected("fresh checkout needs SQLite serialization and held-directory inventory")
    if len(member_name(stem)) != 1 or automatic and len(stem.encode()) > 230:
        raise Rejected("choose one portable checkout name with room for a collision suffix")
    parent = Path(parent).absolute() if private_stage else Path(parent).resolve(strict=True)
    dest, store, state = paths(parent/stem,store,state)
    if os.path.lexists(state) or not automatic and os.path.lexists(dest):
        raise Rejected("checkout state or requested destination is occupied")
    owner = _parent(parent)
    if selected is not None:
        if (not private_stage or automatic or Path(source).absolute() != selected.snapshot.path or
                dest != selected.destination or store != selected.store.path or state != selected.state or
                expected != selected.snapshot.exact):
            raise Rejected("checkout command differs from the reviewed original selection")
        selected.check_new()
    operation = new_id() if selected is None else check_id(selected.operation)
    worktree = new_id()
    with ExitStack() as held:
        base = mount = None
        if selected is not None:
            # Borrow the actual three picker FDs; do not select a new directory by its display path.
            base, target, store_root = selected.base, selected.target.check(), selected.store
            mount = selected.check_roots()
            m = os.fstat(target)
            if owner != (str(m.st_dev), str(m.st_ino), stat.S_IFDIR): raise Rejected("selected target changed")
        elif private_stage:
            base = held.enter_context(held_candidates(state.parent))
            target, _ = held.enter_context(_held_directory(parent, owner))
            mount = staging.same_mount(base.parent, target)
            store_root = held.enter_context(held_store(store))
            staging.separate_roots(base.parent, target, store_root, mount)
            if _parent(parent) != owner: raise Rejected("chosen target parent changed")
        area, marker, sha, copy = held.enter_context(_source(source, state.parent, expected, exclusive=base, mount=mount, selected=selected, operation=operation))
        fields = (operation, worktree, marker.encode(), sha, str(parent), owner[0], owner[1],
                  str(store), stem, int(automatic))
        if base is None:
            image = _initial_image(fields)  # In-memory SQL is not evidence about the STATE volume.
            with writing("STATE", state, "写入、安装并同步首笔签出登记", operation, area.parent):
                _install_initial(area, copy, image, state)
        else:
            observer = None if selected is None else selected.preparation
            with writing("STATE", area.path/staging.LEAF, "在原私处解包并同步完整工作树", operation, area.parent):
                stage = held.enter_context(staging.materialize(area, copy, marker, mount, observer=observer))
            if observer is not None:
                observer.section("")
                observer.begin("record")
            image = _initial_image(fields, prepared=(*stage.identity, *area.identity, base.identity[1]))
            if _parent(parent) != owner: raise Rejected("chosen target parent changed")
            staging.separate_roots(base.parent, target, store_root, mount)
            with writing("STATE", state, "写入、安装并同步首笔签出登记", operation, base.parent):
                _install_initial(area, copy, image, state, base=base, stage=stage, selected=selected, worktree=worktree)
            if selected is not None:
                selected.retain_initial_seed(CandidateDirectory(state, area.parent, area.identity), copy)
    if selected is not None:
        selected.registered = True  # Seed/tree, outer parent, and all producer closes succeeded.
        return finish(state, operation, selected=selected)
    return finish(state,operation)


def _verify_record(row,operation):
    if row["operation"] != check_id(operation): raise Rejected("checkout retry must name the original operation")
    parent = Path(row["parent"])
    if _parent(parent) != (row["device"],row["inode"],stat.S_IFDIR):
        raise Rejected("registered checkout parent directory changed")
    return parent, Commit.decode(row["marker"])


def _catalog_matches(state,row,dest, *, parent_fd=None, live=None):
    from .worktree import locked
    with locked(state, read_only=True, live=live) as (con,attached,_):
        original = instance(con)
        if (attached["worktree"],attached["location"],attached["store"],attached["repo"]) != (
            row["worktree"],str(dest),row["store"],Commit.decode(row["marker"]).repo
        ) or (original["device"],original["inode"]) != (row["stage_dev"],row["stage_ino"]):
            raise Rejected("an unrelated catalog occupies the local registration; no metadata was overwritten")
        visible, hidden = locations(attached)
        phase = original["phase"]
        path = visible if phase == "visible" else hidden if phase == "hidden" else None
        if path is None or path.parent != dest.parent: return None
        named = who(path) if parent_fd is None else who(path.name, dir_fd=parent_fd)
        return path if named == (original["device"], original["inode"], stat.S_IFDIR) else None


@contextmanager
def _held_directory(name, expected, *, parent_fd=None):
    fd = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent_fd)
    try:
        m = os.fstat(fd)
        if not m.st_ino or (str(m.st_dev), str(m.st_ino), stat.S_IFMT(m.st_mode)) != expected:
            raise Rejected("registered checkout directory was replaced")
        yield fd, m
    finally:
        os.close(fd)


def finish(state, operation, *, selected=None):
    from .worktree import create_catalog, paths
    require_native_writer()
    if selected is not None: selected.authorize_finish(state, operation)
    with _record(state, write=True, selected=selected, operation=operation) as (con,row,state):
        parent, marker = _verify_record(row,operation)
        _private_origin(row)  # A damaged private record cannot fall through to the legacy extractor.
        dest = _target(row)
        if row["done"]:
            _catalog_matches(state,row,dest)
            return dest,marker  # Historical receipt, never reattach or move a later checkout.
        if row["stage"] is None:
            local = state/"original.zxs"
            if digest_file(local) != row["sha"]: raise Rejected("original checkout archive is unavailable")
            with writing("工作目录父", parent, "建立原签出暂存目录", operation):
                stage = Path(tempfile.mkdtemp(prefix=".zip-checkout-",dir=parent))
            held = os.open(stage, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                m = os.fstat(held); sid = str(m.st_dev), str(m.st_ino), stat.S_IFDIR
                # Legacy extraction reads original.zxs on STATE as well as writing a different
                # filesystem. Do not attribute arbitrary archive-reader errors to the target.
                represented,_ = read_archive(local,sink=stage)
                if (represented != marker or digest_file(local) != row["sha"]
                        or observe_tree(stage).digest() != marker.tree or _parent(stage) != sid):
                    raise Rejected("private checkout stage is not the original verified directory")
                with writing("工作目录父", parent, "同步原签出暂存目录项", operation, held):
                    sync_dir(parent)
            finally: os.close(held)
            con.execute("UPDATE opening SET stage=?,stage_dev=?,stage_ino=? WHERE singleton=1", (stage.name,sid[0],sid[1]))
    # The original stage decision is durable before any visible rename.
    while True:
        with ExitStack() as held:
            with _record(state, write=True, selected=selected, operation=operation) as (con, row, state):
                parent, marker = _verify_record(row, operation)
                dest = _target(row)
                paths(dest, Path(row["store"]), state)
                stage = row["stage"]
                private_id = _private_origin(row)
                if private_id is None and (not isinstance(stage, str) or Path(stage).name != stage
                                           or not stage.startswith(".zip-checkout-")):
                    raise Rejected("invalid registered checkout stage")
                if row["done"]:
                    _catalog_matches(state, row, dest)
                    return dest, marker  # A past receipt, not proof the checkout is still visible.
                sid = row["stage_dev"], row["stage_ino"], stat.S_IFDIR
                pid = row["device"], row["inode"], stat.S_IFDIR
                pfd, _ = held.enter_context(_held_directory(parent, pid))
                private = None
                source_fd = pfd
                if private_id is not None:
                    private = held.enter_context(held_candidates(state))
                    if tuple(map(str, private.identity)) != private_id:
                        raise Rejected("the original private checkout parent changed")
                    base = held.enter_context(held_candidates(state.parent))
                    if tuple(map(str, base.identity)) != (row["private_device"], row["private_base_inode"]):
                        raise Rejected("the private STATE parent was replaced")
                    mount = staging.same_mount(base.parent, pfd)
                    if staging.same_mount(private.parent, pfd) != mount:
                        raise Rejected("installed private STATE is on a different mount")
                    source_fd = private.parent
                    current_store = held.enter_context(held_store(Path(row["store"])))
                    if selected is not None: selected.check_reopened(base.parent, pfd, current_store.fd)
                    original_tree = int(row["stage_dev"]), int(row["stage_ino"])

                def check_parent():
                    if selected is not None: selected.check_registered(state)
                    if private_id is not None:
                        staging.check_private(base)
                        staging.check_named(base.parent, state.name, private.parent)
                        staging.check_private(private)
                        staging.check_mount_name(base.parent, state.name, private.identity, mount)
                        current_store.check_store()
                    if who(parent) != pid: raise Rejected("checkout parent no longer names its original directory")

                def pair(name):
                    check_parent()
                    names = who(stage, dir_fd=source_fd), who(name, dir_fd=pfd)
                    if private_id is not None:
                        first, second = names
                        if first == sid: staging.check_mount_name(source_fd, stage, original_tree, mount)
                        if second == sid: staging.check_mount_name(pfd, name, original_tree, mount)
                    check_parent()
                    return names

                def next_name():
                    source, occupied = pair(dest.name)
                    if source != sid or occupied is None or occupied == sid:
                        raise Rejected("uncertain original stage/target collision")
                    if not row["allocate"] or row["ordinal"] >= MAX_AUTO:
                        raise Rejected("requested checkout name or all automatic candidates are occupied")
                    con.execute("UPDATE opening SET ordinal=ordinal+1 WHERE singleton=1")

                attached = who(state / "worktree.sqlite") is not None
                if attached and selected is not None and selected.resume_mode is not None:
                    raise Rejected("原操作已出现登记库，本窗口不会打开或接管它")
                if attached:
                    # Another attempt already created this original instance. It may since have
                    # been edited or hidden, so don't hash it against the old checkout tree.
                    if private is None: private = held.enter_context(held_candidates(state))
                    def catalog_name():
                        private.check_state()
                        m = os.stat("worktree.sqlite", dir_fd=private.parent, follow_symlinks=False)
                        if not stat.S_ISREG(m.st_mode) or not m.st_ino or m.st_nlink != 1:
                            raise Rejected("unverifiable installed checkout catalog")
                        return m.st_dev, m.st_ino

                    catalog_id = catalog_name()
                    location = _catalog_matches(state, row, dest, parent_fd=pfd)
                    if location is None or catalog_name() != catalog_id:
                        raise Rejected("existing registration has no matching original directory")
                    receipt_name = current = location.name
                    from_stage = False
                else:
                    source, target = pair(dest.name)
                    if source == sid and target is not None:
                        next_name()
                        continue
                    if (source, target) not in ((sid, None), (None, sid)):
                        raise Rejected("neither complete original stage/destination pair is present")
                    from_stage = source == sid
                    current = stage if from_stage else dest.name
                    receipt_name = dest.name
                child, m = held.enter_context(_held_directory(current, sid, parent_fd=source_fd if from_stage else pfd))
                if private_id is not None and staging.mount_id(child) != mount:
                    raise Rejected("original checkout directory is on another mount")
                if selected is not None: selected.verify_recovery_child(child, from_stage)

                if private_id is not None:
                    staging.separate_roots(base.parent, pfd, current_store, mount)
                    # A visible STATE is not durable if its original outer dirsync failed.
                    check_parent()
                    with writing("STATE", state.parent, "复核原签出登记的父目录屏障", operation, base.parent):
                        os.fsync(base.parent)
                    check_parent()

                def checked_root():
                    if pair(receipt_name) != (None, sid):
                        raise Rejected("original checkout directory is not the only installed name")
                    return os.fstat(child)  # This is the identity to insert, not a fresh pathname lookup.

                if not attached:
                    check = None
                    if private_id is not None:
                        staging.member_identity(child, mount)
                        check = lambda _, fd: staging.member_identity(fd, mount)
                    if observe_held(child, (m.st_dev, m.st_ino), require_ids=True, check_held=check).digest() != marker.tree:
                        raise Rejected("original prepared checkout contents changed; nothing adopted")
                    if from_stage:
                        source, target = pair(dest.name)
                        if source != sid: raise Rejected("original stage disappeared before publication")
                        if target is not None:
                            next_name()
                            continue
                        try:
                            if private_id is None:
                                rename_new_at(pfd, stage, dest.name, expected_source=m)
                            else:
                                staging.separate_roots(base.parent, pfd, current_store, mount)
                                rename_new_at(source_fd, stage, dest.name, expected_source=m, target_fd=pfd)
                        except FileExistsError:
                            next_name()
                            continue
                        except OSError as error:
                            note(error, "工作目录父", parent, "不可覆盖安装工作目录；须核原签出结果", operation, pfd)
                            raise
                checked_root()
                if private_id is not None: staging.separate_roots(base.parent, pfd, current_store, mount)
                with writing("工作目录父", parent, "同步工作目录的正式名称；改名可能已生效", operation, pfd):
                    os.fsync(pfd)
                checked_root()
                if private_id is not None and not attached:
                    with writing("STATE", state, "同步原暂存所在目录；工作目录可能已出现", operation, source_fd):
                        os.fsync(source_fd)  # Also required if the rename lost its reply.
                    checked_root()
                if attached:
                    # A prior attempt may have installed the catalog name, then lost its STATE dirsync.
                    if catalog_name() != catalog_id: raise Rejected("installed checkout catalog changed")
                    with writing("STATE", state, "同步已安装的工作树登记", operation, private.parent):
                        os.fsync(private.parent)
                    if catalog_name() != catalog_id: raise Rejected("installed checkout catalog changed")
                else:
                    live = selected.new_session(row, child, private.parent, current_store.fd) if selected is not None else None
                    options = {"live": live} if live is not None else {}
                    with writing("STATE", state, "创建并同步工作树登记；工作目录可能已安装", operation,
                                 private.parent if private is not None else None):
                        try:
                            create_catalog(dest, Path(row["store"]), state, (marker, row["sha"]), row["worktree"],
                                           checked_root=checked_root, **options)
                        except sqlite3.Error as error:
                            note(error, "STATE", state/"worktree.sqlite", "建立工作树登记", operation, database=True)
                            raise
                    if _catalog_matches(state, row, dest, parent_fd=pfd, live=live) != dest:
                        raise Rejected("original directory absent after new catalog publication")
                checked_root()
                con.execute("UPDATE opening SET done=1 WHERE singleton=1")
            checked_root()  # Original parent and child are still held through the SQLite commit.
            if selected is not None:
                # A visible done row is not the final directory barrier for
                # its rollback-journal removal. Keep the actual STATE root.
                with writing("STATE", state, "同步这笔签出的原完成回执；工作目录可能已安装", operation, private.parent):
                    os.fsync(private.parent)
                selected.receipt_synced = True
                checked_root()
            if selected is not None and selected.provisional is not None:
                selected.provisional.check_catalog(state / "worktree.sqlite")
            return dest, marker
