# Deliberately no import or execution of Samba. Only owned temporary fixtures
# and the guest's non-authorizing predicates; these are not an interoperability test.
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

path = Path(__file__).resolve().parents[2] / "resources" / "samba-domain" / "fresh_guest.py"
spec = importlib.util.spec_from_file_location("private_sam_guest_source", path)
guest = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guest)


class FreshOnly(unittest.TestCase):
    def test_old_private_name_cannot_be_adopted_or_removed(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = os.open(tmp, os.O_DIRECTORY)
            try:
                self.assertTrue(guest._is_just(d, ()))
                for foreign in ("MACHINE.SID", "secrets.tdb", "other"):
                    p = Path(tmp, foreign)
                    p.write_bytes(b"keep")
                    self.assertFalse(guest._is_just(d, ()))
                    self.assertEqual(p.read_bytes(), b"keep")
                    p.unlink()
                    self.assertTrue(guest._is_just(d, ()))
            finally:
                os.close(d)

    def test_store_name_must_still_select_the_native_original_after_growth(self):
        with tempfile.TemporaryDirectory() as tmp:
            orig = Path(tmp, "secrets.tdb")
            orig.touch(mode=0o600)
            d = os.open(tmp, os.O_DIRECTORY)
            try:
                dev, uid = os.fstat(d).st_dev, os.geteuid()
                held = guest._store_at(d, uid, dev)
                # The real Samba getter adds material; do not require the
                # initial zero length/timestamps to stay unchanged.
                with orig.open("ab") as stream:
                    stream.write(b"new material")
                self.assertEqual(guest._store_at(d, uid, dev, held), held)
                orig.rename(Path(tmp, "old"))
                orig.touch(mode=0o600)
                orig.write_bytes(b"new material")
                self.assertNotEqual(guest._store_at(d, uid, dev), held)
                with self.assertRaises(guest.Refused):
                    guest._store_at(d, uid, dev, held)
            finally:
                os.close(d)

    def test_requires_typed_local_domain_shape_not_the_workgroups_user_sid(self):
        self.assertEqual(guest._domain("S-1-5-21-1-2-3").hex(), "010400000000000515000000010000000200000003000000")
        for value in ["S-1-5-21-0-0-0", "S-1-5-21-1-2-3-1000", "S-1-5-21-01-2-3",
                      "S-1-5-21-1-2-4294967296", "ZXSWORK", "S-1-5-21-1-2-3\n"]:
            with self.assertRaises(guest.Refused):
                guest._domain(value)
