"""Inert signed-reference assertions. Caller supplies only a separately approved disposable completed mount."""
from hashlib import sha256

from zipworkspace.macos_fskit_handoff import CompletedDrive, KnownDrive
from zipworkspace.macos_retained_submit import review_original
from zipworkspace.model import Rejected


def require_original(connection, drive, state, original, expected, *, changed):
    """Real missing/late writer, ENOSPC, forced/host loss and corrupted marker are separate failure cases."""
    key = KnownDrive.from_original(drive)
    if KnownDrive.decode(key.encode()) != key:
        raise AssertionError("original key did not round-trip")
    completed = CompletedDrive.after_normal(drive, connection)
    kept = completed.borrow(connection, original, confirmed=True).retain(connection, state, confirmed=True)
    if (kept.body.length, kept.body.sha256) != (len(expected), sha256(expected).hexdigest()):
        raise AssertionError("original normal readback mismatch")
    if bool(kept.key) != changed:
        raise AssertionError("an unchanged body must not reserve a remote operation")
    if changed:
        before = review_original(connection, state, *original, confirmed=True)  # Local only, no POST.
        if before.key != kept.key or before.action.body != kept.body:
            raise AssertionError("not the same complete original operation")
    # Singleton fixture: close native handles; independent cold reopen reuses the
    # same physical operation and must not allocate another Plan for the same edit.
    completed.release(confirmed=True)
    again = CompletedDrive.reopen(key, connection, confirmed=True)
    repeated = again.borrow(connection, original, confirmed=True).retain(connection, state, confirmed=True)
    if repeated != kept:
        raise AssertionError("same session did not reopen its own final generation and key")
    again.release(confirmed=True)


def refuse_unknown(known, connection):
    """Supply a known original whose app, provider or host was interrupted before both receipts."""
    try:
        CompletedDrive.reopen(known, connection, confirmed=True)
    except (OSError, Rejected):
        return
    raise AssertionError("incomplete original mount was admitted as normally finished")
