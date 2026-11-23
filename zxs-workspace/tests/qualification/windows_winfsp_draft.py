"""Explicit native WinFsp callback fixture with synthetic R3 bytes; does not create any OS volume."""
from pathlib import Path
import sys
import unittest

if __name__ == "__main__":
    here = Path(__file__).resolve()
    sys.path[:0] = [str(here.parents[2]), str(here.parent)]

from windows_draft_qualification import LocalWorkingFile
from windows_node_qualification import ORIGIN, TARGET
from zipworkspace._windows_access import NativeSecurity
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
        case = FileCallbacks("test_exact_file_through_native_sdk_callbacks", root, spelling, security, actor, profile, zone)
        result = unittest.TextTestRunner(verbosity=2).run(unittest.TestSuite([case]))
        return int(not (result.wasSuccessful() and result.testsRun == 1 and not result.skipped))


if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1].startswith("-"):
        raise SystemExit("Supply one existing disposable ordinary-user private NTFS directory.")
    raise SystemExit(qualify(Path(sys.argv[1])))
