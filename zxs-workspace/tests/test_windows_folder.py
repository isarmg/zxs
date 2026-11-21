"""Bounds and source-selection regressions. Native cases require genuine local fixed NTFS."""
from hashlib import sha256
import os
from pathlib import Path
import struct
import tempfile
import unittest

from zipworkspace.desktop.selection import Verification
from zipworkspace.model import CHUNK, Rejected
from zipworkspace import windows_folder as wf, windows_nt as nt


def record(name, *, fid=1, attrs=0x20, nxt=0, tag=0xdeadbeef):
    raw = name.encode("utf-16-le","surrogatepass")
    header = nt.HEADER.pack(nxt,0,1,2,3,4,0,0,attrs,len(raw),0,tag,fid.to_bytes(16,"little"))
    return (header + raw).ljust(max(nxt, len(header)+len(raw)), b"\0")


class DirectoryRecords(unittest.TestCase):
    def test_aligned_names_ignore_undefined_nonreparse_tag_and_last_record_ends_only_page(self):
        rows = list(nt.entries(record("甲",nxt=96) + record("file",fid=2) + b"\0"*12))
        self.assertEqual([n for n,_ in rows], ["甲","file"])
        self.assertEqual(rows[1][1].file_id,(2).to_bytes(16,"little"))
        self.assertEqual(list(nt.entries(record(".",fid=0))), [(".",None)])

    def test_parser_rejects_incomplete_offsets_invalid_utf16_ids_alias_syntax_and_recall(self):
        valid = record("a")
        for bad in (b"", valid[:85], struct.pack("<I",1)+valid[4:], struct.pack("<I",96)+valid[4:],
                    valid[:60]+struct.pack("<I",3)+valid[64:], record("\ud800"),record("file:stream"),
                    record("sub/name"),record("CON"),record("a",fid=0),record("x",attrs=0x10|0x400),
                    record("x",attrs=0x1000),record("x",attrs=0x40000),record("x",attrs=0x400000)):
            with self.subTest(length=len(bad)), self.assertRaises(Rejected): list(nt.entries(bad))

    def test_only_absolute_ordinary_one_component_selections(self):
        self.assertEqual(wf.drive_path("d:/这里/目录"), ("D",("这里","目录")))
        self.assertEqual(wf.drive_path("C:/"),("C",()))
        for p in ("x","C:relative","//host/share","\\\\?\\C:\\x","\\\\.\\C:\\x",
                  "C:/../x","C:/x//z","C:/x:stream","C:/a. ","C:/NUL.txt"):
            with self.subTest(path=p), self.assertRaises(Rejected): wf.drive_path(p)


class Open:
    def __init__(self, api, value): self.api, self.value = api, value
    def __enter__(self): return self.value
    def close(self):
        if self.value is not None:
            self.api.active.pop(self.value)
            self.value = None
    def __exit__(self,*_): self.close()


class Volume:
    """Deterministic native returns for races without privileges, processes or an SMB server."""
    guid = "\\\\?\\Volume{11111111-1111-1111-1111-111111111111}\\"
    def __init__(self):
        self.active={}; self.n=0; self.data_reads=[]; self.passes=0; self.selected=2; self.member=3
        self.body=b"file"; self.unlisted=False; self.between=None; self.substitute_file=None
        self.names={1:"",2:"local",3:"local\\name",4:"local",5:"local\\name",6:"local\\empty"}
    def _handle(self,n):
        self.n+=1; self.active[self.n]=[n,0]
        return Open(self,self.n)
    def root(self, letter, enumerate_it):
        if letter != "C": raise Rejected("wrong drive")
        return self._handle(1)
    def admit_volume(self, h): return self.guid
    def guid_name(self,h): return self.guid + self.names[self.active[h][0]]
    def child(self,parent,name,kind):
        n = self.active[parent][0]
        if n==1 and name=="local": child=self.selected
        elif n in (2,4) and name=="name": child=self.substitute_file or self.member
        elif n in (2,4) and name=="empty": child=6
        else: raise Rejected("missing held relative name")
        return self._handle(child)
    def _row(self,n):
        return nt.entry(n.to_bytes(16,"little"), 0x20 if n in (3,5) else 0x10,
                        1,2,3,len(self.body) if n in (3,5) else 0)
    def info(self,h): return 101,self._row(self.active[h][0])
    def listing(self,h):
        n=self.active[h][0]
        yield ".",None
        yield "..",None
        if n not in (2,4): return
        self.passes+=1
        if self.passes==2 and self.between: self.between()
        if not self.unlisted: yield "name",self._row(self.member)
        yield "empty",self._row(6)
    def read(self,h,length):
        n,at=self.active[h]
        self.data_reads.append(n)
        block=self.body[at:at+length]
        self.active[h][1]+=len(block)
        return block


