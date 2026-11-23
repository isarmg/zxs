"""Source fixtures for the editable data plane. No pyfuse3, mount, or transport."""
from contextlib import ExitStack
import errno
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch


@unittest.skipUnless(sys.platform == "linux", "qualified local Linux test files only")
class LocalEditFacts(unittest.TestCase):
    def setUp(self):
        from zipworkspace import linux_edit, worktree
        from zipworkspace.desktop.live_checkout import LiveCheckout
        from zipworkspace.model import Rejected

        self.edit, self.rejected = linux_edit, Rejected
        stack = ExitStack()
        self.addCleanup(stack.close)
        base = Path(stack.enter_context(tempfile.TemporaryDirectory(dir=Path.home())))
        self.work, self.state, self.store = (base / n for n in ("work", "state", "store"))
        for path in (self.work, self.state, self.store): path.mkdir(mode=0o700)

        def selected(path):
            fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            stack.callback(os.close, fd)
            return fd

        fds = tuple(selected(path) for path in (self.work, self.state, self.store))
        try:
            proc = linux_edit.ProcView(stack)
            uid = linux_edit._ordinary_user(read_proc=proc.read)
            linux_edit.owned_directory(stack, self.work, uid, proc.read)
            linux_edit.local_mount(fds[0], read_proc=proc.read)
        except (OSError, Rejected) as err:
            self.skipTest("home is not an admitted ordinary Linux working directory: " + str(err))
        self.live = LiveCheckout(self.work, self.state, self.store, "1" * 32, "2" * 32,
                                 *fds, initial_branch="3" * 32)
        stack.callback(self.live.close)
        worktree.create_catalog(self.work, self.store, self.state, None, self.live.instance,
                                checked_root=lambda: os.fstat(fds[0]), live=self.live)

    def owner(self):
        return self.edit.edit_live_worktree(self.live, exclusive=True)

    def test_stop_waits_for_original_writer_then_lends_a_real_frozen_root(self):
        (self.work / "story").write_bytes(b"one")
        with self.owner() as editing:
            ino, _ = editing.lookup(self.edit.ROOT, b"story")
            bound = editing.bind_file(ino)
            writer = editing.open(ino, os.O_RDWR | os.O_APPEND)
            editing.forget([(ino, 1)])  # Open capability, not the directory lookup, keeps this inode held.
            self.assertEqual(editing.stop_writes(), 1)
            with self.assertRaises(OSError) as busy: editing.open(ino, os.O_WRONLY)
            self.assertEqual(busy.exception.errno, errno.EBUSY)
            with self.assertRaises(OSError) as active:
                with editing.freeze(): self.fail("a live writer did not drain")
            self.assertEqual(active.exception.errno, errno.EBUSY)
            self.assertEqual(editing.write(writer, 0, b" two"), 4)
            self.assertEqual(editing.read(writer, 0, 99), b"one two")
            editing.fsync(writer)
            editing.release(writer)
            with editing.freeze() as cut:
                self.assertEqual(len(cut.changed_files), 1)
                self.assertIs(cut.changed_files[0], bound)
                with cut.open_file(bound) as image:
                    self.assertEqual((image.bound.path, image.size), ("story", 7))
                    m = os.fstat(image.fd)
                    self.assertEqual((m.st_dev, m.st_ino), bound.local_identity)
                    self.assertEqual(os.pread(image.fd, 99, 0), b"one two")
                    self.assertEqual(self.edit.fcntl.fcntl(image.fd, self.edit.fcntl.F_GETFL)
                                     & os.O_ACCMODE, os.O_RDONLY)
                    image.check_file()
                with self.assertRaises(OSError): image.check_file()
                cut.check_work()
                with self.assertRaises(OSError) as stopped:
                    editing.create(self.edit.ROOT, b"new", 0o600, os.O_RDWR)
                self.assertEqual(stopped.exception.errno, errno.EBUSY)
            with self.assertRaises(OSError): cut.check_work()

    def test_create_never_replaces_and_directory_cookies_stay_with_original_open(self):
        (self.work / "Note").write_bytes(b"incumbent")
        with self.owner() as editing:
            root = self.edit.ROOT
            old_listing = editing.opendir(root)
            for alias in (b"Note", b"note"):
                with self.assertRaises(FileExistsError):
                    editing.create(root, alias, 0o600, os.O_RDWR)
            for path in (b"../other", b"/outside", b".", b"bad\\name"):
                with self.assertRaises(OSError):
                    editing.create(root, path, 0o600, os.O_RDWR)
            fh, (ino, _) = editing.create(root, b"Fresh", 0o666, os.O_RDWR)
            self.assertEqual(editing.write(fh, 0, b"first"), 5)
            editing.truncate(ino, 2, fh)
            editing.release(fh)
            self.assertEqual((self.work / "Fresh").read_bytes(), b"fi")
            self.assertEqual((self.work / "Note").read_bytes(), b"incumbent")
            rows = []
            editing.readdir(old_listing, 0, lambda name, entry, cookie: not rows.append((name, cookie)))
            self.assertEqual(rows, [(b"Note", 1)])  # No values skipped or repeated by the new creation.
            editing.releasedir(old_listing)

    def test_wrong_root_types_and_same_name_replacement_do_not_get_write_authority(self):
        (self.work / "entry").write_bytes(b"old")
        outside = self.state / "outside"
        outside.write_bytes(b"untouched")
        (self.work / "link").symlink_to(outside)
        os.link(outside, self.work / "hard")
        with self.owner() as editing:
            root = self.edit.ROOT
            for alias in (b"link", b"hard"):
                with self.assertRaises((OSError, self.rejected)): editing.lookup(root, alias)
            ino, _ = editing.lookup(root, b"entry")
            fh = editing.open(ino, os.O_RDWR)
            (self.work / "entry").rename(self.work / "original")
            (self.work / "entry").write_bytes(b"replacement")
            with self.assertRaises((OSError, self.rejected)): editing.write(fh, 0, b"bad")
            editing.release(fh)
            self.assertEqual((self.work / "original").read_bytes(), b"old")
            self.assertEqual((self.work / "entry").read_bytes(), b"replacement")
            self.assertEqual(outside.read_bytes(), b"untouched")
            editing.stop_writes()
            with self.assertRaises(OSError) as refused:
                with editing.freeze(): self.fail("an observed original-name loss was ignored")
            self.assertEqual(refused.exception.errno, errno.EIO)

    def test_exact_cut_excludes_track_only_and_does_not_adopt_an_outside_edit(self):
        unchanged, changed = self.work / "untouched", self.work / "edited"
        unchanged.write_bytes(b"do not submit")
        changed.write_bytes(b"before")
        with self.owner() as editing:
            root = self.edit.ROOT
            track_only = editing.bind_file(editing.lookup(root, b"untouched")[0])
            ino = editing.lookup(root, b"edited")[0]
            present = editing.bind_file(ino)
            fh = editing.open(ino, os.O_RDWR | os.O_TRUNC)
            editing.write(fh, 0, b"our bytes")
            editing.release(fh)
            editing.stop_writes()
            changed.write_bytes(b"external size differs")  # No cooperating role, outside the mount.
            rejected = []
            with self.assertRaises(OSError):
                with editing.freeze() as cut:
                    self.assertEqual(cut.changed_files, (present,))
                    with self.assertRaises(OSError) as not_member:
                        with cut.open_file(track_only): self.fail("binding alone admitted unseen data")
                    self.assertEqual(not_member.exception.errno, errno.ESTALE)
                    rejected.append("not a member")
                    with self.assertRaises(OSError) as stale:
                        with cut.open_file(present): self.fail("adopted the later outside edit as this operation")
                    self.assertEqual(stale.exception.errno, errno.ESTALE)
                    rejected.append("outside edit")
                    cut.check_work()  # Lost original evidence invalidates even this outer cut.
            self.assertEqual(rejected, ["not a member", "outside edit"])  # Never pass on an early setup failure.
            self.assertEqual(changed.read_bytes(), b"external size differs")

    def test_failed_native_sync_never_becomes_a_cut_and_second_provider_gets_no_lock(self):
        with self.owner() as editing:
            with self.assertRaises(BlockingIOError):
                with self.owner(): self.fail("a second actor acquired the same original WORK")
            fh, _ = editing.create(self.edit.ROOT, b"dirty", 0o600, os.O_RDWR)
            editing.write(fh, 0, b"edit")
            editing.stop_writes()
            with patch("zipworkspace.linux_edit.os.fsync", side_effect=OSError(errno.EIO, "injected")):
                with self.assertRaises(OSError): editing.release(fh)
            with self.assertRaises(OSError) as refused:
                with editing.freeze(): self.fail("failed file fsync advanced the barrier")
            self.assertEqual(refused.exception.errno, errno.EIO)


if __name__ == "__main__":
    unittest.main()
