"""Opt-in ordinary-user ext4/XFS, no device changes; leaves only synthetic cases for inspection."""
from concurrent.futures import Future
from contextlib import ExitStack
from dataclasses import replace
import errno
import hashlib
from importlib.util import find_spec
import os
from pathlib import Path
from types import SimpleNamespace
import stat
import sys
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import comparison, lifecycle, worktree
from zipworkspace.comparison import Folder
from zipworkspace.desktop import member_copy, store_import, text_preview
from zipworkspace.desktop.commit_choice import CommitChoice
from zipworkspace.desktop.initialize_choice import InitializeChoice
from zipworkspace.desktop.selection import PlainSelection, Selection, StoreSelection, Verification
from zipworkspace.model import CHUNK, Commit, Manifest, Rejected, new_id
from zipworkspace.publish import Uncertain


@unittest.skipUnless(sys.platform == "linux", "Linux native export qualification")
class LinuxMemberCopy(unittest.TestCase):
    def setUp(self):
        explicit = os.environ.get("ZXS_LINUX_COPY_SCRATCH")
        if not explicit: self.skipTest("give an exclusive disposable ext4/XFS directory in ZXS_LINUX_COPY_SCRATCH")
        scratch = Path(explicit)
        if scratch.anchor != "/" or str(scratch) != explicit or ".." in scratch.parts:
            self.fail("give the exact absolute path of the existing scratch directory")
        leaf = "member-copy-" + new_id()
        with ExitStack() as held:
            proc = member_copy._ProcView(held)
            uid = member_copy._ordinary_user(read_proc=proc.read)
            parent, _ = member_copy._directory(held, scratch, uid, proc.read)
            member_copy.local_mount(parent, read_proc=proc.read)
            if stat.S_IMODE(os.fstat(parent).st_mode) != 0o700:
                self.fail("the fixture scratch directory must be exclusive 0700")
            # Exclusive case creation under the held scratch; no cleanup by name.
            os.mkdir(leaf, mode=0o700, dir_fd=parent)
        case = scratch / leaf
        self.dest = case / "out"
        self.private = case / "scratch"
        self.dest.mkdir(mode=0o700)
        self.private.mkdir(mode=0o700)
        self.path = self.dest / "saved.bin"
        self.data = b"\x00original\xff" * 110_000  # crosses a real copy block and is never opened/executed
        self.archive = case / "chosen.zxs"
        tree = Manifest()
        with self.archive.open("xb") as raw, zipfile.ZipFile(raw, "w") as archive:
            for name, data in (("original/same.bin", self.data), ("other/same.bin", b"x" * (2*CHUNK)),
                               ("original/zero", b""), ("tree/nested/value", b"only this subtree"),
                               ("tree/empty/", None), ("tree/zero", b""),
                               ("tree2/foreign", b"lookalike prefix"), ("empty-root/", None)):
                if data is None:
                    tree.add(name[:-1], True, 0, None)
                else:
                    tree.add(name, False, len(data), hashlib.sha256(data).hexdigest())
                archive.writestr(name, data or b"")
            archive.comment = Commit("1"*32, "2"*32, 1, "3"*32, None, tree.digest()).encode()
        self.original = self.archive.read_bytes()
        self.selected = Selection.open(self.archive, retain=True)
        self.addCleanup(self.selected.release_display)

    def plan(self, name="same.bin"):
        entry = self.selected.browser.entry("original", name)
        plan = member_copy.MemberCopy(self.selected, "original", entry, str(self.path))
        self.addCleanup(plan.close)
        return plan

    def zip_plan(self, name="portable.zip"):
        plan = member_copy.ZipCopy(self.selected, str(self.dest/name))
        self.addCleanup(plan.close)
        return plan

    def stored_version(self, generation, parent=None, *, same=None, year=1980, path=None):
        body = f"original body of one version: {generation}".encode()
        tree = Manifest()
        tree.add("item", False, len(body), hashlib.sha256(body).hexdigest())
        tree.add("empty", True, 0, None)
        marker = same or Commit("a"*32, "b"*32, generation, new_id(), parent, tree.digest())
        path = self.dest/marker.filename if path is None else path
        with path.open("xb") as out, zipfile.ZipFile(out, "w", compression=zipfile.ZIP_STORED) as z:
            for name, mode, value in (("item", 0o100644, body), ("empty/", 0o40755, b"")):
                entry = zipfile.ZipInfo(name, (year, 1, 1, 0, 0, 0)); entry.external_attr = mode << 16
                z.writestr(entry, value)
            z.comment = marker.encode()
        return path, marker

    def import_version(self):
        parent, marker = self.stored_version(1)
        original, _ = self.stored_version(2, marker.operation, path=self.private/"current.zxs")
        selected = Selection.open(original, retain=True)
        self.addCleanup(selected.release_display)
        store = StoreSelection.open(self.dest)
        catalog = selected.catalog(store)
        return selected, store, catalog, parent

    def import_plan(self, selected, store, catalog):
        plan = store_import.StoreSuccessor(selected, store, catalog)
        self.addCleanup(plan.close)
        return plan

    def compare_folder_fixture(self, right=b"<new>\n", left=b"<old>\r\n"):
        path = self.private / "compared.zxs"
        folder = self.dest / "observed"
        (folder / "original").mkdir(parents=True)
        (folder / "other").mkdir()
        (folder / "original" / "same").write_bytes(right)
        (folder / "other" / "same").write_bytes(b"other new")
        (folder / "original" / "hard").write_bytes(b"linked right")
        (folder / "other" / "hard").hardlink_to(folder / "original" / "hard")
        tree = Manifest()
        with zipfile.ZipFile(path, "x", compression=zipfile.ZIP_STORED) as package:
            for name, data in (("original/same", left), ("other/same", b"other old"),
                               ("original/hard", b"not right")):
                tree.add(name, False, len(data), hashlib.sha256(data).hexdigest())
                package.writestr(name, data)
            package.comment = Commit("1"*32, "2"*32, 1, "3"*32, None, tree.digest()).encode()
        selected = Selection.open(path, retain=True)
        self.addCleanup(selected.release_display)
        return selected, folder

    def folder_report(self, selected, folder):
        with patch.object(comparison, "named_document", side_effect=AssertionError("do not reopen the left body")):
            report = comparison.compare(selected, Folder.choose(folder), observer=Verification(), retain_text=True)
        self.addCleanup(report.close)
        return report, next(row for row in report.rows if row.path == "original/same")

    def test_folder_line_diff_uses_only_both_original_report_files(self):
        selected, folder = self.compare_folder_fixture()
        right = folder / "original" / "same"
        original_right = right.stat()
        report, row = self.folder_report(selected, folder)
        other = next(r for r in report.rows if r.path == "original/hard")
        self.assertFalse(report.text.supports(selected, other))  # ordinary report still includes hard links
        self.assertFalse(report.text.supports(selected, replace(row)))
        with self.assertRaises(Rejected):
            text_preview.compare_folder_text(selected, report, replace(row), observer=Verification())
        with self.assertRaises(Rejected):
            with report.text.open_pair(replace(selected), row, lambda: None): pass
        fd = selected.held.fileno()
        os.lseek(fd, 17, os.SEEK_SET)
        real_read = os.read
        real_open = os.open
        pieces = []

        def right_only(fd, count):
            self.assertTrue(os.path.samestat(os.fstat(fd), original_right))
            value = real_read(fd, count)
            pieces.append(value)
            return value

        def metadata_open(name, flags, *args, **kwargs):
            if os.fspath(name) in (str(selected.path), str(folder)):
                self.assertTrue(flags & os.O_PATH)
            self.assertFalse(flags & (os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_TRUNC))
            return real_open(name, flags, *args, **kwargs)

        with patch.object(os, "open", side_effect=metadata_open), patch.object(os, "read", side_effect=right_only), \
             patch.object(comparison, "observe_held", side_effect=AssertionError("do not rescan a folder")), \
             patch.object(text_preview, "stored_member", wraps=text_preview.stored_member) as stored:
            value = text_preview.compare_folder_text(selected, report, row, observer=Verification())
        stored.assert_called_once()
        self.assertEqual(stored.call_args.args[3], "original/same")
        self.assertEqual(pieces, [b"<new>\n", b""])
        self.assertEqual(os.lseek(fd, 0, os.SEEK_CUR), 17)
        self.assertIn("-<old>", value)
        self.assertIn("+<new>", value)
        self.assertNotIn("other", value)
        report.close()
        self.assertFalse(report.text.supports(selected, row))
        with self.assertRaises(Rejected):
            text_preview.compare_folder_text(selected, report, row, observer=Verification())

    def test_folder_line_diff_rejects_same_byte_replacement_and_different_actual_reads(self):
        selected, folder = self.compare_folder_fixture()
        report, row = self.folder_report(selected, folder)
        original = os.read
        def wrong(fd, n):
            body = original(fd, n)
            return b"X" + body[1:] if body else body

        with patch.object(os, "read", side_effect=wrong), self.assertRaisesRegex(Rejected, "SHA-256"):
            text_preview.compare_folder_text(selected, report, row, observer=Verification())
        leaf = folder / "original" / "same"
        raw = leaf.read_bytes()
        leaf.rename(leaf.with_name("before")); leaf.write_bytes(raw)
        with patch.object(os, "read", side_effect=AssertionError("cannot read a replacement inode")), \
             self.assertRaisesRegex(Rejected, "原名称、实体或状态"):
            text_preview.compare_folder_text(selected, report, row, observer=Verification())
        # Right root replacement is not authority to follow the old relative name.
        folder.rename(self.dest / "before-folder"); folder.mkdir()
        with self.assertRaisesRegex(Rejected, "原文件夹"):
            text_preview.compare_folder_text(selected, report, row, observer=Verification())

    def test_folder_line_diff_rechecks_the_relative_leaf_before_delivering_text(self):
        selected, folder = self.compare_folder_fixture()
        report, row = self.folder_report(selected, folder)
        leaf = folder / "original" / "same"
        unified = text_preview._unified

        def rename_after_read(*args, **kwargs):
            value = unified(*args, **kwargs)
            old = leaf.with_name("moved")
            leaf.rename(old)
            leaf.symlink_to(old.name)
            return value

        with patch.object(text_preview, "_unified", side_effect=rename_after_read), self.assertRaises(Rejected):
            text_preview.compare_folder_text(selected, report, row, observer=Verification())

    def test_folder_line_diff_uses_text_bounds_and_no_unsupported_mount_fallback(self):
        selected, folder = self.compare_folder_fixture(right=b"\xff")
        report, row = self.folder_report(selected, folder)
        with self.assertRaisesRegex(Rejected, "UTF-8"):
            text_preview.compare_folder_text(selected, report, row, observer=Verification())
        leaf = folder / "original" / "same"
        leaf.write_bytes(b"x\n" * (text_preview.COMPARE_LINES + 1))
        report, row = self.folder_report(selected, folder)
        with self.assertRaisesRegex(Rejected, "1,024 行"):
            text_preview.compare_folder_text(selected, report, row, observer=Verification())
        leaf.write_bytes(b"x" * (comparison.COMPARE_BYTES + 1))
        report, row = self.folder_report(selected, folder)
        self.assertFalse(report.text.supports(selected, row))
        with patch.object(comparison, "local_mount", side_effect=Rejected("unqualified mount")):
            unchanged, listed = self.folder_report(selected, folder)
        self.assertEqual(unchanged.rows, report.rows)
        self.assertEqual(listed, row)
        self.assertIsNone(unchanged.text)

    @unittest.skipUnless(find_spec("PySide6"), "optional native Qt controls")
    def test_folder_line_diff_qt_requires_this_report_and_ignores_late_results(self):
        from PySide6.QtWidgets import QApplication, QPlainTextEdit
        from zipworkspace.desktop import comparison_pane as ui
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        selected, root = self.compare_folder_fixture()
        win = Window()
        pane = win.archive.comparison

        def queue(fn):
            task = Future()
            with patch.object(win.pool, "submit", return_value=task) as work:
                fn()
            self.assertIs(win.pending[0], task)
            return task, work.call_args.args[0]

        try:
            win.archive.show_selection(selected)
            win.archive.views.setCurrentWidget(pane)
            future, action = queue(lambda: pane.compare_path(root))
            future.set_result(action()); win.poll()
            report = pane.report
            pane.table.selectRow(next(i for i, (_, r) in enumerate(pane.table.rows.rows) if r.path == "original/same"))
            self.assertTrue(pane.lines.isEnabled())
            with patch.object(ui, "confirm", return_value=False), \
                 patch.object(win.pool, "submit", side_effect=AssertionError("reading needs this explicit consent")):
                pane.lines.click()
            with patch.object(ui, "confirm", return_value=True) as consent:
                future, action = queue(pane.lines.click)
            self.assertIn(str(root), consent.call_args.args[2])
            self.assertIn("original/same", consent.call_args.args[2])
            future.set_result(action()); win.poll()
            self.assertIn("+<new>", pane.text_dialog.findChild(QPlainTextEdit).toPlainText())
            pane.close_lines()
            with patch.object(ui, "confirm", return_value=True):
                future, action = queue(pane.lines.click)
            result = action()
            self.assertIsNotNone(result[0])
            pane.close_report()  # Cleared before the actual job callback, even though the worker succeeded.
            note = win.note.text()
            future.set_result(result); win.poll()
            self.assertIsNone(pane.text_dialog)
            self.assertEqual(win.note.text(), note)
            self.assertTrue(all(not report.text.supports(selected, r) for r in report.rows))
        finally:
            if win.pending is not None:
                win.pending[0].cancel(); win.poll()
            win.close()

    def test_imports_only_the_held_raw_successor_with_native_noreplace(self):
        selected, store, catalog, parent = self.import_version()
        plan = self.import_plan(selected, store, catalog)
        before, raw = parent.read_bytes(), selected.path.read_bytes()
        original = os.link
        def install(source, name, **kwargs):
            self.assertEqual({p.name for p in self.dest.iterdir()}, {parent.name})
            self.assertEqual(os.fstat(int(source)).st_nlink, 0)
            return original(source, name, **kwargs)
        with patch.object(Selection, "open", side_effect=AssertionError("no source reopened by spelling")), \
             patch.object(Selection, "catalog", side_effect=AssertionError("do not reselect or read a newer head")), \
             patch.object(worktree, "commit", side_effect=AssertionError("no workspace registration")):
            self.assertIn(catalog.anchor.sha256, plan.review())
            with self.assertRaises(Rejected): plan.run()
            with patch.object(os, "link", side_effect=install) as link:
                self.assertIn(str(plan.path), plan.run(confirmed=True))
                link.assert_called_once()
        self.assertEqual({p.name for p in self.dest.iterdir()}, {parent.name, plan.path.name})
        self.assertEqual((plan.path.read_bytes(), selected.path.read_bytes(), parent.read_bytes()), (raw, raw, before))
        with zipfile.ZipFile(plan.path) as z:
            self.assertEqual(z.comment, selected.browser.commit.encode())
            self.assertTrue(z.getinfo("empty/").is_dir())
            self.assertIsNone(z.testzip())
        with self.assertRaises(Rejected): plan.run(confirmed=True)

    def test_original_and_parent_require_exact_zip_bytes_not_just_same_tree(self):
        selected, store, catalog, original_parent = self.import_version()
        wrong_source, _ = self.stored_version(2, same=catalog.anchor.marker, year=2000, path=self.private/"other.zxs")
        dig = hashlib.sha256(wrong_source.read_bytes()).hexdigest()
        self.assertNotEqual(dig, catalog.anchor.sha256)
        with patch.object(catalog, "anchor", replace(catalog.anchor, sha256=dig)):
            plan = self.import_plan(selected, store, catalog)
            plan.review()
            with patch.object(os, "link", side_effect=AssertionError("wrong original raw SHA")), \
                 self.assertRaisesRegex(Rejected, "原始字节不等于"):
                plan.run(confirmed=True)
            self.assertFalse(plan.path.exists())
        predecessor = catalog.nodes[catalog.anchor.marker.parent]
        other, _ = self.stored_version(1, same=predecessor.marker, year=2000, path=self.private/"other-parent.zxs")
        dig = hashlib.sha256(other.read_bytes()).hexdigest()
        self.assertNotEqual(dig, predecessor.sha256)
        false = replace(predecessor, sha256=dig)
        with patch.dict(catalog.nodes, {false.marker.operation: false}), \
             patch.dict(catalog.slots, {(false.marker.branch, false.marker.generation): false}):
            plan = self.import_plan(selected, store, catalog)
            with self.assertRaisesRegex(Rejected, "原上一代不再符合"): plan.review()
        self.assertEqual(list(self.dest.iterdir()), [original_parent])

    def test_review_is_invalidated_by_a_new_generation_or_same_byte_parent_replacement(self):
        selected, store, catalog, parent = self.import_version()
        prior = parent.read_bytes()
        plan = self.import_plan(selected, store, catalog)
        plan.review()
        late, _ = self.stored_version(3, selected.browser.commit.operation)
        with self.assertRaisesRegex(Rejected, "规范槽已增补"): plan.run(confirmed=True)
        self.assertTrue(late.is_file()); self.assertFalse(plan.path.exists())
        late.rename(self.private/"kept-later.zxs")
        plan = self.import_plan(selected, store, catalog)
        plan.review()
        old = parent.rename(self.private/"actual-parent.zxs")
        parent.write_bytes(prior)
        self.assertNotEqual(old.stat().st_ino, parent.stat().st_ino)
        with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertTrue(parent.is_file()); self.assertFalse(plan.path.exists())

    def test_destination_catalog_keeps_space_for_the_new_slot(self):
        selected, store, catalog, parent = self.import_version()
        sidecar = self.dest/"existing-note"
        sidecar.write_bytes(b"not this repo")
        plan = self.import_plan(selected, store, catalog)
        with patch.object(store_import, "STORE_BUDGET", 2), \
             self.assertRaisesRegex(Rejected, "有界目录的上限"):
            plan.review()
        self.assertEqual(set(self.dest.iterdir()), {sidecar, parent})

    def test_last_prepublication_inventory_follows_the_last_source_check(self):
        selected, store, catalog, _ = self.import_version()
        plan = self.import_plan(selected, store, catalog)
        plan.review()
        original, late = plan._source, []
        def read_source():
            original()
            if not late and "读回" in plan.progress()[0]:
                late.append(self.stored_version(3, selected.browser.commit.operation)[0])
        with patch.object(plan, "_source", side_effect=read_source), \
             patch.object(os, "link", side_effect=AssertionError("stale evidence must not publish")), \
             self.assertRaisesRegex(Rejected, "规范槽已增补"):
            plan.run(confirmed=True)
        self.assertEqual(len(late), 1)
        self.assertTrue(late[0].is_file())
        self.assertFalse(plan.path.exists())

    def test_exact_bytes_late_collision_stays_a_refusal_not_a_success(self):
        selected, store, catalog, parent = self.import_version()
        plan = self.import_plan(selected, store, catalog)
        plan.review()
        raw, actual = selected.path.read_bytes(), os.link
        def occupy(source, name, **kwargs):
            with plan.path.open("xb") as incumbent: incumbent.write(raw)
            actual(source, name, **kwargs)
        with patch.object(os, "link", side_effect=occupy) as link, \
             self.assertRaisesRegex(Rejected, "目的名称已被占用"):
            plan.run(confirmed=True)
        link.assert_called_once()
        self.assertEqual(plan.path.read_bytes(), raw)
        self.assertEqual(set(self.dest.iterdir()), {parent, plan.path})

    def test_post_link_parent_barrier_failure_remains_unknown_and_does_not_cleanup(self):
        selected, store, catalog, parent = self.import_version()
        plan = self.import_plan(selected, store, catalog)
        plan.review()
        expected, actual = self.dest.stat(), os.fsync
        def sync(fd):
            if os.path.samestat(os.fstat(fd), expected) and plan.path.exists():
                raise OSError(errno.EIO, "synthetic original STORE barrier error")
            actual(fd)
        with patch.object(os, "fsync", side_effect=sync), self.assertRaises(Uncertain):
            plan.run(confirmed=True)
        self.assertEqual(plan.path.read_bytes(), selected.path.read_bytes())
        self.assertEqual(set(self.dest.iterdir()), {parent, plan.path})
        with self.assertRaises(Rejected): plan.run(confirmed=True)

    @unittest.skipUnless(find_spec("PySide6"), "optional Qt")
    def test_original_qt_confirmation_cannot_publish_after_its_catalog_is_revoked(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import archive_pane
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        selected, store, catalog, parent = self.import_version()
        pane = win.archive
        pane.show_selection(selected); pane.use_store(store); pane.show_catalog(catalog)
        self.assertTrue(pane.import_version.isEnabled())
        pending = []
        def enqueue(task):
            original = Future()
            pending.append((original, task))
            return original
        def confirm(*args, **kwargs):
            pane.clear_catalog()  # Programmatic change inside the modal dialog loop.
            return True
        try:
            with patch.object(win.pool, "submit", side_effect=enqueue), \
                 patch.object(archive_pane, "confirm", side_effect=confirm) as asked, \
                 patch.object(store_import.StoreSuccessor, "run", side_effect=AssertionError("old decision")):
                pane.import_version.click()
                first, prepare = pending.pop()
                self.assertTrue(first.set_running_or_notify_cancel())
                first.set_result(prepare())
                win.poll()
                asked.assert_called_once()
                self.assertEqual(pending, [])
                self.assertIsNone(pane.store_import_request)
                self.assertEqual(list(self.dest.iterdir()), [parent])
        finally:
            if win.pending is not None:
                win.pending[0].cancel(); win.poll()
            win.close()

    def original_folder(self):
        original = self.private/"folder"
        original.mkdir(mode=0o700)
        for name in ("one", "two", "empty"): (original/name).mkdir(mode=0o700)
        (original/"one/name").write_bytes(self.data)
        (original/"two/name").write_bytes(b"different")
        return original

    def test_chosen_folder_can_make_a_first_standalone_zip_without_registering(self):
        original = self.original_folder()
        selection = Folder.choose(str(original))
        plan = member_copy.FolderSnapshot(selection, str(self.dest/"standalone.zxs"))
        self.addCleanup(plan.close)
        with patch.object(worktree, "create_catalog", side_effect=AssertionError("no STATE")), \
             patch.object(worktree, "commit", side_effect=AssertionError("no commit")), \
             patch.object(worktree, "anonymous_capture", side_effect=AssertionError("no second frozen copy")):
            self.assertIn("不为目录建立 STATE", plan.review())
            self.assertFalse(plan.path.exists())
            with self.assertRaises(Rejected): plan.run()
            actual = os.link
            def link(source, name, **kw):
                self.assertFalse(plan.path.exists())
                self.assertEqual(os.fstat(int(source)).st_nlink, 0)
                return actual(source, name, **kw)
            with patch.object(os, "link", side_effect=link) as native:
                message = plan.run(confirmed=True)
                native.assert_called_once()
        self.assertIsNone(selection._fd)
        result = Selection.open(plan.path)
        self.addCleanup(result.release_display)
        marker = result.browser.commit
        self.assertEqual((marker.repo, marker.branch, marker.operation, marker.generation, marker.parent),
                         (plan.repo, plan.branch, plan.operation, 1, None))
        self.assertIn(marker.operation, message)
        with zipfile.ZipFile(plan.path) as z:
            self.assertIsNone(z.testzip())
            self.assertEqual(z.read("one/name"), self.data)
            self.assertEqual(z.read("two/name"), b"different")
            self.assertTrue(z.getinfo("empty/").is_dir())
            self.assertTrue(all(info.compress_type == zipfile.ZIP_STORED for info in z.infolist()))
        self.assertEqual(list(self.dest.iterdir()), [plan.path])
        self.assertEqual((original/"one/name").read_bytes(), self.data)
        previous = plan.path.read_bytes()
        occupied = member_copy.FolderSnapshot(Folder.choose(str(original)), str(plan.path))
        with self.assertRaisesRegex(Rejected, "已有文件"): occupied.review()
        self.assertEqual(plan.path.read_bytes(), previous)

    def test_folder_snapshot_checks_source_identity_and_excludes_an_inside_destination(self):
        original = self.original_folder()
        source = Folder.choose(str(original))
        inside = member_copy.FolderSnapshot(source, str(original/"one/ignored.zxs"))
        with self.assertRaisesRegex(Rejected, "原目录之外"): inside.review()
        self.assertIsNone(source._fd)
        self.assertFalse(inside.path.exists())
        chosen = Folder.choose(str(original))
        accepted = member_copy.FolderSnapshot(chosen, str(self.dest/"unchanged.zxs"))
        self.addCleanup(accepted.close)
        accepted.review()
        old = original.rename(original.with_name("old-folder"))
        self.original_folder()  # New inode and files of identical size/content.
        with patch.object(os, "link", side_effect=AssertionError("replacement cannot be published")):
            with self.assertRaises(Rejected): accepted.run(confirmed=True)
        self.assertIsNone(chosen._fd)
        self.assertFalse(accepted.path.exists())
        self.assertEqual((old/"two/name").read_bytes(), b"different")

    def test_folder_snapshot_shows_uncertain_after_the_real_link_if_only_parent_sync_fails(self):
        original = self.original_folder()
        plan = member_copy.FolderSnapshot(Folder.choose(str(original)), str(self.dest/"maybe.zxs"))
        self.addCleanup(plan.close)
        plan.review()
        real_sync = os.fsync
        def sync(fd):
            if os.path.samestat(os.fstat(fd), self.dest.stat()):
                self.assertTrue(plan.path.is_file())  # The only new name is already a complete candidate.
                raise OSError(errno.EIO, "parent sync")
            return real_sync(fd)
        with patch.object(os, "fsync", side_effect=sync):
            with self.assertRaisesRegex(Uncertain, "完整副本可能已经出现"): plan.run(confirmed=True)
        with zipfile.ZipFile(plan.path) as z:
            self.assertEqual(z.read("one/name"), self.data)
        with self.assertRaises(Rejected): plan.run(confirmed=True)

    def test_cancel_folder_after_readback_does_not_publish_or_leave_a_named_partial(self):
        original = self.original_folder()
        plan = member_copy.FolderSnapshot(Folder.choose(str(original)), str(self.dest/"cancelled.zxs"))
        self.addCleanup(plan.close)
        plan.review()
        real_verify = plan._verify
        def after_readback(*args):
            observation = real_verify(*args)
            self.assertTrue(plan.cancel())
            return observation
        with patch.object(plan, "_verify", side_effect=after_readback), \
             patch.object(os, "link", side_effect=AssertionError("cancelled before publication")):
            with self.assertRaises(member_copy.CancelledCopy): plan.run(confirmed=True)
        self.assertEqual(list(self.dest.iterdir()), [])

    @unittest.skipUnless(find_spec("PySide6"), "optional native Qt controls")
    def test_folder_snapshot_uses_actual_menu_and_explicit_new_path_confirmation(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import archive_pane
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        self.addCleanup(win.close)
        original = self.original_folder()
        target = self.dest/"explicit.zxs"
        action = next(a for a in win.menuBar().actions()[0].menu().actions() if "生成全新 .zxs" in a.text())
        tasks = []
        def submit(work):
            future = Future()
            tasks.append((future, work))
            return future
        def poll():
            result, work = tasks.pop(0)
            result.set_result(work())
            win.poll()
        with patch.object(win.pool, "submit", side_effect=submit), \
             patch.object(archive_pane.QFileDialog, "getExistingDirectory", return_value=str(original)), \
             patch.object(archive_pane.QFileDialog, "getSaveFileName", return_value=(str(target), "")):
            for permitted in (False, True):
                with patch.object(archive_pane, "confirm", return_value=permitted) as confirm:
                    action.trigger()
                    self.assertFalse(target.exists())
                    poll()
                    self.assertFalse(target.exists())
                    confirm.assert_called_once()
                    self.assertIn(str(original), confirm.call_args.args[2])
                    self.assertIn(str(target), confirm.call_args.args[2])
                    self.assertEqual(len(tasks), int(permitted))
                    if permitted: poll()
                    self.assertIsNone(win.archive.folder_snapshot)
                    self.assertIsNone(win.archive.checkout.live)
                    self.assertIsNone(win.archive.selection)
        self.assertTrue(target.is_file())

    def test_managed_zip_copy_round_trips_through_standard_reader_and_plain_extraction(self):
        plan = self.zip_plan()
        fd = self.selected.held.fileno()
        os.lseek(fd, 11, os.SEEK_SET)
        details = plan.review()
        self.assertIn("逐字节另存原 Store ZIP", details)
        self.assertFalse(plan.path.exists())
        with self.assertRaises(Rejected): plan.run()
        real_link = os.link
        def link(source, name, **kwargs):
            m = os.fstat(int(source))
            self.assertEqual((m.st_size, m.st_nlink), (len(self.original), 0))
            self.assertFalse(plan.path.exists())
            real_link(source, name, **kwargs)
        with patch.object(member_copy.os, "link", side_effect=link) as native:
            self.assertIn("已另存并核验", plan.run(confirmed=True))
            native.assert_called_once()
        self.assertEqual(os.lseek(fd, 0, os.SEEK_CUR), 11)
        self.assertEqual(plan.path.read_bytes(), self.original)
        self.assertEqual(self.archive.read_bytes(), self.original)
        self.assertEqual(stat.S_IMODE(plan.path.stat().st_mode), 0o600)
        with zipfile.ZipFile(plan.path) as ordinary:
            self.assertIsNone(ordinary.testzip())
            self.assertEqual(ordinary.read("original/same.bin"), self.data)
            self.assertTrue(ordinary.getinfo("empty-root/").is_dir())
            self.assertEqual(ordinary.read("other/same.bin"), b"x" * (2*CHUNK))
            self.assertEqual(ordinary.comment, self.selected.browser.commit.encode())

        # Feed that exact output (not a second generated fixture) to the real
        # ordinary ZIP preview and directory writer, without interpreting its marker.
        selected_zip = PlainSelection.open(str(plan.path))
        self.addCleanup(selected_zip.release_display)
        single = self.dest/"one-saved-member.bin"
        row = selected_zip.index.entry("original", "same.bin")
        other = selected_zip.index.entry("other", "same.bin")
        with self.assertRaises(Rejected):
            member_copy.MemberCopy(selected_zip, "original", other, str(single))
        save_one = member_copy.MemberCopy(selected_zip, "original", row, str(single))
        self.addCleanup(save_one.close)
        details = save_one.review()
        self.assertIn(selected_zip.preview.sha256, details)
        self.assertIn("original/same.bin", details)
        self.assertNotIn("节点：", details)
        self.assertFalse(single.exists())
        with patch.object(member_copy, "stored_member", wraps=member_copy.stored_member) as bind:
            save_one.run(confirmed=True)
        self.assertEqual(bind.call_args.args[2:5], (None, "original/same.bin", len(self.data)))
        self.assertEqual(single.read_bytes(), self.data)
        self.assertEqual(stat.S_IMODE(single.stat().st_mode), 0o600)
        self.assertCountEqual(self.dest.iterdir(), [plan.path, single])
        retained = single.stat()
        occupied = member_copy.MemberCopy(selected_zip, "original", row, str(single))
        self.addCleanup(occupied.close)
        with self.assertRaises(Rejected): occupied.review()
        self.assertTrue(os.path.samestat(retained, single.stat()))
        self.assertEqual(single.read_bytes(), self.data)

        target = self.dest/"round-trip"
        extracted = member_copy.PlainZipCopy(selected_zip, str(target))
        self.addCleanup(extracted.close)
        extracted.pick_private(str(self.private)); extracted.review()
        extracted.run(confirmed=True)
        original_note, other_note = target/"original/same.bin", target/"other/same.bin"
        self.assertEqual(original_note.read_bytes(), self.data)
        self.assertEqual(other_note.read_bytes(), b"x" * (2*CHUNK))
        self.assertEqual(list((target/"empty-root").iterdir()), [])
        before_target, before_note = target.stat(), original_note.stat()

        again = member_copy.PlainZipCopy(selected_zip, str(target))
        self.addCleanup(again.close)
        again.pick_private(str(self.private))
        with patch.object(member_copy.private_stage, "new_child", side_effect=AssertionError("must reject before staging")):
            with self.assertRaises(Rejected): again.review()
            with self.assertRaises(Rejected): again.run(confirmed=True)
        self.assertTrue(os.path.samestat(before_target, target.stat()))
        self.assertTrue(os.path.samestat(before_note, original_note.stat()))
        self.assertEqual(original_note.read_bytes(), self.data)
        self.assertEqual(other_note.read_bytes(), b"x" * (2*CHUNK))
        self.assertEqual(list((target/"empty-root").iterdir()), [])
        self.assertEqual(plan.path.read_bytes(), self.original)
        self.assertEqual(self.archive.read_bytes(), self.original)
        self.assertEqual(list(self.private.iterdir()), [])

    def test_space_advisory_uses_the_original_output_mount_and_does_not_reserve_or_block(self):
        archive = self.zip_plan()
        def tiny(fd):
            self.assertTrue(os.path.samestat(os.fstat(fd), self.dest.stat()))
            return SimpleNamespace(f_frsize=1, f_blocks=10, f_bfree=2, f_bavail=0)
        with patch.object(member_copy.os, "fstatvfs", side_effect=tiny) as observed:
            hint = archive.review()
            observed.assert_called_once()
        self.assertIn(f"新增的文件逻辑量：{len(self.original):,} B", hint)
        self.assertIn("当前报告普通用户可用 0 B", hint)
        self.assertIn("可能不足", hint)
        self.assertFalse(archive.path.exists())
        archive.run(confirmed=True)  # A reporting estimate is neither a reservation nor a hard refusal.
        self.assertEqual(archive.path.read_bytes(), self.original)

        subtree = self.subtree_plan()
        def unavailable(fd):
            self.assertTrue(os.path.samestat(os.fstat(fd), self.private.stat()))
            raise OSError(errno.EIO, "unavailable capacity estimate")
        with patch.object(member_copy.os, "fstatvfs", side_effect=unavailable) as observed:
            self.assertIn("当前可用量未知", subtree.review())
            observed.assert_called_once()
        self.assertFalse(subtree.path.exists())
        self.assertEqual(list(self.private.iterdir()), [])

    def test_copy_stage_byte_counts_use_actual_writes_and_full_readback_not_a_success_percentage(self):
        plan = self.zip_plan()
        plan.review()
        real_phase, seen = plan._phase, []
        def phase(label, total=None):
            seen.append(plan.progress())
            real_phase(label, total)
        with patch.object(plan, "_phase", side_effect=phase):
            plan.run(confirmed=True)
        physical = len(self.original)
        self.assertIn(("正在写入未发布的匿名副本", physical, physical), seen)
        self.assertIn(("正在核验匿名 ZIP 里的完整目录和成员", 0, None), seen)
        self.assertIn(("正在读回未发布的匿名副本", physical, physical), seen)
        self.assertEqual(plan.progress(), ("正在正式发布并同步所选父目录", 0, None))
        self.assertEqual(plan.path.read_bytes(), self.original)

    def test_copy_can_cancel_after_copying_but_before_complete_zip_readback(self):
        plan = self.zip_plan()
        plan.review()
        real = member_copy.read_managed_document
        def readback(doc, **kwargs):
            self.assertTrue(plan.cancel())
            self.assertFalse(plan.path.exists())
            return real(doc, **kwargs)  # The actual guarded readback observes cancellation.
        with patch.object(member_copy, "read_managed_document", side_effect=readback), \
             patch.object(member_copy.os, "link", side_effect=AssertionError("must not publish a cancelled copy")):
            with self.assertRaisesRegex(member_copy.CancelledCopy, "未发布"):
                plan.run(confirmed=True)
        self.assertFalse(plan.path.exists())
        self.assertEqual(self.archive.read_bytes(), self.original)
        self.assertFalse(plan.cancel())  # The consumed plan can't become a new operation.

    def test_cancellation_cannot_undo_the_single_link_once_committed_to_it(self):
        plan = self.zip_plan()
        plan.review()
        real = os.link
        def link(*args, **kwargs):
            self.assertFalse(plan.cancel())
            return real(*args, **kwargs)
        with patch.object(member_copy.os, "link", side_effect=link) as native:
            plan.run(confirmed=True)
            native.assert_called_once()
        self.assertEqual(plan.path.read_bytes(), self.original)

    @unittest.skipUnless(find_spec("PySide6"), "optional native Qt controls")
    def test_cancel_button_stays_reachable_when_only_archive_tabs_are_disabled(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        self.addCleanup(win.close)
        pending, taps = Future(), []
        phase = [("读取原档", 3, 20)]
        with patch.object(win.pool, "submit", return_value=pending):
            win.job("复制原包", lambda: self.fail("fixture only controls the Future"),
                    lambda _: self.fail("cancelled is not a completed result"),
                    cancel=lambda: taps.append(True) or True, progress_report=lambda: phase[0])
        win.poll()
        self.assertEqual(win.progress.format(), "3 / 20 B")
        self.assertIn("不代表已经完成", win.note.text())
        self.assertFalse(win.tabs.isEnabled())
        self.assertTrue(win.transfer_cancel.isEnabled())
        self.assertFalse(win.transfer_cancel.isHidden())
        win.transfer_cancel.click()
        self.assertEqual(taps, [True])
        phase[0] = ("不应覆盖已受理取消", 8, 20)
        win.poll()
        self.assertIn("等待正在进行的系统调用", win.note.text())
        pending.set_exception(member_copy.CancelledCopy("已取消，未发布"))
        win.poll()
        self.assertTrue(win.transfer_cancel.isHidden())
        self.assertTrue(win.tabs.isEnabled())
        self.assertIn("未发布", win.note.text())

    def test_raw_copy_with_a_damaged_member_never_links_even_if_the_source_name_is_unchanged(self):
        plan = self.zip_plan()
        plan.review()
        from zipworkspace.archive import LOCAL
        with self.archive.open("rb") as fh:
            fixed = LOCAL.unpack(fh.read(LOCAL.size))
        body = LOCAL.size + fixed[-2] + fixed[-1]
        actual = os.pread
        def damaged(fd, size, offset):
            data = actual(fd, size, offset)
            if fd == plan.source and offset <= body < offset+len(data):
                at = body-offset
                return data[:at]+bytes([data[at]^1])+data[at+1:]
            return data
        with patch.object(member_copy.os, "pread", side_effect=damaged), \
             patch.object(member_copy.os, "link", side_effect=AssertionError("reject before exposing a ZIP")):
            with self.assertRaises(zipfile.BadZipFile): plan.run(confirmed=True)
        self.assertEqual(list(self.dest.iterdir()), [])
        self.assertEqual(self.archive.read_bytes(), self.original)

    def plain_plan(self, name="plain-output", *, offer_work=False):
        original = self.archive.with_name("plain.zip")
        with original.open("xb") as raw, zipfile.ZipFile(raw, "w") as archive:
            archive.comment = b"a plain ZIP; no Workspace identity"
            info = zipfile.ZipInfo("folder/note")
            info.external_attr = (stat.S_IFREG | 0o755) << 16
            archive.writestr(info, b"file payload")
            archive.writestr("empty/", b"")
            archive.writestr("zero", b"")
        selected = PlainSelection.open(str(original))
        self.addCleanup(selected.release_display)
        plan = member_copy.PlainZipCopy(selected, str(self.dest/name), offer_work=offer_work)
        plan.pick_private(str(self.private))
        self.addCleanup(plan.close)
        return original, selected, plan

    def test_plain_zip_explicitly_extracts_all_verified_bytes_into_one_new_directory(self):
        original, selected, plan = self.plain_plan()
        before = original.read_bytes()
        display = selected.held.fileno()
        os.lseek(display, 7, os.SEEK_SET)
        details = plan.review()
        self.assertIn(selected.preview.tree, details)
        self.assertFalse(plan.path.exists())
        self.assertEqual(list(self.private.iterdir()), [])
        with self.assertRaises(Rejected): plan.run()
        real = member_copy.rename_new_at
        def publish(parent, name, target, **kw):
            stage = self.private/name
            self.assertFalse(plan.path.exists())
            self.assertEqual((stage/"folder/note").read_bytes(), b"file payload")
            self.assertEqual(list((stage/"empty").iterdir()), [])
            self.assertEqual(stat.S_IMODE((stage/"folder/note").stat().st_mode), 0o600)
            self.assertEqual((stage/"zero").read_bytes(), b"")
            self.assertTrue(os.path.samestat(kw["expected_source"], stage.stat()))
            real(parent, name, target, **kw)
        with patch.object(member_copy, "rename_new_at", side_effect=publish) as native:
            self.assertIn("已另存并核验目录", plan.run(confirmed=True))
            native.assert_called_once()
        self.assertEqual(os.lseek(display, 0, os.SEEK_CUR), 7)
        self.assertEqual(original.read_bytes(), before)
        self.assertEqual(sorted(x.name for x in plan.path.iterdir()), ["empty", "folder", "zero"])
        self.assertEqual(list(self.private.iterdir()), [])

    def copied_work(self):
        # Even a valid managed comment remains opaque in the ordinary ZIP workflow.
        source = self.archive.with_suffix(".zip")
        source.write_bytes(self.original)
        selected = PlainSelection.open(str(source))
        self.addCleanup(selected.release_display)
        plan = member_copy.PlainZipCopy(selected, str(self.dest/"edited-copy"), offer_work=True)
        self.addCleanup(plan.close)
        plan.pick_private(str(self.private)); plan.review()
        delivered = plan.run(confirmed=True)
        self.assertIn("已另存并核验目录", delivered.message)
        self.assertIsNotNone(delivered.work)
        self.addCleanup(delivered.work.close)
        self.assertEqual(list(self.private.iterdir()), [])
        return delivered.work, source

    def register_copy(self, handoff, *, leaf="new-state", store=None):
        if store is None:
            store = self.archive.parent/"repo-out"
            store.mkdir(mode=0o700)
        choice = InitializeChoice()
        self.addCleanup(choice.close)
        with patch.object(member_copy.Folder, "choose", side_effect=AssertionError("do not pick a new work path")):
            choice.pick_extracted(handoff)
        choice.pick_private(self.private); choice.pick_store(store)
        self.assertIn(handoff.archive_sha256, choice.review(leaf))
        self.assertFalse(choice.state.exists())
        self.assertEqual(list(store.iterdir()), [])
        return choice, store

    def test_completed_plain_extraction_offers_the_same_worktree_for_a_separate_genesis(self):
        original, external_zip = self.copied_work()
        original_fd = original.original._fd
        self.assertTrue(os.path.samestat(os.fstat(original_fd), original.path.stat()))
        local_story = original.path/"original/same.bin"
        local_story.write_bytes(b"my first local edit")
        choice, store = self.register_copy(original)
        second, _ = self.register_copy(original, leaf="duplicate", store=store)
        with patch.object(lifecycle, "observe_held", side_effect=AssertionError("registration doesn't rescan")), \
             patch.object(worktree, "commit", side_effect=AssertionError("registration cannot commit")):
            with self.assertRaises(Rejected): choice.run()
            self.assertFalse(choice.state.exists())
            choice.run(confirmed=True)
            with self.assertRaisesRegex(Rejected, "不能重用"): second.run(confirmed=True)
        self.assertTrue(original.closed)
        self.assertFalse(second.state.exists())
        live = choice.take_session()
        self.addCleanup(live.close)
        self.assertTrue(os.path.samestat(os.fstat(live.work_fd), original.path.stat()))
        self.assertEqual(live.status()["base_generation"], 0)
        self.assertNotEqual(live.repo, self.selected.browser.commit.repo)
        self.assertEqual(list(store.iterdir()), [])

        attempt = CommitChoice(live)
        self.addCleanup(attempt.release)
        attempt.prepare()
        slot, marker = attempt.run()
        self.assertEqual((marker.repo, marker.generation, marker.parent), (live.repo, 1, None))
        with zipfile.ZipFile(slot) as checked:
            self.assertEqual(checked.read("original/same.bin"), b"my first local edit")
            self.assertTrue(checked.getinfo("empty-root/").is_dir())
        self.assertEqual(external_zip.read_bytes(), self.original)

    def test_same_named_extracted_replacement_or_failed_barrier_never_gives_a_registration(self):
        original, _ = self.copied_work()
        choice, store = self.register_copy(original)
        original.path.rename(original.path.with_name("actual-kept"))
        original.path.mkdir(mode=0o700)
        with self.assertRaises(Rejected): choice.run(confirmed=True)
        self.assertIsNone(choice.take_session())
        self.assertFalse(choice.state.exists())
        self.assertEqual(list(store.iterdir()), [])

        _, _, incomplete = self.plain_plan("late-sync", offer_work=True)
        incomplete.review()
        native = os.fsync
        def sync(fd):
            if os.path.samestat(os.fstat(fd), self.dest.stat()): raise OSError(errno.EIO, "injected original parent sync failure")
            return native(fd)
        with patch.object(os, "fsync", side_effect=sync), \
             patch.object(incomplete, "_handoff", side_effect=AssertionError("no work custody before both barriers")):
            with self.assertRaises(Uncertain): incomplete.run(confirmed=True)
        self.assertTrue(incomplete.path.is_dir())

    def test_successful_plain_extraction_does_not_invent_a_worktree_when_ticket_allocation_fails(self):
        _, _, extraction = self.plain_plan("complete-without-ticket", offer_work=True)
        extraction.review()
        with patch.object(extraction, "_handoff", side_effect=OSError(errno.EMFILE, "synthetic descriptor budget")):
            published = extraction.run(confirmed=True)
        self.assertIn("已另存并核验目录", published.message)
        self.assertIsNone(published.work)
        self.assertEqual((extraction.path/"folder/note").read_bytes(), b"file payload")
        self.assertEqual(list(self.private.iterdir()), [])

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1" and find_spec("PySide6"),
                         "optional Linux desktop fixture")
    def test_actual_zip_window_offers_but_does_not_register_without_another_confirmation(self):
        from PySide6.QtWidgets import QApplication, QFileDialog, QInputDialog
        from zipworkspace.desktop import checkout_pane, import_pane
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        _, selected, fixture = self.plain_plan()
        fixture.close()
        destination = self.dest/"window-work"
        store = self.archive.parent/"window-store"
        store.mkdir(mode=0o700)
        state = self.private/"window-state"
        win = Window()
        pane, checkout = win.plain_import, win.archive.checkout
        jobs = []

        def queue(work):
            future = Future()
            jobs.append((future, work))
            return future

        def complete():
            future, work = jobs.pop(0)
            future.set_result(work())
            win.poll()

        try:
            pane.show_selection(selected)
            win.tabs.setCurrentWidget(pane)
            self.assertFalse(pane.enroll.isEnabled())
            with patch.object(win.pool, "submit", side_effect=queue), \
                 patch.object(QFileDialog, "getSaveFileName", return_value=(str(destination), "")), \
                 patch.object(QFileDialog, "getExistingDirectory", return_value=str(self.private)), \
                 patch.object(import_pane, "confirm", return_value=True) as extraction:
                pane.extract.click()
                complete(); complete()
                extraction.assert_called_once()
                self.assertTrue(destination.is_dir())
                self.assertTrue(pane.enroll.isEnabled())
                self.assertIsNone(checkout.live)
                self.assertFalse(state.exists())
                for allow in (False, True):
                    win.tabs.setCurrentWidget(pane)
                    with patch.object(checkout, "_directory", side_effect=[str(store), str(self.private)]) as where, \
                         patch.object(QInputDialog, "getText", return_value=(state.name, True)), \
                         patch.object(checkout_pane, "confirm", return_value=allow) as registration:
                        pane.enroll.click()
                        self.assertIs(win.archive.views.currentWidget(), checkout)
                        complete()
                    self.assertEqual(where.call_count, 2)  # No work directory picker after verified extraction.
                    registration.assert_called_once()
                    self.assertFalse(state.exists())
                    self.assertEqual(len(jobs), int(allow))
                    while jobs: complete()
                self.assertIsNotNone(checkout.live)
                self.assertEqual(checkout.live.status()["base_generation"], 0)
                self.assertTrue(os.path.samestat(os.fstat(checkout.live.work_fd), destination.stat()))
                self.assertEqual(list(store.iterdir()), [])
                win.tabs.setCurrentWidget(pane)
                self.assertFalse(pane.enroll.isEnabled())
        finally:
            if win.pending is not None:
                win.pending[0].cancel(); win.poll()
            win.close()

    def test_plain_zip_replacement_and_crc_failure_never_publish_an_extraction(self):
        original, _, plan = self.plain_plan()
        plan.review()
        from zipworkspace.archive import LOCAL
        with original.open("rb") as raw:
            fields = LOCAL.unpack(raw.read(LOCAL.size))
            body = LOCAL.size + fields[-2] + fields[-1]
        real = os.pread
        def corrupt(fd, n, offset):
            data = real(fd, n, offset)
            if fd == plan.source and offset <= body < offset + len(data):
                i = body-offset
                return data[:i] + bytes([data[i]^1]) + data[i+1:]
            return data
        with patch.object(member_copy.os, "pread", side_effect=corrupt), \
             patch.object(member_copy, "rename_new_at", side_effect=AssertionError("never publish damaged data")):
            with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertFalse(plan.path.exists())
        self.assertTrue((self.private/plan.stage_name).is_dir())

        # A fresh inspection still cannot adopt an equal-bytes pathname replacement.
        selected = PlainSelection.open(str(original))
        self.addCleanup(selected.release_display)
        following = member_copy.PlainZipCopy(selected, str(self.dest/"second"))
        self.addCleanup(following.close)
        following.pick_private(str(self.private)); following.review()
        bytes_before = original.read_bytes()
        original.rename(original.with_name("prior.zip"))
        original.write_bytes(bytes_before)
        with patch.object(member_copy.private_stage, "new_child", side_effect=AssertionError("reject replaced original first")):
            with self.assertRaises(Rejected): following.run(confirmed=True)
        self.assertFalse(following.path.exists())

    def history_plan(self):
        source_store = self.archive.parent / "history-origin"
        source_store.mkdir(mode=0o700)
        manifest = Manifest()
        manifest.add("file", False, 4, hashlib.sha256(b"body").hexdigest())
        repo, main, branch, tree = "d"*32, "e"*32, "f"*32, manifest.digest()
        first = Commit(repo, main, 1, "1"*32, None, tree)
        parent = Commit(repo, main, 2, "2"*32, first.operation, tree)
        tip = Commit(repo, branch, 1, "3"*32, parent.operation, tree)
        after = Commit(repo, main, 3, "4"*32, parent.operation, tree)
        for marker in (first, parent, tip, after):
            with (source_store/marker.filename).open("xb") as raw, zipfile.ZipFile(raw, "w") as z:
                z.writestr("file", b"body")
                z.comment = marker.encode()
        selected = Selection.open(source_store/tip.filename, retain=True)
        self.addCleanup(selected.release_display)
        choice = StoreSelection.open(source_store)
        catalog = selected.catalog(choice)
        plan = member_copy.HistoryCopy(selected, choice, catalog, str(self.dest/"local-store"))
        self.addCleanup(plan.close)
        plan.pick_private(str(self.private))
        return plan, source_store, selected, choice, catalog, (first, parent, tip, after)

    def test_copy_only_the_original_complete_ancestry_as_one_new_store(self):
        plan, original, selected, _, _, (first, parent, tip, unrelated) = self.history_plan()
        display = selected.held.fileno()
        os.lseek(display, 5, os.SEEK_SET)
        files = {p.name: p.read_bytes() for p in original.iterdir()}
        details = plan.review()
        self.assertIn(tip.operation, details)
        self.assertFalse(plan.path.exists())
        self.assertEqual(list(self.private.iterdir()), [])
        with self.assertRaises(Rejected): plan.run()
        included = {first.filename, parent.filename, tip.filename}
        real = member_copy.rename_new_at
        def publish(pfd, leaf, dest, **kw):
            stage = self.private/leaf
            self.assertFalse(plan.path.exists())
            self.assertEqual({p.name for p in stage.iterdir()}, included)
            self.assertTrue(os.path.samestat(kw["expected_source"], stage.stat()))
            real(pfd, leaf, dest, **kw)
        with patch.object(member_copy, "rename_new_at", side_effect=publish) as rename:
            plan.run(confirmed=True)
            rename.assert_called_once()
        self.assertEqual(os.lseek(display, 0, os.SEEK_CUR), 5)
        self.assertEqual({p.name:p.read_bytes() for p in plan.path.iterdir()}, {n:files[n] for n in included})
        self.assertEqual({p.name:p.read_bytes() for p in original.iterdir()}, files)
        self.assertFalse((plan.path/unrelated.filename).exists())
        local_store = StoreSelection.open(plan.path)
        local = Selection.open(plan.path/tip.filename)
        self.assertEqual(local.catalog(local_store).select_branch(tip.branch).marker, tip)

    def test_all_observed_complete_branches_are_copied_without_adding_late_or_other_repo_slots(self):
        branch_only, origin, selected, store, catalog, (first, parent, tip, other) = self.history_plan()
        branch_only.close()
        plan = member_copy.HistoryCopy(selected, store, catalog, str(self.dest/"all-observed"), all_observed=True)
        self.addCleanup(plan.close)
        plan.pick_private(str(self.private))
        detail = plan.review()
        self.assertIn("2 个分支、4 个规范版本", detail)
        self.assertIn("不同内容", detail)
        late = Commit(first.repo, other.branch, 4, "7"*32, other.operation, other.tree)
        elsewhere = Commit("a"*32, "b"*32, 1, "8"*32, None, first.tree)
        for marker in (late, elsewhere):
            with zipfile.ZipFile(origin/marker.filename, "x") as z:
                z.writestr("file", b"body"); z.comment = marker.encode()
        before = {p.name: p.read_bytes() for p in origin.iterdir()}
        plan.run(confirmed=True)
        included = {item.filename for item in (first, parent, tip, other)}
        self.assertEqual({p.name:p.read_bytes() for p in plan.path.iterdir()}, {name:before[name] for name in included})
        self.assertEqual({p.name:p.read_bytes() for p in origin.iterdir()}, before)
        verified = Selection.open(plan.path / tip.filename).catalog(StoreSelection.open(plan.path))
        self.assertEqual(verified.select_branch(tip.branch).marker, tip)
        self.assertEqual(verified.select_branch(other.branch).marker, other)

    def test_repo_copy_refuses_unresolved_observed_branch_even_when_chosen_chain_is_complete(self):
        branch_only, origin, selected, store, _, (first, _, tip, _) = self.history_plan()
        branch_only.close()
        missing = Commit(first.repo, "c"*32, 1, "8"*32, "9"*32, first.tree)
        with zipfile.ZipFile(origin/missing.filename, "x") as z:
            z.writestr("file", b"body"); z.comment = missing.encode()
        catalog = selected.catalog(store)
        self.assertIsNone(catalog.history(tip.operation)[1])
        self.assertEqual(catalog.store_gaps[missing.operation], missing.parent)
        self.assertFalse(member_copy.HistoryCopy.complete(catalog))
        with self.assertRaisesRegex(Rejected, "不完整"):
            member_copy.HistoryCopy(selected, store, catalog, str(self.dest/"must-not-start"), all_observed=True)
        one = member_copy.HistoryCopy(selected, store, catalog, str(self.dest/"still-only-one"))
        self.addCleanup(one.close)
        self.assertNotIn(missing, tuple(node.marker for node in one.originals))

    def test_repo_copy_rechecks_a_side_branch_slot_after_copy_before_publication(self):
        branch_only, origin, selected, store, catalog, (_, _, _, side) = self.history_plan()
        branch_only.close()
        plan = member_copy.HistoryCopy(selected, store, catalog, str(self.dest/"multi-replaced"), all_observed=True)
        self.addCleanup(plan.close)
        plan.pick_private(str(self.private)); plan.review()
        native_sync = member_copy.private_stage._Output.sync
        def replace_after_copy(sink, *args, **kw):
            native_sync(sink, *args, **kw)
            chosen = origin/side.filename
            retained = origin/"original-observed-slot"
            chosen.rename(retained)
            chosen.write_bytes(retained.read_bytes())
        with patch.object(member_copy.private_stage._Output, "sync", replace_after_copy), \
             patch.object(member_copy, "rename_new_at", side_effect=AssertionError("never publish rebound source")):
            with self.assertRaisesRegex(Rejected, "原历史归档已替换"): plan.run(confirmed=True)
        self.assertFalse(plan.path.exists())
        self.assertTrue((self.private/plan.stage_name).is_dir())

    def test_original_ancestor_cannot_be_replaced_or_omitted_and_never_creates_stage(self):
        plan, original, selected, choice, _, (first, _, tip, _) = self.history_plan()
        plan.review()
        leaf = original/first.filename
        old = original/"removed"
        leaf.rename(old)
        leaf.write_bytes(old.read_bytes())
        with patch.object(member_copy.private_stage, "new_child", side_effect=AssertionError("must check original slots first")):
            with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertFalse(plan.path.exists())
        self.assertEqual(list(self.private.iterdir()), [])
        leaf.rename(original/"replacement")
        missing = selected.catalog(choice)
        self.assertEqual(missing.history(tip.operation)[1], first.operation)
        with self.assertRaises(Rejected):
            member_copy.HistoryCopy(selected, choice, missing, str(self.dest/"must-not-start"))

    def subtree_plan(self, name="tree", *, output="tree-copy", scratch=None):
        entry = self.selected.browser.entry("", name)
        plan = member_copy.DirectoryCopy(self.selected, "", entry, str(self.dest/output))
        self.addCleanup(plan.close)
        plan.pick_private(str(self.private if scratch is None else scratch))
        return plan

    def test_one_native_rename_after_complete_subtree_and_empty_roots(self):
        plan = self.subtree_plan()
        detail = plan.review()
        stage = self.private / plan.stage_name
        self.assertIn(repr(str(stage)), detail)
        self.assertEqual(list(self.private.iterdir()), [])
        self.assertEqual(list(self.dest.iterdir()), [])
        with self.assertRaises(Rejected): plan.run()
        real_move, real_read = member_copy.rename_new_at, os.pread
        seen = []
        def move(src_fd, src, dst, *, expected_source, target_fd):
            self.assertEqual((src, dst), (plan.stage_name, plan.path.name))
            self.assertTrue(os.path.samestat(os.fstat(src_fd), self.private.stat()))
            self.assertTrue(os.path.samestat(os.fstat(target_fd), self.dest.stat()))
            self.assertTrue(os.path.samestat(expected_source, stage.stat()))
            self.assertFalse(plan.path.exists())
            self.assertEqual((stage / "nested/value").read_bytes(), b"only this subtree")
            self.assertEqual(list((stage / "empty").iterdir()), [])
            self.assertEqual((stage / "zero").read_bytes(), b"")
            real_move(src_fd, src, dst, expected_source=expected_source, target_fd=target_fd)
            self.assertTrue(os.path.samestat(expected_source, plan.path.stat()))
        def read(fd, size, offset):
            if fd == plan.source: seen.append((offset, offset+size))
            return real_read(fd, size, offset)
        display = self.selected.held.fileno()
        os.lseek(display, 9, os.SEEK_SET)
        with patch.object(member_copy, "rename_new_at", side_effect=move) as moved, \
             patch.object(member_copy.os, "pread", side_effect=read):
            self.assertIn("已另存并核验目录", plan.run(confirmed=True))
            self.assertEqual(moved.call_count, 1)
        self.assertFalse(any(a <= len(self.data) + CHUNK < b for a, b in seen))
        self.assertEqual(os.lseek(display, 0, os.SEEK_CUR), 9)
        self.assertFalse(stage.exists())
        self.assertEqual(sorted(x.name for x in plan.path.iterdir()), ["empty", "nested", "zero"])
        self.assertEqual(self.archive.read_bytes(), self.original)

        empty = self.subtree_plan("empty-root", output="empty-copy")
        empty.review()
        empty.run(confirmed=True)
        self.assertEqual(list(empty.path.iterdir()), [])
        self.assertEqual(stat.S_IMODE(empty.path.stat().st_mode), 0o700)

    def test_directory_cancelled_during_actual_private_readback_never_renames(self):
        plan = self.subtree_plan()
        plan.review()
        stage = self.private/plan.stage_name
        original = os.read
        cancelled = []
        def read(fd, n):
            data = original(fd, n)
            if data == b"only this subtree" and stage.is_dir():
                self.assertTrue(plan.cancel())
                cancelled.append(True)
            return data
        with patch.object(member_copy.os, "read", side_effect=read), \
             patch.object(member_copy, "rename_new_at", side_effect=AssertionError("no cancelled directory is public")):
            with self.assertRaisesRegex(member_copy.CancelledCopy, "可能仍有准备材料"):
                plan.run(confirmed=True)
        self.assertEqual(cancelled, [True])
        self.assertFalse(plan.path.exists())
        self.assertEqual((stage/"nested/value").read_bytes(), b"only this subtree")
        self.assertEqual(self.archive.read_bytes(), self.original)

    def test_cancel_before_starting_an_ordinary_extraction_does_not_create_a_private_stage(self):
        original, _, plan = self.plain_plan()
        stored = original.read_bytes()
        plan.review()
        self.assertTrue(plan.cancel())
        with patch.object(member_copy.private_stage, "new_child", side_effect=AssertionError("already cancelled")):
            with self.assertRaises(member_copy.CancelledCopy): plan.run(confirmed=True)
        self.assertEqual(list(self.private.iterdir()), [])
        self.assertFalse(plan.path.exists())
        self.assertEqual(original.read_bytes(), stored)

    def test_subtree_corrupt_store_body_can_leave_private_stage_but_not_a_final_name(self):
        plan = self.subtree_plan()
        plan.review()
        with zipfile.ZipFile(self.archive) as structure:
            offset = structure.getinfo("tree/nested/value").header_offset
        from zipworkspace.archive import LOCAL
        with self.archive.open("rb") as raw:
            raw.seek(offset)
            fields = LOCAL.unpack(raw.read(LOCAL.size))
        body = offset + LOCAL.size + fields[-2] + fields[-1]
        real_read = os.pread
        def read(fd, size, offset):
            data = real_read(fd, size, offset)
            if data and fd == plan.source and offset == body:
                return b"\x00" + data[1:]
            return data
        with patch.object(member_copy.os, "pread", side_effect=read), \
             patch.object(member_copy, "rename_new_at", side_effect=AssertionError("never publish a bad CRC")):
            with self.assertRaisesRegex(Rejected, "Bad CRC") as refused:
                plan.run(confirmed=True)
        self.assertIn(plan.stage_name, str(refused.exception))
        self.assertTrue((self.private / plan.stage_name / "empty").is_dir())
        self.assertFalse(plan.path.exists())

    def test_rebound_private_or_nested_final_parent_is_not_accepted(self):
        nested = self.dest / "bad-scratch"
        nested.mkdir(mode=0o700)
        with self.assertRaises(Rejected): self.subtree_plan(scratch=nested).review()
        plan = self.subtree_plan()
        plan.review()
        original = self.private.with_name("initial-scratch")
        self.private.rename(original)
        self.private.mkdir(mode=0o700)
        with patch.object(member_copy, "rename_new_at", side_effect=AssertionError("do not adopt a replacement")):
            with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertEqual(list(original.iterdir()), [])
        self.assertEqual(list(self.private.iterdir()), [])
        self.assertFalse(plan.path.exists())

    def test_late_collision_with_an_existing_empty_directory_is_no_replace(self):
        plan = self.subtree_plan()
        plan.review()
        real = member_copy.rename_new_at
        protected = []
        def compete(*args, **kwargs):
            plan.path.mkdir(mode=0o700)
            protected.append(plan.path.stat().st_ino)
            return real(*args, **kwargs)
        with patch.object(member_copy, "rename_new_at", side_effect=compete):
            with self.assertRaisesRegex(Rejected, "没有覆盖"): plan.run(confirmed=True)
        self.assertEqual(plan.path.stat().st_ino, protected[0])
        self.assertEqual(list(plan.path.iterdir()), [])
        self.assertEqual((self.private / plan.stage_name / "nested/value").read_bytes(), b"only this subtree")

    def test_final_name_on_another_mount_is_not_acknowledged_even_with_equal_inode(self):
        plan = self.subtree_plan()
        plan.review()
        real_open, real_id = os.open, member_copy.mount_id
        latest = None
        def opened(name, flags, *args, **kwargs):
            nonlocal latest
            fd = real_open(name, flags, *args, **kwargs)
            if name == plan.path.name and flags & os.O_PATH:
                latest = fd
            return fd
        def mount(fd, *, read_proc=None):
            if fd == latest:
                self.assertTrue(os.path.samestat(os.fstat(fd), plan.path.stat()))
                return b"not-the-original-mount"
            return real_id(fd, read_proc=read_proc)
        with patch.object(member_copy.os, "open", side_effect=opened), \
             patch.object(member_copy, "mount_id", side_effect=mount):
            with self.assertRaisesRegex(Uncertain, "不在选定的实际挂载"):
                plan.run(confirmed=True)
        self.assertEqual((plan.path / "nested/value").read_bytes(), b"only this subtree")

    def test_failed_source_or_final_parent_sync_after_move_never_reports_success(self):
        for name, fail in (("source", self.private), ("target", self.dest)):
            with self.subTest(name=name):
                plan = self.subtree_plan(output=name)
                plan.review()
                actual = os.fsync
                calls = []
                def sync(fd):
                    if any(os.path.samestat(os.fstat(fd), p.stat()) for p in (self.private, self.dest)):
                        self.assertTrue(plan.path.is_dir())
                        calls.append(fd)
                    if os.path.samestat(os.fstat(fd), fail.stat()): raise OSError(errno.EIO, name)
                    actual(fd)
                with patch.object(os, "fsync", side_effect=sync):
                    with self.assertRaisesRegex(Uncertain, "完整副本可能已经出现"): plan.run(confirmed=True)
                self.assertEqual(len(calls), 2 if name == "source" else 1)
                self.assertEqual((plan.path / "nested/value").read_bytes(), b"only this subtree")
                self.assertFalse((self.private / plan.stage_name).exists())

    def test_non_proc_root_fails_before_any_proc_paths_are_interpreted(self):
        spoof = self.dest / "not-proc"
        spoof.mkdir(mode=0o700)
        (spoof / "self").symlink_to(str(os.getpid()))
        plan = self.plan()
        real_open = os.open
        def opened(path, *args, **kwargs):
            return real_open(spoof if path == "/proc" else path, *args, **kwargs)
        with patch.object(member_copy.os, "open", side_effect=opened), \
             patch.object(member_copy._ProcView, "read", side_effect=AssertionError("never trust presented mountinfo")):
            with self.assertRaisesRegex(Rejected, "真正的 Linux procfs"):
                plan.review()
        self.assertFalse(self.path.exists())

    def test_publish_links_only_the_complete_private_inode_without_rewinding_the_display(self):
        plan = self.plan()
        fd = self.selected.held.fileno()
        os.lseek(fd, 7, os.SEEK_SET)
        detail = plan.review()
        self.assertIn(repr("original/same.bin"), detail)
        self.assertIn(f"{len(self.data):,}", detail)
        self.assertEqual(list(self.dest.iterdir()), [])
        with self.assertRaises(Rejected):
            plan.run()  # a prepared but unconfirmed call must still make no file
        real_link, real_read = os.link, os.pread
        seen = []
        def publish(src, dst, **kwargs):
            source = int(src)
            m = os.fstat(source)
            self.assertEqual((m.st_nlink, m.st_size, stat.S_IMODE(m.st_mode)), (0, len(self.data), 0o600))
            self.assertEqual(list(self.dest.iterdir()), [])
            real_link(src, dst, **kwargs)
            self.assertEqual(self.path.stat().st_ino, m.st_ino)
        def read(fd, size, offset):
            if fd == plan.source: seen.append((offset, offset+size))
            return real_read(fd, size, offset)
        with patch.object(member_copy.os, "link", side_effect=publish), \
             patch.object(member_copy.os, "pread", side_effect=read):
            self.assertIn("已另存并核验", plan.run(confirmed=True))
        # The fixed EOCD tail may touch payload. Ensure there was no walk through
        # the middle of the unrelated 2 MiB member to find/check the selected row.
        elsewhere = len(self.data) + CHUNK
        self.assertFalse(any(start <= elsewhere < stop for start, stop in seen))
        self.assertEqual(os.lseek(fd, 0, os.SEEK_CUR), 7)
        self.assertEqual(self.path.read_bytes(), self.data)
        self.assertEqual(self.archive.read_bytes(), self.original)
        with self.assertRaises(Rejected): plan.run(confirmed=True)

    def test_an_existing_dangling_symlink_and_a_late_collision_are_not_replaced(self):
        self.path.symlink_to("missing-original")
        with self.assertRaisesRegex(Rejected, "已有"):
            self.plan().review()
        self.assertEqual(os.readlink(self.path), "missing-original")
        self.path = self.dest / "another-new-name"
        plan = self.plan()
        plan.review()
        real_link = os.link
        def occupy(*args, **kwargs):
            with self.path.open("xb") as original: original.write(b"somebody else")
            return real_link(*args, **kwargs)
        with patch.object(member_copy.os, "link", side_effect=occupy):
            with self.assertRaisesRegex(Rejected, "没有覆盖"):
                plan.run(confirmed=True)
        self.assertEqual(self.path.read_bytes(), b"somebody else")

    def test_wrong_archive_or_parent_is_not_reselected(self):
        plan = self.plan()
        plan.review()
        original = self.archive.with_name("old.zxs")
        self.archive.rename(original)
        self.archive.write_bytes(self.original)
        with patch.object(member_copy.os, "link", side_effect=AssertionError("cannot publish from a replacement archive")):
            with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertFalse(self.path.exists())
        self.selected.release_display()
        self.selected = Selection.open(self.archive, retain=True)
        self.addCleanup(self.selected.release_display)
        plan = self.plan()
        plan.review()
        prior = self.dest.with_name("former-out")
        self.dest.rename(prior)
        self.dest.mkdir(mode=0o700)
        with patch.object(member_copy.os, "link", side_effect=AssertionError("cannot publish into a rebound parent")):
            with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertEqual(list(self.dest.iterdir()), [])
        self.assertEqual(list(prior.iterdir()), [])
        self.assertEqual(original.read_bytes(), self.original)

    def test_bad_crc_in_original_stream_never_links(self):
        plan = self.plan()
        plan.review()
        from zipworkspace.archive import LOCAL
        with self.archive.open("rb") as raw:
            fixed = LOCAL.unpack(raw.read(LOCAL.size))
        first_body = LOCAL.size + fixed[-2] + fixed[-1]
        real_read = os.pread
        def read(fd, size, offset):
            chunk = real_read(fd, size, offset)
            if chunk and fd == plan.source and offset == first_body:
                return bytes([chunk[0] ^ 1]) + chunk[1:]
            return chunk
        with patch.object(member_copy.os, "pread", side_effect=read), \
             patch.object(member_copy.os, "link", side_effect=AssertionError("must wait for original CRC")):
            with self.assertRaises(zipfile.BadZipFile):
                plan.run(confirmed=True)
        self.assertEqual(list(self.dest.iterdir()), [])

    def test_anonymous_readback_must_match_and_parent_sync_failure_is_indeterminate(self):
        plan = self.plan()
        plan.review()
        real_read = os.pread
        def read(fd, size, offset):
            chunk = real_read(fd, size, offset)
            if fd != plan.source and offset == 0:
                return bytes([chunk[0] ^ 1]) + chunk[1:]
            return chunk
        with patch.object(member_copy.os, "pread", side_effect=read), \
             patch.object(member_copy.os, "link", side_effect=AssertionError("must wait for real readback")):
            with self.assertRaisesRegex(Rejected, "匿名副本读回"):
                plan.run(confirmed=True)
        self.assertEqual(list(self.dest.iterdir()), [])

        plan = self.plan("zero")
        plan.review()
        real_sync = os.fsync
        def sync(fd):
            if stat.S_ISDIR(os.fstat(fd).st_mode):
                self.assertTrue(self.path.is_file())
                raise OSError(errno.EIO, "simulated late parent sync failure")
            real_sync(fd)
        with patch.object(member_copy.os, "fsync", side_effect=sync):
            with self.assertRaisesRegex(Uncertain, "完整副本可能已经出现"):
                plan.run(confirmed=True)
        self.assertEqual(self.path.read_bytes(), b"")
        self.assertEqual(self.archive.read_bytes(), self.original)

    @unittest.skipUnless(find_spec("PySide6"), "optional native Qt controls")
    def test_real_table_and_worker_do_not_publish_after_cancel_or_change_of_page(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import archive_pane
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        window = Window()
        self.addCleanup(window.pool.shutdown)
        pane = window.archive
        pane.show_selection(self.selected)

        def start(confirm):
            pane.show_folder("original")
            pane.files.selectRow(0)
            queued = Future()
            with patch.object(archive_pane.QFileDialog, "getSaveFileName", return_value=(str(self.path), "")) as pick, \
                 patch.object(window.pool, "submit", return_value=queued) as submit, \
                 patch.object(archive_pane, "confirm", side_effect=confirm) as consent:
                self.assertTrue(pane.save_member_action.isEnabled())
                pane.save_member_action.click()
                self.assertTrue(pick.call_args.kwargs["options"] & archive_pane.QFileDialog.Option.ReadOnly)
                self.assertIs(window.pending[0], queued)
                queued.set_result(submit.call_args.args[0]())
                window.poll()
                self.assertEqual(consent.call_count, 1)
                self.assertEqual(submit.call_count, 1)
                self.assertIsNone(window.pending)
                self.assertFalse(self.path.exists())

        start(lambda *_a, **_k: False)
        def change(*_a, **_k):
            pane.show_folder("other")
            pane.files.selectRow(0)  # Same basename from a different folder.
            return True
        start(change)

        pane.show_folder("")
        row = next(i for i, (_, item) in enumerate(pane.files.rows.rows) if item.name == "tree")
        pane.files.selectRow(row)
        self.assertIn("文件夹", pane.save_member_action.text())
        queued = Future()
        with patch.object(archive_pane.QFileDialog, "getSaveFileName", return_value=(str(self.path), "")), \
             patch.object(archive_pane.QFileDialog, "getExistingDirectory", return_value=str(self.private)), \
             patch.object(archive_pane, "confirm", return_value=False) as consent, \
             patch.object(window.pool, "submit", return_value=queued) as submit:
            pane.save_member_action.click()
            queued.set_result(submit.call_args.args[0]())
            window.poll()
            self.assertIn("暂存目录由本人独占", consent.call_args.kwargs["attestation"])
            self.assertEqual(submit.call_count, 1)
            self.assertFalse(self.path.exists())
            self.assertEqual(list(self.private.iterdir()), [])
