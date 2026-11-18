from concurrent.futures import ThreadPoolExecutor
from threading import Barrier
from pathlib import Path
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
        native=opening.rename_new
        def lost(src,dst):
            native(src,dst)
            if dst==self.parent/"Project": raise OSError("disconnect after original rename")
        with patch.object(opening,"rename_new",side_effect=lost):
            with self.assertRaises(OSError): self.create(state,automatic=False)
        progress=opening.status(state); self.assertEqual(progress["recorded_stage"],"verified_stage")
        wanted=self.parent/"Project"; ino=wanted.stat().st_ino
        self.archive.rename(self.root/"offline-original.zxs")
        self.assertFalse((state/"worktree.sqlite").exists())
        with patch.object(opening,"rename_new",side_effect=AssertionError("no second rename")):
            path,node=opening.finish(state,progress["operation"])
        self.assertEqual(node,self.marker); self.assertEqual(path.stat().st_ino,ino)
        self.assertEqual((path/"ordinary").read_bytes(),b"selected original")
        self.assertEqual(w.status(state)["worktree"],progress["worktree"])

    def test_unknown_dirty_or_same_bytes_rebound_stages_are_never_adopted(self):
        state=self.root/"pending"; native=opening.rename_new
        def stop(src,dst):
            if dst==self.parent/"Project": raise OSError("before original rename")
            native(src,dst)
        with patch.object(opening,"rename_new",side_effect=stop):
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

    def test_two_concurrent_originals_choose_distinct_real_directories(self):
        a,b = self.root/"race-a", self.root/"race-b"
        native=opening.rename_new; rendezvous=Barrier(2)
        def race(src,dst):
            if src.name.startswith(".zip-checkout-") and dst == self.parent/"Project":
                rendezvous.wait(timeout=5)
            return native(src,dst)
        with patch.object(opening,"rename_new",side_effect=race), ThreadPoolExecutor(max_workers=2) as pool:
            outcomes=list(pool.map(self.create,[a,b]))
        self.assertEqual({x[0].name for x in outcomes},{"Project","Project (2)"})
        self.assertNotEqual(outcomes[0][0].stat().st_ino,outcomes[1][0].stat().st_ino)
        for state, outcome in zip((a,b),outcomes):
            self.assertEqual(opening.finish(state,opening.status(state)["operation"]),outcome)
            self.assertEqual((outcome[0]/"ordinary").read_bytes(),b"selected original")
