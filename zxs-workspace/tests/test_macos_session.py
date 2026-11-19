"""No kernel access. The optional actual SDK trace is off unless separately enabled on a qualified Mac."""
import os
from pathlib import Path
import sys
import tempfile
from threading import Event
import unittest
from unittest.mock import Mock, patch
import zipfile

from zipworkspace import _macos_session as session
from zipworkspace.desktop.selection import Selection
from zipworkspace.model import Rejected

OPT = "ONE_OFFLINE_GENERIC_SESSION_NO_MOUNT_OR_DEVICE"


class Admission(unittest.TestCase):
    def test_no_consent_and_cancel_do_not_choose_folder_or_load_extension(self):
        with patch.object(session.sys, "platform", "darwin"), patch.object(session.Folder, "choose") as choose:
            event = Event()
            with self.assertRaises(Rejected): session.observe(None, "/unused", event)
            event.set()
            with self.assertRaises(InterruptedError): session.observe(None, "/unused", event, consent=True)
            choose.assert_not_called()

    def test_other_marker_is_not_native_input(self):
        selection = Mock()
        with patch.object(session.sys, "platform", "darwin"), patch.object(session.Folder, "choose") as choose:
            with self.assertRaises(Rejected): session.observe(selection, "/unused", Event(), consent=True)
            choose.assert_not_called()


@unittest.skipUnless(sys.platform == "darwin", "ordinary user, resident local APFS, private no-ACL temporary directory")
class FixedInputs(unittest.TestCase):
    def setUp(self):
        base = self.enterContext(tempfile.TemporaryDirectory())
        path = Path(base) / "example.zxs"
        with zipfile.ZipFile(path, "w") as doc:
            doc.writestr(session.NAME, session.BODY)
            doc.comment = session.synthetic_marker().encode()
        self.selected = Selection.open(path, retain=True)
        self.addCleanup(self.selected.release_display)
        self.point = Path(base)/"empty"
        self.point.mkdir(mode=0o700)
        self.stop = Event()

    def observe(self):
        return session.observe(self.selected, str(self.point), self.stop, consent=True)

    def test_nonempty_selected_point_refuses_without_writing_or_opening_sdk(self):
        sentinel = self.point/"unchanged"
        sentinel.write_bytes(b"ordinary sentinel")
        with patch.object(session, "_installed_release", side_effect=AssertionError("no SDK")):
            with self.assertRaises(Rejected): self.observe()
        self.assertEqual(sentinel.read_bytes(), b"ordinary sentinel")

    def test_selection_replaced_during_index_cannot_be_admitted_as_original_point(self):
        moved = self.point.with_name("previous")
        actual = os.pread
        def exchange(fd, n, at):
            if not moved.exists():
                self.point.rename(moved)
                self.point.mkdir(mode=0o700)
            return actual(fd, n, at)
        with patch("os.pread", side_effect=exchange), patch.object(session, "_installed_release") as install:
            with self.assertRaises(Rejected): self.observe()
            install.assert_not_called()
        self.assertEqual(list(self.point.iterdir()), [])

    def test_cancellation_during_one_selected_index_never_calls_native(self):
        actual = os.pread
        def cancel(fd, n, at):
            self.stop.set()
            return actual(fd, n, at)
        with patch("os.pread", side_effect=cancel), patch.object(session, "_installed_release") as install:
            with self.assertRaises(InterruptedError): self.observe()
            install.assert_not_called()
        self.assertEqual(list(self.point.iterdir()), [])

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_MACFUSE_SESSION") == OPT,
                         "separate consent and trusted prebuilt macFUSE 5.5.0 qualification module required")
    def test_one_native_session_reads_original_and_cancels_a_second_without_mounting(self):
        self.assertIn("未挂载", self.observe())
        self.assertEqual(list(self.point.iterdir()), [])
        original = session._Dispatch.open
        def open_then_cancel(hooks, path, flags):
            handle = original(hooks, path, flags)
            self.stop.set()
            return handle
        release = session._Dispatch.release
        seen = []
        def releasing(hooks, path, handle):
            seen.append((path, handle))
            release(hooks, path, handle)
            raise RuntimeError("late cleanup error must not replace cancellation")
        with patch.object(session._Dispatch, "open", open_then_cancel), patch.object(session._Dispatch, "release", releasing):
            with self.assertRaises(InterruptedError): self.observe()
        self.assertEqual(len(seen), 1)
        self.assertEqual(seen[0][0], "/probe.txt")
        self.assertGreater(seen[0][1], 0)
        self.assertEqual(list(self.point.iterdir()), [])

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_MACFUSE_SESSION") == OPT,
                         "same separately opted-in native module; test data only")
    def test_native_observation_refuses_a_same_length_wrong_reply(self):
        real = session._Dispatch.read
        def wrong(hooks, path, size, off, ino):
            actual = real(hooks, path, size, off, ino)
            return b"!" + actual[1:]
        with patch.object(session._Dispatch, "read", wrong):
            with self.assertRaises(RuntimeError): self.observe()
        self.assertEqual(list(self.point.iterdir()), [])
