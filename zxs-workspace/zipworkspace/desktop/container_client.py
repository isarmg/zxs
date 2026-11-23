"""Bind desktop actions to the existing sealed per-container R3 client."""

from contextlib import AbstractContextManager
from dataclasses import dataclass, replace
from pathlib import Path
import re
from typing import Callable, Protocol

from ..container_http import AuthenticatedExchange, ConfirmedAction, SubmitAbsent, SubmitConflict, invoke_original
from ..container_plan import FullBody, ReceiptFence
from ..container_spool import reopen_submit
from ..model import Rejected
from .container_management import (Change, Container, CreateBranch, Detail, Outcome, Phase, Receipt,
                                   Recycle, Rename, Save, Tags, Ticket, branch_choice, id_ok, list_checked,
                                   open_checked, valid_change, valid_ticket)


@dataclass(frozen=True, slots=True)
class Original:
    submission: str
    operation: str


@dataclass(frozen=True, slots=True)
class _OriginalExchange:
    exchange: AuthenticatedExchange
    require_original: Callable[[], None]

    @property
    def destination_identity(self):
        self.require_original()
        return self.exchange.destination_identity

    def request(self, message):
        # The receipt GET and the possible mutation use separate connections.
        # Losing the owner during GET cannot authorize the following send.
        self.require_original()
        result = self.exchange.request(message)
        self.require_original()
        return result


class NativeMetadata(Protocol):
    """Reads owned by one authenticated selection."""

    destination_identity: str

    def list_containers(self) -> tuple[Container, ...]: ...
    def open_container(self, container: str) -> Detail:
        """Verified metadata plus any exact prepared body and unresolved original."""
        ...


class PendingRegistry(Protocol):
    """A native owner of the durable per-container original outbox."""

    destination_identity: str

    def require_available(self) -> None: ...
    def pending(self, container: str) -> Outcome | None: ...
    def hold(self, container: str) -> AbstractContextManager[None]:
        """Exclusive for this container across windows, not a cross-container upload lock."""
        ...

    def reserve(self, change: Change) -> Original:
        """Bind one explicit action to its durable pending spool and native selection.

        Never send here. A saved body must already have its immutable Plan. Do
        not allocate another operation if the original is still unconfirmed.
        """
        ...

    def require_prepared(self, original: Original, change: Change, *, settled: bool = False) -> None:
        """Validate a retained exact ticket inside its already held per-container lease."""
        ...

    def prefix(self, submission: str, container: str) -> tuple[ReceiptFence, ...]:
        """The durable, independently verified prefix of this original container queue."""
        ...

    def record(self, original: Original, confirmation: ConfirmedAction) -> None:
        """Durably record this match; recording the same receipt again is safe."""
        ...


@dataclass(frozen=True, slots=True)
class BoundOutbox:
    state: Path
    registry: PendingRegistry
    require_original: Callable[[], None]


@dataclass(frozen=True, slots=True)
class ContainerIndex:
    live: tuple[Container, ...]
    pending_recycles: tuple[Outcome, ...] = ()
    local_complete: bool = True


def index_for(operations, live=None):
    live = list_checked(operations) if live is None else live
    if type(operations) is ContainerClient:
        return operations.recovery_index(live)
    return ContainerIndex(live)


