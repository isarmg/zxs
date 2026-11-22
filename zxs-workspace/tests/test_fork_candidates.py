"""CLI conflict-fork identity boundaries; these fixtures do not exercise a new freeze."""
from contextlib import contextmanager
from hashlib import sha256
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import worktree
from zipworkspace.archive import read_archive
from zipworkspace.model import Commit, Manifest, Rejected, new_id
from zipworkspace.private_candidates import CandidateDirectory
from zipworkspace.publish import require_native_writer


def archive(path, marker, body):
    with zipfile.ZipFile(path, "x", compression=zipfile.ZIP_STORED) as out:
        out.writestr("original", body)
        out.comment = marker.encode()
    return path.read_bytes()


def digest_tree(body):
    tree = Manifest()
    tree.add("original", False, len(body), sha256(body).hexdigest())
    return tree.digest()


class PendingFork(unittest.TestCase):
    def setUp(self):
        try: require_native_writer()
        except Rejected as err: self.skipTest(str(err))
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.work, self.store, self.state = (self.root/n for n in ("working","store","state"))
        self.work.mkdir(); self.store.mkdir()
        worktree.enroll(self.work, self.store, self.state)
        # Supply the pre-existing original pending receipt, not a newly captured working tree.
        with worktree.locked(self.state) as (con, row, _):
            body = b"original recorded material"
            self.old = Commit(row["repo"], row["branch"], 1, new_id(), None, digest_tree(body))
            self.source = self.state / (self.old.operation + ".zxs")
            self.raw = archive(self.source, self.old, body)
            con.execute("UPDATE checkout SET pending=?,pending_sha=? WHERE singleton=1",
                        (self.old.encode(), sha256(self.raw).hexdigest()))
        winner = Commit(self.old.repo, self.old.branch, 1, new_id(), None, digest_tree(b"winner"))
        self.winner = self.store/winner.filename
        self.winner_raw = archive(self.winner, winner, b"winner")
        self.branch, self.operation = new_id(), new_id()
        self.output = self.state/(self.operation+".zxs")

    def fork(self):
        with patch.object(worktree, "new_id", side_effect=[self.branch, self.operation]):
            return worktree.fork_pending(self.state, self.old.operation)

    def still_pending(self):
        self.assertEqual(worktree.status(self.state)["pending_operation"], self.old.operation)
        self.assertEqual(self.winner.read_bytes(), self.winner_raw)
        self.assertEqual(list(self.store.iterdir()), [self.winner])
        with worktree.locked(self.state) as (con, _, __):
            self.assertEqual(con.execute("SELECT COUNT(*) FROM forked_original").fetchone()[0], 0)

    def test_known_unavailable_publisher_never_opens_sqlite_or_creates_a_successor(self):
        before = sorted(p.name for p in self.state.iterdir())
        with patch.object(worktree, "require_native_writer", side_effect=Rejected("no native rename")), \
             patch.object(worktree, "locked", side_effect=AssertionError("no SQLite")), \
             patch.object(worktree, "held_candidates", side_effect=AssertionError("no candidate staging")):
            with self.assertRaisesRegex(Rejected, "no native rename"): self.fork()
        self.assertEqual(sorted(p.name for p in self.state.iterdir()), before)
        self.assertEqual(self.source.read_bytes(), self.raw)
        self.still_pending()

    def test_exact_original_bytes_fork_and_lost_reply_reuses_only_recorded_successor(self):
        (self.work/"original").write_bytes(b"later working bytes")
        hold, publish = worktree.held_store, worktree._publish
        held = []

        @contextmanager
        def keep_original(store):
            with hold(store) as scope:
                held.append(scope)
                yield scope

        def use_original(local, scope, marker, digest):
            self.assertEqual(held, [scope])
            self.assertTrue(os.path.samestat(os.fstat(scope.fd), self.store.stat()))
            return publish(local, scope, marker, digest)

        with patch.object(worktree, "held_store", keep_original), \
             patch.object(worktree, "publish", side_effect=AssertionError("do not reopen STORE")), \
             patch.object(worktree, "_publish", side_effect=use_original):
            final, marker = self.fork()  # CLI first-generation conflicts remain eligible.
        self.assertEqual((marker.branch,marker.operation,marker.parent,marker.tree),
                         (self.branch,self.operation,self.old.parent,self.old.tree))
        self.assertEqual(read_archive(self.output)[0], marker)
        self.assertEqual(final.read_bytes(),self.output.read_bytes())
        self.assertEqual(self.source.read_bytes(),self.raw)
        self.source.rename(self.state/"offline-original")
        self.winner.rename(self.root/"offline-winner")
        with patch.object(CandidateDirectory, "fork", side_effect=AssertionError("must not rebranch")), \
             patch.object(worktree, "relative_document", side_effect=AssertionError("no old winner needed")):
            self.assertEqual(worktree.fork_pending(self.state, self.old.operation), (final,marker))

    def test_slot_symlink_does_not_authorize_a_new_successor(self):
        retained = self.root/"held-winner"
        self.winner.rename(retained); self.winner.symlink_to(retained)
        with patch.object(CandidateDirectory, "fork", side_effect=AssertionError("no private fork")):
            with self.assertRaises((OSError, Rejected)): self.fork()
        self.assertTrue(self.winner.is_symlink())
        self.assertFalse(self.output.exists())
        self.still_pending()

    def test_equal_byte_winner_rebound_after_private_copy_rolls_back_the_successor(self):
        copy = CandidateDirectory.fork
        retained = self.root/"held-winner"

        def copy_then_rebind(area, old, expected_sha, new):
            digest = copy(area, old, expected_sha, new)
            self.winner.rename(retained); self.winner.write_bytes(retained.read_bytes())
            return digest

        with patch.object(CandidateDirectory, "fork", copy_then_rebind), \
             patch.object(worktree, "finish", side_effect=AssertionError("no accepted successor to finish")):
            with self.assertRaisesRegex(Rejected, "original archive leaf was replaced or modified"): self.fork()
        self.assertFalse(os.path.samestat(self.winner.stat(), retained.stat()))
        self.assertTrue(self.output.is_file())  # The unaccepted private output is not cleaned up or published.
        self.still_pending()

    def test_store_change_after_local_receipt_never_retargets_the_first_publish(self):
        finish, retained = worktree.finish, self.root/"held-store"

        def switch_store(state, operation, *, original_store):
            self.assertEqual(operation, self.operation)
            self.assertTrue(os.path.samestat(os.fstat(original_store.fd), self.store.stat()))
            self.store.rename(retained); self.store.mkdir()
            return finish(state, operation, original_store=original_store)

        with patch.object(worktree, "finish", side_effect=switch_store), \
             patch.object(worktree, "_publish", side_effect=AssertionError("no upload into a renamed STORE")):
            with self.assertRaisesRegex(Rejected, "selected archive store changed"): self.fork()
        self.assertEqual(list(self.store.iterdir()), [])
        self.assertEqual(list(retained.iterdir()), [retained/self.winner.name])
        self.assertEqual(worktree.status(self.state)["pending_operation"], self.operation)
        self.assertTrue(self.output.is_file())
        self.store.rename(self.root/"replacement-store"); retained.rename(self.store)
        self.winner.rename(self.root/"offline-winner")
        with patch.object(worktree, "relative_document", side_effect=AssertionError("no old winner needed")):
            final, marker = worktree.fork_pending(self.state, self.old.operation)
        self.assertEqual((final.parent, marker.operation), (self.store, self.operation))

    def test_input_name_changes_after_admission_never_copies_the_other_input(self):
        candidate = CandidateDirectory.candidate
        retained = self.state/"retained-input"
        @contextmanager
        def displace(area, operation, **options):
            with candidate(area, operation, **options) as handles:
                self.source.rename(retained)
                self.source.write_bytes(self.winner_raw)
                yield handles
        with patch.object(CandidateDirectory, "candidate", displace), self.assertRaises(Rejected):
            self.fork()
        self.assertEqual(self.output.read_bytes(), self.raw)
        self.assertEqual(self.source.read_bytes(), self.winner_raw)
        self.assertEqual(retained.read_bytes(), self.raw)
        self.still_pending()

    def test_substituting_a_symlink_immediately_before_comment_cannot_edit_another_zip(self):
        saved = self.state/"held-successor"
        other = self.root/"different.zip"
        other.write_bytes(self.winner_raw)
        real = zipfile.ZipFile
        def before_comment(file, mode="r", *args, **kwargs):
            if mode == "a":
                self.output.rename(saved)
                self.output.symlink_to(other)
                self.assertTrue(os.path.samestat(os.fstat(file.fileno()), saved.stat()))
            return real(file, mode, *args, **kwargs)
        with patch.object(zipfile, "ZipFile", side_effect=before_comment), self.assertRaises(Rejected):
            self.fork()
        self.assertTrue(self.output.is_symlink())
        self.assertEqual(other.read_bytes(), self.winner_raw)
        self.assertEqual(self.source.read_bytes(), self.raw)
        self.assertTrue(saved.is_file())
        self.still_pending()

    def test_byte_identical_replacement_during_directory_sync_never_settles_pending(self):
        sync = os.fsync
        retained = self.state/"uncertain-successor"
        def displace(fd):
            sync(fd)
            if os.path.samestat(self.state.stat(), os.fstat(fd)):
                self.output.rename(retained)
                self.output.write_bytes(retained.read_bytes())
        with patch.object(os, "fsync", side_effect=displace), self.assertRaises(Rejected):
            self.fork()
        self.assertEqual(self.output.read_bytes(),retained.read_bytes())
        self.assertFalse(os.path.samestat(self.output.stat(), retained.stat()))
        self.still_pending()

    def test_preoccupied_successor_is_never_opened_for_update(self):
        self.output.write_bytes(self.winner_raw)
        with self.assertRaises(FileExistsError): self.fork()
        self.assertEqual(self.output.read_bytes(), self.winner_raw)
        self.still_pending()
