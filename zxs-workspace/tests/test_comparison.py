from concurrent.futures import Future
import errno
import hashlib
import os
from importlib.util import find_spec
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import comparison, freeze
from zipworkspace.desktop.selection import Selection
from zipworkspace.model import Commit, Manifest, Rejected


def snapshot(path, rows, operation="3"*32):
    manifest = Manifest()
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as out:
        for name, data in rows:
            is_dir = data is None
            manifest.add(name, is_dir, 0 if is_dir else len(data),
                         None if is_dir else hashlib.sha256(data).hexdigest())
            out.writestr(name + ("/" if is_dir else ""), data or b"")
        out.comment = Commit("1"*32, "2"*32, 1, operation, None, manifest.digest()).encode()
    return Selection.open(path)


@unittest.skipUnless(comparison.available(), "requires verified descriptor-relative folder reads")
class CompareFolder(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(); self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.path = self.root/"source.zxs"
        self.local = self.root/"local"; self.local.mkdir()
        self.chosen = comparison.Folder.choose(self.local)
        self.addCleanup(self.chosen.close)

    def test_content_types_empty_folders_and_missing_rows_without_registering_or_writing(self):
        chosen = snapshot(self.path, [("same",b"kept"), ("sized",b"old"), ("was-file",b""),
                                      ("gone/empty",None), ("was-dir/leaf",b"x")])
        for name, data in (("same",b"kept"), ("sized",b"new"), ("was-dir",b""), ("new",b"")):
            (self.local/name).write_bytes(data)
        (self.local/"was-file").mkdir(); (self.local/"empty-added").mkdir()
        before = sorted(p.relative_to(self.root) for p in self.root.rglob("*"))
        original_zip = self.path.read_bytes()
        with patch.object(sqlite3,"connect",side_effect=AssertionError("not a state database")), \
             patch.object(zipfile.ZipFile,"extractall",side_effect=AssertionError("no extraction")):
            report = comparison.compare(chosen, self.chosen)
        rows = {r.path: (r.change, r.snapshot, r.local) for r in report.page(0)}
        self.assertEqual((report.added,report.changed,report.missing), (2,3,3))
        self.assertEqual(rows, {"empty-added":("added",None,"directory"), "new":("added",None,"file"),
            "sized":("changed","file","file"), "was-dir":("changed","directory","file"),
            "was-file":("changed","file","directory"), "was-dir/leaf":("missing","file",None),
            "gone":("missing","directory",None), "gone/empty":("missing","directory",None)})
        self.assertEqual(sorted(p.relative_to(self.root) for p in self.root.rglob("*")), before)
        self.assertEqual(self.path.read_bytes(), original_zip)
        self.assertEqual((self.local/"sized").read_bytes(), b"new")

    def test_all_changes_are_pageable_with_no_truncation_or_fake_clean_state(self):
        chosen = snapshot(self.path, [("unchanged", b"")])
        (self.local/"unchanged").write_bytes(b"")
        empty = comparison.compare(chosen, self.chosen)
        self.assertEqual((empty.added,empty.changed,empty.missing,empty.page(0)), (0,0,0,()))
        for number in range(102): (self.local/f"leaf-{number:03}").write_bytes(b"")
        report = comparison.compare(chosen,comparison.Folder.choose(self.local))
        self.assertEqual(report.added, 102)
        self.assertEqual([r.path for r in report.page(0)+report.page(1)],
                         [f"leaf-{n:03}" for n in range(102)])
        self.assertEqual(len(report.page(0)), report.PAGE)
        for bad in (2, -1, True):
            with self.assertRaises(Rejected): report.page(bad)

    def test_same_name_substitution_of_folder_or_archive_is_not_a_new_selection(self):
        chosen = snapshot(self.path, [])
        fd = self.chosen.check()
        self.local.rename(self.root/"retained"); self.local.mkdir()
        held = os.fstat(fd)
        self.assertEqual((held.st_dev,held.st_ino), self.chosen.evidence.identity)
        self.assertNotEqual(held.st_ino, self.local.stat().st_ino)
        with patch.object(comparison,"observe_held",side_effect=AssertionError("never scan the replacement")), self.assertRaises(Rejected):
            comparison.compare(chosen,self.chosen)
        with self.assertRaises(OSError) as closed: os.fstat(fd)
        self.assertEqual(closed.exception.errno, errno.EBADF)
        current = comparison.Folder.choose(self.local)
        original = self.root/"first.zxs"; self.path.rename(original)
        self.path.write_bytes(original.read_bytes())
        with self.assertRaises(Rejected): comparison.compare(chosen, current)
        self.assertEqual(comparison.compare(Selection.open(self.path), comparison.Folder.choose(self.local)).page(0), ())

    def test_changes_during_inventory_link_and_late_root_replacement_return_no_report(self):
        chosen = snapshot(self.path, [("entry",b"first")])
        leaf = self.local/"entry"; leaf.write_bytes(b"first")
        original_scan = freeze._scan
        def edit_after_hash(fd, *args, **kwargs):
            result = original_scan(fd, *args, **kwargs)
            if kwargs.get("hash_files"): leaf.write_bytes(b"a longer generation")
            return result
        with patch.object(freeze,"_scan",side_effect=edit_after_hash), self.assertRaises(Rejected):
            comparison.compare(chosen,self.chosen)
        alias = self.local/"alias"; alias.symlink_to(self.path)
        with self.assertRaises(Rejected): comparison.compare(chosen,comparison.Folder.choose(self.local))
        alias.unlink()
        original_observe = comparison.observe_held
        def replace_after_scan(fd, identity, **kwargs):
            tree = original_observe(fd, identity, **kwargs)
            self.local.rename(self.root/"prior"); self.local.mkdir()
            return tree
        with patch.object(comparison,"observe_held",side_effect=replace_after_scan), self.assertRaises(Rejected):
            comparison.compare(chosen,comparison.Folder.choose(self.local))

    def test_distinct_spelling_is_not_an_inferred_rename_and_intra_tree_aliases_refuse(self):
        chosen = snapshot(self.path, [("Foo",b"same")])
        (self.local/"foo").write_bytes(b"same")
        report = comparison.compare(chosen,self.chosen)
        self.assertEqual([(r.path,r.change) for r in report.rows],[("Foo","missing"),("foo","added")])
        if not (self.local/"Foo").exists():  # A filesystem that supports both spellings.
            (self.local/"Foo").write_bytes(b"same")
            with self.assertRaises(Rejected): comparison.compare(chosen,comparison.Folder.choose(self.local))


@unittest.skipUnless(comparison.available() and find_spec("PySide6"), "requires desktop extra and folder reader")
class CompareWidget(unittest.TestCase):
    def test_real_controls_keep_the_original_row_and_ignore_a_late_different_snapshot(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); folder = root/"ordinary"; folder.mkdir()
            chosen = snapshot(root/"a.zxs", [("not-local",b"")])
            later = snapshot(root/"b.zxs", [], "4"*32)
            window = Window()
            try:
                archive = window.archive; pane = archive.comparison
                self.assertFalse(pane.choose.isEnabled())
                archive.show_selection(chosen)
                with patch.object(window,"job") as runner:
                    pane.compare_path(folder)
                    _, work, finish = runner.call_args.args
                    result = work(); finish(result)
                    self.assertEqual(pane.table.rows.rows[0][0], ("not-local","缺少","文件","无"))
                    archive.show_selection(later)
                    finish(result)
                    self.assertIsNone(pane.report)
                    self.assertEqual(pane.table.rows.rows,())
                    self.assertIn(later.browser.commit.operation,pane.summary.text())
            finally: window.close()

    def test_folder_fd_released_if_job_is_refused_fails_submission_or_is_cancelled_before_start(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); folder = root/"ordinary"; folder.mkdir()
            selected = snapshot(root/"base.zxs", [])
            window = Window(); window.archive.show_selection(selected)
            pane = window.archive.comparison
            choose = comparison.Folder.choose
            def capture(path):
                chosen = choose(path)
                self.addCleanup(chosen.close)
                held.append(chosen.check())
                return chosen
            try:
                for result in (None, RuntimeError("submission failed"), Future()):
                    held = []
                    with patch.object(comparison.Folder,"choose",side_effect=capture), \
                            patch.object(window,"job",side_effect=result if isinstance(result,Exception) else None,
                                         return_value=result):
                        pane.compare_path(folder)
                    self.assertEqual(len(held),1)
                    if isinstance(result,Future):
                        self.assertEqual(os.fstat(held[0]).st_ino, folder.stat().st_ino)
                        self.assertTrue(result.cancel())
                    with self.assertRaises(OSError) as closed: os.fstat(held[0])
                    self.assertEqual(closed.exception.errno,errno.EBADF)
            finally: window.close()
