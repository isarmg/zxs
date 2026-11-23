"""Synthetic PE layout qualification only; no sign, executable, native driver, sockets or launch."""
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path
import struct
import unittest

recipe = Path(__file__).resolve().parents[2] / "packaging/windows-amd64/verify-signed.py"
spec = spec_from_file_location("workspace_final_inventory_fixture", recipe)
scan = module_from_spec(spec)
spec.loader.exec_module(scan)


class AuthenticodeExtent(unittest.TestCase):
    def setUp(self):
        # Layout fixture, NOT a real certificate or signature. OS cryptographic admission is separate.
        unsigned = bytearray(537)
        unsigned[:2] = b"MZ"
        struct.pack_into("<I", unsigned, 0x3c, 0x80)
        unsigned[0x80:0x84] = b"PE\0\0"
        struct.pack_into("<H", unsigned, 0x84, 0x8664)
        struct.pack_into("<H", unsigned, 0x80+20, 240)
        optional = 0x80+24
        struct.pack_into("<H", unsigned, optional, 0x20b)
        struct.pack_into("<I", unsigned, optional+108, 16)
        unsigned[400:409] = b"PYZ-bytes"
        scan._PKG_COOKIE.pack_into(unsigned, 449, scan._PKG_MAGIC, 137, 17, 32, 314, b"python314.dll")
        signed = bytearray(unsigned)
        check, security = scan.pe_fields(unsigned)
        offset = (len(unsigned)+7) & ~7
        signed.extend(bytes(offset-len(unsigned)) + struct.pack("<IHH", 16, 0x200, 2) + b"faketest")
        struct.pack_into("<II", signed, security, offset, 16)
        struct.pack_into("<I", signed, check, 123456)
        self.unsigned, self.signed = unsigned, signed
        self.security, self.offset = security, offset

    def test_only_checksum_and_complete_appended_table(self):
        scan.signing_only(self.unsigned, self.signed)

    def test_overlay_replacement_or_trailer_is_not_signing(self):
        for at in (0x40, 501, 536, 537, self.offset+4):
            with self.subTest(at=at):
                modified = bytearray(self.signed)
                modified[at] ^= 1
                with self.assertRaises(ValueError):
                    scan.signing_only(self.unsigned, modified)
        with self.assertRaises(ValueError):
            scan.signing_only(self.unsigned, self.signed + b"unlisted trailer")

    def test_changed_or_missing_final_archive_identity(self):
        modified = bytearray(self.signed)
        modified[-8:] = scan._PKG_MAGIC
        with self.assertRaises(ValueError):
            scan.signing_only(self.unsigned, modified)
        self.unsigned[449] = self.signed[449] = 0
        with self.assertRaises(ValueError):
            scan.signing_only(self.unsigned, self.signed)

    def test_invalid_offset_or_length_and_unsigned_preexisting_certificate(self):
        for offset, length in ((self.offset-1, 17), (self.offset, 0), (self.offset, 1<<24)):
            with self.subTest(offset=offset, length=length):
                modified = bytearray(self.signed)
                struct.pack_into("<II", modified, self.security, offset, length)
                with self.assertRaises(ValueError):
                    scan.signing_only(self.unsigned, modified)
        self.unsigned[self.security] = 1
        with self.assertRaises(ValueError):
            scan.signing_only(self.unsigned, self.signed)


if __name__ == "__main__":
    unittest.main()
