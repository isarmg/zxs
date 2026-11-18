"""Final-slot acknowledgement on real temporary directories; no new working-tree freeze."""
from contextlib import contextmanager
import errno
from hashlib import sha256
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import publish as media, worktree
from zipworkspace.model import Commit, Manifest, Rejected, new_id


class StoreAcknowledgement(unittest.TestCase):
    def setUp(self):
        try: media.require_native_writer()
        except Rejected as err: self.skipTest(str(err))
        area = tempfile.TemporaryDirectory()
        self.addCleanup(area.cleanup)
        self.root = Path(area.name)
        work, self.store, self.state = (self.root / name for name in ("work", "store", "state"))
        work.mkdir(); self.store.mkdir()
        worktree.enroll(work, self.store, self.state)
        body = b"frozen original content"
        tree = Manifest()
        tree.add("item", False, len(body), sha256(body).hexdigest())
        with worktree.locked(self.state) as (con, row, _):
            self.marker = Commit(row["repo"], row["branch"], 1, new_id(), None, tree.digest())
            local = self.state / (self.marker.operation + ".zxs")
            with zipfile.ZipFile(local, "x", compression=zipfile.ZIP_STORED) as z:
                z.writestr("item", body); z.comment = self.marker.encode()
            self.raw = local.read_bytes()
            self.digest = sha256(self.raw).hexdigest()
            con.execute("UPDATE checkout SET pending=?,pending_sha=? WHERE singleton=1",
                        (self.marker.encode(), self.digest))
        self.target = self.store / self.marker.filename

    def resume(self):
        return worktree.finish(self.state, self.marker.operation)

    def assert_pending(self):
        with worktree.locked(self.state) as (con, row, _):
            self.assertEqual((row["pending"], row["pending_sha"]), (self.marker.encode(), self.digest))
            self.assertIsNone(con.execute("SELECT 1 FROM completed WHERE operation=?",
                                          (self.marker.operation,)).fetchone())

    def test_only_an_absent_canonical_leaf_in_the_still_named_store_is_missing(self):
        self.assertFalse(media.confirm_final(self.target, self.marker, self.digest))
        with self.assertRaises(Rejected):
            media.confirm_final(self.store / "another.zxs", self.marker, self.digest)
        with media.held_store(self.store) as original:
            self.store.rename(self.root / "unmounted")
            with self.assertRaises(media.Uncertain):
                original.confirm(self.marker, self.digest)
        with self.assertRaises(FileNotFoundError):
            media.confirm_final(self.target, self.marker, self.digest)
        self.assert_pending()

    def test_disappearance_after_open_does_not_start_another_upload(self):
        self.target.write_bytes(self.raw)
        digest = media.hash_document
        def disappear(doc):
            result = digest(doc)
            self.target.rename(self.root / "preserved")
            return result
        with patch.object(media, "hash_document", side_effect=disappear), \
             patch.object(media.tempfile, "mkstemp", side_effect=AssertionError("must not upload")), \
             self.assertRaises(media.Uncertain):
            self.resume()
        self.assertEqual((self.root / "preserved").read_bytes(), self.raw)
        self.assert_pending()

    def test_file_and_directory_sync_failures_do_not_settle_the_operation(self):
        self.target.write_bytes(self.raw)
        real = os.fsync
        for path in (self.target, self.store):
            with self.subTest(path=path):
                selected = path.stat()
                def fail(fd):
                    if os.path.samestat(selected, os.fstat(fd)): raise OSError("unconfirmed sync")
                    real(fd)
                with patch.object(os, "fsync", side_effect=fail), self.assertRaisesRegex(OSError, "unconfirmed sync"):
                    self.resume()
                self.assert_pending()
        with patch.object(media, "rename_new", side_effect=AssertionError("already present")):
            self.assertEqual(self.resume(), (self.target, self.marker))

    def test_equal_bytes_different_inode_during_real_directory_sync_is_not_acknowledged(self):
        self.target.write_bytes(self.raw)
        original = self.root / "retained-slot"
        real, directory = os.fsync, self.store.stat()
        def substitute(fd):
            real(fd)
            if os.path.samestat(directory, os.fstat(fd)):
                self.target.rename(original)
                self.target.write_bytes(self.raw)
        with patch.object(os, "fsync", side_effect=substitute), self.assertRaises(media.Uncertain):
            self.resume()
        self.assertFalse(os.path.samestat(original.stat(), self.target.stat()))
        self.assertEqual(original.read_bytes(), self.target.read_bytes())
        self.assert_pending()

    def test_substituting_the_store_itself_at_real_directory_sync_preserves_pending(self):
        self.target.write_bytes(self.raw)
        alternate, original = self.root / "alternate", self.root / "selected-original"
        alternate.mkdir()
        (alternate / self.marker.filename).write_bytes(self.raw)
        real, directory = os.fsync, self.store.stat()
        def rebind(fd):
            real(fd)
            if os.path.samestat(directory, os.fstat(fd)):
                self.store.rename(original)
                alternate.rename(self.store)
        with patch.object(os, "fsync", side_effect=rebind), self.assertRaises(media.Uncertain):
            self.resume()
        self.assert_pending()
        self.assertEqual(self.target.read_bytes(), (original / self.marker.filename).read_bytes())
        self.store.rename(alternate)
        original.rename(self.store)
        with patch.object(media, "rename_new", side_effect=AssertionError("already present")):
            self.assertEqual(self.resume(), (self.target, self.marker))

    def test_successful_rename_must_ack_its_uploaded_inode_but_cold_receipt_proves_bytes(self):
        retained = self.root / "uploaded-original"
        primitive = media.rename_new
        def after_rename(src, dst):
            primitive(src, dst)
            dst.rename(retained)
            dst.write_bytes(self.raw)
        with patch.object(media, "rename_new", side_effect=after_rename), self.assertRaises(media.Uncertain):
            self.resume()
        self.assert_pending()
        self.assertFalse(os.path.samestat(retained.stat(), self.target.stat()))
        # The durable receipt records a marker and byte digest, not a past inode.
        with patch.object(media, "rename_new", side_effect=AssertionError("must not republish")):
            self.assertEqual(self.resume(), (self.target, self.marker))
            self.assertEqual(self.resume(), (self.target, self.marker))

    def test_original_writer_and_read_verifier_overlap_only_for_identity_handoff(self):
        handoff, parse, rename = media._pin_upload, media.read_managed_document, media.rename_new
        observations = []
        @contextmanager
        def pin(scope, name, writer):
            self.assertFalse(writer.closed)
            self.assertTrue(writer.writable())
            with handoff(scope, name, writer) as held:
                wfd = writer.fileno()
                self.assertNotEqual(wfd, held)
                self.assertTrue(os.path.samestat(os.fstat(wfd), os.fstat(held)))
                observations.append((writer, wfd, held))
                yield held
        def writer_is_closed():
            writer, wfd, held = observations[0]
            self.assertTrue(writer.closed)
            with self.assertRaises(OSError) as exc: os.fstat(wfd)
            self.assertEqual(exc.exception.errno, errno.EBADF)
            return held
        checked = []
        def inspect(doc, **kw):
            # Only where the original FD number has not already been reused
            # for the later separate final-slot FD. The stage check is the first.
            if not checked:
                self.assertEqual(doc.fileno(), writer_is_closed())
                checked.append("closed-before-zip")
            return parse(doc, **kw)
        def move(src, dst):
            self.assertTrue(os.path.samestat(os.fstat(writer_is_closed()), src.stat()))
            checked.append("closed-before-rename")
            return rename(src, dst)
        with patch.object(media, "_pin_upload", pin), patch.object(media, "read_managed_document", inspect), \
             patch.object(media, "rename_new", move):
            self.assertEqual(self.resume(), (self.target, self.marker))
        self.assertEqual(checked, ["closed-before-zip", "closed-before-rename"])

    def test_substituted_stage_at_writer_handoff_is_not_the_uploaded_file(self):
        original = self.root / "original-partial"
        handoff = media._pin_upload
        @contextmanager
        def before_pin(scope, name, writer):
            slot = self.store / name
            slot.rename(original)
            slot.write_bytes(self.raw)
            self.assertTrue(os.path.samestat(os.fstat(writer.fileno()), original.stat()))
            with handoff(scope, name, writer) as held: yield held
        with patch.object(media, "_pin_upload", before_pin), \
             patch.object(media, "rename_new", side_effect=AssertionError("must not promote")), \
             self.assertRaises(media.Uncertain):
            self.resume()
        self.assertEqual(original.read_bytes(), self.raw)
        [other] = self.store.glob("*.partial")
        self.assertEqual(other.read_bytes(), self.raw)
        self.assertFalse(os.path.samestat(other.stat(), original.stat()))
        self.assertFalse(self.target.exists())
        self.assert_pending()

    def test_raw_digest_alone_does_not_authenticate_an_inconsistent_member_tree(self):
        with zipfile.ZipFile(self.target, "x", compression=zipfile.ZIP_STORED) as z:
            z.writestr("item", b"wrong content"); z.comment = self.marker.encode()
        with self.assertRaises(Rejected):
            media.confirm_final(self.target, self.marker, sha256(self.target.read_bytes()).hexdigest())
        self.assert_pending()
