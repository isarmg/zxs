import hashlib
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile
from zipworkspace import lifecycle, worktree as w
from zipworkspace.archive import read_archive
from zipworkspace.model import Commit, Manifest, Rejected, new_id
from zipworkspace.publish import Conflict


class LocalWorktree(unittest.TestCase):
    def setUp(self):
        self.area = tempfile.TemporaryDirectory()
        self.addCleanup(self.area.cleanup)
        self.root = Path(self.area.name)
        self.work, self.store, self.state = (self.root / n for n in ["work", "store", "state"])
        self.work.mkdir(); self.store.mkdir()
        (self.work / "story").write_bytes(b"old")
        w.enroll(self.work, self.store, self.state)
        self.base, self.marker = w.commit(self.state)

    def test_clean_visibility_is_the_original_inode_and_restores_non_overwriting(self):
        identity = lifecycle.who(self.work)
        hidden = lifecycle.start(self.state, "hide")
        self.assertFalse(self.work.exists())
        self.assertEqual(w.status(self.state)["visibility"], "hidden")
        with self.assertRaises(Rejected): w.commit(self.state)
        self.work.mkdir()
        (self.work / "occupied").write_text("outside worktree")
        with self.assertRaises(Rejected): lifecycle.start(self.state, "restore")
        self.assertEqual((self.work / "occupied").read_text(), "outside worktree")
        self.work.rename(self.root / "unrelated")
        restored = lifecycle.start(self.state, "restore")
        self.assertEqual(lifecycle.who(self.work), identity)
        self.assertEqual((self.work / "story").read_bytes(), b"old")
        report = lifecycle.finish(self.state, hidden["operation"])
        self.assertEqual((report["original_outcome"], report["recorded_phase"]), ("hidden", "visible"))
        self.assertTrue(lifecycle.finish(self.state, restored["operation"])["current_original_named"])

    def test_dirty_and_same_data_new_inode_are_preserved_not_treated_as_original(self):
        (self.work / "story").write_bytes(b"changed")
        self.assertEqual(lifecycle.changes(self.state)["changed"]["first_paths"], ["story"])
        with self.assertRaises(Rejected): lifecycle.start(self.state, "hide")
        self.assertEqual(w.status(self.state)["visibility"], "visible")
        self.work.rename(self.root / "displaced")
        self.work.mkdir()
        (self.work / "story").write_bytes(b"old")
        for action in [lambda: w.commit(self.state), lambda: lifecycle.start(self.state, "hide")]:
            with self.assertRaises(Rejected): action()
        self.assertEqual((self.root / "displaced" / "story").read_bytes(), b"changed")

    def test_lost_reply_after_local_move_resumes_original_without_second_rename(self):
        primitive = lifecycle.rename_new
        def interrupted(src, dst):
            primitive(src, dst); raise OSError("interrupted after moving")
        with patch.object(lifecycle, "rename_new", side_effect=interrupted):
            with self.assertRaises(OSError): lifecycle.start(self.state, "hide")
        pending = w.status(self.state)
        self.assertEqual(pending["visibility"], "hide-pending")
        with patch.object(lifecycle, "rename_new", side_effect=AssertionError("must not move again")):
            result = lifecycle.finish(self.state, pending["pending_visibility_operation"])
        self.assertEqual(result["original_outcome"], "hidden")

    def test_explicit_fork_uses_conflicted_candidate_not_later_worktree_edits(self):
        x, xs = self.root / "x", self.root / "xs"
        w.checkout(self.base, x, self.store, xs)
        (x / "story").write_bytes(b"winner")
        _, winner = w.commit(xs)
        (self.work / "story").write_bytes(b"conflicting frozen bytes")
        with self.assertRaises(Conflict): w.commit(self.state)
        old = w.status(self.state)["pending_operation"]
        (self.work / "story").write_bytes(b"later not included")
        fork, node = w.fork_pending(self.state, old)
        self.assertNotEqual(node.branch, winner.branch)
        self.assertEqual(node.parent, self.marker.operation)
        self.assertEqual(w.fork_pending(self.state, old), (fork, node))
        with zipfile.ZipFile(fork) as z: self.assertEqual(z.read("story"), b"conflicting frozen bytes")
        self.assertTrue((self.state / (old + ".zxs")).is_file())
        self.assertEqual((self.work / "story").read_bytes(), b"later not included")

    def test_two_real_publishers_competing_for_one_slot(self):
        from concurrent.futures import ThreadPoolExecutor
        from threading import Barrier
        from zipworkspace import publish as media
        x, xs = self.root / "x", self.root / "xs"
        w.checkout(self.base, x, self.store, xs)
        (x / "story").write_bytes(b"other")
        (self.work / "story").write_bytes(b"mine")
        barrier = Barrier(2)
        primitive = media.rename_new
        def race(src, dst):
            barrier.wait(timeout=10); primitive(src, dst)
        def one(state):
            try: return w.commit(state)[1], None
            except Conflict: return None, w.status(state)["pending_operation"]
        with patch.object(media, "rename_new", side_effect=race), ThreadPoolExecutor(2) as pool:
            results = list(pool.map(one, [self.state, xs]))
        self.assertEqual(sum(node is not None for node, _ in results), 1)
        self.assertEqual(sum(conflict is not None for _, conflict in results), 1)
        self.assertEqual(len(list(self.store.glob("*.zxs"))), 2)

    def test_packaging_reads_previously_frozen_generation_only(self):
        implementation = w.pack
        def edit_after_freeze(frozen, manifest, marker, local):
            (self.work / "story").write_bytes(b"later editor")
            return implementation(frozen, manifest, marker, local)
        (self.work / "story").write_bytes(b"stable capture")
        with patch.object(w, "pack", side_effect=edit_after_freeze):
            path, _ = w.commit(self.state)
        with zipfile.ZipFile(path) as z: self.assertEqual(z.read("story"), b"stable capture")
        self.assertEqual((self.work / "story").read_bytes(), b"later editor")


class ZIP64(unittest.TestCase):
    def test_standard_zip64_footer_at_real_65536_member_boundary(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "many.zxs"
            tree = Manifest()
            empty = hashlib.sha256(b"").hexdigest()
            with zipfile.ZipFile(p, "w", compression=zipfile.ZIP_STORED, allowZip64=True) as z:
                for n in range(65536):
                    name = f"n{n:05d}"
                    z.writestr(name, b"")
                    tree.add(name, False, 0, empty)
                marker = Commit(new_id(), new_id(), 1, new_id(), None, tree.digest())
                z.comment = marker.encode()
            with p.open("rb") as f:
                f.seek(-1024, 2); self.assertIn(b"PK\x06\x06", f.read())
            observed, view = read_archive(p)
            self.assertEqual(observed, marker)
            self.assertEqual(len(view.rows), 65536)
            subprocess.run(["unzip", "-tqq", str(p)], check=True)
