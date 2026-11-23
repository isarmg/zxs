"""Release one exact confirmed Windows SaveNode slot by two no-replace same-parent renames, never deletion."""
from contextlib import ExitStack

from .._windows_access import OutputProtection
from .._windows_output import ATTR, DELETE, FILE_ONLY, OPEN, READ, READ_CONTROL, SYNC, PinnedParent, _NTError
from ..model import Rejected
from .container_windows_draft import _require_reader
from .container_windows_receipt import _original_proof, _ticket
from .container_windows_save import WindowsSaveKey, _decode, _key_record, _key_slot, _read_record, _slot


class WindowsRetirementPending(Rejected):
    def __init__(self, key, receipt):
        self.key, self.receipt = key, receipt
        super().__init__(f"The original save is confirmed at sequence {receipt.revision}; "
                         "only local release of its pending slot is unconfirmed. Do not submit again.")


def _named_record(keep, home, live, archived, *, move):
    api = home.api
    def existing(name):
        if move:
            return api._request(home.handle, name, SYNC | ATTR | READ | READ_CONTROL | DELETE, 0, OPEN, FILE_ONLY)
        return api.inspected_file(home.handle, name)

    try:
        selected = existing(archived)
    except _NTError as err:
        if err.code not in (0xc0000034, 0xc000000f):
            raise
        handle, name = keep.enter_context(existing(live)), live
    else:
        handle, name = keep.enter_context(selected), archived
    acl = OutputProtection(home.handle)
    fid = api.ordinary(handle, home.volume, home.name(name)).file_id
    return [handle, name, fid, acl]


def _read(home, row):
    h, name, fid, acl = row
    return _read_record(home, h, name, fid, acl)


def _chosen_key(keep, home, source, key, *, move=False):
    live = _key_slot(source.destination, source.account, key.container)
    archive = key.submission + ".zwk"
    row = _named_record(keep, home, live, archive, move=move)
    expected = _key_record(key, home, row[2], source.destination, source.origin, source.account)
    def check():
        if _read(home, row) != expected:
            raise Rejected("the exact Windows recovery key or its original location changed")
    check()
    return row, check


def retire_original_windows_save(connection, private_parent, key, *, confirmed=False):
    """Local only: prove .zrr + both original records, then archive intent FIRST and recovery key LAST.

    If both archived names already exist, don't open whatever now occupies the live slots.
    If a rename response/barrier was lost, finish only the remaining exact original transition.
    """
    if confirmed is not True or type(key) is not WindowsSaveKey:
        raise Rejected("select the original Windows operation before releasing its local slots")
    h2, auth, reader = connection._exchange, connection._authorization, connection.operations
    dest, origin, account = h2.destination_identity, h2._location, connection._account
    def live():
        _require_reader(connection, reader, h2, auth, dest, origin, account)
    live()
    with PinnedParent(private_parent, output=True) as home, ExitStack() as keep:
        api = home.api
        al = key.submission + ".zwi"
        intent = _named_record(keep, home, _slot(dest, account, key.container), al, move=True)
        raw = _read(home, intent)
        source, plan, _, _ = _decode(raw, key, home, intent[2], connection)
        ticket = _ticket(key, source, plan)
        krow, check_key = _chosen_key(keep, home, source, key, move=True)
        kl = key.submission + ".zwk"
        if krow[1] == kl and intent[1] != al:
            raise Rejected("unexpected Windows slot state: original intent was not retired before its key")
        receipt = _original_proof(home, key, ticket)
        if receipt is None:
            raise Rejected("no retained server-confirmed original save to retire")
        def check():
            live()
            home.check()
            if _read(home, intent) != raw or _original_proof(home, key, ticket) != receipt:
                raise Rejected("original Windows intent or confirmed proof changed")
            check_key()
            live()
        try:
            check()
            # Also complete a previously interrupted directory-sync response at each exact position.
            for row, archive in ((intent, al), (krow, kl)):
                if row[1] != archive:
                    api.rename_new(row[0], home.handle, archive)
                    row[1] = archive
                api.flush(home.handle)
                check()
            return receipt
        except Exception as error:
            raise WindowsRetirementPending(key, receipt) from error
