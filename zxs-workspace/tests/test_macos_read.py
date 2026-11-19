"""Native policy boundary fixtures; macOS product qualification is separate."""
import ctypes as C
import errno
import hashlib
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import comparison, macos_read as mac
from zipworkspace.desktop.selection import Selection
from zipworkspace.model import Commit, Manifest, Rejected


class ThreadIO:
    """Injected thread policy and fixed-width Darwin replies, not a substitute filesystem."""
    def __init__(self, initial=0):
        self.value = initial
        self.changes = []
        self.fail = None
        self.volumes = []
        self.reply = (28, 0x80000000, 0x4000, 0, 0, 0, 0x1000)
        self.calls = (self.get, self.set, self.attrs)

    def get(self, kind, scope):
        assert (kind, scope) == (3, 1)
        return self.value

    def set(self, kind, scope, value):
        assert (kind, scope) == (3, 1)
        self.changes.append(value)
        if self.fail == value: return -1
        self.value = value
        return 0

    def attrs(self, fd, request, output, size, options):
        assert self.value == 1  # No filesystem access before this calling thread opts out.
        query = C.cast(request, C.POINTER(mac._Attributes)).contents
        assert (query.count,query.reserved,query.common,query.volume,query.directory,query.file,query.fork) == (
            5, 0, 0x80000000, 0x80004000, 0, 0, 0)
        assert size == 28 and options == 4
        self.volumes.append(fd)
        output[:] = self.reply
        return 0


class Boundary(unittest.TestCase):
    def test_default_on_and_existing_off_are_restored_on_error_including_a_nested_scope(self):
        for initial in (0, 2, 1):
            api = ThreadIO(initial)
            with self.subTest(initial=initial), patch.object(mac, "_api", return_value=api.calls):
                with self.assertRaisesRegex(Rejected, "可能只在云端"):
                    with mac.no_materialization(), mac.no_materialization():
                        self.assertEqual(api.value, 1)
                        raise OSError(errno.EDEADLK, "provider content unavailable")
                self.assertEqual(api.value, initial)
                self.assertEqual(api.changes, [] if initial == 1 else [1, initial])
        with patch.object(mac, "_api", return_value=ThreadIO(99).calls), self.assertRaises(Rejected):
            with mac.no_materialization(): self.fail("unknown original is not authority to read")
        api = ThreadIO(2); api.fail = 1
        with patch.object(mac, "_api", return_value=api.calls), self.assertRaises(Rejected):
            with mac.no_materialization(): self.fail("failed opt-out must not read")
        self.assertEqual(api.value, 2)

    def test_interruption_returning_from_initial_policy_change_restores_without_any_file_io(self):
        api = ThreadIO(2)
        def interrupted(kind, scope, value):
            rc = api.set(kind, scope, value)
            if value == 1: raise KeyboardInterrupt  # The actual policy changed before Python returned.
            return rc
        with patch.object(mac,"_api",return_value=(api.get, interrupted, api.attrs)), self.assertRaises(KeyboardInterrupt):
            with mac.no_materialization(): self.fail("interrupted setup must not inspect any paths")
        self.assertEqual((api.value, api.changes, api.volumes), (2, [1,2], []))

    def test_fixed_returned_volume_attributes_require_the_actual_positive_local_flag(self):
        api = ThreadIO(1)
        with patch.object(mac, "_api", return_value=api.calls):
            mac.local_volume(14)
            for index, value in ((0,24), (0,32), (1,0), (2,0x80000000), (3,1), (6,0), (6,2)):
                reply = [28,0x80000000,0x4000,0,0,0,0x1000]; reply[index] = value; api.reply = tuple(reply)
                with self.subTest(index=index,value=value), self.assertRaises(Rejected): mac.local_volume(19)
            self.assertEqual(set(api.volumes), {14,19})
        for meta in (SimpleNamespace(), SimpleNamespace(st_flags=0x40000000)):
            with patch.object(os,"fstat",return_value=meta), self.assertRaises(Rejected): mac.resident("leaf", 4)


