"""Durable native original-action registry. This module has no transport."""
from contextlib import contextmanager
from dataclasses import asdict
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import stat
from threading import local

from ..container_http import ConfirmedAction
from ..container_plan import Action, Plan, ReceiptFence
from ..container_spool import reopen_submit, seal_metadata_submit
from ..model import Rejected, canonical, new_id
from ..private_candidates import held_candidates
from ..private_freeze import local_mount, mount_id
from ..publish import rename_new_at
from .container_client import Original, _matches
from .container_management import (Body, Change, Container, CreateBranch, Outcome, Phase, Recycle,
                                   Rename, Save, Tags, Ticket, branch_ok, id_ok, valid_change, valid_ticket)

_APP = 0x5A584F31
_LIMIT = 2 << 20
_SAFE = getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
_SCHEMA = """
CREATE TABLE owner(destination TEXT PRIMARY KEY, enrollment BLOB NOT NULL) STRICT, WITHOUT ROWID;
CREATE TABLE plans(container TEXT NOT NULL, submission TEXT NOT NULL, digest TEXT NOT NULL,
    ready INTEGER NOT NULL CHECK(ready IN (0,1)), done INTEGER NOT NULL CHECK(done IN (0,1)),
    PRIMARY KEY(container,submission)) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX one_unsettled ON plans(container) WHERE done=0;
CREATE TABLE tickets(container TEXT NOT NULL, submission TEXT NOT NULL, operation TEXT NOT NULL, body BLOB NOT NULL,
    PRIMARY KEY(container,submission,operation),
    FOREIGN KEY(container,submission) REFERENCES plans(container,submission)) STRICT, WITHOUT ROWID;
CREATE TABLE receipts(container TEXT NOT NULL, submission TEXT NOT NULL, position INTEGER NOT NULL CHECK(position>=0),
    record BLOB NOT NULL, PRIMARY KEY(container,submission,position),
    FOREIGN KEY(container,submission) REFERENCES plans(container,submission)) STRICT, WITHOUT ROWID;
"""
_SHAPE = frozenset(q.strip() for q in _SCHEMA.split(";") if q.strip())
_ACCOUNT = re.compile(r"[a-z0-9][a-z0-9._-]{0,63}")
_CHANGE = {Rename: "rename", Tags: "set_tags", Recycle: "delete", Save: "save_node", CreateBranch: "create_branch"}


def _destination(value):
    if type(value) is not str or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise Rejected("invalid original outbox destination")
    return value


def _enrollment(destination, account, state, registry):
    if type(account) is not str or _ACCOUNT.fullmatch(account) is None:
        raise Rejected("invalid selected native outbox principal")
    return canonical({"format": 1, "destination": destination, "account": account,
                      "state": state, "registry": registry})


def _id(value):
    if not id_ok(value):
        raise Rejected("invalid original outbox identity")
    return value


def _stamp(meta):
    if (not stat.S_ISREG(meta.st_mode) or meta.st_nlink != 1 or meta.st_uid != os.geteuid()
            or stat.S_IMODE(meta.st_mode) != 0o600):
        raise Rejected("outbox requires its private original registry")
    return meta.st_dev, meta.st_ino


def _change_bytes(change):
    if not valid_change(change):
        raise Rejected("invalid original container action")
    return canonical({"kind": _CHANGE[type(change)], "change": asdict(change)})


def _change(raw):
    try:
        stored = json.loads(raw)
        fields = dict(stored["change"])
        fields["target"] = Container(**fields["target"])
        kind = stored["kind"]
        if kind == "save_node":
            fields["body"] = Body(**fields["body"])
            change = Save(**fields)
        elif kind == "set_tags":
            fields["values"] = tuple(fields["values"])
            change = Tags(**fields)
        elif kind == "rename":
            change = Rename(**fields)
        elif kind == "delete":
            change = Recycle(**fields)
        elif kind == "create_branch":
            change = CreateBranch(**fields)
        else:
            raise Rejected("unknown local mutation kind")
        if _change_bytes(change) != raw:
            raise Rejected("noncanonical original local action")
        return change
    except (TypeError, ValueError, KeyError) as error:
        raise Rejected("cannot read the original local action") from error


