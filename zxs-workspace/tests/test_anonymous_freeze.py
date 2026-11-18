"""Fresh Linux freeze capability, actual source/extent/candidate and failure boundaries."""
from contextlib import ExitStack
from hashlib import sha256
from pathlib import Path
import os
import sqlite3
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import freeze, private_freeze as private, worktree
from zipworkspace.model import Commit, Manifest, Rejected


class AnonymousFreeze(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory(); self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name); self.work=self.root/"work"; self.work.mkdir()
        self.store=self.root/"store"; self.store.mkdir()
        self.state=self.root/"state"; self.state.mkdir(mode=0o700)

    def admitted(self, stack):
        try: return stack.enter_context(private.anonymous_capture(self.state))
        except (Rejected, OSError) as error: self.skipTest("requires actual qualified anonymous STATE: "+str(error))

    def test_unsupported_capability_refuses_before_sqlite_or_source_and_never_uses_a_named_scratch(self):
        with patch.object(private,"local_mount",side_effect=Rejected("volume not qualified")), \
             patch.object(sqlite3,"connect",side_effect=AssertionError("cannot open or recover SQLite")), \
             patch.object(worktree,"frozen_tree",side_effect=AssertionError("cannot scan a worktree")), \
             patch.object(tempfile,"mkdtemp",side_effect=AssertionError("no named fallback")):
            with self.assertRaises((Rejected,OSError)): worktree.commit(self.state)
        self.assertEqual(list(self.state.iterdir()),[])

    def test_actual_held_spans_cover_short_empty_and_unicode_files_and_not_later_live_edits(self):
        (self.work/"sub").mkdir(); (self.work/"empty").mkdir()
        (self.work/"zero").write_bytes(b""); (self.work/"sub/小").write_bytes(b"original")
        original = self.work.stat()
        with ExitStack() as stack:
            capture=self.admitted(stack); fd=capture.body
            self.assertEqual(os.fstat(fd).st_nlink,0)
            self.assertEqual(os.fstat(fd).st_dev,self.state.stat().st_dev)
            with freeze.frozen_tree(self.work,capture,(original.st_dev,original.st_ino),self.store) as (frozen,tree):
                self.assertEqual(list(self.state.iterdir()),[])
                self.assertEqual(capture.length,len(b"original"))
                (self.work/"sub/小").write_bytes(b"later")
                marker=Commit("1"*32,"2"*32,1,"3"*32,None,tree.digest())
                with capture.candidate(marker.operation) as (name,owned,out):
                    freeze.pack(frozen,tree,marker,out); out.close()
                    digest=capture.verify_candidate(name,owned,marker)
                result=self.state/name
                self.assertEqual(digest,sha256(result.read_bytes()).hexdigest())
                with zipfile.ZipFile(result) as z:
                    self.assertEqual(z.read("sub/小"),b"original"); self.assertEqual(z.read("zero"),b"")
                    self.assertIn("empty/",z.namelist())
        with self.assertRaises(OSError): os.fstat(fd)
        self.assertEqual(list(self.state.iterdir()),[result])

    def test_rebound_private_root_is_never_written_or_recursively_cleaned_on_success_or_exception(self):
        for index,fail in enumerate((False,True)):
            saved=self.root/f"original-{index}"
            def leave():
                with ExitStack() as stack:
                    capture=self.admitted(stack)
                    capture.append(b"privately captured")
                    self.state.rename(saved); self.state.mkdir(mode=0o700)
                    evidence=self.state/"freeze-not-ours"; evidence.mkdir(); (evidence/"keep").write_bytes(b"unrelated")
                    with self.assertRaises(Rejected):
                        with capture.candidate("4"*32): self.fail("wrong root admitted")
                    if fail: raise RuntimeError("simulated capture interruption")
            if fail:
                with self.assertRaisesRegex(RuntimeError,"capture interruption"): leave()
            else: leave()
            self.assertEqual((self.state/"freeze-not-ours/keep").read_bytes(),b"unrelated")
            self.assertEqual(list(saved.iterdir()),[])
            # A new independently selected private root for the second subcase.
            self.state=self.root/f"new-state-{index}"; self.state.mkdir(mode=0o700)

    def test_replaced_registered_worktree_and_replaced_closed_candidate_are_rejected(self):
        original=self.work.stat()
        self.work.rename(self.root/"old-work"); self.work.mkdir()
        with ExitStack() as stack:
            capture=self.admitted(stack)
            with self.assertRaisesRegex(Rejected,"registered worktree"):
                with freeze.frozen_tree(self.work,capture,(original.st_dev,original.st_ino),self.store): pass
            self.assertEqual(capture.length,0)
            tree=Manifest(); marker=Commit("1"*32,"2"*32,1,"5"*32,None,tree.digest())
            with capture.candidate(marker.operation) as (name,owned,out):
                freeze.pack(capture,tree,marker,out); out.close()
                old=self.state/"retained"; (self.state/name).rename(old)
                (self.state/name).write_bytes(old.read_bytes())
                with self.assertRaisesRegex(Rejected,"different private ZIP"): capture.verify_candidate(name,owned,marker)
                self.assertEqual(old.read_bytes(),(self.state/name).read_bytes())

    def test_name_replaced_during_real_parent_sync_and_extra_link_never_complete(self):
        with ExitStack() as stack:
            capture=self.admitted(stack)
            tree=Manifest()
            for op,link in (("6",False),("7",True)):
                marker=Commit("1"*32,"2"*32,1,op*32,None,tree.digest())
                with capture.candidate(marker.operation) as (name,owned,out):
                    freeze.pack(capture,tree,marker,out); out.close()
                    named=self.state/name; retained=self.state/(op+"-retained")
                    original=owned
                    sync=os.fsync
                    def change_on_sync(fd):
                        sync(fd)
                        if fd==capture.parent:
                            if link: os.link(named,retained)
                            else: named.rename(retained); named.write_bytes(retained.read_bytes())
                    with patch.object(os,"fsync",side_effect=change_on_sync), self.assertRaises(Rejected):
                        capture.verify_candidate(name,original,marker)
                    self.assertEqual(named.read_bytes(),retained.read_bytes())
                    self.assertEqual(os.path.samestat(named.stat(),retained.stat()),link)

    def test_named_partial_error_preserves_exact_operation_note_without_claiming_a_pending_receipt(self):
        with ExitStack() as stack:
            capture=self.admitted(stack)
            operation="8"*32
            with self.assertRaises(OSError) as failed:
                with capture.candidate(operation) as (_,__,out):
                    out.write(b"partial"); raise OSError("simulated interrupted private ZIP")
            path=self.state/(operation+".zxs")
            self.assertEqual(path.read_bytes(),b"partial")
            self.assertIn(str(path),failed.exception.__notes__[0])

    def test_short_writes_interruption_and_truncation_cannot_cross_extent_boundaries(self):
        with ExitStack() as stack:
            capture=self.admitted(stack)
            write=os.write; interrupted=[False]
            def short(fd,block):
                if not interrupted[0]: interrupted[0]=True; raise InterruptedError()
                return write(fd,block[:2])
            with patch.object(os,"write",side_effect=short): capture.append(b"12345")
            capture.spans={"left":(0,2),"right":(2,3)}
            tree=Manifest()
            for name,body in (("left",b"12"),("right",b"345")):
                tree.add(name,False,len(body),sha256(body).hexdigest())
            capture.validate(tree)
            self.assertEqual(b"".join(capture.blocks("right",3)),b"345")
            capture.spans["right"]=(1,3)
            with self.assertRaises(Rejected): capture.validate(tree)
            capture.spans["right"]=(2,3)
            os.ftruncate(capture.body,3)
            with self.assertRaises(Rejected): capture.validate(tree)
            with self.assertRaises(Rejected): list(capture.blocks("right",3))