@unittest.skipUnless(os.name == "posix" and comparison.available(), "requires descriptor-relative POSIX traversal")
class ComparisonBoundary(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory(); self.addCleanup(tmp.cleanup)
        self.home = Path(tmp.name)
        self.directory = self.home/"ordinary"; self.directory.mkdir()
        self.leaf = self.directory/"z"; self.leaf.write_bytes(b"body")
        self.archive = self.home/"source.zxs"
        tree = Manifest(); tree.add("z", False, 4, hashlib.sha256(b"body").hexdigest())
        marker = Commit("1"*32, "2"*32, 1, "3"*32, None, tree.digest())
        with zipfile.ZipFile(self.archive, "x", compression=zipfile.ZIP_STORED) as z:
            z.writestr("z", b"body"); z.comment = marker.encode()
        self.selected = Selection.open(self.archive)

    def test_picker_and_worker_hold_separate_guards_and_dataless_leaf_stops_before_read(self):
        api = ThreadIO(); reads = []
        real_resident = mac.resident
        def verify(name, fd):
            self.assertEqual(api.value, 1)
            meta = os.fstat(fd)
            if os.path.samestat(meta, self.leaf.stat()):
                with patch.object(os,"fstat",return_value=SimpleNamespace(st_flags=0x40000000)):
                    real_resident(name, fd)
        def no_body(*_):
            reads.append(1); raise AssertionError("dataless leaf contents must never be read")
        with patch.object(sys,"platform","darwin"), patch.object(mac,"_api",return_value=api.calls), \
             patch.object(mac,"resident",side_effect=verify):
            folder = comparison.Folder.choose(self.directory)
            self.assertEqual(api.changes,[1,0])
            fd = folder._fd
            with patch.object(os,"read",side_effect=no_body), self.assertRaisesRegex(Rejected,"未将占位"):
                comparison.compare(self.selected, folder)
            self.assertEqual((api.changes,reads), ([1,0,1,0],[]))
            with self.assertRaises(OSError) as error: os.fstat(fd)
            self.assertEqual(error.exception.errno, errno.EBADF)

    def test_remote_archive_stops_before_directory_walk_and_picker_restore_failure_closes_its_fd(self):
        api = ThreadIO()
        with patch.object(sys,"platform","darwin"), patch.object(mac,"_api",return_value=api.calls), \
             patch.object(mac,"resident",side_effect=lambda _,fd: self.assertEqual(api.value,1)):
            folder = comparison.Folder.choose(self.directory)
            fd = folder._fd
            api.reply = (28, 0x80000000, 0x4000, 0, 0, 0, 0)  # The reopened archive is not local.
            with patch.object(folder, "observe", side_effect=AssertionError("do not scan")), self.assertRaises(Rejected):
                comparison.compare(self.selected,folder)
            self.assertIsNone(folder._fd); self.assertEqual(api.value,0)
            with self.assertRaises(OSError): os.fstat(fd)
            api.reply = (28, 0x80000000, 0x4000, 0, 0, 0, 0x1000)
            api.fail = 0
            with self.assertRaisesRegex(Rejected,"无法恢复"): comparison.Folder.choose(self.directory)
            with self.assertRaises(OSError) as closed: os.fstat(api.volumes[-1])
            self.assertEqual(closed.exception.errno, errno.EBADF)

    @unittest.skipUnless(sys.platform == "darwin", "real macOS local volume and native thread policy required")
    def test_real_local_volume_compares_exact_files_and_restores_both_thread_policies(self):
        from concurrent.futures import ThreadPoolExecutor
        get, _, _ = mac._require()
        ui_policy = get(3, 1)
        folder = comparison.Folder.choose(self.directory)
        self.addCleanup(folder.close)
        self.assertEqual(get(3, 1), ui_policy)
        def worker():
            original = get(3, 1)
            report = comparison.compare(self.selected,folder)
            self.assertEqual(get(3, 1), original)
            return report
        with ThreadPoolExecutor(max_workers=1) as pool:
            report = pool.submit(worker).result()
        self.assertEqual(report.rows, ()); self.assertEqual(report.operation, self.selected.browser.commit.operation)
        self.assertEqual(get(3, 1), ui_policy)
