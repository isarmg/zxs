"""Query a single original Windows R3 SaveNode GET; no network mutation, body open or retirement."""
from contextlib import ExitStack
from dataclasses import asdict

from .._windows_access import OutputProtection
from .._windows_output import _NTError
from ..container_http import Reply, ReplyKind, SubmitUnconfirmed, lookup_message, verified_result
from ..model import Rejected, canonical, new_id
from ..publish import Uncertain
from .container_client import _receipt
from .container_management import Body, Outcome, Phase, Receipt, Save, Ticket, _outcome, valid_ticket
from .container_windows_save import _LIMIT, _folder, _new_record, _original_record, _read_record


class WindowsReceiptUncertain(Uncertain):
    """The exact server result was observed; local retention is uncertain. Do not reissue the save."""
    def __init__(self, key, receipt):
        self.key, self.receipt = key, receipt
        super().__init__(f"Server matched the original save at sequence {receipt.revision}; "
                         f"local receipt retention is unconfirmed. Query only the same key {key!r}.")


def _ticket(key, source, plan):
    action, = plan.actions
    body = Body(key.container, key.submission, source.branch, source.head.id if source.head is not None else None,
                action.body.length, action.body.sha256, action.branch, action.node_id)
    ticket = Ticket(Save(source.container, body, action.branch, action.fork_from_main), key.operation, key.submission, action.base_revision)
    if not valid_ticket(ticket):
        raise Rejected("Windows original record has no exact save ticket")
    return ticket


def _proof(key, receipt, home, file_id):
    return canonical({"format": "windows-r3-receipt/1", "key": asdict(key), "folder": _folder(home),
                      "file_id": file_id.hex(), "receipt": asdict(receipt)})


def _original_proof(home, key, ticket):
    import json
    api, leaf = home.api, key.submission + ".zrr"
    try:
        file = api.inspected_file(home.handle, leaf)
    except _NTError as error:
        if error.code in (0xc0000034, 0xc000000f):
            return None
        raise
    with file as h:
        protection = OutputProtection(home.handle)
        fid = api.ordinary(h, home.volume, home.name(leaf)).file_id
        raw = _read_record(home, h, leaf, fid, protection)
        try:
            sequence = json.loads(raw)["receipt"]["revision"]
            b = ticket.change.body
            receipt = Receipt(ticket, sequence, branch=b.target_branch, node=b.node, parent=b.selected_base,
                              length=b.length, sha256=b.sha256)
            if (not _outcome(Outcome(ticket, Phase.CONFIRMED, receipt), ticket)
                    or _proof(key, receipt, home, fid) != raw):
                raise Rejected("not the complete original Windows durable receipt")
        except (ValueError, KeyError, TypeError, RecursionError) as error:
            raise Rejected("retained Windows receipt is not the original typed proof") from error
        return receipt


def _retain(home, key, receipt, check):
    api, leaf, temp = home.api, key.submission + ".zrr", new_id() + ".zrt"
    check()
    with ExitStack() as keep:
        h, fid, access = _new_record(keep, home, temp)
        raw = _proof(key, receipt, home, fid)
        if len(raw) > _LIMIT:
            raise Rejected("Windows original receipt exceeds bounded evidence size")
        api.write(h, raw)
        api.flush(h)
        if _read_record(home, h, temp, fid, access) != raw:
            raise Rejected("the local server receipt failed readback")
        check()
        api.rename_new(h, home.handle, leaf)
        api.flush(home.handle)
        if _read_record(home, h, leaf, fid, access) != raw:
            raise Rejected("the selected server receipt failed readback")
        api.absent(home.handle, temp)
        check()


def lookup_original_windows_receipt(connection, private_parent, key, *, confirmed=False):
    """One deliberate metadata-only GET. A match is retained locally; absence never enables a send.

    This fetch works without the live or frozen file body. It cannot create a missing original
    intent, grant a writable connection, remove/replace a pending slot or publish anything to R3.
    """
    with _original_record(connection, private_parent, key, confirmed=confirmed) as selection:
        home, source, plan, _, _, check_record, _, retired = selection
        action = plan.actions[0]
        ticket = _ticket(key, source, plan)
        # Only the actual native credential supplier may attest the account, not just a replaced
        # OpenConnection label. A runtime without that capability cannot issue even this new GET.
        account_check = getattr(connection._authorization, "require_account", None)
        if not callable(account_check):
            raise Rejected("the original Windows credential cannot attest its login")
        def check():
            check_record()
            account_check(source.account)
        check()
        prior = _original_proof(home, key, ticket)
        check()
        reply = connection._exchange.request(lookup_message(action))  # Only GET /operations/{original}.
        check()
        if type(reply) is not Reply:
            return Outcome(ticket, Phase.UNKNOWN)
        if reply.kind is not ReplyKind.SELECTED:
            if retired or prior is not None or reply.receipt is not None:
                return Outcome(ticket, Phase.UNKNOWN)  # A locally settled operation never becomes absent/retryable.
            phase = {ReplyKind.ABSENT: Phase.ABSENT, ReplyKind.CONFLICT: Phase.CONFLICT}.get(reply.kind, Phase.UNKNOWN)
            return Outcome(ticket, phase)
        try:
            result = verified_result(action, ticket.base_sequence, reply.receipt)
        except SubmitUnconfirmed:
            return Outcome(ticket, Phase.UNKNOWN)
        return _retain_verified(home, key, ticket, action, result, prior, check)


def _retain_verified(home, key, ticket, action, result, prior, check):
    receipt = _receipt(ticket, action, result)
    confirmed_outcome = Outcome(ticket, Phase.CONFIRMED, receipt)
    if not _outcome(confirmed_outcome, ticket) or (prior is not None and prior != receipt):
        return Outcome(ticket, Phase.UNKNOWN)
    try:
        check()
        if prior is None:
            _retain(home, key, receipt, check)
        else:
            # If a prior publication lost its final directory-sync response, finish that
            # barrier; do not write another marker, replace it, or rearm the original Save.
            home.api.flush(home.handle)
        if _original_proof(home, key, ticket) != receipt:
            raise Rejected("retained Windows proof no longer matches the server")
        check()
    except Exception as error:
        raise WindowsReceiptUncertain(key, receipt) from error
    return confirmed_outcome
