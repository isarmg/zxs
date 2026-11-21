"""Linux native worktree initialization; no application takeover of an existing catalog."""
from concurrent.futures import Future
from contextlib import ExitStack
import errno
import hashlib
from importlib.util import find_spec
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import lifecycle, live_candidate, private_stage, publish, worktree
from zipworkspace.archive import read_archive
from zipworkspace.desktop.branch_choice import SnapshotBranch
from zipworkspace.desktop.commit_choice import CommitChoice
from zipworkspace.desktop.initialize_choice import InitializeChoice, available
from zipworkspace.desktop.pending_fork import PendingFork
from zipworkspace.desktop.retry_choice import PendingRetry
from zipworkspace.model import Commit, Manifest, Rejected
from zipworkspace.private_candidates import held_candidates


@unittest.skipUnless(sys.platform == "linux" and available(), "actual Linux ext4/XFS writer required")
class NewOriginalWorktree(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        self.work, self.private, self.store = (root / leaf for leaf in ("edited", "status", "archives"))
        for path in (self.work, self.private, self.store): path.mkdir(mode=0o700)
        self.state = self.private / "this-tree"
        try:
            publish.require_native_writer()
            with held_candidates(self.private) as parent:
                private_stage.same_mount(parent.parent, parent.parent)
                fd = os.open(".", os.O_TMPFILE | os.O_EXCL | os.O_RDWR, 0o600, dir_fd=parent.parent)
                os.close(fd)
            from zipworkspace.linux_mount import _ordinary_user
            with ExitStack() as native:
                proc = live_candidate.ProcView(native)
                _ordinary_user(read_proc=proc.read)
        except (OSError, Rejected) as error:
            self.skipTest(f"fixture needs an admitted ext4/XFS mount with anonymous files: {error}")
        self.story = self.work / "story"
        self.story.write_bytes(b"user data before any version")
        (self.work / "empty").mkdir()

    def plan(self):
        action = InitializeChoice()
        self.addCleanup(action.close)
        action.pick_work(self.work)
        action.pick_store(self.store)
        action.pick_private(self.private)
        review = action.review(self.state.name)
        self.assertIn(str(self.work), review)
        self.assertIn(action.branch, review)
        self.assertFalse(self.state.exists())
        self.assertEqual(list(self.store.iterdir()), [])
        return action

    def register(self):
        action = self.plan()
        with patch.object(worktree, "commit", side_effect=AssertionError("registration cannot commit")), \
             patch.object(lifecycle, "observe_held", side_effect=AssertionError("registration does not scan work files")):
            with self.assertRaises(Rejected): action.run()  # Only an explicit confirmed call may write.
            action.run(confirmed=True)
        live = action.take_session()
        self.assertIsNotNone(live)
        self.addCleanup(live.close)
        self.assertEqual(set(self.state.iterdir()), {self.state / "worktree.sqlite"})
        self.assertEqual(list(self.store.iterdir()), [])
        self.assertEqual(self.story.read_bytes(), b"user data before any version")
        result = live.status()
        self.assertEqual((result["base_generation"], result["base_operation"], result["pending_operation"]),
                         (0, None, None))
        self.assertEqual((result["worktree"], result["repo"], result["branch"]),
                         (action.instance, action.repo, action.branch))
        self.assertEqual(worktree.status(self.state), result)
        self.assertFalse(live.branch_source_available)
        with self.assertRaises(Rejected): SnapshotBranch(live)
        with self.assertRaises(Rejected): action.run(confirmed=True)
        return live

    def first(self, live):
        attempt = CommitChoice(live)
        self.addCleanup(attempt.release)
        self.assertIn("尚无已提交的快照", attempt.prepare())
        return attempt

    def test_first_contents_is_admitted_local_inventory_even_with_store_offline(self):
        live = self.register()
        absent = self.store.rename(self.store.with_name("store-offline"))
        db = self.state / "worktree.sqlite"
        original = db.read_bytes()
        with patch.object(lifecycle, "relative_document", side_effect=AssertionError("no STORE document")), \
             patch.object(worktree, "anonymous_capture", side_effect=AssertionError("no freeze")):
            report, size = lifecycle.observe_first_contents(live)
        self.assertEqual(size, self.story.stat().st_size)
        self.assertEqual(
            [(r.path, r.before, r.after, r.after_bytes) for r in report.page(0)],
            [("empty", None, "directory", None), ("story", None, "file", size)])
        self.assertEqual(db.read_bytes(), original)
        self.assertEqual((live.status()["base_generation"], live.status()["pending_operation"]), (0, None))
        self.assertEqual(list(absent.iterdir()), [])

    def test_no_first_contents_if_another_writer_changes_the_registration_during_observation(self):
        live = self.register()
        actual_read = lifecycle._observe_local_work
        def read(*args, **kwargs):
            contents = actual_read(*args, **kwargs)
            # A newly changed/inconsistent checkout cannot get a stale no-pending report.
            with worktree.locked(self.state) as (con, _, __):
                con.execute("UPDATE checkout SET pending_sha=? WHERE singleton=1", ("a"*64,))
            return contents
        with patch.object(lifecycle, "_observe_local_work", side_effect=read):
            with self.assertRaisesRegex(Rejected, "无待决"): lifecycle.observe_first_contents(live)
        with worktree.locked(self.state, read_only=True) as (_, row, __):
            self.assertEqual((row["generation"], row["pending"], row["pending_sha"]), (0, None, "a"*64))
        self.assertEqual(list(self.store.iterdir()), [])

    def test_register_then_manually_publish_the_exact_genesis_with_no_parent(self):
        live = self.register()
        attempt = self.first(live)
        with patch.object(live_candidate, "capture_live", side_effect=AssertionError("unconfirmed cannot freeze")):
            with self.assertRaises(Rejected): worktree.commit(self.state, selected=attempt)
        self.assertEqual(list(self.store.iterdir()), [])
        final, marker = attempt.run()
        self.assertEqual((marker.repo, marker.branch, marker.generation, marker.parent, marker.operation),
                         (live.repo, live.initial_base[1], 1, None, attempt.operation))
        self.assertEqual((final.parent, final.name), (self.store, marker.filename))
        actual, tree = read_archive(final)
        self.assertEqual(actual, marker)
        self.assertEqual(tree.rows["story"][:2], (False, self.story.stat().st_size))
        self.assertEqual(tree.rows["empty"], (True, 0, None))
        self.assertEqual(live.status()["base_operation"], marker.operation)
        self.assertEqual(attempt.result()[0], "completed")
        self.assertEqual(self.story.read_bytes(), b"user data before any version")
        with patch.object(lifecycle, "_observe_local_work", side_effect=AssertionError("no stale zero-generation scan")):
            with self.assertRaisesRegex(Rejected, "仍处在零代"): lifecycle.observe_first_contents(live)

        final.rename(self.store / "unavailable-at-the-base.zxs")
        next_ = CommitChoice(live)
        self.addCleanup(next_.release)
        with self.assertRaisesRegex(Rejected, "缺少原基线"): next_.prepare()

    def test_reject_a_rebound_work_directory_before_state_exists(self):
        selected = self.plan()
        self.work.rename(self.work.with_name("former-work"))
        self.work.mkdir(mode=0o700)
        (self.work / "story").write_bytes(b"user data before any version")
        with self.assertRaises(Rejected): selected.run(confirmed=True)
        self.assertIsNone(selected.take_session())
        self.assertFalse(self.state.exists())
        self.assertEqual(list(self.store.iterdir()), [])

    def test_an_existing_state_name_is_not_adopted(self):
        selected = self.plan()
        self.state.mkdir(mode=0o700)
        record = self.state / "keep-me"
        record.write_bytes(b"prior")
        with self.assertRaisesRegex(Rejected, "名称已被占用"): selected.run(confirmed=True)
        self.assertIsNone(selected.take_session())
        self.assertEqual(list(self.state.iterdir()), [record])
        self.assertEqual(record.read_bytes(), b"prior")
        self.assertEqual(list(self.store.iterdir()), [])

    def test_outer_state_parent_sync_error_cannot_offer_a_live_session(self):
        selected = self.plan()
        actual, seen = os.fsync, []
        def sync(fd):
            if os.path.samestat(os.fstat(fd), self.private.stat()):
                self.assertTrue((self.state / "worktree.sqlite").is_file())
                seen.append(True)
                raise OSError(errno.EIO, "outer parent barrier")
            actual(fd)
        with patch.object(os, "fsync", side_effect=sync):
            with self.assertRaisesRegex(Rejected, "无法确认本次登记完成"): selected.run(confirmed=True)
        self.assertEqual(seen, [True])
        self.assertIsNone(selected.take_session())
        self.assertEqual(worktree.status(self.state)["base_generation"], 0)  # Visibility is not the missing barrier.
        self.assertEqual(list(self.store.iterdir()), [])

    def test_visible_first_file_without_receipt_retries_only_original_ack(self):
        live = self.register()
        attempt = self.first(live)
        launch = worktree._publish
        def after_output(*args, **kwargs):
            launch(*args, **kwargs)
            raise OSError(errno.EIO, "lost first receipt after successful native publication")
        with patch.object(worktree, "_publish", side_effect=after_output):
            with self.assertRaises(OSError): attempt.run()
        self.assertEqual(attempt.result()[0], "pending")
        original = self.store / attempt.marker.filename
        archived = original.read_bytes()
        self.story.write_bytes(b"new edited text must not be captured on retry")
        retry = PendingRetry(attempt)
        self.addCleanup(retry.release)
        self.assertIn("只确认这个原文件，不上传", retry.prepare())
        with patch.object(worktree, "frozen_tree", side_effect=AssertionError("no refreeze")), \
             patch.object(publish, "_new_upload", side_effect=AssertionError("no reupload")):
            final, marker = retry.run()
        self.assertEqual((final, marker, original.read_bytes()), (original, attempt.marker, archived))
        self.assertEqual(live.status()["base_operation"], attempt.operation)
        self.assertEqual(self.story.read_bytes(), b"new edited text must not be captured on retry")

    def test_first_slot_occupied_after_review_stops_before_capturing_files(self):
        live = self.register()
        attempt = self.first(live)
        occupied = self.store / attempt.first_slot
        occupied.write_bytes(b"not ours")
        with patch.object(live_candidate.LiveArchiveCandidate, "build", side_effect=AssertionError("must not scan work files")):
            with self.assertRaisesRegex(Rejected, "第 1 代名称已占用"): attempt.run()
        self.assertEqual(occupied.read_bytes(), b"not ours")
        self.assertIsNone(live.status()["pending_operation"])
        self.assertEqual(attempt.result()[0], "unrecorded")

    def test_orphan_pending_digest_under_the_writer_stops_the_first_capture(self):
        live = self.register()
        attempt = self.first(live)
        with worktree.locked(self.state) as (con, _, __):
            con.execute("UPDATE checkout SET pending_sha=? WHERE singleton=1", ("a"*64,))
        with patch.object(live_candidate.LiveArchiveCandidate, "build", side_effect=AssertionError("inconsistent state must not capture")):
            with self.assertRaisesRegex(Rejected, "incomplete original pending record"): attempt.run()
        self.assertEqual(list(self.store.iterdir()), [])
        with worktree.locked(self.state, read_only=True) as (_, row, __):
            self.assertEqual((row["pending"], row["pending_sha"]), (None, "a"*64))

    def test_first_generation_conflict_never_offers_a_second_root(self):
        live = self.register()
        attempt = self.first(live)
        launch = worktree._publish
        winner_file = self.store / attempt.first_slot
        def same_slot(local, scope, marker, digest, **kw):
            # STORE races after the original pending intent was durably recorded.
            body = b"another valid genesis, not a corrupt or empty placeholder"
            tree = Manifest()
            tree.add("story", False, len(body), hashlib.sha256(body).hexdigest())
            other = ("1" if marker.operation == "0"*32 else "0")*32
            winner = Commit(marker.repo, marker.branch, 1, other, None, tree.digest())
            with zipfile.ZipFile(winner_file, "x", compression=zipfile.ZIP_STORED) as archive:
                archive.writestr("story", body)
                archive.comment = winner.encode()
            return launch(local, scope, marker, digest, **kw)
        with patch.object(worktree, "_publish", side_effect=same_slot):
            with self.assertRaises(publish.Conflict): attempt.run()
        tag, detail = attempt.result()
        self.assertEqual((tag, attempt.pending_seen, attempt.conflict_pending_seen), ("pending", False, False))
        self.assertIn("不在窗口另造另一仓库起点", detail)
        # The direct backend choice must also reject even when called without a UI.
        attempt.conflict_pending_seen = True
        with self.assertRaisesRegex(Rejected, "第二个起点"): PendingFork(attempt)
        self.assertEqual(list(self.store.iterdir()), [winner_file])
        winner, _ = read_archive(winner_file)
        self.assertIsNone(winner.parent)
        self.assertNotEqual(winner.operation, attempt.operation)
        self.assertEqual(live.status()["pending_operation"], attempt.operation)

    @unittest.skipUnless(find_spec("PySide6"), "optional Qt window")
    def test_real_window_starts_without_snapshot_and_requires_independent_commit(self):
        from PySide6.QtWidgets import QApplication, QLabel, QPushButton
        from zipworkspace.desktop import checkout_pane, commit_controls
        from zipworkspace.desktop.widgets import Table
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        window = Window()
        self.addCleanup(window.pool.shutdown)
        self.addCleanup(window.archive.checkout.close_session)
        pane = window.archive.checkout
        self.assertIsNone(pane.snapshot)
        self.assertTrue(pane.initialize.isEnabled())
        self.assertFalse(pane.start.isEnabled())

        def start(control, expected_jobs, *, before_poll=None):
            # Capture real call/receipt flow through Window.job and Window.poll.
            works = []
            def submit(work):
                result = Future()
                works.append((result, work))
                return result
            with patch.object(window.pool, "submit", side_effect=submit):
                control.click()
                for i in range(expected_jobs):
                    self.assertGreater(len(works), i)
                    future, call = works[i]
                    future.set_result(call())
                    if before_poll is not None: before_poll()
                    window.poll()
                self.assertEqual(len(works), expected_jobs)

        def choose(permit, jobs):
            with patch.object(pane, "_directory", side_effect=map(str, (self.work, self.store, self.private))), \
                 patch.object(checkout_pane.QInputDialog, "getText", return_value=(self.state.name, True)), \
                 patch.object(checkout_pane, "confirm", return_value=permit) as accept:
                start(pane.initialize, jobs)
                self.assertEqual(accept.call_count, 1)
                self.assertIn("不在 STORE 创建归档", accept.call_args.args[2])

        choose(False, 1)
        self.assertFalse(self.state.exists())
        choose(True, 3)  # Read-only review, explicit initialization, read-only status.
        self.assertIsNotNone(pane.live)
        self.assertIn("尚无已提交快照", pane.details.text())
        self.assertFalse(pane.initialize.isEnabled())
        self.assertTrue(pane.changes.isEnabled())
        self.assertIn("查看首次提交", pane.changes.text())
        self.assertFalse(pane.initial_changes.isEnabled())
        for i in range(100): (self.work/f"part-{i:03}").touch()
        start(pane.changes, 1, before_poll=window.cancel_transfer)
        self.assertIsNone(pane.changes_dialog)
        self.assertIn("已取消首次提交内容清单", window.note.text())
        start(pane.changes, 1)
        dialog = pane.changes_dialog
        self.assertEqual(dialog.windowTitle(), "首次提交内容清单")
        self.assertTrue(any(f"{self.story.stat().st_size:,} B" in title.text()
                            for title in dialog.findChildren(QLabel)))
        table = dialog.findChild(Table)
        self.assertEqual(table.rows.columnCount(), 2)
        self.assertEqual(table.rows.rowCount(), 100)
        self.assertIsNone(dialog.findChild(QPushButton, "restore-selected-missing-member"))
        next_ = next(control for control in dialog.findChildren(QPushButton) if control.text() == "下一页")
        next_.click()
        self.assertEqual([row.path for _, row in table.rows.rows], ["part-099", "story"])
        dialog.reject()
        self.assertIsNone(pane.changes_dialog)
        self.assertEqual(list(self.store.iterdir()), [])
        with patch.object(commit_controls, "confirm", return_value=True) as submit:
            start(pane.commits.save, 3)  # Distinct consent: prepare, commit, status.
            self.assertIn("第 1 代", submit.call_args.args[2])
            self.assertIn("停止所有工作树写入者", submit.call_args.kwargs["attestation"])
        self.assertEqual(len(list(self.store.glob("*.zxs"))), 1)
        self.assertIn("基线第 1 代", pane.details.text())
        self.assertEqual(pane.changes.text(), "查看工作目录相对基线的改动…")
