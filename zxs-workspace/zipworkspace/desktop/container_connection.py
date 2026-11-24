"""Native credentials and typed R3 metadata for one user-chosen origin."""

from base64 import b64encode
from dataclasses import dataclass, replace
import re
import sys
from threading import Lock
from typing import Callable

from ..container_https import HttpsExchange
from ..container_plan import display_name_ok as _catalog_name
from ..model import Rejected
from ..private_candidates import CandidateDirectory, held_candidates
from ..private_freeze import local_mount
from .container_client import BoundOutbox, ContainerClient, index_for
from .container_management import (Body, Branch, Container, Detail, Kind, Node, branch_ok, id_ok,
                                   list_checked, number, text_ok)

_DETAIL_KEYS = frozenset(("container_id", "sequence", "name", "tags", "heads", "next_after"))
_DETAIL_HEAD = frozenset(("branch", "head"))
_DETAIL_PAGE_ROWS = 256
_CATALOG_KEYS = frozenset(("items", "next_after"))
_CATALOG_ROW = frozenset(("container_id", "sequence", "name"))
_CATALOG_PAGE_ROWS = 256
_NODE = frozenset(("node_id", "operation_id", "parent_node", "original_branch", "sequence", "length", "sha256"))
_HISTORY_KEYS = frozenset(("head", "nodes", "next"))
_HISTORY_PAGE_ROWS = 256
_LOGIN = re.compile(r"[a-z0-9][a-z0-9._-]{0,63}")
_HASH = re.compile(r"[0-9a-f]{64}")


class _Authorization:
    """An in-memory native supplier, never handed to the view or saved as a preference."""

    def __init__(self, login, password):
        if type(login) is not str or _LOGIN.fullmatch(login) is None or type(password) is not str:
            raise Rejected("invalid native account")
        credential = password.encode("utf-8")
        if not 1 <= len(credential) <= 1024 or any(value < 32 or value == 127 for value in credential):
            raise Rejected("invalid native password")
        self._lock = Lock()
        self._login = login  # Non-secret identity bound to this exact credential, not a UI label.
        self._header = bytearray(b"Basic " + b64encode(login.encode("ascii") + b":" + credential))

    def __call__(self):
        with self._lock:
            if not self._header:
                raise Rejected("this window disconnected")
            return self._header.decode("ascii")

    def require_live(self):
        with self._lock:
            if not self._header:
                raise Rejected("this window disconnected")

    def require_account(self, login):
        self.require_live()
        if login != self._login:
            raise Rejected("not the account of this original credential")

    def revoke(self):
        with self._lock:
            self._header[:] = bytes(len(self._header))
            self._header.clear()


class NativeWriteProfile:
    """Only an authorized native owner can pass its still-held private STATE here."""

    def __init__(self, destination_identity: str, account: str, state: CandidateDirectory,
                 allowed: frozenset[Kind], *, require_owner: Callable[[], None],
                 prepared: Callable[[Detail], Body | None] | None = None):
        if (type(destination_identity) is not str or _HASH.fullmatch(destination_identity) is None
                or type(account) is not str or _LOGIN.fullmatch(account) is None
                or type(state) is not CandidateDirectory or type(allowed) is not frozenset
                or not all(type(kind) is Kind for kind in allowed) or not callable(require_owner)
                or (prepared is not None and not callable(prepared))):
            raise Rejected("invalid native writable selection")
        self._identity, self._account, self._state = destination_identity, account, state
        self._state_selection = state.path, state.parent, state.identity
        self._allowed, self._prepared, self._require_owner = allowed, prepared, require_owner
        self._lock, self._attached, self._refused = Lock(), None, False

    def require_state(self):
        self._require_owner()
        path, original, identity = self._state_selection
        if (self._state.path, self._state.parent, self._state.identity) != self._state_selection:
            raise Rejected("native STATE selection changed")
        self._state.check_state()
        mount = local_mount(original)
        with held_candidates(path) as named:
            if named.identity != identity or local_mount(named.parent) != mount:
                raise Rejected("private STATE no longer names this original held Linux mount")
        self._require_owner()

    def bind(self, destination, account):
        if sys.platform != "linux" or (destination, account) != (self._identity, self._account) or not self._allowed:
            return None
        with self._lock:
            if self._refused:
                return None
            try:
                from fcntl import flock  # gate the real Linux locking primitive without a test lockfile
                if not callable(flock):
                    raise Rejected("original per-container locking is unavailable")
                self.require_state()
                if self._attached is None:
                    from .container_outbox import NativeOutbox
                    registry = NativeOutbox(self._state.path, self._identity, account=self._account)  # existing only
                    self.require_state()
                    self._attached = BoundOutbox(self._state.path, registry, self.require_state)
                else:
                    self._attached.registry.require_available()
                    self.require_state()
                return self._attached
            except (Rejected, OSError, ImportError):
                self._refused = True  # a native owner must explicitly supply a new approved profile
                return None

    def present(self, read: Detail) -> Detail:
        self.require_state()
        body = self._prepared(read) if Kind.SAVE in self._allowed and self._prepared is not None else None
        if body is not None and (type(body) is not Body or body.container != read.container.id):
            raise Rejected("prepared body belongs to a different container")
        self.require_state()
        return replace(read, allowed=self._allowed if body is not None else self._allowed - {Kind.SAVE}, body=body)


