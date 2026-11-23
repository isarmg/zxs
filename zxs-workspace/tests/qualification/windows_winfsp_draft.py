"""Explicit native WinFsp callback fixture with synthetic R3 bytes; does not create any OS volume."""
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

if __name__ == "__main__":
    here = Path(__file__).resolve()
    sys.path[:0] = [str(here.parents[2]), str(here.parent)]

from windows_draft_qualification import LocalWorkingFile
from windows_node_qualification import ORIGIN, TARGET
from zipworkspace._windows_access import NativeSecurity
from zipworkspace._windows_node import _remote_mark
from zipworkspace._windows_output import PinnedParent
from zipworkspace.desktop.container_windows_draft import _effective_zone
from zipworkspace.desktop.container_windows_fileplane import WindowsDraftFilePlane
from zipworkspace.model import Rejected


class FileCallbacks(LocalWorkingFile):
    def test_exact_file_through_native_sdk_callbacks(self):
        from _workspace_winfsp_draft_io import check
        choice, _ = self.choose_draft(b"seed", max_bytes=32)
        location = choice.local_file
        with choice.begin(confirmed=True) as draft:
            for name in ("CON", "nul .exe", "../x", "x:stream", "x.", "\u0000", "a" * 256):
                with self.assertRaises(Rejected):
                    WindowsDraftFilePlane(draft, name, confirmed=True)
            plane = WindowsDraftFilePlane(draft, "review.bin", confirmed=True)
            check(plane)
            with self.assertRaises(Rejected):
                draft.review(confirmed=True)  # No actual WinFsp detach evidence was issued.
        self.assertEqual(Path(location).read_bytes(), b"sXQ")
        self.assertEqual(self.requests, [TARGET])


    def test_original_namespace_and_attachment(self):
        from _workspace_winfsp_draft_io import check_namespace
        choice, _ = self.choose_draft(b"seed", max_bytes=32)
        location = choice.local_file
        with choice.begin(confirmed=True) as draft:
            plane = WindowsDraftFilePlane(draft, "review.bin", confirmed=True)
            with self.assertRaises(Rejected):
                WindowsDraftFilePlane(draft, "a" * 234, confirmed=True).bind(object(), True)
            check_namespace(plane, _remote_mark(ORIGIN + TARGET, _effective_zone(ORIGIN + TARGET)))
            with self.assertRaises(Rejected):
                draft.review(confirmed=True)
        self.assertEqual(Path(location).read_bytes(), b"Weed")
        self.assertEqual(self.requests, [TARGET])


    def test_scratch_barrier_failure_does_not_displace_the_selected_primary(self):
        # Model only: real held NTFS and provenance; no driver, mount or accepted detach.
        choice, _ = self.choose_draft(b"seed", max_bytes=16)
        with choice.begin(confirmed=True) as draft:
            plane = WindowsDraftFilePlane(draft, "review.bin", confirmed=True, replacement=True)
            lease = object()
            plane.bind(lease, True, True)
            scratch = plane.retained_scratch
            try:
                writer, info = plane.create_scratch("\\editor.tmp", 2 << 24, 2)
                self.assertEqual((info[0], info[-1]), (0, 3))
                self.assertEqual(plane.write(writer, 0, b"new", False, False)[0], 3)
                plane.flush(writer)
                plane.close_file(writer)
                rename, _ = plane.open_file("\\editor.tmp", 1 << 24, 0x10000)
                old, info = plane.open_file("\\review.bin", 1 << 24, 1)
                self.assertEqual((info[0], info[-1]), (4, 2))
                with self.assertRaises(BlockingIOError):
                    plane.replace(rename, "\\editor.tmp", "\\review.bin")
                self.assertEqual(plane.read(old, 0, 16), b"seed")
                plane.close_file(old)

                def lost_barrier(h):
                    if h is draft._scratch.handle:
                        raise OSError("synthetic scratch flush failure")
                    return flush(h)
                api = draft._home.api
                flush = api.flush
                with patch.object(api, "flush", lost_barrier), self.assertRaises(OSError):
                    plane.replace(rename, "\\editor.tmp", "\\review.bin")
                old, info = plane.open_file("\\review.bin", 1 << 24, 1)
                self.assertEqual((info[-1], plane.read(old, 0, 16)), (2, b"seed"))
                self.assertEqual(draft._file_id, draft._initial.file_id)
                self.assertIsNone(plane.replacement_witness())
                plane.close_file(old)
                plane.close_file(rename)
                self.assertEqual(plane.stop_opening(), 0)
            finally:
                plane.abandon(lease)
            with self.assertRaises(Rejected):
                draft.review(confirmed=True)
        self.assertEqual(Path(choice.local_file).read_bytes(), b"seed")
        self.assertEqual(Path(scratch).read_bytes(), b"new")
        self.assertEqual(self.requests, [TARGET])


def qualify(spelling):
    if sys.platform != "win32":
        raise SystemExit("Requires a real ordinary Windows NTFS user and the separate trusted native extension.")
    with PinnedParent(spelling, output=True) as root:
        security = NativeSecurity()
        actor = security.caller()
        profile = security.read(root.handle, actor, directory=True)
        zone = _effective_zone(ORIGIN + TARGET)
        if zone not in (3, 4):
            raise SystemExit("No qualifying existing policy for the inert synthetic URL.")
        names = ("test_exact_file_through_native_sdk_callbacks", "test_original_namespace_and_attachment",
                 "test_scratch_barrier_failure_does_not_displace_the_selected_primary")
        suite = unittest.TestSuite(FileCallbacks(name, root, spelling, security, actor, profile, zone) for name in names)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return int(not (result.wasSuccessful() and result.testsRun == len(names) and not result.skipped))


if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1].startswith("-"):
        raise SystemExit("Supply one existing disposable ordinary-user private NTFS directory.")
    raise SystemExit(qualify(Path(sys.argv[1])))
