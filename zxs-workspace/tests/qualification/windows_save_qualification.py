"""Manual NTFS original-save custody qualification. Never connect, send or mount."""
from dataclasses import replace
from pathlib import Path
import json
import sys
import unittest
from unittest.mock import patch

if __name__ == "__main__":
    here = Path(__file__).resolve()
    sys.path[:0] = [str(here.parents[2]), str(here.parent)]

from windows_draft_qualification import LocalWorkingFile
from windows_node_qualification import CONTAINER, ORIGIN, TARGET
from zipworkspace._windows_access import NativeSecurity
from zipworkspace._windows_output import PinnedParent, OutputNative
from zipworkspace.container_http import lookup_message, project_reply
from zipworkspace.desktop.container_connection import _Authorization
from zipworkspace.desktop.container_management import Phase
from zipworkspace.desktop import container_windows_receipt as receipts
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


    def receipt_case(self):
        choice, draft, review = self.edited()
        key = draft.prepare_save(review, confirmed=True)
        conn = choice._connection
        draft.close()
        with self.reopen(conn, key) as pending:
            action = pending.plan.actions[0]
        seen = []
        data = {"container_id": CONTAINER, "operation_id": key.operation, "kind": "save_node",
                "sequence": 3, "node_id": action.node_id, "length": review.length, "sha256": review.sha256,
                "target": {"kind": "branch", "branch": action.branch, "expected_head": action.head}}
        answer = [200, data]
        def get(message):
            self.assertEqual(message, lookup_message(action))
            seen.append(message)
            return project_reply(answer[0], json.dumps(answer[1]).encode("utf-8"), lookup=True)
        conn._exchange.request = get
        return conn, key, seen, answer

    def test_save_receipt_metadata_only_get_and_local_confirmation_survives_repeat(self):
        conn, key, seen, answer = self.receipt_case()
        original_body = self.staging / (key.submission + ".zvb")
        original_body.rename(original_body.with_suffix(".unavailable"))
        lookup = lambda: receipts.lookup_original_windows_receipt(conn, self.staging, key, confirmed=True)
        with self.assertRaises(Rejected):
            receipts.lookup_original_windows_receipt(conn, self.staging, key)
        self.assertEqual(seen, [])
        result = lookup()
        self.assertEqual(result.phase, Phase.CONFIRMED)
        self.assertEqual(result.receipt.revision, 3)
        proof = self.staging / (key.submission + ".zrr")
        original = proof.read_bytes()
        self.assertEqual(lookup(), result)
        answer[:] = [404, {"state": "not_found"}]
        self.assertEqual(lookup().phase, Phase.UNKNOWN)
        self.assertEqual(proof.read_bytes(), original)
        self.assertEqual(len(seen), 3)

    def test_save_receipt_absent_or_wrong_effect_never_arms_send(self):
        conn, key, seen, answer = self.receipt_case()
        lookup = lambda: receipts.lookup_original_windows_receipt(conn, self.staging, key, confirmed=True)
        correct = answer[1]
        for wire, phase in (([404, {"state": "not_found"}], Phase.ABSENT), ([409, {}], Phase.CONFLICT),
                            ([200, {**correct, "sha256": "1" * 64}], Phase.UNKNOWN),
                            ([200, {**correct, "operation_id": "4" * 32}], Phase.UNKNOWN)):
            answer[:] = wire
            self.assertEqual(lookup().phase, phase)
        self.assertEqual(len(seen), 4)
        self.assertFalse((self.staging / (key.submission + ".zrr")).exists())

    def test_save_receipt_lost_local_rename_retains_same_verified_server_result(self):
        conn, key, seen, _ = self.receipt_case()
        rename = OutputNative.rename_new
        def lose(api, h, parent, name):
            result = rename(api, h, parent, name)
            if name.endswith(".zrr"):
                raise OSError("synthetic lost local receipt rename response")
            return result
        with patch.object(OutputNative, "rename_new", lose), self.assertRaises(receipts.WindowsReceiptUncertain) as lost:
            receipts.lookup_original_windows_receipt(conn, self.staging, key, confirmed=True)
        self.assertEqual(lost.exception.key, key)
        self.assertEqual(lost.exception.receipt.revision, 3)
        result = receipts.lookup_original_windows_receipt(conn, self.staging, key, confirmed=True)
        self.assertEqual(result.receipt, lost.exception.receipt)
        self.assertEqual(len(seen), 2)

    def test_save_receipt_requires_supplier_login_not_just_connection_label(self):
        conn, key, seen, _ = self.receipt_case()
        wrong = _Authorization("other", "synthetic")
        self.addCleanup(wrong.revoke)
        forged_label = replace(conn, _authorization=wrong)
        with patch.object(conn._exchange, "_authorization", wrong), self.assertRaises(Rejected):
            receipts.lookup_original_windows_receipt(forged_label, self.staging, key, confirmed=True)
        self.assertEqual(seen, [])


def qualify(spelling):
    if sys.platform != "win32":
        raise SystemExit("An ordinary-user Windows NTFS environment is required.")
    if not callable(getattr(_Authorization, "require_account", None)):
        raise SystemExit("NOT RUN: receipt qualification requires native credential login attestation.")
    with PinnedParent(spelling, output=True) as root:
        security = NativeSecurity()
        actor = security.caller()
        profile = security.read(root.handle, actor, directory=True)
        zone = _effective_zone(ORIGIN + TARGET)
        if zone not in (3, 4):
            raise SystemExit("This synthetic origin requires the unmodified Windows marking policy.")
        cases = [name for name in unittest.defaultTestLoader.getTestCaseNames(OriginalSave)
                 if name.startswith("test_save_")]
        if len(cases) != 10:
            raise RuntimeError("all ten original-save and receipt cases are required")
        suite = unittest.TestSuite(OriginalSave(name, root, spelling, security, actor, profile, zone) for name in cases)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return int(not (result.wasSuccessful() and result.testsRun == len(cases) and not result.skipped))


if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1].startswith("-"):
        raise SystemExit("Supply one existing disposable ordinary-user private NTFS directory.")
    raise SystemExit(qualify(Path(sys.argv[1])))
