import io
from pathlib import Path
import struct
import tempfile
from unittest import TestCase, mock
import zipfile
from zipworkspace.archive import read_archive
from zipworkspace import worktree as w
from zipworkspace.model import Rejected


class Stream:
    def __init__(self): self.buffer = io.BytesIO()
    def write(self,data): return self.buffer.write(data)
    def flush(self): pass

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

    def test_missing_native_write_contract_refuses_before_catalog_or_checkout_creation(self):
        work, store, state = (self.root / n for n in ["work","store","state"])
        work.mkdir(); store.mkdir()
        with mock.patch("zipworkspace.worktree.require_native_writer", side_effect=Rejected("unsupported provider")):
            with self.assertRaises(Rejected): w.enroll(work,store,state)
            with self.assertRaises(Rejected): w.import_zip(self.path,self.root/"out")
        self.assertFalse(state.exists()); self.assertFalse((self.root/"out").exists())