class ContainerClient:
    """One authorized native connection; mutable capability requires an explicit original outbox."""

    def __init__(self, selected: NativeMetadata, exchange: AuthenticatedExchange, *, outbox: BoundOutbox | None = None):
        destination = exchange.destination_identity
        if (type(destination) is not str or re.fullmatch(r"[0-9a-f]{64}", destination) is None
                or selected.destination_identity != destination
                or (outbox is not None and outbox.registry.destination_identity != destination)):
            raise Rejected("unbound authenticated Workspace destination")
        self._selected, self._exchange, self._destination, self._outbox = selected, exchange, destination, outbox

    def _require_destination(self):
        if (self._selected.destination_identity != self._destination
                or self._exchange.destination_identity != self._destination
                or (self._outbox is not None and self._outbox.registry.destination_identity != self._destination)):
            raise Rejected("the original Workspace destination changed")

    def _require_outbox(self):
        self._require_destination()
        if self._outbox is None:
            raise PermissionError("this native connection is read-only")
        self._outbox.require_original()
        return self._outbox

    def list_containers(self) -> tuple[Container, ...]:
        self._require_destination()
        items = self._selected.list_containers()
        self._require_destination()
        return items

    def recovery_index(self, live: tuple[Container, ...]) -> ContainerIndex:
        if self._outbox is None:
            return ContainerIndex(live)
        keys = frozenset(item.id for item in live)
        try:
            outbox = self._require_outbox()
            recovered, complete = outbox.registry.pending_recycles(keys)
            self._require_outbox()
        except (Rejected, OSError):
            return ContainerIndex(live, local_complete=False)
        if (type(recovered) is not tuple or type(complete) is not bool
                or any(type(item) is not Outcome or item.phase is not Phase.PENDING or item.receipt is not None
                       or not valid_ticket(item.ticket) or type(item.ticket.change) is not Recycle
                       or item.ticket.change.target.id in keys for item in recovered)
                or len({item.ticket.change.target.id for item in recovered}) != len(recovered)):
            return ContainerIndex(live, local_complete=False)
        return ContainerIndex(live, recovered, complete)

    def open_container(self, container: str) -> Detail:
        self._require_destination()
        detail = self._selected.open_container(container)
        if type(detail) is not Detail or detail.container.id != container:
            raise Rejected("metadata is not the selected container")
        if self._outbox is None:
            detail = replace(detail, allowed=frozenset(), body=None, original=None)
        else:
            original = self._require_outbox().registry.pending(container)
            if original is not None:
                detail = replace(detail, original=original)
        self._require_destination()
        return detail

    def prepare(self, change: Change) -> Ticket:
        if not valid_change(change):
            raise Rejected("invalid selected container change")
        container = change.target.id
        outbox = self._require_outbox()
        if type(change) is CreateBranch:
            # Recheck the exact inherited node and grant before reserving.
            # The current head and unrelated metadata are not the wire target.
            read = open_checked(self._selected, container)
            source = next((branch for branch in read.branches if branch.id == change.source_branch), None)
            if (read.container.revision < change.target.revision
                    or branch_choice(read, source, change.source_node, change.branch) is None):
                raise Rejected("selected source node or new branch is no longer available")
        registry = outbox.registry
        with registry.hold(container):
            self._require_outbox()
            original = registry.reserve(change)
            self._require_outbox()
            if type(original) is not Original or not id_ok(original.submission) or not id_ok(original.operation):
                raise Rejected("no original native submission")
            if type(change) is Save and original.submission != change.body.frozen:
                raise Rejected("selected body is not the original sealed submission")
            with reopen_submit(outbox.state, original.submission, destination=self._destination) as pending:
                _action, base, _prefix, confirmed = self._exact(registry, pending, original, change)
                if confirmed:
                    raise Rejected("original already has a receipt; read its status")
                return Ticket(change, original.operation, original.submission, base)

    def apply(self, ticket: Ticket) -> Outcome:
        return self._advance(ticket, send=True)

    def status(self, ticket: Ticket) -> Outcome:
        return self._advance(ticket, send=False)

    def _exact(self, registry, pending, original, change):
        key = change.target.id
        prefix = registry.prefix(original.submission, key)
        if type(prefix) is not tuple:
            raise Rejected("no exact previously confirmed container prefix")
        pending.plan.next_for(key, prefix)  # validate the full held prefix before truncating it for a lookup
        queue = dict(pending.plan.queues()).get(key, ())
        position = next((i for i, action in enumerate(queue) if action.operation_id == original.operation), None)
        if position is None or position > len(prefix):
            raise Rejected("an earlier operation has not confirmed on this container")
        prior = prefix[:position]
        action, base = pending.plan.next_for(key, prior)
        if (action.operation_id != original.operation
                or not _matches(action, base, original, change)):
            raise Rejected("the selected action is not this original pending plan")
        return action, base, prior, position < len(prefix)

    def _advance(self, ticket, *, send):
        if not valid_ticket(ticket):
            raise Rejected("invalid original container ticket")
        original = Original(ticket.submission, ticket.operation)
        key = ticket.change.target.id
        outbox = self._require_outbox()
        registry = outbox.registry
        with registry.hold(key, existing=True):
            self._require_outbox()
            with reopen_submit(outbox.state, original.submission, destination=self._destination) as pending:
                action, base, prefix, confirmed = self._exact(registry, pending, original, ticket.change)
                if base != ticket.base_sequence:
                    raise Rejected("the original resolved sequence changed")
                registry.require_prepared(original, ticket.change, settled=confirmed)
                self._require_outbox()
                try:
                    result = invoke_original(pending, key, prefix,
                                             _OriginalExchange(self._exchange, self._require_outbox),
                                             send_if_absent=send and not confirmed)
                except SubmitAbsent:
                    if confirmed or send:  # never rearm a locally verified completed operation
                        raise Rejected("original receipt disagrees with local settlement")
                    self._require_outbox()
                    return Outcome(ticket, Phase.ABSENT)
                except SubmitConflict:
                    return Outcome(ticket, Phase.CONFLICT)
                if type(result) is not ConfirmedAction:
                    raise Rejected("no exact original settlement")
                self._require_outbox()
                receipt = _receipt(ticket, action, result)
                registry.record(original, result)
                return Outcome(ticket, Phase.CONFIRMED, receipt)