def _metadata_plan(change, operation):
    if type(change) is Save:
        raise Rejected("an absent save must never be recaptured as metadata")
    key = change.target.id
    if type(change) is CreateBranch:
        return Plan((Action(operation, key, "create_branch", change.target.revision, None,
                            branch=change.branch, head=change.source_node),), ())
    return Plan((Action(operation, key, _CHANGE[type(change)], change.target.revision, None,
                        name=change.name if type(change) is Rename else None,
                        tags=tuple(sorted(change.values)) if type(change) is Tags else None),),
                () if type(change) is Recycle else ((key, change.target.name),))


def _digest(plan):
    return hashlib.sha256(plan.encode()).hexdigest()


def _fence_bytes(fence):
    def number(value):
        return value is None or type(value) is int and 0 <= value < (1 << 64) - 1

    if (type(fence) is not ReceiptFence or not id_ok(fence.container_id) or not id_ok(fence.operation_id)
            or fence.kind not in ("save_node", "create_branch", "rename", "set_tags", "delete")
            or not number(fence.base_revision) or not number(fence.sequence)
            or fence.branch is not None and not branch_ok(fence.branch)
            or fence.head is not None and not id_ok(fence.head)):
        raise Rejected("not an exact action receipt")
    return canonical(asdict(fence))


def _fence(raw):
    try:
        fence = ReceiptFence(**json.loads(raw))
        if _fence_bytes(fence) != raw:
            raise Rejected("noncanonical durable receipt")
        return fence
    except (TypeError, ValueError) as error:
        raise Rejected("cannot read the original receipt") from error


