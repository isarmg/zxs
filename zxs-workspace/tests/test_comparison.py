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
from zipworkspace.desktop.selection import (PlainSelection, Selection, StoreSelection, Verification,
                                            compare_snapshot_file, compare_version)
from zipworkspace.model import CHUNK, Commit, Manifest, Rejected


def snapshot(path, rows, operation="3"*32, *, generation=1, parent=None, repo="1"*32):
    manifest = Manifest()
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as out:
        for name, data in rows:
            is_dir = data is None
            manifest.add(name, is_dir, 0 if is_dir else len(data),
                         None if is_dir else hashlib.sha256(data).hexdigest())
            out.writestr(name + ("/" if is_dir else ""), data or b"")
        out.comment = Commit(repo, "2"*32, generation, operation, parent, manifest.digest()).encode()
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

    def test_compares_plain_zip_as_verified_contents_not_a_repository_or_an_open_path(self):
        origin = self.root / "roundtrip.zip"
        origin.write_bytes(self.left.path.read_bytes())
        copied = PlainSelection.open(origin, retain=False)
        with patch.object(os, "open", side_effect=AssertionError("no reopening")), \
             patch.object(sqlite3, "connect", side_effect=AssertionError("no state work")):
            identical = comparison.compare_plain(self.left.browser, copied.rows)
        self.assertFalse(identical.rows)
        ordinary = self.root / "independent.zip"
        with zipfile.ZipFile(ordinary, "w") as z:
            z.writestr("empty-new/", b"")
            z.writestr("foo", b"same")
            z.writestr("kind/", b"")
            z.writestr("note", b"new")  # equal size; content hash differs
            z.comment = b"unrelated"
        view = PlainSelection.open(ordinary, retain=False)
        ordinary.unlink()  # The two completed in-memory readings do not require either name.
        with patch.object(os, "open", side_effect=AssertionError("no reopening")):
            changes = comparison.compare_plain(self.left.browser, view.rows)
        self.assertEqual((changes.added, changes.changed, changes.missing), (2, 2, 2))
        names = {r.path: r for r in changes.page(0)}
        self.assertEqual((names["empty-old"].change, names["empty-new"].change), ("missing", "added"))
        self.assertEqual((names["Foo"].change, names["foo"].change), ("missing", "added"))
        self.assertEqual((names["kind"].before, names["kind"].after), ("file", "directory"))
        self.assertEqual((names["note"].before_bytes, names["note"].after_bytes), (3, 3))
        self.assertEqual(len(view.rows), 4)  # a comparison consumes only a temporary dict

    def test_two_explicit_files_from_different_repos_need_no_store_or_left_reopen(self):
        from zipworkspace.desktop import selection
        other = snapshot(self.root/"other-project.zxs",
                         [("note", b"new"), ("empty-new", None)], "6"*32, repo="9"*32)
        saved = self.left.path.rename(self.root/"older-left.zxs")
        original = selection.named_document
        def only_other(path):
            self.assertEqual(path, other.path)
            return original(path)
        with patch.object(selection, "named_document", side_effect=only_other) as opens, \
             patch.object(selection, "Catalog", side_effect=AssertionError("no inventory")), \
             patch.object(sqlite3, "connect", side_effect=AssertionError("no worktree")):
            path, marker, report = compare_snapshot_file(self.left, other.path, observer=Verification())
        opens.assert_called_once()
        self.assertTrue(saved.is_file())
        self.assertEqual((path, marker.repo), (other.path, other.browser.commit.repo))
        self.assertEqual((report.added, report.changed, report.missing), (1, 1, 3))
        with self.assertRaisesRegex(Rejected, "not from one"):
            comparison.compare_snapshots(self.left.browser, other.browser)  # history remains distinct

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop")
    def test_direct_snapshot_picker_compares_without_store_and_rejects_a_late_previous_view(self):
        from PySide6.QtWidgets import QApplication, QFileDialog
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        try:
            win.archive.show_selection(self.left)
            pane = win.archive.comparison
            other = snapshot(self.root/"different.zxs", [("folder", None)], "6"*32, repo="9"*32)
            win.archive.views.setCurrentWidget(pane)

            def begin():
                future = Future()
                with patch.object(QFileDialog, "getOpenFileName", return_value=(str(other.path), "")), \
                     patch.object(win.pool, "submit", return_value=future) as queued:
                    pane.choose_snapshot.click()
                return future, queued.call_args.args[0]

            future, run = begin()
            future.set_result(run()); win.poll()
            self.assertIsNone(win.archive.store)
            self.assertIs(win.archive.selection, self.left)
            self.assertEqual(pane.report.added, 1)
            self.assertIn("不推断仓库", pane.summary.text())

            future, run = begin()
            result = run()
            win.archive.show_selection(other)
            notice = win.note.text()
            win.poll()
            future.set_result(result); win.poll()
            self.assertIsNone(pane.report)
            self.assertEqual(win.note.text(), notice)
        finally:
            if win.pending is not None:
                win.pending[0].cancel(); win.poll()
            win.close()

    def test_catalog_cancellation_stops_before_reading_the_next_slot(self):
        from zipworkspace import refs
        from zipworkspace.desktop.selection import Verification

        class Watch(Verification):
            def in_archive(self, index, count):
                # first non-anchor catalog slot, after the named anchor passed
                self.assertion = index, count
                self.cancel()
                super().in_archive(index, count)

        watch = Watch()
        with patch.object(refs, "_read_node", wraps=refs._read_node) as native:
            with self.assertRaisesRegex(Rejected, "取消"):
                self.left.catalog(self.store, observer=watch)
        self.assertEqual(watch.assertion, (1, 2))
        self.assertEqual(native.call_count, 1)
        self.assertEqual(native.call_args.args[1], self.left.path)

    def test_comparing_the_right_version_can_stop_on_a_real_member_block(self):
        from zipworkspace.desktop.selection import Verification

        class StopAtBody(Verification):
            def advance(self, size):
                super().advance(size)
                phase, done, total = self.progress()
                if phase == self.PHASE["members"] and done and total > done:
                    self.cancel()

        observer = StopAtBody()
        with patch("zipworkspace.desktop.selection.archive_sha", side_effect=AssertionError("body stopped before raw read")), \
             patch.object(comparison, "compare_snapshots", side_effect=AssertionError("no partial comparison")):
            with self.assertRaisesRegex(Rejected, "取消"):
                compare_version(self.left, self.store, self.catalog, self.right, observer=observer)
        phase, processed, total = observer.progress()
        self.assertEqual(phase, Verification.PHASE["members"])
        self.assertEqual(processed, 4)  # first ordinary member, not empty directory or ZIP metadata
        self.assertGreater(total, processed)

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1" and find_spec("PySide6"),
                         "optional Qt source fixture")
    def test_completed_but_cancelled_catalog_does_not_replace_the_left_snapshot(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        try:
            pane = win.archive
            pane.show_selection(self.left)
            pane.use_store(self.store)
            future = Future()
            with patch.object(win.pool, "submit", return_value=future) as queued:
                pane.reload.click()
            result = queued.call_args.args[0]()
            self.assertIsNone(result[1])
            self.assertTrue(result[0].branches())
            future.set_result(result)
            win.transfer_cancel.click()
            win.poll()
            self.assertIs(pane.selection, self.left)
            self.assertIsNone(pane.catalog)
            self.assertEqual(pane.branches.rows.rows, ())
            self.assertEqual(pane.history.rows.rows, ())
            self.assertIn("已取消核验", win.note.text())
        finally:
            if win.pending is not None:
                win.pending[0].cancel(); win.poll()
            win.close()

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1" and find_spec("PySide6"),
                         "optional Qt source fixture")
    def test_ordinary_zip_action_clears_late_comparison_after_switching_the_right_view(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        try:
            win.archive.show_selection(self.left)
            source = self.root / "ordinary.zip"
            with zipfile.ZipFile(source, "w") as z: z.writestr("note", b"new")
            plain = PlainSelection.open(source, retain=False)
            win.plain_import.show_selection(plain)
            pane = win.archive.comparison

            def start():
                win.tabs.setCurrentWidget(win.plain_import)
                future = Future()
                with patch.object(win.pool, "submit", return_value=future) as queued:
                    win.plain_import.compare.click()
                self.assertIs(win.pending[0], future)
                self.assertIs(win.tabs.currentWidget(), win.archive)
                self.assertIs(win.archive.views.currentWidget(), pane)
                return future, queued.call_args.args[0]

            future, run = start()
            future.set_result(run())
            win.poll()
            self.assertIn(plain.preview.sha256, pane.summary.text())
            self.assertIn(("note", "不同", "文件 · 3 B", "文件 · 3 B"),
                          tuple(r[0] for r in pane.table.rows.rows))
            self.assertIs(win.archive.selection, self.left)

            future, run = start()
            late = run()
            win.plain_import.release_display()
            new_status = win.note.text()
            future.set_result(late)
            win.poll()
            self.assertIsNone(pane.report)
            self.assertIsNone(pane.plain)
            self.assertEqual(pane.table.rows.rows, ())
            self.assertEqual(win.note.text(), new_status)
        finally:
            if win.pending is not None:
                win.pending[0].cancel(); win.poll()
            win.close()

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
            result = run()
            self.assertIsNotNone(result[0])
            future.set_result(result)
            win.transfer_cancel.click()  # a returned but not yet applied result is still discardable
            win.poll()
            self.assertIsNone(pane.report)
            self.assertEqual(pane.table.rows.rows, ())
            self.assertIs(archive.selection, self.left)
            self.assertIn("已取消版本对照", win.note.text())

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
            win.poll()  # Old progress may not return to the status bar even before the reply arrives.
            self.assertEqual(win.note.text(), replacement_note)
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

    def test_comparison_measures_one_body_pass_and_can_stop_before_the_next_block(self):
        chosen = snapshot(self.path, [])
        body = b"x" * (CHUNK + 23)
        (self.local / "large").write_bytes(body)
        (self.local / "empty").mkdir()

        class Watch(Verification):
            def __init__(self, stop):
                super().__init__()
                self.stop, self.counts, self.phases = stop, [], []
            def begin(self, phase, total=None):
                super().begin(phase, total)
                self.phases.append(phase)
            def advance(self, size):
                super().advance(size)
                self.counts.append(self.progress()[1:])
                if self.stop: self.cancel()

        first = Watch(True)
        with self.assertRaisesRegex(Rejected, "取消"):
            comparison.compare(chosen, self.chosen, observer=first)
        (read, total), = first.counts
        self.assertLessEqual(read, CHUNK)
        self.assertGreater(read, 0)
        self.assertIsNone(total)
        self.assertNotIn("inventory", first.phases)
        with self.assertRaises(Rejected): self.chosen.check()  # the consumed native handle closed

        second = Watch(False)
        report = comparison.compare(chosen, comparison.Folder.choose(self.local), observer=second)
        self.assertEqual(second.counts[-1], (len(body), None))
        self.assertEqual(second.phases, ["identity", "folder", "inventory", "compare", "identity"])
        self.assertEqual(report.added, 2)

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

    def test_folder_cancellation_and_late_progress_are_bound_to_the_original_snapshot(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); local = root/"local"; local.mkdir()
            chosen = snapshot(root/"a.zxs", []); later = snapshot(root/"b.zxs", [], "4"*32)
            window = Window()
            try:
                window.archive.show_selection(chosen)
                pane = window.archive.comparison
                def begin():
                    future = Future()
                    with patch.object(window.pool, "submit", return_value=future) as queued:
                        pane.compare_path(local)
                    self.assertIs(window.pending[0], future)
                    return future, queued.call_args.args[0]

                future, work = begin()
                finished = work()
                self.assertIsNotNone(finished[0])
                future.set_result(finished); window.transfer_cancel.click(); window.poll()
                self.assertIs(window.archive.selection, chosen)
                self.assertIsNone(pane.report)
                self.assertIn("已取消文件夹对照", window.note.text())

                future, work = begin()
                window.archive.show_selection(later)
                now = window.note.text()
                window.poll()  # An obsolete in-flight progress sampler cannot update the window.
                self.assertEqual(window.note.text(), now)
                future.set_result(work()); window.poll()
                self.assertIsNone(pane.report)
                self.assertEqual(window.note.text(), now)
            finally:
                if window.pending is not None:
                    window.pending[0].cancel(); window.poll()
                window.close()

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
