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
from zipworkspace.model import Rejected
from zipworkspace.windows_nt import I32, P, U32, _function

CONSENT = "--consent-create-volume-start-installed-winfsp-and-mount-private-directory"


class ActualDirectory(LocalWorkingFile):
    @contextmanager
    def handle(self, path, rights):
        k = c.WinDLL("kernel32.dll", use_last_error=True, winmode=0x800)
        open_ = _function(k, "CreateFileW", P, c.c_wchar_p, U32, U32, P, U32, U32, P)
        close = _function(k, "CloseHandle", I32, P)
        h = open_(str(path), rights, 7, None, 3, 0x80, None)
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
                with self.handle(path + ":Zone.Identifier", 0x80000000) as zone:
                    self.assertTrue(read(zone, buf, len(buf), c.byref(n), None))
                    self.assertEqual(bytes(buf)[:n.value], expected)
                with self.assertRaises(OSError):
                    with self.handle(path + ":Zone.Identifier", 2):
                        self.fail("the actual native provenance stream granted write")
                with self.handle(path, 2) as editor:
                    self.assertTrue(write(editor, b"N", 1, c.byref(n), None))
                    self.assertEqual(n.value, 1)
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
                self.assertEqual((review.length, review.sha256), (4, sha256(b"NWed").hexdigest()))
                self.assertFalse(Path(location).exists())
            finally:
                if not job.done():
                    owner.abandon(confirmed=True)
        self.assertEqual(self.requests, [TARGET])

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


def qualify(spelling):
    if sys.platform != "win32":
        raise SystemExit("Requires a separately trusted installed WinFsp 2.1 x64 SDK/DLL and disposable native Windows.")
    with PinnedParent(spelling, output=True) as root:
        security = NativeSecurity()
        actor = security.caller()
        profile = security.read(root.handle, actor, directory=True)
        zone = _effective_zone(ORIGIN + TARGET)
        if zone not in (3, 4):
            raise SystemExit("No existing qualifying attachment policy for the synthetic URL.")
        names = ("test_edit_through_os_then_wait_for_original_close", "test_abort_never_certifies_or_submits")
        suite = unittest.TestSuite(ActualDirectory(name, root, spelling, security, actor, profile, zone) for name in names)
        r = unittest.TextTestRunner(verbosity=2).run(suite)
        return int(not (r.wasSuccessful() and r.testsRun == len(names) and not r.skipped))


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] != CONSENT or sys.argv[2].startswith("-"):
        raise SystemExit(f"Explicitly consent with {CONSENT} and an existing disposable private NTFS directory.")
    raise SystemExit(qualify(Path(sys.argv[2])))
