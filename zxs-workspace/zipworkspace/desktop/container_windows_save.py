"""Closed Windows handoff of one original R3 SaveNode. No ZIP, database, mount or automatic send."""
from contextlib import ExitStack, contextmanager
from dataclasses import asdict, dataclass
from hashlib import sha256
import json
import re
from threading import Lock

from .._windows_access import OutputProtection
from .._windows_node import _check_origin, _effective_zone, _remote_mark, _same_file
from .._windows_output import PinnedParent, _NTError
from ..container_https import HttpsExchange
from ..container_plan import Action, FullBody, Plan
from ..model import CHUNK, Rejected, canonical, new_id
from ..publish import Uncertain
from .container_connection import OpenConnection
from .container_management import Container, Node, _container, _node, branch_ok, id_ok
from .container_windows_draft import DraftSource, _require_reader

_LIMIT = 16 << 10
_HEX = re.compile(r"[0-9a-f]{64}")
_DRAFT = re.compile(r"[0-9a-f]{32}\.zwd")


@dataclass(frozen=True, slots=True)
class WindowsSaveKey:
    """Keep these four values apart from the pending files; the digest pins the complete local record."""
    container: str
    submission: str
    operation: str
    record_sha256: str

    def __post_init__(self):
        if (not all(map(id_ok, (self.container, self.submission, self.operation)))
                or type(self.record_sha256) is not str or _HEX.fullmatch(self.record_sha256) is None):
            raise Rejected("invalid original Windows transfer key")


class WindowsSaveUncertain(Uncertain):
    def __init__(self, key, path):
        self.key = key
        super().__init__(f"Original Windows intent may already be selected at {path!r}; "
                         f"keep this exact key, do not recapture: {key!r}")


def _slot(destination, account, container):
    # Disjoint targets/accounts; one no-replace slot for competing prepares of a single container.
    return sha256(canonical(["windows-r3-save", destination, account, container])).hexdigest() + ".zwi"


def _record(source, plan, submission, home, body_id, record_id, zone):
    return canonical({"format": "windows-r3-save/1", "source": asdict(source), "submission": submission,
                      "plan": plan.encode().decode("utf-8"), "source_zone": zone,
                      "folder": _folder(home),
                      "body_file_id": body_id.hex(), "intent_file_id": record_id.hex()})


def _new_record(keep, home, leaf):
    home.check()
    protection = OutputProtection(home.handle)
    h = keep.enter_context(home.api.new_file(home.handle, leaf))
    row = _same_file(home, h, leaf, 0)
    protection.check(h)
    home.api.only_default_stream(h, 0)
    return h, row.file_id, protection


def _read_record(home, h, leaf, file_id, protection):
    protection.check(h)
    row = home.api.ordinary(h, home.volume, home.name(leaf))
    if row.file_id != file_id or not 0 < row.size <= _LIMIT:
        raise Rejected("invalid original Windows submit record identity or length")
    home.api.only_default_stream(h, row.size)
    home.api.rewind(h)
    raw = home.api.read(h, row.size)
    if (len(raw) != row.size or home.api.read(h, 1)
            or _same_file(home, h, leaf, row.size, file_id) != row):
        raise Rejected("short or changed Windows submit record")
    return raw


def _folder(home):
    return [home.guid, home.volume, home.chain[-1][1].hex()]


def _key_slot(destination, account, container):
    return _slot(destination, account, container).removesuffix(".zwi") + ".zwk"


def _key_record(key, directory, file_id, destination, origin, account):
    return canonical({"format": "windows-r3-key/1", "key": asdict(key), "destination": destination,
                      "origin": origin, "account": account, "folder": _folder(directory),
                      "key_file_id": file_id.hex()})


