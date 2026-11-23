"""Native Linux Workspace owner, never enabled by a read or a pathname alone."""
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
import os
import sys
from threading import Event

from .. import lifecycle, worktree
from ..linux_edit_broker import EditMountChoice
from ..model import Rejected
from ..private_candidates import held_candidates
from ..private_freeze import local_mount
from .container_connection import NativeWriteProfile, OpenConnection
from .container_management import Body, Container, Detail, Kind, Outcome, Save, valid_change
from .container_outbox import NativeOutbox
from .live_checkout import LiveCheckout


def _state_matches(live, area):
    held = os.fstat(live.state_fd)
    area.check_state()
    return ((held.st_dev, held.st_ino) == area.identity
            and local_mount(live.state_fd) == local_mount(area.parent))


def _save_body(container, submission, action):
    source_branch = "main" if action.fork_from_main is not None else action.branch
    source_node = action.fork_from_main if action.fork_from_main is not None else action.head
    return Body(container, submission, source_branch, source_node, action.body.length,
                action.body.sha256, action.branch, action.node_id)


@dataclass(frozen=True, slots=True)
class FirstSaveOffer:
    submission: str
    container: str
    name: str
    node: str
    length: int
    sha256: str
    operation: str
    original: Outcome | None


@dataclass(frozen=True, slots=True)
class NativeEditOwner:
    profile: NativeWriteProfile
    live: LiveCheckout
    original: LiveCheckout
    registry: NativeOutbox
    account: str
    destination: str

    def require_live(self):
        if self.original.closed or self.live.closed or self.original.review_origin is not self.live.review_origin:
            raise Rejected("the original live checkout owner has closed")
        self.profile.require_state()
        self.registry.require_available()

    def edit_choice(self, point, *, confirmed=False):
        self.require_live()
        if confirmed is not True:
            raise Rejected("select the original native checkout mountpoint explicitly")
        # Keep the UI's actual creating Window LiveCheckout, not a replacement handle.
        # The accepted Window later constructs the controller with its own live connection.
        return EditMountChoice(self.original, point)

    def first_saves(self, connection):
        """Discover exact local new-file queues, even before any remote container exists."""
        self.require_live()
        if (type(connection) is not OpenConnection or not connection.writable
                or Kind.SAVE not in self.profile._allowed or connection._account != self.account
                or connection._exchange.destination_identity != self.destination):
            raise Rejected("not this owner's selected first-file connection")
        connection._authorization.require_live()
        outbox = self.profile.bind(self.destination, self.account)
        if outbox is None or connection.operations._outbox is not outbox:
            raise Rejected("not this owner's authenticated original outbox")
        rows, complete = self.registry.first_registered_saves()
        self.require_live()
        connection._authorization.require_live()
        return tuple(FirstSaveOffer(submission, action.container_id, action.name, action.node_id,
                                    action.body.length, action.body.sha256, action.operation_id, original)
                     for submission, action, original in rows), complete

    def prepare_first_save(self, connection, submission, container, *, confirmed=False):
        """Choose the exact locally registered first node of a new container; never send."""
        self.require_live()
        if (confirmed is not True or type(connection) is not OpenConnection or not connection.writable
                or Kind.SAVE not in self.profile._allowed or connection._account != self.account
                or connection._exchange.destination_identity != self.destination):
            raise Rejected("explicitly select the original first SaveNode for this owner")
        outbox = self.profile.bind(self.destination, self.account)
        if outbox is None or connection.operations._outbox is not outbox:
            raise Rejected("not this owner's authenticated original outbox")
        selected = self.registry.next_registered_save(container)
        if selected is None or selected[0] != submission:
            raise Rejected("no original first-node queue for this submission")
        _, action, base = selected
        if (base != 0 or action.branch != "main" or action.head is not None
                or action.fork_from_main is not None or action.name is None):
            raise Rejected("this is not a first SaveNode with initial main")
        change = Save(Container(container, action.name, 0), _save_body(container, submission, action), "main")
        if not valid_change(change):
            raise Rejected("invalid first original SaveNode")
        original = self.registry.pending(container)
        if original is not None:
            ticket = original.ticket
            if ticket.change != change or ticket.submission != submission or ticket.operation != action.operation_id:
                raise Rejected("durable first-file ticket did not match this exact offer")
            return ticket
        return connection.operations.prepare(change)


@contextmanager
def linux_write_profile(connection, live, *, allowed, confirmed=False, initialize=False):
    """Explicitly borrow the real Window's original STATE and open/create its outbox.

    Preparing the profile (including creation if explicitly requested) is
    local only. Ordinary saved files and launching the application do not
    invoke this. Server authorization is still required at each real action.
    """
    if (sys.platform != "linux" or confirmed is not True or type(initialize) is not bool
            or type(live) is not LiveCheckout or type(connection) is not OpenConnection
            or type(allowed) is not frozenset or not allowed or not all(type(item) is Kind for item in allowed)):
        raise Rejected("explicitly select one real native checkout and allowed actions")
    connection._authorization.require_live()
    destination, account = connection._exchange.destination_identity, connection._account
    with ExitStack() as retained:
        original = live.borrow()
        retained.callback(original.close)
        with worktree.locked(original.state, read_only=True, live=original, local_only=True) as (db, row, _):
            original.check_instance(row, lifecycle.instance(db))
            lifecycle.require_visible(db, row)
        state = retained.enter_context(held_candidates(original.state))
        if live.closed or not _state_matches(original, state):
            raise Rejected("native profile and checkout did not select the same original STATE")
        registry = (NativeOutbox.create(original.state, destination, account=account) if initialize
                    else NativeOutbox(original.state, destination, account=account))

        origin = live.review_origin
        revoked = Event()

        def require_owner():
            if (revoked.is_set() or live.closed or original.closed
                    or live.review_origin is not origin or original.review_origin is not origin):
                raise Rejected("the original native checkout owner is closed or replaced")
            if not _state_matches(original, state):
                raise Rejected("the original native checkout changed STATE")

        def prepared(detail):
            require_owner()
            if type(detail) is not Detail:
                raise Rejected("invalid selected native metadata for body resolver")
            selected = registry.next_registered_save(detail.container.id)
            if selected is None:
                return None
            submission, action, base = selected
            if not base or action.name is not None:
                return None  # first body has its own explicit new-container entry
            body = _save_body(detail.container.id, submission, action)
            branches = {branch.id: branch for branch in detail.branches}
            branch = branches.get(body.branch)
            if branch is None:
                return None
            if action.fork_from_main is not None:
                if body.target_branch in branches or not any(node.id == body.selected_base for node in branch.nodes):
                    return None  # a previous mutate may have succeeded with no response
            elif branch.head != body.selected_base:
                return None  # do not block status of the retained original ticket
            return body

        profile = NativeWriteProfile(destination, account, state, allowed,
                                     require_owner=require_owner, prepared=prepared)
        owner = NativeEditOwner(profile, original, live, registry, account, destination)
        owner.require_live()
        try:
            yield owner
        finally:
            revoked.set()  # revoke old clients before releasing held descriptors
