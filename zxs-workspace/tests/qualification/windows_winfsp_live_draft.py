"""Separate real-driver qualification only. Explicit consent required; no installers, sockets or Submit."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import ctypes as c
from hashlib import sha256
from pathlib import Path
import sys
import time
import unittest

if __name__ == "__main__":
    here = Path(__file__).resolve()
    sys.path[:0] = [str(here.parents[2]), str(here.parent)]

from windows_draft_qualification import LocalWorkingFile
from windows_node_qualification import ORIGIN, TARGET
from zipworkspace._windows_access import NativeSecurity
from zipworkspace._windows_node import _effective_zone, _remote_mark
from zipworkspace._windows_output import PinnedParent
from zipworkspace.desktop.container_windows_mount import WindowsPrivateDraftMount
from zipworkspace.desktop.container_windows_save import open_original_windows_save
from zipworkspace.model import Rejected
from zipworkspace.windows_nt import I32, P, U32, _function

CONSENT = "--consent-create-volume-start-installed-winfsp-and-mount-private-directory"
CONSENT_REPLACE = "--consent-live-winfsp-scratch-replace-and-retain-both-private-files"
CONSENT_QSAVE = "--consent-live-winfsp-qt-save-and-retain-both-private-files"


class FileRenameInfo(c.Structure):
    _fields_ = [("replace", U32), ("root", P), ("length", U32), ("first", c.c_wchar)]


def classic_replace(k, source, target, replace=True):
    update = _function(k, "SetFileInformationByHandle", I32, P, I32, P, U32)
    target = Path(target)
    if not target.is_absolute():
        raise AssertionError("use the exact mounted absolute destination, not CWD-relative paths")
    raw = str(target).encode("utf-16-le")
    buf = c.create_string_buffer(c.sizeof(FileRenameInfo) + len(raw))
    fields = FileRenameInfo.from_buffer(buf)
    fields.replace, fields.root, fields.length = int(replace), None, len(raw)
    c.memmove(c.addressof(buf) + FileRenameInfo.first.offset, raw, len(raw))
    return bool(update(source, 3, buf, len(buf)))  # FileRenameInfo, not FileRenameInfoEx.


class ActualDirectory(LocalWorkingFile):
    @contextmanager
    def handle(self, path, rights, *, disposition=3, flags=0x80, share=7):
        k = c.WinDLL("kernel32.dll", use_last_error=True, winmode=0x800)
        open_ = _function(k, "CreateFileW", P, c.c_wchar_p, U32, U32, P, U32, U32, P)
        close = _function(k, "CloseHandle", I32, P)
        h = open_(str(path), rights, share, None, disposition, flags, None)
        if h in (None, c.c_void_p(-1).value):
            raise c.WinError(c.get_last_error())
        try:
            yield h
        finally:
            if not close(h):
                raise c.WinError(c.get_last_error())

    def test_edit_through_os_then_wait_for_original_close(self):
        choice, _ = self.choose_draft(b"seed", max_bytes=1024*1024)
        k = c.WinDLL("kernel32.dll", use_last_error=True, winmode=0x800)
        read = _function(k, "ReadFile", I32, P, P, U32, c.POINTER(U32), P)
        write = _function(k, "WriteFile", I32, P, P, U32, c.POINTER(U32), P)
        flush = _function(k, "FlushFileBuffers", I32, P)
        seek = _function(k, "SetFilePointerEx", I32, P, c.c_int64, P, U32)
        end = _function(k, "SetEndOfFile", I32, P)
        expected = _remote_mark(ORIGIN + TARGET, self.zone)
        with choice.begin(confirmed=True) as draft, ThreadPoolExecutor(max_workers=1) as worker:
            owner = WindowsPrivateDraftMount(draft, "review.bin", confirmed=True)
            job = worker.submit(owner.run, allow_installed_driver_start=True, allow_directory_mount=True)
            try:
                location = owner.wait_location(30)
                self.assertIsNotNone(location, job.result() if job.done() else "driver observation timed out")
                path = str(Path(location) / "review.bin")
                self.assertEqual([p.name for p in Path(location).iterdir()], ["review.bin"])
                buf, n = c.create_string_buffer(2048), U32()
                for rights in (1, 2):
                    with self.assertRaises(OSError), self.handle(path, rights, flags=0x00200000):
                        self.fail("no-follow provenance base open granted data access")
                with self.handle(path + ":Zone.Identifier", 0x80000000) as zone:
                    self.assertTrue(read(zone, buf, len(buf), c.byref(n), None))
                    self.assertEqual(bytes(buf)[:n.value], expected)
                with self.assertRaises(OSError):
                    with self.handle(path + ":Zone.Identifier", 2):
                        self.fail("the actual native provenance stream granted write")
                with self.handle(location, 1, flags=0x02000000):  # FILE_FLAG_BACKUP_SEMANTICS, ordinary root list only.
                    pass
                with self.handle(path, 0xc0000000, disposition=4):  # OPEN_ALWAYS on an EXISTING file does not create.
                    pass
                for rejected, access, disposition in ((str(Path(location) / "other"), 0xc0000000, 4),
                        (path, 0xc0000000, 2), (path, 0x10000, 3), (path, 0x20, 3),
                        (path + ":Zone.Identifier", 0x40000000, 3)):
                    with self.subTest(rejected=rejected, access=access, disposition=disposition), self.assertRaises(OSError):
                        with self.handle(rejected, access, disposition=disposition):
                            self.fail("unexpected namespace, overwrite or access grant")
                with self.handle(path, 0xc0000000) as editor:  # Ordinary GENERIC_READ | GENERIC_WRITE, OPEN_EXISTING.
                    self.assertTrue(read(editor, buf, 4, c.byref(n), None))
                    self.assertEqual((n.value, bytes(buf)[:4]), (4, b"seed"))
                    self.assertTrue(seek(editor, 3, None, 0))
                    self.assertTrue(end(editor))
                    self.assertTrue(seek(editor, 0, None, 0))
                    self.assertTrue(write(editor, b"N", 1, c.byref(n), None))
                    self.assertEqual(n.value, 1)
                    with self.handle(path + ":Zone.Identifier", 0x80000000) as zone:
                        self.assertTrue(read(zone, buf, len(buf), c.byref(n), None))
                        self.assertEqual(bytes(buf)[:n.value], expected)
                    owner.stop()
                    until = time.monotonic() + 15
                    while owner.remaining is None and not job.done() and time.monotonic() < until:
                        time.sleep(0.05)
                    self.assertIsNotNone(owner.remaining)
                    self.assertGreaterEqual(owner.remaining, 1)
                    self.assertFalse(job.done())
                    with self.assertRaises(OSError):
                        with self.handle(path, 1):
                            self.fail("new data open admitted during genuine drain")
                    self.assertTrue(write(editor, b"W", 1, c.byref(n), None))
                    self.assertTrue(flush(editor))
                report = job.result(timeout=30)
                self.assertEqual(report["outcome"], "original-detached", report)
                self.assertEqual(report["drained"], 1)
                review = draft.review(confirmed=True)
                self.assertEqual((review.length, review.sha256), (3, sha256(b"NWe").hexdigest()))
                self.assertFalse(Path(location).exists())
            finally:
                if not job.done():
                    owner.abandon(confirmed=True)
        self.assertEqual(self.requests, [TARGET])

    def _wait_closed(self, owner):
        until = time.monotonic() + 15
        while time.monotonic() < until:
            with owner._plane._lock:  # Test observation only; never release on behalf of the kernel.
                if not owner._plane._rows:
                    return
            time.sleep(0.05)
        self.fail("original native final Close not delivered")

    def _scratch_campaign(self, successful):
        before, after = b"seed", b"full exact editor save"
        choice, _ = self.choose_draft(before, max_bytes=64)
        k = c.WinDLL("kernel32.dll", use_last_error=True, winmode=0x800)
        write = _function(k, "WriteFile", I32, P, P, U32, c.POINTER(U32), P)
        flush = _function(k, "FlushFileBuffers", I32, P)
        rm = _function(k, "DeleteFileW", I32, c.c_wchar_p)
        update = _function(k, "SetFileInformationByHandle", I32, P, I32, P, U32)
        expected = _remote_mark(ORIGIN + TARGET, self.zone)
        with choice.begin(confirmed=True) as draft, ThreadPoolExecutor(max_workers=1) as worker:
            owner = WindowsPrivateDraftMount(draft, "review.bin", confirmed=True, replacement=True)
            job = worker.submit(owner.run, allow_installed_driver_start=True, allow_directory_mount=True)
            try:
                location = owner.wait_location(30)
                self.assertIsNotNone(location, job.result() if job.done() else "driver observation timed out")
                primary, scratch = Path(location) / "review.bin", Path(location) / "editor.tmp"
                with self.assertRaises(OSError), self.handle(str(Path(location) / "REVIEW.BIN"), 0xc0000000, disposition=1):
                    self.fail("case alias was opened as another original scratch file")
                with self.handle(str(scratch), 0xc0000000, disposition=1) as f:
                    n = U32()
                    self.assertTrue(write(f, after, len(after), c.byref(n), None))
                    self.assertEqual(n.value, len(after))
                    if successful:
                        self.assertTrue(flush(f), c.get_last_error())
                self.assertEqual({e.name for e in Path(location).iterdir()}, {"review.bin", "editor.tmp"})
                self.assertEqual(Path(str(scratch) + ":Zone.Identifier").read_bytes(), expected)
                self.assertFalse(rm(str(scratch)), "scratch DELETE requested via disposition must fail")
                for other, rights, disp, flags in (
                    (str(Path(location) / "second.tmp"), 0xc0000000, 1, 0x80),
                    (str(Path(location) / "other"), 0xc0000000, 4, 0x80),
                    (str(primary), 0x10000, 3, 0x80),
                    (str(primary), 0xc0000000, 2, 0x80),
                    (str(scratch), 0x10000, 3, 0x04000000),  # FILE_FLAG_DELETE_ON_CLOSE
                    (str(scratch) + ":Zone.Identifier", 2, 3, 0x80),
                    (str(scratch), 0x40000, 3, 0x80),  # WRITE_DAC
                ):
                    with self.subTest(name=other, rights=rights), self.assertRaises(OSError):
                        with self.handle(other, rights, disposition=disp, flags=flags):
                            self.fail("unexpected write, delete, security or name authority")
                self._wait_closed(owner)
                with self.handle(str(scratch), 0x10000 | 0x100000) as rename:
                    armed = c.c_ubyte(1)
                    self.assertFalse(update(rename, 4, c.byref(armed), c.sizeof(armed)), "FileDispositionInfo unexpectedly armed deletion")
                    # A failing replace must leave the previous physical and virtual body intact.
                    with self.handle(str(primary), 1):
                        self.assertFalse(classic_replace(k, rename, primary))
                    self._wait_only_rename(owner)
                    self.assertFalse(classic_replace(k, rename, Path(location) / "elsewhere.bin"))
                    self.assertFalse(classic_replace(k, rename, primary, replace=False))
                    if successful:
                        self.assertTrue(classic_replace(k, rename, primary), c.get_last_error())
                    else:
                        self.assertFalse(classic_replace(k, rename, primary))
                self.assertEqual(primary.read_bytes(), after if successful else before)
                self.assertEqual(Path(str(primary) + ":Zone.Identifier").read_bytes(), expected)
                self.assertEqual(scratch.exists(), not successful)
                self._wait_closed(owner)
                owner.stop()
                result = job.result(timeout=30)
                self.assertEqual(result["outcome"], "original-detached", result)
                self.assertFalse(Path(location).exists())
                if successful:
                    review = draft.review(confirmed=True)
                    self.assertEqual((review.local_file, review.sha256), (owner.retained_scratch, sha256(after).hexdigest()))
                    key = draft.prepare_save(review, confirmed=True)
                    with open_original_windows_save(choice._connection, self.staging, key, confirmed=True) as pending:
                        action, = pending.plan.actions
                        self.assertEqual((action.name, action.head, action.branch), (None, choice.source.head.id, "main"))
                        self.assertEqual(pending.plan.placements[0][1], Path(review.local_file).name)
                        with pending.body(action) as blocks:
                            self.assertEqual(b"".join(blocks), after)
                else:
                    with self.assertRaises(Rejected):
                        draft.review(confirmed=True)
            finally:
                if not job.done():
                    owner.abandon(confirmed=True)
        self.assertEqual(Path(choice.local_file).read_bytes(), before)
        self.assertEqual(Path(owner.retained_scratch).read_bytes(), after)
        self.assertEqual(Path(owner.retained_scratch + ":Zone.Identifier").read_bytes(), expected)
        self.assertEqual(self.requests, [TARGET])  # Not a real endpoint and no Submit, even after preparation.

    def _wait_only_rename(self, owner):
        until = time.monotonic() + 15
        while time.monotonic() < until:
            with owner._plane._lock:
                if len(owner._plane._rows) == 1:
                    return
            time.sleep(0.05)
        self.fail("other native contexts have not finally closed")

    def test_one_legacy_replace_then_current_full_body_only(self):
        self._scratch_campaign(True)

    def test_unflushed_scratch_cannot_replace_or_prepare(self):
        self._scratch_campaign(False)

    def _held_creator(self, owner, primary, scenario):
        k = c.WinDLL("kernel32.dll", use_last_error=True, winmode=0x800)
        write = _function(k, "WriteFile", I32, P, P, U32, c.POINTER(U32), P)
        flush = _function(k, "FlushFileBuffers", I32, P)
        seek = _function(k, "SetFilePointerEx", I32, P, c.c_int64, P, U32)
        end = _function(k, "SetEndOfFile", I32, P)
        update = _function(k, "SetFileInformationByHandle", I32, P, I32, P, U32)
        rw_delete = 0xc0010000  # GENERIC_READ | GENERIC_WRITE | DELETE, NOT FILE_ALL_ACCESS.
        scratch = primary.with_name("editor.tmp")
        for rights, share in ((rw_delete, 7), (0x10002, 0), (rw_delete | 0x40000, 0)):
            with self.subTest(rights=rights, share=share), self.assertRaises(OSError):
                with self.handle(scratch, rights, share=share, disposition=1):
                    self.fail("broad/nonexclusive create was accepted; it also spent the only slot")
        self.assertFalse(scratch.exists())
        with self.handle(scratch, rw_delete, share=0, disposition=1) as writer:
            n = U32()
            self.assertTrue(write(writer, b"whole", 5, c.byref(n), None))
            self.assertEqual(n.value, 5)
            self.assertFalse(classic_replace(k, writer, primary), "rename borrowed an implicit or missing writer flush")
            self.assertTrue(flush(writer), c.get_last_error())
            armed = c.c_ubyte(1)
            self.assertFalse(update(writer, 4, c.byref(armed), c.sizeof(armed)), "DELETE armed disposition")
            if scenario != "abandon":
                with self.handle(primary, 1):
                    self.assertFalse(classic_replace(k, writer, primary), "old primary still had a read handle")
                self._wait_only_rename(owner)
                self.assertFalse(classic_replace(k, writer, primary.with_name("different")))
                self.assertFalse(classic_replace(k, writer, primary, replace=False))
                self.assertTrue(classic_replace(k, writer, primary), c.get_last_error())
                # This HANDLE still refers to the promoted original. Late data and EOF must not
                # affect the earlier private primary or retain its pre-rename flush as evidence.
                self.assertTrue(seek(writer, 0, None, 0))
                self.assertTrue(write(writer, b"late", 4, c.byref(n), None))
                self.assertEqual(n.value, 4)
                self.assertTrue(end(writer))  # 5 -> 4, an actual EOF change.
                if scenario == "reflushed":
                    self.assertTrue(flush(writer), c.get_last_error())
        self._wait_closed(owner)
        if scenario == "abandon":
            for rights in (0x10000 | 0x100000, rw_delete):
                with self.assertRaises(OSError), self.handle(scratch, rights, share=0):
                    self.fail("a new FILE_OPEN adopted the closed sole creator")
        if scenario == "dirty-close":
            with self.handle(primary, 0x40000000) as later:
                self.assertTrue(flush(later))  # Not the creator: cannot cure an unsealed final Close.
        return scratch.name, b"whole" if scenario == "abandon" else b"late"

    def _qt_creator(self, primary, scenario):
        from PySide6.QtCore import QIODevice, QSaveFile, qVersion
        self.assertEqual(qVersion(), "6.11.2", "this separate fixture targets exactly Qt 6.11.2")
        out = QSaveFile(str(primary))
        out.setDirectWriteFallback(False)
        self.assertTrue(out.open(QIODevice.OpenModeFlag.WriteOnly), out.errorString())
        scratch, = {p.name for p in primary.parent.iterdir()} - {primary.name}
        self.assertEqual(out.write(b"whole"), 5)
        self.assertTrue(out.flush(), out.errorString())
        if scenario == "qt-cancel":
            out.cancelWriting()
            self.assertFalse(out.commit(), "cancel published a virtual replacement")
        else:
            self.assertTrue(out.commit(), out.errorString())
        return scratch, b"whole"

    def _creator_campaign(self, scenario):
        seed = b"seed"
        choice, _ = self.choose_draft(seed, max_bytes=64)
        expected_mark = _remote_mark(ORIGIN + TARGET, self.zone)
        published = scenario not in ("abandon", "qt-cancel")
        reviewable = published and scenario != "dirty-close"
        with choice.begin(confirmed=True) as draft, ThreadPoolExecutor(max_workers=1) as worker:
            owner = WindowsPrivateDraftMount(draft, "review.bin", confirmed=True, replacement=True)
            job = worker.submit(owner.run, allow_installed_driver_start=True, allow_directory_mount=True)
            try:
                location = owner.wait_location(30)
                self.assertIsNotNone(location, job.result() if job.done() else "driver observation timed out")
                primary = Path(location) / "review.bin"
                scratch, data = (self._qt_creator(primary, scenario) if scenario.startswith("qt-")
                                 else self._held_creator(owner, primary, scenario))
                self.assertEqual(primary.read_bytes(), data if published else seed)
                self.assertEqual(primary.with_name(scratch).exists(), not published)
                self.assertEqual(Path(str(primary) + ":Zone.Identifier").read_bytes(), expected_mark)
                self._wait_closed(owner)
                owner.stop()
                report = job.result(timeout=30)
                self.assertEqual((report["outcome"], report["drained"]), ("original-detached", 1), report)
                if reviewable:
                    review = draft.review(confirmed=True)
                    self.assertEqual((review.local_file, review.length, review.sha256),
                                     (owner.retained_scratch, len(data), sha256(data).hexdigest()))
                    # Still local: prepare only after a separate confirmation, freeze only the selected body.
                    key = draft.prepare_save(review, confirmed=True)
                    with open_original_windows_save(choice._connection, self.staging, key, confirmed=True) as pending:
                        action, = pending.plan.actions
                        self.assertEqual((action.name, action.head, action.branch), (None, choice.source.head.id, "main"))
                        with pending.body(action) as blocks:
                            self.assertEqual(b"".join(blocks), data)
                else:
                    with self.assertRaises(Rejected):
                        draft.review(confirmed=True)
            finally:
                if not job.done():
                    owner.abandon(confirmed=True)
        self.assertEqual(Path(choice.local_file).read_bytes(), seed)
        self.assertEqual(Path(owner.retained_scratch).read_bytes(), data)
        for path in (choice.local_file, owner.retained_scratch):
            self.assertEqual(Path(path + ":Zone.Identifier").read_bytes(), expected_mark)
        self.assertEqual(self.requests, [TARGET])  # Only initial fixture download; no upload even after freeze.

    def test_same_creator_flush_rename_late_write_and_reflush(self):
        self._creator_campaign("reflushed")

    def test_other_handle_cannot_cure_dirty_creator_close(self):
        self._creator_campaign("dirty-close")

    def test_closed_unpromoted_creator_cannot_be_reopened_for_rename(self):
        self._creator_campaign("abandon")

    def test_real_qsavefile_no_direct_fallback(self):
        self._creator_campaign("qt-save")

    def test_real_qsavefile_cancel_preserves_both_originals(self):
        self._creator_campaign("qt-cancel")

    def test_abort_never_certifies_or_submits(self):
        choice, _ = self.choose_draft(b"seed")
        with choice.begin(confirmed=True) as draft, ThreadPoolExecutor(max_workers=1) as worker:
            owner = WindowsPrivateDraftMount(draft, "review.bin", confirmed=True)
            job = worker.submit(owner.run, allow_installed_driver_start=True, allow_directory_mount=True)
            try:
                self.assertIsNotNone(owner.wait_location(30))
            finally:
                owner.abandon(confirmed=True)
            report = job.result(timeout=30)
            self.assertEqual(report["drained"], 0)
            with self.assertRaises(Rejected):
                draft.review(confirmed=True)
        self.assertEqual(Path(choice.local_file).read_bytes(), b"seed")
        self.assertEqual(self.requests, [TARGET])


def qualify(spelling, *, replacement=False, qsavefile=False):
    if sys.platform != "win32":
        raise SystemExit("Requires a separately trusted installed WinFsp 2.1 x64 SDK/DLL and disposable native Windows.")
    with PinnedParent(spelling, output=True) as root:
        security = NativeSecurity()
        actor = security.caller()
        profile = security.read(root.handle, actor, directory=True)
        zone = _effective_zone(ORIGIN + TARGET)
        if zone not in (3, 4):
            raise SystemExit("No existing qualifying attachment policy for the synthetic URL.")
        if qsavefile:
            names = ("test_real_qsavefile_no_direct_fallback", "test_real_qsavefile_cancel_preserves_both_originals")
        elif replacement:
            names = ("test_one_legacy_replace_then_current_full_body_only", "test_unflushed_scratch_cannot_replace_or_prepare",
                     "test_same_creator_flush_rename_late_write_and_reflush", "test_other_handle_cannot_cure_dirty_creator_close",
                     "test_closed_unpromoted_creator_cannot_be_reopened_for_rename")
        else:
            names = ("test_edit_through_os_then_wait_for_original_close", "test_abort_never_certifies_or_submits")
        suite = unittest.TestSuite(ActualDirectory(name, root, spelling, security, actor, profile, zone) for name in names)
        r = unittest.TextTestRunner(verbosity=2).run(suite)
        return int(not (r.wasSuccessful() and r.testsRun == len(names) and not r.skipped))


if __name__ == "__main__":
    modes = (CONSENT, CONSENT_REPLACE, CONSENT_QSAVE)
    if len(sys.argv) != 3 or sys.argv[1] not in modes or sys.argv[2].startswith("-"):
        raise SystemExit(f"Explicitly select one of {modes} and one existing disposable NTFS directory.")
    raise SystemExit(qualify(Path(sys.argv[2]), replacement=sys.argv[1] == CONSENT_REPLACE,
                             qsavefile=sys.argv[1] == CONSENT_QSAVE))
