"""Opt-in Linux source fixtures for moving only the original, still-held clean worktree."""
from concurrent.futures import Future
from contextlib import ExitStack, closing
import errno
from importlib.util import find_spec
import os
from pathlib import Path
import sqlite3
import stat
import sys
from dataclasses import replace
import unittest
from unittest.mock import patch

from zipworkspace import lifecycle, publish, worktree
from zipworkspace.capacity import describe
from zipworkspace.desktop.commit_choice import CommitChoice
from zipworkspace.desktop.initialize_choice import InitializeChoice
from zipworkspace.desktop.member_copy import RestoreMissingMember
from zipworkspace.linux_authority import ProcView, owned_directory
from zipworkspace.linux_mount import _ordinary_user
from zipworkspace.model import Rejected, new_id
from zipworkspace.private_freeze import local_mount


@unittest.skipUnless(sys.platform == "linux", "same local Linux ext4/XFS worktree visibility")
class LinuxVisibility(unittest.TestCase):
    def setUp(self):
        spelling = os.environ.get("ZXS_LINUX_VISIBILITY_SCRATCH")
        if not spelling: self.skipTest("supply an exclusive synthetic fixture directory in ZXS_LINUX_VISIBILITY_SCRATCH")
        scratch = Path(spelling)
        if scratch.anchor != "/" or str(scratch) != spelling or ".." in scratch.parts:
            self.fail("give an existing literal absolute path")
        leaf = "visibility-" + new_id()
        with ExitStack() as held:
            proc = ProcView(held)
            uid = _ordinary_user(read_proc=proc.read)
            parent, _ = owned_directory(held, scratch, uid, proc.read)
            local_mount(parent, read_proc=proc.read)
            self.assertEqual(stat.S_IMODE(os.fstat(parent).st_mode), 0o700)
            os.mkdir(leaf, 0o700, dir_fd=parent)
        # Dedicated synthetic cases are retained, not removed by reused pathname.
        root = scratch / leaf
        self.work_parent, self.private, self.store = (root / n for n in ("trees", "states", "archives"))
        for folder in (self.work_parent, self.private, self.store): folder.mkdir(mode=0o700)
        self.tree, self.state = self.work_parent / "work", self.private / "original"
        self.tree.mkdir(mode=0o700)
        (self.tree/"empty").mkdir(mode=0o700)
        (self.tree/"story").write_bytes(b"initial")
        action = InitializeChoice()
        self.addCleanup(action.close)
        action.pick_work(self.tree); action.pick_private(self.private); action.pick_store(self.store)
        action.review(self.state.name); action.run(confirmed=True)
        self.live = action.take_session()
        self.addCleanup(self.live.close)
        self.hidden = self.tree.with_name(".zip-workspace-hidden-" + self.live.instance)

    def commit(self):
        plan = CommitChoice(self.live)
        try:
            plan.prepare()
            self.archive, self.marker = plan.run()
        finally: plan.release()
        self.archive_bytes = self.archive.read_bytes()

    def plan(self, direction, *, prepare=True):
        plan = lifecycle.LiveVisibility(self.live, direction)
        self.addCleanup(plan.close)
        if prepare:
            preview = plan.prepare()
            self.assertIn(str(self.tree), preview)
            self.assertIn(str(self.hidden), preview)
            self.assertIn(str(self.state), preview)
            self.assertIn(str(self.store), preview)
            self.assertIn(self.marker.operation, preview)
            self.assertIn(plan.operation, preview)
        return plan

    def missing_decision(self, name="story"):
        marker, report, evidence = lifecycle.observe_live_changes(self.live, retain_missing=True)
        self.assertEqual(marker, self.marker)
        self.assertIsNotNone(evidence)
        with closing(evidence):
            row = next(r for r in report.rows if r.path == name)
            with self.assertRaisesRegex(Rejected, "本次基线对照"):
                RestoreMissingMember(self.live, evidence, replace(row))
            decision = RestoreMissingMember(self.live, evidence, row)
            self.addCleanup(decision.close)
        self.assertIsNone(evidence.held)
        return decision

    def test_restore_one_missing_file_uses_original_basis_and_does_not_commit(self):
        (self.tree/"other").mkdir(mode=0o700)
        (self.tree/"other/story").write_bytes(b"different original basename")
        self.commit()
        other = self.tree/"other/story"
        other.write_bytes(b"later edit remains")
        target = self.tree/"story"
        target.unlink()
        decision = self.missing_decision()
        baseline = self.live.status()
        originals = (self.state/"worktree.sqlite").read_bytes()
        with patch.object(lifecycle, "read_managed_document", side_effect=AssertionError("no second full tree read")), \
             patch.object(lifecycle, "archive_sha", side_effect=AssertionError("no second ZIP read")), \
             patch.object(worktree, "_publish", side_effect=AssertionError("restore is not a commit")):
            self.assertIn(self.marker.operation, decision.review())
            self.assertFalse(target.exists())
            with self.assertRaises(Rejected): decision.run()
            self.assertIn("已另存并核验", decision.run(confirmed=True))
        self.assertEqual(target.read_bytes(), b"initial")
        self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o600)
        self.assertEqual(other.read_bytes(), b"later edit remains")
        self.assertEqual(self.archive.read_bytes(), self.archive_bytes)
        self.assertEqual((self.state/"worktree.sqlite").read_bytes(), originals)
        self.assertEqual(self.live.status(), baseline)
        with self.assertRaises(Rejected): decision.run(confirmed=True)
        _, changes, evidence = lifecycle.observe_live_changes(self.live, retain_missing=True)
        self.assertIsNone(evidence)
        self.assertEqual((changes.missing, changes.changed), (0, 1))

    def test_restore_refuses_late_name_without_overwriting_it(self):
        self.commit()
        target = self.tree/"story"
        target.unlink()
        decision = self.missing_decision()
        decision.review()
        native = os.link
        occupied = []
        def late(source, name, **options):
            target.write_bytes(b"created by another application")
            occupied.append(target.stat())
            return native(source, name, **options)
        with patch.object(os, "link", side_effect=late) as link:
            with self.assertRaisesRegex(Rejected, "没有覆盖"): decision.run(confirmed=True)
            link.assert_called_once()
        self.assertTrue(os.path.samestat(occupied[0], target.stat()))
        self.assertEqual(target.read_bytes(), b"created by another application")
        self.assertIsNone(self.live.status()["pending_operation"])

    def test_restore_refuses_missing_parent_or_pending_without_payload(self):
        parent = self.tree/"other"
        parent.mkdir(mode=0o700)
        nested = parent/"story"
        nested.write_bytes(b"previous contents of a nested file")
        self.commit()
        nested.unlink(); parent.rmdir()
        decision = self.missing_decision("other/story")
        with self.assertRaisesRegex(Rejected, "原目的目录"): decision.review()
        self.assertFalse(parent.exists())

        (self.tree/"story").unlink()
        pending = self.missing_decision()
        pending.review()
        with worktree.locked(self.state) as (con, _, __):
            con.execute("UPDATE checkout SET pending_sha=? WHERE singleton=1", ("b"*64,))
        with patch.object(pending, "_payload", side_effect=AssertionError("stale state must not start copy")), \
             patch.object(os, "link", side_effect=AssertionError("stale state must not publish")):
            with self.assertRaisesRegex(Rejected, "无待决操作"): pending.run(confirmed=True)
        self.assertFalse((self.tree/"story").exists())

    def test_restore_will_not_adopt_an_equal_byte_replacement_of_the_held_base(self):
        self.commit()
        target = self.tree/"story"
        target.unlink()
        decision = self.missing_decision()
        decision.review()
        retained = self.archive.with_suffix(".original")
        self.archive.rename(retained)
        self.archive.write_bytes(self.archive_bytes)
        with patch.object(decision, "_payload", side_effect=AssertionError("do not stage from an obsolete source")):
            with self.assertRaisesRegex(Rejected, "原归档已被移走、替换"): decision.run(confirmed=True)
        self.assertFalse(target.exists())

    def test_restore_empty_member_and_parent_sync_error_have_no_second_attempt(self):
        target = self.tree / "zero"
        target.write_bytes(b"")
        self.commit()
        target.unlink()
        decision = self.missing_decision("zero")
        decision.review()
        native = os.fsync
        calls = []
        def sync(fd):
            if os.path.samestat(os.fstat(fd), self.tree.stat()):
                calls.append(fd)
                raise OSError(errno.EIO, "synthetic selected parent sync failure")
            return native(fd)
        with patch.object(os, "fsync", side_effect=sync):
            with self.assertRaisesRegex(publish.Uncertain, "完整副本可能已经出现"):
                decision.run(confirmed=True)
        self.assertEqual(len(calls), 1)
        self.assertEqual(target.read_bytes(), b"")
        self.assertIsNone(self.live.status()["pending_operation"])
        with self.assertRaises(Rejected): decision.run(confirmed=True)
        self.assertEqual(target.read_bytes(), b"")

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1" and find_spec("PySide6"),
                         "optional Qt file recovery fixture")
    def test_only_explicit_selected_missing_row_can_request_window_recovery(self):
        from PySide6.QtWidgets import QApplication, QPushButton
        from zipworkspace.desktop import checkout_pane
        from zipworkspace.desktop.widgets import Table
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        self.commit()
        target = self.tree/"story"
        target.unlink()
        window = Window()
        pane = window.archive.checkout
        pane.live = self.live
        jobs = []

        def queue(action):
            future = Future()
            jobs.append((future, action))
            return future

        def completed():
            future, action = jobs.pop(0)
            future.set_result(action())
            window.poll()

        try:
            with patch.object(window.pool, "submit", side_effect=queue):
                for allow in (False, True):
                    pane.compare_live(); completed()
                    dialog = pane.changes_dialog
                    self.assertIsNotNone(dialog)
                    recovery = dialog.findChild(QPushButton, "restore-selected-missing-member")
                    self.assertFalse(recovery.isEnabled())
                    dialog.findChild(Table).selectRow(0)
                    self.assertTrue(recovery.isEnabled())
                    with patch.object(checkout_pane, "confirm", return_value=allow) as consent:
                        recovery.click()
                        self.assertIsNone(pane.changes_dialog)
                        self.assertFalse(target.exists())
                        completed()
                    consent.assert_called_once()
                    self.assertIn(str(target), consent.call_args.args[2])
                    self.assertEqual(len(jobs), int(allow))
                    if allow: completed()
                    self.assertIsNone(pane.restoring_member)
            self.assertEqual(target.read_bytes(), b"initial")
        finally:
            window.close()

    def test_round_trip_renames_same_inode_after_observation_and_durable_intent(self):
        self.commit()
        actual = lifecycle.rename_new_at
        seen = []
        def rename(parent, old, new, *, expected_source):
            self.assertTrue(os.path.samestat(os.fstat(parent), self.work_parent.stat()))
            self.assertTrue(os.path.samestat(expected_source, os.fstat(self.live.work_fd)))
            recorded = worktree.status(self.state)
            seen.append((old, new, recorded["pending_visibility_operation"]))
            actual(parent, old, new, expected_source=expected_source)

        hide = self.plan("hide")
        with self.assertRaises(Rejected): hide.run()
        self.assertEqual(self.live.status()["visibility"], "visible")
        self.assertTrue(self.tree.is_dir())
        with patch.object(lifecycle, "rename_new_at", side_effect=rename), \
             patch.object(lifecycle, "observe_held", wraps=lifecycle.observe_held) as scan:
            self.assertEqual(hide.run(confirmed=True), "hidden")
            self.assertTrue(hide.archive.closed)
            self.assertFalse(self.tree.exists())
            self.assertTrue(os.path.samestat(self.hidden.stat(), os.fstat(self.live.work_fd)))
            restore = self.plan("restore")
            self.assertEqual(restore.run(confirmed=True), "visible")
            self.assertEqual(scan.call_count, 2)  # Once per approved action, each with two inventories.
        self.assertEqual(seen, [(self.tree.name, self.hidden.name, hide.operation),
                                (self.hidden.name, self.tree.name, restore.operation)])
        self.assertTrue(os.path.samestat(self.tree.stat(), os.fstat(self.live.work_fd)))
        self.assertEqual(hide.result()[0], "completed")  # The old receipt is not a new hide.
        self.assertEqual(restore.result()[0], "completed")
        self.assertFalse(self.hidden.exists())
        self.assertEqual((self.tree/"story").read_bytes(), b"initial")
        self.assertEqual(list((self.tree/"empty").iterdir()), [])
        self.assertEqual(self.archive.read_bytes(), self.archive_bytes)
        with self.assertRaises(Rejected): hide.run(confirmed=True)

    def test_only_positive_clean_idle_original_without_orphan_pending_sha(self):
        with self.assertRaisesRegex(Rejected, "已提交基线"): self.plan("hide", prepare=False).prepare()
        self.commit()
        dirty = self.plan("hide")
        (self.tree/"story").write_bytes(b"changed")  # Same length, different SHA.
        with patch.object(lifecycle, "_start_visibility", side_effect=AssertionError("dirty never writes intent")):
            with self.assertRaisesRegex(Rejected, "未提交改动"): dirty.run(confirmed=True)
        self.assertFalse(dirty.started)
        self.assertTrue(self.tree.exists())
        (self.tree/"story").write_bytes(b"initial")
        pending = self.plan("hide")
        with worktree.locked(self.state) as (con, _, __):
            con.execute("UPDATE checkout SET pending_sha=? WHERE singleton=1", ("b"*64,))
        with patch.object(lifecycle, "observe_held", side_effect=AssertionError("pending never scans")):
            with self.assertRaisesRegex(Rejected, "无待决工作树"): pending.run(confirmed=True)
        self.assertFalse(pending.started)
        self.assertEqual(self.live.status()["visibility"], "visible")
        self.assertFalse(self.hidden.exists())

    def test_restore_preserves_a_real_write_through_an_editor_fd_open_before_hiding(self):
        self.commit()
        baseline = self.live.status()
        file_id = (self.tree/"story").stat()
        hide = self.plan("hide")
        with (self.tree/"story").open("r+b", buffering=0) as editor:
            self.assertEqual(hide.run(confirmed=True), "hidden")
            self.assertFalse(self.tree.exists())
            self.assertEqual(editor.write(b"edited!"), 7)
        with worktree.locked(self.state, read_only=True) as (con, _, __):
            history = tuple(lifecycle._visibility_row(con, hide.operation))
        with self.assertRaisesRegex(Rejected, "CLI restore requires the committed tree"):
            lifecycle.start(self.state, "restore")
        self.assertEqual((self.hidden/"story").read_bytes(), b"edited!")
        self.assertEqual(self.live.status()["visibility"], "hidden")
        self.assertIsNone(self.live.status()["pending_visibility_operation"])

        restore = self.plan("restore")
        self.assertEqual(restore.run(confirmed=True), "visible")
        self.assertTrue(os.path.samestat(self.tree.stat(), os.fstat(self.live.work_fd)))
        self.assertTrue(os.path.samestat((self.tree/"story").stat(), file_id))
        self.assertFalse(self.hidden.exists())
        self.assertEqual((self.tree/"story").read_bytes(), b"edited!")
        self.assertEqual(self.live.status(), baseline)
        with worktree.locked(self.state, read_only=True) as (con, _, __):
            self.assertEqual(tuple(lifecycle._visibility_row(con, hide.operation)), history)
            accepted = lifecycle._visibility_row(con, restore.operation)
            self.assertEqual((accepted["action"], accepted["result"]), ("restore", "visible"))
            self.assertNotEqual(accepted["original_tree"], self.marker.tree)
        self.assertEqual(sorted(self.store.iterdir()), [self.archive])
        self.assertEqual(self.archive.read_bytes(), self.archive_bytes)
        marker, changes = lifecycle.observe_live_changes(self.live)
        self.assertEqual(marker, self.marker)
        self.assertEqual([row.path for row in changes.rows], ["story"])

    def test_interrupted_restore_can_only_complete_the_same_accepted_tree(self):
        self.commit()
        self.plan("hide").run(confirmed=True)
        (self.hidden/"story").write_bytes(b"edited!")
        fd = self.live.work_fd
        held = os.fstat(fd)
        observed = lifecycle.observe_held(fd, (held.st_dev, held.st_ino)).digest()
        restore = self.plan("restore")
        actual = os.fsync
        def fail_parent(fd):
            if os.path.samestat(os.fstat(fd), self.work_parent.stat()):
                self.assertTrue(os.path.samestat(self.tree.stat(), held))
                raise OSError(errno.EIO, "failed restore parent barrier")
            actual(fd)
        with patch.object(lifecycle.os, "fsync", side_effect=fail_parent):
            with self.assertRaisesRegex(OSError, "failed restore parent barrier"): restore.run(confirmed=True)
        self.assertEqual(self.live.status()["pending_visibility_operation"], restore.operation)
        with worktree.locked(self.state, read_only=True) as (con, _, __):
            receipt = lifecycle._visibility_row(con, restore.operation)
            self.assertEqual((receipt["original_tree"], receipt["result"]), (observed, None))
        outcome, detail = restore.result()
        self.assertEqual(outcome, "pending")
        self.assertIn(f"resume-visibility STATE {restore.operation}", detail)
        with patch.object(lifecycle, "rename_new", side_effect=AssertionError("already moved")), \
             patch.object(lifecycle, "observe_tree", side_effect=AssertionError("resume must read the held original")):
            (self.tree/"story").write_bytes(b"newer!!")
            with self.assertRaisesRegex(Rejected, "accepted by this exact visibility operation"):
                lifecycle.finish(self.state, restore.operation)
            self.assertEqual((self.tree/"story").read_bytes(), b"newer!!")
            self.assertEqual(self.live.status()["pending_visibility_operation"], restore.operation)
            (self.tree/"story").write_bytes(b"edited!")
            self.assertEqual(lifecycle.finish(self.state, restore.operation)["original_outcome"], "visible")
        self.assertEqual(self.live.status()["base_operation"], self.marker.operation)
        self.assertEqual((self.tree/"story").read_bytes(), b"edited!")
        self.assertEqual(self.archive.read_bytes(), self.archive_bytes)

    def test_restore_only_gives_hidden_location_when_it_still_names_the_original(self):
        self.commit()
        self.plan("hide").run(confirmed=True)
        self.tree.mkdir(mode=0o700)
        (self.tree/"untouched").write_bytes(b"occupied")
        with self.assertRaises(Rejected) as conflict:
            self.plan("restore", prepare=False).prepare()
        note = describe(conflict.exception)
        self.assertIn(f"本次核到原隐藏目录：{str(self.hidden)!r}", note)
        self.assertIn("独立的新目录，不覆盖任何文件", note)
        original = self.hidden.with_name("saved-original")
        self.hidden.rename(original)
        self.hidden.mkdir(mode=0o700)
        (self.hidden/"story").write_bytes(b"initial")
        with self.assertRaises(Rejected) as replacement:
            self.plan("restore", prepare=False).prepare()
        self.assertNotIn(str(self.hidden), describe(replacement.exception))
        self.assertEqual((original/"story").read_bytes(), b"initial")
        self.assertEqual((self.tree/"untouched").read_bytes(), b"occupied")
        self.assertIsNone(worktree.status(self.state)["pending_visibility_operation"])

    def test_does_not_use_a_replacement_work_parent_even_with_the_same_work_inode(self):
        self.commit()
        plan = self.plan("hide")
        retained = self.work_parent.with_name("original-parent")
        self.work_parent.rename(retained)
        self.work_parent.mkdir(mode=0o700)
        (retained/self.tree.name).rename(self.tree)
        self.assertTrue(os.path.samestat(self.tree.stat(), os.fstat(self.live.work_fd)))
        with patch.object(worktree, "locked", side_effect=AssertionError("wrong parent cannot open the writer")):
            with self.assertRaisesRegex(Rejected, "原父或上级路径已变化"): plan.run(confirmed=True)
        self.assertFalse(plan.started)
        self.assertTrue(self.tree.is_dir())
        self.assertFalse(self.hidden.exists())

    def test_same_archive_bytes_in_a_new_slot_do_not_authorize_this_original_move(self):
        self.commit()
        plan = self.plan("hide")
        saved = self.archive.with_name("preserved.zxs")
        self.archive.rename(saved)
        self.archive.write_bytes(self.archive_bytes)
        with patch.object(worktree, "locked", side_effect=AssertionError("wrong source cannot open the writer")):
            with self.assertRaises(Rejected): plan.run(confirmed=True)
        self.assertFalse(plan.started)
        self.assertEqual(saved.read_bytes(), self.archive.read_bytes())
        self.assertTrue(self.tree.exists())
        self.assertFalse(self.hidden.exists())

    def test_late_empty_original_name_is_never_overwritten_by_restore(self):
        self.commit()
        self.plan("hide").run(confirmed=True)
        restore = self.plan("restore")
        actual, occupied = lifecycle.rename_new_at, []
        def race(*args, **kw):
            self.tree.mkdir(mode=0o700)
            occupied.append(self.tree.stat())
            actual(*args, **kw)
        with patch.object(lifecycle, "rename_new_at", side_effect=race):
            with self.assertRaisesRegex(Rejected, "刚被占用"): restore.run(confirmed=True)
        self.assertTrue(os.path.samestat(self.tree.stat(), occupied[0]))
        self.assertTrue(os.path.samestat(self.hidden.stat(), os.fstat(self.live.work_fd)))
        self.assertEqual(list(self.tree.iterdir()), [])
        outcome, note = restore.result()
        self.assertEqual(outcome, "pending")
        self.assertNotIn("resume-visibility", note)
        self.assertEqual(self.live.status()["pending_visibility_operation"], restore.operation)

    def test_after_move_parent_sync_failure_is_pending_then_original_receipt_may_be_read(self):
        self.commit()
        plan = self.plan("hide")
        real = os.fsync
        def stop(fd):
            if os.path.samestat(os.fstat(fd), self.work_parent.stat()):
                self.assertTrue(os.path.samestat(self.hidden.stat(), os.fstat(self.live.work_fd)))
                raise OSError(errno.EIO, "original parent barrier")
            real(fd)
        with patch.object(lifecycle.os, "fsync", side_effect=stop):
            with self.assertRaisesRegex(OSError, "original parent barrier"): plan.run(confirmed=True)
        with patch.object(lifecycle, "finish", side_effect=AssertionError("query does not resume")), \
             patch.object(lifecycle, "rename_new_at", side_effect=AssertionError("query does not rename")), \
             patch.object(lifecycle.os, "fsync", side_effect=AssertionError("query cannot confirm durability")):
            status, detail = plan.result()
            self.assertEqual(status, "pending")
            self.assertIn("没有完成回执", detail)
            self.assertIn(plan.operation, detail)
        self.assertEqual(self.live.status()["pending_visibility_operation"], plan.operation)
        with patch.object(lifecycle, "rename_new", side_effect=AssertionError("CLI sees exact destination; no second move")):
            lifecycle.finish(self.state, plan.operation)
        self.assertEqual(plan.result()[0], "completed")
        self.assertEqual(self.live.status()["visibility"], "hidden")

    def test_window_resume_moves_only_the_original_already_synced_intent(self):
        self.commit()
        plan = self.plan("hide")
        actual = lifecycle.rename_new_at
        with patch.object(lifecycle, "rename_new_at", side_effect=OSError(errno.EIO, "move reply failed")):
            with self.assertRaises(OSError): plan.run(confirmed=True)
        self.assertTrue(plan.intent_synced)
        self.assertTrue(plan.archive.closed)
        self.assertTrue(self.tree.is_dir())

        decision, details = plan.review_resume()
        self.assertEqual(decision, ("move",))
        self.assertIn(plan.operation, details)
        plan.cancel_resume(decision)
        with self.assertRaisesRegex(Rejected, "最新的一次继续确认"):
            plan.resume(decision, confirmed=True)
        decision, _ = plan.review_resume()
        with patch.object(lifecycle, "rename_new_at", wraps=actual) as move, \
             patch.object(lifecycle, "_start_visibility", side_effect=AssertionError("no second intent")), \
             patch.object(lifecycle, "relative_document", side_effect=AssertionError("no reopened base")):
            self.assertEqual(plan.resume(decision, confirmed=True), "hidden")
            move.assert_called_once()
        self.assertEqual(plan.result()[0], "completed")
        self.assertTrue(os.path.samestat(self.hidden.stat(), os.fstat(self.live.work_fd)))
        self.assertEqual(self.live.status()["base_operation"], self.marker.operation)
        self.assertEqual(self.archive.read_bytes(), self.archive_bytes)

    def test_window_restore_ack_never_switches_to_rename_or_accepts_later_contents(self):
        self.commit()
        self.plan("hide").run(confirmed=True)
        (self.hidden/"story").write_bytes(b"edited!")
        plan = self.plan("restore")
        actual = os.fsync
        def fail_parent(fd):
            if os.path.samestat(os.fstat(fd), self.work_parent.stat()):
                self.assertTrue(self.tree.is_dir())
                raise OSError(errno.EIO, "parent barrier after move")
            actual(fd)
        with patch.object(lifecycle.os, "fsync", side_effect=fail_parent):
            with self.assertRaises(OSError): plan.run(confirmed=True)
        decision, details = plan.review_resume()
        self.assertEqual(decision, ("ack",))
        self.assertIn("不再次改名", details)
        self.tree.rename(self.hidden)  # Model a changed physical pair during the confirmation.
        with patch.object(lifecycle, "rename_new_at", side_effect=AssertionError("ACK can never move")):
            with self.assertRaisesRegex(Rejected, "不切换"):
                plan.resume(decision, confirmed=True)
            self.assertTrue(self.hidden.is_dir())
            self.hidden.rename(self.tree)
            (self.tree/"story").write_bytes(b"later!!")
            decision, _ = plan.review_resume()
            with self.assertRaisesRegex(Rejected, "不等于原号接受的内容"):
                plan.resume(decision, confirmed=True)
            self.assertEqual((self.tree/"story").read_bytes(), b"later!!")
            self.assertEqual(self.live.status()["pending_visibility_operation"], plan.operation)
            (self.tree/"story").write_bytes(b"edited!")
            decision, _ = plan.review_resume()
            self.assertEqual(plan.resume(decision, confirmed=True), "visible")
        self.assertEqual(plan.result()[0], "completed")
        self.assertTrue(os.path.samestat(self.tree.stat(), os.fstat(self.live.work_fd)))
        self.assertEqual((self.tree/"story").read_bytes(), b"edited!")

    def test_intent_database_directory_barrier_failure_never_moves_or_suggests_resume(self):
        self.commit()
        plan = self.plan("hide")
        actual = os.fsync
        def sync(fd):
            if os.path.samestat(os.fstat(fd), self.state.stat()):
                self.assertEqual(self.live.status()["pending_visibility_operation"], plan.operation)
                self.assertTrue(self.tree.is_dir())
                raise OSError(errno.EIO, "first original STATE barrier")
            actual(fd)
        with patch.object(lifecycle.os, "fsync", side_effect=sync), \
             patch.object(lifecycle, "rename_new_at", side_effect=AssertionError("no move without intent barrier")):
            with self.assertRaisesRegex(OSError, "first original STATE barrier"): plan.run(confirmed=True)
        self.assertTrue(plan.archive.closed)
        self.assertFalse(plan.intent_synced)
        self.assertFalse(self.hidden.exists())
        outcome, readback = plan.result()
        self.assertEqual(outcome, "pending")
        self.assertIn("没有确认原意图已同步", readback)
        self.assertNotIn("resume-visibility", readback)
        with patch.object(worktree, "locked", side_effect=AssertionError("no valid intent")):
            with self.assertRaisesRegex(Rejected, "没有本窗口已确认同步"): plan.review_resume()

    def test_failed_state_receipt_barrier_does_not_upgrade_a_visible_completion_row(self):
        self.commit()
        plan = self.plan("hide")
        actual, calls = os.fsync, []
        def sync(fd):
            if os.path.samestat(os.fstat(fd), self.state.stat()):
                calls.append(self.live.status()["visibility"])
                if len(calls) == 2: raise OSError(errno.EIO, "last original STATE barrier")
            actual(fd)
        with patch.object(lifecycle.os, "fsync", side_effect=sync):
            with self.assertRaisesRegex(OSError, "last original STATE barrier"): plan.run(confirmed=True)
        self.assertEqual(calls, ["hide-pending", "hidden"])
        self.assertTrue(plan.archive.closed)
        self.assertTrue(os.path.samestat(self.hidden.stat(), os.fstat(self.live.work_fd)))
        result, note = plan.result()
        self.assertEqual(result, "recorded")
        self.assertIn("没有确认原 STATE", note)
        self.assertNotIn("resume-visibility", note)

    def test_existing_operation_number_cannot_be_taken_as_the_new_decision(self):
        self.commit()
        plan = self.plan("hide")
        with worktree.locked(self.state) as (con, _, __):
            con.execute("INSERT INTO visibility_log VALUES(?,?,?,?)",
                        (plan.operation, "hide", self.marker.tree, "hidden"))
        with patch.object(lifecycle, "observe_held", side_effect=AssertionError("duplicate number never scans")):
            with self.assertRaisesRegex(Rejected, "操作号已在原日志中使用"): plan.run(confirmed=True)
        self.assertTrue(plan.archive.closed)
        self.assertFalse(plan.started)
        self.assertTrue(self.tree.exists())
        with self.assertRaises(Rejected): plan.result()

    def test_foreign_journal_mode_never_moves_or_silently_changes_mode(self):
        self.commit()
        plan = self.plan("hide")
        with closing(sqlite3.connect(self.state/"worktree.sqlite", isolation_level=None)) as native:
            self.assertEqual(native.execute("PRAGMA journal_mode=WAL").fetchone()[0], "wal")
        with patch.object(lifecycle, "observe_held", side_effect=AssertionError("unadmitted journal never scans")):
            with self.assertRaisesRegex(Rejected, "回滚日志模式"): plan.run(confirmed=True)
        with closing(sqlite3.connect(self.state/"worktree.sqlite", isolation_level=None)) as native:
            self.assertEqual(native.execute("PRAGMA journal_mode").fetchone()[0], "wal")
        self.assertFalse(plan.started)
        self.assertTrue(plan.archive.closed)
        self.assertFalse(self.hidden.exists())

    @unittest.skipUnless(find_spec("PySide6"), "requires the optional desktop Qt interface")
    def test_real_window_resume_follows_original_query_and_requires_a_fresh_confirmation(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import checkout_pane
        from zipworkspace.desktop.window import Window
        self.commit()
        app = QApplication.instance() or QApplication([])
        win = Window()
        self.addCleanup(win.close)
        pane = win.archive.checkout
        pane._attach(self.live)
        queue = []
        def submit(job):
            task = Future()
            queue.append((task, job))
            return task
        def drain(n):
            for _ in range(n):
                task, job = queue.pop(0)
                self.assertTrue(task.set_running_or_notify_cancel())
                task.set_result(job())
                win.poll()
            self.assertFalse(queue)
        with patch.object(win.pool, "submit", side_effect=submit), \
             patch.object(checkout_pane, "confirm", side_effect=(True, False, True)) as confirm:
            pane.refresh_live(); drain(1)
            pane.move.click()
            with patch.object(lifecycle, "rename_new_at", side_effect=OSError(errno.EIO, "move not issued")):
                drain(2)
            original = pane.visibility_last
            self.assertTrue(original.intent_synced)
            self.assertFalse(pane.move_resume.isEnabled())
            pane.move_query.click(); drain(2)
            self.assertTrue(pane.move_resume.isEnabled())
            pane.move_resume.click(); drain(1)
            self.assertIsNone(original.resume_review)
            self.assertTrue(self.tree.is_dir())
            self.assertFalse(self.hidden.exists())
            pane.move_resume.click(); drain(3)
            self.assertIs(pane.visibility_last, original)
            self.assertFalse(pane.move_resume.isEnabled())
            self.assertFalse(self.tree.exists())
            self.assertTrue(os.path.samestat(self.hidden.stat(), os.fstat(self.live.work_fd)))
            self.assertEqual(confirm.call_count, 3)
            self.assertIn(original.operation, confirm.call_args.args[2])

    @unittest.skipUnless(find_spec("PySide6"), "requires the optional desktop Qt interface")
    def test_real_window_click_cancel_and_confirm_then_restore_still_same_session(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import checkout_pane
        from zipworkspace.desktop.window import Window
        self.commit()
        app = QApplication.instance() or QApplication([])
        win = Window()
        self.addCleanup(win.close)
        pane = win.archive.checkout
        pane._attach(self.live)
        queue = []
        def submit(job):
            task = Future()
            queue.append((task, job))
            return task
        def drain(n):
            for _ in range(n):
                task, job = queue.pop(0)
                self.assertTrue(task.set_running_or_notify_cancel())
                task.set_result(job())
                win.poll()
            self.assertFalse(queue)
        with patch.object(win.pool, "submit", side_effect=submit), \
             patch.object(checkout_pane, "confirm", side_effect=(False, True, True)) as confirm:
            pane.refresh_live()
            drain(1)
            pane.move.click()
            drain(1)
            self.assertIsNone(pane.visibility_last)
            self.assertTrue(self.tree.is_dir())
            self.assertFalse(self.hidden.exists())
            pane.move.click()
            drain(3)
            self.assertIn("恢复", pane.move.text())
            self.assertFalse(pane.commits.save.isEnabled())
            self.assertTrue(self.hidden.is_dir())
            pane.move_query.click()
            drain(2)
            self.assertIn("历史结果", pane.move_result.text())
            pane.move.click()
            drain(3)
            self.assertTrue(pane.commits.save.isEnabled())
            self.assertTrue(self.tree.is_dir())
            self.assertFalse(self.hidden.exists())
            self.assertIs(pane.live, self.live)
            self.assertEqual(confirm.call_count, 3)
            self.assertIn("已关闭编辑器", confirm.call_args.kwargs["attestation"])
            self.assertIn("暂停后台同步", confirm.call_args.kwargs["attestation"])
            self.assertEqual(confirm.call_args.kwargs["action_text"], "恢复原路径并保留改动")
            self.assertIn("仍未提交", confirm.call_args.args[2])
