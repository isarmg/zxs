"""Opt-in, synthetic Workspace qualification on the two GitHub-hosted desktops."""
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import sysconfig
import tempfile
import time
import unittest

HERE = Path(__file__).resolve()
PROJECT = HERE.parents[2]
COMMON = (
    "test_headers.LocalHeader",
    "test_browser.Browse.test_borrowed_document_returns_verified_immediate_children_and_preserves_file",
    "test_browser.Browse.test_cli_does_not_show_even_a_first_page_until_whole_archive_is_verified",
    "test_plain_import.PlainPreview.test_cli_preview_is_explicit_and_default_refuses_before_any_scan_or_write",
    "test_comparison.FilterComparisonRows",
    "test_comparison.CompareVersions.test_two_explicit_files_from_different_repos_need_no_store_or_left_reopen",
    "test_comparison.CompareVersions.test_selected_text_difference_reads_only_each_original_exact_member",
    "test_desktop_archive.DesktopArchiveWidgets.test_exact_path_selects_original_parent_page_in_both_views_and_rejects_a_retired_dialog",
    "test_desktop_text_preview.DesktopTextPreview.test_first_open_cancelled_after_worker_return_releases_original_before_display",
)
WINDOWS = (
    "test_windows_folder.NativeLocalFolder.test_native_picker_reads_exact_default_stream_and_refuses_same_name_replacement",
    "test_windows_folder.NativeLocalFolder.test_fixed_native_report_reads_exact_nested_peer_from_held_zip_and_preserves_zone",
    "test_windows_snapshot.DataPlane",
)
MACOS = (
    "test_macos_read.ComparisonBoundary.test_real_local_volume_compares_exact_files_and_restores_both_thread_policies",
    "test_macos_read.ComparisonBoundary.test_real_apfs_pair_retains_originals_and_accepts_no_case_alias",
    "test_macos_snapshot_save.FreshFolderSnapshot.test_fresh_standard_zip_has_new_first_generation_and_preserves_empty_directory",
    "test_macos_snapshot_save.FreshFolderSnapshot.test_fresh_unmarked_snapshot_can_open_but_mark_preserving_zip_save_never_stages",
    "test_macos_snapshot.MacCallbacks.test_nfd_nfc_original_inode_case_and_no_synthetic_appledouble",
    "test_macos_snapshot.MacCallbacks.test_missing_retention_wrong_listed_raw_and_no_shared_display_cursor",
)


def require(ok, reason):
    if not ok:
        raise RuntimeError(reason)


def run(arguments, env, *, timeout=240):
    subprocess.run([str(arg) for arg in arguments], cwd=PROJECT, env=env, timeout=timeout, check=True)


def python_args(executable):
    return [executable, "-I", "-B", "-X", "utf8"]


def prepare_environment(root, mac, env):
    venv, wheels = root / "venv", root / "wheels"
    run([*python_args(sys.executable), "-m", "venv", venv], env)
    executable = venv / ("bin/python" if mac else "Scripts/python.exe")
    lock = PROJECT / "packaging" / ("macos-arm64" if mac else "windows-amd64") / "requirements.txt"
    if not mac:
        # Server runs source only. It needs Qt, not the Win11 packaging dependencies.
        lines = [line for line in lock.read_text("ascii").splitlines()
                 if line.startswith(("pyside6-essentials==", "shiboken6=="))]
        require(len(lines) == 2, "the Windows lock must supply both Qt wheels")
        lock = root / "qt-requirements.txt"
        lock.write_text("\n".join(lines) + "\n", encoding="ascii")
    wheels.mkdir(mode=0o700)
    pip = [*python_args(executable), "-m", "pip", "--isolated", "--disable-pip-version-check", "--no-input"]
    pinned = ["--no-deps", "--only-binary=:all:", "--require-hashes", "--no-cache-dir", "-r", lock]
    run([*pip, "download", "--index-url", "https://pypi.org/simple", "--dest", wheels, *pinned], env, timeout=480)
    run([*pip, "install", "--no-index", "--find-links", wheels, *pinned], env)
    run([*pip, "check"], env)
    return executable


def expose_sources():
    # Explicit local roots allow isolated Python without an editable install or network build dependencies.
    sys.path[:0] = [str(PROJECT), str(PROJECT / "tests"), str(HERE.parent)]


