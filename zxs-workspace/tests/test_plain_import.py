"""The release surface is a full ZIP preview, not a filesystem writer."""
from contextlib import ExitStack, contextmanager, redirect_stderr, redirect_stdout
from hashlib import sha256
from importlib.util import find_spec
import io
import json
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import plain_import as importer, worktree
from zipworkspace.cli import main
from zipworkspace.model import Rejected


@contextmanager
def no_output():
    real_open = os.open
    def read_only(path, flags, *args, **kwargs):
        if flags & (os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_TRUNC | os.O_APPEND):
            raise AssertionError("attempted writable open")
        return real_open(path, flags, *args, **kwargs)
    with ExitStack() as stack:
        stack.enter_context(patch.object(os, "open", side_effect=read_only))
        for action in ("mkdir", "rename", "replace", "unlink", "rmdir", "fsync"):
            stack.enter_context(patch.object(os, action, side_effect=AssertionError(action)))
        yield


class PlainPreview(unittest.TestCase):
    def setUp(self):
        area = tempfile.TemporaryDirectory(); self.addCleanup(area.cleanup)
        self.root = Path(area.name); self.archive = self.root/"example.zip"
        with zipfile.ZipFile(self.archive, "w", compression=zipfile.ZIP_STORED) as z:
            z.writestr("folder/note", b"original"); z.writestr("empty/", b"")
        self.parent = self.root/"output"; self.parent.mkdir(mode=0o700)
        self.dest = self.parent/"copy"

    def test_all_members_and_raw_digest_verified_with_no_extraction_or_cleanup(self):
        untouched = self.parent/".zip-checkout-stale"
        untouched.mkdir(); leaf = untouched/"existing"; leaf.write_bytes(b"not ours")
        original = leaf.stat()
        with no_output():
            p = importer.inspect(self.archive, self.dest)
            self.assertEqual((p.count, p.size, p.sha256), (3, 8, sha256(self.archive.read_bytes()).hexdigest()))
            with self.assertRaises(importer.DestinationUnavailable): importer.execute(p)
            with self.assertRaises(importer.DestinationUnavailable): worktree.import_zip(self.archive, self.dest)
        self.assertEqual(list(self.parent.iterdir()), [untouched])
        self.assertTrue(os.path.samestat(original, leaf.stat()))
        self.assertEqual(leaf.read_bytes(), b"not ours")

    def test_changed_input_output_and_parent_never_turn_a_previous_preview_into_authority(self):
        p = importer.inspect(self.archive, self.dest)
        original_parent = self.root/"old-parent"; self.parent.rename(original_parent); self.parent.mkdir()
        old_archive = self.root/"old.zip"; self.archive.rename(old_archive)
        self.archive.write_bytes(old_archive.read_bytes())
        self.dest.mkdir(); unrelated = self.dest/"other"; unrelated.write_bytes(b"keep")
        with no_output():
            with self.assertRaises(importer.DestinationUnavailable): importer.execute(p)
            with self.assertRaises(Rejected): importer.inspect(self.archive, self.dest)
        self.assertEqual(unrelated.read_bytes(), b"keep")
        self.assertEqual(list(original_parent.iterdir()), [])
        self.assertEqual(list(self.parent.iterdir()), [self.dest])

    def test_cli_preview_is_explicit_and_default_refuses_before_any_scan_or_write(self):
        args = ["import-zip", str(self.archive), str(self.dest)]
        out, err = io.StringIO(), io.StringIO()
        with no_output(), redirect_stdout(out), redirect_stderr(err):
            self.assertEqual(main(args + ["--preview"]), 0)
            info = json.loads(out.getvalue())
            self.assertEqual((info["preview_only"], info["write_available"], info["entries"]), (True, False, 3))
            self.assertEqual(info["archive_sha256"], sha256(self.archive.read_bytes()).hexdigest())
            self.assertNotIn("imported_entries", info)
            with patch.object(importer, "inspect", side_effect=AssertionError("denied writer should not scan")):
                self.assertEqual(main(args), 1)
        self.assertIn(importer.OUTPUT_UNAVAILABLE, err.getvalue())
        self.assertEqual(list(self.parent.iterdir()), [])

    def test_preview_does_not_hide_corruption_or_present_partial_metadata(self):
        bad = self.root/"malformed.zip"
        with zipfile.ZipFile(bad, "w") as z: z.writestr("../escape", b"no")
        with no_output(), self.assertRaises(Rejected) as failure: importer.inspect(bad, self.dest)
        self.assertNotIsInstance(failure.exception, importer.DestinationUnavailable)
        self.assertFalse(self.dest.exists()); self.assertFalse((self.root/"escape").exists())


@unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1" and find_spec("PySide6"),
                     "opt in with a working Qt display backend")
class PlainPreviewWindow(unittest.TestCase):
    def test_actual_picker_only_verifies_and_reports_why_there_is_no_write_action(self):
        from PySide6.QtTest import QTest
        from PySide6.QtWidgets import QApplication, QPushButton
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); source = root/"read.zip"; dest = root/"new"
            with zipfile.ZipFile(source,"w") as z: z.writestr("plain", b"ok")
            win = Window(); win.show()
            try:
                pane = win.plain_import; win.tabs.setCurrentWidget(pane)
                with patch("zipworkspace.desktop.import_pane.QFileDialog.getOpenFileName", return_value=(str(source),"")), \
                     patch("zipworkspace.desktop.import_pane.QFileDialog.getExistingDirectory", return_value=str(root)), \
                     patch("zipworkspace.desktop.import_pane.QInputDialog.getText", return_value=(dest.name,True)), \
                     patch.object(importer, "execute", side_effect=AssertionError("no GUI publication")):
                    pane.choose.click()
                    deadline = time.monotonic()+15
                    while win.pending is not None and time.monotonic()<deadline: QTest.qWait(10)
                self.assertIsNone(win.pending); self.assertIsNotNone(pane.proposal)
                self.assertIn(sha256(source.read_bytes()).hexdigest(), pane.details.text())
                self.assertIn(importer.OUTPUT_UNAVAILABLE, pane.outcome.text())
                self.assertEqual(pane.findChildren(QPushButton), [pane.choose])
                self.assertEqual(set(root.iterdir()), {source})
            finally: win.close()
