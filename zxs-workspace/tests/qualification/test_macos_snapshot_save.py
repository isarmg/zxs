"""Opt-in synthetic macOS qualification; no default disk writes or changes to security labels.

For a fresh-folder candidate supply a dedicated 0700 scratch on local APFS.
Cloning additionally needs a disposable, already-marked synthetic .zxs there.
The fixture never marks an unmarked input or deletes cases or staging files.
"""
from contextlib import ExitStack, closing
from pathlib import Path
import errno
import os
import sys
import stat
import unittest
from unittest.mock import patch
from zipfile import ZIP_STORED, ZipFile

from zipworkspace import macos_copy as copy, macos_read
from zipworkspace._macos_copy_evidence import _RESOURCE
from zipworkspace.desktop.selection import Selection
from zipworkspace.document import named_document, archive_sha
from zipworkspace.model import Rejected, new_id
from zipworkspace.publish import Uncertain


SOURCE = os.environ.get("ZIPWORKSPACE_APFS_SYNTHETIC_MARKED_ZXS")
SCRATCH = os.environ.get("ZIPWORKSPACE_APFS_DISPOSABLE_PRIVATE_DIRECTORY")


@unittest.skipUnless(sys.platform == "darwin" and SOURCE and SCRATCH, "requires an explicitly designated marked synthetic archive and private APFS scratch")
class SnapshotSave(unittest.TestCase):
    def setUp(self):
        self.source, self.root = Path(SOURCE).absolute(), Path(SCRATCH).absolute()
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        with macos_read.no_materialization():
            self.selected = Selection.open(self.source, retain=True)
            self.stack.callback(self.selected.release_display)
            scratch = copy.Folder.choose(self.root)
            self.stack.callback(scratch.close)
            fd = scratch.check()
            copy._attributes(fd, directory=True)
            with named_document(self.source) as doc:
                copy._attributes(doc.fileno())
                self.assertEqual(copy.apfs(fd), copy.apfs(doc.fileno()))
                copy.native().attrs(doc.fileno())
                with copy.native().quarantine(self.source, doc.fileno()): pass
            leaf = "save-" + new_id()
            os.mkdir(leaf, 0o700, dir_fd=fd)  # Under this held, explicitly quiescent scratch; no cleanup.
            self.folder = self.root / leaf
            self.plan = copy.SnapshotCopy(self.selected, self.folder)
            self.stack.callback(self.plan.close)
            self.assertEqual(self.plan.folder.evidence.identity, (
                os.fstat(fd).st_dev, os.stat(leaf, dir_fd=fd, follow_symlinks=False).st_ino))
            self.out = self.folder / self.source.name

    def test_original_bytes_and_system_quarantine_survive_real_noreplace_rename(self):
        self.assertFalse(self.out.exists())
        self.plan.review()
        expected = self.plan.sha
        self.assertFalse(self.out.exists())
        self.plan.run(confirmed=True)
        with macos_read.no_materialization(), named_document(self.source) as src, named_document(self.out) as dst:
            self.assertEqual(archive_sha(dst), expected)
            self.assertNotEqual(os.fstat(src.fileno()).st_ino, os.fstat(dst.fileno()).st_ino)
            api = copy.native()
            self.assertEqual(api.attrs(src.fileno()), api.attrs(dst.fileno()))
            with api.quarantine(self.source, src.fileno()) as a, api.quarantine(self.out, dst.fileno()) as b:
                self.assertTrue(api.equal(a, b))
        self.assertEqual(Selection.open(self.out).browser.commit, self.selected.browser.commit)
        self.assertFalse((self.folder / self.plan.stage).exists())

    def test_standard_zip_keeps_original_bytes_comment_and_system_quarantine(self):
        output = self.out.with_suffix(".zip")
        with ExitStack() as held:
            plan = copy.SnapshotCopy(self.selected, self.folder, as_zip=True)
            held.callback(plan.close)
            display = self.selected.held.fileno()
            os.lseek(display, 11, os.SEEK_SET)
            review = plan.review()
            self.assertIn(str(output), review)
            self.assertIn("归档注释及版本标识全部保留", review)
            self.assertFalse(output.exists())
            plan.run(confirmed=True)
            # A normal ZIP reader, with no managed archive parser or output extraction.
            with ZipFile(self.source) as original, ZipFile(output) as portable:
                self.assertEqual(portable.namelist(), original.namelist())
                self.assertEqual(portable.comment, self.selected.browser.commit.encode())
                self.assertTrue(all(entry.compress_type == ZIP_STORED for entry in portable.infolist()))
                self.assertIsNone(portable.testzip())
            with macos_read.no_materialization(), named_document(output) as final:
                self.assertEqual(archive_sha(final), plan.sha)
                api = copy.native()
                self.assertEqual(api.attrs(display), api.attrs(final.fileno()))
                with api.quarantine(self.source, display) as original, api.quarantine(output, final.fileno()) as actual:
                    self.assertTrue(api.equal(original, actual))
            self.assertEqual(os.lseek(display, 0, os.SEEK_CUR), 11)
            self.assertFalse(self.out.exists())  # only the requested extension is published

    def test_existing_standard_zip_is_left_unchanged_without_a_stage(self):
        existing = self.out.with_suffix(".zip")
        with existing.open("xb") as file: file.write(b"another archive")
        plan = copy.SnapshotCopy(self.selected, self.folder, as_zip=True)
        self.stack.callback(plan.close)
        with self.assertRaisesRegex(Rejected, "目的名称已存在"): plan.review()
        self.assertEqual(existing.read_bytes(), b"another archive")
        self.assertFalse((self.folder / plan.stage).exists())

    def test_copy_cannot_reselect_a_name_without_the_original_display_fd(self):
        detached = Selection.open(self.source)
        self.assertIsNone(detached.held)
        with self.assertRaisesRegex(Rejected, "原浏览句柄"):
            copy.SnapshotCopy(detached, self.folder, as_zip=True)
        self.assertEqual(list(self.folder.iterdir()), [])

    def test_review_and_unconfirmed_action_create_no_file(self):
        self.plan.review()
        with self.assertRaises(Rejected): self.plan.run()
        self.plan.close()
        with self.assertRaises(Rejected): self.plan.run(confirmed=True)
        self.assertEqual(list(self.folder.iterdir()), [])

    def test_late_collision_is_not_replaced_and_unknown_sibling_is_not_removed(self):
        self.plan.review(); real = copy.rename_new_at
        def collide(fd, stage, final):
            incumbent = os.open(final, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600, dir_fd=fd)
            try: os.write(incumbent, b"prior occupant")
            finally: os.close(incumbent)
            real(fd, stage, final)  # The actual exclusive rename must reject the raced name.
        with patch.object(copy, "rename_new_at", side_effect=collide), self.assertRaises(Rejected):
            self.plan.run(confirmed=True)
        self.assertEqual(self.out.read_bytes(), b"prior occupant")
        self.assertTrue((self.folder / self.plan.stage).is_file())

    def test_private_clone_without_confirmed_origin_is_not_published_or_repaired(self):
        self.plan.review(); api = copy.native()
        observed, real = [], api.attrs
        def read(fd):
            if os.fstat(fd).st_ino != self.selected.observed[1]:
                observed.append(fd)
                return {}   # Provider cannot confirm the cloned object's source metadata.
            return real(fd)
        with patch.object(api, "attrs", side_effect=read), patch.object(copy, "rename_new_at") as rename:
            with self.assertRaisesRegex(Rejected, "来源元数据不符"): self.plan.run(confirmed=True)
        self.assertTrue(observed); rename.assert_not_called()
        self.assertFalse(self.out.exists())
        self.assertEqual((self.folder / self.plan.stage).stat().st_size, self.selected.observed[2])

    def test_independent_complete_stage_readback_detects_same_length_change(self):
        self.plan.review(); api = copy.native()
        real = api.clone
        def changed(srcfd, parent, name, flags):
            rc = real(srcfd, parent, name, flags)
            if rc == 0:
                fd = os.open(name, os.O_WRONLY | os.O_NOFOLLOW, dir_fd=parent)
                try: os.pwrite(fd, b"XX", 0)   # Only this new synthetic clone, never the user's source.
                finally: os.close(fd)
            return rc
        with patch.object(api, "clone", side_effect=changed), self.assertRaisesRegex(Rejected, "读回不符"):
            self.plan.run(confirmed=True)
        self.assertFalse(self.out.exists())
        with named_document(self.source) as original:
            self.assertEqual(archive_sha(original), self.plan.sha)

    def test_real_native_stage_name_collision_never_opens_or_replaces_the_unknown_file(self):
        self.plan.review()
        with (self.folder / self.plan.stage).open("xb") as prior: prior.write(b"owned elsewhere")
        with patch.object(copy, "rename_new_at") as rename, self.assertRaisesRegex(Rejected, "临时名称已被占用"):
            self.plan.run(confirmed=True)
        rename.assert_not_called()
        self.assertEqual((self.folder / self.plan.stage).read_bytes(), b"owned elsewhere")
        self.assertFalse(self.out.exists())

    def test_parent_barrier_failure_after_real_rename_is_unknown_and_cannot_be_repeated(self):
        self.plan.review(); renamed = False
        real_rename, real_sync = copy.rename_new_at, os.fsync
        def rename(fd, old, new):
            nonlocal renamed
            real_rename(fd, old, new); renamed = True
        def sync(fd):
            if renamed: raise OSError(errno.EIO, "synthetic final directory barrier failure")
            real_sync(fd)
        with patch.object(copy, "rename_new_at", side_effect=rename), patch.object(os, "fsync", side_effect=sync):
            with self.assertRaisesRegex(Uncertain, "正式名称可能已出现"): self.plan.run(confirmed=True)
        with self.assertRaises(Rejected): self.plan.run(confirmed=True)
        self.assertTrue(self.out.is_file())