@dataclass(frozen=True, slots=True)
class OpenConnection:
    operations: ContainerClient
    _authorization: _Authorization
    _exchange: HttpsExchange
    _account: str
    writable: bool = False
    _listed: bool = False

    def _local(self, profile):
        outbox = profile.bind(self._exchange.destination_identity, self._account)
        self._authorization.require_live()
        if outbox is None:
            return None
        approved = ContainerClient(_Metadata(self._exchange, profile), self._exchange, outbox=outbox)
        return replace(self, operations=approved, writable=True)

    def establish(self, profile: NativeWriteProfile | None):
        items = list_checked(self.operations)
        self._authorization.require_live()
        listed = replace(self, _listed=True)
        chosen = listed._local(profile) if type(profile) is NativeWriteProfile else None
        active = chosen if chosen is not None else listed
        return active, index_for(active.operations, items)

    def adopt_local(self, profile: NativeWriteProfile, *, confirmed=False):
        """Attach only this already authenticated connection; this does no request."""
        if (confirmed is not True or type(profile) is not NativeWriteProfile
                or not self._listed or self.writable):
            raise Rejected("an already authenticated original reader and explicit local profile are required")
        self._authorization.require_live()
        active = self._local(profile)
        if active is None:
            raise Rejected("the original local profile is not available for this connection")
        return active

    def revoke(self):
        self._authorization.revoke()


def open_connection(origin, login, password):
    """Prepare in memory only. The caller explicitly requests the first read."""
    authority = _Authorization(login, password)
    try:
        exchange = HttpsExchange(origin, authorization=authority)
        return OpenConnection(ContainerClient(_Metadata(exchange), exchange), authority, exchange, login)
    except Exception:
        authority.revoke()
        raise


def _detail_page(raw, container, after=None):
    if (type(raw) is not dict or raw.keys() != _DETAIL_KEYS
            or raw["container_id"] != container or not id_ok(container)
            or not _catalog_name(raw["name"])
            or not number(raw["sequence"]) or raw["sequence"] == 0
            or type(raw["tags"]) is not list or not all(map(text_ok, raw["tags"]))
            or type(raw["heads"]) is not list or len(raw["heads"]) > _DETAIL_PAGE_ROWS):
        raise Rejected("invalid R3 container detail page")
    owner = Container(container, raw["name"], raw["sequence"])
    heads, previous = [], after
    for row in raw["heads"]:
        if (type(row) is not dict or row.keys() != _DETAIL_HEAD
                or not branch_ok(row["branch"]) or not id_ok(row["head"])
                or (previous is not None and row["branch"] <= previous)):
            raise Rejected("invalid or unordered R3 branch heads")
        previous = row["branch"]
        heads.append((previous, row["head"]))
    required = previous if len(heads) == _DETAIL_PAGE_ROWS else None
    if raw["next_after"] != required:
        raise Rejected("invalid R3 detail continuation")
    return owner, tuple(raw["tags"]), tuple(heads), required


def _catalog_page(raw, after):
    if (type(raw) is not dict or raw.keys() != _CATALOG_KEYS
            or type(raw["items"]) is not list or len(raw["items"]) > _CATALOG_PAGE_ROWS):
        raise Rejected("invalid R3 catalog page")
    rows, previous = [], after
    for row in raw["items"]:
        if (type(row) is not dict or row.keys() != _CATALOG_ROW or not id_ok(row["container_id"])
                or (previous is not None and row["container_id"] <= previous)
                or not _catalog_name(row["name"])
                or not number(row["sequence"]) or row["sequence"] == 0):
            raise Rejected("invalid or unordered R3 catalog row")
        previous = row["container_id"]
        rows.append(Container(previous, row["name"], row["sequence"]))
    required = previous if len(rows) == _CATALOG_PAGE_ROWS else None
    if raw["next_after"] != required:
        raise Rejected("invalid R3 catalog continuation")
    return rows, required


