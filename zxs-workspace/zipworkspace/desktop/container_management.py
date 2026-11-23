"""Typed, transport-free boundary for the embedded Workspace container page."""

from dataclasses import dataclass, replace
from enum import Enum
from html import escape
import json
import re
from typing import Protocol

_HEX_ID = re.compile(r"[0-9a-f]{32}")
_SHA = re.compile(r"[0-9a-f]{64}")
_MAX = (1 << 64) - 1


def id_ok(value):
    return type(value) is str and _HEX_ID.fullmatch(value) is not None and value != "0" * 32


def branch_ok(value):
    return value == "main" or id_ok(value)


def number(value):
    return type(value) is int and 0 <= value <= _MAX


def text_ok(value):
    if type(value) is not str or not value or "\x00" in value:
        return False
    try:
        value.encode("utf-8")
    except UnicodeError:
        return False
    return True


def tags_ok(tags):
    return (type(tags) is tuple and all(map(text_ok, tags))
            and len(json.dumps({"tags": tags}, ensure_ascii=False,
                               separators=(",", ":")).encode("utf-8")) <= 1 << 20)


@dataclass(frozen=True, slots=True)
class Container:
    id: str
    name: str
    revision: int


@dataclass(frozen=True, slots=True)
class Node:
    id: str
    parent: str | None
    length: int
    sha256: str


@dataclass(frozen=True, slots=True)
class Branch:
    id: str
    head: str | None
    nodes: tuple[Node, ...]


@dataclass(frozen=True, slots=True)
class Body:
    """Selected native frozen body of one container, not a path or an entire virtual disk."""
    container: str
    frozen: str
    branch: str
    selected_base: str | None
    length: int
    sha256: str
    target_branch: str
    node: str


class Kind(Enum):
    RENAME = "重命名"
    TAGS = "替换标签"
    RECYCLE = "删除到回收盘"
    SAVE = "保存新节点"
    CREATE_BRANCH = "新建分支"


@dataclass(frozen=True, slots=True)
class Rename:
    target: Container
    name: str


@dataclass(frozen=True, slots=True)
class Tags:
    target: Container
    values: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class Recycle:
    target: Container


@dataclass(frozen=True, slots=True)
class Save:
    target: Container
    body: Body
    branch: str
    from_main: str | None = None  # If present, fork and first complete save are one operation.


@dataclass(frozen=True, slots=True)
class CreateBranch:
    target: Container
    source_branch: str
    source_node: str
    branch: str


Change = Rename | Tags | Recycle | Save | CreateBranch
_KINDS = {Rename: Kind.RENAME, Tags: Kind.TAGS, Recycle: Kind.RECYCLE,
          Save: Kind.SAVE, CreateBranch: Kind.CREATE_BRANCH}


@dataclass(frozen=True, slots=True)
class Ticket:
    """Prepared and durably identified by the authorized native client before network send."""
    change: Change
    operation: str
    submission: str
    base_sequence: int


class Phase(Enum):
    PENDING = "原操作待决"
    UNKNOWN = "结果未知"
    ABSENT = "原回执尚不存在"
    CONFLICT = "原操作冲突"
    CONFIRMED = "已确认"


@dataclass(frozen=True, slots=True)
class Receipt:
    # Construction belongs to the client after verification of the original result.
    ticket: Ticket
    revision: int | None
    branch: str | None = None
    node: str | None = None
    parent: str | None = None
    length: int | None = None
    sha256: str | None = None
    recycled: bool = False
    recycle_source_cleanup_pending: bool | None = None


@dataclass(frozen=True, slots=True)
class Outcome:
    ticket: Ticket
    phase: Phase
    receipt: Receipt | None = None


@dataclass(frozen=True, slots=True)
class Detail:
    container: Container
    tags: tuple[str, ...]
    branches: tuple[Branch, ...]
    allowed: frozenset[Kind] = frozenset()
    body: Body | None = None
    original: Outcome | None = None


class NativeOperations(Protocol):
    """One authorized native client; recheck rights at every action. No browser credentials."""

    def list_containers(self) -> tuple[Container, ...]: ...
    def open_container(self, container: str) -> Detail:
        """Return actual metadata and any unresolved original from the durable local journal."""
        ...

    def prepare(self, change: Change) -> Ticket:
        """Recheck selection and bind the actual journaled original; do not send."""
        ...

    def apply(self, ticket: Ticket) -> Outcome:
        """Use this exact ticket; unrelated containers may run concurrently."""
        ...

    def status(self, ticket: Ticket) -> Outcome:
        """Read only this original result; never issue a write or choose another operation."""
        ...


