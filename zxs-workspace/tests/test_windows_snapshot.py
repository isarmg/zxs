"""No driver or mount. Exercise the actual business reader with a single private file cursor."""
import errno
import hashlib
import io
import os
import tempfile
import unittest
from unittest import mock
import zipfile

from zipworkspace.model import CHUNK, Commit, Manifest, Rejected
from zipworkspace.snapshot_view import MAX_READ, ROOT
from zipworkspace.windows_snapshot import _Volume, _WindowsView, selected_snapshot


def synthetic(doc, extra=()):
    """The optional native callback fixture uses this exact synthetic layout as well."""
    manifest = Manifest()
    with zipfile.ZipFile(doc, "w") as z:
        for name, data in (("empty", b""), ("folder/sample.bin", b"A"*CHUNK + b"BC"),
                           ("folder/𠀀.txt", b"text"), ("none/", None), *extra):
            if data is None:
                manifest.add(name[:-1], True, 0, None)
                z.writestr(name, b"")
            else:
                manifest.add(name, False, len(data), hashlib.sha256(data).hexdigest())
                with z.open(name, "w", force_zip64=True) as f: f.write(data)
        marker = Commit("1"*32, "2"*32, 1, "3"*32, None, manifest.digest())
        z.comment = marker.encode()
    doc.seek(0)
    return marker


class DataPlane(unittest.TestCase):
    def open(self, extra=()):
        doc = self.enterContext(tempfile.TemporaryFile(mode="w+b", buffering=0))
        mark = synthetic(doc, extra)
        view = _WindowsView(doc, display_name="synthetic.zxs")
        self.assertEqual(mark, view.commit)
        return _Volume(view, lambda: None)

    def test_case_exact_lookup_and_sorted_strictly_after_arbitrary_marker(self):
        v = self.open()
        root = lambda mark: [name for name, _ in v.directory(ROOT, mark)]
        self.assertEqual(root(None), ["empty", "folder", "none"])
        self.assertEqual(root("."), root(None))
        self.assertEqual(root("f"), ["folder", "none"])
        folder = v.lookup("\\folder")[0]
        rows = lambda m: [name for name,_ in v.directory(folder, m)]
        self.assertEqual(rows(None), [".", "..", "sample.bin", "𠀀.txt"])
        self.assertEqual(rows("."), ["..", "sample.bin", "𠀀.txt"])
        self.assertEqual(rows(".."), ["sample.bin", "𠀀.txt"])
        self.assertEqual(rows("r"), ["sample.bin", "𠀀.txt"])
        self.assertEqual(rows("sample.bin"), ["𠀀.txt"])
        self.assertEqual(rows("𠀀.txt"), [])
        with self.assertRaises(FileNotFoundError): v.lookup("\\Folder")
        with self.assertRaises(NotADirectoryError): v.lookup("\\empty\\child")
        for path in ("relative", "\\\\server", "\\folder\\..", "\\empty:other", "\\folder/sample.bin"):
            with self.subTest(path=path), self.assertRaises(ValueError): v.lookup(path)
        with self.assertRaises(ValueError): self.open((("bad .txt", b"x"),))

    def test_only_exact_requested_bytes_from_verified_blocks_including_partial_eof(self):
        v = self.open()
        ino, d, n = v.lookup("\\folder\\sample.bin")
        self.assertEqual((d,n), (False,CHUNK+2))
        with mock.patch.object(v.view, "_read_at", wraps=v.view._read_at) as r:
            self.assertEqual(v.read(ino, CHUNK-1, 8), b"ABC")
            start = v.view.node(ino).start
            self.assertEqual([c.args for c in r.call_args_list], [(CHUNK,start),(2,start+CHUNK)])
        self.assertEqual(v.read(ino, n, 0), b"")
        with self.assertRaises(EOFError): v.read(ino, n, 1)
        with self.assertRaises(EOFError): v.read(v.lookup("\\empty")[0], 0, 1)
        with self.assertRaises(IsADirectoryError): v.read(ROOT, 0, 1)
        with self.assertRaises(OSError): v.read(ino, 0, MAX_READ + 1)

    def test_changed_later_block_never_returns_prefix_then_poison_including_eof(self):
        v = self.open()
        ino = v.lookup("\\folder\\sample.bin")[0]
        original = v.view._read_at
        def wrong(n, off):
            b = original(n, off)
            return b"XX" if n == 2 else b
        with mock.patch.object(v.view, "_read_at", side_effect=wrong):
            with self.assertRaises(OSError) as err: v.read(ino, CHUNK-1, 3)
        self.assertEqual(err.exception.errno, errno.EIO)
        with self.assertRaises(OSError): v.read(ino, CHUNK+2, 0)
        with self.assertRaises(OSError): v.lookup("\\")

    def test_shared_cursor_concurrent_ranges_stay_with_their_own_request(self):
        from concurrent.futures import ThreadPoolExecutor
        v = self.open()
        ino = v.lookup("\\folder\\sample.bin")[0]
        ranges = [(CHUNK-1, 3, b"ABC"), (4, 3, b"AAA"), (CHUNK, 2, b"BC")]*5
        with ThreadPoolExecutor(3) as workers:
            requests = [workers.submit(v.read, ino, off, n) for off,n,_ in ranges]
            self.assertEqual([f.result() for f in requests], [expected for _,_,expected in ranges])

    def test_dead_or_closed_capability_never_returns_cached_data(self):
        v = self.open()
        v.observe = mock.Mock(side_effect=Rejected("different original"))
        with self.assertRaises(OSError): v.volume_size()
        v.observe = lambda: None
        with self.assertRaises(OSError): v.info(ROOT)
        v = self.open()
        v.close()
        with self.assertRaises(OSError): v.directory(ROOT, None)
        with self.assertRaises(OSError): v.read(v.view.lookup(ROOT, b"empty"), 0, 0)


@unittest.skipUnless(os.name == "nt" and os.environ.get("ZIP_WORKSPACE_WINFSP_FIXTURE"),
                     "explicit existing synthetic local NTFS archive and independently built trusted SDK fixture")
class NativeCallbacks(unittest.TestCase):
    def test_vendor_interface_against_real_held_synthetic_snapshot_without_a_volume(self):
        from pathlib import Path
        from zipworkspace import _test_winfsp_callbacks  # Only in the separately built trusted test environment.
        import msvcrt
        from zipworkspace.archive import stamp
        from zipworkspace.document import archive_sha
        from zipworkspace.refs import Node
        path = Path(os.environ["ZIP_WORKSPACE_WINFSP_FIXTURE"])
        with selected_snapshot(path) as source:
            # The operator's synthetic archive must be exactly the known tree, not arbitrary user data.
            with io.BytesIO() as scratch:
                marker = synthetic(scratch)
            self.assertEqual(source.view.commit, marker)
            exact = Node(marker, path, archive_sha(source.view.document), stamp(source.view.document))
            native = msvcrt.get_osfhandle(source.view.document.fileno())
            self.assertNotEqual(native, -1)
            _test_winfsp_callbacks.check(source)
            _test_winfsp_callbacks.check_session(source)
        with selected_snapshot(path, expected=exact) as again:
            self.assertEqual(again.lookup("\\folder\\sample.bin")[2], CHUNK+2)
