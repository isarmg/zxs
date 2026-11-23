"""Inert checks for an explicitly supplied disposable, signed macOS 27 desktop/R3 fixture. No entry point."""
from unittest.mock import patch

from zipworkspace import macos_drive_owner as owner
from zipworkspace._macos_fskit_release import packaged
from zipworkspace.macos_retained_submit import Phase, status_original, submit_original
from zipworkspace.publish import Uncertain


def unqualified_panel(panel):
    """An ordinary unsigned/ad-hoc distribution, not an injected native adapter."""
    from zipworkspace.desktop.container_macos_controls import MacDiskControls
    controls = panel.platform_disk
    if packaged() or type(controls) is not MacDiskControls:
        raise AssertionError("need the unqualified ordinary macOS panel")
    controls.paint()
    if controls.edit.isEnabled() or controls.recover.isEnabled() or controls.busy():
        raise AssertionError("missing release evidence must leave native actions disabled")


def recover_after_first_archive_move(connection, capture, receipt):
    """Interrupted between original-key archive and active-route archive. Does not mount or send."""
    if capture.key is None or receipt.phase is not Phase.CONFIRMED:
        raise AssertionError("requires a real already-settled disposable original")
    expected = owner.original_save(connection,capture)
    actual = owner._move_record
    def fail_between(area,src,dst,fd,moved):
        if dst=="ended-"+capture.known.token: raise OSError("injected between original moves")
        return actual(area,src,dst,fd,moved)
    with patch.object(owner,"_move_record",fail_between):
        try: owner.archive(connection,capture,receipt)
        except Uncertain: pass
        else: raise AssertionError("partial archive was reported as complete")
    route,known,cap = owner.recover(connection,capture.route.container)
    if (route,known,cap)!=(capture.route,capture.known,capture) or owner.original_save(connection,cap)!=expected:
        raise AssertionError("lost or changed original after its first physical move")
    owner.archive(connection,cap,receipt)
    if owner.recover(connection,route.container,ended_token=known.token)[2]!=capture:
        raise AssertionError("archived original cannot be recovered by its own token")


def missing_archived_receipt_is_not_send_authority(connection, capture, request_log):
    """Fixture: original physical key already archived; durable local receipt is missing and mock R3 would return ABSENT."""
    chosen=owner.original_save(connection,capture)
    before=len(request_log)
    observed=status_original(connection,chosen,confirmed=True)
    again=submit_original(connection,chosen,confirmed=True)
    if any(v.phase is not Phase.UNKNOWN or v.fence is not None for v in (observed,again)) or len(request_log)!=before:
        raise AssertionError("missing retired receipt must not induce even a new GET/POST or an absent result")


def verify_original_parked(connection, drive, registration, outcome):
    """Call after the independently observed original kernel normal gone. No reattach or network."""
    from zipworkspace.macos_fskit_handoff import CompletedDrive
    if outcome=="waiting-provider":
        drive.park_system_gone(confirmed=True)
        try: CompletedDrive.reopen(registration,connection,confirmed=True)
        except OSError: return
        raise AssertionError("parking the original metadata turned an unclean resource into an accepted capture")
    completed=CompletedDrive.after_normal(drive,connection)
    completed.park(confirmed=True)
    again=CompletedDrive.reopen(registration,connection,confirmed=True)
    again.park(confirmed=True)
