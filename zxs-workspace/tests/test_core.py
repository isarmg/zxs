import os
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
from unittest import mock
import zipfile
from zipworkspace.archive import read_archive
from zipworkspace.model import Commit, Manifest, Rejected
from zipworkspace import worktree as w
from zipworkspace import publish as media


class Repo(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.work, self.store, self.state = [self.root / n for n in ("work", "store", "state")]
        self.work.mkdir(); self.store.mkdir()
        (self.work / "空目录").mkdir()
        (self.work / "組件").mkdir()
        (self.work / "組件" / "data.bin").write_bytes(bytes(range(256)))
        (self.work / "plain.txt").write_text("original", encoding="utf-8")
        w.enroll(self.work, self.store, self.state)

    def test_standard_zip_and_unicode_empty_directory_and_exact_history(self):
        first, initial = w.commit(self.state)
        marker, tree = read_archive(first)
        self.assertEqual(marker, initial)
        self.assertEqual((True, 0, None), tree.rows["空目录"])
        copy = self.root / "ordinary.zip"; copy.write_bytes(first.read_bytes())
        with zipfile.ZipFile(copy) as z:
            self.assertTrue(all(i.compress_type == zipfile.ZIP_STORED and not (i.flag_bits & 1) for i in z.infolist()))
            self.assertEqual(set(z.namelist()), {"空目录/", "plain.txt", "組件/", "組件/data.bin"})
            self.assertEqual(z.read("組件/data.bin"), bytes(range(256)))
        subprocess.run(["unzip", "-tqq", str(copy)], check=True)
        (self.work / "plain.txt").write_text("new")
        second, head = w.commit(self.state)
        self.assertEqual(w.history(second, self.store), ([head, initial], None))
        self.assertEqual(w.finish(self.state, initial.operation), (first, initial))
        other = self.root / "other"; otherstate = self.root / "otherstate"
        w.checkout(first, other, self.store, otherstate)
        self.assertEqual((other / "plain.txt").read_text(), "original")
        self.assertTrue((other / "空目录").is_dir())
        self.assertNotEqual(w.status(self.state)["worktree"], w.status(otherstate)["worktree"])

    def test_independent_clean_checkout_competition_and_explicit_fork(self):
        first, initial = w.commit(self.state)
        for n in ("a", "b"):
            w.checkout(first, self.root / n, self.store, self.root / (n+"s"))
            (self.root / n / "plain.txt").write_text(n)
        advance, head = w.commit(self.root / "as")
        self.assertEqual(head.parent, initial.operation)
        # Fork explicitly before a conflict; no implicit branch from stale head.
        forked, fork = w.commit(self.root / "bs", fork=True)
        self.assertEqual(fork.parent, initial.operation)
        self.assertNotEqual(fork.branch, head.branch)
        self.assertEqual(w.history(forked, self.store), ([fork, initial], None))
        (self.work / "plain.txt").write_text("third candidate")
        with self.assertRaises(media.Conflict): w.commit(self.state)
        self.assertIsNotNone(w.status(self.state)["pending_operation"])
        self.assertEqual(read_archive(advance)[0], head)
        self.assertEqual(read_archive(first)[0], initial)

    def test_interruption_retains_original_and_subsequent_edits_are_not_implicitly_packed(self):
        orig, marker = w.commit(self.state)
        (self.work / "plain.txt").write_text("expected")
        with mock.patch("zipworkspace.publish.rename_new", side_effect=OSError("response lost before publish")):
            with self.assertRaises(media.Uncertain): w.commit(self.state)
        op = w.status(self.state)["pending_operation"]
        self.assertIsNotNone(op)
        self.assertEqual(read_archive(orig)[0], marker)
        (self.work / "plain.txt").write_text("not part of that request")
        result, committed = w.finish(self.state, op)
        with zipfile.ZipFile(result) as z: self.assertEqual(z.read("plain.txt"), b"expected")
        self.assertEqual(w.finish(self.state, op), (result, committed))
        self.assertEqual(len(list(self.store.glob("*.zxs"))), 2)

    def test_lost_reply_after_rename_is_resolved_by_original_not_replaced(self):
        original = media.rename_new
        def lost(src, dst):
            original(src, dst)
            raise OSError("link lost after publication")
        with mock.patch("zipworkspace.publish.rename_new", side_effect=lost):
            path, committed = w.commit(self.state)
        self.assertEqual(read_archive(path)[0], committed)
        self.assertIsNone(w.status(self.state)["pending_operation"])

    def test_plain_import_and_managed_open_are_separate(self):
        plain = self.root / "plain.zip"
        with zipfile.ZipFile(plain, "w", zipfile.ZIP_STORED) as z: z.writestr("hello", b"ok")
        copy = self.root / "plain.zxs"; copy.write_bytes(plain.read_bytes())
        with self.assertRaises(Rejected): read_archive(copy)
        with self.assertRaises(Rejected): read_archive(plain)
        dest = self.root / "import"
        w.import_zip(plain, dest)
        self.assertEqual((dest / "hello").read_bytes(), b"ok")
        with self.assertRaises(Rejected): w.import_zip(plain, dest)

    def test_malformed_tree_is_never_exposed_at_destination(self):
        for i, names in enumerate((["../off"], ["a/file", "A/file2"], ["file", "file/dir"], ["CON.txt"])):
            path = self.root / f"in{i}.zip"
            with zipfile.ZipFile(path, "w") as z:
                for name in names: z.writestr(name, b"123")
            dest = self.root / "out"
            with self.assertRaises(Rejected): w.import_zip(path, dest)
            self.assertFalse(dest.exists())
        self.assertFalse((self.root / "off").exists())

    def test_changed_generation_or_link_does_not_publish(self):
        (self.work / "alias").symlink_to("plain.txt")
        with self.assertRaises(Rejected): w.commit(self.state)
        self.assertEqual(list(self.store.iterdir()), [])
        self.assertIsNone(w.status(self.state)["pending_operation"])

    def test_marker_and_resource_limits_are_strict(self):
        from zipworkspace.model import member_name
        for bad in ("NUL", "LPT¹.log", "trailing.", "e\u0301", "A\\B", "a//b", "x\0z"):
            with self.subTest(bad=bad), self.assertRaises(Rejected): member_name(bad)
        empty = Manifest().digest()
        from zipworkspace.model import new_id
        c = Commit(new_id(), new_id(), 1, new_id(), None, empty)
        with self.assertRaises(Rejected): Commit.decode(c.encode() + b" ")
        with self.assertRaises(Rejected): Commit.decode(b"{}")
        plain = self.root / "forged.zip"
        with zipfile.ZipFile(plain, "w") as z: z.writestr("ok", b"data")
        raw = bytearray(plain.read_bytes())
        footer = raw.rfind(b"PK\x05\x06")
        struct.pack_into("<HH", raw, footer+8, 0, 0); plain.write_bytes(raw)
        with self.assertRaises(Rejected): read_archive(plain, managed=False)

if __name__ == "__main__":
    unittest.main()
