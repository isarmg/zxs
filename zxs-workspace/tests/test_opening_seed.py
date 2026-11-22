"""Cold first checkout: synthetic input and real, previously nonexistent local names only."""
from contextlib import ExitStack, closing
import errno
import hashlib
import os
from pathlib import Path
import sqlite3
import stat
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import opening
from zipworkspace.model import Commit, Manifest, Rejected


class InitialCheckoutRecord(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory(); self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.store = self.root / "store"; self.store.mkdir()
        self.archive = self.root / "chosen.zxs"
        self.marker = Commit("1" * 32, "2" * 32, 1, "3" * 32, None, Manifest().digest())
        with zipfile.ZipFile(self.archive, "x", compression=zipfile.ZIP_STORED) as z:
            z.comment = self.marker.encode()
        self.state = self.root / "state"; self.dest = self.root / "work"

    def begin(self):
        return opening.begin(self.archive, self.root, self.dest.name, self.store, self.state, automatic=False)

    def seed(self):
        return next(self.root.glob(".zip-opening-*"))

    def record_only(self):
        with patch.object(opening, "finish", side_effect=InterruptedError("STATE installed")):
            with self.assertRaisesRegex(InterruptedError, "STATE installed"): self.begin()
        info = opening.status(self.state)
        self.assertEqual(info["recorded_stage"], "original_recorded")
        return info["operation"]

    def test_unstaged_original_resumes_offline_using_one_read_only_seed(self):
        from fcntl import F_GETFL, fcntl

        operation = self.record_only()
        self.archive.rename(self.root / "offline.zxs")
        local = self.state / "original.zxs"
        held, extract = [], opening.extract_checkout_document

        def materialize(document, *, display_name, scratch):
            meta = os.fstat(document.fileno())
            self.assertTrue(os.path.samestat(meta, local.stat()))
            self.assertEqual(fcntl(document.fileno(), F_GETFL) & os.O_ACCMODE, os.O_RDONLY)
            self.assertEqual((display_name, scratch.parent), (local.name, self.dest.parent))
            held.append(document)
            return extract(document, display_name=display_name, scratch=scratch)

        with patch.object(opening, "relative_document", wraps=opening.relative_document) as opened, \
             patch.object(opening, "archive_sha", wraps=opening.archive_sha) as digest, \
             patch.object(opening, "extract_checkout_document", side_effect=materialize):
            self.assertEqual(opening.finish(self.state, operation), (self.dest, self.marker))
        opened.assert_called_once()
        doc, = held
        self.assertEqual([c.args for c in digest.call_args_list], [(doc,), (doc,)])
        self.assertTrue(doc.closed)
        self.assertEqual(opening.status(self.state)["recorded_stage"], "attached")

    def test_wrong_seed_digest_is_rejected_before_creating_scratch(self):
        operation = self.record_only()
        (self.state / "original.zxs").write_bytes(b"wrong archive")
        with patch.object(opening.tempfile, "mkdtemp", side_effect=AssertionError("no scratch")):
            with self.assertRaisesRegex(Rejected, "original checkout archive is unavailable"):
                opening.finish(self.state, operation)
        self.assertEqual(opening.status(self.state)["recorded_stage"], "original_recorded")
        self.assertFalse(self.dest.exists())

    def test_equal_bytes_seed_rebound_at_scratch_barrier_is_not_registered(self):
        operation = self.record_only()
        local, displaced = self.state / "original.zxs", self.state / "displaced"
        sync = opening.sync_dir

        def swap_after_sync(parent):
            self.assertEqual(parent, self.dest.parent)
            sync(parent)
            local.rename(displaced); local.write_bytes(displaced.read_bytes())

        with patch.object(opening, "sync_dir", side_effect=swap_after_sync):
            with self.assertRaisesRegex(Rejected, "original archive leaf was replaced or modified"):
                opening.finish(self.state, operation)
        self.assertEqual(opening.status(self.state)["recorded_stage"], "original_recorded")
        self.assertNotEqual(local.stat().st_ino, displaced.stat().st_ino)
        self.assertEqual(local.read_bytes(), displaced.read_bytes())
        self.assertFalse(self.dest.exists())
        self.assertFalse((self.state / "worktree.sqlite").exists())
        self.assertEqual(len(list(self.root.glob(".zip-checkout-*"))), 1)  # Never clean up on an uncertain result.

    def test_verified_stage_resumes_without_reopening_the_seed(self):
        operation = self.record_only()
        with patch.object(opening, "rename_new_at", side_effect=InterruptedError("before publish")):
            with self.assertRaisesRegex(InterruptedError, "before publish"): opening.finish(self.state, operation)
        self.assertEqual(opening.status(self.state)["recorded_stage"], "verified_stage")
        (self.state / "original.zxs").rename(self.state / "displaced")
        with patch.object(opening, "relative_document", side_effect=AssertionError("no seed reopen")):
            self.assertEqual(opening.finish(self.state, operation), (self.dest, self.marker))

    def test_only_the_prepared_row_is_installed_and_normal_sqlite_resumes_that_operation(self):
        connect = sqlite3.connect
        def memory_only(path, *args, **kwargs):
            self.assertEqual(path, ":memory:"); return connect(path, *args, **kwargs)
        with patch.object(opening.sqlite3, "connect", side_effect=memory_only), \
             patch.object(opening, "finish", side_effect=OSError("stopped before first native recovery")):
            with self.assertRaisesRegex(OSError, "before first"): self.begin()
        db = self.state / "opening.sqlite"
        self.assertEqual({p.name for p in self.state.iterdir()}, {"original.zxs", db.name})
        self.assertLessEqual(db.stat().st_size, opening.MAX_INITIAL_DB)
        with closing(connect(db.as_uri() + "?mode=ro", uri=True)) as con:
            con.row_factory = sqlite3.Row
            row, = con.execute("SELECT * FROM opening")
            tables = con.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()
        self.assertEqual([t[0] for t in tables], ["opening"])
        self.assertEqual((row["marker"], row["sha"]), (self.marker.encode(), hashlib.sha256(self.archive.read_bytes()).hexdigest()))
        self.assertEqual((row["stage"], row["done"], row["ordinal"]), (None, 0, 1))
        self.assertNotEqual(row["operation"], self.marker.operation)
        self.assertFalse(self.dest.exists())
        # Serialization is a fresh-create requirement, not a new requirement on existing receipts.
        with patch.object(opening.sqlite3, "Connection", object):
            self.assertEqual(opening.status(self.state)["operation"], row["operation"])
            self.assertEqual(opening.finish(self.state, row["operation"]), (self.dest, self.marker))
        self.assertEqual(opening.status(self.state)["recorded_stage"], "attached")

    def test_plain_database_fd_is_closed_before_state_can_be_opened_by_native_sqlite(self):
        fdopen, syscall_open, rename = os.fdopen, os.open, opening.rename_new
        database_fds = []
        def note_writer(fd, mode, **kwargs):
            if mode == "w+b":
                name = self.seed() / "opening.sqlite"
                if name.exists() and os.path.samestat(name.stat(), os.fstat(fd)):
                    database_fds.append(fd)
            return fdopen(fd, mode, **kwargs)
        def refuse_db_reopen(path, *args, **kwargs):
            self.assertNotEqual(Path(path).name, "opening.sqlite", "handoff may only stat the database")
            return syscall_open(path, *args, **kwargs)
        with ExitStack() as scope:
            def publish(src, dst):
                self.assertEqual(dst, self.state)
                fd, = database_fds
                with self.assertRaises(OSError) as closed: os.fstat(fd)
                self.assertEqual(closed.exception.errno, errno.EBADF)
                rename(src, dst)
                scope.enter_context(patch.object(opening.os, "open", side_effect=refuse_db_reopen))
            scope.enter_context(patch.object(opening.os, "fdopen", side_effect=note_writer))
            scope.enter_context(patch.object(opening, "rename_new", side_effect=publish))
            finish = scope.enter_context(patch.object(opening, "finish"))
            self.begin()
            finish.assert_called_once()
        self.assertTrue((self.state / "opening.sqlite").is_file())

    def test_fresh_capability_refusal_precedes_archive_capture_and_seed_creation(self):
        with patch.object(opening.sqlite3, "Connection", object), \
             patch.object(opening, "named_document", side_effect=AssertionError("no source access")):
            with self.assertRaisesRegex(Rejected, "fresh checkout needs"): self.begin()
        self.assertEqual(list(self.root.glob(".zip-opening-*")), [])
        self.assertFalse(self.state.exists()); self.assertFalse(self.dest.exists())

    def test_rebound_private_seed_cannot_receive_the_database(self):
        build = opening._initial_image
        def replace(fields):
            image = build(fields)
            seed = self.seed(); seed.rename(self.root / "held-original")
            seed.mkdir(mode=0o700); (seed / "keep").write_text("unrelated")
            return image
        with patch.object(opening, "_initial_image", side_effect=replace):
            with self.assertRaises(Rejected): self.begin()
        self.assertEqual({p.name for p in self.seed().iterdir()}, {"keep"})
        self.assertEqual({p.name for p in (self.root / "held-original").iterdir()}, {"original.zxs"})
        self.assertFalse(self.state.exists()); self.assertFalse(self.dest.exists())

    def test_unknown_sqlite_files_are_neither_opened_nor_published(self):
        before, build = set(), opening._initial_image
        for name in ("opening.sqlite", "opening.sqlite-journal", "opening.sqlite-wal", "opening.sqlite-shm", "other"):
            with self.subTest(name=name):
                def obstruct(fields):
                    image = build(fields)
                    seed, = set(self.root.glob(".zip-opening-*")) - before
                    (seed / name).write_bytes(b"preexisting data")
                    return image
                with patch.object(opening, "_initial_image", side_effect=obstruct):
                    with self.assertRaisesRegex(Rejected, "unexpected entry"): self.begin()
                seed, = set(self.root.glob(".zip-opening-*")) - before
                self.assertEqual((seed / name).read_bytes(), b"preexisting data")
                self.assertEqual({p.name for p in seed.iterdir()}, {"original.zxs", name})
                before.add(seed)
        self.assertFalse(self.state.exists()); self.assertFalse(self.dest.exists())

    def test_equal_byte_database_replacement_at_the_joint_parent_sync_is_not_registered(self):
        sync = os.fsync
        def replace_after_sync(fd):
            sync(fd)
            if not stat.S_ISDIR(os.fstat(fd).st_mode): return
            seed = self.seed(); db = seed / "opening.sqlite"
            if not db.exists(): return  # Earlier original ZIP barrier.
            self.assertTrue(os.path.samestat(os.fstat(fd), seed.stat()))
            old = self.root / "displaced.sqlite"; db.rename(old); db.write_bytes(old.read_bytes())
        with patch.object(opening.os, "fsync", side_effect=replace_after_sync):
            with self.assertRaises(Rejected): self.begin()
        db = self.seed() / "opening.sqlite"; old = self.root / "displaced.sqlite"
        self.assertEqual(old.read_bytes(), db.read_bytes())
        self.assertNotEqual(old.stat().st_ino, db.stat().st_ino)
        self.assertFalse(self.state.exists()); self.assertFalse(self.dest.exists())

    def test_truncated_seed_or_failed_directory_sync_preserves_unregistered_evidence(self):
        sync, earlier = os.fsync, set()
        for fault in ("truncated", "not synced"):
            with self.subTest(fault=fault):
                def interrupt(fd):
                    seed, = set(self.root.glob(".zip-opening-*")) - earlier
                    db = seed / "opening.sqlite"
                    if db.exists():
                        m = os.fstat(fd)
                        if fault == "truncated" and os.path.samestat(m, db.stat()):
                            os.ftruncate(fd, 7)
                        elif fault == "not synced" and os.path.samestat(m, seed.stat()):
                            raise OSError("real seed dirsync refused")
                    return sync(fd)
                with patch.object(opening.os, "fsync", side_effect=interrupt):
                    with self.assertRaises(Rejected if fault == "truncated" else OSError): self.begin()
                seed, = set(self.root.glob(".zip-opening-*")) - earlier
                self.assertTrue((seed / "opening.sqlite").is_file())
                self.assertEqual((seed / "original.zxs").read_bytes(), self.archive.read_bytes())
                self.assertFalse(self.state.exists()); self.assertFalse(self.dest.exists())
                earlier.add(seed)

    def test_record_input_budget_counts_encoded_bytes_before_sqlite(self):
        with patch.object(opening.sqlite3, "connect", side_effect=AssertionError("no SQLite")):
            with self.assertRaisesRegex(Rejected, "size budget"):
                opening._initial_image(("非" * (opening.MAX_INITIAL_DB // 4),))
