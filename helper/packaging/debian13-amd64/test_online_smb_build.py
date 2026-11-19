"""Source fixtures for the future nonroot release worker; not run during static review."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import online_sources as sources
import online_objects as objects

def h(b):return hashlib.sha256(b).hexdigest()

class OnlineSources(unittest.TestCase):
    def test_immutable_project_not_worktree(self):
        with tempfile.TemporaryDirectory() as d:
            base=Path(d);repo=base/"store";repo.mkdir();(repo/"Cargo.toml").write_bytes(b"untracked replacement")
            actual=b"authenticated"
            oid=hashlib.sha1(b"blob 13\x00"+actual,usedforsecurity=False).hexdigest()
            tree="1"*40;record=f"100644 13 {h(actual)} Cargo.toml\n".encode("ascii")
            lock=dict(project_commit="0"*40,project_tree=tree,project_files=[1,13,h(record)])
            def git(argv,*_,**__):
                if "rev-parse" in argv:return (tree+"\n").encode()
                if "ls-tree" in argv:return f"100644 blob {oid}    13\tCargo.toml\0".encode()
                self.assertEqual(argv[-3:],["cat-file","blob",oid]);return actual
            with patch.object(sources,"command",git):
                out=base/"original";sources.git_export(repo,out,lock,{})
                self.assertEqual((out/"Cargo.toml").read_bytes(),actual)
                lock["project_files"]=[1,13,"e"*64]
                with self.assertRaises(ValueError):sources.git_export(repo,base/"wrong",lock,{})
    def test_overlay_is_not_replacement_or_link(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);(root/"source3").mkdir();dest=root/"source3/entry.c"
            self.assertEqual(sources.member(root,"source3/entry.c",fresh=True),dest)
            dest.write_bytes(b"already owned")
            with self.assertRaises(ValueError):sources.member(root,"source3/entry.c",fresh=True)
            (root/"alias").symlink_to(root/"source3",target_is_directory=True)
            with self.assertRaises(ValueError):sources.member(root,"alias/new.c",fresh=True)
            for n in ("../tmp/x","a//b","/abs","a/.git/b"):
                with self.assertRaises(ValueError):sources.path_name(n)
            row="f"*64+"  source3/entry.c\n"
            with self.assertRaises(ValueError):sources.originals(row+row)
    def test_no_fallback_when_rebuilt_object_wrong_for_requester(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);public,private=objects.DIRECTORIES
            actual=root/private/"libmanaged-samba4.so.0.1";actual.parent.mkdir(parents=True);actual.write_bytes(b"synthetic")
            (actual.parent/"libmanaged-samba4.so.0").symlink_to(actual.name)
            with self.assertRaises(ValueError):objects.needed_source(root,[public],"libmanaged-samba4.so.0")
            self.assertEqual(objects.needed_source(root,[private,public],"libmanaged-samba4.so.0"),
                             (private+"/libmanaged-samba4.so.0",actual))
            self.assertIsNone(objects.needed_source(root,[public],"libother.so.2"))
            with self.assertRaises(ValueError):objects.needed_source(root,[public,private],"libmissing-samba.so.0")
            (actual.parent/"liboutside.so.0").symlink_to("/usr/lib/liboutside.so.0")
            with self.assertRaises(ValueError):objects.staged(root,private+"/liboutside.so.0")
