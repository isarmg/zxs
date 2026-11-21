"""Source-only release regression cases. These never invoke tools or open a device."""
import hashlib
import io
import os
from datetime import datetime, timezone
from pathlib import Path
import tempfile
import tarfile
import unittest
from archive_inputs import copy_checked, hashes, paragraphs, relative, release_time


class Inputs(unittest.TestCase):
    def test_tool_material_selection_admits_only_exact_offline_lock(self):
        import json
        from build import HERE,require_tool_inputs
        locked=json.loads((HERE/"lock.json").read_text())
        require_tool_inputs(locked)
        payload=next(iter(locked["payload"].values()))
        payload["usr/bin/unknown-provider"]="bin/unknown-provider"
        with self.assertRaises(ValueError): require_tool_inputs(locked)

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
