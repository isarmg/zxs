"""Source-only release regression cases. These never invoke tools or open a device."""
import hashlib
import io
import os
from datetime import datetime, timezone
from pathlib import Path
import tempfile
import subprocess
import tarfile
import unittest
from archive_inputs import copy_checked, hashes, paragraphs, relative, release_time


class Inputs(unittest.TestCase):
    def test_release_history_needs_an_explicit_first_or_a_real_previous_version(self):
        from build import release_history
        cur="1.0.0+diskpack20261119.2";prev="1.0.0+diskpack20261119.1"
        release_history(cur,True,None);release_history(cur,False,prev)
        for first,previous in [(False,None),(False,""),(False,"nightly"),(True,prev),(True,"")]:
            with self.assertRaises(ValueError): release_history(cur,first,previous)

    def test_single_bsd_notice_requires_the_named_file_not_a_dual_license_substitute(self):
        from build import rust_notices
        from unittest.mock import patch
        for member, accepted in (("LICENSE", True), ("LICENSE-MIT", False)):
            with self.subTest(member=member), tempfile.TemporaryDirectory() as d:
                root=Path(d); crate="subtle-2.6.1"; archive=root/"source/rust"/(crate+".crate")
                archive.parent.mkdir(parents=True)
                (root/"Cargo.lock").write_text('[[package]]\nname="subtle"\nversion="2.6.1"\nsource="registry+fixture"\n')
                fixture=b"synthetic notice; not upstream license text\n"
                with tarfile.open(archive,"w:gz") as tar:
                    entry=tarfile.TarInfo(crate+"/"+member);entry.size=len(fixture)
                    tar.addfile(entry,io.BytesIO(fixture))
                with patch("build.PROJECT",root):
                    if accepted:
                        rust_notices(root,root/"docs")
                        self.assertEqual((root/"docs/rust-notices"/crate/"LICENSE").read_bytes(),fixture)
                    else:
                        with self.assertRaises(ValueError): rust_notices(root,root/"docs")

    def test_source_archive_exact_for_repo_root_or_nested_helper(self):
        from build import archive_project
        for nested in (False,True):
            with self.subTest(nested=nested), tempfile.TemporaryDirectory() as d:
                root=Path(d); repo=root/"repo"; repo.mkdir()
                def git(*args):
                    return subprocess.check_output(["/usr/bin/git","-c","user.name=fixture","-c",
                        "user.email=fixture@example.invalid",*args],cwd=repo)
                git("init","-q")
                project=(repo/"helper") if nested else repo
                src=project/"packaging/debian13-amd64/build.py"; src.parent.mkdir(parents=True)
                src.write_bytes(b"source only\n"); (project/"Cargo.toml").write_bytes(b"[package]\n")
                if nested: (repo/"unrelated").write_text("not part of the project")
                git("add","--","."); git("commit","-qm","synthetic")
                archive=root/"helper.tar"
                archive_project(project,archive,git("rev-parse","HEAD").decode().strip())
                with tarfile.open(archive) as tar:
                    self.assertEqual({m.name for m in tar if m.isfile()},
                        {"helper/packaging/debian13-amd64/build.py","helper/Cargo.toml"})
                    self.assertEqual(tar.extractfile("helper/packaging/debian13-amd64/build.py").read(),b"source only\n")

    def test_indexes_require_unique_fields_and_safe_hash_keys(self):
        self.assertEqual(list(paragraphs(io.StringIO("Package: sample\nVersion: 1\n\n"))),
                         [{"Package":"sample","Version":"1"}])
        with self.assertRaises(ValueError):
            list(paragraphs(["Package: a\n","Package: b\n"]))
        digest = "ab"*32
        for name in ["../outside", "/absolute", "pool/../outside", "pool//b", "./entry"]:
            with self.assertRaises(ValueError):
                relative(name)
        self.assertEqual(hashes(f"{digest} 3 pool/payload"),{Path("pool/payload"):(digest,3)})
        with self.assertRaises(ValueError):
            hashes(f"{digest} 3 pool/payload\n{digest} 3 pool/payload")

    def test_signed_security_metadata_expires_instead_of_silent_fallback(self):
        row = {"Date":"Sun, 01 Nov 2026 00:00:00 UTC","Valid-Until":"Tue, 03 Nov 2026 00:00:00 UTC"}
        dt = lambda d: datetime(2026,11,d,tzinfo=timezone.utc)
        release_time(row, True, dt(2))
        with self.assertRaises(ValueError): release_time(row, True, dt(3))
        with self.assertRaises(KeyError): release_time({"Date":row["Date"]}, True, dt(2))

    def test_budget_and_fifo_fail_before_output_creation(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d); fifo=root/"pipe"; os.mkfifo(fifo)
            src=root/"source"; src.write_bytes(b"abcdef")
            for f,size,cap in ((fifo,None,8),(src,9,8),(src,None,4),(src,5,8)):
                with self.assertRaises(ValueError):
                    copy_checked(f,root/"out","11"*32,size,cap=cap)
                self.assertFalse((root/"out").exists())

    def test_equal_names_do_not_overwrite_or_certify_different_material(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d); src=root/"upstream"; dest=root/"distribution"/"sample"
            src.write_bytes(b"abc")
            h=hashlib.sha256(b"abc").hexdigest()
            self.assertEqual(copy_checked(src,dest,h,3),dest)
            dest.write_bytes(b"bad")
            with self.assertRaises(ValueError): copy_checked(src,dest,h,3)
            self.assertEqual(dest.read_bytes(),b"bad")

if __name__=="__main__":
    unittest.main()