def _digest(value):
    return type(value) is str and _SHA.fullmatch(value) is not None


def _container(value):
    return (type(value) is Container and id_ok(value.id) and text_ok(value.name)
            and number(value.revision))


def list_checked(ops):
    items = ops.list_containers()
    if (type(items) is not tuple or not all(map(_container, items))
            or len({item.id for item in items}) != len(items)):
        raise ValueError("unexpected container list")
    return items


def _node(value):
    return (type(value) is Node and id_ok(value.id) and (value.parent is None or id_ok(value.parent))
            and value.parent != value.id and number(value.length) and _digest(value.sha256))


def _branch(value):
    if (type(value) is not Branch or not branch_ok(value.id) or type(value.nodes) is not tuple
            or not all(map(_node, value.nodes))):
        return False
    if value.head is None:
        return value.id == "main" and not value.nodes
    ids = {node.id for node in value.nodes}
    return len(ids) == len(value.nodes) and id_ok(value.head) and value.head in ids


def _body(value, container):
    return (type(value) is Body and value.container == container and id_ok(value.frozen)
            and branch_ok(value.branch) and (value.selected_base is None or id_ok(value.selected_base))
            and number(value.length) and _digest(value.sha256)
            and branch_ok(value.target_branch) and id_ok(value.node) and value.node != value.selected_base)


def valid_change(value):
    if type(value) not in _KINDS or not _container(value.target):
        return False
    if type(value) is Rename:
        return value.target.revision != 0 and text_ok(value.name)
    if type(value) is Tags:
        return (value.target.revision != 0 and tags_ok(value.values)
                and len(set(value.values)) == len(value.values))
    if type(value) is CreateBranch:
        return (value.target.revision != 0 and branch_ok(value.source_branch)
                and id_ok(value.source_node) and id_ok(value.branch) and value.branch != value.source_branch)
    if type(value) is Save:
        if not _body(value.body, value.target.id) or value.branch != value.body.target_branch:
            return False
        if value.target.revision == 0:
            return value.from_main is None and value.body.selected_base is None and value.branch == value.body.branch == "main"
        if value.from_main is None:
            return value.branch == value.body.branch and id_ok(value.body.selected_base)
        return (value.body.branch == "main" and id_ok(value.branch)
                and value.from_main == value.body.selected_base and id_ok(value.from_main))
    return value.target.revision != 0


def valid_ticket(value):
    if (type(value) is not Ticket or not id_ok(value.operation) or not id_ok(value.submission)
            or not number(value.base_sequence) or value.base_sequence == _MAX or not valid_change(value.change)):
        return False
    change = value.change
    if type(change) is Save:
        # Existing SaveNode targets a branch/head, not an unrelated metadata CAS.
        return (value.submission == change.body.frozen
                and (value.base_sequence == 0) == (change.target.revision == 0))
    return value.base_sequence == change.target.revision


def kind(change):
    return _KINDS[type(change)]


def _outcome(value, ticket):
    if (not valid_ticket(ticket) or type(value) is not Outcome or value.ticket != ticket
            or type(value.phase) is not Phase):
        return False
    result = value.receipt
    if value.phase is not Phase.CONFIRMED:
        return result is None
    if type(result) is not Receipt or result.ticket != ticket:
        return False
    change = ticket.change
    empty = all(field is None for field in (result.branch, result.node, result.parent, result.length, result.sha256))
    cleanup = result.recycle_source_cleanup_pending
    if type(change) is Recycle:
        return (result.recycled is True and result.revision is None and empty
                and (cleanup is None or type(cleanup) is bool))
    if (result.recycled is not False or cleanup is not None or not number(result.revision)
            or not ticket.base_sequence < result.revision < _MAX):
        return False
    if type(change) is CreateBranch:
        return (result.branch == change.branch and result.node == change.source_node
                and result.parent is None and result.length is None and result.sha256 is None)
    if type(change) is Save:
        return (result.node == change.body.node and result.branch == change.branch
                and result.parent == change.body.selected_base and number(result.length)
                and result.length == change.body.length and result.sha256 == change.body.sha256)
    return empty


