from itertools import count
import unittest

from zipworkspace.container_plan import (Baseline, BranchIntent, DeleteIntent, FullBody, LocalFile, Plan, ReceiptFence,
                                         from_atomic_editor_replace, plan_submit)
from zipworkspace.model import Rejected


A, B = "a" * 32, "b" * 32
OLD, HEAD, BRANCH, NEW_NODE = "1" * 32, "2" * 32, "3" * 32, "4" * 32
ORIGINAL, CHANGED = FullBody(7, "c" * 64), FullBody(21, "d" * 64)


class ContainerPlanTest(unittest.TestCase):
    def setUp(self):
        self.base = Baseline(A, "report.txt", 8, ORIGINAL, ("approved",), selected_node=HEAD, head=HEAD)
        self.ids = count(1)

    def plan(self, observed, deletions=(), complete=True, branch_intents=()):
        return plan_submit([self.base], observed, deletions, attested_complete=complete,
                           operation_ids=lambda: f"{next(self.ids):032x}", branch_ids=lambda: BRANCH, node_ids=lambda: NEW_NODE,
                           branch_intents=branch_intents)

    def test_editor_temporary_overwrite_retains_container_and_sends_one_complete_file(self):
        original = LocalFile(A, "draft/report.txt", ORIGINAL, ("approved",))
        temporary = LocalFile(None, "draft/.from-editor", CHANGED)
        replaced = from_atomic_editor_replace(temporary, original, journal_target_id=A,
                                              journal_target_path="draft/report.txt")
        plan = self.plan([replaced])
        action, = plan.actions
        self.assertEqual(("save_node", A, CHANGED, 8), (action.kind, action.container_id, action.body, action.base_revision))
        self.assertEqual(plan, Plan.decode(plan.encode()))
        self.assertNotIn(b".from-editor", plan.encode())
        self.assertEqual(("main", HEAD, NEW_NODE), (action.branch, action.head, action.node_id))
        with self.assertRaises(Rejected):
            from_atomic_editor_replace(LocalFile(B, temporary.path, CHANGED), original,
                                       journal_target_id=A, journal_target_path=original.path)

    def test_final_rename_and_tags_do_not_upload_unchanged_bytes_or_temporary_names(self):
        # Native journal resolved old -> temporary -> final as the same original container.
        plan = self.plan([LocalFile(A, "other/final.txt", ORIGINAL, ("review",))])
        rename, labels = plan.actions
        self.assertEqual(("rename", "final.txt", None), (rename.kind, rename.name, rename.body))
        self.assertEqual(("set_tags", ("review",), rename.operation_id), (labels.kind, labels.tags, labels.after))
        self.assertIsNone(labels.base_revision)
        self.assertEqual(((A, "other/final.txt"),), plan.placements)

    def test_pure_local_directory_move_does_not_touch_server(self):
        plan = self.plan([LocalFile(A, "new-folder/report.txt", ORIGINAL, ("approved",))])
        self.assertEqual((), plan.actions)
        self.assertEqual(((A, "new-folder/report.txt"),), plan.placements)

    def test_missing_or_stale_delete_never_turns_into_remote_deletion(self):
        for deletions, complete in (((), True), ((DeleteIntent(A, 7),), True), ((DeleteIntent(A, 8),), False)):
            with self.assertRaises(Rejected):
                self.plan([], deletions, complete)
        delete, = self.plan([], (DeleteIntent(A, 8),)).actions
        self.assertEqual(("delete", A, 8), (delete.kind, delete.container_id, delete.base_revision))
        self.assertIsNone(delete.body)

    def test_first_full_save_creates_named_container_and_node_on_main_in_one_action(self):
        plan = self.plan([LocalFile(A, "report.txt", ORIGINAL, ("approved",)), LocalFile(B, "manual.txt", CHANGED)])
        save, = plan.actions
        self.assertEqual(("save_node", B, 0, "main", None, "manual.txt", CHANGED),
                         (save.kind, save.container_id, save.base_revision, save.branch, save.head, save.name, save.body))

    def test_historical_main_edit_has_one_atomic_fork_and_whole_file_save(self):
        self.base = Baseline(A, "report.txt", 8, ORIGINAL, selected_node=OLD, head=HEAD)
        plan = self.plan([LocalFile(A, "report.txt", CHANGED)])
        save, = plan.actions
        self.assertEqual(("save_node", BRANCH, None, OLD, NEW_NODE, 8, CHANGED),
                         (save.kind, save.branch, save.head, save.fork_from_main, save.node_id,
                          save.base_revision, save.body))
        self.assertEqual((save, 8), plan.next_for(A))
        self.assertEqual((save, 8), plan.next_for(A))
        self.assertIsNone(plan.next_for(A, (ReceiptFence(A, save.operation_id, save.kind, None, 12,
                                                          BRANCH, NEW_NODE),)))
        with self.assertRaises(Rejected):
            plan.next_for(A, (ReceiptFence(A, save.operation_id, save.kind, None, 12, BRANCH, HEAD),))

    def test_branch_only_is_explicit_and_does_not_send_an_unchanged_body(self):
        plan = self.plan([LocalFile(A, "report.txt", ORIGINAL, ("approved",))],
                         branch_intents=[BranchIntent(A, BRANCH, HEAD)])
        create, = plan.actions
        self.assertEqual(("create_branch", BRANCH, HEAD, None), (create.kind, create.branch, create.head, create.body))
        self.assertEqual(plan, Plan.decode(plan.encode()))

    def test_distinct_container_queues_can_continue_without_finishing_the_first(self):
        plan = self.plan([LocalFile(A, "report.txt", CHANGED, ("approved",)), LocalFile(B, "manual.txt", CHANGED)])
        self.assertEqual([A, B], [container_id for container_id, _ in plan.queues()])
        original, original_base = plan.next_for(A)
        save, base = plan.next_for(B)
        self.assertEqual((8, 0), (original_base, base))
        self.assertIsNone(plan.next_for(B, (ReceiptFence(B, save.operation_id, save.kind, None, 1, "main", NEW_NODE),)))
        self.assertEqual((original, original_base), plan.next_for(A))


    def test_save_receipt_can_advance_over_unrelated_sequence_and_tags_use_its_exact_sequence(self):
        plan = self.plan([LocalFile(A, "report.txt", CHANGED, ("review",))])
        save, tags = plan.actions
        settled = ReceiptFence(A, save.operation_id, save.kind, None, 12, "main", NEW_NODE)
        self.assertEqual((tags, 12), plan.next_for(A, (settled,)))
        self.assertIsNone(plan.next_for(A, (settled, ReceiptFence(A, tags.operation_id, tags.kind, 12, 14))))
        with self.assertRaises(Rejected):
            plan.next_for(A, (settled, ReceiptFence(A, tags.operation_id, tags.kind, 8, 14)))
