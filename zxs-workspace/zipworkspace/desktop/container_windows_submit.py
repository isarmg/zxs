"""A separately confirmed capability for exactly one original Windows full SaveNode. No generic write client."""
from contextlib import ExitStack
from dataclasses import asdict, dataclass
from threading import Lock
from types import SimpleNamespace

from .._windows_access import OutputProtection
from .._windows_output import _NTError
from ..container_http import (Message, Reply, ReplyKind, SubmitAbsent, SubmitConflict, SubmitUnconfirmed,
                              invoke_original, lookup_message, mutation_message, verified_result)
from ..model import Rejected, canonical, new_id
from .container_client import _receipt
from .container_management import Outcome, Phase, Ticket
from .container_windows_draft import DraftSource
from .container_windows_receipt import WindowsReceiptUncertain, _original_proof, _retain_verified, _ticket
from .container_windows_retire import WindowsRetirementPending, _chosen_key, retire_original_windows_save
from .container_windows_save import (WindowsSaveKey, _LIMIT, _folder, _held_windows_body, _new_record,
                                     _original_record, _read_record)


@dataclass(frozen=True, slots=True)
class WindowsSaveReview:
    key: WindowsSaveKey
    source: DraftSource  # Includes the exact remote spelling, trusted destination, original account/branch/head.
    ticket: Ticket       # Includes immutable original body hash, length and new node. No rename.


def _arm(keep, home, key, check):
    # This is a POSSIBLE request, not proof any network byte was sent. Never replace it.
    api, leaf = home.api, key.submission + ".zsa"
    def record(fid):
        return canonical({"format": "windows-r3-attempt/1", "key": asdict(key), "folder": _folder(home),
                          "file_id": fid.hex()})
    try:
        taken = api.inspected_file(home.handle, leaf)
    except _NTError as error:
        if error.code not in (0xc0000034, 0xc000000f):
            raise
        temp = new_id() + ".zst"
        h, fid, acl = _new_record(keep, home, temp)
        raw = record(fid)
        if not 0 < len(raw) <= _LIMIT:
            raise Rejected("invalid original attempt evidence size")
        api.write(h, raw)
        api.flush(h)
        if _read_record(home, h, temp, fid, acl) != raw:
            raise Rejected("original Windows attempt did not survive readback")
        check()
        api.rename_new(h, home.handle, leaf)
        api.absent(home.handle, temp)
    else:
        h = keep.enter_context(taken)
        acl = OutputProtection(home.handle)
        fid = api.ordinary(h, home.volume, home.name(leaf)).file_id
        raw = record(fid)
    api.flush(home.handle)  # Before the first possible POST, including a prior lost barrier.
    def held():
        check()
        if _read_record(home, h, leaf, fid, acl) != raw:
            raise Rejected("the original durable Windows attempt record no longer matches")
    held()
    return held


class _OneExchange:
    """The trusted invoke_original engine receives no general-purpose writer."""
    def __init__(self, exchange, action, pending, check, arm):
        self._exchange, self._action, self._pending, self._check, self._arm = exchange, action, pending, check, arm
        self.destination_identity = exchange.destination_identity
        self._state, self.may_have_posted, self.post_reply = "lookup", False, None

    def request(self, message):
        self._check()
        if self._exchange.destination_identity != self.destination_identity or type(message) is not Message:
            raise Rejected("Windows submit origin or message was replaced")
        if self._state == "lookup" and message == lookup_message(self._action):
            self._state = "consumed"
            reply = self._exchange.request(message)
            self._check()
            if type(reply) is not Reply:
                return Reply(ReplyKind.UNKNOWN)
            if reply.kind is ReplyKind.ABSENT and reply.receipt is None and self._arm is not None:
                self._state = "original-absent"
            return reply
        if (self._state != "original-absent" or message.body is None or
                message != mutation_message(self._action, self._action.base_revision, payload=message.body)):
            raise Rejected("no authoritative absence for this original, unchanged SaveNode")
        self._state = "consumed"
        self._pending._issued(message.body)
        held = self._arm()
        held()
        self._pending._issued(message.body)
        self.may_have_posted = True  # The next call is the first possible wire write.
        reply = self._exchange.request(message)  # No loop, no new IDs, no redirect or mutation on close.
        self.post_reply = reply if type(reply) is Reply else None
        held()
        return reply if type(reply) is Reply else Reply(ReplyKind.UNKNOWN)


