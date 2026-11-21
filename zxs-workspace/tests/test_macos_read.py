"""Native policy boundary fixtures; macOS product qualification is separate."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack, contextmanager
import ctypes as C
import errno
import hashlib
import os
from pathlib import Path
import struct
import sys
import tempfile
from threading import current_thread, local
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import comparison, macos_read as mac
from zipworkspace import _macos_copy_evidence as writing
from zipworkspace.desktop import text_preview
from zipworkspace.desktop.selection import Selection, Verification
from zipworkspace.model import Commit, Manifest, Rejected


class ThreadIO:
    """Injected thread policy and fixed-width Darwin replies, not a substitute filesystem."""
    def __init__(self, initial=0):
        self.initial, self._policy = initial, local()
        self.changes = []
        self.policy_threads, self.failed_restorations = [], []
        self.fail = None
        self.volumes = []
        self.reply = (28, 0x80000000, 0x4000, 0, 0, 0, 0x1000)
        self.calls = (self.get, self.set, self.attrs)

    @property
    def value(self): return getattr(self._policy, "value", self.initial)

    @value.setter
    def value(self, value): self._policy.value = value

    def get(self, kind, scope):
        assert (kind, scope) == (3, 1)
        return self.value

    def set(self, kind, scope, value):
        assert (kind, scope) == (3, 1)
        self.changes.append(value)
        self.policy_threads.append(current_thread())
        if self.fail == value:
            self.failed_restorations.append(current_thread())
            return -1
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

    def test_restore_failure_isolated_from_gui_and_shared_worker_followup(self):
        api = ThreadIO(0)
        def operation():
            with mac.no_materialization():
                self.assertEqual(api.value, 1)
                return current_thread()

        with patch.object(mac,"_api",return_value=api.calls), ThreadPoolExecutor(max_workers=1) as shared:
            def through_shared(): return mac.in_disposable_thread(operation)
            shared_before = shared.submit(lambda: (current_thread(), api.value)).result()
            api.fail = 0
            with self.assertRaisesRegex(Rejected,"无法恢复"):
                shared.submit(through_shared).result()
            failed, = api.failed_restorations
            self.assertFalse(failed.is_alive())
            self.assertIsNot(failed, shared_before[0]); self.assertIsNot(failed, current_thread())
            self.assertEqual(api.value, 0)
            self.assertEqual(shared.submit(lambda: (current_thread(), api.value)).result(), shared_before)
            api.fail = None
            later = shared.submit(through_shared).result()
            self.assertIsNot(later, failed)
            self.assertFalse(later.is_alive())
            self.assertEqual(shared.submit(lambda: api.value).result(), 0)

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

    def test_apfs_read_evidence_does_not_open_the_existing_write_qualifier(self):
        flags = 0x1001  # Local, explicitly read-only.
        reply = bytearray(128)
        volume = b"\x01" * 16
        def attributes(_fd, request, output, size, opts):
            query = C.cast(request, C.POINTER(mac._Attributes)).contents
            self.assertEqual((query.volume, size, opts), (0x80144000, 128, 4))
            C.memmove(output, bytes(reply), size)
            return 0

        struct.pack_into("=7I", reply, 0, 57, 0x80000000, 0x144000, 0, 0, 0, flags)
        reply[28:44] = volume
        struct.pack_into("=iI5s", reply, 44, 8, 5, b"apfs\0")
        with patch.object(mac, "_require", return_value=(None,None,attributes)), \
             patch.object(os, "fstat", return_value=SimpleNamespace(st_flags=0, st_dev=19)):
            self.assertEqual(mac.apfs_volume(4), ((19,volume), flags))
            with self.assertRaises(Rejected): writing.apfs(4)
            struct.pack_into("=I", reply, 24, 0x1400)  # Volume quarantine still blocks fresh writes.
            self.assertEqual(mac.apfs_volume(4)[0], (19,volume))
            with self.assertRaisesRegex(Rejected,"系统卷级隔离"): writing.apfs(4, fresh_snapshot=True)
            struct.pack_into("=i", reply, 44, -4)
            with self.assertRaises(Rejected): mac.apfs_volume(4)


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

    @contextmanager
    def literal_fixture(self):
        # A real held ZIP and POSIX files, with only the Darwin answers modeled.
        self.leaf.write_bytes(b"next")
        display = Selection.open(self.archive.resolve(strict=True), retain=True)
        self.addCleanup(display.release_display)
        path = self.directory.resolve(strict=True)
        api = ThreadIO()
        def active(*_): self.assertEqual(api.value, 1)
        def volume(fd):
            active()
            return (os.fstat(fd).st_dev,b"original APFS id"), 0x1000
        with patch.object(sys, "platform", "darwin"), patch.object(mac,"_api",return_value=api.calls), \
             patch.object(mac,"resident_stat",side_effect=active), patch.object(mac,"apfs_volume",side_effect=volume), \
             patch.object(mac,"check_literal_path",side_effect=active), patch.object(mac,"check_spelling",side_effect=active), \
             patch.object(mac,"_O_EVTONLY",0):
            yield api, display, path

    def test_literal_report_never_reopens_zip_and_reads_one_exact_right_file_under_a_new_policy(self):
        original = self.leaf.stat()
        with self.literal_fixture() as (api, display, path), \
             patch.object(type(path), "stat", side_effect=AssertionError("no generic absolute stat")), \
             patch.object(comparison, "named_document", side_effect=AssertionError("no ZIP reopen")):
            folder = comparison.choose_comparison_folder(path, literal=True)
            self.assertEqual((api.value, api.changes), (0,[1,0]))
            report = comparison.compare(display, folder, observer=Verification(), retain_text=True)
            self.addCleanup(report.close)
            self.assertEqual((api.value, api.changes), (0,[1,0,1,0]))
            self.assertIsNone(folder._fd)
            row, = report.rows
            actual, seen = os.read, []
            def one(fd, count):
                self.assertEqual(api.value, 1)
                self.assertTrue(os.path.samestat(os.fstat(fd), original))
                block = actual(fd, count)
                seen.append(block)
                return block
            with patch.object(os, "read", side_effect=one), \
                 patch.object(comparison,"observe_held",side_effect=AssertionError("no inventory redo")):
                result = text_preview.compare_folder_text(display, report, row, observer=Verification())
            self.assertIn("-body", result); self.assertIn("+next", result)
            self.assertEqual(seen, [b"next", b""])
            self.assertEqual((api.value, api.changes), (0,[1,0,1,0,1,0]))
            stopped = Verification(); stopped.cancel()
            with self.assertRaisesRegex(Rejected, "取消"):
                text_preview.compare_folder_text(display, report, row, observer=stopped)
            report.close()
            self.assertFalse(report.text.supports(display, row))

    def test_literal_apfs_proof_refuses_a_link_and_not_proven_apfs_keeps_only_the_held_list(self):
        with self.literal_fixture() as (api, display, path):
            alternate = path.parent / "aliased"
            alternate.symlink_to(path.parent, target_is_directory=True)
            with self.assertRaises((OSError, Rejected)):
                comparison.choose_comparison_folder(alternate/path.name, literal=True)
            folder = comparison.choose_comparison_folder(path, literal=True)
            with patch.object(mac, "apfs_volume", side_effect=Rejected("unknown filesystem")), \
                 patch.object(type(path), "stat", side_effect=AssertionError("no generic stat fallback")), \
                 patch.object(comparison,"named_document",side_effect=AssertionError("no archive reopen")):
                report = comparison.compare(display, folder, observer=Verification(), retain_text=True)
            self.assertEqual([r.path for r in report.rows], ["z"])
            self.assertIsNone(report.text)
            self.assertEqual(api.value, 0)

    def test_literal_picker_and_inventory_fail_without_leaving_policy_on_the_caller(self):
        with self.literal_fixture() as (api, display, path):
            api.fail = 0
            with self.assertRaisesRegex(Rejected,"无法恢复"):
                comparison.choose_comparison_folder(path, literal=True)
            picker = api.failed_restorations[-1]
            self.assertFalse(picker.is_alive())
            self.assertIsNot(picker, current_thread())
            self.assertEqual(api.value, 0)
            api.fail = None
            folder = comparison.choose_comparison_folder(path, literal=True)
            api.fail = 0
            with self.assertRaisesRegex(Rejected,"无法恢复"):
                comparison.compare(display, folder, observer=Verification(), retain_text=True)
            scanning = api.failed_restorations[-1]
            self.assertFalse(scanning.is_alive())
            self.assertIsNot(scanning, picker)
            self.assertIsNone(folder._fd)
            self.assertEqual(api.value, 0)

    def test_dataless_or_late_same_name_copy_rejects_and_restore_error_prevents_returning_text(self):
        with self.literal_fixture() as (api, display, path):
            folder = comparison.choose_comparison_folder(path, literal=True)
            report = comparison.compare(display, folder, observer=Verification(), retain_text=True)
            self.addCleanup(report.close)
            row, = report.rows
            original = (path / row.path).stat()
            def no_offline(name, found):
                self.assertEqual(api.value, 1)
                if os.path.samestat(found, original): raise Rejected("not resident")
            with patch.object(mac,"resident_stat",side_effect=no_offline), \
                 patch.object(os,"read",side_effect=AssertionError("no placeholder read")), self.assertRaisesRegex(Rejected,"not resident"):
                text_preview.compare_folder_text(display, report, row, observer=Verification())
            native_close, native_read = os.close, os.read
            closed, read = [], []
            def only_close(fd):
                self.assertEqual(api.value, 1)
                closed.append(fd)
                native_close(fd)
            def actual_read(fd, n):
                read.append(fd)
                return native_read(fd, n)
            api.fail = 0
            with patch.object(os,"close",side_effect=only_close), patch.object(os,"read",side_effect=actual_read), \
                 self.assertRaisesRegex(Rejected,"无法恢复"):
                text_preview.compare_folder_text(display, report, row, observer=Verification())
            self.assertTrue(read)
            self.assertTrue(set(read).issubset(closed))
            failed, = api.failed_restorations
            self.assertFalse(failed.is_alive())
            self.assertIsNot(failed, current_thread())
            self.assertEqual(api.value, 0)
            api.fail = None
            leaf = path / row.path
            data, diff = leaf.read_bytes(), text_preview._unified
            def replace_after_body(*args):
                value = diff(*args)
                leaf.rename(leaf.with_name("retained")); leaf.write_bytes(data)
                return value
            with patch.object(text_preview, "_unified", side_effect=replace_after_body), self.assertRaises(Rejected):
                text_preview.compare_folder_text(display, report, row, observer=Verification())
            self.assertEqual(api.value, 0)
            with patch.object(os,"read",side_effect=AssertionError("no replacement body")), self.assertRaises(Rejected):
                text_preview.compare_folder_text(display, report, row, observer=Verification())
            self.assertIsNot(api.policy_threads[-1], failed)
            self.assertFalse(api.policy_threads[-1].is_alive())

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

    @unittest.skipUnless(sys.platform == "darwin", "real APFS native name and vnode evidence required")
    def test_real_apfs_pair_retains_originals_and_accepts_no_case_alias(self):
        get, _, _ = mac._require()
        root = self.directory.resolve(strict=True)
        (root / "a").mkdir(); (root / "other").mkdir()
        (root / "z").rename(root / "a" / "z")
        (root / "a" / "z").write_bytes(b"next")
        (root / "other" / "z").write_bytes(b"not selected")
        package = root.parent / "pair.zxs"
        index = Manifest()
        with zipfile.ZipFile(package, "x", compression=zipfile.ZIP_STORED) as z:
            for name, body in (("a/z", b"old"), ("other/z", b"another")):
                index.add(name, False, len(body), hashlib.sha256(body).hexdigest())
                z.writestr(name, body)
            z.comment = Commit("1"*32,"2"*32,1,"3"*32,None,index.digest()).encode()
        displayed = Selection.open(package, retain=True)
        with ExitStack() as owned:
            owned.callback(displayed.release_display)
            policy = get(3, 1)
            folder = comparison.choose_comparison_folder(root, literal=True)
            owned.callback(folder.close)
            with mac.no_materialization():
                try: mac.apfs_volume(folder._fd)
                except (OSError, Rejected): self.skipTest("qualification requires a real local APFS folder")
            self.assertEqual(get(3, 1), policy)
            with self.assertRaises((OSError, Rejected)):
                comparison.choose_comparison_folder(root.with_name(root.name.upper()), literal=True)
            with patch.object(type(root), "stat", side_effect=AssertionError("no generic absolute stat")), \
                 patch.object(comparison,"named_document",side_effect=AssertionError("no ZIP reopened by path")):
                report = comparison.compare(displayed, folder, observer=Verification(), retain_text=True)
                owned.callback(report.close)
                self.assertIsNotNone(report.text)
                row = next(r for r in report.rows if r.path == "a/z")
                old = displayed.held.fileno()
                os.lseek(old, 17, os.SEEK_SET)
                value = text_preview.compare_folder_text(displayed, report, row, observer=Verification())
                self.assertIn("+next", value)
                self.assertNotIn("not selected", value)
                self.assertEqual(os.lseek(old, 0, os.SEEK_CUR), 17)
                self.assertEqual(get(3, 1), policy)
