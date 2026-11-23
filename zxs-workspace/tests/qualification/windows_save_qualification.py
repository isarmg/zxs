"""Manual NTFS original-save custody qualification. Never connect, send or mount."""
from dataclasses import replace
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

if __name__ == "__main__":
    here = Path(__file__).resolve()
    sys.path[:0] = [str(here.parents[2]), str(here.parent)]

from windows_draft_qualification import LocalWorkingFile
from windows_node_qualification import CONTAINER, ORIGIN, TARGET
from zipworkspace._windows_access import NativeSecurity
from zipworkspace._windows_output import PinnedParent
from zipworkspace.desktop.container_windows_draft import _effective_zone
from zipworkspace.desktop import container_windows_save as saves
from zipworkspace.model import Rejected


class OriginalSave(LocalWorkingFile):
    def edited(self, before=b"seed", after=b"edited"):
        choice, _ = self.choose_draft(before, max_bytes=max(64, len(after)))
        draft = choice.begin(confirmed=True)
        self.addCleanup(draft.close)
        h = draft.open(writable=True)
        draft.resize(h, 0)
        draft.write(h, 0, after)
        draft.release(h)
        draft.begin_drain()
        return choice, draft, draft.review(confirmed=True)

    def reopen(self, conn, key):
        return saves.open_original_windows_save(conn, self.staging, key, confirmed=True)

    def recover(self, conn):
        return saves.load_original_windows_key(conn, self.staging, CONTAINER, confirmed=True)

    def test_save_changed_body_only_cold_reopen_and_inert_stale_iterator(self):
        choice, draft, review = self.edited()
        with self.assertRaises(Rejected):
            draft.prepare_save(review)
        with self.assertRaises(Rejected):
            draft.prepare_save(replace(review), confirmed=True)
        key = draft.prepare_save(review, confirmed=True)
        with self.assertRaises(Rejected):
            draft.prepare_save(review, confirmed=True)
        source, conn, draft_path = choice.source, choice._connection, choice.local_file
        draft.close()
        self.assertEqual(self.recover(conn), key)
        # Cold recovery reads the independently pinned body, not this live or later WORK file.
        Path(draft_path).write_bytes(b"later!")
        with self.assertRaises(Rejected):
            with self.reopen(conn, replace(key, record_sha256="0" * 64)):
                self.fail("admitted a different original key")
        with self.reopen(conn, key) as pending:
            action, = pending.plan.actions
            self.assertEqual((action.operation_id, action.container_id, action.branch, action.head,
                              action.name, action.body.length, action.body.sha256),
                             (key.operation, CONTAINER, "main", source.head.id, None,
                              review.length, review.sha256))
            self.assertEqual(pending.plan.placements, ((CONTAINER, choice._leaf),))
            self.assertEqual(source.container.name, "CON")  # No remapped local name in the SaveNode.
            with pending.body(action) as parts:
                with self.assertRaises(Rejected):
                    with pending.body(action):
                        self.fail("second cursor on a borrowed NTFS handle")
                self.assertEqual(b"".join(parts), b"edited")
            with pending.body(action) as escaped:
                pass
            with self.assertRaises(Rejected):
                next(escaped)
        with self.assertRaises(Rejected):
            with pending.body(action):
                self.fail("closed pending scope")
        self.assertEqual(self.requests, [TARGET])

    def test_save_competing_prepare_never_replaces_an_existing_transfer(self):
        first, a, reviewed_a = self.edited(after=b"winner")
        second, b, reviewed_b = self.edited(after=b"loser")
        key = a.prepare_save(reviewed_a, confirmed=True)
        before = sorted(p.name for p in self.staging.iterdir())
        with self.assertRaisesRegex(Rejected, "occupies"):
            b.prepare_save(reviewed_b, confirmed=True)
        self.assertEqual(before, sorted(p.name for p in self.staging.iterdir()))
        self.assertEqual(key, self.recover(second._connection))
        with self.reopen(first._connection, key) as pending, pending.body(pending.plan.actions[0]) as blocks:
            self.assertEqual(b"".join(blocks), b"winner")
        self.assertEqual(self.requests, [TARGET, TARGET])

    def test_save_original_body_substitution_same_bytes_is_refused(self):
        choice, draft, review = self.edited()
        key = draft.prepare_save(review, confirmed=True)
        draft.close()
        original = self.staging / (key.submission + ".zvb")
        moved = original.with_suffix(".fixture-original")
        original.rename(moved)  # Keep old FileId alive, so this case is not about possible ID recycling.
        original.write_bytes(moved.read_bytes())
        Path(str(original) + ":Zone.Identifier").write_bytes(Path(str(moved) + ":Zone.Identifier").read_bytes())
        with self.assertRaises(Rejected):
            with self.reopen(choice._connection, key):
                self.fail("accepted a byte-identical replacement file")
        self.assertEqual(self.requests, [TARGET])

    def test_save_lost_intent_rename_can_recover_original_key_and_exact_body(self):
        choice, draft, review = self.edited(after=b"")
        conn, api = choice._connection, draft._home.api
        rename = api.rename_new
        def lose_reply(h, parent, name):
            result = rename(h, parent, name)
            if name.endswith(".zwi"):
                raise OSError("synthetic loss of the completed original pending rename response")
            return result
        with patch.object(api, "rename_new", lose_reply), self.assertRaises(saves.WindowsSaveUncertain) as failure:
            draft.prepare_save(review, confirmed=True)
        draft.close()
        key = self.recover(conn)
        self.assertEqual(key, failure.exception.key)
        with self.reopen(conn, key) as pending, pending.body(pending.plan.actions[0]) as parts:
            self.assertEqual(b"".join(parts), b"")
        self.assertEqual(self.requests, [TARGET])

    def test_save_key_precedes_body_and_incomplete_intent_is_not_adopted(self):
        choice, draft, review = self.edited(after=b"qualified body")
        api, conn = draft._home.api, choice._connection
        write = api.write
        def lose_body(h, data):
            if data == b"qualified body":
                raise OSError("synthetic failure before sealed payload writes")
            return write(h, data)
        with patch.object(api, "write", lose_body), self.assertRaisesRegex(Rejected, "No confirmed"):
            draft.prepare_save(review, confirmed=True)
        draft.close()
        key = self.recover(conn)
        self.assertEqual((self.staging / (key.submission + ".zvb")).read_bytes(), b"")
        self.assertFalse(any(p.suffix == ".zwi" for p in self.staging.iterdir()))
        with self.assertRaises(Rejected):
            with self.reopen(conn, key):
                self.fail("adopted a private temporary instead of the selected original")
        self.assertEqual(self.requests, [TARGET])

    def test_save_key_byte_identical_substitution_and_wrong_scope_are_refused(self):
        choice, draft, review = self.edited()
        key = draft.prepare_save(review, confirmed=True)
        conn = choice._connection
        draft.close()
        with self.assertRaises(Rejected):
            self.recover(replace(conn, _account="other-fixture"))
        with self.assertRaises(Rejected):
            saves.load_original_windows_key(conn, self.destination, CONTAINER, confirmed=True)
        key_file = self.staging / saves._key_slot(choice.source.destination, choice.source.account, key.container)
        moved = key_file.with_suffix(".fixture-original")
        key_file.rename(moved)
        key_file.write_bytes(moved.read_bytes())
        with self.assertRaises(Rejected):
            self.recover(conn)
        # An independently retained original key still pins the true unmodified plan and body.
        with self.reopen(conn, key) as pending, pending.body(pending.plan.actions[0]) as blocks:
            self.assertEqual(b"".join(blocks), b"edited")
        self.assertEqual(self.requests, [TARGET])


def qualify(spelling):
    if sys.platform != "win32":
        raise SystemExit("An ordinary-user Windows NTFS environment is required.")
    with PinnedParent(spelling, output=True) as root:
        security = NativeSecurity()
        actor = security.caller()
        profile = security.read(root.handle, actor, directory=True)
        zone = _effective_zone(ORIGIN + TARGET)
        if zone not in (3, 4):
            raise SystemExit("This synthetic origin requires the unmodified Windows marking policy.")
        cases = [name for name in unittest.defaultTestLoader.getTestCaseNames(OriginalSave)
                 if name.startswith("test_save_")]
        if len(cases) != 6:
            raise RuntimeError("all six original-save cases are required")
        suite = unittest.TestSuite(OriginalSave(name, root, spelling, security, actor, profile, zone) for name in cases)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return int(not (result.wasSuccessful() and result.testsRun == len(cases) and not result.skipped))


if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1].startswith("-"):
        raise SystemExit("Supply one existing disposable ordinary-user private NTFS directory.")
    raise SystemExit(qualify(Path(sys.argv[1])))
