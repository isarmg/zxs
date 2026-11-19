"""Explicit synthetic macOS qualification; no default disk writes or changes to security labels.

Supply a disposable, already-marked, non-executable synthetic .zxs and a separate
dedicated 0700 scratch on its local APFS volume. The fixture never marks an unmarked
input and never deletes the resulting cases, occupied names or private staging files.
"""
from contextlib import ExitStack
from pathlib import Path
import errno
import os
import sys
import unittest
from unittest.mock import patch

from zipworkspace import macos_copy as copy, macos_read
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
            self.selected = Selection.open(self.source)
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