class WindowsOriginalSaveChoice:
    """Construct to show local evidence. Only apply(exact same review, confirmed=True) can touch R3."""
    def __init__(self, connection, private_parent, key):
        with _original_record(connection, private_parent, key, confirmed=True) as selection:
            home, source, plan, _, _, check, _, _ = selection
            check()
            self.review = WindowsSaveReview(key, source, _ticket(key, source, plan))
            self._folder = _folder(home)
        self._connection, self._parent, self._key = connection, private_parent, key
        self._lock, self._used = Lock(), False

    def apply(self, review, *, confirmed=False):
        if review is not self.review or confirmed is not True:
            raise Rejected("explicitly confirm this original destination/account, container, branch, body and operation")
        with self._lock:
            if self._used:
                raise Rejected("this Windows Submit decision has already been consumed")
            self._used = True
        trace = {"exchange": None, "settled": None}
        try:
            return self._one(review, trace)
        except (WindowsReceiptUncertain, WindowsRetirementPending):
            raise
        except Exception as error:
            # Includes ExitStack/native HANDLE close failures *after* a network effect.
            settled, exchange = trace["settled"], trace["exchange"]
            if settled is not None and settled.phase is Phase.CONFIRMED:
                raise WindowsRetirementPending(self._key, settled.receipt) from error
            if exchange is None or not exchange.may_have_posted:
                raise
            reply, action, ticket = exchange.post_reply, exchange._action, review.ticket
            if reply is not None and reply.kind is ReplyKind.SELECTED:
                try:
                    exact = verified_result(action, ticket.base_sequence, reply.receipt)
                except SubmitUnconfirmed:
                    pass
                else:
                    raise WindowsReceiptUncertain(self._key, _receipt(ticket, action, exact)) from error
            return Outcome(ticket, Phase.UNKNOWN)

    def _one(self, review, trace):
        conn, key = self._connection, self._key
        with _original_record(conn, self._parent, key, confirmed=True) as selected, ExitStack() as keep:
            home, source, plan, _, _, check_record, _, retired = selected
            ticket, action = _ticket(key, source, plan), plan.actions[0]
            if self._folder != _folder(home) or (source, ticket) != (review.source, review.ticket):
                raise Rejected("the reviewed original Windows save or private directory changed")
            keyrow, check_key = _chosen_key(keep, home, source, key)
            if not retired and keyrow[1] == key.submission + ".zwk":
                raise Rejected("original recovery key has already moved without its intent")
            def check():
                check_record()
                check_key()
                conn._authorization.require_account(source.account)
            check()
            prior = _original_proof(home, key, ticket)
            send = prior is None and not retired
            if send:
                pending = keep.enter_context(_held_windows_body(selected, conn, key))  # Full readback before GET.
                arm = lambda: _arm(keep, home, key, check)
            else:
                pending = SimpleNamespace(plan=plan, destination=source.destination)
                arm = None
            trace["exchange"] = exchange = _OneExchange(conn._exchange, action, pending, check, arm)
            try:
                result = invoke_original(pending, key.container, (), exchange, send_if_absent=send)
            except SubmitConflict:
                return Outcome(ticket, Phase.UNKNOWN if prior is not None or retired else Phase.CONFLICT)
            except (SubmitAbsent, SubmitUnconfirmed):
                return Outcome(ticket, Phase.UNKNOWN)
            trace["settled"] = settled = _retain_verified(home, key, ticket, action, result, prior, check)
        if settled.phase is Phase.CONFIRMED:
            try:
                if retire_original_windows_save(conn, self._parent, key, confirmed=True) != settled.receipt:
                    raise Rejected("local Windows slot did not retire the exact confirmed receipt")
            except WindowsRetirementPending:
                raise
            except Exception as error:
                raise WindowsRetirementPending(key, settled.receipt) from error
        return settled