class NativeOutbox:
    """Explicitly selected private STATE, one immutable destination.

    No credentials, bodies, connections, or requests are created here. A
    container lease can span the native caller's network exchange; every
    SQLite writer/read transaction ends before it yields back to that caller.
    """

    @classmethod
    def create(cls, state: Path, destination_identity: str, *, account):
        """Issue a fresh registry once; never replace an existing journal."""
        destination = _destination(destination_identity)
        path = Path(state).absolute()
        name = f"outbox-{destination}.sqlite"
        with held_candidates(path) as area:
            # A missing registry with existing submitted intents is an
            # unknown original, not permission to start with empty receipts.
            if any(item.endswith(".pending") or item.startswith(f"outbox-{destination}-")
                   or item == name for item in os.listdir(area.parent)):
                raise Rejected("existing original submits require their unchanged outbox")
            temporary = ".outbox-" + new_id()
            fd = os.open(temporary, os.O_RDWR | os.O_CREAT | os.O_EXCL | _SAFE, 0o600, dir_fd=area.parent)
            try:
                known = _stamp(os.fstat(fd))
            finally:
                os.close(fd)
            enrollment = _enrollment(destination, account, area.identity, known)
            dbfile = path / temporary
            try:
                db = sqlite3.connect(dbfile.as_uri() + "?mode=rw", uri=True, isolation_level=None)
                try:
                    db.execute("PRAGMA journal_mode=DELETE")
                    db.execute("PRAGMA synchronous=FULL")
                    db.executescript("BEGIN IMMEDIATE;" + _SCHEMA
                                     + f"PRAGMA application_id={_APP}; PRAGMA user_version=2;")
                    db.execute("INSERT INTO owner VALUES(?,?)", (destination, enrollment))
                    db.commit()
                finally:
                    db.close()
                area.check_state()
                if _stamp(os.stat(temporary, dir_fd=area.parent, follow_symlinks=False)) != known:
                    raise Rejected("new outbox candidate changed")
                rename_new_at(area.parent, temporary, name)
                os.fsync(area.parent)
                area.check_state()
            except sqlite3.Error as error:
                raise Rejected("cannot establish the original local outbox") from error
        return cls(path, destination, account=account)

    def __init__(self, state: Path, destination_identity: str, *, account):
        self._state = Path(state).absolute()
        self._destination = _destination(destination_identity)
        self._leaf = f"outbox-{self._destination}.sqlite"
        self._held = local()
        self._account = account
        with held_candidates(self._state) as area:
            self._state_identity = area.identity
            self._identity = self._named(area)
            self._enrollment = _enrollment(self._destination, self._account, self._state_identity, self._identity)
            # Confirm visible registry naming after a stopped issuer.
            os.fsync(area.parent)
        self.require_available()

    def require_available(self) -> None:
        """Check the existing original registry without creating a lock or schema."""
        with self._db():
            pass

    @property
    def destination_identity(self):
        return self._destination

    def _named(self, area):
        area.check_state()
        if area.identity != self._state_identity:
            raise Rejected("outbox no longer uses its original private STATE")
        try:
            return _stamp(os.stat(self._leaf, dir_fd=area.parent, follow_symlinks=False))
        except FileNotFoundError as error:
            raise Rejected("missing original outbox; never issue another operation") from error

    @contextmanager
    def _db(self, *, write=False):
        uri = (self._state / self._leaf).as_uri() + "?mode=rw"
        with held_candidates(self._state) as area:
            if self._named(area) != self._identity:
                raise Rejected("outbox registry was replaced")
            db = None
            try:
                db = sqlite3.connect(uri, uri=True, isolation_level=None)
                db.setlimit(sqlite3.SQLITE_LIMIT_LENGTH, _LIMIT)
                db.execute("PRAGMA trusted_schema=OFF")
                db.execute("PRAGMA foreign_keys=ON")
                db.execute("PRAGMA synchronous=FULL")
                if (db.execute("PRAGMA journal_mode").fetchone() != ("delete",)
                        or db.execute("PRAGMA application_id").fetchone() != (_APP,)
                        or db.execute("PRAGMA user_version").fetchone() != (2,)
                        or frozenset(r[0] for r in db.execute(
                            "SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL")) != _SHAPE):
                    raise Rejected("unknown exact outbox registry format")
                db.execute("BEGIN IMMEDIATE" if write else "BEGIN")
                if db.execute("SELECT destination,enrollment FROM owner").fetchall() != [
                    (self._destination, self._enrollment)
                ]:
                    raise Rejected("outbox was not enrolled for this actual STATE, registry and principal")
                yield db
                if self._named(area) != self._identity:
                    raise Rejected("outbox registry changed during local decision")
                db.commit()
            except sqlite3.Error as error:
                raise Rejected("cannot establish the exact original local outbox state") from error
            finally:
                if db is not None:
                    db.close()

    def prepared_original_ticket(self, container, submission, operation):
        """Read one exact current, already armed original; never create a lock or ticket."""
        submission, operation = _id(submission), _id(operation)
        with self.hold(container, existing=True):
            outcome = self._pending_held(container)
            if (outcome is None or outcome.ticket.submission != submission
                    or outcome.ticket.operation != operation):
                raise Rejected("no such exact current prepared original ticket")
            ticket = outcome.ticket
            self.require_prepared(Original(submission, operation), ticket.change)
            return ticket

    @contextmanager
    def hold(self, container: str, *, existing: bool = False):
        import fcntl
        key = _id(container)
        if type(existing) is not bool:
            raise Rejected("invalid original lock selection")
        if getattr(self._held, "container", None) is not None:
            raise Rejected("cannot replace an active original container lease")
        name = f"outbox-{self._destination}-{key}.lock"
        with held_candidates(self._state) as area:
            if self._named(area) != self._identity:
                raise Rejected("outbox registry changed before opening its container lease")
            mount = local_mount(area.parent)
            flags = os.O_RDWR | _SAFE | os.O_NONBLOCK
            fd = os.open(name, flags | (0 if existing else os.O_CREAT), 0o600, dir_fd=area.parent)
            try:
                if (_stamp(os.fstat(fd)) != _stamp(os.stat(name, dir_fd=area.parent, follow_symlinks=False))
                        or mount_id(fd) != mount):
                    raise Rejected("not the original private container lease")
                try:
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError as error:
                    raise Rejected("another window holds the original container action") from error
                if self._named(area) != self._identity:
                    raise Rejected("outbox registry changed before the container lease")
                stamp = area.check_file(name, fd)
                self._held.container = key
                self._held.lease = area, name, fd, stamp
                try:
                    yield
                finally:
                    self._held.container = None
                    self._held.lease = None
                    area.check_file(name, fd, stamp)
            finally:
                os.close(fd)

    def _require(self, container):
        if getattr(self._held, "container", None) != _id(container):
            raise Rejected("no held original container action lease")
        area, name, fd, stamp = self._held.lease
        area.check_file(name, fd, stamp)

    def _row(self, db, container, submission=None):
        if submission is None:
            return db.execute("SELECT submission,digest,ready,done FROM plans WHERE container=? AND done=0", (container,)).fetchone()
        return db.execute("SELECT submission,digest,ready,done FROM plans WHERE container=? AND submission=?",
                          (container, _id(submission))).fetchone()

    def _resolved(self, submission, digest, *, allow_missing=False):
        try:
            with reopen_submit(self._state, submission, destination=self._destination) as pending:
                if _digest(pending.plan) != digest:
                    raise Rejected("no exact original sealed plan")
                return pending.plan
        except FileNotFoundError as error:
            if allow_missing:
                raise
            raise Rejected("original sealed plan missing; do not recreate or reissue") from error

    def _prefix(self, db, submission, container, plan):
        rows = db.execute("SELECT position,record FROM receipts WHERE container=? AND submission=? ORDER BY position",
                          (container, submission)).fetchall()
        values = []
        for position, blob in rows:
            if position != len(values) or type(blob) is not bytes or len(blob) > _LIMIT:
                raise Rejected("missing or reordered original receipt prefix")
            values.append(_fence(blob))
        fences = tuple(values)
        plan.next_for(container, fences)
        return fences

    def bind_submission(self, container: str, submission: str):
        """Register this container queue from an already frozen Submit.

        A native capture owner may call this under its exact container
        lease. Nothing is sent; no other container queue is settled here.
        """
        self._require(container)
        with reopen_submit(self._state, _id(submission), destination=self._destination) as pending:
            if pending.plan.next_for(container) is None:
                raise Rejected("this container has no original action in the submitted plan")
            digest = _digest(pending.plan)
        with self._db(write=True) as db:
            if self._row(db, container) not in (None, (submission, digest, 1, 0)):
                raise Rejected("this container has a different unresolved original")
            known = self._row(db, container, submission)
            if known is None:
                db.execute("INSERT INTO plans VALUES(?,?,?,1,0)", (container, submission, digest))
            elif known != (submission, digest, 1, 0):
                raise Rejected("sealed queue changed or already completed")
        return digest

    def _resume_metadata(self, key, row, change):
        submission, digest, ready, done = row
        if ready or done:
            raise Rejected("a selected original spool cannot be recreated")
        with self._db() as db:
            saved = db.execute("SELECT operation,body FROM tickets WHERE container=? AND submission=?",
                               (key, submission)).fetchall()
        if len(saved) != 1 or saved[0][1] != _change_bytes(change):
            raise Rejected("a different local action is still pending")
        op = _id(saved[0][0])
        plan = _metadata_plan(change, op)
        if _digest(plan) != digest:
            raise Rejected("unarmed metadata plan changed")
        try:
            self._resolved(submission, digest, allow_missing=True)
        except FileNotFoundError:
            # Safe only while READY=0. Until this original plan is fsync'ed
            # and the SQLite bit advances, no caller has received a ticket.
            seal_metadata_submit(self._state, submission, self._destination, plan)
            self._resolved(submission, digest)
        with self._db(write=True) as db:
            if self._row(db, key, submission) != row:
                raise Rejected("the unarmed original metadata changed before sealing")
            db.execute("UPDATE plans SET ready=1 WHERE container=? AND submission=?", (key, submission))

    def reserve(self, change: Change) -> Original:
        raw = _change_bytes(change)
        key = change.target.id
        self._require(key)
        with self._db() as db:
            row = self._row(db, key)
        if row is None and type(change) is Save:
            submission = _id(change.body.frozen)
            digest = self.bind_submission(key, submission)
            row = (submission, digest, 1, 0)
        elif row is None:
            submission, operation = new_id(), new_id()
            digest = _digest(_metadata_plan(change, operation))
            row = (submission, digest, 0, 0)
            # Persist original identities before the separate spool rename.
            with self._db(write=True) as db:
                if self._row(db, key) is not None:
                    raise Rejected("the original container acquired a different action")
                db.execute("INSERT INTO plans VALUES(?,?,?,0,0)", (key, submission, digest))
                db.execute("INSERT INTO tickets VALUES(?,?,?,?)", (key, submission, operation, raw))
        if not row[2]:
            self._resume_metadata(key, row, change)
            row = (row[0], row[1], 1, 0)
        submission, digest, _ready, _done = row
        plan = self._resolved(submission, digest)
        with self._db(write=True) as db:
            if self._row(db, key, submission) != row:
                raise Rejected("durable original container selection changed")
            current = plan.next_for(key, self._prefix(db, submission, key, plan))
            if current is None:
                raise Rejected("this original queue is already settled")
            action, base = current
            original = Original(submission, action.operation_id)
            if not _matches(action, base, original, change):
                raise Rejected("this is not the next original action for this container")
            ticket = db.execute("SELECT body FROM tickets WHERE container=? AND submission=? AND operation=?",
                                (key, submission, action.operation_id)).fetchone()
            if ticket is None:
                db.execute("INSERT INTO tickets VALUES(?,?,?,?)", (key, submission, action.operation_id, raw))
            elif ticket != (raw,):
                raise Rejected("the original prepared action has a different body")
        return original

    def require_prepared(self, original: Original, change: Change, *,
                         settled: bool = False) -> None:
        """Prove one durable ticket; settled selects readback-only admission."""
        if type(original) is not Original or type(settled) is not bool:
            raise Rejected("missing exact original outbox ticket")
        submission, operation = _id(original.submission), _id(original.operation)
        raw = _change_bytes(change)
        key = change.target.id
        self._require(key)
        with self._db() as db:
            row = self._row(db, key, submission)
        if row is None or row[2] != 1:
            raise Rejected("no armed original submission")
        plan = self._resolved(submission, row[1])
        with self._db() as db:
            if self._row(db, key, submission) != row:
                raise Rejected("original queue changed before transport")
            prefix = self._prefix(db, submission, key, plan)
            current = plan.next_for(key, prefix)
            if bool(row[3]) != (current is None):
                raise Rejected("completion bit differs from the durable prefix")
            if settled:
                index = next((i for i, fence in enumerate(prefix) if fence.operation_id == operation), None)
                current = None if index is None else plan.next_for(key, prefix[:index])
            if current is None or current[0].operation_id != operation or not _matches(*current, original, change):
                raise Rejected("not the requested original action for this container")
            token = db.execute("SELECT body FROM tickets WHERE container=? AND submission=? AND operation=?",
                               (key, submission, operation)).fetchone()
            if token != (raw,):
                raise Rejected("original action was not durably prepared with this exact body")

    def prefix(self, submission: str, container: str) -> tuple[ReceiptFence, ...]:
        self._require(container)
        with self._db() as db:
            row = self._row(db, container, submission)
        if row is None or row[2] != 1:
            raise Rejected("missing original armed submission registry")
        plan = self._resolved(submission, row[1])
        with self._db() as db:
            if self._row(db, container, submission) != row:
                raise Rejected("original queue registry changed")
            prefix = self._prefix(db, submission, container, plan)
            if bool(row[3]) != (plan.next_for(container, prefix) is None):
                raise Rejected("completion bit differs from the durable prefix")
            return prefix

    def record(self, original: Original, confirmation: ConfirmedAction) -> None:
        if type(original) is not Original or type(confirmation) is not ConfirmedAction:
            raise Rejected("missing exact original action confirmation")
        fence = confirmation.fence
        raw = _fence_bytes(fence)
        key, operation = _id(fence.container_id), _id(fence.operation_id)
        self._require(key)
        if operation != original.operation:
            raise Rejected("durable receipt belongs to a different original operation")
        cleanup = confirmation.recycle_source_cleanup_pending
        if (cleanup is not None and type(cleanup) is not bool) or (fence.kind != "delete" and cleanup is not None):
            raise Rejected("receipt reports an unrelated cleanup state")
        with self._db() as db:
            row = self._row(db, key, original.submission)
        if row is None or row[2] != 1:
            raise Rejected("original receipt has no retained armed queue")
        plan = self._resolved(original.submission, row[1])
        with self._db(write=True) as db:
            if self._row(db, key, original.submission) != row:
                raise Rejected("original receipt queue changed")
            prefix = self._prefix(db, original.submission, key, plan)
            if bool(row[3]) != (plan.next_for(key, prefix) is None):
                raise Rejected("completion bit differs from the durable prefix")
            replay = next((r for r in prefix if r.operation_id == operation), None)
            if replay is not None:
                if replay != fence:
                    raise Rejected("the original receipt has a different durable outcome")
                return
            current = plan.next_for(key, prefix)
            if current is None or current[0].operation_id != operation:
                raise Rejected("receipt is not the next original queue operation")
            token = db.execute("SELECT body FROM tickets WHERE container=? AND submission=? AND operation=?",
                               (key, original.submission, operation)).fetchone()
            if token is None or not _matches(current[0], current[1], original, _change(token[0])):
                raise Rejected("receipt has no exact locally prepared action")
            advanced = plan.next_for(key, prefix + (fence,))
            db.execute("INSERT INTO receipts VALUES(?,?,?,?)", (key, original.submission, len(prefix), raw))
            if advanced is None:
                db.execute("UPDATE plans SET done=1 WHERE container=? AND submission=?", (key, original.submission))

    def next_registered_save(self, container):
        """Read an armed SaveNode from the registered exact spool, without reserving a ticket."""
        container = _id(container)
        with self._db() as db:
            row = self._row(db, container)
        if row is None:
            return None
        with self.hold(container, existing=True):
            submission, digest, ready, done = row
            if ready != 1 or done:
                raise Rejected("the original submission is not armed")
            plan = self._resolved(submission, digest)
            with self._db() as db:
                if self._row(db, container) != row:
                    raise Rejected("the original submission changed during selection")
                prior = self._prefix(db, submission, container, plan)
            step = plan.next_for(container, prior)
            if step is None:
                raise Rejected("the original submission completion is unresolved")
            action, base = step
            if action.kind != "save_node":
                return None
            return submission, action, base

    def first_registered_saves(self):
        """Original unconfirmed first-main candidates, including retained recovery tickets."""
        with self._db() as db:
            keys = tuple(container for (container,) in db.execute(
                "SELECT container FROM plans WHERE ready=1 AND done=0 ORDER BY container"))
        rows, complete = [], True
        for container in keys:
            try:
                with self.hold(container, existing=True):
                    row = self._row_snapshot(container)
                    if row is None:
                        continue  # independently confirmed meanwhile
                    submission, digest, ready, done = row
                    if ready != 1 or done:
                        raise Rejected("original first-node queue not armed")
                    plan = self._resolved(submission, digest)
                    with self._db() as db:
                        if self._row(db, container) != row:
                            raise Rejected("original first-node queue changed")
                        step = plan.next_for(container, self._prefix(db, submission, container, plan))
                    if step is None:
                        raise Rejected("unsettled original queue has no next action")
                    action, base = step
                    if base != 0:
                        continue
                    if (action.kind != "save_node" or action.name is None or action.branch != "main"
                            or action.head is not None or action.fork_from_main is not None):
                        raise Rejected("invalid first-main original queue")
                    rows.append((submission, action, self._pending_held(container)))
            except (Rejected, OSError):
                complete = False
        return tuple(rows), complete

    def _row_snapshot(self, container):
        with self._db() as db:
            return self._row(db, container)

    def pending(self, container: str) -> Outcome | None:
        with self.hold(container):
            return self._pending_held(container)

    def pending_recycles(self, live: frozenset[str]) -> tuple[tuple[Outcome, ...], bool]:
        """Discover retained armed recycle tickets only; no lock, intent or request is created."""
        if type(live) is not frozenset or not all(map(id_ok, live)):
            raise Rejected("invalid observed live container index")
        with self._db() as db:
            keys = tuple(container for (container,) in db.execute(
                "SELECT p.container FROM plans AS p WHERE p.ready=1 AND p.done=0 "
                "AND EXISTS (SELECT 1 FROM tickets AS t WHERE t.container=p.container "
                "AND t.submission=p.submission) ORDER BY p.container") if container not in live)
        pending, complete = [], True
        for key in keys:
            try:
                with self.hold(key, existing=True):
                    result = self._pending_held(key)
                    if result is not None and type(result.ticket.change) is Recycle:
                        pending.append(result)
            except (Rejected, OSError):
                complete = False  # no unverified row is projected for this original
        return tuple(pending), complete

    def _pending_held(self, container: str) -> Outcome | None:
        self._require(container)
        with self._db() as db:
            row = self._row(db, container)
        if row is None:
            return None
        submission, digest, ready, _done = row
        if not ready:
            raise Rejected("local original exists but was not armed for sending")
        plan = self._resolved(submission, digest)
        with self._db() as db:
            if self._row(db, container, submission) != row:
                raise Rejected("original pending record changed")
            prefix = self._prefix(db, submission, container, plan)
        next_ = plan.next_for(container, prefix)
        if next_ is None:
            raise Rejected("original queue completion is unresolved")
        action, base = next_
        with self._db() as db:
            record = db.execute("SELECT body FROM tickets WHERE container=? AND submission=? AND operation=?",
                                (container, submission, action.operation_id)).fetchone()
        if record is None:
            # No local ticket for this dependency: the UI must still
            # explicitly select it; it is not silently armed on reopen.
            return None
        original = Original(submission, action.operation_id)
        change = _change(record[0])
        ticket = Ticket(change, original.operation, original.submission, base)
        if not valid_ticket(ticket) or not _matches(action, base, original, change):
            raise Rejected("the retained ticket differs from its original plan")
        return Outcome(ticket, Phase.PENDING)