def _write_key(keep, directory, key, source, check):
    name = _key_slot(source.destination, source.account, key.container)
    temp = new_id() + ".zwt"
    api = directory.api
    directory.check()
    api.absent(directory.handle, name)
    api.flush(directory.handle)
    check()
    h, fid, access = _new_record(keep, directory, temp)
    raw = _key_record(key, directory, fid, source.destination, source.origin, source.account)
    api.write(h, raw)
    api.flush(h)
    if _read_record(directory, h, temp, fid, access) != raw:
        raise Rejected("original transfer key readback failed; no submit intent was selected")
    directory.check()
    check()
    # Losing this call or the key-parent barrier prohibits publishing the body intent.
    api.rename_new(h, directory.handle, name)
    api.flush(directory.handle)
    if _read_record(directory, h, name, fid, access) != raw:
        raise Rejected("original recovery key could not be confirmed; no submit intent was selected")
    api.absent(directory.handle, temp)
    directory.check()
    check()

    def original_key():
        if _read_record(directory, h, name, fid, access) != raw:
            raise Rejected("the selected original transfer key no longer matches")
    return original_key


def load_original_windows_key(connection, private_parent, container, *, confirmed=False):
    """Read one original key from the chosen private STATE, not by searching for surviving files.

    Like an original supplied from outside STATE, this is evidence of a reserved operation identity,
    not evidence its body or final intent exists. No HTTP, recovery mutation or new ID.
    """
    if (confirmed is not True or type(connection) is not OpenConnection
            or type(connection._exchange) is not HttpsExchange or not id_ok(container)):
        raise Rejected("confirm the original private NTFS directory, authenticated reader and exact container")
    reader, h2, auth = connection.operations, connection._exchange, connection._authorization
    destination, origin, account = h2.destination_identity, h2._location, connection._account
    def check():
        _require_reader(connection, reader, h2, auth, destination, origin, account)
    check()
    with PinnedParent(private_parent, output=True) as directory:
        protection = OutputProtection(directory.handle)
        name = _key_slot(destination, account, container)
        with directory.api.inspected_file(directory.handle, name) as h:
            fid = directory.api.ordinary(h, directory.volume, directory.name(name)).file_id
            raw = _read_record(directory, h, name, fid, protection)
            try:
                key = WindowsSaveKey(**json.loads(raw)["key"])
            except (ValueError, TypeError, KeyError, UnicodeError, RecursionError) as error:
                raise Rejected("invalid Windows recovery key") from error
            if key.container != container or _key_record(key, directory, fid, destination, origin, account) != raw:
                raise Rejected("the Windows recovery record is not the original authenticated selection")
            directory.check()
            protection.check(h)
            check()
            return key