@unittest.skipUnless(sys.platform == "darwin" and SCRATCH, "requires explicitly designated private APFS scratch")
class FreshFolderSnapshot(unittest.TestCase):
    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        root = Path(SCRATCH).absolute()
        with macos_read.no_materialization():
            scratch = self.stack.enter_context(closing(copy.Folder.choose(root)))
            fd = scratch.check()
            copy._attributes(fd, directory=True)
            copy.check_literal_path(root, fd)
            copy.apfs(fd)
            copy.require_exclusive_rename(fd)
            case = "new-folder-" + new_id()
            os.mkdir(case, 0o700, dir_fd=fd)
            home = root / case
            self.source, self.dest = home / "source", home / "output"
            self.source.mkdir(mode=0o700)
            self.dest.mkdir(mode=0o700)
            for leaf in ("one", "two", "empty"): (self.source / leaf).mkdir(mode=0o700)
            (self.source / "one" / "equal-name").write_bytes(b"first directory")
            (self.source / "two" / "equal-name").write_bytes(b"another directory")
            folder = self.stack.enter_context(closing(copy.Folder.choose(self.source)))
            self.final = self.dest / "new.zxs"
            self.plan = copy.FolderSnapshot(folder, str(self.final))
            self.stack.callback(self.plan.close)
            self.stage = self.dest / self.plan.stage

    def test_fresh_standard_zip_has_new_first_generation_and_preserves_empty_directory(self):
        self.assertIn(str(self.final), self.plan.review())
        with self.assertRaises(Rejected): self.plan.run()
        self.assertEqual(list(self.dest.iterdir()), [])
        self.plan.run(confirmed=True)
        self.assertFalse(self.stage.exists())
        with ZipFile(self.final) as archive:
            self.assertIsNone(archive.testzip())
            self.assertTrue(all(info.compress_type == ZIP_STORED for info in archive.infolist()))
            self.assertTrue(archive.getinfo("empty/").is_dir())
            self.assertEqual(archive.read("one/equal-name"), b"first directory")
            self.assertEqual(archive.read("two/equal-name"), b"another directory")
        marker = Selection.open(self.final).browser.commit
        self.assertEqual((marker.repo, marker.branch, marker.operation, marker.generation, marker.parent),
                         (self.plan.repo, self.plan.branch, self.plan.operation, 1, None))
        self.assertEqual((self.source / "one" / "equal-name").read_bytes(), b"first directory")

    def test_fresh_unmarked_snapshot_can_open_but_mark_preserving_zip_save_never_stages(self):
        self.plan.review()
        self.plan.run(confirmed=True)
        selected = Selection.open(self.final, retain=True)
        self.stack.callback(selected.release_display)
        self.assertEqual(selected.browser.commit.operation, self.plan.operation)
        original = self.final.read_bytes()
        save = self.stack.enter_context(closing(copy.SnapshotCopy(selected, self.dest, as_zip=True)))
        with patch.object(copy.native(), "clone", side_effect=AssertionError("never clone an unmarked archive")):
            with self.assertRaisesRegex(Rejected, "只接受有原隔离来源的归档"):
                save.review()
            with self.assertRaises(Rejected): save.run(confirmed=True)
        self.assertFalse(save.destination.exists())
        self.assertFalse((self.dest / save.stage).exists())
        self.assertEqual(self.final.read_bytes(), original)

    def test_inaccessible_quarantine_status_fails_before_staging(self):
        with patch.object(copy.native(), "property", return_value=False):
            with self.assertRaisesRegex(Rejected, "不会当成无标记"): self.plan.review()
        self.assertEqual(list(self.dest.iterdir()), [])

    def test_volume_quarantine_refuses_fresh_snapshot_before_staging(self):
        get, set_, actual = macos_read._require()
        def attrs(fd, request, result, size, flags):
            ret = actual(fd, request, result, size, flags)
            if not ret and len(result) == 32 and result[2] & 0x4000:
                result[6] |= 0x400  # Only substitute this synthetic mount flag in the native reply.
            return ret
        with patch.object(macos_read, "_require", return_value=(get, set_, attrs)):
            with self.assertRaisesRegex(Rejected, "卷级隔离"): self.plan.review()
        self.assertEqual(list(self.dest.iterdir()), [])

    def test_a_member_with_extra_metadata_is_not_silently_skipped_or_stripped(self):
        self.plan.review()
        api = copy.native()
        actual = api.names
        marked = (self.source / "one" / "equal-name").stat()
        def names(fd, *args):
            if os.path.samestat(os.fstat(fd), marked):
                return 1  # No attribute is added, changed, read, or removed.
            return actual(fd, *args)
        with patch.object(api, "names", side_effect=names), patch.object(copy, "rename_new_at") as rename:
            with self.assertRaisesRegex(Rejected, "扩展属性"): self.plan.run(confirmed=True)
        rename.assert_not_called()
        self.assertFalse(self.final.exists())
        self.assertTrue(self.stage.is_file())
        self.assertEqual((self.source / "one" / "equal-name").read_bytes(), b"first directory")

    def test_even_empty_resource_fork_refuses_when_absent_from_visible_list(self):
        self.plan.review()
        api = copy.native()
        actual = api.value
        marked = (self.source / "one" / "equal-name").stat()
        def value(fd, name, buffer, length, offset, options):
            if name == _RESOURCE and os.path.samestat(os.fstat(fd), marked):
                self.assertEqual((buffer, length, offset, options), (None, 0, 0, 0))
                return 0
            return actual(fd, name, buffer, length, offset, options)
        with patch.object(api, "value", side_effect=value), patch.object(copy, "rename_new_at") as rename:
            with self.assertRaisesRegex(Rejected, "资源叉"): self.plan.run(confirmed=True)
        rename.assert_not_called()
        self.assertFalse(self.final.exists())

    def test_cancellation_just_before_rename_keeps_private_stage_and_no_final(self):
        self.plan.review()
        begin = self.plan._begin_publication
        def stop_now():
            self.assertTrue(self.plan.cancel())
            begin()
        with patch.object(self.plan, "_begin_publication", side_effect=stop_now), \
             patch.object(copy, "rename_new_at") as rename:
            with self.assertRaisesRegex(Rejected, "已停止"): self.plan.run(confirmed=True)
        rename.assert_not_called()
        self.assertTrue(self.stage.is_file())
        self.assertFalse(self.final.exists())

    def test_late_collision_uses_native_exclusive_rename_and_preserves_both(self):
        self.plan.review()
        real = copy.rename_new_at
        def collide(fd, stage, final):
            with self.final.open("xb") as incumbent: incumbent.write(b"another owner-authorized file")
            real(fd, stage, final)
        with patch.object(copy, "rename_new_at", side_effect=collide):
            with self.assertRaisesRegex(Rejected, "没有覆盖"): self.plan.run(confirmed=True)
        self.assertEqual(self.final.read_bytes(), b"another owner-authorized file")
        self.assertTrue(self.stage.is_file())

    def test_post_rename_parent_sync_failure_does_not_report_success_or_remove_output(self):
        self.plan.review()
        real = os.fsync
        def sync(fd):
            if os.path.samestat(os.fstat(fd), self.dest.stat()) and self.final.exists():
                raise OSError(errno.EIO, "synthetic final parent barrier failure")
            real(fd)
        with patch.object(os, "fsync", side_effect=sync):
            with self.assertRaisesRegex(Uncertain, "正式名称可能已出现"): self.plan.run(confirmed=True)
        self.assertTrue(self.final.is_file())

    def test_late_held_directory_close_error_still_reports_uncertain(self):
        self.plan.review()
        close = self.plan.close
        def close_and_report_error():
            close()  # Release both held folders even in this synthetic case.
            if self.final.exists(): raise OSError(errno.EIO, "synthetic held directory close failure")
        with patch.object(self.plan, "close", side_effect=close_and_report_error):
            with self.assertRaisesRegex(Uncertain, "正式名称可能已出现"): self.plan.run(confirmed=True)
        self.assertTrue(self.final.is_file())
        self.assertFalse(self.stage.exists())
        with self.assertRaises(Rejected): self.plan.run(confirmed=True)


