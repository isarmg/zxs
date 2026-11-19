from concurrent.futures import Future
from contextlib import contextmanager
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
from zipworkspace.desktop.selection import Selection, StoreSelection, compare_version
from zipworkspace.model import Commit, Manifest, Rejected


def snapshot(path, rows, operation="3"*32, *, generation=1, parent=None):
    manifest = Manifest()
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as out:
        for name, data in rows:
            is_dir = data is None
            manifest.add(name, is_dir, 0 if is_dir else len(data),
                         None if is_dir else hashlib.sha256(data).hexdigest())
            out.writestr(name + ("/" if is_dir else ""), data or b"")
        out.comment = Commit("1"*32, "2"*32, generation, operation, parent, manifest.digest()).encode()
    return Selection.open(path)


class CompareVersions(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(); self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.archive_store = self.root / "store"; self.archive_store.mkdir()
        self.left = snapshot(self.root / "current.zxs",
                             [("empty-old", None), ("Foo", b"same"), ("kind", b""), ("note", b"old")])
        first = self.left.browser.commit
        (self.archive_store / first.filename).write_bytes(self.left.path.read_bytes())
        second = snapshot(self.root / "second.zxs",
            [("empty-new", None), ("foo", b"same"), ("kind", None), ("note", b"new")] +
            [(f"right-{i:03d}", b"x") for i in range(102)],
            "4"*32, generation=2, parent=first.operation)
        second.path.rename(self.archive_store / second.browser.commit.filename)
        self.store = StoreSelection.open(self.archive_store)
        self.catalog = self.left.catalog(self.store)
        self.right = self.catalog.select_branch(first.branch)

    def test_compares_only_the_listed_node_without_reopening_the_displayed_left_or_reselecting_head(self):
        from zipworkspace.desktop import selection
        observed = self.left.path.rename(self.root / "left-original.zxs")
        snapshot(self.left.path, [("not-in-the-displayed-view", b"x")])
        third = snapshot(self.root / "third.zxs", [("not-compared", b"x")], "5"*32,
                         generation=3, parent=self.right.marker.operation)
        third.path.rename(self.archive_store / third.browser.commit.filename)
        named = selection.named_document
        def selected_right(path):
            self.assertEqual(str(path), str(self.right.path))
            return named(path)
        with patch.object(selection,"named_document",side_effect=selected_right) as opens, \
             patch.object(type(self.catalog),"select_branch",side_effect=AssertionError("never reselect HEAD")), \
             patch.object(sqlite3,"connect",side_effect=AssertionError("not worktree registration")):
            report = compare_version(self.left, self.store, self.catalog, self.right)
        self.assertTrue(observed.is_file())
        self.assertEqual(opens.call_count, 1)
        self.assertEqual((report.added,report.changed,report.missing), (104,2,2))
        self.assertEqual(len(report.page(0)), 100)
        self.assertTrue(report.page(1))
        self.assertEqual([r.path for r in report.page(0)+report.page(1)], sorted(r.path for r in report.rows))
        rows = {r.path:r for r in report.rows}
        self.assertEqual((rows["Foo"].change,rows["foo"].change), ("missing","added"))
        self.assertEqual((rows["note"].change,rows["note"].before_bytes,rows["note"].after_bytes), ("changed",3,3))
        self.assertEqual((rows["kind"].before,rows["kind"].after,rows["kind"].before_bytes), ("file","directory",0))
        self.assertEqual((rows["empty-old"].change,rows["empty-new"].change), ("missing","added"))
        self.assertNotIn("not-compared",rows)

    def test_rejects_the_same_bytes_in_a_replaced_slot_or_a_loose_catalog_anchor(self):
        slot = self.right.path
        loose = self.root / "separate.zxs"
        loose.write_bytes(slot.read_bytes())
        different = Selection.open(loose).catalog(self.store)
        self.assertEqual(different.anchor.marker, self.right.marker)
        with self.assertRaises(Rejected):
            compare_version(self.left, self.store, different, different.anchor)
        slot.rename(self.root / "retained-right.zxs")  # Keep the original inode alive.
        slot.write_bytes(loose.read_bytes())
        with self.assertRaises(Rejected):
            compare_version(self.left, self.store, self.catalog, self.right)

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop")
    def test_actual_branch_action_and_window_poll_ignore_an_error_from_the_old_store(self):
        from PySide6.QtWidgets import QApplication, QPushButton
        from zipworkspace.desktop import comparison_pane as ui
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        try:
            archive = win.archive
            archive.show_selection(self.left)
            archive.use_store(self.store)
            archive.show_catalog(self.catalog)
            table = archive.branches
            action = table.parentWidget().findChild(QPushButton, "compare-selected-revision")
            self.assertIsNotNone(action)

            def start():
                archive.views.setCurrentWidget(table.parentWidget())
                table.selectRow(0)
                future = Future()
                with patch.object(win.pool, "submit", return_value=future) as queue:
                    action.click()
                self.assertIs(win.pending[0], future)
                self.assertFalse(win.tabs.isEnabled())
                return future, queue.call_args.args[0]

            future, run = start()
            future.set_result(run()); win.poll()
            pane = archive.comparison
            self.assertIs(archive.selection, self.left)
            self.assertIs(archive.views.currentWidget(), pane)
            self.assertIn(self.right.marker.operation, pane.summary.text())
            self.assertIn(self.left.browser.commit.operation, pane.summary.text())
            self.assertEqual(pane.table.rows.rows[0][0], ("Foo","仅当前","文件 · 4 B","无"))
            self.assertTrue(pane.more.isEnabled())
            pane.more.click(); self.assertEqual(pane.number, 1)

            future, run = start()
            with patch.object(ui, "compare_listed_version", side_effect=Rejected("current archive was changed")):
                future.set_result(run())
            win.poll()
            self.assertIsNone(pane.report)
            self.assertIn("current archive was changed", pane.summary.text())
            self.assertIn("未完成版本比较", win.note.text())

            future, run = start()
            directory = self.root / "different-store"; directory.mkdir()
            archive.use_store(StoreSelection.open(directory))
            replacement_note = win.note.text()
            with patch.object(ui, "compare_listed_version", side_effect=Rejected("late old version error")):
                future.set_result(run())
            win.poll()
            self.assertTrue(win.tabs.isEnabled())
            self.assertIsNone(pane.report)
            self.assertNotIn(self.right.marker.operation, pane.summary.text())
            self.assertNotIn("late old version error", pane.summary.text())
            self.assertEqual(win.note.text(), replacement_note)
        finally:
            if win.pending is not None:  # Never leave this fixture's unstarted Future owning a window.
                win.pending[0].cancel(); win.poll()
            win.close()


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
        rows = {r.path: (r.change, r.before, r.after) for r in report.page(0)}
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

    @unittest.skipUnless(os.name == "posix", "checks POSIX fd/inode lifetime")
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

    @unittest.skipUnless(os.name == "posix", "injects the POSIX directory scanner")
    def test_changes_during_inventory_link_and_late_root_replacement_return_no_report(self):
        chosen = snapshot(self.path, [("entry",b"first")])
        leaf = self.local/"entry"; leaf.write_bytes(b"first")
        scandir = os.scandir
        passes = 0
        def edit_before_second_walk(fd):
            nonlocal passes
            passes += 1  # This fixture has no nested directory.
            if passes == 2: leaf.write_bytes(b"a longer generation")
            return scandir(fd)
        with patch.object(os, "scandir", side_effect=edit_before_second_walk), self.assertRaises(Rejected):
            comparison.compare(chosen,self.chosen)
        self.assertEqual(passes, 2)
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

    @unittest.skipUnless(os.name == "posix", "exercises native descriptor-relative inventories")
    def test_second_inventory_consumes_each_original_name_once_in_any_order(self):
        chosen = snapshot(self.path, [("sub/a", b"a"), ("sub/b", b"b")])
        sub = self.local / "sub"; sub.mkdir()
        for name in ("a", "b"): (sub / name).write_bytes(name.encode())
        original = sub.stat()
        scandir = os.scandir
        for first, second, accept in (("ab", "ba", True), ("ab", "a", False),
                                      ("a", "ab", False), ("ab", "aab", False)):
            with self.subTest(first=first, second=second):
                folder = comparison.Folder.choose(self.local)
                seen = []
                @contextmanager
                def inventory(fd):
                    with scandir(fd) as entries:
                        if not os.path.samestat(os.fstat(fd), original):
                            yield entries
                            return
                        # Model an incomplete/repeated native enumeration without changing
                        # the real names, their contents, or the parent timestamps.
                        by_name = {e.name: e for e in entries}
                        order = (first, second)[len(seen)]
                        seen.append(order)
                        yield (by_name[name] for name in order)
                with patch.object(os, "scandir", side_effect=inventory), \
                     patch.object(freeze, "Manifest", wraps=Manifest) as manifests:
                    if accept:
                        self.assertEqual(comparison.compare(chosen, folder).rows, ())
                    else:
                        with self.assertRaises(Rejected): comparison.compare(chosen, folder)
                self.assertEqual(seen, [first, second])
                self.assertEqual(manifests.call_count, 1)

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
        from PySide6.QtWidgets import QApplication, QFileDialog
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
                with patch.object(window,"job") as runner, \
                     patch.object(QFileDialog,"getExistingDirectory",return_value=str(folder)) as picker:
                    pane.pick()
                    self.assertTrue(picker.call_args.args[3] & QFileDialog.Option.DontResolveSymlinks)
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
                held.append((chosen,chosen.check()))
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
                        self.assertEqual(held[0][0].check(), held[0][1])
                        self.assertTrue(result.cancel())
                    with self.assertRaises(Rejected): held[0][0].check()
                    if os.name == "posix":
                        with self.assertRaises(OSError) as closed: os.fstat(held[0][1])
                        self.assertEqual(closed.exception.errno,errno.EBADF)
            finally: window.close()