class HeldTraversal(unittest.TestCase):
    def setUp(self):
        self.api=Volume()
        self.folder=wf.WindowsFolder.choose("C:/local",self.api)
        self.addCleanup(self.folder.close)

    def test_unnamed_bytes_empty_directories_and_no_persistent_handles_in_result(self):
        self.assertEqual(self.folder.observe().rows,
                         {"name":(False,4,sha256(b"file").hexdigest()), "empty":(True,0,None)})
        self.assertEqual(self.api.data_reads,[3,3])
        self.assertEqual(self.api.passes,2)
        self.assertEqual(len(self.api.active),1)
        self.folder.close()
        self.assertFalse(self.api.active)
        with self.assertRaises(Rejected): self.folder.check()

    def test_held_picker_survives_but_replaced_name_is_refused_before_content(self):
        chosen=self.folder.handle.value
        self.api.selected=4
        self.assertEqual(self.api.active[chosen][0],2)
        with self.assertRaises(Rejected): self.folder.observe()
        self.assertFalse(self.api.data_reads)
        self.assertEqual(list(self.api.active),[chosen])

    def test_cancels_between_actual_default_stream_reads_without_a_partial_observation(self):
        class StopAfterChunk(Verification):
            def advance(self, n):
                super().advance(n)
                self.cancel()

        self.api.body = b"x" * (CHUNK + 7)
        watch = StopAfterChunk()
        with self.assertRaisesRegex(Rejected, "取消"): self.folder.observe(observer=watch)
        self.assertEqual(watch.progress(), (Verification.PHASE["folder"], CHUNK, None))
        self.assertEqual(self.api.data_reads, [3])  # no next ReadFile or EOF probe
        self.assertEqual(self.api.passes, 1)
        self.assertEqual(list(self.api.active), [self.folder.handle.value])

    def test_enumerated_id_and_actual_child_disagree_before_any_bytes(self):
        self.api.substitute_file=5
        with self.assertRaises(Rejected): self.folder.observe()
        self.assertFalse(self.api.data_reads)
        self.assertEqual(len(self.api.active),1)

    def test_missing_or_equally_sized_replaced_ordinary_file_after_hash_never_yields_report(self):
        for missing in (False,True):
            with self.subTest(missing=missing):
                self.api.passes=0; self.api.member=3; self.api.unlisted=False
                self.api.between=lambda: setattr(self.api,"unlisted" if missing else "member",True if missing else 5)
                with self.assertRaises(Rejected): self.folder.observe()
                self.assertEqual(len(self.api.active),1)


@unittest.skipUnless(os.name=="nt", "requires an actual Windows fixed local NTFS temporary directory")
class NativeLocalFolder(unittest.TestCase):
    def test_native_picker_reads_exact_default_stream_and_refuses_same_name_replacement(self):
        from zipworkspace import comparison
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); picked=root/"original"; picked.mkdir()
            (picked/"空").mkdir(); (picked/"file").write_bytes(b"first")
            held=comparison.Folder.choose(picked)
            try:
                self.assertEqual(held.observe().rows,{"空":(True,0,None),"file":(False,5,sha256(b"first").hexdigest())})
                original=held.evidence.identity
                try: picked.rename(root/"retained")
                except PermissionError:
                    # An external Windows sharing restriction prevented substitution.
                    self.assertEqual(held.observe().rows["file"][2],sha256(b"first").hexdigest())
                    return
                picked.mkdir(); (picked/"file").write_bytes(b"other")
                with self.assertRaises(Rejected): held.observe()
                again=comparison.Folder.choose(picked)
                try: self.assertNotEqual(again.evidence.identity,original)
                finally: again.close()
            finally: held.close()

    def test_native_read_sharing_conflict_and_reparse_refuse_without_mutating_contents(self):
        from zipworkspace import comparison
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); local=root/"local"; local.mkdir()
            f=local/"file"; f.write_bytes(b"keep")
            chosen=comparison.Folder.choose(local)
            try:
                with f.open("r+b") as writer, self.assertRaises((OSError,Rejected)):
                    chosen.observe()
                self.assertEqual(f.read_bytes(),b"keep")
                outside=root/"other"; outside.write_bytes(b"outside")
                try: (local/"link").symlink_to(outside)
                except OSError as err: self.skipTest("native symlink unavailable without changing privileges: "+str(err))
                with self.assertRaises((OSError,Rejected)): chosen.observe()
                self.assertEqual(outside.read_bytes(),b"outside")
            finally: chosen.close()
