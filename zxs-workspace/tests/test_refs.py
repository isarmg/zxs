from dataclasses import replace
from pathlib import Path
from unittest.mock import patch
import tempfile
import unittest
import zipfile
from zipworkspace import refs, worktree as w
from zipworkspace.model import Rejected, new_id


class DirectoryRefs(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.work, self.store, self.state = (self.root / s for s in ("work", "store", "state"))
        self.work.mkdir(); self.store.mkdir()
        (self.work / "ordinary").write_text("old")
        w.enroll(self.work, self.store, self.state)
        self.oldfile, self.old = w.commit(self.state)
        (self.work / "ordinary").write_text("new")
        self.newfile, self.new = w.commit(self.state)

    def test_head_and_historical_checkout_have_independent_stable_selection(self):
        cat = refs.Catalog(self.oldfile, self.store)
        tip = cat.select_branch(self.old.branch)
        historic = cat.select_node(self.old.operation)
        self.assertEqual(tip.marker, self.new)
        self.assertEqual(historic.marker, self.old)
        # A later whole-directory commit never changes the already selected node.
        w.commit(self.state)
        for base, selected, expected in (("hist", historic, "old"), ("head", tip, "new")):
            refs.checkout(selected, self.root / base, self.store, self.root / (base+"-state"))
            self.assertEqual((self.root / base / "ordinary").read_text(), expected)

    def test_partial_store_does_not_announce_unverified_branch_head(self):
        loose = self.root / "loose.zxs"; self.oldfile.rename(loose)
        cat = refs.Catalog(loose, self.store)
        row = cat.branches()[0]
        self.assertEqual(row["missing_parent"], self.old.operation)
        self.assertIsNone(row["complete_observed_head"])
        with self.assertRaises(Rejected): cat.select_branch(self.old.branch)
        self.assertEqual(cat.select_node(self.new.operation).marker, self.new)
        self.assertEqual(refs.Catalog(self.newfile, self.store).history(self.new.operation)[1], self.old.operation)

    def test_detects_conflicting_slot_parent_and_independent_genesis(self):
        with zipfile.ZipFile(self.newfile, "a") as z: z.comment=replace(self.new, parent=new_id()).encode()
        with self.assertRaises(Rejected): refs.Catalog(self.oldfile, self.store)
        self.newfile.unlink()
        fake = replace(self.old, branch=new_id(), operation=new_id())
        p = self.store / fake.filename; p.write_bytes(self.oldfile.read_bytes())
        with zipfile.ZipFile(p, "a") as z: z.comment=fake.encode()
        with self.assertRaises(Rejected): refs.Catalog(self.oldfile, self.store)

    def test_exact_selection_rejects_same_named_replacement_before_registration(self):
        node = refs.Catalog(self.oldfile, self.store).select_node(self.old.operation)
        self.oldfile.write_bytes(self.newfile.read_bytes())
        with self.assertRaises(Rejected):
            refs.checkout(node, self.root/"other", self.store, self.root/"other-state")
        self.assertFalse((self.root/"other").exists())
        self.assertFalse((self.root/"other-state").exists())

    def test_changed_original_during_private_extraction_never_exposes_wrong_destination(self):
        selected = refs.Catalog(self.oldfile, self.store).select_node(self.old.operation)
        from zipworkspace import opening
        original = opening.read_archive
        def swap_after_read(archive, **args):
            result = original(archive, **args)
            self.oldfile.write_bytes(self.newfile.read_bytes())
            return result
        with patch.object(opening, "read_archive", side_effect=swap_after_read):
            with self.assertRaises(Rejected):
                refs.checkout(selected, self.root/"no-destination", self.store, self.root/"no-state")
        self.assertFalse((self.root/"no-destination").exists())
        self.assertFalse((self.root/"no-state").exists())