def open_checked(ops, container):
    info = ops.open_container(container)
    if (type(info) is not Detail or not _container(info.container) or info.container.id != container
            or not tags_ok(info.tags) or type(info.branches) is not tuple
            or not all(map(_branch, info.branches)) or type(info.allowed) is not frozenset
            or not all(type(k) is Kind for k in info.allowed)):
        raise ValueError("unexpected container detail")
    branches = {branch.id: branch for branch in info.branches}
    if ("main" not in branches or len(branches) != len(info.branches)
            or (branches["main"].head is None) != (info.container.revision == 0)
            or (info.container.revision == 0 and len(branches) != 1)):
        raise ValueError("missing, duplicate or inconsistent original branch")
    if info.body is not None:
        if not _body(info.body, container):
            raise ValueError("unexpected frozen body")
        branch = branches.get(info.body.branch)
        if branch is None or (info.body.selected_base != branch.head and (
            branch.id != "main" or not any(node.id == info.body.selected_base for node in branch.nodes)
        )):
            info = replace(info, body=None)  # read-only information remains usable
    if info.original is not None:
        if (type(info.original) is not Outcome or not _outcome(info.original, info.original.ticket)
                or info.original.ticket.change.target.id != container):
            raise ValueError("unexpected original receipt")
    return info


def unresolved(result):
    return result is not None and result.phase in (Phase.PENDING, Phase.UNKNOWN, Phase.ABSENT)


def perform(ops, change):
    if not valid_change(change):
        raise ValueError("invalid selected change")
    ticket = ops.prepare(change)
    if not valid_ticket(ticket) or ticket.change != change:
        raise ValueError("unbound local operation")
    return _observe(ops.apply, ticket)


def lookup(ops, ticket):
    if not valid_ticket(ticket):
        raise ValueError("invalid original operation")
    return _observe(ops.status, ticket)


def retry(ops, prior, *, confirmed=False):
    """Explicitly reattempt only the exact unresolved ticket after receipt absence."""
    if (confirmed is not True or type(prior) is not Outcome
            or prior.phase is not Phase.ABSENT or prior.receipt is not None
            or not valid_ticket(prior.ticket)):
        raise ValueError("select and confirm one exact original operation with no receipt")
    # Recheck under the original per-container lease; it may have settled
    # after the first GET. Ambiguous replies never allow any send.
    return _observe(ops.apply, prior.ticket)


def _observe(method, ticket):
    try:
        value = method(ticket)
        if _outcome(value, ticket):
            return value
    except Exception:
        pass  # a lost reply is not evidence of failure or success
    return Outcome(ticket, Phase.UNKNOWN)


def branch_source(info, source, node):
    return (info.container.revision != 0 and Kind.CREATE_BRANCH in info.allowed
            and source in info.branches and id_ok(node) and any(version.id == node for version in source.nodes))


def branch_choice(info, source, node, new_branch):
    if (branch_source(info, source, node) and id_ok(new_branch)
            and all(item.id != new_branch for item in info.branches)):
        return CreateBranch(info.container, source.id, node, new_branch)
    return None


def save_mode(info, branch, node):
    body = info.body
    if (Kind.SAVE not in info.allowed or body is None or branch not in info.branches
            or body.branch != branch.id or body.selected_base != node):
        return None
    if node == branch.head and body.target_branch == branch.id:
        return "append"
    if (branch.id == "main" and id_ok(node) and node != branch.head and id_ok(body.target_branch)
            and any(item.id == node for item in branch.nodes)
            and all(item.id != body.target_branch for item in info.branches)):
        return "fork"
    return None


def save_choice(info, branch, node):
    mode = save_mode(info, branch, node)
    if mode is not None:
        return Save(info.container, info.body, info.body.target_branch, node if mode == "fork" else None)
    return None


def page(title, summary, rows=()):
    """Local HTML body only: escape text, no links, scripts, or external resources."""
    grid = "".join("<tr><th align='left'>" + escape(key) + "</th><td>" + escape(value)
                   + "</td></tr>" for key, value in rows)
    card = ("<table width='100%' cellpadding='8' cellspacing='0' border='0'>" + grid + "</table>") if grid else ""
    return "<h2>" + escape(title) + "</h2><p>" + escape(summary) + "</p>" + card
