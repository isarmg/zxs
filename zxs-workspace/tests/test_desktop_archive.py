from concurrent.futures import Future
from contextlib import ExitStack
from dataclasses import replace
import errno
import hashlib
from importlib.util import find_spec
import os
from pathlib import Path
import tempfile
import sys
from types import SimpleNamespace
from unittest.mock import patch
import unittest
import zipfile

from zipworkspace.desktop.selection import (Selection, StoreSelection, Verification, duplicate_selected_view,
                                            open_store_preview, selected_raw_sha)
from zipworkspace.model import CHUNK, Commit, Manifest, Rejected


def write_snapshot(path, *, marker=None, year=2000, entries=None):
    rows = Manifest()
    if entries is None: entries = {"design/notes": b"desktop selection", "empty/": b""}
    for name, data in entries.items():
        directory = name.endswith("/")
        rows.add(name.removesuffix("/"), directory, len(data), None if directory else hashlib.sha256(data).hexdigest())
    marker = marker or Commit("1"*32, "2"*32, 1, "3"*32, None, rows.digest())
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as out:
        for name, data in entries.items():
            entry = zipfile.ZipInfo(name, (year, 1, 1, 0, 0, 0))
            entry.external_attr = ((0o40755 if name.endswith("/") else 0o100644) << 16)
            out.writestr(entry, data)
        out.comment = marker.encode()
    return marker


