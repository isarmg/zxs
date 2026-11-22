"""Read-only status and comparison reports observe SQLite records; neither repairs them."""
from contextlib import closing
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from zipworkspace import lifecycle, worktree
from zipworkspace.model import new_id


class StatusSnapshot(unittest.TestCase):
    def setUp(self):
        area = tempfile.TemporaryDirectory()
        self.addCleanup(area.cleanup)
        root = Path(area.name)
        self.work, store, self.state = (root / name for name in ("work", "store", "state"))
        self.work.mkdir(); store.mkdir()
        worktree.enroll(self.work, store, self.state)
        self.db = self.state / "worktree.sqlite"

    def test_an_active_rollback_writer_does_not_prevent_reading_the_committed_status(self):
        before = worktree.status(self.state)
        with worktree.locked(self.state) as (writer, _, __):
            writer.execute("UPDATE instance SET phase='hide-pending',active=? WHERE singleton=1", (new_id(),))
            self.assertEqual(worktree.status(self.state), before)
        self.assertEqual(worktree.status(self.state)["visibility"], "hide-pending")

    def test_changes_reads_committed_catalog_while_a_rollback_writer_holds_a_reservation(self):
        (self.work / "draft").write_bytes(b"not committed")
        with worktree.locked(self.state) as (writer, _, __):
            writer.execute("UPDATE instance SET phase='hide-pending',active=? WHERE singleton=1", (new_id(),))
            self.assertEqual(lifecycle.changes(self.state),
                             {"added": {"count": 1, "first_paths": ["draft"]},
                              "removed": {"count": 0, "first_paths": []},
                              "changed": {"count": 0, "first_paths": []}})
        self.assertEqual(worktree.status(self.state)["visibility"], "hide-pending")

    def test_committed_wal_is_visible_but_two_status_rows_never_straddle_a_commit(self):
        before = worktree.status(self.state)
        other_branch, operation = new_id(), new_id()
        original = lifecycle.instance
        with closing(sqlite3.connect(self.db)) as writer:
            self.assertEqual(writer.execute("PRAGMA journal_mode=WAL").fetchone()[0], "wal")
            writer.execute("PRAGMA wal_autocheckpoint=0")

            def commit_between_reads(reader):
                # The checkout row was already read. Native WAL lets another writer
                # commit both rows now without invalidating this recorded snapshot.
                with writer:
                    writer.execute("UPDATE checkout SET branch=? WHERE singleton=1", (other_branch,))
                    writer.execute("UPDATE instance SET phase='hide-pending',active=? WHERE singleton=1", (operation,))
                return original(reader)

            with patch.object(lifecycle, "instance", side_effect=commit_between_reads):
                self.assertEqual(worktree.status(self.state), before)
            after = worktree.status(self.state)
            self.assertEqual((after["branch"], after["visibility"], after["pending_visibility_operation"]),
                             (other_branch, "hide-pending", operation))

    def test_hot_rollback_journal_is_not_repaired_by_a_status_or_changes_read(self):
        # A separate native writer spills dirty pages then exits without closing.
        subprocess.run([sys.executable, "-I", "-c", """
import os, sqlite3, sys
con = sqlite3.connect(sys.argv[1])
assert con.execute("PRAGMA journal_mode=DELETE").fetchone() == ("delete",)
con.executescript("PRAGMA synchronous=FULL; PRAGMA cache_size=2; PRAGMA cache_spill=ON; "
                  "BEGIN IMMEDIATE; CREATE TABLE interrupted (data BLOB); "
                  "INSERT INTO interrupted VALUES (zeroblob(1048576));")
os._exit(0)
""", str(self.db)], check=True, timeout=20)
        journal = self.state / "worktree.sqlite-journal"
        before_db, before_journal = self.db.read_bytes(), journal.read_bytes()
        self.assertGreater(len(before_journal), 512)
        self.assertNotEqual(before_journal[:8], bytes(8))
        for query in (worktree.status, lifecycle.changes):
            with self.subTest(query=query.__name__):
                with self.assertRaises(sqlite3.OperationalError) as failure:
                    query(self.state)
                self.assertEqual(failure.exception.sqlite_errorcode, sqlite3.SQLITE_READONLY_ROLLBACK)
                self.assertEqual((self.db.read_bytes(), journal.read_bytes()), (before_db, before_journal))

    def test_missing_catalog_is_not_recreated_or_reported_as_idle(self):
        self.db.rename(self.state / "original.sqlite")
        for query in (worktree.status, lifecycle.changes):
            with self.subTest(query=query.__name__), self.assertRaises(sqlite3.OperationalError):
                query(self.state)
        self.assertFalse(self.db.exists())
