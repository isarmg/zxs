"""Opt-in ordinary-user ext4/XFS, no device changes; leaves only synthetic cases for inspection."""
from concurrent.futures import Future
from contextlib import ExitStack
import errno
import hashlib
from importlib.util import find_spec
import os
from pathlib import Path
import stat
import sys
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace.desktop import member_copy
from zipworkspace.desktop.selection import Selection
from zipworkspace.model import CHUNK, Commit, Manifest, Rejected, new_id
from zipworkspace.publish import Uncertain


@unittest.skipUnless(sys.platform == "linux", "Linux native export qualification")
class LinuxMemberCopy(unittest.TestCase):
    def setUp(self):
        explicit = os.environ.get("ZXS_LINUX_COPY_SCRATCH")
        if not explicit: self.skipTest("give an exclusive disposable ext4/XFS directory in ZXS_LINUX_COPY_SCRATCH")
        scratch = Path(explicit)
        if scratch.anchor != "/" or str(scratch) != explicit or ".." in scratch.parts:
            self.fail("give the exact absolute path of the existing scratch directory")
        leaf = "member-copy-" + new_id()
        with ExitStack() as held:
            proc = member_copy._ProcView(held)
            uid = member_copy._ordinary_user(read_proc=proc.read)
            parent, _ = member_copy._directory(held, scratch, uid, proc.read)
            member_copy.local_mount(parent, read_proc=proc.read)
            if stat.S_IMODE(os.fstat(parent).st_mode) != 0o700:
                self.fail("the fixture scratch directory must be exclusive 0700")
            # Exclusive case creation under the held scratch; no cleanup by name.
            os.mkdir(leaf, mode=0o700, dir_fd=parent)
        case = scratch / leaf
        self.dest = case / "out"
        self.dest.mkdir(mode=0o700)
        self.path = self.dest / "saved.bin"
        self.data = b"\x00original\xff" * 110_000  # crosses a real copy block and is never opened/executed
        self.archive = case / "chosen.zxs"
        tree = Manifest()
        with self.archive.open("xb") as raw, zipfile.ZipFile(raw, "w") as archive:
            for name, data in (("original/same.bin", self.data), ("other/same.bin", b"x" * (2*CHUNK)),
                               ("original/zero", b"")):
                tree.add(name, False, len(data), hashlib.sha256(data).hexdigest())
                archive.writestr(name, data)
            archive.comment = Commit("1"*32, "2"*32, 1, "3"*32, None, tree.digest()).encode()
        self.original = self.archive.read_bytes()
        self.selected = Selection.open(self.archive, retain=True)
        self.addCleanup(self.selected.release_display)

    def plan(self, name="same.bin"):
        entry = self.selected.browser.entry("original", name)
        plan = member_copy.MemberCopy(self.selected, "original", entry, str(self.path))
        self.addCleanup(plan.close)
        return plan

    def test_non_proc_root_fails_before_any_proc_paths_are_interpreted(self):
        spoof = self.dest / "not-proc"
        spoof.mkdir(mode=0o700)
        (spoof / "self").symlink_to(str(os.getpid()))
        plan = self.plan()
        real_open = os.open
        def opened(path, *args, **kwargs):
            return real_open(spoof if path == "/proc" else path, *args, **kwargs)
        with patch.object(member_copy.os, "open", side_effect=opened), \
             patch.object(member_copy._ProcView, "read", side_effect=AssertionError("never trust presented mountinfo")):
            with self.assertRaisesRegex(Rejected, "真正的 Linux procfs"):
                plan.review()
        self.assertFalse(self.path.exists())

    def test_publish_links_only_the_complete_private_inode_without_rewinding_the_display(self):
        plan = self.plan()
        fd = self.selected.held.fileno()
        os.lseek(fd, 7, os.SEEK_SET)
        detail = plan.review()
        self.assertIn(repr("original/same.bin"), detail)
        self.assertIn(f"{len(self.data):,}", detail)
        self.assertEqual(list(self.dest.iterdir()), [])
        with self.assertRaises(Rejected):
            plan.run()  # a prepared but unconfirmed call must still make no file
        real_link, real_read = os.link, os.pread
        seen = []
        def publish(src, dst, **kwargs):
            source = int(src)
            m = os.fstat(source)
            self.assertEqual((m.st_nlink, m.st_size, stat.S_IMODE(m.st_mode)), (0, len(self.data), 0o600))
            self.assertEqual(list(self.dest.iterdir()), [])
            real_link(src, dst, **kwargs)
            self.assertEqual(self.path.stat().st_ino, m.st_ino)
        def read(fd, size, offset):
            if fd == plan.source: seen.append((offset, offset+size))
            return real_read(fd, size, offset)
        with patch.object(member_copy.os, "link", side_effect=publish), \
             patch.object(member_copy.os, "pread", side_effect=read):
            self.assertIn("已另存并核验", plan.run(confirmed=True))
        # The fixed EOCD tail may touch payload. Ensure there was no walk through
        # the middle of the unrelated 2 MiB member to find/check the selected row.
        elsewhere = len(self.data) + CHUNK
        self.assertFalse(any(start <= elsewhere < stop for start, stop in seen))
        self.assertEqual(os.lseek(fd, 0, os.SEEK_CUR), 7)
        self.assertEqual(self.path.read_bytes(), self.data)
        self.assertEqual(self.archive.read_bytes(), self.original)
        with self.assertRaises(Rejected): plan.run(confirmed=True)

    def test_an_existing_dangling_symlink_and_a_late_collision_are_not_replaced(self):
        self.path.symlink_to("missing-original")
        with self.assertRaisesRegex(Rejected, "已有"):
            self.plan().review()
        self.assertEqual(os.readlink(self.path), "missing-original")
        self.path = self.dest / "another-new-name"
        plan = self.plan()
        plan.review()
        real_link = os.link
        def occupy(*args, **kwargs):
            with self.path.open("xb") as original: original.write(b"somebody else")
            return real_link(*args, **kwargs)
        with patch.object(member_copy.os, "link", side_effect=occupy):
            with self.assertRaisesRegex(Rejected, "没有覆盖"):
                plan.run(confirmed=True)
        self.assertEqual(self.path.read_bytes(), b"somebody else")

    def test_wrong_archive_or_parent_is_not_reselected(self):
        plan = self.plan()
        plan.review()
        original = self.archive.with_name("old.zxs")
        self.archive.rename(original)
        self.archive.write_bytes(self.original)
        with patch.object(member_copy.os, "link", side_effect=AssertionError("cannot publish from a replacement archive")):
            with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertFalse(self.path.exists())
        self.selected.release_display()
        self.selected = Selection.open(self.archive, retain=True)
        self.addCleanup(self.selected.release_display)
        plan = self.plan()
        plan.review()
        prior = self.dest.with_name("former-out")
        self.dest.rename(prior)
        self.dest.mkdir(mode=0o700)
        with patch.object(member_copy.os, "link", side_effect=AssertionError("cannot publish into a rebound parent")):
            with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertEqual(list(self.dest.iterdir()), [])
        self.assertEqual(list(prior.iterdir()), [])
        self.assertEqual(original.read_bytes(), self.original)

    def test_bad_crc_in_original_stream_never_links(self):
        plan = self.plan()
        plan.review()
        from zipworkspace.archive import LOCAL
        with self.archive.open("rb") as raw:
            fixed = LOCAL.unpack(raw.read(LOCAL.size))
        first_body = LOCAL.size + fixed[-2] + fixed[-1]
        real_read = os.pread
        def read(fd, size, offset):
            chunk = real_read(fd, size, offset)
            if chunk and fd == plan.source and offset == first_body:
                return bytes([chunk[0] ^ 1]) + chunk[1:]
            return chunk
        with patch.object(member_copy.os, "pread", side_effect=read), \
             patch.object(member_copy.os, "link", side_effect=AssertionError("must wait for original CRC")):
            with self.assertRaises(zipfile.BadZipFile):
                plan.run(confirmed=True)
        self.assertEqual(list(self.dest.iterdir()), [])

    def test_anonymous_readback_must_match_and_parent_sync_failure_is_indeterminate(self):
        plan = self.plan()
        plan.review()
        real_read = os.pread
        def read(fd, size, offset):
            chunk = real_read(fd, size, offset)
            if fd != plan.source and offset == 0:
                return bytes([chunk[0] ^ 1]) + chunk[1:]
            return chunk
        with patch.object(member_copy.os, "pread", side_effect=read), \
             patch.object(member_copy.os, "link", side_effect=AssertionError("must wait for real readback")):
            with self.assertRaisesRegex(Rejected, "匿名副本读回"):
                plan.run(confirmed=True)
        self.assertEqual(list(self.dest.iterdir()), [])

        plan = self.plan("zero")
        plan.review()
        real_sync = os.fsync
        def sync(fd):
            if stat.S_ISDIR(os.fstat(fd).st_mode):
                self.assertTrue(self.path.is_file())
                raise OSError(errno.EIO, "simulated late parent sync failure")
            real_sync(fd)
        with patch.object(member_copy.os, "fsync", side_effect=sync):
            with self.assertRaisesRegex(Uncertain, "完整副本可能已经出现"):
                plan.run(confirmed=True)
        self.assertEqual(self.path.read_bytes(), b"")
        self.assertEqual(self.archive.read_bytes(), self.original)

    @unittest.skipUnless(find_spec("PySide6"), "optional native Qt controls")
    def test_real_table_and_worker_do_not_publish_after_cancel_or_change_of_page(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import archive_pane
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        window = Window()
        self.addCleanup(window.pool.shutdown)
        pane = window.archive
        pane.show_selection(self.selected)

        def start(confirm):
            pane.show_folder("original")
            pane.files.selectRow(0)
            queued = Future()
            with patch.object(archive_pane.QFileDialog, "getSaveFileName", return_value=(str(self.path), "")) as pick, \
                 patch.object(window.pool, "submit", return_value=queued) as submit, \
                 patch.object(archive_pane, "confirm", side_effect=confirm) as consent:
                self.assertTrue(pane.save_member_action.isEnabled())
                pane.save_member_action.click()
                self.assertTrue(pick.call_args.kwargs["options"] & archive_pane.QFileDialog.Option.ReadOnly)
                self.assertIs(window.pending[0], queued)
                queued.set_result(submit.call_args.args[0]())
                window.poll()
                self.assertEqual(consent.call_count, 1)
                self.assertEqual(submit.call_count, 1)
                self.assertIsNone(window.pending)
                self.assertFalse(self.path.exists())

        start(lambda *_a, **_k: False)
        def change(*_a, **_k):
            pane.show_folder("other")
            pane.files.selectRow(0)  # Same basename from a different folder.
            return True
        start(change)
