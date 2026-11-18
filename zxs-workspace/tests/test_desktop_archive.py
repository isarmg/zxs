from dataclasses import replace
import hashlib
from importlib.util import find_spec
from pathlib import Path
import tempfile
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
            self.assertEqual(catalog.select_node(first.operation), selection.node)
            later = replace(first, generation=2, operation="4"*32, parent=first.operation)
            write_snapshot(store / later.filename, marker=later)
            partial = selection.catalog(chosen_store)
            self.assertEqual(partial.branches()[0]["missing_parent"], first.operation)
            with self.assertRaises(Rejected): partial.select_branch(first.branch)
            (store / first.filename).write_bytes(loose.read_bytes())
            complete = selection.catalog(chosen_store)
            head = complete.select_branch(first.branch)
            self.assertEqual(Selection.open(head.path, expected=head).browser.commit, later)

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

    def test_same_logical_tree_reencoded_is_not_the_selected_original_for_a_write(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "original.zxs"
            marker = write_snapshot(path)
            original = Selection.open(path)
            write_snapshot(path, marker=marker, year=2001)
            self.assertEqual(Selection.open(path).browser.commit, marker)
            self.assertNotEqual(Selection.open(path).node.sha256, original.node.sha256)
            with self.assertRaises(Rejected): Selection.open(path, expected=original.node)
            with self.assertRaises(Rejected): original.catalog(StoreSelection.open(root))


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