class SelectedOriginal(unittest.TestCase):
    def test_explicit_store_names_do_not_open_archive_bodies_or_grant_history(self):
        from zipworkspace.desktop import selection as view
        with tempfile.TemporaryDirectory() as tmp, ExitStack() as held:
            root = Path(tmp); loose = root/"loose.zxs"
            marker = write_snapshot(loose)
            slot = root/marker.filename; loose.rename(slot)
            (root/"copy.zxs").write_bytes(slot.read_bytes())
            another = replace(marker, repo="a"*32)
            bad = root/another.filename; bad.write_bytes(slot.read_bytes())
            store = StoreSelection.open(root)
            with patch.object(view, "ArchiveBrowser", side_effect=AssertionError("inventory must not read ZIP")), \
                 patch.object(view, "named_document", side_effect=AssertionError("inventory is not a file open")):
                candidates = {row.repo:row for row in store.previews(observer=Verification())}
            self.assertEqual(set(candidates), {marker.repo, another.repo})
            with patch.object(view, "archive_sha", side_effect=AssertionError("not a history or raw request")):
                chosen = open_store_preview(store, candidates[marker.repo], observer=Verification())
            held.callback(chosen.release_display)
            self.assertEqual(chosen.browser.commit, marker)
            self.assertIsNone(chosen.exact)
            self.assertTrue(os.path.samestat(os.fstat(chosen.held.fileno()), slot.stat()))
            with self.assertRaisesRegex(Rejected, "规范槽名"):
                open_store_preview(store, candidates[another.repo], observer=Verification())

    def test_store_entries_reject_rebound_bytes_first_and_never_complete_a_changed_listing(self):
        from zipworkspace import refs
        from zipworkspace.desktop import selection as view
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); path = root/"loose.zxs"
            marker = write_snapshot(path)
            slot = root/marker.filename; path.rename(slot)
            store = StoreSelection.open(root)
            row, = store.previews(observer=Verification())
            retained = root/"old-slot"; slot.rename(retained); slot.write_bytes(retained.read_bytes())
            with patch.object(view, "ArchiveBrowser", side_effect=AssertionError("never read a replacement")):
                with self.assertRaisesRegex(Rejected, "未读取替件正文"):
                    open_store_preview(store, row, observer=Verification())

            actual = view.slot_previews
            added = root/replace(marker, branch="b"*32).filename
            def add_after_first(*args, **kw):
                current = actual(*args, **kw)
                if not added.exists(): added.write_bytes(slot.read_bytes())
                return current
            with patch.object(view, "slot_previews", side_effect=add_after_first):
                with self.assertRaisesRegex(Rejected, "不显示部分列表"):
                    store.previews(observer=Verification())
            with patch.object(refs, "STORE_BUDGET", 1):
                with self.assertRaisesRegex(Rejected, "不显示部分列表"):
                    store.previews(observer=Verification())

    def test_loose_document_can_be_read_but_is_not_an_unpublished_branch_head(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            store = root / "store"; store.mkdir()
            loose = root / "design.zxs"
            first = write_snapshot(loose)
            selection = Selection.open(loose)
            self.assertEqual(selection.browser.list_directory("empty").entries, ())
            chosen_store = StoreSelection.open(store)
            catalog = selection.catalog(chosen_store)
            self.assertEqual(catalog.branches(), [])
            self.assertIsNone(selection.exact)
            self.assertEqual(catalog.select_node(first.operation), catalog.anchor)
            self.assertEqual(catalog.anchor.marker, selection.browser.commit)
            later = replace(first, generation=2, operation="4"*32, parent=first.operation)
            write_snapshot(store / later.filename, marker=later)
            partial = selection.catalog(chosen_store)
            self.assertEqual(partial.branches()[0]["missing_parent"], first.operation)
            with self.assertRaises(Rejected): partial.select_branch(first.branch)
            (store / first.filename).write_bytes(loose.read_bytes())
            complete = selection.catalog(chosen_store)
            head = complete.select_branch(first.branch)
            self.assertEqual(Selection.open(head.path, expected=head).browser.commit, later)

    def test_first_page_and_navigation_never_compute_the_raw_zip_digest(self):
        from zipworkspace import archive, refs
        from zipworkspace.desktop import selection as view
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); path = root/"chosen.zxs"
            marker = write_snapshot(path)
            with patch.object(view,"archive_sha",side_effect=AssertionError("no full raw pass on initial browse")), \
                    patch.object(refs,"archive_sha",side_effect=AssertionError("catalog cannot be implicit")), \
                    patch.object(archive,"_read_checked",wraps=archive._read_checked) as verify:
                selected = Selection.open(path)
                self.assertEqual(verify.call_count,1)
                self.assertIsNone(selected.exact)
                self.assertEqual(selected.browser.list_directory("design").entries[0].sha256,
                                 hashlib.sha256(b"desktop selection").hexdigest())
                self.assertEqual(selected.browser.list_directory("empty").entries,())
                self.assertEqual(verify.call_count,1)
            with patch.object(refs,"archive_sha",wraps=refs.archive_sha) as whole:
                node = selected.catalog(StoreSelection.open(root)).anchor
                self.assertEqual(whole.call_count,1)
            self.assertEqual(node.marker,marker)
            with patch.object(view,"archive_sha",wraps=view.archive_sha) as exact:
                self.assertEqual(Selection.open(path,expected=node).exact,node)
                self.assertEqual(exact.call_count,1)
            with self.assertRaises(Rejected):
                Selection.open(path, expected=replace(node, sha256="0" * 64))

    def test_canonical_anchor_is_verified_once_and_held_through_both_inventories(self):
        from zipworkspace import refs
        with tempfile.TemporaryDirectory() as tmp:
            store = Path(tmp)
            source = store / "setup.zxs"; marker = write_snapshot(source)
            slot = store / marker.filename; source.rename(slot)
            later = replace(marker, generation=2, operation="4"*32, parent=marker.operation)
            peer = store / later.filename; write_snapshot(peer, marker=later)
            selected = Selection.open(slot)
            chosen = StoreSelection.open(store)
            read, digest, members = refs.read_managed_document, refs.archive_sha, refs._members
            for _ in range(2):  # A new catalog always re-verifies both original files.
                documents, hashes, inventories = {}, [], []
                def check(doc, *, display_name):
                    self.assertNotIn(display_name, documents)
                    documents[display_name] = doc
                    return read(doc, display_name=display_name)
                def raw(doc):
                    hashes.append(doc)
                    return digest(doc)
                def inventory(*args):
                    inventories.append(not documents[marker.filename].closed)
                    return members(*args)
                with patch.object(refs, "read_managed_document", side_effect=check), \
                     patch.object(refs, "archive_sha", side_effect=raw), \
                     patch.object(refs, "_members", side_effect=inventory):
                    cat = selected.catalog(chosen)
                self.assertEqual(set(documents), {marker.filename, later.filename})
                self.assertCountEqual(hashes, documents.values())
                self.assertEqual(inventories, [True, True])
                self.assertTrue(all(doc.closed for doc in documents.values()))
                self.assertEqual(cat.select_node(marker.operation), cat.anchor)
                self.assertEqual(cat.select_branch(marker.branch).marker, later)

    def test_requested_raw_hash_is_from_the_held_zip_without_reparse_or_shared_seek(self):
        from zipworkspace.desktop import selection as view
        with tempfile.TemporaryDirectory() as tmp, ExitStack() as held:
            path = Path(tmp) / "selected.zxs"
            marker = write_snapshot(path, entries={"first/body": b"A" * (2 * CHUNK), "empty/": b""})
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            self.assertNotEqual(digest, marker.tree)
            selected = Selection.open(path, retain=True)
            held.callback(selected.release_display)
            fd = selected.held.fileno()
            os.lseek(fd, 15, os.SEEK_SET)

            class Watch(Verification):
                def __init__(self, cancel=False):
                    super().__init__()
                    self.copied, self.stop = 0, cancel

                def advance(self, size):
                    super().advance(size)
                    self.copied += size
                    if self.stop: self.cancel()

            with patch.object(view, "named_document", side_effect=AssertionError("no reopen")), \
                 patch.object(view, "ArchiveBrowser", side_effect=AssertionError("no ZIP reparse")):
                read = Watch()
                self.assertEqual(selected_raw_sha(selected, observer=read), digest)
                self.assertEqual(read.copied, selected.observed[2])
                cancelled = Watch(cancel=True)
                with self.assertRaisesRegex(Rejected, "已取消"):
                    selected_raw_sha(selected, observer=cancelled)
                self.assertGreater(cancelled.copied, 0)
                self.assertLessEqual(cancelled.copied, CHUNK)
            self.assertEqual(os.lseek(fd, 0, os.SEEK_CUR), 15)
            self.assertIsNone(selected.exact)  # The read doesn't mint a history or writer capability.

    @unittest.skipIf(os.name == "nt", "POSIX synthetic same-name replacement with the original held")
    def test_requested_raw_hash_rejects_same_byte_replacement(self):
        with tempfile.TemporaryDirectory() as tmp, ExitStack() as held:
            path = Path(tmp) / "chosen.zxs"
            write_snapshot(path)
            selected = Selection.open(path, retain=True)
            held.callback(selected.release_display)
            moved = path.with_suffix(".original")
            path.rename(moved)
            path.write_bytes(moved.read_bytes())
            with self.assertRaisesRegex(Rejected, "原归档已变化"):
                selected_raw_sha(selected, observer=Verification())

    def test_requested_raw_hash_checks_a_listed_digest_when_present(self):
        with tempfile.TemporaryDirectory() as tmp, ExitStack() as held:
            path = Path(tmp) / "chosen.zxs"
            write_snapshot(path)
            view = Selection.open(path)
            node = view.catalog(StoreSelection.open(path.parent)).anchor
            selected = Selection.open(path, expected=node, retain=True)
            held.callback(selected.release_display)
            self.assertEqual(selected_raw_sha(selected, observer=Verification()), node.sha256)
            with self.assertRaisesRegex(Rejected, "已列原版本不符"):
                selected_raw_sha(replace(selected, exact=replace(node, sha256="0" * 64)), observer=Verification())

    def test_independent_display_shares_the_original_identity_and_windows_seek_guard(self):
        from zipworkspace.desktop import selection as view
        with tempfile.TemporaryDirectory() as tmp, ExitStack() as held:
            path = Path(tmp) / "chosen.zxs"
            write_snapshot(path)
            original = Selection.open(path, retain=True)
            held.callback(original.release_display)
            with patch.object(view, "named_document", side_effect=AssertionError("do not reopen")), \
                 patch.object(view, "ArchiveBrowser", side_effect=AssertionError("do not reparse the archive")), \
                 patch.object(view, "archive_sha", side_effect=AssertionError("do not reread the ZIP")):
                copied = duplicate_selected_view(original, observer=Verification())
            held.callback(copied.release_display)
            self.assertIsNot(original, copied)
            self.assertIsNot(original.held, copied.held)
            self.assertIs(original.held._seek_lock, copied.held._seek_lock)
            self.assertIs(original.browser, copied.browser)
            self.assertEqual(original.observed, copied.observed)
            original.release_display()
            self.assertFalse(copied.held.closed)
            self.assertEqual(selected_raw_sha(copied, observer=Verification()), hashlib.sha256(path.read_bytes()).hexdigest())

    @unittest.skipIf(os.name == "nt", "POSIX replacement under a still-held FD")
    def test_independent_view_does_not_reopen_a_same_byte_replacement(self):
        with tempfile.TemporaryDirectory() as tmp, ExitStack() as held:
            path = Path(tmp)/"chosen.zxs"; write_snapshot(path)
            original = Selection.open(path, retain=True); held.callback(original.release_display)
            old = path.with_suffix(".kept"); path.rename(old); path.write_bytes(old.read_bytes())
            with self.assertRaisesRegex(Rejected, "原归档已变化"):
                duplicate_selected_view(original, observer=Verification())
            self.assertFalse(original.held.closed)

    def test_same_inode_at_a_different_name_is_not_reused_as_the_store_slot(self):
        from zipworkspace import refs
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); store = root / "store"; store.mkdir()
            loose = root / "loose.zxs"; marker = write_snapshot(loose)
            slot = store / marker.filename
            try: os.link(loose, slot)
            except (OSError, NotImplementedError) as e: self.skipTest(f"hard links unavailable: {e}")
            with patch.object(refs, "archive_sha", wraps=refs.archive_sha) as raw:
                cat = refs.Catalog(loose, store)
            self.assertEqual(raw.call_count, 2)
            self.assertEqual(cat.anchor.path, loose)
            self.assertEqual(cat.select_node(marker.operation).path, slot)
            self.assertEqual(cat.select_branch(marker.branch).marker, marker)

    def test_corrupt_member_outside_root_view_is_rejected_without_a_raw_zip_pass(self):
        from zipworkspace.desktop import selection as view
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)/"corrupt.zxs"
            write_snapshot(path)
            data = path.read_bytes()
            self.assertEqual(data.count(b"desktop selection"),1)
            path.write_bytes(data.replace(b"desktop selection",b"desktop mutation!"))
            with patch.object(view,"archive_sha",side_effect=AssertionError("no raw-digest rescue")):
                with self.assertRaises((Rejected,zipfile.BadZipFile)): Selection.open(path)

    def test_selected_store_and_physical_slot_cannot_be_silently_substituted(self):
        from zipworkspace import refs
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            loose = root / "chosen.zxs"
            marker = write_snapshot(loose)
            store = root / "store"; store.mkdir()
            chosen = StoreSelection.open(store)
            member = store / marker.filename
            member.write_bytes(loose.read_bytes())
            selection = Selection.open(loose)
            self.assertEqual(selection.catalog(chosen).select_branch(marker.branch).marker, marker)
            # Replace only the store slot after the real first inventory; the
            # loose anchor stays valid but cannot authorize the equal-byte replacement.
            members = refs._members
            original = root / "old-slot"
            def replace_after_inventory(*args):
                before = members(*args)
                if not original.exists():
                    member.rename(original)
                    member.write_bytes(original.read_bytes())
                return before
            with patch.object(refs, "_members", side_effect=replace_after_inventory):
                with self.assertRaises(Rejected): selection.catalog(chosen)
            store.rename(root / "old-store")
            store.mkdir()
            (store / marker.filename).write_bytes(loose.read_bytes())
            with self.assertRaises(Rejected): selection.catalog(chosen)
            # A deliberate new picker selection is distinct and can be admitted.
            self.assertEqual(selection.catalog(StoreSelection.open(store)).anchor.marker, marker)

    def test_previously_listed_row_refuses_a_byte_identical_replacement_at_the_same_slot(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); store = root / "store"; store.mkdir()
            loose = root / "loose.zxs"
            marker = write_snapshot(loose)
            slot = store / marker.filename
            slot.write_bytes(loose.read_bytes())
            chosen = StoreSelection.open(store)
            view = Selection.open(loose)
            listed = view.catalog(chosen).select_branch(marker.branch)
            self.assertEqual(Selection.open(slot, expected=listed).exact, listed)
            retained = root / "original-slot"
            slot.rename(retained)  # Keep the predecessor alive: its inode cannot be reused.
            slot.write_bytes(retained.read_bytes())
            # Same storage name, commit and raw ZIP SHA, but not the listed file.
            fresh = view.catalog(chosen).select_branch(marker.branch)
            self.assertEqual((fresh.path, fresh.marker, fresh.sha256), (listed.path, listed.marker, listed.sha256))
            self.assertNotEqual(fresh.observed[:2], listed.observed[:2])
            with self.assertRaises(Rejected): Selection.open(slot, expected=listed)
            self.assertIsNone(Selection.open(slot).exact)
            self.assertEqual(Selection.open(slot, expected=fresh).exact, fresh)

    def test_same_logical_tree_reencoded_is_not_the_selected_original_for_a_write(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "original.zxs"
            marker = write_snapshot(path)
            original = Selection.open(path)
            store = StoreSelection.open(root)
            exact = original.catalog(store).anchor  # The stronger decision binds the raw ZIP bytes.
            replacement = root / "new.zxs"
            write_snapshot(replacement, marker=marker, year=2001)
            replacement.replace(path)
            current = Selection.open(path)
            self.assertEqual(current.browser.commit, marker)
            self.assertIsNone(current.exact)
            self.assertNotEqual(current.catalog(store).anchor.sha256, exact.sha256)
            with self.assertRaises(Rejected): Selection.open(path, expected=exact)
            with self.assertRaises(Rejected): original.catalog(store)


@unittest.skipUnless(find_spec("PySide6"), "optional desktop extra not installed")
class DesktopArchiveWidgets(unittest.TestCase):
    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1", "optional Qt source fixture")
    def test_workspace_can_open_an_exact_store_row_from_a_blank_window(self):
        from PySide6.QtWidgets import QApplication, QPushButton
        from zipworkspace.desktop.widgets import Table
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); loose = root/"choose-me.zxs"
            marker = write_snapshot(loose); loose.rename(root/marker.filename)
            win = Window(); pane = win.archive
            jobs = []
            def queue(job):
                ready = Future(); jobs.append((ready, job)); return ready
            def complete():
                ready, job = jobs.pop(0); ready.set_result(job()); win.poll()
            try:
                pane.use_store(StoreSelection.open(root))
                with patch.object(win.pool, "submit", side_effect=queue):
                    pane.show_slots.click(); complete()
                    listing = pane.slots_dialog
                    self.assertIsNone(pane.selection)
                    self.assertIsNone(pane.catalog)
                    listing.findChild(Table).selectRow(0)
                    next(b for b in listing.findChildren(QPushButton) if b.text() == "核验并打开所选归档").click()
                    self.assertIsNone(pane.slots_dialog)
                    complete()
                self.assertEqual(pane.selection.browser.commit, marker)
                self.assertIsNone(pane.catalog)
                self.assertIsNone(pane.checkout.live)
            finally:
                if win.pending is not None:
                    win.pending[0].cancel(); win.poll()
                win.close()

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1", "optional Qt source fixture")
    def test_actual_window_copies_only_current_read_only_view_and_drops_late_clone(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); path, later = root/"chosen.zxs", root/"later.zxs"
            write_snapshot(path); write_snapshot(later, year=2001)
            delivered = []
            def new():
                child = Window()
                delivered.append(child)
                return child
            win = Window(new_window=new)
            pane = win.archive
            source = Selection.open(path, retain=True)
            pane.show_selection(source)
            store = StoreSelection.open(root); pane.use_store(store); pane.show_catalog(source.catalog(store))
            def begin():
                future = Future()
                with patch.object(win.pool, "submit", return_value=future) as run:
                    pane.duplicate_action.click()
                return future, run.call_args.args[0]
            try:
                future, job = begin()
                future.set_result(job()); win.poll()
                fresh = delivered[0].archive
                self.assertIs(pane.selection, source)
                self.assertIs(pane.store, store)
                self.assertIs(fresh.selection.browser, source.browser)
                self.assertIsNone(fresh.store)
                self.assertIsNone(fresh.catalog)
                self.assertIsNone(fresh.checkout.live)
                copied = fresh.selection
                delivered[0].close()
                self.assertTrue(copied.held.closed)
                self.assertFalse(source.held.closed)
                future, job = begin()
                outcome = job()
                pane.show_selection(Selection.open(later, retain=True))
                future.set_result(outcome); win.poll()
                self.assertEqual(len(delivered), 1)
                self.assertTrue(outcome[0].held.closed)
            finally:
                if win.pending is not None:
                    win.pending[0].cancel(); win.poll()
                win.close()

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1", "optional Qt source fixture")
    def test_raw_digest_needs_confirmation_and_remains_in_the_original_view(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import archive_pane
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with tempfile.TemporaryDirectory() as tmp:
            first, second = Path(tmp) / "first.zxs", Path(tmp) / "second.zxs"
            write_snapshot(first)
            write_snapshot(second, year=2001)
            win = Window()
            pane = win.archive
            pane.show_selection(Selection.open(first, retain=True))
            try:
                with patch.object(archive_pane, "confirm", return_value=False), \
                     patch.object(win.pool, "submit", side_effect=AssertionError("confirmation missing")):
                    pane.raw_check.click()
                self.assertEqual(pane.raw_result.text(), "")

                def start_read():
                    ready = Future()
                    with patch.object(archive_pane, "confirm", return_value=True), \
                         patch.object(win.pool, "submit", return_value=ready) as run:
                        pane.raw_check.click()
                    ready.set_result(run.call_args.args[0]())

                start_read()
                pane.show_selection(Selection.open(second, retain=True))
                win.poll()
                self.assertEqual(pane.raw_result.text(), "")
                start_read()
                win.poll()
                self.assertIn(hashlib.sha256(second.read_bytes()).hexdigest(), pane.raw_result.text())
                pane.close_snapshot()
                self.assertEqual(pane.raw_result.text(), "")
            finally:
                win.close()

    def test_real_window_reuses_the_selected_file_and_shows_the_verified_folders(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); path = root / "chosen.zxs"
            marker = write_snapshot(path)
            win = Window()
            try:
                pane = win.archive
                selected = Selection.open(path)
                pane.show_selection(selected)
                pane.files.selectRow(0); pane.enter()
                self.assertEqual(pane.directory, "design")
                self.assertEqual(pane.files.rows.rows[0][0][0], "notes")
                pane.go_up()
                self.assertEqual(pane.directory, "")
                store = StoreSelection.open(root)
                pane.use_store(store)
                pane.show_catalog(selected.catalog(store))
                self.assertEqual(pane.history.rows.rows[0][1].marker, marker)
                self.assertEqual(pane.branches.rows.rows, ())
            finally:
                win.close()

    def test_name_hit_in_later_real_directory_page_does_not_open_or_export(self):
        from concurrent.futures import Future
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); path = root/"chosen.zxs"
            entries = {f"right/filler{i:03d}": b"" for i in range(211)}
            entries.update({"left/two[1]": b"other", "right/two[1]": b"chosen", "empty/": b""})
            write_snapshot(path, entries=entries)
            choice = Selection.open(path)
            path.rename(root/"preserved.zxs")
            win = Window()
            self.addCleanup(win.close)
            pane = win.archive
            pane.show_selection(choice)
            jobs = []
            def submit(job):
                future = Future()
                jobs.append((future, job))
                return future
            def drain():
                future, job = jobs.pop(0)
                self.assertTrue(future.set_running_or_notify_cancel())
                future.set_result(job())
                win.poll()
                self.assertFalse(jobs)
            with patch.object(win.pool, "submit", side_effect=submit), \
                 patch.object(Selection, "open", side_effect=AssertionError("search must not reopen the ZIP")), \
                 patch("zipworkspace.browser.read_managed_document", side_effect=AssertionError("no body reads")), \
                 patch.object(pane, "enter", side_effect=AssertionError("locating is not opening")):
                pane.find.click()
                dialog = pane.search_dialog
                dialog.term.setText("filler")
                dialog.find.click()
                self.assertFalse(win.tabs.isEnabled())
                self.assertTrue(dialog.close_button.isEnabled())
                drain()
                self.assertEqual(len(dialog.hits.rows.rows), 100)
                dialog.more.click()
                self.assertEqual(dialog.hits.rows.rows[0][0][0], "right/filler100")
                dialog.term.setText("two[1]")
                dialog.find.click()
                drain()
                self.assertEqual([text[0] for text, _ in dialog.hits.rows.rows], ["right/two[1]", "left/two[1]"])
                dialog.hits.selectRow(0)
                self.assertEqual(dialog.path.text(), "right/two[1]")
                dialog.locate.click()
                self.assertIsNone(pane.search_dialog)
                self.assertIs(pane.selection, choice)
                self.assertEqual((pane.directory, pane.page_number), ("right", 2))
                self.assertIs(pane.files.item(), choice.browser.entry("right", "two[1]"))
                pane.back.click()
                self.assertEqual(pane.page_number, 1)
                self.assertEqual(pane.file_page.entries[0].name, "filler100")
                pane.more.click()
                self.assertEqual(pane.page_number, 2)
                self.assertEqual(pane.file_page.entries[11].name, "two[1]")
            self.assertEqual({p.name for p in root.iterdir()}, {"preserved.zxs"})

    def test_closed_or_replaced_search_keeps_original_folder_and_drops_queued_results(self):
        from concurrent.futures import Future
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)/"chosen.zxs"
            write_snapshot(path)
            choices = [Selection.open(path), Selection.open(path)]
            self.assertEqual(choices[0].browser.commit, choices[1].browser.commit)
            win = Window()
            self.addCleanup(win.close)
            pane = win.archive
            for mode in ("close-first", "late-success", "late-error"):
                with self.subTest(mode=mode):
                    pane.show_selection(choices[0])
                    pane.show_folder("design")
                    prior = pane.file_page
                    future, submitted = Future(), []
                    def queue(work):
                        submitted.append(work)
                        return future
                    with patch.object(win.pool, "submit", side_effect=queue):
                        pane.find.click()
                        dialog = pane.search_dialog
                        dialog.term.setText("notes")
                        dialog.find.click()
                        request = dialog.request
                        if mode == "close-first":
                            dialog.close_button.click()
                            self.assertTrue(request.is_set())
                        self.assertTrue(future.set_running_or_notify_cancel())
                        if mode == "late-error":
                            with patch.object(choices[0].browser, "find_names",
                                              side_effect=Rejected("old search failure must not paint")):
                                future.set_result(submitted[0]())
                        else:
                            future.set_result(submitted[0]())
                        if mode != "close-first":
                            pane.show_selection(choices[1])  # Same path, bytes and marker, different original selection.
                            prior = pane.file_page
                            self.assertTrue(request.is_set())
                        win.inform("current view status")
                        win.poll()
                        self.assertIsNone(pane.search_dialog)
                        self.assertIs(pane.file_page, prior)
                        self.assertEqual(win.note.text(), "current view status")