COMPRESSED = os.environ.get("ZIPWORKSPACE_APFS_SYNTHETIC_COMPRESSED_MARKED_ZXS")
# Operator-prepared inert evidence on the synthetic source. Never create, strip or
# decompress a real file merely to satisfy this qualification.
_FIXTURE_XATTR = b"org.zipworkspace.qualification"


@unittest.skipUnless(sys.platform == "darwin" and COMPRESSED and SCRATCH,
                     "requires a pre-existing, system-compressed and quarantined synthetic APFS snapshot")
class ResidentCompressedSource(unittest.TestCase):
    def test_resident_but_compressed_is_not_dataless_and_currently_refuses_without_a_clone(self):
        source = Path(COMPRESSED).absolute()
        with macos_read.no_materialization(), ExitStack() as held:
            folder = copy.Folder.choose(Path(SCRATCH).absolute())
            held.callback(folder.close)
            p = folder.check()
            copy._attributes(p, directory=True)
            doc = held.enter_context(named_document(source))
            fd = doc.fileno()
            api = copy.native()
            self.assertEqual(copy.apfs(fd), copy.apfs(p))  # Includes resident and ownership-enabled checks.
            meta = os.fstat(fd)
            self.assertEqual((meta.st_uid, stat.S_IMODE(meta.st_mode), meta.st_nlink),
                             (os.geteuid(), 0o600, 1))
            self.assertEqual(meta.st_flags, stat.UF_COMPRESSED)
            self.assertFalse(source.name.startswith("."))
            copy.check_spelling(fd, source.name)
            api.no_acl(fd)
            def tag():
                import ctypes
                buf = ctypes.create_string_buffer(64)
                n = api.value(fd, _FIXTURE_XATTR, buf, len(buf), 0, 0)
                self.assertGreater(n, 0)
                return buf.raw[:n]
            self.assertEqual(tag(), b"harmless source qualification")
            selected = Selection.open(source, retain=True)
            held.callback(selected.release_display)
            expected = archive_sha(doc)  # Whole logical ZIP read, not a measure of clone creation time.
            with self.assertRaises(FileNotFoundError):
                os.stat(source.name, dir_fd=p, follow_symlinks=False)
            with api.quarantine(source, fd) as before, patch.object(api, "clone") as clone:
                # st_flags=UF_COMPRESSED is outside this version's admitted ordinary source.
                # This is a negative policy case, NOT evidence of successful compressed copying.
                with self.assertRaisesRegex(Rejected, "0600 普通文件"):
                    unexpected = copy.SnapshotCopy(selected, folder.evidence.path)
                    unexpected.close()  # Do not leak if future policy starts admitting this case.
                clone.assert_not_called()
                with api.quarantine(source, fd) as after: self.assertTrue(api.equal(before, after))
            with self.assertRaises(FileNotFoundError):
                os.stat(source.name, dir_fd=p, follow_symlinks=False)
            self.assertEqual((tag(), archive_sha(doc), os.fstat(fd).st_flags),
                             (b"harmless source qualification", expected, stat.UF_COMPRESSED))
