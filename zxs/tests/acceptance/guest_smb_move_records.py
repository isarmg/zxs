"""Independently observe the one original v21 SYSTEM for the disposable case."""

from collections import defaultdict
from contextlib import contextmanager
from pathlib import Path
import os
import sqlite3
import stat

from guest_smb_move_media import selected

SOURCE = b"Source"
TARGET = b"Target"
NAME = b"zxs-normal-move.bin"
ROOT = bytes(16)
APPLICATION = 0x5A585331
SCHEMA = 21
DB = "SYSTEM.sqlite3"
QUERY = Path(__file__).with_name("rust_smb_cross_f_receipt.sql")


def require(check):
    if not check:
        raise RuntimeError("original SYSTEM did not prove the scoped MOVE state")


def identity(raw, width=16):
    return isinstance(raw, bytes) and len(raw) == width and any(raw)


def one(collection):
    require(len(collection) == 1)
    return collection[0]


@contextmanager
def original_system():
    parent = selected("system")
    fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        def current():
            return os.stat(DB, dir_fd=fd, follow_symlinks=False)

        original = current()
        held = os.fstat(fd)
        require(stat.S_ISREG(original.st_mode) and stat.S_IMODE(original.st_mode) == 0o600
                and original.st_nlink == 1 and original.st_dev == held.st_dev
                and original.st_uid == held.st_uid and original.st_gid == held.st_gid)
        # The named read-only SQLite connection must see the actual rollback
        # journal, not a copied DB, immutable URI or /proc/self/fd alias.
        connection = sqlite3.connect((parent / DB).as_uri() + "?mode=ro", uri=True,
                                     isolation_level=None, timeout=1.0)
        try:
            connection.execute("PRAGMA query_only=ON")
            connection.execute("PRAGMA trusted_schema=OFF")
            require(os.path.samestat(held, parent.stat(follow_symlinks=False))
                    and os.path.samestat(original, current()))
            yield connection
            require(os.path.samestat(held, parent.stat(follow_symlinks=False))
                    and os.path.samestat(original, current()))
        finally:
            connection.close()
    finally:
        os.close(fd)


def identify_original():
    # Done after creation on F1, before any destination operation. Never
    # reconstruct this input from the name installed at F2.
    with original_system() as db:
        db.execute("BEGIN")
        require(db.execute("PRAGMA main.application_id").fetchall() == [(APPLICATION,)]
                and db.execute("PRAGMA main.user_version").fetchall() == [(SCHEMA,)])
        file, = one(db.execute(
            "SELECT l.file_id FROM main.file_location l"
            " JOIN main.root_share r ON r.volume_id=l.volume_id"
            " JOIN main.operation o ON o.op_id=l.position_op"
            " WHERE r.display_label=?1 AND l.display=?2 AND l.parent_id=?3"
            " AND l.role=2 AND l.generation=1 AND l.backing_op IS NULL AND o.kind=5 AND o.state=1",
            (SOURCE, NAME, ROOT)).fetchall())
        require(identity(file))
        db.execute("COMMIT")
        return file


def snapshot(file):
    require(identity(file))
    facts = defaultdict(list)
    pragmas = []
    statement = ""
    inputs = {"file": file, "from_label": SOURCE, "to_label": TARGET}
    with original_system() as db:
        for line in QUERY.read_text(encoding="ascii").splitlines(keepends=True):
            statement += line
            if not sqlite3.complete_statement(statement):
                continue
            rows = db.execute(statement, inputs).fetchall()
            for row in rows:
                if len(row) == 1 and isinstance(row[0], int):
                    pragmas.append(row[0])
                else:
                    facts[row[0]].append(row[1:])
            statement = ""
        require(not statement.strip() and pragmas == [APPLICATION, SCHEMA])
    require({"pool", "account", "folder", "birth", "location", "work", "recorded-work", "node"}
            <= set(facts) <= {"pool", "account", "folder", "birth", "location", "work", "recorded-work", "node", "move"})
    return facts