def _matches(action, base, original, change):
    if action.container_id != change.target.id:
        return False
    if type(change) is Save:
        body = change.body
        first = change.target.revision == 0
        return (
            original.submission == body.frozen
            and action.kind == "save_node" and (base == 0) == first
            and action.branch == change.branch == body.target_branch and action.node_id == body.node
            and action.body == FullBody(body.length, body.sha256)
            and action.fork_from_main == change.from_main
            and action.head == (None if first or change.from_main is not None else body.selected_base)
            and action.name == (change.target.name if first else None)
        )
    if base != change.target.revision or not base:
        return False
    if type(change) is CreateBranch:
        return action.kind == "create_branch" and (action.branch, action.head) == (change.branch, change.source_node)
    if type(change) is Rename:
        return action.kind == "rename" and action.name == change.name
    if type(change) is Tags:
        return action.kind == "set_tags" and action.tags == tuple(sorted(change.values))
    return type(change) is Recycle and action.kind == "delete"


def _receipt(ticket, action, result):
    fence = result.fence
    if (type(fence) is not ReceiptFence
            or (fence.container_id, fence.operation_id, fence.kind)
            != (action.container_id, action.operation_id, action.kind)):
        raise Rejected("receipt belongs to a different original action")
    if type(ticket.change) is Recycle:
        if fence.sequence is not None or fence.base_revision != ticket.base_sequence:
            raise Rejected("no exact dedicated recycle receipt")
        return Receipt(ticket, None, recycled=True,
                       recycle_source_cleanup_pending=result.recycle_source_cleanup_pending)
    if type(ticket.change) is CreateBranch:
        change = ticket.change
        if (fence.branch, fence.head, fence.base_revision) != (change.branch, change.source_node, None):
            raise Rejected("new branch receipt differs from the selected original node")
        return Receipt(ticket, fence.sequence, branch=fence.branch, node=fence.head)
    if type(ticket.change) is Save:
        if (fence.branch, fence.head, fence.base_revision) != (action.branch, action.node_id, None):
            raise Rejected("save receipt differs from original branch or node")
        body = ticket.change.body
        return Receipt(ticket, fence.sequence, branch=fence.branch, node=fence.head,
                       parent=body.selected_base, length=body.length, sha256=body.sha256)
    if fence.base_revision != ticket.base_sequence:
        raise Rejected("metadata receipt differs from original base")
    return Receipt(ticket, fence.sequence)
