"""Plain files only: held input, exclusive final directory and verified output."""
from importlib.util import find_spec
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import plain_import as importer
from zipworkspace.model import Rejected


class PlainImport(unittest.TestCase):
    def setUp(self):
        area = tempfile.TemporaryDirectory()
        self.addCleanup(area.cleanup)
        self.root = Path(area.name)
        self.archive = self.root/"example.zip"
        with zipfile.ZipFile(self.archive,"w",compression=zipfile.ZIP_STORED) as z:
            z.writestr("folder/note",b"original")
            z.writestr("empty/",b"")
        self.parent = self.root/"output"; self.parent.mkdir()
        self.dest = self.parent/"copy"

    def writer(self):
        try: importer.require_native_writer()
        except (Rejected,OSError): self.skipTest("native no-replace provider required")

    def test_preview_writes_nothing_and_exact_import_yields_only_ordinary_files(self):
        proposal = importer.inspect(self.archive,self.dest)
        self.assertEqual((proposal.count,proposal.size),(3,8))
        self.assertEqual(list(self.parent.iterdir()),[])
        self.writer()
        result = importer.execute(proposal)
        self.assertEqual(result.digest(),proposal.tree)
        self.assertEqual((self.dest/"folder/note").read_bytes(),b"original")
        self.assertEqual(sorted(str(p.relative_to(self.dest)) for p in self.dest.rglob("*")),["empty","folder","folder/note"])
        self.assertEqual(list(self.parent.iterdir()),[self.dest])
        with self.assertRaises(Rejected): importer.execute(proposal)
        self.assertEqual((self.dest/"folder/note").read_bytes(),b"original")

    def test_changed_or_same_byte_replaced_input_never_displays_a_success(self):
        self.writer()
        proposal = importer.inspect(self.archive,self.dest)
        self.archive.rename(self.root/"original")
        self.archive.write_bytes((self.root/"original").read_bytes())
        with self.assertRaises(Rejected): importer.execute(proposal)
        self.assertEqual(list(self.parent.iterdir()),[])
        self.archive.unlink(); (self.root/"original").rename(self.archive)
        with zipfile.ZipFile(self.archive,"w") as z: z.writestr("other",b"different")
        with self.assertRaises(Rejected): importer.execute(proposal)
        self.assertEqual(list(self.parent.iterdir()),[])

    def test_substituting_parent_never_receives_staged_data_or_visible_directory(self):
        self.writer()
        proposal = importer.inspect(self.archive,self.dest)
        retained = self.root/"held"
        primitive = importer.rename_new_at
        def move_parent(fd,src,dst,**bound):
            self.parent.rename(retained); self.parent.mkdir()
            return primitive(fd,src,dst,**bound)
        with patch.object(importer,"rename_new_at",side_effect=move_parent), self.assertRaises(Rejected):
            importer.execute(proposal)
        self.assertEqual(list(self.parent.iterdir()),[])
        self.assertEqual((retained/self.dest.name/"folder/note").read_bytes(),b"original")
        with self.assertRaises(Rejected): importer.execute(proposal)

    def test_same_bytes_under_another_stage_or_final_inode_are_not_confirmed(self):
        self.writer()
        primitive = importer.rename_new_at
        for before in (True,False):
            p = importer.inspect(self.archive,self.parent/("before" if before else "after"))
            retained = self.root/("saved-before" if before else "saved-after")
            def other(fd,src,dst,**bound):
                if not before: primitive(fd,src,dst,**bound)
                changed = self.parent/(src if before else dst)
                changed.rename(retained); changed.mkdir()
                (changed/"empty").mkdir(); (changed/"folder").mkdir()
                (changed/"folder/note").write_bytes(b"original")
                if before: primitive(fd,src,dst,**bound)
            with self.subTest(before=before), patch.object(importer,"rename_new_at",side_effect=other), self.assertRaises(Rejected):
                importer.execute(p)
            self.assertEqual((retained/"folder/note").read_bytes(),b"original")
            self.assertEqual(p.target.exists(),not before)
            if p.target.exists(): self.assertNotEqual(p.target.stat().st_ino,retained.stat().st_ino)

    def test_final_content_is_independently_read_even_if_original_directory_inode_remains(self):
        self.writer()
        p = importer.inspect(self.archive,self.dest)
        primitive = importer.rename_new_at
        def changed(fd,src,dst,**bound):
            primitive(fd,src,dst,**bound)
            (self.dest/"folder/note").write_bytes(b"changed!")
        with patch.object(importer,"rename_new_at",side_effect=changed), self.assertRaises(Rejected):
            importer.execute(p)
        self.assertEqual((self.dest/"folder/note").read_bytes(),b"changed!")


@unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1" and find_spec("PySide6"),
                     "opt in with a working Qt display backend")
class ImportWindow(unittest.TestCase):
    def test_native_window_cancel_then_confirm_selected_plain_zip(self):
        from PySide6.QtCore import QTimer
        from PySide6.QtTest import QTest
        from PySide6.QtWidgets import QApplication, QDialog
        from zipworkspace.desktop.window import Window
        try: importer.require_native_writer()
        except (Rejected,OSError): self.skipTest("native no-replace provider required")
        app = QApplication.instance() or QApplication([])
        with tempfile.TemporaryDirectory() as area:
            root = Path(area); source = root/"one.zip"; dest = root/"new"
            with zipfile.ZipFile(source,"w") as z: z.writestr("plain",b"result")
            proposal = importer.inspect(source,dest)
            win = Window(); win.show()
            try:
                pane = win.plain_import
                win.tabs.setCurrentWidget(pane)
                pane.show_proposal(proposal)
                def answer(ok):
                    dialog = app.activeModalWidget()
                    if isinstance(dialog,QDialog): dialog.accept() if ok else dialog.reject()
                QTimer.singleShot(0,lambda: answer(False)); pane.start.click()
                self.assertFalse(dest.exists()); self.assertIs(pane.proposal,proposal)
                QTimer.singleShot(0,lambda: answer(True)); pane.start.click()
                deadline = time.monotonic()+15
                while win.pending is not None and time.monotonic()<deadline: QTest.qWait(10)
                self.assertIsNone(win.pending); self.assertIsNone(pane.proposal)
                self.assertIn(str(dest),pane.outcome.text())
                self.assertEqual((dest/"plain").read_bytes(),b"result")
                self.assertEqual(set(root.iterdir()),{source,dest})
            finally: win.close()
