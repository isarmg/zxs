"""Explicit native authority to settle one exact old transfer without opening WORK."""
from contextlib import contextmanager
from pathlib import Path
import sys
from threading import Event, Lock

from ..model import Rejected
from ..private_candidates import held_candidates
from ..private_freeze import local_mount
from .container_client import BoundOutbox, ContainerClient
from .container_connection import OpenConnection
from .container_management import Phase, id_ok, lookup, retry
from .container_outbox import NativeOutbox


class ColdTransfer:
    """No connection setup, lists, prepare/new IDs, mount, or ordinary operations."""
    def __init__(self, client, require_original, ticket):
        self._client, self._require_original, self._ticket = client, require_original, ticket
        self._lock, self._absence = Lock(), None

    @property
    def ticket(self):
        return self._ticket

    def status(self):
        with self._lock:
            self._require_original()
            self._absence = None
            result = lookup(self._client, self._ticket)
            self._require_original()
            if result.phase is Phase.ABSENT:
                self._absence = result
            return result

    def retry(self, observed, *, confirmed=False):
        with self._lock:
            self._require_original()
            if (confirmed is not True or observed is None or observed is not self._absence
                    or observed.ticket != self._ticket):
                raise Rejected("check this exact original receipt first, then confirm its retry")
            self._absence = None  # each deliberate retry consumes the preceding exact observation
            result = retry(self._client, observed, confirmed=True)
            self._require_original()
            return result


@contextmanager
def reopen_original_transfer(connection, state, *, container, submission, operation, confirmed=False):
    """Previously enrolled STATE and a newly established read connection only.

    This factory reads no WORK/catalog records. Setup performs no request; the
    first exposed request is exact receipt status, not a mutation.
    """
    if (sys.platform != "linux" or confirmed is not True or not isinstance(state, Path)
            or not state.is_absolute() or type(connection) is not OpenConnection
            or connection._listed is not True or connection.writable
            or not all(map(id_ok, (container, submission, operation)))):
        raise Rejected("explicit original private STATE, authenticated reader and known original IDs required")
    connection._authorization.require_live()
    destination, account = connection._exchange.destination_identity, connection._account
    revoked = Event()
    with held_candidates(state) as area:
        admitted_mount = local_mount(area.parent)
        registry = NativeOutbox(state, destination, account=account)

        def require_original():
            if revoked.is_set():
                raise Rejected("this exact cold transfer authority has closed")
            connection._authorization.require_live()
            area.check_state()
            if local_mount(area.parent) != admitted_mount or registry._state_identity != area.identity:
                raise Rejected("selected original STATE was replaced")
            registry.require_available()

        require_original()
        ticket = registry.prepared_original_ticket(container, submission, operation)
        require_original()
        bound = BoundOutbox(state, registry, require_original)
        # Use immutable selected destination for the existing native client.
        # Its regular mutation preparation/list APIs are never exposed here.
        client = ContainerClient(connection.operations._selected, connection._exchange, outbox=bound)
        try:
            yield ColdTransfer(client, require_original, ticket)
        finally:
            revoked.set()
