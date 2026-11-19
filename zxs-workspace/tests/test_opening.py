from concurrent.futures import ThreadPoolExecutor
from threading import Barrier
from pathlib import Path
import os
import stat
import tempfile
import unittest
from unittest.mock import patch
from zipworkspace import opening, lifecycle, worktree as w
from zipworkspace.model import Rejected, new_id


class OriginalCheckout(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.work, self.store, self.state, self.parent = (self.root/n for n in ("source","store","source-state","checkouts"))
        self.work.mkdir(); self.store.mkdir(); self.parent.mkdir()
        (self.work/"ordinary").write_bytes(b"selected original")
        w.enroll(self.work,self.store,self.state)
        self.archive,self.marker = w.commit(self.state)

    def create(self,state, *, automatic=True):
        return opening.begin(self.archive,self.parent,"Project",self.store,state,automatic=automatic)

    def test_copy_verification_reads_the_exclusive_output_not_a_rebound_name(self):
        verify = opening.read_managed_document
        seen = []
        def replace_then_read(doc, *, display_name):
            name = next(self.root.glob(".zip-opening-*/original.zxs"))
            old = name.with_name("displaced")
            name.rename(old); name.write_bytes(b"unrelated")
            self.assertTrue(os.path.samestat(os.fstat(doc.fileno()), old.stat()))
            result = verify(doc, display_name=display_name); seen.append(result[0])
            return result
        with patch.object(opening, "read_managed_document", side_effect=replace_then_read):
            with self.assertRaises(Rejected): self.create(self.root/"unregistered", automatic=False)
        self.assertEqual(seen, [self.marker])
        name = next(self.root.glob(".zip-opening-*/original.zxs"))
        self.assertEqual(name.read_bytes(), b"unrelated")
        self.assertEqual(name.with_name("displaced").read_bytes(), self.archive.read_bytes())
        self.assertFalse((name.parent/"opening.sqlite").exists())
        self.assertFalse((self.root/"unregistered").exists()); self.assertFalse((self.parent/"Project").exists())

    def test_equal_byte_replacement_at_original_copy_directory_sync_prevents_registration(self):
        sync = os.fsync
        def replace_on_directory(fd):
            sync(fd)
            if stat.S_ISDIR(os.fstat(fd).st_mode):
                name = next(self.root.glob(".zip-opening-*/original.zxs"))
                self.assertTrue(os.path.samestat(os.fstat(fd), name.parent.stat()))
                old = name.with_name("displaced")
                name.rename(old); name.write_bytes(old.read_bytes())
        with patch.object(opening.os, "fsync", side_effect=replace_on_directory):
            with self.assertRaises(Rejected): self.create(self.root/"missing-state")
        name = next(self.root.glob(".zip-opening-*/original.zxs"))
        self.assertNotEqual(name.stat().st_ino, name.with_name("displaced").stat().st_ino)
        self.assertFalse((name.parent/"opening.sqlite").exists()); self.assertFalse((self.parent/"Project").exists())

    def test_existing_private_leaf_is_never_overwritten_or_cleaned_as_scratch(self):
        make = tempfile.mkdtemp
        def occupied(**kwargs):
            seed = Path(make(**kwargs)); (seed/"original.zxs").write_bytes(b"preexisting")
            return str(seed)
        with patch.object(opening.tempfile, "mkdtemp", side_effect=occupied):
            with self.assertRaises(FileExistsError): self.create(self.root/"not-created")
        name = next(self.root.glob(".zip-opening-*/original.zxs"))
        self.assertEqual(name.read_bytes(), b"preexisting")
        self.assertFalse((name.parent/"opening.sqlite").exists()); self.assertFalse((self.root/"not-created").exists())

    def test_real_name_collisions_allocate_independent_original_identities_and_do_not_delete(self):
        existing = self.parent/"Project"; existing.mkdir(); (existing/"foreign").write_text("keep")
        a, b = self.root/"a",self.root/"b"
        p, marker = self.create(a); q,_ = self.create(b)
        self.assertEqual(p.name,"Project (2)"); self.assertEqual(q.name,"Project (3)")
        self.assertNotEqual(w.status(a)["worktree"],w.status(b)["worktree"])
        self.assertEqual((existing/"foreign").read_text(),"keep")
        old=opening.status(a)["operation"]
        lifecycle.start(a,"hide")
        self.assertEqual(opening.finish(a,old),(p,marker))
        self.assertFalse(p.exists()) # Original retry never reopens a later hidden checkout.
        self.assertEqual(opening.status(a)["recorded_stage"],"attached")

    def test_actual_lost_rename_reply_attaches_exact_original_no_second_rename_or_source_reselection(self):
        state=self.root/"state"
        native=opening.rename_new_at
        def lost(fd, src, dst, *, expected_source):
            native(fd, src, dst, expected_source=expected_source)
            if dst == "Project": raise OSError("disconnect after original rename")
        with patch.object(opening,"rename_new_at",side_effect=lost):
            with self.assertRaises(OSError): self.create(state,automatic=False)
        progress=opening.status(state); self.assertEqual(progress["recorded_stage"],"verified_stage")
        wanted=self.parent/"Project"; ino=wanted.stat().st_ino
        self.archive.rename(self.root/"offline-original.zxs")
        self.assertFalse((state/"worktree.sqlite").exists())
        with patch.object(opening,"rename_new_at",side_effect=AssertionError("no second rename")):
            path,node=opening.finish(state,progress["operation"])
        self.assertEqual(node,self.marker); self.assertEqual(path.stat().st_ino,ino)
        self.assertEqual((path/"ordinary").read_bytes(),b"selected original")
        self.assertEqual(w.status(state)["worktree"],progress["worktree"])

    def test_unknown_dirty_or_same_bytes_rebound_stages_are_never_adopted(self):
        state=self.root/"pending"; native=opening.rename_new_at
        def stop(fd, src, dst, *, expected_source):
            if dst == "Project": raise OSError("before original rename")
            native(fd, src, dst, expected_source=expected_source)
        with patch.object(opening,"rename_new_at",side_effect=stop):
            with self.assertRaises(OSError): self.create(state,automatic=False)
        p=opening.status(state); stage=next(self.parent.glob(".zip-checkout-*"))
        with self.assertRaises(Rejected): opening.finish(state,new_id())
        (stage/"ordinary").write_bytes(b"not the original")
        with self.assertRaises(Rejected): opening.finish(state,p["operation"])
        stage.rename(self.root/"displaced"); stage.mkdir(); (stage/"ordinary").write_bytes(b"selected original")
        with self.assertRaises(Rejected): opening.finish(state,p["operation"])
        self.assertFalse((self.parent/"Project").exists()); self.assertFalse((state/"worktree.sqlite").exists())

    def test_occupied_name_budget_does_not_wrap_or_overwrite(self):
        for n in range(1,5):
            d=self.parent/("Project" if n==1 else f"Project ({n})")
            d.mkdir(); (d/"old").write_text(str(n))
        state=self.root/"busy"
        with patch.object(opening,"MAX_AUTO",4):
            with self.assertRaises(Rejected): self.create(state)
            with self.assertRaises(Rejected): opening.finish(state,opening.status(state)["operation"])
        self.assertEqual((self.parent/"Project (4)"/"old").read_text(),"4")
        self.assertFalse((state/"worktree.sqlite").exists())

    def test_catalog_survives_lost_final_record_only_exact_current_directory_can_close_it(self):
        state=self.root/"original"
        with patch.object(opening,"_catalog_matches",side_effect=OSError("after installation before receipt")):
            with self.assertRaises(OSError): self.create(state)
        progress=opening.status(state)
        self.assertTrue((state/"worktree.sqlite").exists())
        dest=self.parent/"Project"
        dest.rename(self.root/"original-dir"); dest.mkdir(); (dest/"ordinary").write_bytes(b"selected original")
        with self.assertRaises(Rejected): opening.finish(state,progress["operation"])
        dest.rename(self.root/"unrelated"); (self.root/"original-dir").rename(dest)
        self.assertEqual(opening.finish(state,progress["operation"]),(dest,self.marker))

    def test_rebound_named_parent_at_native_rename_never_publishes_into_its_replacement(self):
        state, moved, stranger = (self.root / n for n in ("state", "original-parent", "replacement-parent"))
        native = opening.rename_new_at
        def switch_parent(fd, src, dst, *, expected_source):
            self.assertTrue(os.path.samestat(os.fstat(fd), self.parent.stat()))
            self.parent.rename(moved); self.parent.mkdir()
            other = self.parent / src; other.mkdir(); (other / "keep").write_bytes(b"untouched")
            native(fd, src, dst, expected_source=expected_source)
        with patch.object(opening, "rename_new_at", side_effect=switch_parent):
            with self.assertRaisesRegex(Rejected, "parent"): self.create(state, automatic=False)
        self.assertEqual((moved / "Project" / "ordinary").read_bytes(), b"selected original")
        self.assertFalse((self.parent / "Project").exists())
        self.assertEqual(next(self.parent.glob(".zip-checkout-*/keep")).read_bytes(), b"untouched")
        self.assertFalse((state / "worktree.sqlite").exists())
        self.parent.rename(stranger); moved.rename(self.parent)
        operation = opening.status(state)["operation"]
        with patch.object(opening, "rename_new_at", side_effect=AssertionError("original was already moved")):
            dest, node = opening.finish(state, operation)
        self.assertEqual((dest, node), (self.parent / "Project", self.marker))

    def test_failed_sync_of_held_target_parent_keeps_original_stage_receipt(self):
        state = self.root / "state"; renamed = []
        native, sync = opening.rename_new_at, os.fsync
        def move(fd, src, dst, *, expected_source):
            native(fd, src, dst, expected_source=expected_source)
            renamed.append(os.fstat(fd))
        def fail_after_move(fd):
            if renamed and os.path.samestat(os.fstat(fd), renamed[0]):
                raise OSError("target parent sync refused")
            return sync(fd)
        with patch.object(opening, "rename_new_at", side_effect=move), patch.object(opening.os, "fsync", side_effect=fail_after_move):
            with self.assertRaisesRegex(OSError, "target parent sync refused"): self.create(state)
        status = opening.status(state)
        self.assertEqual(status["recorded_stage"], "verified_stage")
        self.assertFalse((state / "worktree.sqlite").exists())
        with patch.object(opening, "rename_new_at", side_effect=AssertionError("do not republish")):
            self.assertEqual(opening.finish(state, status["operation"]), (self.parent / "Project", self.marker))

    def test_replacement_during_catalog_creation_cannot_be_registered_as_the_original(self):
        state, saved = self.root / "state", self.root / "actual-instance"
        dest = self.parent / "Project"; install = lifecycle.install
        def swap(con, root, *, original):
            root.rename(saved); root.mkdir(); (root / "ordinary").write_bytes(b"stranger")
            self.assertTrue(os.path.samestat(original, saved.stat()))
            install(con, root, original=original)
            self.assertEqual(con.execute("SELECT device,inode FROM instance").fetchone(),
                             (str(saved.stat().st_dev), str(saved.stat().st_ino)))
        with patch.object(lifecycle, "install", side_effect=swap):
            with self.assertRaises(Rejected): self.create(state)
        self.assertFalse((state / "worktree.sqlite").exists())
        self.assertEqual((dest / "ordinary").read_bytes(), b"stranger")
        self.assertEqual(opening.status(state)["recorded_stage"], "verified_stage")
        dest.rename(self.root / "stranger"); saved.rename(dest)
        self.assertEqual(opening.finish(state, opening.status(state)["operation"]), (dest, self.marker))

    def test_permission_error_at_relative_target_is_not_a_free_name(self):
        state, actual = self.root / "state", opening.who
        def observe(name, *, dir_fd=None):
            if name == "Project" and dir_fd is not None: raise PermissionError("target observation denied")
            return actual(name, dir_fd=dir_fd)
        with patch.object(opening, "who", side_effect=observe), \
             patch.object(opening, "rename_new_at", side_effect=AssertionError("no rename on denial")):
            with self.assertRaisesRegex(PermissionError, "observation denied"): self.create(state)
        self.assertFalse((self.parent / "Project").exists())
        self.assertFalse((state / "worktree.sqlite").exists())
        self.assertEqual(opening.status(state)["recorded_stage"], "verified_stage")

    def test_existing_attached_original_can_be_hidden_before_lost_opening_reply_is_observed(self):
        state = self.root / "state"; dest = self.parent / "Project"
        with patch.object(opening, "_catalog_matches", side_effect=OSError("registration already installed")):
            with self.assertRaisesRegex(OSError, "already installed"): self.create(state)
        self.assertTrue((state / "worktree.sqlite").is_file())
        operation = opening.status(state)["operation"]
        lifecycle.start(state, "hide")
        with patch.object(opening, "rename_new_at", side_effect=AssertionError("never show it again")):
            self.assertEqual(opening.finish(state, operation), (dest, self.marker))
        self.assertFalse(dest.exists())
        self.assertEqual(opening.status(state)["recorded_stage"], "attached")

    def test_attached_original_retries_lost_catalog_directory_sync_before_final_receipt(self):
        state = self.root / "state"; catalog = state / "worktree.sqlite"; dest = self.parent / "Project"
        real_dirsync, sync = w.sync_dir, os.fsync
        def fail_catalog_dir(directory):
            if directory == state:
                self.assertTrue(catalog.is_file())
                raise OSError("lost first STATE dirsync")
            return real_dirsync(directory)
        with patch.object(w, "sync_dir", side_effect=fail_catalog_dir):
            with self.assertRaisesRegex(OSError, "first STATE dirsync"): self.create(state)
        info = opening.status(state); self.assertEqual(info["recorded_stage"], "verified_stage")
        original = dest.stat().st_ino, catalog.stat().st_ino
        def still_unavailable(fd):
            if os.path.samestat(os.fstat(fd), state.stat()): raise OSError("original STATE still cannot sync")
            return sync(fd)
        with patch.object(opening, "rename_new_at", side_effect=AssertionError("original already moved")), \
             patch.object(w, "create_catalog", side_effect=AssertionError("original already attached")):
            with patch.object(opening.os, "fsync", side_effect=still_unavailable):
                with self.assertRaisesRegex(OSError, "STATE still cannot sync"):
                    opening.finish(state, info["operation"])
            self.assertEqual(opening.status(state)["recorded_stage"], "verified_stage")
            self.assertEqual(opening.finish(state, info["operation"]), (dest, self.marker))
        self.assertEqual(opening.status(state)["recorded_stage"], "attached")
        self.assertEqual((dest.stat().st_ino, catalog.stat().st_ino), original)

    def test_two_concurrent_originals_choose_distinct_real_directories(self):
        a,b = self.root/"race-a", self.root/"race-b"
        native=opening.rename_new_at; rendezvous=Barrier(2)
        def race(fd, src, dst, *, expected_source):
            if src.startswith(".zip-checkout-") and dst == "Project":
                rendezvous.wait(timeout=5)
            return native(fd, src, dst, expected_source=expected_source)
        with patch.object(opening,"rename_new_at",side_effect=race), ThreadPoolExecutor(max_workers=2) as pool:
            outcomes=list(pool.map(self.create,[a,b]))
        self.assertEqual({x[0].name for x in outcomes},{"Project","Project (2)"})
        self.assertNotEqual(outcomes[0][0].stat().st_ino,outcomes[1][0].stat().st_ino)
        for state, outcome in zip((a,b),outcomes):
            self.assertEqual(opening.finish(state,opening.status(state)["operation"]),outcome)
            self.assertEqual((outcome[0]/"ordinary").read_bytes(),b"selected original")