@unittest.skipUnless(os.name == "posix" and sys.platform.startswith("linux"),
                     "operator-controlled private staging is Linux-only")
class OriginalDesktopCheckout(unittest.TestCase):
    """The positive path uses an admitted real temporary mount, never a fake ext4 success."""
    def setUp(self):
        from zipworkspace import private_stage
        temp = tempfile.TemporaryDirectory(); self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.private, self.parent, self.store = (self.root / n for n in ("states", "work", "archives"))
        for path in (self.private, self.parent, self.store): path.mkdir(mode=0o700)
        a = os.open(self.private, os.O_DIRECTORY); b = os.open(self.parent, os.O_DIRECTORY)
        try:
            try: private_stage.same_mount(a, b)
            except (OSError, Rejected) as exc: self.skipTest("requires a genuine admitted mount: " + str(exc))
        finally: os.close(b); os.close(a)
        self.loose = self.root / "arbitrary-name.zxs"
        self.marker = write_snapshot(self.loose)
        self.target, self.state = self.parent / "Design", self.private / "Design"

    def choice(self, snapshot=None, *, review=True, keep_session=False):
        from zipworkspace.desktop.checkout_action import CheckoutChoice
        source = Selection.open(self.loose, retain=True) if snapshot is None else snapshot
        self.addCleanup(source.release_display)
        plan = CheckoutChoice(source, keep_session=keep_session)
        self.addCleanup(plan.close)
        plan.pick_store(self.store); plan.pick_target(self.parent); plan.pick_private(self.private)
        if review:
            msg = plan.review("Design")
            for item in (self.target, self.state, self.store, self.marker.operation, plan.operation):
                self.assertIn(str(item), msg)
        return plan

    def test_capacity_observation_counts_only_this_original_copy_and_tree_on_one_held_mount(self):
        # A second physical baseline and an older retained candidate already occupy disk.
        (self.store/self.marker.filename).write_bytes(self.loose.read_bytes())
        (self.private/"previous.zxs").write_bytes(b"prior retained bytes")
        original = self.choice(review=False)
        raw = self.loose.stat().st_size; body = len(b"desktop selection")
        self.assertEqual(original.snapshot.browser.data_bytes, body)
        def low(fd):
            self.assertEqual(fd, original.base.parent)
            self.assertTrue(os.path.samestat(os.fstat(fd), self.private.stat()))
            return SimpleNamespace(f_frsize=1024, f_blocks=100, f_bfree=91, f_bavail=0)
        with patch.object(os, "fstatvfs", side_effect=low) as only, \
             patch("zipworkspace.browser.read_managed_document", side_effect=AssertionError("no ZIP rescan")), \
             patch("zipworkspace.desktop.selection.archive_sha", side_effect=AssertionError("no extra raw read")), \
             patch("zipworkspace.opening.begin", side_effect=AssertionError("review may not write")):
            detail = original.review("Design")
        only.assert_called_once_with(original.base.parent)
        self.assertIn(f"新增逻辑字节：{raw+body:,} B", detail)
        self.assertIn(f"原包副本 {raw:,} B，解包正文 {body:,} B", detail)
        self.assertIn("普通用户可用 0 B", detail); self.assertIn("按当前报告可能不足", detail)
        self.assertFalse(self.target.exists()); self.assertFalse(self.state.exists())
        self.assertEqual((self.private/"previous.zxs").read_bytes(), b"prior retained bytes")

    def test_unknown_or_inconsistent_space_does_not_become_zero_or_a_capacity_guarantee(self):
        valid = SimpleNamespace(f_frsize=4096, f_blocks=2000, f_bfree=1024, f_bavail=512)
        broken = SimpleNamespace(f_frsize=0, f_blocks=2000, f_bfree=1024, f_bavail=512)
        invalid = SimpleNamespace(f_frsize=4096, f_blocks=2000, f_bfree=1024, f_bavail=None)
        for reading in (valid, broken, invalid, OSError("statistics unavailable")):
            with self.subTest(reading=reading):
                plan = self.choice(review=False)
                with patch.object(os, "fstatvfs", side_effect=[reading]) as one:
                    detail = plan.review("Design")
                one.assert_called_once_with(plan.base.parent)
                self.assertIn("不保证配额或写入成功", detail)
                self.assertNotIn("按当前报告可能不足", detail)
                if reading is valid:
                    self.assertIn("普通用户可用 2,097,152 B", detail)
                else:
                    self.assertIn("无法估算", detail); self.assertNotIn("普通用户可用 0 B", detail)
                self.assertFalse(self.target.exists()); self.assertFalse(self.state.exists())

    def test_replaced_private_parent_during_volume_observation_never_yields_a_confirmable_plan(self):
        plan = self.choice(review=False)
        real = os.fstatvfs
        def switch(fd):
            result = real(fd)
            self.private.rename(self.root/"original-private"); self.private.mkdir(mode=0o700)
            return result
        with patch.object(os, "fstatvfs", side_effect=switch), self.assertRaises(Rejected):
            plan.review("Design")
        self.assertFalse(self.target.exists()); self.assertFalse(any(self.private.iterdir()))
        self.assertFalse(any((self.root/"original-private").iterdir()))

    def test_loose_selected_original_creates_an_actual_cli_checkout_without_reopening_source(self):
        from zipworkspace import opening, worktree
        display = Selection.open(self.loose, retain=True)
        fd = display.held.fileno()
        os.lseek(fd, 13, os.SEEK_SET)
        original_open, original_read, ranges = os.open, os.pread, []
        data_length = self.loose.stat().st_size

        def guarded(path, flags, *args, **kwargs):
            if os.fspath(path) == str(self.loose):
                self.assertTrue(flags & os.O_PATH)  # Name/actual mount evidence, no second body open.
            return original_open(path, flags, *args, **kwargs)

        def short_read(handle, size, offset):
            self.assertTrue(os.path.samestat(os.fstat(handle), os.fstat(fd)))
            self.assertEqual(os.lseek(fd, 0, os.SEEK_CUR), 13)
            data = original_read(handle, min(17, size), offset)
            ranges.append((offset, offset + len(data), size))
            return data

        with patch.object(os, "open", new=guarded), \
             patch.object(os, "supports_dir_fd", os.supports_dir_fd | {guarded}), \
             patch.object(os, "pread", side_effect=short_read), \
             patch.object(opening, "named_document", side_effect=AssertionError("must borrow original file")):
            plan = self.choice(display)
            self.assertFalse(self.target.exists()); self.assertFalse(any(self.private.iterdir()))
            self.assertEqual(plan.run(), (self.target, self.marker))
        self.assertEqual(os.lseek(fd, 0, os.SEEK_CUR), 13)
        self.assertEqual(ranges[0][0], 0)
        self.assertEqual(ranges[-1], (data_length, data_length, 1))
        self.assertEqual([r[1] for r in ranges[:-1]], [r[0] for r in ranges[1:]])
        self.assertEqual((self.target / "design/notes").read_bytes(), b"desktop selection")
        self.assertTrue((self.target / "empty").is_dir())
        self.assertEqual((self.state / "original.zxs").read_bytes(), self.loose.read_bytes())
        self.assertEqual(worktree.status(self.state)["base_operation"], self.marker.operation)
        self.assertEqual(opening.status(self.state)["operation"], plan.operation)
        self.assertFalse(any(self.store.iterdir()))
        with self.assertRaises(Rejected): plan.run()


    def test_prepared_original_read_rejects_disconnect_or_early_eof_without_publication(self):
        from zipworkspace import opening
        for broken in (b"", OSError(errno.EIO, "read connection lost")):
            with self.subTest(broken=broken):
                plan = self.choice()
                positioned = os.pread
                def read(fd, size, offset):
                    self.assertEqual(fd, plan.source_fd)
                    if offset == 0: return positioned(fd, min(17, size), 0)
                    if isinstance(broken, Exception): raise broken
                    return broken
                with patch.object(opening, "named_document", side_effect=AssertionError("never retry a new source")), \
                     patch.object(os, "pread", side_effect=read):
                    with self.assertRaisesRegex(Rejected, "尚未安装原登记"): plan.run()
                self.assertFalse(self.target.exists())
                self.assertFalse(self.state.exists())
                self.assertEqual(list(self.store.iterdir()), [])

    def test_missing_or_closed_display_fd_and_different_source_mount_refuse_before_prepare(self):
        from zipworkspace import private_stage
        from zipworkspace.desktop.checkout_action import CheckoutChoice
        detached = Selection.open(self.loose)
        closed = Selection.open(self.loose, retain=True)
        closed.release_display()
        for choice in (detached, closed):
            with self.subTest(choice=choice), self.assertRaisesRegex(Rejected, "首次核验的原归档句柄"):
                CheckoutChoice(choice)
        plan = self.choice()
        seen = self.loose.stat()
        native = private_stage.mount_id
        def another_mount(fd):
            if fd != plan.source_fd and os.path.samestat(os.fstat(fd), seen):
                return b"another-mount-same-inode"
            return native(fd)
        with patch.object(private_stage, "mount_id", side_effect=another_mount):
            with self.assertRaisesRegex(Rejected, "原挂载"): plan.run()
        self.assertFalse(self.state.exists())
        self.assertEqual(list(self.private.iterdir()), [])

    def test_listed_slot_requires_exact_raw_bytes_and_checks_the_real_end_of_input(self):
        original = self.store / self.marker.filename
        self.loose.rename(original)
        current = Selection.open(original)
        exact = current.catalog(StoreSelection.open(self.store)).select_branch(self.marker.branch)
        display = Selection.open(exact.path, expected=exact, retain=True)
        other = self.root/"reencoded.zxs"
        write_snapshot(other, marker=self.marker, year=2001)
        alternative = other.read_bytes()
        self.assertEqual(len(alternative), exact.observed[2])
        self.assertNotEqual(hashlib.sha256(alternative).hexdigest(), exact.sha256)
        for wrong_eof, why in ((False, "selected original was replaced"), (True, "selected source grew")):
            plan = self.choice(display)
            positioned = os.pread
            def read(fd, size, offset):
                self.assertEqual(fd, plan.source_fd)
                if wrong_eof:
                    return b"x" if offset == exact.observed[2] else positioned(fd, size, offset)
                return alternative[offset: offset + size]
            with self.subTest(wrong_eof=wrong_eof), patch.object(os, "pread", side_effect=read):
                with self.assertRaisesRegex(Rejected, why): plan.run()
            self.assertFalse(self.target.exists())
            self.assertFalse(self.state.exists())
            self.assertEqual(list(self.store.iterdir()), [original])

    def test_checkout_hint_uses_only_the_exact_store_entry_and_never_promotes_its_bytes_to_a_verified_base(self):
        slot = self.store/self.marker.filename
        for kind, message in (("missing","未看到原分支基线槽"),("plain","未在这里核验"),
                              ("directory","非普通文件占用"),("symlink","不跟随链接")):
            with self.subTest(kind=kind):
                if kind=="plain": slot.write_bytes(b"not even a valid ZIP")
                elif kind=="directory": slot.mkdir()
                elif kind=="symlink": slot.symlink_to(self.root/"absent-target")
                plan = self.choice(review=False)
                text = plan.review("Design")
                self.assertIn(message,text); self.assertIn(self.marker.filename,text)
                self.assertFalse(self.target.exists()); self.assertFalse(self.state.exists())
                if kind=="plain":
                    self.assertEqual(slot.read_bytes(),b"not even a valid ZIP")
                if kind=="directory": slot.rmdir()
                elif kind!="missing": slot.unlink()

    def test_unavailable_slot_is_not_reported_absent_and_loose_checkout_never_repairs_it(self):
        plan = self.choice(review=False)
        real_stat = os.stat
        def unavailable(path,*args,**kwargs):
            if path==self.marker.filename and kwargs.get("dir_fd")==plan.store.fd:
                raise PermissionError("synthetic metadata lookup unavailable")
            return real_stat(path,*args,**kwargs)
        with patch.object(os,"stat",side_effect=unavailable):
            note = plan.review("Design")
        self.assertIn("无法核对原分支基线槽的属性",note)
        self.assertNotIn("未看到原分支基线槽",note)
        self.assertEqual(plan.run(),(self.target,self.marker))
        self.assertFalse(any(self.store.iterdir()))
        self.assertEqual((self.target/"design/notes").read_bytes(),b"desktop selection")

    def test_head_advancing_after_selection_does_not_change_which_historical_node_is_written(self):
        from zipworkspace import opening
        original = self.store / self.marker.filename
        self.loose.rename(original)
        current = Selection.open(original)
        exact = current.catalog(StoreSelection.open(self.store)).select_branch(self.marker.branch)
        plan = self.choice(Selection.open(exact.path, expected=exact, retain=True))
        newer = replace(self.marker, generation=2, operation="4"*32, parent=self.marker.operation)
        write_snapshot(self.store / newer.filename, marker=newer)
        with patch.object(Selection, "catalog", side_effect=AssertionError("not another head selection")):
            self.assertEqual(plan.run(), (self.target, self.marker))
        self.assertEqual(opening.status(self.state)["source_node"], exact.marker.operation)
        self.assertEqual(hashlib.sha256((self.state/"original.zxs").read_bytes()).hexdigest(), exact.sha256)

    def test_path_replacement_after_review_cannot_redirect_any_of_the_four_selections(self):
        from zipworkspace import opening
        for original in (self.loose, self.store, self.parent, self.private):
            with self.subTest(original=original):
                plan = self.choice()
                backup = self.root / "held-original"
                original.rename(backup)
                if backup.is_dir(): original.mkdir(mode=0o700)
                else: original.write_bytes(backup.read_bytes())  # Equal bytes, distinct original.
                try:
                    with patch.object(opening, "_source", side_effect=AssertionError("no source capture")):
                        with self.assertRaisesRegex(Rejected, "尚未安装原登记"): plan.run()
                    self.assertFalse(any(self.private.iterdir())); self.assertFalse(any(self.parent.iterdir()))
                finally:
                    if original.is_dir(): original.rmdir()
                    else: original.unlink()
                    backup.rename(original)

    def test_new_collision_and_later_mount_change_are_refused_before_any_preparation(self):
        from zipworkspace import opening, private_stage
        plan = self.choice()
        self.target.mkdir(); (self.target / "mine").write_bytes(b"untouched")
        with self.assertRaisesRegex(Rejected, "占用"): plan.run()
        self.assertEqual((self.target / "mine").read_bytes(), b"untouched")
        other = self.choice(review=False)
        native_id = private_stage.mount_id
        def distinct(fd):
            return b"different-actual-mount" if os.path.samestat(os.fstat(fd), self.parent.stat()) else native_id(fd)
        with patch.object(private_stage, "mount_id", side_effect=distinct), \
             patch.object(opening, "_source", side_effect=AssertionError("no ZIP copy or private preparation")):
            with self.assertRaisesRegex(Rejected, "same original mount"): other.review("Other")
        self.assertFalse(any(self.private.iterdir()))

    def test_failed_materialization_is_not_a_durable_receipt_or_resume_instruction(self):
        from zipworkspace import private_stage
        plan = self.choice()
        with patch.object(private_stage, "materialize", side_effect=OSError("stop during private preparation")):
            with self.assertRaises(Rejected) as fail: plan.run()
        self.assertIn("没有可续作的回执", str(fail.exception))
        self.assertNotIn("resume-checkout", str(fail.exception))
        self.assertFalse(self.state.exists()); self.assertFalse(self.target.exists())
        seed, = self.private.iterdir()
        self.assertEqual((seed / "original.zxs").read_bytes(), self.loose.read_bytes())

    def test_stopping_raw_copy_counts_only_captured_bytes_and_leaves_no_state_or_worktree(self):
        from zipworkspace import opening
        plan = self.choice()
        positioned, count = os.pread, self.loose.stat().st_size
        seen = []

        def read(fd, size, offset):
            self.assertEqual(fd, plan.source_fd)
            data = positioned(fd, min(size, 17), offset)
            if offset:
                seen.append(plan.preparation.progress())
                self.assertTrue(plan.cancel())
            return data

        with patch.object(os, "pread", side_effect=read), +             patch.object(opening, "rename_new_at", side_effect=AssertionError("no installation after cancel")), +             self.assertRaisesRegex(Rejected, "没有安装本次 STATE") as failure:
            plan.run()
        self.assertEqual(len(seen), 1)
        self.assertIn("写入私有准备目录", seen[0][0])
        self.assertEqual(seen[0][1:], (17, count))
        seed, = self.private.iterdir()
        self.assertEqual((seed/"original.zxs").read_bytes(), self.loose.read_bytes()[:17])
        self.assertIn(str(seed), str(failure.exception))
        self.assertNotIn("resume-checkout", str(failure.exception))
        self.assertIsNone(plan.publication); self.assertIsNone(plan.take_session())
        self.assertFalse(self.state.exists()); self.assertFalse(self.target.exists())
        self.assertFalse(plan.cancel())
        with self.assertRaises(Rejected): plan.run()

    def test_stop_after_private_database_sync_prevents_even_the_first_native_rename(self):
        from zipworkspace import opening
        plan = self.choice()
        native_sync = os.fsync
        cancelled = []

        def sync(fd):
            native_sync(fd)
            for seed in self.private.iterdir():
                record = seed/"opening.sqlite"
                if record.is_file() and os.path.samestat(os.fstat(fd), record.stat()):
                    self.assertIn("准备新 STATE", plan.preparation.progress()[0])
                    self.assertTrue(plan.cancel())
                    cancelled.append(seed)

        with patch.object(os, "fsync", side_effect=sync), +             patch.object(opening, "rename_new_at", side_effect=AssertionError("original STATE must not be installed")), +             self.assertRaisesRegex(Rejected, "已取消本次签出"):
            plan.run()
        seed, = cancelled
        self.assertEqual(list(self.private.iterdir()), [seed])
        self.assertTrue((seed/"opening.sqlite").is_file())
        self.assertEqual((seed/"prepared-worktree/design/notes").read_bytes(), b"desktop selection")
        self.assertIsNone(plan.publication)
        self.assertFalse(self.state.exists()); self.assertFalse(self.target.exists())

    def test_cancellation_at_the_native_install_does_not_suppress_the_original_result(self):
        from zipworkspace import opening
        plan = self.choice(keep_session=True)
        native = opening.rename_new_at
        attempted = []

        def rename(fd, src, dst, **details):
            if os.path.samestat(os.fstat(fd), self.private.stat()):
                self.assertEqual(dst, self.state.name)
                self.assertFalse(plan.cancel())
                self.assertIn("现在不能撤销", plan.preparation.progress()[0])
                attempted.append(dst)
            return native(fd, src, dst, **details)

        with patch.object(opening, "rename_new_at", side_effect=rename):
            self.assertEqual(plan.run(), (self.target, self.marker))
        self.assertEqual(attempted, [self.state.name])
        self.assertEqual(opening.status(self.state)["operation"], plan.operation)
        self.assertEqual(opening.status(self.state)["recorded_stage"], "attached")
        self.assertIsNotNone(plan.session)

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop extra not installed")
    def test_stop_control_remains_available_outside_the_disabled_worktree_tab(self):
        from concurrent.futures import Future
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        window = Window(); self.addCleanup(window.close)
        pane, plan = window.archive.checkout, self.choice()
        pane.set_snapshot(plan.snapshot)
        pane.active = True
        pending, jobs = Future(), []

        def queue(work):
            jobs.append(work)
            return pending

        with patch.object(window.pool, "submit", side_effect=queue):
            pane._submit(plan, "准备签出", plan.run, lambda _: self.fail("nothing should attach"),
                         preparation=plan.preparation)
        self.assertFalse(window.tabs.isEnabled())
        self.assertTrue(window.transfer_cancel.isEnabled())
        window.transfer_cancel.click()
        self.assertTrue(plan.preparation.cancelled)
        with self.assertRaisesRegex(Rejected, "已取消本次签出") as failure: jobs[0]()
        pending.set_exception(failure.exception); window.poll()
        self.assertFalse(pane.active); self.assertFalse(pane.unresolved)
        self.assertIsNone(pane.live); self.assertFalse(any(self.private.iterdir()))
        self.assertIn("没有安装本次 STATE", pane.result.text())

    def test_lost_reply_after_real_cross_parent_move_names_only_original_durable_operation(self):
        from zipworkspace import opening, private_stage
        plan = self.choice(); native = opening.rename_new_at
        def lose_reply(fd, src, dst, **kwargs):
            native(fd, src, dst, **kwargs)
            if src == private_stage.LEAF and kwargs.get("target_fd") is not None:
                raise InterruptedError("lost move response")
        with patch.object(opening, "rename_new_at", side_effect=lose_reply):
            with self.assertRaises(Rejected) as fail: plan.run()
        self.assertIn("已读到本次原登记", str(fail.exception)); self.assertIn(plan.operation, str(fail.exception))
        self.assertIn("resume-checkout", str(fail.exception))
        self.assertEqual(opening.status(self.state)["recorded_stage"], "verified_stage")
        inode = self.target.stat().st_ino
        self.assertEqual(opening.finish(self.state, plan.operation), (self.target, self.marker))
        self.assertEqual(self.target.stat().st_ino, inode)
        self.assertEqual(opening.status(self.state)["recorded_stage"], "attached")

    def stopped_after_registration(self):
        from zipworkspace import opening
        plan = self.choice(keep_session=True)
        with patch.object(opening, "finish", side_effect=InterruptedError("lost after durable STATE")), \
             self.assertRaisesRegex(Rejected, "已读到本次原登记"):
            plan.run()
        self.assertTrue(plan.can_review_recovery)
        self.assertTrue(plan.registered)
        self.assertIsNone(plan.source_fd)
        self.assertFalse(self.target.exists())
        return plan

    def test_same_window_can_move_only_the_original_prepared_tree_with_a_separate_confirmation(self):
        from zipworkspace import opening, private_stage
        plan = self.stopped_after_registration()
        stage = self.state/private_stage.LEAF
        inode = stage.stat().st_ino
        self.loose.rename(self.root/"offline.zxs")
        native = opening.sqlite3.connect
        modes = []

        def connect(name, **options):
            modes.append(name.endswith("?mode=ro"))
            return native(name, **options)

        with patch.object(opening.sqlite3, "connect", side_effect=connect):
            offer, body = plan.review_recovery()
        self.assertEqual(modes, [True])
        self.assertIn(plan.operation, body); self.assertIn(str(self.target), body)
        self.assertIn("仍在原私处", body); self.assertFalse(self.target.exists())
        with patch.object(opening.sqlite3, "connect", side_effect=AssertionError("no writer without confirmation")):
            with self.assertRaisesRegex(Rejected, "明确继续"):
                opening.finish(plan.state, plan.operation, selected=plan)
            with self.assertRaisesRegex(Rejected, "明确确认"):
                plan.resume(object(), confirmed=True)
            with self.assertRaisesRegex(Rejected, "明确确认"):
                plan.resume(offer)
        with patch.object(opening, "_source", side_effect=AssertionError("no archive recapture")):
            self.assertEqual(plan.resume(offer, confirmed=True), (self.target, self.marker))
        self.assertTrue(plan.receipt_synced); self.assertFalse(plan.can_review_recovery)
        self.assertEqual(self.target.stat().st_ino, inode)
        self.assertEqual((self.target/"design/notes").read_bytes(), b"desktop selection")
        self.assertFalse(stage.exists()); self.assertFalse(any(self.store.iterdir()))
        self.assertEqual(opening.status(self.state)["operation"], plan.operation)
        self.assertIsNotNone(plan.session)
        with self.assertRaises(Rejected): plan.resume(offer, confirmed=True)

    def test_recovery_of_a_moved_original_does_not_switch_to_a_second_rename(self):
        from zipworkspace import opening, private_stage
        plan = self.choice(keep_session=True)
        original = opening.rename_new_at
        def lose_reply(fd, source, dest, **options):
            original(fd, source, dest, **options)
            if source == private_stage.LEAF: raise InterruptedError("lost after exact worktree rename")
        with patch.object(opening, "rename_new_at", side_effect=lose_reply), \
             self.assertRaisesRegex(Rejected, "已读到本次原登记"):
            plan.run()
        self.assertTrue(self.target.is_dir())
        inode = self.target.stat().st_ino
        offer, text = plan.review_recovery()
        self.assertIn("已经在原正式路径", text)
        old_stage = self.state/private_stage.LEAF
        self.target.rename(old_stage)
        with patch.object(opening, "rename_new_at", side_effect=AssertionError("do not swap ACK for MOVE")), \
             self.assertRaisesRegex(Rejected, "不会转换"):
            plan.resume(offer, confirmed=True)
        self.assertFalse(self.target.exists()); self.assertEqual(old_stage.stat().st_ino, inode)
        self.assertEqual(opening.status(self.state)["recorded_stage"], "verified_stage")
        old_stage.rename(self.target)
        again, _ = plan.review_recovery()  # Now a new single decision after observing the same original.
        with patch.object(opening, "rename_new_at", side_effect=AssertionError("only acknowledge this prior move")):
            self.assertEqual(plan.resume(again, confirmed=True), (self.target, self.marker))
        self.assertEqual(self.target.stat().st_ino, inode)
        self.assertTrue(plan.receipt_synced)

    def test_replaced_opening_database_is_not_reopened_for_review_even_if_the_bytes_match(self):
        from zipworkspace import opening
        plan = self.stopped_after_registration()
        db, old = self.state/"opening.sqlite", self.state/"saved-opening"
        db.rename(old); db.write_bytes(old.read_bytes()); db.chmod(0o600)
        self.assertTrue(os.path.samestat(os.fstat(plan.prepared[2]), old.stat()))
        with patch.object(opening.sqlite3, "connect", side_effect=AssertionError("not the creator's original DB")), \
             self.assertRaisesRegex(Rejected, "原登记数据库已变化"):
            plan.review_recovery()
        self.assertFalse(self.target.exists())

    def test_recovery_does_not_adopt_an_equal_copied_stage(self):
        from shutil import copytree
        from zipworkspace import private_stage
        plan = self.stopped_after_registration()
        stage, old = self.state/private_stage.LEAF, self.state/"saved-preparation"
        stage.rename(old); copytree(old, stage)
        self.assertTrue(os.path.samestat(os.fstat(plan.prepared[1]), old.stat()))
        self.assertEqual((stage/"design/notes").read_bytes(), (old/"design/notes").read_bytes())
        with self.assertRaisesRegex(Rejected, "名称已变化"):
            plan.review_recovery()
        self.assertFalse(self.target.exists())

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop extra not installed")
    def test_read_only_recovery_button_does_not_start_a_writer_on_decline(self):
        from concurrent.futures import Future
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import checkout_pane
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        window = Window(); self.addCleanup(window.close)
        pane, plan = window.archive.checkout, self.stopped_after_registration()
        pane.set_snapshot(plan.snapshot)
        pane._discard(plan, "synthetic lost reply")
        self.assertTrue(pane.recover.isEnabled())
        pending, workers = Future(), []
        def submit(work):
            workers.append(work)
            return pending
        with patch.object(window.pool, "submit", side_effect=submit), \
             patch.object(checkout_pane, "confirm", return_value=False) as confirm:
            pane.recover.click()
            self.assertEqual(len(workers), 1)
            pending.set_result(workers[0]())
            window.poll()
        confirm.assert_called_once()
        self.assertEqual(len(workers), 1)
        self.assertIsNone(plan.resume_offer); self.assertIs(pane.recover_checkout, plan)
        self.assertFalse(self.target.exists()); self.assertFalse(pane.active)
        self.assertIn("没有继续原签出", pane.result.text())
        pane.set_snapshot(None)
        self.assertTrue(plan.closed); self.assertIsNone(pane.recover_checkout)

    def test_first_outer_barrier_failure_does_not_offer_same_window_continuation(self):
        from zipworkspace import opening
        plan = self.choice(keep_session=True)
        real = os.fsync
        def barrier(fd):
            if self.state.is_dir() and os.path.samestat(os.fstat(fd), self.private.stat()):
                raise OSError("first outer STATE barrier lost")
            return real(fd)
        with patch.object(os, "fsync", side_effect=barrier), \
             self.assertRaisesRegex(Rejected, "未确认准备与父目录屏障"):
            plan.run()
        self.assertFalse(plan.can_review_recovery)
        self.assertEqual(opening.status(self.state)["operation"], plan.operation)
        self.assertFalse(self.target.exists())

    def test_visible_done_row_after_the_final_barrier_fails_is_only_a_read_only_receipt(self):
        from zipworkspace import opening
        plan = self.choice(keep_session=True)
        real, calls = os.fsync, []
        def barrier(fd):
            if ((self.state/"worktree.sqlite").is_file() and os.path.samestat(os.fstat(fd), self.state.stat())):
                calls.append(fd)
                if len(calls) == 2: raise OSError("last original STATE barrier lost")
            return real(fd)
        with patch.object(os, "fsync", side_effect=barrier), \
             self.assertRaisesRegex(Rejected, "未确认其耐久"):
            plan.run()
        self.assertEqual(len(calls), 2)
        self.assertFalse(plan.receipt_synced); self.assertIsNone(plan.session)
        self.assertEqual(opening.status(self.state)["recorded_stage"], "attached")
        offer, result = plan.review_recovery()
        self.assertIsNone(offer); self.assertIn("没有写入或接管", result)
        with self.assertRaises(Rejected): plan.resume(offer, confirmed=True)

    def test_substituted_state_after_initial_install_never_opens_or_resumes_the_replacement(self):
        from zipworkspace import opening
        plan = self.choice(); native = opening.finish
        saved = self.private / "retained-original"
        def exchange(state, operation, **kwargs):
            state.rename(saved); state.mkdir(mode=0o700)
            (state / "opening.sqlite").write_bytes(b"not the original database")
            with patch.object(opening.sqlite3, "connect", side_effect=AssertionError("no replacement SQLite open")):
                return native(state, operation, **kwargs)
        with patch.object(opening, "finish", side_effect=exchange):
            with self.assertRaises(Rejected) as fail: plan.run()
        self.assertIn("签出结果未知", str(fail.exception)); self.assertNotIn("resume-checkout", str(fail.exception))
        self.assertEqual((self.state / "opening.sqlite").read_bytes(), b"not the original database")
        self.assertTrue((saved / "prepared-worktree/design/notes").is_file())
        self.assertFalse(self.target.exists())

    def test_session_pins_the_exclusive_new_catalog_before_exposure_and_reports_real_status(self):
        import fcntl
        from zipworkspace import worktree
        from zipworkspace.desktop.live_checkout import LiveCheckout
        plan = self.choice(keep_session=True)
        installed = []; creator = LiveCheckout.new_catalog
        def witness(live, path, original_fd):
            self.assertFalse((self.state/"worktree.sqlite").exists())
            creator(live, path, original_fd)
            self.assertTrue(os.path.samestat(os.fstat(original_fd), os.fstat(live.db)))
            self.assertEqual(fcntl.fcntl(live.db, fcntl.F_GETFL) & os.O_PATH, os.O_PATH)
            installed.append(os.fstat(live.db))
        with patch.object(LiveCheckout, "new_catalog", new=witness): plan.run()
        live = plan.take_session(); self.assertIsNotNone(live); self.addCleanup(live.close)
        self.assertIsNone(plan.take_session())
        self.assertTrue(os.path.samestat(installed[0], (self.state/"worktree.sqlite").stat()))
        (self.target / "design/notes").write_bytes(b"editing does not imply another version")
        active = live.status()
        self.assertEqual(active, worktree.status(self.state))
        self.assertEqual(active["worktree"], live.instance)
        self.assertEqual(active["base_operation"], self.marker.operation)
        self.assertIsNone(active["pending_operation"])
        live.close()
        with self.assertRaises(Rejected): live.status()

    def test_explicitly_discarding_a_returned_plan_closes_its_untaken_session(self):
        plan = self.choice(keep_session=True); plan.run()
        still_owned = plan.session
        self.assertIsNotNone(still_owned); self.assertFalse(still_owned.closed)
        plan.close()
        self.assertTrue(still_owned.closed); self.assertIsNone(plan.take_session())

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop extra not installed")
    def test_late_or_failed_checkout_display_closes_the_undelivered_original_session(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window(); self.addCleanup(win.close)
        pane = win.archive.checkout
        for mode in ("changed", "failed"):
            plan = self.choice(review=False, keep_session=True)
            plan.review(mode)
            pane.set_snapshot(plan.snapshot)
            pane.active = True
            result = plan.run(); original = plan.session
            if mode == "changed":
                pane.set_snapshot(Selection.open(self.loose))  # Selected anew after the real write returned.
                pane._complete(plan, result)
            else:
                with patch.object(pane.result, "setText", side_effect=RuntimeError("view lost")):
                    with self.assertRaisesRegex(RuntimeError, "view lost"): pane._complete(plan, result)
            self.assertTrue(original.closed); self.assertIsNone(pane.live); self.assertFalse(pane.active)
            self.assertEqual((self.parent/mode/"design/notes").read_bytes(), b"desktop selection")

    def test_live_status_refuses_an_equal_copy_of_database_before_opening_sqlite(self):
        from zipworkspace import worktree
        plan = self.choice(keep_session=True); plan.run()
        live = plan.take_session(); self.addCleanup(live.close)
        db, original = self.state/"worktree.sqlite", self.state/"original-catalog"
        db.rename(original); db.write_bytes(original.read_bytes()); db.chmod(0o600)
        with patch.object(worktree.sqlite3, "connect", side_effect=AssertionError("no SQLite on the new inode")):
            with self.assertRaises(Rejected): live.status()
        self.assertEqual(db.read_bytes(), original.read_bytes())
        db.unlink(); original.rename(db)
        self.assertEqual(live.status()["worktree"], live.instance)
        for directory in (self.target, self.store, self.state):
            with self.subTest(directory=directory):
                original = directory.with_name("saved-" + directory.name)
                directory.rename(original); directory.mkdir(mode=0o700)
                try:
                    with self.assertRaises((OSError, Rejected)): live.status()
                    self.assertFalse(any(directory.iterdir()))
                finally:
                    directory.rmdir(); original.rename(directory)

    def test_original_visible_and_hidden_status_come_from_the_same_cli_instance(self):
        from zipworkspace import lifecycle
        (self.store / self.marker.filename).write_bytes(self.loose.read_bytes())
        plan = self.choice(keep_session=True); plan.run()
        live = plan.take_session(); self.addCleanup(live.close)
        receipt = lifecycle.start(self.state, "hide")
        self.assertEqual(live.status()["visibility"], "hidden")
        self.assertFalse(self.target.exists())
        self.assertEqual(receipt["original_outcome"], "hidden")
        lifecycle.start(self.state, "restore")
        self.assertEqual(live.status()["visibility"], "visible")
        self.assertTrue(os.path.samestat(self.target.stat(), os.fstat(live.work_fd)))

    def test_completed_elsewhere_is_a_valid_checkout_receipt_but_never_a_window_session(self):
        from zipworkspace import opening
        native = opening.finish
        def other_first(state, op, *, selected):
            # Selected caller admits its true prepared DB; another legitimate CLI
            # actor finishes that same operation before this caller's next transaction.
            with opening._record(state, write=False, selected=selected): pass
            self.assertEqual(native(state, op), (self.target, self.marker))
            return native(state, op, selected=selected)
        plan = self.choice(keep_session=True)
        with patch.object(opening, "finish", side_effect=other_first):
            self.assertEqual(plan.run(), (self.target, self.marker))
        self.assertIsNone(plan.take_session())
        self.assertEqual(opening.status(self.state)["recorded_stage"], "attached")

    def test_failed_catalog_directory_barrier_keeps_original_receipt_but_closes_provisional_session(self):
        from zipworkspace import opening
        from zipworkspace.desktop.live_checkout import LiveCheckout
        plan = self.choice(keep_session=True)
        native, sync = LiveCheckout.new_catalog, os.fsync
        scopes = []
        def remember(live, path, fd):
            native(live, path, fd); scopes.append(live)
        def stop(fd):
            if (scopes and (self.state/"worktree.sqlite").is_file()
                    and os.path.samestat(os.fstat(fd), os.fstat(scopes[0].state_fd))):
                raise OSError("lost original STATE sync")
            return sync(fd)
        with patch.object(LiveCheckout, "new_catalog", new=remember), patch.object(os, "fsync", side_effect=stop):
            with self.assertRaisesRegex(Rejected, "lost original STATE sync"): plan.run()
        self.assertTrue(scopes[0].closed)
        self.assertIsNone(plan.take_session())
        self.assertEqual(opening.status(self.state)["recorded_stage"], "verified_stage")
        self.assertEqual(opening.finish(self.state, plan.operation), (self.target, self.marker))

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop extra not installed")
    def test_widget_cancellation_stale_confirmation_and_reentry_do_not_create_a_second_operation(self):
        from concurrent.futures import Future
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import checkout_pane as ui
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        self.addCleanup(win.close)
        pane = win.archive.checkout
        def dispatch(title, work, apply, *, failed=None, **controls):
            if title.startswith("正在按原节点"):
                self.assertTrue(callable(controls["cancel"]))
                self.assertTrue(callable(controls["progress_report"]))
            future = Future()
            try: result = work()
            except Exception as error:
                future.set_exception(error); failed(error)
            else:
                future.set_result(result); apply(result)
            return future
        for mode in ("cancel", "selection_changed", "accept"):
            with self.subTest(mode=mode):
                win.archive.show_selection(Selection.open(self.loose, retain=True))
                def decide(_parent, _title, detail, *, attestation):
                    self.assertIn(str(self.target), detail); self.assertIn("独占", attestation)
                    pane.pick()  # The confirmation is already active; must not open another picker.
                    if mode == "selection_changed": pane.set_snapshot(None)
                    return mode != "cancel"
                with patch.object(win, "job", side_effect=dispatch), \
                     patch.object(ui.QFileDialog, "getExistingDirectory",
                                  side_effect=list(map(str, (self.store,self.parent,self.private)))) as picks, \
                     patch.object(ui.QInputDialog, "getText", return_value=("Design", True)), \
                     patch.object(ui, "confirm", side_effect=decide):
                    pane.pick()
                self.assertEqual(picks.call_count, 3)
                opts = picks.call_args_list[0].kwargs["options"]
                self.assertTrue(opts & ui.QFileDialog.Option.ReadOnly)
                self.assertFalse(pane.active)
                self.assertEqual(self.target.is_dir(), mode == "accept")
                self.assertEqual(self.state.is_dir(), mode == "accept")
        self.assertIn(self.marker.operation, pane.result.text())
        self.assertEqual((self.target/"design/notes").read_bytes(), b"desktop selection")
        self.assertIn(pane.live.instance, pane.details.text())
        self.assertIn("未检查工作文件", pane.details.text())
        active, displayed = pane.live, pane.snapshot
        win.archive.clear_document()
        self.assertTrue(displayed.held.closed)
        self.assertIs(pane.live, active)  # Leaving this display does not close the existing CLI worktree.
        self.assertFalse(active.closed)
        pane.close_session()
        self.assertTrue(active.closed); self.assertIsNone(pane.live)
        self.assertFalse(pane.refresh.isEnabled())
