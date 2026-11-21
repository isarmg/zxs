import io
from pathlib import Path
import struct
import tempfile
from unittest import TestCase, mock
import zipfile
from zlib import crc32
from zipworkspace.archive import read_archive, stored_member
from zipworkspace import worktree as w
from zipworkspace.model import Rejected


class Stream:
    def __init__(self): self.buffer = io.BytesIO()
    def write(self,data): return self.buffer.write(data)
    def flush(self): pass


def record(name, body, offset, *, central_offset=False, central_sizes=False, local_sizes=False,
           wide=False, signature=False, version=None, residue=b""):
    """A synthetic ordinary Store stream; ZIP64 size and offset decisions are independent."""
    name = name.encode("ascii")
    size, crc = len(body), crc32(body)
    extra = struct.pack("<HHQQ", 1, 16, 0, 0) if local_sizes else b""
    local, local_version = (0xffffffff, 45) if local_sizes else (0, 20)
    start = struct.pack("<4s5H3I2H", b"PK\x03\x04", local_version, 8, 0, 0, 33, 0, local, local,
                        len(name), len(extra)) + name + extra
    descriptor = (b"PK\x07\x08" if signature else b"") + struct.pack("<IQQ" if wide else "<III", crc, size, size)
    central_values = ([size, size] if central_sizes else []) + ([offset] if central_offset else [])
    n = len(central_values)
    extra = struct.pack(f"<HH{n}Q", 1, n*8, *central_values) if n else b""
    if version is None: version = 45 if n or local_sizes else 20
    declared = 0xffffffff if central_sizes else size
    central = struct.pack("<4s6H3I5H2I", b"PK\x01\x02", 3*256+version, version, 8, 0, 0, 33, crc,
                          declared, declared, len(name), len(extra), 0, 0, 0, 0o100600 << 16,
                          0xffffffff if central_offset else offset)
    return start + body + descriptor + residue, central + name + extra


def directory(central, offset, count):
    tail = b""
    if offset > 0xffffffff:
        at = offset + len(central)
        tail = struct.pack("<4sQ2H2I4Q", b"PK\x06\x06", 44, 45, 45, 0, 0, count, count, len(central), offset)
        tail += struct.pack("<4sIQI", b"PK\x06\x07", 0, at, 1)
    return central + tail + struct.pack("<4s4H2IH", b"PK\x05\x06", 0, 0, count, count,
                                        len(central), min(offset, 0xffffffff), 0)


class SparseRead:
    """Fixed-size synthetic input with one small, real ZIP suffix at a large offset."""
    def __init__(self, offset, suffix):
        self.offset, self.suffix, self.position = offset, suffix, 0
        self.length = offset + len(suffix)

    def tell(self): return self.position

    def seek(self, n, whence=0):
        self.position = {0:0, 1:self.position, 2:self.length}[whence] + n
        if self.position < 0: raise ValueError("invalid sparse offset")
        return self.position

    def read(self, n):
        if not 0 <= n <= 2 << 20: raise AssertionError("unexpected sparse read window")
        first = self.position
        if first >= self.length or not n: return b""
        self.position = end = min(self.length, first+n)
        zeroes = max(0, min(end, self.offset)-first)
        return bytes(zeroes) + self.suffix[max(0, first-self.offset):max(0, end-self.offset)]


class LocalHeader(TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.path = self.root/"plain.zip"

    def test_contradictory_local_store_flags_crc_and_sizes_are_not_an_ordinary_archive(self):
        with zipfile.ZipFile(self.path, "w", compression=zipfile.ZIP_STORED) as z:
            z.writestr("member", b"small ordinary fixture")
        initial = self.path.read_bytes()
        for offset, pattern, value in ((6,"<H",1),(8,"<H",8),(14,"<I",1),(22,"<I",2)):
            with self.subTest(offset=offset):
                data = bytearray(initial); struct.pack_into(pattern,data,offset,value); self.path.write_bytes(data)
                with self.assertRaises(Rejected): read_archive(self.path, managed=False)

    def test_actual_standard_stream_descriptor_and_local_zip64_are_admitted(self):
        for force in (False, True):
            output = Stream()
            with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED) as z:
                with z.open("member","w",force_zip64=force) as f: f.write(b"content")
            original = output.buffer.getvalue()
            self.path.write_bytes(original)
            self.assertEqual(read_archive(self.path, managed=False)[1].rows["member"][1],7)
            # The original streaming writer emitted a descriptor independently
            # from the central record: changing only its CRC must fail.
            position = original.index(b"PK\x07\x08")
            modified = bytearray(original); modified[position+4] ^= 1
            self.path.write_bytes(modified)
            with self.assertRaises(Rejected): read_archive(self.path, managed=False)

    def test_offset_only_zip64_can_read_complete_narrow_or_wide_descriptor_at_large_offset(self):
        offset = (1 << 32) + 809
        for content in (b"", b"selected small member"):
            for wide in (False, True):
                for signature in (False, True):
                    with self.subTest(content=content, wide=wide, signature=signature):
                        body, central = record("selected", content, offset, central_offset=True,
                                               wide=wide, signature=signature)
                        suffix = directory(central, offset+len(body), 1)
                        doc = SparseRead(offset, body + suffix)
                        with stored_member(doc, doc.length, None, "selected", len(content), locate=True) as (selected, at):
                            self.assertGreater(at, 1 << 32)
                            self.assertEqual(selected.read(), content)
                        if wide:
                            # A changed high 32-bit tail must not pass as a successful short all-zero prefix.
                            broken = SparseRead(offset, body[:-1] + b"\x01" + suffix)
                            with self.assertRaisesRegex(Rejected, "actual member"):
                                with stored_member(broken, broken.length, None, "selected", len(content)): pass

    def test_descriptor_form_and_physical_order_are_checked_on_complete_and_selected_reads(self):
        first, c1 = record("first", b"one", 0, central_offset=True, wide=True)
        second, c2 = record("second", b"two", len(first), signature=True)
        self.path.write_bytes(first + second + directory(c2+c1, len(first)+len(second), 2))
        self.assertEqual(set(read_archive(self.path, managed=False)[1].rows), {"first", "second"})

        # Padded/truncated or misdeclared streamed members do not acquire a
        # descriptor by sampling the following header or accepting a prefix.
        for options, error in [
                ({"central_offset":True, "version":20}, "central ZIP64"),
                ({"central_sizes":True}, "descriptor width"),
                ({"wide":True}, "descriptor width"),
                ({"signature":True, "residue":b"00"}, "member boundary"),
                ({"local_sizes":True, "wide":False}, "descriptor width")]:
            with self.subTest(options=options):
                first, c1 = record("first", b"one", 0, **options)
                second, c2 = record("second", b"two", len(first))
                raw = first + second + directory(c1+c2, len(first)+len(second), 2)
                self.path.write_bytes(raw)
                with self.assertRaisesRegex(Rejected, error): read_archive(self.path, managed=False)
                with self.assertRaisesRegex(Rejected, error):
                    with stored_member(io.BytesIO(raw), len(raw), None, "first", 3): pass

    def test_missing_native_write_contract_refuses_before_catalog_or_checkout_creation(self):
        work, store, state = (self.root / n for n in ["work","store","state"])
        work.mkdir(); store.mkdir()
        with mock.patch("zipworkspace.worktree.require_native_writer", side_effect=Rejected("unsupported provider")):
            with self.assertRaises(Rejected): w.enroll(work,store,state)
            with self.assertRaises(Rejected): w.import_zip(self.path,self.root/"out")
        self.assertFalse(state.exists()); self.assertFalse((self.root/"out").exists())
