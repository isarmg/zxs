import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock
import zipfile

from zipworkspace.browser import ArchiveBrowser
from zipworkspace.cli import main
from zipworkspace.model import Commit, Manifest, Rejected


class Browse(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.path = self.root / "original.zxs"

    def write(self, path, *, operation="3" * 32, corrupt=False):
        rows = {"a/empty/": None, "a/same": b"", "b/same": b"second",
                "c": b"c", "z": b"not a visible first-page member"}
        manifest = Manifest()
        with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as archive:
            for name, data in rows.items():
                archive.writestr(name, data or b"")
                digest = hashlib.sha256(data).hexdigest() if data is not None else None
                manifest.add(name.removesuffix("/"), data is None, len(data or b""), digest)
            commit = Commit("1"*32, "2"*32, 1, operation, None,
                            "0"*64 if corrupt else manifest.digest())
            archive.comment = commit.encode()
        return commit

    def test_borrowed_document_returns_verified_immediate_children_and_preserves_file(self):
        commit = self.write(self.path)
        before = set(self.root.iterdir())
        with self.path.open("rb") as doc:
            doc.seek(7)
            browser = ArchiveBrowser(doc, display_name="选择的归档.zxs")
            self.assertEqual(doc.tell(), 7)
        self.assertEqual(browser.commit, commit)
        page = browser.list_directory(limit=1)
        self.assertEqual([(x.name, x.kind) for x in page.entries], [("a", "directory")])
        self.assertEqual([x.name for x in browser.list_directory(cursor=page.next_cursor).entries],
                         ["b", "c", "z"])
        empty, file = browser.list_directory("a").entries
        self.assertEqual((empty.name, empty.kind, empty.size_bytes, empty.sha256),
                         ("empty", "directory", None, None))
        self.assertEqual((file.name, file.size_bytes, file.sha256),
                         ("same", 0, hashlib.sha256(b"").hexdigest()))
        self.assertEqual(browser.list_directory("b").entries[0].sha256, hashlib.sha256(b"second").hexdigest())
        self.assertEqual(browser.list_directory("a/empty").entries, ())
        self.assertIsNone(browser.list_directory("a/empty").next_cursor)
        self.assertEqual(set(self.root.iterdir()), before)

    def test_continuations_reopen_only_the_same_logical_snapshot_and_folder(self):
        self.write(self.path)
        first = ArchiveBrowser.open(self.path)
        cursor = first.list_directory(limit=1).next_cursor
        same = ArchiveBrowser.open(self.path)
        self.assertEqual(same.list_directory(cursor=cursor).entries[0].name, "b")
        for folder, token in (("a", cursor), ("", cursor + "missing"),
                              ("", cursor.split(":")[0] + ":/a")):
            with self.subTest(folder=folder, token=token), self.assertRaises(Rejected):
                same.list_directory(folder, cursor=token)
        for folder in ("c", "missing", "../a", "a/"):
            with self.subTest(folder=folder), self.assertRaises(Rejected):
                same.list_directory(folder)
        for limit in (False, 0, 257):
            with self.assertRaises(Rejected): same.list_directory(limit=limit)
        self.write(self.path, operation="4"*32)
        with self.assertRaises(Rejected): ArchiveBrowser.open(self.path).list_directory(cursor=cursor)
        self.assertEqual(first.list_directory(cursor=cursor).entries[0].name, "b")

    def test_cli_does_not_show_even_a_first_page_until_whole_archive_is_verified(self):
        self.write(self.path)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            self.assertEqual(main(["browse", str(self.path), "a", "--limit", "1"]), 0)
        reply = json.loads(buf.getvalue())
        self.assertEqual((reply["directory"], reply["entries"][0]["name"]), ("a", "empty"))
        self.assertIsNotNone(reply["next_cursor"])
        # A valid CRC and a well-formed marker are not enough when the represented tree disagrees.
        self.write(self.path, corrupt=True)
        blocked = io.StringIO()
        with contextlib.redirect_stdout(blocked), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(main(["browse", str(self.path), "--limit", "1"]), 1)
        self.assertEqual(blocked.getvalue(), "")
        with zipfile.ZipFile(self.path, "w") as plain: plain.writestr("x", b"hello")
        with self.assertRaises(Rejected): ArchiveBrowser.open(self.path)
        with self.path.open("rb") as fp, self.assertRaises(Rejected):
            ArchiveBrowser(fp, display_name="plain.zip")

    def test_named_selection_never_matches_two_unavailable_identities(self):
        self.write(self.path)
        actual = self.path.stat()
        unknown = SimpleNamespace(st_mode=actual.st_mode, st_dev=actual.st_dev, st_ino=0)
        with mock.patch.object(Path, "stat", return_value=unknown), self.assertRaises(Rejected):
            ArchiveBrowser.open(self.path)

    def test_replacing_selected_name_after_open_does_not_display_a_substitute(self):
        self.write(self.path)
        replacement = self.root / "other.zxs"
        self.write(replacement, operation="4"*32)
        from zipworkspace.browser import read_managed_document
        blocked = []
        def replace_after_read(document, *, display_name):
            verified = read_managed_document(document, display_name=display_name)
            try:
                replacement.replace(self.path)
            except PermissionError:
                if os.name != "nt": raise
                blocked.append(True)  # The OS can itself forbid replacing the held file.
            return verified
        with mock.patch("zipworkspace.browser.read_managed_document", side_effect=replace_after_read):
            try:
                selected = ArchiveBrowser.open(self.path)
            except Rejected:
                self.assertFalse(blocked)
            else:
                self.assertTrue(blocked)
                self.assertEqual(selected.commit.operation, "3"*32)

    def test_mismatched_final_named_identity_is_rejected_even_on_nonreplacing_platforms(self):
        self.write(self.path)
        replacement = self.root / "other.zxs"
        self.write(replacement, operation="4"*32)
        first, other = self.path.stat(), replacement.stat()
        with mock.patch.object(Path, "stat", side_effect=[first, other]), self.assertRaises(Rejected):
            ArchiveBrowser.open(self.path)