def current_binding(facts, file):
    pool, init, active, digest = one(facts["pool"])
    account, credential, epoch, web, smb = one(facts["account"])
    require(active == 1 and epoch == 1 and identity(digest, 32)
            and all(identity(v) for v in (pool, init, account, credential, web, smb))
            and len({account, web, smb}) == 3)
    folders = {row[0]: row[1:] for row in facts["folder"]}
    require(len(facts["folder"]) == 2 and set(folders) == {SOURCE, TARGET})
    for volume, share, setting, auth, actor in folders.values():
        require(all(identity(x) for x in (volume, share, setting)) and (auth, actor) == (credential, smb))
    left, right = folders[SOURCE], folders[TARGET]
    require(left[0] != right[0] and left[1] != right[1] and left[2] != right[2])
    birth = one(facts["birth"])
    require(birth[0] == file and all(identity(x) for x in birth[:5]) and birth[5:] == (5, 1))
    position = one(facts["location"])
    require(position[0] == file and position[4:7] == (2, NAME, ROOT))
    chosen = one(facts["work"])
    require(chosen[0] == file and all(identity(v) for v in chosen[:3]) and chosen[5] == 4096)
    return pool, left, right, position, chosen, web, smb, credential


def before_move(facts, file):
    pool, left, right, location, current, web, smb, credential = current_binding(facts, file)
    require(location[1] == one(facts["birth"])[1]
            and location[2] == 1 and location[3] == left[0] and location[7:] == (None, 5, 1)
            and not facts["move"])
    edits, nodes = facts["recorded-work"], facts["node"]
    require(len(edits) == len(nodes) == 2)
    for index, (edit, node) in enumerate(zip(edits, nodes)):
        require(edit[0] == file and all(identity(v) for v in edit[:3]) and identity(edit[12], 32)
                and edit[3] == index + 1 and edit[5] == 4096 and edit[8:12] == (10, 1, smb, credential))
        require(node[0] == index + 1 and all(identity(v) for v in node[1:4])
                and node[4] == (None if index == 0 else nodes[0][3]) and node[5:8] == (edit[1], edit[2], edit[4])
                and node[8:12] == (credential, 11, 1, web) and identity(node[12], 32))
    require(len({x[1] for x in nodes}) == len({x[2] for x in nodes}) == len({x[1] for x in edits}) == 2
            and current == edits[1][:8])
    return pool, left[1], right[1], tuple(row[1] for row in nodes)


def after_move(previous, facts, file, case, observed_index):
    require(case in ("stock", "original") and observed_index == file[:8].hex())
    pool, left, right, old, old_work, web, smb, credential = current_binding(previous, file)
    new = current_binding(facts, file)
    require(new[:3] == (pool, left, right) and new[5:] == (web, smb, credential))
    location, work = new[3:5]
    require(facts["account"] == previous["account"] and facts["pool"] == previous["pool"]
            and facts["birth"] == previous["birth"] and facts["node"] == previous["node"])
    edits = facts["recorded-work"]
    old_edits = previous["recorded-work"]
    if case == "stock":
        require(edits == old_edits and work == old_work)
    else:
        require(len(edits) == 3 and edits[:2] == old_edits)
        last = edits[2]
        require(last[0] == file and identity(last[1]) and identity(last[2]) and last[3] == old_work[3] + 1
                and last[5] == 4096 and last[8:12] == (10, 1, smb, credential)
                and identity(last[12], 32) and work == last[:8])
    move = one(facts["move"])
    require(identity(move[0]) and move[1:15] ==
            (file, 3, old[1], old[2], left[0], NAME, right[0], NAME, ROOT, ROOT, left[2], right[2],
             work[1], previous["node"][-1][3])
            and move[15:18] == (12, 1, smb) and identity(move[18], 32)
            and move[19:22] == (1, 0, None) and move[22] in (0, 1)
            and 84 <= move[24] <= 299019)
    require(location == (file, move[0], old[2] + 1, right[0], 2, NAME, ROOT, move[0], 12, 1))
    return move[0], "retired" if move[22] == 1 else "pending"
