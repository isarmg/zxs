"""Native credentials and typed R3 metadata for one user-chosen origin."""

from base64 import b64encode
from dataclasses import dataclass, replace
import re
import sys
from threading import Lock
from typing import Callable

from ..container_https import HttpsExchange
from ..model import Rejected
from ..private_candidates import CandidateDirectory, held_candidates
from ..private_freeze import local_mount
from .container_client import BoundOutbox, ContainerClient, index_for
from .container_management import (Body, Branch, Container, Detail, Kind, Node, branch_ok, id_ok,
                                   list_checked, number, text_ok)

_ROOT = "/zxs/v1/containers"
_CONTAINER = frozenset(("container_id", "sequence", "name", "tags", "heads"))
_CATALOG_KEYS = frozenset(("items", "next_after"))
_CATALOG_ROW = frozenset(("container_id", "sequence", "name"))
_CATALOG_PAGE_ROWS = 256
_NODE = frozenset(("node_id", "operation_id", "parent_node", "original_branch", "sequence", "length", "sha256"))
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


def _container(raw):
    if (type(raw) is not dict or raw.keys() != _CONTAINER
            or not id_ok(raw["container_id"]) or not text_ok(raw["name"])
            or not number(raw["sequence"]) or raw["sequence"] == 0
            or type(raw["tags"]) is not list or not all(map(text_ok, raw["tags"]))
            or type(raw["heads"]) is not dict or "main" not in raw["heads"]
            or any(not branch_ok(branch) or not id_ok(head) for branch, head in raw["heads"].items())):
        raise Rejected("invalid R3 container metadata")
    return (Container(raw["container_id"], raw["name"], raw["sequence"]),
            tuple(raw["tags"]), tuple(sorted(raw["heads"].items())))


def _catalog_name(name):
    return (text_ok(name) and len(name.encode("utf-8")) <= 255 and name not in (".", "..")
            and not name.endswith((".", " "))
            and not any(ord(ch) < 32 or 127 <= ord(ch) <= 159 or ch in '<>:"/\\|?*' for ch in name))


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


def _nodes(raw, branch, head, sequence):
    if type(raw) is not list or not raw:
        raise Rejected("no exact R3 branch ancestry")
    expected, ceiling = head, sequence + 1
    seen, views = set(), []
    for row in raw:
        if (type(row) is not dict or row.keys() != _NODE or not id_ok(row["node_id"])
                or row["node_id"] != expected or row["node_id"] in seen
                or not id_ok(row["operation_id"]) or not branch_ok(row["original_branch"])
                or not number(row["sequence"]) or not 0 < row["sequence"] < ceiling
                or not number(row["length"]) or type(row["sha256"]) is not str
                or _HASH.fullmatch(row["sha256"]) is None
                or (row["parent_node"] is not None and not id_ok(row["parent_node"]))):
            raise Rejected("inconsistent R3 ancestry for the selected branch")
        seen.add(row["node_id"])
        views.append(Node(row["node_id"], row["parent_node"], row["length"], row["sha256"]))
        expected, ceiling = row["parent_node"], row["sequence"]
    if expected is not None:
        raise Rejected("truncated R3 branch ancestry")
    return Branch(branch, head, tuple(views))


class _Metadata:
    """Read-only projection; never downloads content, selects a path, or grants mutations."""

    def __init__(self, exchange, profile=None):
        self._exchange, self._profile = exchange, profile

    @property
    def destination_identity(self):
        return self._exchange.destination_identity

    def _read(self, target):
        if self._profile is not None:
            self._profile.require_state()
        result = self._exchange.read_metadata(target)
        if self._profile is not None:
            self._profile.require_state()
        return result

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

    def open_container(self, container):
        if not id_ok(container):
            raise Rejected("invalid selected container")
        target = f"{_ROOT}/{container}"
        observed = _container(self._read(target))
        owner, tags, heads = observed
        if owner.id != container:
            raise Rejected("metadata was not for the selected container")
        views = tuple(_nodes(self._read(f"{target}/branches/{branch}/nodes"),
                             branch, head, owner.revision) for branch, head in heads)
        # Branch lists come from separate requests. A deleted container or
        # changed sequence/head set cannot be returned as the same selection.
        if _container(self._read(target)) != observed:
            raise Rejected("container metadata changed while reading the selected branches")
        read = Detail(owner, tags, views)
        return read if self._profile is None else self._profile.present(read)
