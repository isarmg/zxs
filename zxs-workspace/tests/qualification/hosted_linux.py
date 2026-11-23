"""Opt-in Workspace source fixtures on hosted Linux, without package or mount qualification."""
import argparse
from contextlib import ExitStack
import os
from pathlib import Path
import stat
import sys
import time
import unittest

HERE = Path(__file__).resolve()
PROJECT = HERE.parents[2]

FILESYSTEM = (
    "test_linux_visibility.LinuxVisibility.test_restore_subdirectory_publishes_whole_original_and_no_siblings",
    "test_linux_visibility.LinuxVisibility.test_restore_one_missing_file_uses_original_basis_and_does_not_commit",
    "test_linux_visibility.LinuxVisibility.test_round_trip_renames_same_inode_after_observation_and_durable_intent",
    "test_linux_member_copy.LinuxMemberCopy.test_folder_line_diff_uses_only_both_original_report_files",
    "test_linux_member_copy.LinuxMemberCopy.test_imports_only_the_held_raw_successor_with_native_noreplace",
    "test_linux_member_copy.LinuxMemberCopy.test_chosen_folder_can_make_a_first_standalone_zip_without_registering",
    "test_desktop_commit.ExplicitDesktopCommit.test_reviewed_commit_takes_the_original_report_before_close_and_freezes_exact_content",
    "test_desktop_commit.ExplicitDesktopCommit.test_commit_publishes_exact_new_branch_slot_and_only_observes_the_final_receipt",
    "test_snapshot_mount.SnapshotReads",
    "test_snapshot_mount.PointSelection",
    "test_desktop_mount.DesktopMount.test_real_inherited_snapshot_and_ancestors_serve_the_displayed_node_without_opening_its_name",
    "test_desktop_mount.DesktopMount.test_mountinfo_requires_one_exact_escaped_point_old_parent_tag_type_and_read_only_flags",
    "test_linux_package",
    "test_container_plan",
    "test_container_spool",
    "test_container_outbox",
    "test_container_management",
    "test_container_http",
    "test_container_https",
    "test_container_https_read",
)
VIRTUAL_QT = (
    "test_desktop_commit.ExplicitDesktopCommit.test_window_cancels_without_writing_then_commits_once_after_fresh_consent",
    "test_desktop_commit.ExplicitDesktopCommit.test_review_button_transfers_original_proof_to_existing_manual_confirmation",
    "test_linux_visibility.LinuxVisibility.test_only_explicit_selected_missing_row_can_request_window_recovery",
    "test_linux_visibility.LinuxVisibility.test_real_worktree_text_dialog_drops_a_late_read_on_close",
    "test_linux_member_copy.LinuxMemberCopy.test_folder_snapshot_uses_actual_menu_and_explicit_new_path_confirmation",
    "test_desktop_mount.DesktopMount.test_qt_cancel_and_changed_selection_do_not_spawn_or_grant",
    "test_desktop_mount.DesktopMount.test_actual_window_keeps_cancel_enabled_with_ready_helper_and_an_unrelated_pending_job",
)


def require(condition, reason):
    if not condition:
        raise RuntimeError(reason)


def fixture_directory(spelling, phase):
    # Use the application's actual ordinary-user, original ancestry and mount gate.
    from zipworkspace.linux_authority import ProcView, owned_directory
    from zipworkspace.linux_mount import _ordinary_user
    from zipworkspace.private_freeze import local_mount

    root = Path(spelling)
    require(root.anchor == "/" and root.name and str(root) == spelling and ".." not in root.parts
            and not root.is_relative_to(PROJECT), "choose an existing exact private fixture root outside the source")
    with ExitStack() as held:
        proc = ProcView(held)
        uid = _ordinary_user(read_proc=proc.read)
        parent, _ = owned_directory(held, root, uid, proc.read)
        require(stat.S_IMODE(os.fstat(parent).st_mode) == 0o700, "fixture root must be an existing private 0700 directory")
        mount = local_mount(parent, read_proc=proc.read)
        os.mkdir(phase, 0o700, dir_fd=parent)
    scratch = str(root / phase)
    os.environ.update(TMPDIR=scratch, ZXS_LINUX_VISIBILITY_SCRATCH=scratch, ZXS_LINUX_COPY_SCRATCH=scratch)
    print(f"Admitted actual fixture mount ID {mount.decode('ascii')}; {phase}", flush=True)


def virtual_display():
    from PySide6.QtCore import qVersion
    from PySide6.QtWidgets import QApplication, QWidget

    require(os.environ.get("DISPLAY"), "the job must supply its virtual X11 display")
    os.environ.update(QT_QPA_PLATFORM="xcb", ZIP_WORKSPACE_GUI_TESTS="1")
    app = QApplication(["workspace-linux-qualification", "-platform", "xcb"])
    app.setQuitOnLastWindowClosed(False)
    require(app.platformName() == "xcb" and app.primaryScreen() is not None, "Qt did not select XCB; no fallback")
    probe = QWidget()
    probe.show()
    try:
        deadline = time.monotonic() + 5
        while True:
            app.processEvents()
            handle = probe.windowHandle()
            if handle is not None and handle.isExposed():
                break
            require(time.monotonic() < deadline, "Qt did not expose an XCB window on the virtual display")
            time.sleep(0.02)
    finally:
        probe.close()
    print(f"Qt {qVersion()}: XCB window exposed on Xvfb (virtual display, not a physical seat)", flush=True)
    return app


def https_imports():
    import ssl

    import certifi
    from zipworkspace.container_https import HttpsExchange

    # Constructor and trust initialization only: no socket, server or enrollment.
    exchange = HttpsExchange("https://workspace-ci.invalid")
    tls = exchange._tls()
    require(ssl.HAS_ALPN and tls.check_hostname and tls.verify_mode == ssl.CERT_REQUIRED,
            "the locked HTTPS runtime cannot validate a TLS hostname with ALPN")
    ca = Path(certifi.where()).resolve(strict=True)
    require(ca.is_relative_to(Path(sys.prefix).resolve()) and ca.is_file()
            and tls.cert_store_stats()["x509_ca"] > 0,
            "the locked wheel's CA data was not loaded from the isolated environment")
    print("Locked HTTP/2 prerequisites and CA bundle loaded; no requests made", flush=True)


def run_required(names):
    suite = unittest.defaultTestLoader.loadTestsFromNames(names)
    count = suite.countTestCases()
    require(count >= len(names), "required tests were not collected")
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    require(result.wasSuccessful() and result.testsRun == count and not result.skipped,
            f"failed, unrun or skipped required tests: {[test.id() for test, _ in result.skipped]}")
    print(f"{count} required source fixtures ran; no skips", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("filesystem", "virtual-qt", "https-imports"))
    parser.add_argument("scratch")
    options = parser.parse_args()
    require(sys.platform == "linux" and os.uname().machine == "x86_64" and sys.version_info[:3] == (3, 14, 10),
            "expected hosted Linux x86-64 CPython 3.14.10")
    sys.path[:0] = [str(PROJECT), str(PROJECT / "tests"), str(HERE.parent)]
    os.umask(0o077)
    fixture_directory(options.scratch, options.phase)
    if options.phase == "virtual-qt":
        app = virtual_display()
        run_required(VIRTUAL_QT)
        require(app.platformName() == "xcb", "Qt platform changed")
    elif options.phase == "https-imports":
        https_imports()
    else:
        run_required(FILESYSTEM)


if __name__ == "__main__":
    main()