def run_required_suite(mac):
    expose_sources()
    names = COMMON + (MACOS if mac else WINDOWS)
    suite = unittest.defaultTestLoader.loadTestsFromNames(names)
    count = suite.countTestCases()
    require(count >= len(names), f"expected at least {len(names)} required tests, found {count}")
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    require(result.wasSuccessful() and result.testsRun == count and not result.skipped,
            f"mandatory tests failed, did not run, or skipped: {[case.id() for case, _ in result.skipped]}")
    print(f"{count} required source/NTFS-or-APFS tests ran; no skips.", flush=True)


def qt_smoke(mac):
    expose_sources()
    from PySide6.QtCore import QCoreApplication, QEvent, Qt, qVersion
    from PySide6.QtTest import QTest
    from PySide6.QtWidgets import QApplication, QDialog, QDialogButtonBox
    from test_desktop_archive import write_snapshot
    from zipworkspace.desktop.window import DesktopWindows

    native = "cocoa" if mac else "windows"
    app = QApplication(["zip-workspace-ci", "-platform", native])
    app.setQuitOnLastWindowClosed(False)
    require(app.platformName() == native and app.primaryScreen() is not None,
            f"no native Qt screen ({native}); no headless fallback")

    def wait_for(predicate, label):
        end = time.monotonic() + 15
        while time.monotonic() < end:
            app.processEvents()
            QCoreApplication.sendPostedEvents(None, QEvent.Type.DeferredDelete.value)
            if predicate():
                return
            time.sleep(0.02)
        raise RuntimeError("timed out: " + label)

    def exposed(widget):
        handle = widget.windowHandle()
        return widget.isVisible() and handle is not None and handle.isExposed()

    windows = DesktopWindows()

    def offer(path):
        require(windows.offer_snapshot(str(path)), "native window did not accept the offered path")
        window, = windows.windows.values()
        dialog = window.findChild(QDialog, "open-offered-snapshot")
        require(dialog is not None, "no external-open confirmation")
        wait_for(lambda: exposed(window) and exposed(dialog), "native confirmation exposed")
        require(window.pending is None and window.archive.selection is None, "offer read before consent")
        return window, dialog.findChild(QDialogButtonBox)

    with tempfile.TemporaryDirectory(prefix="desktop-window-") as scratch:
        root = Path(scratch).resolve(strict=True)
        path = root / "合成 workspace.zxs"
        marker = write_snapshot(path, entries={"A/note.txt": b"other\n", "B/note.txt": b"selected\n", "empty/": b""})
        source = path.read_bytes()

        _, box = offer(path)
        QTest.mouseClick(box.button(QDialogButtonBox.StandardButton.Cancel), Qt.MouseButton.LeftButton)
        wait_for(lambda: not windows.windows and windows.request is None, "cancel closes only the offered window")
        window, box = offer(path)
        QTest.mouseClick(box.button(QDialogButtonBox.StandardButton.Open), Qt.MouseButton.LeftButton)
        wait_for(lambda: window.pending is None, "verification worker and Qt event delivery")
        pane = window.archive
        selected = pane.selection
        require(selected is not None and selected.browser.commit == marker,
                f"selected archive was not verified: {window.note.text()}")
        require([row.name for row in pane.file_page.entries] == ["A", "B", "empty"], "wrong root entries")
        require(selected.browser.list_directory("empty").entries == (), "empty directory is not preserved")
        require(pane.mount is None and not pane.checkout.start.isEnabled(), "unexpected managed write or mount entry")

        QTest.mouseClick(pane.find, Qt.MouseButton.LeftButton)
        dialog = pane.search_dialog
        require(dialog is not None, "path locator did not open")
        wait_for(lambda: exposed(dialog), "native locator exposed")
        dialog.exact.setText("B/note.txt")
        QTest.keyClick(dialog.exact, Qt.Key.Key_Return)
        wait_for(lambda: pane.search_dialog is None, "path selected")
        require(pane.directory == "B" and pane.files.item() is selected.browser.entry("B", "note.txt"),
                "exact path did not select its original row")
        window.close()
        wait_for(lambda: not windows.windows, "native window closed")
        require(selected.held.closed and path.read_bytes() == source and list(root.iterdir()) == [path],
                "original handle or synthetic source left changed")
    print(f"Qt {qVersion()}: {native} windows exposed; cancel, verify, exact path and close passed.", flush=True)


