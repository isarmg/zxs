"""The native source boundary is offline, complete and rejects unsafe archives."""
import importlib.util
import hashlib
import tempfile
from pathlib import Path
import tarfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("samba_source", ROOT / "tools/samba-source/manage.py")
SOURCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SOURCE)


class SambaSourceTests(unittest.TestCase):
    def test_official_archive_and_legal_bytes(self):
        receipt = SOURCE.verify()
        self.assertEqual(receipt["version"], "4.23.0")
        self.assertEqual(receipt["sha256"], "b0cb963a63eb1515227f3779a46d3a7f790c1bbfeeb4b31fd43e405eefe7a3af")
        self.assertEqual(receipt["local_modifications"], [])

    def test_streamed_parts_preserve_bytes_and_reject_damage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            base = root / "compliance/sources/samba"
            base.mkdir(parents=True)
            chunks = [b"first original bytes", b"last original bytes"]
            receipt = {"version": "test", "parts": [],
                       "sha256": hashlib.sha256(b"".join(chunks)).hexdigest(),
                       "archive_bytes": sum(map(len, chunks))}
            for index, chunk in enumerate(chunks):
                name = f"samba-test.tar.gz.part-{index:03d}"
                (base / name).write_bytes(chunk)
                receipt["parts"].append({"name": name, "bytes": len(chunk),
                                         "sha256": hashlib.sha256(chunk).hexdigest()})
            with SOURCE.assembled_archive(root, receipt) as archive:
                self.assertEqual(archive.read(), b"".join(chunks))
            first = base / receipt["parts"][0]["name"]
            first.write_bytes(b"corrupt")
            with self.assertRaisesRegex(ValueError, "part integrity"):
                with SOURCE.assembled_archive(root, receipt):
                    self.fail("corrupted part accepted")
            first.unlink()
            with self.assertRaises(FileNotFoundError):
                with SOURCE.assembled_archive(root, receipt):
                    self.fail("missing part accepted")
            first.write_bytes(chunks[0])
            receipt["parts"].reverse()
            with self.assertRaisesRegex(ValueError, "order/name"):
                with SOURCE.assembled_archive(root, receipt):
                    self.fail("reordered parts accepted")
            receipt["parts"].reverse()
            receipt["sha256"] = "0" * 64
            with self.assertRaisesRegex(ValueError, "archive integrity"):
                with SOURCE.assembled_archive(root, receipt):
                    self.fail("wrong complete source accepted")

    def test_internal_symlink_preserved(self):
        member = tarfile.TarInfo("samba-4.23.0/ctdb/include/common/srvid.h")
        member.type = tarfile.SYMTYPE
        member.linkname = "../../common/srvid.h"
        SOURCE.validate_members([member], "samba-4.23.0")

    def test_escaping_paths_rejected(self):
        for path in ["/tmp/escape", "samba-4.23.0/../../escape", "elsewhere/file"]:
            with self.subTest(path=path), self.assertRaises(ValueError):
                SOURCE.validate_members([tarfile.TarInfo(path)], "samba-4.23.0")

    def test_escaping_links_rejected(self):
        for kind in [tarfile.SYMTYPE, tarfile.LNKTYPE]:
            for target in ["/tmp/escape", "../../escape"]:
                member = tarfile.TarInfo("samba-4.23.0/file")
                member.type = kind
                member.linkname = target
                with self.subTest(kind=kind, target=target), self.assertRaises(ValueError):
                    SOURCE.validate_members([member], "samba-4.23.0")

    def test_device_node_rejected(self):
        member = tarfile.TarInfo("samba-4.23.0/device")
        member.type = tarfile.CHRTYPE
        with self.assertRaises(ValueError):
            SOURCE.validate_members([member], "samba-4.23.0")

    def test_build_does_not_download_source(self):
        script = (ROOT / ".github/scripts/smb-sdk.sh").read_text()
        self.assertNotIn("curl ", script)
        self.assertNotIn("wget ", script)
        self.assertIn("python3 tools/samba-source/manage.py verify", script)
        self.assertIn("python3 tools/samba-source/manage.py extract", script)
        self.assertIn("smbd/smbd,client/smbclient", script)


if __name__ == "__main__":
    unittest.main()
