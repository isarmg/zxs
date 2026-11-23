"""Explicit status or one same-operation R3 save from the original private APFS packet."""
from contextlib import ExitStack, contextmanager
from dataclasses import asdict, dataclass, replace
from enum import Enum
from hashlib import sha256
import json
import os

from ._macos_copy_evidence import _absent
from .container_http import (ConfirmedAction, ReplyKind, SubmitAbsent, SubmitConflict, SubmitUnconfirmed,
                             invoke_original, lookup_message, mutation_message)
from .container_plan import Action, ReceiptFence
from .container_spool import _private, _write_all
from .macos_retained_edit import RetentionKey, _attributes, _barrier, _birth, _connection, _fd, _read, _visit_original
from .model import Rejected, canonical
from .publish import Uncertain


@dataclass(frozen=True, slots=True)
class OriginalSave:
    key: RetentionKey
    action: Action


class Phase(Enum):
    ABSENT = "absent"
    CONFIRMED = "confirmed"
    CONFLICT = "conflict"
    UNKNOWN = "unknown"


@dataclass(frozen=True, slots=True)
class SaveResult:
    original: OriginalSave
    phase: Phase
    attempted: bool
    fence: ReceiptFence | None = None


def review_original(connection, state, container, branch, node, *, confirmed=False):
    """Local only: reveal the exact original operation, target, node, length and SHA to review."""
    if confirmed is not True:
        raise Rejected("explicit selection of a retained macOS save required")
    return _visit_original(connection, state, container, branch, node, lambda p: OriginalSave(p.key, p.action))


def status_original(connection, chosen, *, confirmed=False):
    return _advance(connection, chosen, "status", confirmed)


def submit_original(connection, chosen, *, confirmed=False):
    return _advance(connection, chosen, "first", confirmed)


def retry_original(connection, absent, *, confirmed=False):
    if type(absent) is not SaveResult or absent.phase is not Phase.ABSENT or absent.attempted is not True:
        raise Rejected("retry requires the original previously attempted save and a selected absent receipt")
    return _advance(connection, absent.original, "retry", confirmed)


def _prefix(key):
    return sha256(key.encode()).hexdigest()


def _binding(packet):
    key = packet.key
    return sha256(canonical([key.destination, key.account, key.container])).hexdigest()


def _record(fd, kind, binding, value):
    # Each record identifies its physical birth; replacement/torn/unrelated records never authorize a send.
    return canonical({"v": 1, "kind": kind, "binding": binding, "self": _birth(fd), "value": value})


def _create(area, name, kind, binding, value):
    area.check_state()
    with _fd(area.parent, name, create=True) as fd:
        _attributes(fd); area.check_file(name, fd)
        raw = _record(fd, kind, binding, value)
        _write_all(fd, raw)
        os.fchmod(fd, 0o400)
        _barrier(fd)
        actual, _ = _read(area, name, fd, len(raw), metadata=True)
        if actual != raw:
            raise Rejected("private operation marker did not round-trip")
        os.fsync(area.parent)
        _barrier(fd)


def _load(stack, area, name, kind, binding):
    area.check_state()
    try:
        fd = stack.enter_context(_fd(area.parent, name))
    except FileNotFoundError:
        return None
    raw, stamp = _read(area, name, fd, os.fstat(fd).st_size, metadata=True)
    try:
        value = json.loads(raw)["value"]
        if raw != _record(fd, kind, binding, value):
            raise ValueError()
    except (TypeError, ValueError, KeyError, RecursionError) as exc:
        raise Rejected("torn or different original macOS operation marker") from exc
    return value, lambda: _private(area, name, fd, stamp), fd


@contextmanager
def _container_lease(area, key):
    # The same physically bound lock serializes first save, lookup, archive
    # and the next desktop admission for this endpoint/account/container only.
    import fcntl
    name = "lease-" + key
    with ExitStack() as held:
        try:
            _create(area, name, "lease", key, True)
        except FileExistsError:
            pass
        record = _load(held, area, name, "lease", key)
        if record is None or record[0] is not True:
            raise Rejected("no original private container lease")
        _, check, fd = record
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            check()
            yield check
            check()
        finally:
            fcntl.flock(fd, fcntl.LOCK_UN)


@contextmanager
def _lease(packet):
    with _container_lease(packet.root_area, _binding(packet)) as owner:
        def guard():
            packet.require(); owner()
        guard()
        yield guard
        guard()