class _History:
    """Validate the entire anchored chain before returning a branch view."""

    def __init__(self, branch, head, sequence):
        self.branch, self.head = branch, head
        self.expected, self.ceiling = head, sequence + 1
        self.seen, self.views = set(), []

    def accept(self, raw):
        if (self.expected is None or type(raw) is not dict or raw.keys() != _HISTORY_KEYS
                or raw["head"] != self.head or type(raw["nodes"]) is not list
                or not 1 <= len(raw["nodes"]) <= _HISTORY_PAGE_ROWS):
            raise Rejected("invalid R3 branch history page")
        rows = raw["nodes"]
        for row in rows:
            self._accept_node(row)
        if (raw["next"] != self.expected
                or (self.expected is not None
                    and (len(rows) != _HISTORY_PAGE_ROWS or self.expected in self.seen))):
            raise Rejected("invalid R3 branch history continuation")

    def _accept_node(self, row):
        if (type(row) is not dict or row.keys() != _NODE or not id_ok(row["node_id"])
                or row["node_id"] != self.expected or row["node_id"] in self.seen
                or not id_ok(row["operation_id"]) or not branch_ok(row["original_branch"])
                or not number(row["sequence"]) or not 0 < row["sequence"] < self.ceiling
                or not number(row["length"]) or type(row["sha256"]) is not str
                or _HASH.fullmatch(row["sha256"]) is None
                or (row["parent_node"] is not None and not id_ok(row["parent_node"]))):
            raise Rejected("inconsistent R3 ancestry for the selected branch")
        self.seen.add(row["node_id"])
        self.views.append(Node(row["node_id"], row["parent_node"], row["length"], row["sha256"]))
        self.expected, self.ceiling = row["parent_node"], row["sequence"]

    def finish(self):
        if self.expected is not None or not self.views:
            raise Rejected("truncated R3 branch ancestry")
        return Branch(self.branch, self.head, tuple(self.views))


class _Metadata:
    """Read-only projection; never downloads content, selects a path, or grants mutations."""

    def __init__(self, exchange, profile=None):
        self._exchange, self._profile = exchange, profile

    @property
    def destination_identity(self):
        return self._exchange.destination_identity

    def _read_detail(self, container, sequence=None, after=None):
        if self._profile is not None:
            self._profile.require_state()
        raw = self._exchange.read_container_detail_page(container, sequence, after)
        if self._profile is not None:
            self._profile.require_state()
        return _detail_page(raw, container, after)

    def list_containers(self):
        rows, after = [], None
        while True:
            if self._profile is not None:
                self._profile.require_state()
            raw = self._exchange.read_catalog_page(after)
            if self._profile is not None:
                self._profile.require_state()
            page, after = _catalog_page(raw, after)
            rows.extend(page)
            if after is None:
                return tuple(rows)

    def _branch_history(self, container, branch, head, sequence):
        history, next_node = _History(branch, head, sequence), None
        while True:
            if self._profile is not None:
                self._profile.require_state()
            raw = self._exchange.read_history_page(container, branch, head, next_node)
            if self._profile is not None:
                self._profile.require_state()
            history.accept(raw)
            next_node = history.expected
            if next_node is None:
                return history.finish()

    def open_container(self, container):
        if not id_ok(container):
            raise Rejected("invalid selected container")
        first = self._read_detail(container)
        owner, tags, initial_heads, after = first
        heads = list(initial_heads)
        while after is not None:
            page_owner, page_tags, page_heads, after = self._read_detail(container, owner.revision, after)
            if (page_owner, page_tags) != (owner, tags):
                raise Rejected("container metadata changed while reading branch heads")
            heads.extend(page_heads)
        if not any(branch == "main" for branch, _ in heads):
            raise Rejected("missing original main branch")
        views = tuple(self._branch_history(container, branch, head, owner.revision) for branch, head in heads)
        if self._read_detail(container, owner.revision) != first:
            raise Rejected("container metadata changed while reading the selected branches")
        read = Detail(owner, tags, views)
        return read if self._profile is None else self._profile.present(read)