def _capture_review(draft, review):
    # Called only inside the draft lock, after final context release and explicit full review.
    # No mutable draft handle escapes. Publish the small intent LAST, after the separate
    # plain complete body is flushed, hashed by readback and visible in its original directory.
    home, s = draft._home, review.source
    check, api = draft._choice._live, home.api
    head, owner = s.head, s.container
    operation, node, submission = new_id(), new_id(), new_id()
    action = Action(operation, owner.id, "save_node", owner.revision, None,
                    body=FullBody(review.length, review.sha256), branch=s.branch, head=head.id, node_id=node)
    plan = Plan((action,), ((owner.id, draft._leaf),))  # Local generated leaf only. No remote rename.
    slot, temp, body_leaf = _slot(s.destination, s.account, owner.id), new_id() + ".zwt", submission + ".zvb"
    key, possible, material = None, False, False
    try:
        with ExitStack() as keep:
            if draft._observe(review.length, policy=True) != draft._review_row:
                raise Rejected("the held Windows draft changed after review")
            try:
                api.absent(home.handle, slot)
                api.absent(home.handle, _key_slot(s.destination, s.account, owner.id))
            except FileExistsError as error:
                raise Rejected("another original Windows transfer occupies this container slot") from error
            api.flush(home.handle)
            check()
            material = True  # A failed create/open response need not mean no private name exists.
            body, body_id, body_acl = _new_record(keep, home, body_leaf)
            intent, intent_id, intent_acl = _new_record(keep, home, temp)
            raw = _record(s, plan, submission, home, body_id, intent_id, draft._zone)
            if len(raw) > _LIMIT:
                raise Rejected("Windows submit intent too large")
            key = WindowsSaveKey(owner.id, submission, operation, sha256(raw).hexdigest())
            api.write(intent, raw)
            api.flush(intent)
            if _read_record(home, intent, temp, intent_id, intent_acl) != raw:
                raise Rejected("Windows submit record failed readback")
            original_key = _write_key(keep, home, key, s, check)  # Before any payload or pending rename.
            mark = draft._mark
            if mark is not None:
                ads = keep.enter_context(api.new_zone(body))
                api.write(ads, mark)
                api.flush(ads)
                _check_origin(api, body, _same_file(home, body, body_leaf, 0, body_id), mark)
            api.rewind(draft._h)
            left, digest = review.length, sha256()
            while left:
                check()
                part = api.read(draft._h, min(CHUNK, left))
                if not part or len(part) > left:
                    raise Rejected("the held draft did not supply the reviewed full body")
                api.write(body, part)
                digest.update(part)
                left -= len(part)
            if (api.read(draft._h, 1) or digest.hexdigest() != review.sha256
                    or draft._observe(review.length, policy=True) != draft._review_row):
                raise Rejected("Windows capture differed from the reviewed original")
            api.flush(body)
            sealed = _same_file(home, body, body_leaf, review.length, body_id)
            api.verify_bytes(body, review.length, review.sha256, checkpoint=check)
            body_acl.check(body)
            _check_origin(api, body, sealed, mark)
            api.flush(home.handle)
            home.check()
            original_key()
            body_acl.check(body)
            if (_same_file(home, body, body_leaf, review.length, body_id) != sealed
                    or _effective_zone(draft._url) != draft._zone):
                raise Rejected("original frozen Windows body or source policy changed")
            _check_origin(api, body, sealed, mark)
            check()
            possible = True
            try:
                api.rename_new(intent, home.handle, slot)
            except _NTError as error:
                if error.code == 0xc0000035:  # Never replace another window's intent.
                    _same_file(home, intent, temp, len(raw), intent_id)
                    possible = False
                    raise Rejected("another original Windows transfer occupies this container slot") from error
                raise
            api.flush(home.handle)
            if _read_record(home, intent, slot, intent_id, intent_acl) != raw:
                raise Rejected("cannot confirm the selected Windows original submit record")
            api.absent(home.handle, temp)
            home.check()
            original_key()
            check()
        return key
    except Exception as error:
        if possible:
            raise WindowsSaveUncertain(key, home.name(slot)) from error
        if material:
            raise Rejected(f"No confirmed Windows pending selection; keep the original private directory "
                           f"and any recovery key, body {home.name(body_leaf)!r} or intent "
                           f"{home.name(temp)!r}; do not recapture. {error}") from error
        raise


def _decode(raw, key, home, record_id, connection):
    if sha256(raw).hexdigest() != key.record_sha256:
        raise Rejected("this is not the independently pinned original Windows submit record")
    try:
        value = json.loads(raw)
        info = value["source"]
        s = DraftSource(info["destination"], info["origin"], info["account"], Container(**info["container"]),
                        info["branch"], Node(**info["head"]))
        plan = Plan.decode(value["plan"].encode("utf-8"))
        (action,), ((cid, leaf),) = plan.actions, plan.placements
        body_id, zone = bytes.fromhex(value["body_file_id"]), value["source_zone"]
        if (not _container(s.container) or cid != key.container or cid != s.container.id
                or not s.container.revision or not _node(s.head) or not branch_ok(s.branch)
                or _DRAFT.fullmatch(leaf) is None or action.body is None or action.body.length >= 1 << 63
                or s.head.length >= 1 << 63 or len(body_id) != 16 or not any(body_id) or body_id == record_id
                or value["submission"] != key.submission
                or (zone is not None and (type(zone) is not int or zone not in range(5)))
                or action != Action(key.operation, cid, "save_node", s.container.revision, None, body=action.body,
                                    branch=s.branch, head=s.head.id, node_id=action.node_id)
                or _record(s, plan, key.submission, home, body_id, record_id, zone) != raw):
            raise Rejected("not an exact existing-branch Windows save intent")
        _require_reader(connection, connection.operations, connection._exchange, connection._authorization,
                        s.destination, s.origin, s.account)
        return s, plan, body_id, zone
    except (TypeError, KeyError, UnicodeError, ValueError, AttributeError, RecursionError) as error:
        raise Rejected("cannot bind the original Windows save metadata and authenticated reader") from error