def _advance(connection, chosen, mode, confirmed):
    if confirmed is not True or type(chosen) is not OriginalSave or type(chosen.key) is not RetentionKey:
        raise Rejected("explicit review of the exact retained macOS operation required")
    destination, account, authorize = _connection(connection)
    key, exchange = chosen.key, connection._exchange
    if (destination, account) != (key.destination, key.account):
        raise Rejected("not the original authenticated destination and account")

    def visit(p):
        if OriginalSave(p.key, p.action) != chosen:
            raise Rejected("the selected original action no longer matches its physical packet")
        with _lease(p) as check, ExitStack() as records:
            area, binding = p.root_area, _prefix(p.key)
            sent_name, done_name = "attempt-" + key.submission, "receipt-" + key.submission
            attempted = _load(records, area, sent_name, "attempt", binding)
            settled = _load(records, area, done_name, "receipt", binding)
            if attempted is not None and attempted[0] != p.action.operation_id:
                raise Rejected("attempt belongs to a different operation")
            # An archived physical key is evidence of a retired original, never send authority.
            # Do not translate lost/corrupt local receipt evidence into permission to POST.
            if p.archived and settled is None:
                return SaveResult(chosen, Phase.UNKNOWN, attempted is not None)
            fence = None
            if settled is not None:
                try:
                    fence = ReceiptFence(**settled[0])
                    if p.pending.plan.next_for(key.container, (fence,)) is not None:
                        raise ValueError()
                except (TypeError, KeyError, ValueError) as exc:
                    raise Rejected("not this original's verified receipt") from exc

            def guard():
                if connection._exchange is not exchange:
                    raise Rejected("not the originally selected authenticated exchange")
                authorize(); check()
                if attempted: attempted[1]()
                else: _absent(area.parent, sent_name)
                if settled: settled[1]()
                else: _absent(area.parent, done_name)

            queried = can_write = posted = False
            class Exchange:
                destination_identity = destination
                def request(self, message):
                    nonlocal attempted, queried, can_write, posted
                    guard()
                    if not queried and message == lookup_message(p.action):
                        queried = True
                        result = exchange.request(message)
                        guard()
                        can_write = result.kind is ReplyKind.ABSENT and result.receipt is None
                        return result
                    if (not queried or not can_write or posted or mode == "status" or p.archived or settled
                            or mode == "first" and attempted or mode == "retry" and not attempted):
                        raise SubmitUnconfirmed("no authorization for a new attempt on this exact save")
                    expected = mutation_message(p.action, p.action.base_revision, payload=message.body)
                    if message != expected or message.body is None:
                        raise SubmitUnconfirmed("not the exact reviewed complete save")
                    posted = True
                    if attempted is None:
                        _create(area, sent_name, "attempt", binding, p.action.operation_id)
                        attempted = _load(records, area, sent_name, "attempt", binding)
                        if not attempted or attempted[0] != p.action.operation_id:
                            raise Uncertain("original attempted marker could not be confirmed; do not resend")
                    os.fsync(area.parent)
                    _barrier(attempted[2])
                    guard()  # The durable original marker precedes even the first outbound POST byte.
                    def guarded_blocks():
                        it = iter(message.body)
                        while True:
                            guard()
                            try:
                                part = next(it)
                            except StopIteration:
                                return
                            guard()
                            yield part
                    result = exchange.request(replace(message, body=guarded_blocks()))
                    guard()
                    return result

            guard()
            send = not p.archived and not settled and ((mode == "first" and not attempted) or (mode == "retry" and attempted))
            try:
                result = invoke_original(p.pending, key.container, (), Exchange(), send_if_absent=bool(send))
            except SubmitAbsent:
                guard()
                if settled: raise Uncertain("remote absence disagrees with an existing local receipt")
                return SaveResult(chosen, Phase.ABSENT, attempted is not None)
            except (SubmitUnconfirmed, RecursionError) as exc:
                guard()
                # A contradictory 409 cannot refute an earlier matched durable receipt.
                phase = Phase.CONFLICT if isinstance(exc, SubmitConflict) and not settled else Phase.UNKNOWN
                return SaveResult(chosen, phase, attempted is not None)
            if type(result) is not ConfirmedAction or p.pending.plan.next_for(key.container, (result.fence,)) is not None:
                raise Uncertain("no exact original settlement")
            guard()
            if settled is None:
                _create(area, done_name, "receipt", binding, asdict(result.fence))
                settled = _load(records, area, done_name, "receipt", binding)
                if settled is None or settled[0] != asdict(result.fence):
                    raise Uncertain("original receipt was not durably recorded")
            elif fence != result.fence:
                raise Uncertain("authenticated receipt differs from the earlier original")
            os.fsync(area.parent)
            _barrier(settled[2])
            guard()
            return SaveResult(chosen, Phase.CONFIRMED, attempted is not None, result.fence)
    return _visit_original(connection, key.root, key.container, key.branch, key.node, visit, exact_key=key)