def macos_candidate(root, executable, env):
    out = root / "macos-candidate"
    run([*python_args(executable), PROJECT / "packaging/macos-arm64/build.py",
         "--allow-local-adhoc", "--output", out], env, timeout=720)
    record = json.loads((out / "candidate.json").read_text("utf-8"))
    require(record["platform"] == "macos-arm64" and record["signature"] == "ad-hoc (local only)"
            and record["bundle"] == "ZipWorkspace.app"
            and Path(record["file"]).name == record["file"], "unexpected macOS candidate receipt")
    archive = out / record["file"]
    with archive.open("rb") as source:
        require(hashlib.file_digest(source, "sha256").hexdigest() == record["sha256"]
                and archive.stat().st_size == record["bytes"], "candidate ZIP differs from its receipt")
    unpacked = root / "unpacked"
    unpacked.mkdir(mode=0o700)
    run(["/usr/bin/ditto", "-x", "-k", archive, unpacked], env)
    app = unpacked / "ZipWorkspace.app"
    run(["/usr/bin/codesign", "--verify", "--all-architectures", "--deep", "--strict", app], env)
    executable = app / "Contents/MacOS/ZipWorkspace"
    arch = subprocess.check_output(["/usr/bin/lipo", "-archs", str(executable)], env=env, text=True, timeout=30)
    require(arch.strip() == "arm64", "unpacked executable is not a single arm64 slice")
    print(f"macOS local ad-hoc package built, ZIP and unpacked signature/slice checked: {record['sha256']}", flush=True)


def hosted_run():
    require(os.environ.get("GITHUB_ACTIONS") == "true", "run only in the dedicated GitHub-hosted job")
    mac = sys.platform == "darwin"
    if mac:
        require(platform.mac_ver()[0].partition(".")[0] == "26" and platform.machine() == "arm64"
                and os.getuid() == os.geteuid() != 0, "expected an ordinary user on macOS 26 arm64")
    else:
        require(sys.platform == "win32" and sysconfig.get_platform() == "win-amd64"
                and sys.getwindowsversion().product_type in (2, 3), "expected the x64 Windows Server hosted job")
    require(sys.implementation.name == "cpython" and sys.version_info[:3] == (3, 14, 10)
            and sys.version_info.releaselevel == "final" and not sysconfig.get_config_var("Py_GIL_DISABLED"),
            "expected CPython 3.14.10 with the GIL")
    print(f"runner: {platform.platform()}; CPython {platform.python_version()}", flush=True)
    if not mac:
        print("Windows scope: x64 Server/NTFS source and GUI; Win11 portable package and ordinary-account privileges not qualified.", flush=True)
    print("Both platforms: no desktop volume, driver, mounted share, managed checkout or worktree commit.", flush=True)
    # Do not let an ambient opt-in choose a fixture, external SDK, pip config, or Qt backend.
    prefix = ("ZIP_WORKSPACE_", "ZIPWORKSPACE_", "ZXS_", "FUSE_", "WINFSP_", "PIP_", "QT_", "QML", "PYTHON", "DYLD_")
    env = {name: value for name, value in os.environ.items()
           if not name.upper().startswith(prefix) and name.upper() not in ("LD_PRELOAD", "VIRTUAL_ENV")}
    temp = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="workspace-desktop-ci-", dir=temp) as scratch:
        root = Path(scratch)
        tests = root / "tests"
        tests.mkdir(mode=0o700)
        env.update(PIP_CONFIG_FILE=os.devnull, TMPDIR=str(tests), TMP=str(tests), TEMP=str(tests),
                   ZIP_WORKSPACE_GUI_TESTS="1", QT_QPA_PLATFORM="cocoa" if mac else "windows")
        if mac:
            env["ZIPWORKSPACE_APFS_DISPOSABLE_PRIVATE_DIRECTORY"] = str(tests)
        executable = prepare_environment(root, mac, env)
        run([*python_args(executable), HERE, "source-suite"], env)
        run([*python_args(executable), HERE, "native-window"], env)
        if mac:
            macos_candidate(root, executable, env)


if __name__ == "__main__":
    require(os.environ.get("GITHUB_ACTIONS") == "true", "this is an opt-in GitHub-hosted qualification")
    if sys.argv[1:] == ["source-suite"]:
        run_required_suite(sys.platform == "darwin")
    elif sys.argv[1:] == ["native-window"]:
        qt_smoke(sys.platform == "darwin")
    elif len(sys.argv) == 1:
        hosted_run()
    else:
        raise SystemExit("unexpected qualification arguments")
