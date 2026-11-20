"""Opt-in ordinary-user ext4/XFS, no device changes; leaves only synthetic cases for inspection."""
from concurrent.futures import Future
from contextlib import ExitStack
import errno
import hashlib
from importlib.util import find_spec
import os
from pathlib import Path
from types import SimpleNamespace
import stat
import sys
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace.desktop import member_copy
from zipworkspace.desktop.selection import PlainSelection, Selection, StoreSelection
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
        self.private = case / "scratch"
        self.dest.mkdir(mode=0o700)
        self.private.mkdir(mode=0o700)
        self.path = self.dest / "saved.bin"
        self.data = b"\x00original\xff" * 110_000  # crosses a real copy block and is never opened/executed
        self.archive = case / "chosen.zxs"
        tree = Manifest()
        with self.archive.open("xb") as raw, zipfile.ZipFile(raw, "w") as archive:
            for name, data in (("original/same.bin", self.data), ("other/same.bin", b"x" * (2*CHUNK)),
                               ("original/zero", b""), ("tree/nested/value", b"only this subtree"),
                               ("tree/empty/", None), ("tree/zero", b""),
                               ("tree2/foreign", b"lookalike prefix"), ("empty-root/", None)):
                if data is None:
                    tree.add(name[:-1], True, 0, None)
                else:
                    tree.add(name, False, len(data), hashlib.sha256(data).hexdigest())
                archive.writestr(name, data or b"")
            archive.comment = Commit("1"*32, "2"*32, 1, "3"*32, None, tree.digest()).encode()
        self.original = self.archive.read_bytes()
        self.selected = Selection.open(self.archive, retain=True)
        self.addCleanup(self.selected.release_display)

    def plan(self, name="same.bin"):
        entry = self.selected.browser.entry("original", name)
        plan = member_copy.MemberCopy(self.selected, "original", entry, str(self.path))
        self.addCleanup(plan.close)
        return plan

    def zip_plan(self, name="portable.zip"):
        plan = member_copy.ZipCopy(self.selected, str(self.dest/name))
        self.addCleanup(plan.close)
        return plan

    def test_managed_zip_copy_round_trips_through_standard_reader_and_plain_extraction(self):
        plan = self.zip_plan()
        fd = self.selected.held.fileno()
        os.lseek(fd, 11, os.SEEK_SET)
        details = plan.review()
        self.assertIn("逐字节另存原 Store ZIP", details)
        self.assertFalse(plan.path.exists())
        with self.assertRaises(Rejected): plan.run()
        real_link = os.link
        def link(source, name, **kwargs):
            m = os.fstat(int(source))
            self.assertEqual((m.st_size, m.st_nlink), (len(self.original), 0))
            self.assertFalse(plan.path.exists())
            real_link(source, name, **kwargs)
        with patch.object(member_copy.os, "link", side_effect=link) as native:
            self.assertIn("已另存并核验", plan.run(confirmed=True))
            native.assert_called_once()
        self.assertEqual(os.lseek(fd, 0, os.SEEK_CUR), 11)
        self.assertEqual(plan.path.read_bytes(), self.original)
        self.assertEqual(self.archive.read_bytes(), self.original)
        self.assertEqual(stat.S_IMODE(plan.path.stat().st_mode), 0o600)
        with zipfile.ZipFile(plan.path) as ordinary:
            self.assertIsNone(ordinary.testzip())
            self.assertEqual(ordinary.read("original/same.bin"), self.data)
            self.assertTrue(ordinary.getinfo("empty-root/").is_dir())
            self.assertEqual(ordinary.read("other/same.bin"), b"x" * (2*CHUNK))
            self.assertEqual(ordinary.comment, self.selected.browser.commit.encode())

        # Feed that exact output (not a second generated fixture) to the real
        # ordinary ZIP preview and directory writer, without interpreting its marker.
        selected_zip = PlainSelection.open(str(plan.path))
        self.addCleanup(selected_zip.release_display)
        target = self.dest/"round-trip"
        extracted = member_copy.PlainZipCopy(selected_zip, str(target))
        self.addCleanup(extracted.close)
        extracted.pick_private(str(self.private)); extracted.review()
        extracted.run(confirmed=True)
        original_note, other_note = target/"original/same.bin", target/"other/same.bin"
        self.assertEqual(original_note.read_bytes(), self.data)
        self.assertEqual(other_note.read_bytes(), b"x" * (2*CHUNK))
        self.assertEqual(list((target/"empty-root").iterdir()), [])
        before_target, before_note = target.stat(), original_note.stat()

        again = member_copy.PlainZipCopy(selected_zip, str(target))
        self.addCleanup(again.close)
        again.pick_private(str(self.private))
        with patch.object(member_copy.private_stage, "new_child", side_effect=AssertionError("must reject before staging")):
            with self.assertRaises(Rejected): again.review()
            with self.assertRaises(Rejected): again.run(confirmed=True)
        self.assertTrue(os.path.samestat(before_target, target.stat()))
        self.assertTrue(os.path.samestat(before_note, original_note.stat()))
        self.assertEqual(original_note.read_bytes(), self.data)
        self.assertEqual(other_note.read_bytes(), b"x" * (2*CHUNK))
        self.assertEqual(list((target/"empty-root").iterdir()), [])
        self.assertEqual(plan.path.read_bytes(), self.original)
        self.assertEqual(self.archive.read_bytes(), self.original)
        self.assertEqual(list(self.private.iterdir()), [])

    def test_space_advisory_uses_the_original_output_mount_and_does_not_reserve_or_block(self):
        archive = self.zip_plan()
        def tiny(fd):
            self.assertTrue(os.path.samestat(os.fstat(fd), self.dest.stat()))
            return SimpleNamespace(f_frsize=1, f_blocks=10, f_bfree=2, f_bavail=0)
        with patch.object(member_copy.os, "fstatvfs", side_effect=tiny) as observed:
            hint = archive.review()
            observed.assert_called_once()
        self.assertIn(f"新增的文件逻辑量：{len(self.original):,} B", hint)
        self.assertIn("当前报告普通用户可用 0 B", hint)
        self.assertIn("可能不足", hint)
        self.assertFalse(archive.path.exists())
        archive.run(confirmed=True)  # A reporting estimate is neither a reservation nor a hard refusal.
        self.assertEqual(archive.path.read_bytes(), self.original)

        subtree = self.subtree_plan()
        def unavailable(fd):
            self.assertTrue(os.path.samestat(os.fstat(fd), self.private.stat()))
            raise OSError(errno.EIO, "unavailable capacity estimate")
        with patch.object(member_copy.os, "fstatvfs", side_effect=unavailable) as observed:
            self.assertIn("当前可用量未知", subtree.review())
            observed.assert_called_once()
        self.assertFalse(subtree.path.exists())
        self.assertEqual(list(self.private.iterdir()), [])

    def test_copy_can_cancel_after_copying_but_before_complete_zip_readback(self):
        plan = self.zip_plan()
        plan.review()
        real = member_copy.read_managed_document
        def readback(doc, **kwargs):
            self.assertTrue(plan.cancel())
            self.assertFalse(plan.path.exists())
            return real(doc, **kwargs)  # The actual guarded readback observes cancellation.
        with patch.object(member_copy, "read_managed_document", side_effect=readback), \
             patch.object(member_copy.os, "link", side_effect=AssertionError("must not publish a cancelled copy")):
            with self.assertRaisesRegex(member_copy.CancelledCopy, "未发布"):
                plan.run(confirmed=True)
        self.assertFalse(plan.path.exists())
        self.assertEqual(self.archive.read_bytes(), self.original)
        self.assertFalse(plan.cancel())  # The consumed plan can't become a new operation.

    def test_cancellation_cannot_undo_the_single_link_once_committed_to_it(self):
        plan = self.zip_plan()
        plan.review()
        real = os.link
        def link(*args, **kwargs):
            self.assertFalse(plan.cancel())
            return real(*args, **kwargs)
        with patch.object(member_copy.os, "link", side_effect=link) as native:
            plan.run(confirmed=True)
            native.assert_called_once()
        self.assertEqual(plan.path.read_bytes(), self.original)

    @unittest.skipUnless(find_spec("PySide6"), "optional native Qt controls")
    def test_cancel_button_stays_reachable_when_only_archive_tabs_are_disabled(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        self.addCleanup(win.close)
        pending, taps = Future(), []
        with patch.object(win.pool, "submit", return_value=pending):
            win.job("复制原包", lambda: self.fail("fixture only controls the Future"),
                    lambda _: self.fail("cancelled is not a completed result"),
                    cancel=lambda: taps.append(True) or True)
        self.assertFalse(win.tabs.isEnabled())
        self.assertTrue(win.transfer_cancel.isEnabled())
        self.assertFalse(win.transfer_cancel.isHidden())
        win.transfer_cancel.click()
        self.assertEqual(taps, [True])
        self.assertIn("等待正在进行的系统调用", win.note.text())
        pending.set_exception(member_copy.CancelledCopy("已取消，未发布"))
        win.poll()
        self.assertTrue(win.transfer_cancel.isHidden())
        self.assertTrue(win.tabs.isEnabled())
        self.assertIn("未发布", win.note.text())

    def test_raw_copy_with_a_damaged_member_never_links_even_if_the_source_name_is_unchanged(self):
        plan = self.zip_plan()
        plan.review()
        from zipworkspace.archive import LOCAL
        with self.archive.open("rb") as fh:
            fixed = LOCAL.unpack(fh.read(LOCAL.size))
        body = LOCAL.size + fixed[-2] + fixed[-1]
        actual = os.pread
        def damaged(fd, size, offset):
            data = actual(fd, size, offset)
            if fd == plan.source and offset <= body < offset+len(data):
                at = body-offset
                return data[:at]+bytes([data[at]^1])+data[at+1:]
            return data
        with patch.object(member_copy.os, "pread", side_effect=damaged), \
             patch.object(member_copy.os, "link", side_effect=AssertionError("reject before exposing a ZIP")):
            with self.assertRaises(zipfile.BadZipFile): plan.run(confirmed=True)
        self.assertEqual(list(self.dest.iterdir()), [])
        self.assertEqual(self.archive.read_bytes(), self.original)

    def plain_plan(self, name="plain-output"):
        original = self.archive.with_name("plain.zip")
        with original.open("xb") as raw, zipfile.ZipFile(raw, "w") as archive:
            archive.comment = b"a plain ZIP; no Workspace identity"
            info = zipfile.ZipInfo("folder/note")
            info.external_attr = (stat.S_IFREG | 0o755) << 16
            archive.writestr(info, b"file payload")
            archive.writestr("empty/", b"")
            archive.writestr("zero", b"")
        selected = PlainSelection.open(str(original))
        self.addCleanup(selected.release_display)
        plan = member_copy.PlainZipCopy(selected, str(self.dest/name))
        plan.pick_private(str(self.private))
        self.addCleanup(plan.close)
        return original, selected, plan

    def test_plain_zip_explicitly_extracts_all_verified_bytes_into_one_new_directory(self):
        original, selected, plan = self.plain_plan()
        before = original.read_bytes()
        display = selected.held.fileno()
        os.lseek(display, 7, os.SEEK_SET)
        details = plan.review()
        self.assertIn(selected.preview.tree, details)
        self.assertFalse(plan.path.exists())
        self.assertEqual(list(self.private.iterdir()), [])
        with self.assertRaises(Rejected): plan.run()
        real = member_copy.rename_new_at
        def publish(parent, name, target, **kw):
            stage = self.private/name
            self.assertFalse(plan.path.exists())
            self.assertEqual((stage/"folder/note").read_bytes(), b"file payload")
            self.assertEqual(list((stage/"empty").iterdir()), [])
            self.assertEqual(stat.S_IMODE((stage/"folder/note").stat().st_mode), 0o600)
            self.assertEqual((stage/"zero").read_bytes(), b"")
            self.assertTrue(os.path.samestat(kw["expected_source"], stage.stat()))
            real(parent, name, target, **kw)
        with patch.object(member_copy, "rename_new_at", side_effect=publish) as native:
            self.assertIn("已另存并核验目录", plan.run(confirmed=True))
            native.assert_called_once()
        self.assertEqual(os.lseek(display, 0, os.SEEK_CUR), 7)
        self.assertEqual(original.read_bytes(), before)
        self.assertEqual(sorted(x.name for x in plan.path.iterdir()), ["empty", "folder", "zero"])
        self.assertEqual(list(self.private.iterdir()), [])

    def test_plain_zip_replacement_and_crc_failure_never_publish_an_extraction(self):
        original, _, plan = self.plain_plan()
        plan.review()
        from zipworkspace.archive import LOCAL
        with original.open("rb") as raw:
            fields = LOCAL.unpack(raw.read(LOCAL.size))
            body = LOCAL.size + fields[-2] + fields[-1]
        real = os.pread
        def corrupt(fd, n, offset):
            data = real(fd, n, offset)
            if fd == plan.source and offset <= body < offset + len(data):
                i = body-offset
                return data[:i] + bytes([data[i]^1]) + data[i+1:]
            return data
        with patch.object(member_copy.os, "pread", side_effect=corrupt), \
             patch.object(member_copy, "rename_new_at", side_effect=AssertionError("never publish damaged data")):
            with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertFalse(plan.path.exists())
        self.assertTrue((self.private/plan.stage_name).is_dir())

        # A fresh inspection still cannot adopt an equal-bytes pathname replacement.
        selected = PlainSelection.open(str(original))
        self.addCleanup(selected.release_display)
        following = member_copy.PlainZipCopy(selected, str(self.dest/"second"))
        self.addCleanup(following.close)
        following.pick_private(str(self.private)); following.review()
        bytes_before = original.read_bytes()
        original.rename(original.with_name("prior.zip"))
        original.write_bytes(bytes_before)
        with patch.object(member_copy.private_stage, "new_child", side_effect=AssertionError("reject replaced original first")):
            with self.assertRaises(Rejected): following.run(confirmed=True)
        self.assertFalse(following.path.exists())

    def history_plan(self):
        source_store = self.archive.parent / "history-origin"
        source_store.mkdir(mode=0o700)
        manifest = Manifest()
        manifest.add("file", False, 4, hashlib.sha256(b"body").hexdigest())
        repo, main, branch, tree = "d"*32, "e"*32, "f"*32, manifest.digest()
        first = Commit(repo, main, 1, "1"*32, None, tree)
        parent = Commit(repo, main, 2, "2"*32, first.operation, tree)
        tip = Commit(repo, branch, 1, "3"*32, parent.operation, tree)
        after = Commit(repo, main, 3, "4"*32, parent.operation, tree)
        for marker in (first, parent, tip, after):
            with (source_store/marker.filename).open("xb") as raw, zipfile.ZipFile(raw, "w") as z:
                z.writestr("file", b"body")
                z.comment = marker.encode()
        selected = Selection.open(source_store/tip.filename, retain=True)
        self.addCleanup(selected.release_display)
        choice = StoreSelection.open(source_store)
        catalog = selected.catalog(choice)
        plan = member_copy.HistoryCopy(selected, choice, catalog, str(self.dest/"local-store"))
        self.addCleanup(plan.close)
        plan.pick_private(str(self.private))
        return plan, source_store, selected, choice, catalog, (first, parent, tip, after)

    def test_copy_only_the_original_complete_ancestry_as_one_new_store(self):
        plan, original, selected, _, _, (first, parent, tip, unrelated) = self.history_plan()
        display = selected.held.fileno()
        os.lseek(display, 5, os.SEEK_SET)
        files = {p.name: p.read_bytes() for p in original.iterdir()}
        details = plan.review()
        self.assertIn(tip.operation, details)
        self.assertFalse(plan.path.exists())
        self.assertEqual(list(self.private.iterdir()), [])
        with self.assertRaises(Rejected): plan.run()
        included = {first.filename, parent.filename, tip.filename}
        real = member_copy.rename_new_at
        def publish(pfd, leaf, dest, **kw):
            stage = self.private/leaf
            self.assertFalse(plan.path.exists())
            self.assertEqual({p.name for p in stage.iterdir()}, included)
            self.assertTrue(os.path.samestat(kw["expected_source"], stage.stat()))
            real(pfd, leaf, dest, **kw)
        with patch.object(member_copy, "rename_new_at", side_effect=publish) as rename:
            plan.run(confirmed=True)
            rename.assert_called_once()
        self.assertEqual(os.lseek(display, 0, os.SEEK_CUR), 5)
        self.assertEqual({p.name:p.read_bytes() for p in plan.path.iterdir()}, {n:files[n] for n in included})
        self.assertEqual({p.name:p.read_bytes() for p in original.iterdir()}, files)
        self.assertFalse((plan.path/unrelated.filename).exists())
        local_store = StoreSelection.open(plan.path)
        local = Selection.open(plan.path/tip.filename)
        self.assertEqual(local.catalog(local_store).select_branch(tip.branch).marker, tip)

    def test_original_ancestor_cannot_be_replaced_or_omitted_and_never_creates_stage(self):
        plan, original, selected, choice, _, (first, _, tip, _) = self.history_plan()
        plan.review()
        leaf = original/first.filename
        old = original/"removed"
        leaf.rename(old)
        leaf.write_bytes(old.read_bytes())
        with patch.object(member_copy.private_stage, "new_child", side_effect=AssertionError("must check original slots first")):
            with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertFalse(plan.path.exists())
        self.assertEqual(list(self.private.iterdir()), [])
        leaf.rename(original/"replacement")
        missing = selected.catalog(choice)
        self.assertEqual(missing.history(tip.operation)[1], first.operation)
        with self.assertRaises(Rejected):
            member_copy.HistoryCopy(selected, choice, missing, str(self.dest/"must-not-start"))

    def subtree_plan(self, name="tree", *, output="tree-copy", scratch=None):
        entry = self.selected.browser.entry("", name)
        plan = member_copy.DirectoryCopy(self.selected, "", entry, str(self.dest/output))
        self.addCleanup(plan.close)
        plan.pick_private(str(self.private if scratch is None else scratch))
        return plan

    def test_one_native_rename_after_complete_subtree_and_empty_roots(self):
        plan = self.subtree_plan()
        detail = plan.review()
        stage = self.private / plan.stage_name
        self.assertIn(repr(str(stage)), detail)
        self.assertEqual(list(self.private.iterdir()), [])
        self.assertEqual(list(self.dest.iterdir()), [])
        with self.assertRaises(Rejected): plan.run()
        real_move, real_read = member_copy.rename_new_at, os.pread
        seen = []
        def move(src_fd, src, dst, *, expected_source, target_fd):
            self.assertEqual((src, dst), (plan.stage_name, plan.path.name))
            self.assertTrue(os.path.samestat(os.fstat(src_fd), self.private.stat()))
            self.assertTrue(os.path.samestat(os.fstat(target_fd), self.dest.stat()))
            self.assertTrue(os.path.samestat(expected_source, stage.stat()))
            self.assertFalse(plan.path.exists())
            self.assertEqual((stage / "nested/value").read_bytes(), b"only this subtree")
            self.assertEqual(list((stage / "empty").iterdir()), [])
            self.assertEqual((stage / "zero").read_bytes(), b"")
            real_move(src_fd, src, dst, expected_source=expected_source, target_fd=target_fd)
            self.assertTrue(os.path.samestat(expected_source, plan.path.stat()))
        def read(fd, size, offset):
            if fd == plan.source: seen.append((offset, offset+size))
            return real_read(fd, size, offset)
        display = self.selected.held.fileno()
        os.lseek(display, 9, os.SEEK_SET)
        with patch.object(member_copy, "rename_new_at", side_effect=move) as moved, \
             patch.object(member_copy.os, "pread", side_effect=read):
            self.assertIn("已另存并核验目录", plan.run(confirmed=True))
            self.assertEqual(moved.call_count, 1)
        self.assertFalse(any(a <= len(self.data) + CHUNK < b for a, b in seen))
        self.assertEqual(os.lseek(display, 0, os.SEEK_CUR), 9)
        self.assertFalse(stage.exists())
        self.assertEqual(sorted(x.name for x in plan.path.iterdir()), ["empty", "nested", "zero"])
        self.assertEqual(self.archive.read_bytes(), self.original)

        empty = self.subtree_plan("empty-root", output="empty-copy")
        empty.review()
        empty.run(confirmed=True)
        self.assertEqual(list(empty.path.iterdir()), [])
        self.assertEqual(stat.S_IMODE(empty.path.stat().st_mode), 0o700)

    def test_directory_cancelled_during_actual_private_readback_never_renames(self):
        plan = self.subtree_plan()
        plan.review()
        stage = self.private/plan.stage_name
        original = os.read
        cancelled = []
        def read(fd, n):
            data = original(fd, n)
            if data == b"only this subtree" and stage.is_dir():
                self.assertTrue(plan.cancel())
                cancelled.append(True)
            return data
        with patch.object(member_copy.os, "read", side_effect=read), \
             patch.object(member_copy, "rename_new_at", side_effect=AssertionError("no cancelled directory is public")):
            with self.assertRaisesRegex(member_copy.CancelledCopy, "可能仍有准备材料"):
                plan.run(confirmed=True)
        self.assertEqual(cancelled, [True])
        self.assertFalse(plan.path.exists())
        self.assertEqual((stage/"nested/value").read_bytes(), b"only this subtree")
        self.assertEqual(self.archive.read_bytes(), self.original)

    def test_cancel_before_starting_an_ordinary_extraction_does_not_create_a_private_stage(self):
        original, _, plan = self.plain_plan()
        stored = original.read_bytes()
        plan.review()
        self.assertTrue(plan.cancel())
        with patch.object(member_copy.private_stage, "new_child", side_effect=AssertionError("already cancelled")):
            with self.assertRaises(member_copy.CancelledCopy): plan.run(confirmed=True)
        self.assertEqual(list(self.private.iterdir()), [])
        self.assertFalse(plan.path.exists())
        self.assertEqual(original.read_bytes(), stored)

    def test_subtree_corrupt_store_body_can_leave_private_stage_but_not_a_final_name(self):
        plan = self.subtree_plan()
        plan.review()
        with zipfile.ZipFile(self.archive) as structure:
            offset = structure.getinfo("tree/nested/value").header_offset
        from zipworkspace.archive import LOCAL
        with self.archive.open("rb") as raw:
            raw.seek(offset)
            fields = LOCAL.unpack(raw.read(LOCAL.size))
        body = offset + LOCAL.size + fields[-2] + fields[-1]
        real_read = os.pread
        def read(fd, size, offset):
            data = real_read(fd, size, offset)
            if data and fd == plan.source and offset == body:
                return b"\x00" + data[1:]
            return data
        with patch.object(member_copy.os, "pread", side_effect=read), \
             patch.object(member_copy, "rename_new_at", side_effect=AssertionError("never publish a bad CRC")):
            with self.assertRaisesRegex(Rejected, "Bad CRC") as refused:
                plan.run(confirmed=True)
        self.assertIn(plan.stage_name, str(refused.exception))
        self.assertTrue((self.private / plan.stage_name / "empty").is_dir())
        self.assertFalse(plan.path.exists())

    def test_rebound_private_or_nested_final_parent_is_not_accepted(self):
        nested = self.dest / "bad-scratch"
        nested.mkdir(mode=0o700)
        with self.assertRaises(Rejected): self.subtree_plan(scratch=nested).review()
        plan = self.subtree_plan()
        plan.review()
        original = self.private.with_name("initial-scratch")
        self.private.rename(original)
        self.private.mkdir(mode=0o700)
        with patch.object(member_copy, "rename_new_at", side_effect=AssertionError("do not adopt a replacement")):
            with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertEqual(list(original.iterdir()), [])
        self.assertEqual(list(self.private.iterdir()), [])
        self.assertFalse(plan.path.exists())

    def test_late_collision_with_an_existing_empty_directory_is_no_replace(self):
        plan = self.subtree_plan()
        plan.review()
        real = member_copy.rename_new_at
        protected = []
        def compete(*args, **kwargs):
            plan.path.mkdir(mode=0o700)
            protected.append(plan.path.stat().st_ino)
            return real(*args, **kwargs)
        with patch.object(member_copy, "rename_new_at", side_effect=compete):
            with self.assertRaisesRegex(Rejected, "没有覆盖"): plan.run(confirmed=True)
        self.assertEqual(plan.path.stat().st_ino, protected[0])
        self.assertEqual(list(plan.path.iterdir()), [])
        self.assertEqual((self.private / plan.stage_name / "nested/value").read_bytes(), b"only this subtree")

    def test_final_name_on_another_mount_is_not_acknowledged_even_with_equal_inode(self):
        plan = self.subtree_plan()
        plan.review()
        real_open, real_id = os.open, member_copy.mount_id
        latest = None
        def opened(name, flags, *args, **kwargs):
            nonlocal latest
            fd = real_open(name, flags, *args, **kwargs)
            if name == plan.path.name and flags & os.O_PATH:
                latest = fd
            return fd
        def mount(fd, *, read_proc=None):
            if fd == latest:
                self.assertTrue(os.path.samestat(os.fstat(fd), plan.path.stat()))
                return b"not-the-original-mount"
            return real_id(fd, read_proc=read_proc)
        with patch.object(member_copy.os, "open", side_effect=opened), \
             patch.object(member_copy, "mount_id", side_effect=mount):
            with self.assertRaisesRegex(Uncertain, "不在选定的实际挂载"):
                plan.run(confirmed=True)
        self.assertEqual((plan.path / "nested/value").read_bytes(), b"only this subtree")

    def test_failed_source_or_final_parent_sync_after_move_never_reports_success(self):
        for name, fail in (("source", self.private), ("target", self.dest)):
            with self.subTest(name=name):
                plan = self.subtree_plan(output=name)
                plan.review()
                actual = os.fsync
                calls = []
                def sync(fd):
                    if any(os.path.samestat(os.fstat(fd), p.stat()) for p in (self.private, self.dest)):
                        self.assertTrue(plan.path.is_dir())
                        calls.append(fd)
                    if os.path.samestat(os.fstat(fd), fail.stat()): raise OSError(errno.EIO, name)
                    actual(fd)
                with patch.object(os, "fsync", side_effect=sync):
                    with self.assertRaisesRegex(Uncertain, "完整副本可能已经出现"): plan.run(confirmed=True)
                self.assertEqual(len(calls), 2 if name == "source" else 1)
                self.assertEqual((plan.path / "nested/value").read_bytes(), b"only this subtree")
                self.assertFalse((self.private / plan.stage_name).exists())

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

        pane.show_folder("")
        row = next(i for i, (_, item) in enumerate(pane.files.rows.rows) if item.name == "tree")
        pane.files.selectRow(row)
        self.assertIn("文件夹", pane.save_member_action.text())
        queued = Future()
        with patch.object(archive_pane.QFileDialog, "getSaveFileName", return_value=(str(self.path), "")), \
             patch.object(archive_pane.QFileDialog, "getExistingDirectory", return_value=str(self.private)), \
             patch.object(archive_pane, "confirm", return_value=False) as consent, \
             patch.object(window.pool, "submit", return_value=queued) as submit:
            pane.save_member_action.click()
            queued.set_result(submit.call_args.args[0]())
            window.poll()
            self.assertIn("暂存目录由本人独占", consent.call_args.kwargs["attestation"])
            self.assertEqual(submit.call_count, 1)
            self.assertFalse(self.path.exists())
            self.assertEqual(list(self.private.iterdir()), [])
