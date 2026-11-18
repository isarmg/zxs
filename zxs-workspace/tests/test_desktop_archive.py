from dataclasses import replace
import hashlib
from importlib.util import find_spec
from pathlib import Path
import tempfile
from unittest.mock import patch
import unittest
import zipfile

from zipworkspace.desktop.selection import Selection, StoreSelection
from zipworkspace.model import Commit, Manifest, Rejected


def write_snapshot(path, *, marker=None, year=2000):
    rows = Manifest()
    content = b"desktop selection"
    rows.add("design/notes", False, len(content), hashlib.sha256(content).hexdigest())
    rows.add("empty", True, 0, None)
    marker = marker or Commit("1"*32, "2"*32, 1, "3"*32, None, rows.digest())
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as out:
        for name, data in (("design/notes", content), ("empty/", b"")):
            entry = zipfile.ZipInfo(name, (year, 1, 1, 0, 0, 0))
            entry.external_attr = ((0o40755 if name.endswith("/") else 0o100644) << 16)
            out.writestr(entry, data)
        out.comment = marker.encode()
    return marker


class SelectedOriginal(unittest.TestCase):
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
            expected = refs._members(store, marker.repo)[member.name]
            original = root / "old-slot"
            member.rename(original)
            member.write_bytes(original.read_bytes())
            with self.assertRaises(Rejected): refs._load(member, expected=expected)
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