class _HeldWindowsSave:
    def __init__(self, plan, source, api, body, observe):
        self._plan, self._destination = plan, source.destination
        self._api, self._body, self._observe = api, body, observe
        self._lock, self._active, self._live = Lock(), None, True

    @property
    def plan(self):
        return self._plan

    @property
    def destination(self):
        return self._destination

    def _check(self, *, policy=False):
        if not self._live:
            raise Rejected("the original Windows save read authority has closed")
        self._observe(policy=policy)

    def _close(self):
        # Serialize revocation with every original-handle operation. A yielded generator
        # never keeps the lock, cannot read a closed/reused HANDLE and cannot steal the offset.
        with self._lock:
            self._live, self._active = False, None

    @contextmanager
    def body(self, action):
        scope = object()
        with self._lock:
            if action is not self._plan.actions[0] or self._active is not None:
                raise Rejected("not the only reader of this original whole-file operation")
            self._check(policy=True)
            self._api.rewind(self._body)
            self._active = scope

        def blocks():
            rest, digest = action.body.length, sha256()
            while True:
                with self._lock:
                    if self._active is not scope:
                        raise Rejected("the original Windows save body scope has ended")
                    self._check()
                    if not rest:
                        if self._api.read(self._body, 1) or digest.hexdigest() != action.body.sha256:
                            raise Rejected("the held sealed Windows bytes differ from the original intent")
                        break
                    block = self._api.read(self._body, min(CHUNK, rest))
                    if not block or len(block) > rest:
                        raise Rejected("short original Windows save body")
                    rest -= len(block)
                    digest.update(block)
                yield block
        try:
            yield blocks()
            with self._lock:
                self._check(policy=True)
        finally:
            with self._lock:
                if self._active is scope:
                    self._active = None


@contextmanager
def open_original_windows_save(connection, private_parent, key, *, confirmed=False):
    """Recover the exact private plan+body (no request or new IDs). Owner runs a single worker.

    Input key must be the original independently retained selection, never derived from an arbitrary
    pending filename. Hold existing read-only files with share zero throughout verification and use.
    """
    if (confirmed is not True or type(key) is not WindowsSaveKey or type(connection) is not OpenConnection
            or type(connection._exchange) is not HttpsExchange):
        raise Rejected("confirm the selected private directory, exact original key and authenticated reader")
    reader, h2, auth = connection.operations, connection._exchange, connection._authorization
    destination, origin, account = h2.destination_identity, h2._location, connection._account
    def live():
        _require_reader(connection, reader, h2, auth, destination, origin, account)
    live()
    with PinnedParent(private_parent, output=True) as home, ExitStack() as keep:
        api = home.api
        slot, body_leaf = _slot(destination, account, key.container), key.submission + ".zvb"
        intent = keep.enter_context(api.inspected_file(home.handle, slot))
        intent_acl = OutputProtection(home.handle)
        original = api.ordinary(intent, home.volume, home.name(slot))
        raw = _read_record(home, intent, slot, original.file_id, intent_acl)
        source, plan, body_id, zone = _decode(raw, key, home, original.file_id, connection)
        action = plan.actions[0]
        url = origin + h2._node_target(key.container, source.head.id, source.head.length, source.head.sha256)
        if _effective_zone(url) != zone:
            raise Rejected("the original Windows transfer source policy changed")
        mark = _remote_mark(url, zone)
        held = keep.enter_context(api.inspected_file(home.handle, body_leaf))
        body_acl = OutputProtection(home.handle)
        sealed = _same_file(home, held, body_leaf, action.body.length, body_id)

        def observe(*, policy=False):
            live()
            if policy and _effective_zone(url) != zone:
                raise Rejected("original Windows transfer source policy changed")
            home.check()
            intent_acl.check(intent)
            body_acl.check(held)
            if (_same_file(home, intent, slot, len(raw), original.file_id) != original
                    or _same_file(home, held, body_leaf, action.body.length, body_id) != sealed):
                raise Rejected("original Windows transfer record or sealed body changed")
            _check_origin(api, held, sealed, mark)
        observe()
        api.verify_bytes(held, action.body.length, action.body.sha256, checkpoint=live)
        api.flush(home.handle)  # Also closes a lost final same-parent rename barrier.
        observe(policy=True)
        pending = _HeldWindowsSave(plan, source, api, held, observe)
        try:
            yield pending
        finally:
            pending._close()
