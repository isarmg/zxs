import unittest
from dataclasses import replace

from zipworkspace.container_http import ConfirmedAction, mutation_message
from zipworkspace.container_plan import Action, FullBody, Plan, ReceiptFence
from zipworkspace.desktop.container_client import Original, _matches, _receipt
from zipworkspace.desktop.container_management import (
    Body, Branch, Container, CreateBranch, Detail, Kind, Node, Outcome, Phase, Receipt, Recycle, Rename, Tags,
    Ticket, branch_choice, lookup, open_checked, page, perform, save_choice, save_mode, valid_change,
)
from zipworkspace.desktop.container_outbox import _change, _change_bytes, _metadata_plan
from zipworkspace.model import Rejected

C = Container("1" * 32, "容器工艺", 7)
BASE = Node("2" * 32, None, 1, "a" * 64)
HEAD = Node("3" * 32, BASE.id, 2, "b" * 64)
MAIN = Branch("main", HEAD.id, (HEAD, BASE))
OTHER = Branch("4" * 32, HEAD.id, (HEAD, BASE))
FORK_ID = "5" * 32
OP = "6" * 32
NEW_NODE = "7" * 32
SPOOL = "8" * 32


def detail(branch, node):
    return Detail(C, ("生产", "原\r\n标记"), (MAIN, OTHER), frozenset({Kind.SAVE, Kind.RECYCLE}),
                  Body(C.id, SPOOL, branch.id, node, 4096, "d" * 64,
                       FORK_ID if branch.id == "main" and node != branch.head else branch.id, NEW_NODE))


def confirmation(ticket, **overrides):
    body = ticket.change.body
    fields = dict(revision=C.revision + 1, branch=ticket.change.branch, node=NEW_NODE,
                  parent=body.selected_base, length=body.length, sha256=body.sha256)
    fields.update(overrides)
    return Outcome(ticket, Phase.CONFIRMED, Receipt(ticket, **fields))


class NativeFixture:
    def __init__(self):
        self.info = detail(MAIN, BASE.id)
        self.calls = []
        self.reply = confirmation

    def open_container(self, container):
        return self.info

    def prepare(self, change):
        self.calls.append(("prepare", change))
        return Ticket(change, OP, SPOOL, C.revision)

    def apply(self, ticket):
        self.calls.append(("apply", ticket))
        return self.reply(ticket)

    def status(self, ticket):
        self.calls.append(("status", ticket))
        return self.reply(ticket)


class ContainerManagement(unittest.TestCase):
    def test_initial_main_uses_no_parent(self):
        branch = Branch("main", None, ())
        info = replace(detail(MAIN, BASE.id), container=replace(C, revision=0), branches=(branch,),
                       body=Body(C.id, SPOOL, "main", None, 0, "d" * 64, "main", NEW_NODE))
        ready = save_choice(info, branch, None)
        self.assertEqual((ready.branch, ready.body.selected_base, ready.from_main), ("main", None, None))

    def test_current_head_appends_to_the_same_branch(self):
        info = detail(MAIN, HEAD.id)
        self.assertEqual(save_mode(info, MAIN, HEAD.id), "append")
        self.assertEqual(save_choice(info, MAIN, HEAD.id).branch, "main")
        self.assertIsNone(save_choice(info, MAIN, BASE.id))

    def test_historical_main_is_one_atomic_save_with_explicit_new_id(self):
        fixture = NativeFixture()
        info = open_checked(fixture, C.id)
        self.assertEqual(save_mode(info, MAIN, BASE.id), "fork")
        occupied = replace(info, body=replace(info.body, target_branch=OTHER.id))
        self.assertIsNone(save_choice(occupied, MAIN, BASE.id))
        change = save_choice(info, MAIN, BASE.id)
        result = perform(fixture, change)
        self.assertEqual(result.phase, Phase.CONFIRMED)
        self.assertEqual((change.branch, change.from_main, result.receipt.node), (FORK_ID, BASE.id, NEW_NODE))
        self.assertEqual(fixture.calls, [("prepare", change), ("apply", Ticket(change, OP, SPOOL, C.revision))])
        self.assertEqual(MAIN.head, HEAD.id)

    def test_explicit_branch_is_one_no_body_original_and_inherits_exact_node(self):
        fixture = NativeFixture()
        info = replace(fixture.info, allowed=frozenset({Kind.CREATE_BRANCH}))
        original = Original(SPOOL, OP)
        change = branch_choice(info, MAIN, BASE.id, FORK_ID)
        self.assertEqual(change, CreateBranch(C, "main", BASE.id, FORK_ID))
        self.assertIsNone(branch_choice(info, MAIN, NEW_NODE, FORK_ID))
        self.assertIsNone(branch_choice(info, MAIN, BASE.id, OTHER.id))
        self.assertIsNone(branch_choice(fixture.info, MAIN, BASE.id, FORK_ID))
        self.assertEqual(_change(_change_bytes(change)), change)

        plan = _metadata_plan(change, original.operation)
        action, base = plan.next_for(C.id)
        self.assertEqual((plan.placements, action.kind, action.body, action.branch, action.head),
                         ((), "create_branch", None, FORK_ID, BASE.id))
        self.assertTrue(_matches(action, base, original, change))
        self.assertFalse(_matches(action, base, original, replace(change, source_node=HEAD.id)))

        fence = ReceiptFence(C.id, OP, "create_branch", None, 11, FORK_ID, BASE.id)
        fixture.reply = lambda ticket: Outcome(ticket, Phase.CONFIRMED,
                                               _receipt(ticket, action, ConfirmedAction(fence, None)))
        result = perform(fixture, change)
        self.assertEqual((result.phase, result.receipt.branch, result.receipt.node, result.receipt.length),
                         (Phase.CONFIRMED, FORK_ID, BASE.id, None))
        self.assertIsNone(plan.next_for(C.id, (fence,)))
        self.assertEqual([kind for kind, _ in fixture.calls], ["prepare", "apply"])
        with self.assertRaises(Rejected):
            _receipt(result.ticket, action, ConfirmedAction(replace(fence, head=HEAD.id), None))
        fixture.reply = lambda ticket: Outcome(ticket, Phase.CONFIRMED,
                                               Receipt(ticket, 11, branch=FORK_ID, node=HEAD.id))
        self.assertEqual(lookup(fixture, result.ticket).phase, Phase.UNKNOWN)
        self.assertEqual(MAIN.head, HEAD.id)

    def test_new_and_existing_remote_only_names_remain_exact_without_a_local_path(self):
        target = replace(C, name="CON.txt")
        new_name = "Cafe\u0301.zxs"
        for change in (Rename(target, new_name), Tags(target, ("保留",)), Rename(C, new_name)):
            with self.subTest(change=change):
                self.assertTrue(valid_change(change))
                self.assertEqual(_change(_change_bytes(change)), change)
                plan = _metadata_plan(change, OP)
                expected = ((C.id, C.name),) if change.target == C else ()
                self.assertEqual(plan.placements, expected)
                self.assertEqual(Plan.decode(plan.encode()), plan)
                action, base = plan.next_for(C.id)
                self.assertTrue(_matches(action, base, Original(SPOOL, OP), change))
                if type(change) is Rename:
                    self.assertEqual(mutation_message(action, base).body, b'{"name":"' + new_name.encode() + b'"}')
                self.assertIsNone(action.body)

        first = Action(OP, C.id, "save_node", 0, None, body=FullBody(0, "d" * 64),
                       name=new_name, branch="main", node_id=NEW_NODE)
        message = mutation_message(first, 0, payload=())
        self.assertEqual(dict(message.headers)["Zxs-Initial-Name"], new_name.encode().hex())

    def test_invalid_logical_rename_never_prepares_a_ticket(self):
        fixture = NativeFixture()
        for change in (Rename(C, "remote/path"), Rename(C, "a\x85b"), Rename(replace(C, name="a "), "ok")):
            with self.subTest(change=change), self.assertRaises(ValueError):
                perform(fixture, change)
        self.assertEqual(fixture.calls, [])

    def test_preexisting_metadata_plan_bytes_are_unchanged_for_unarmed_recovery(self):
        original = ('{"actions":[{"after":null,"base_revision":7,"body":null,"branch":null,'
                    f'"container_id":"{C.id}","fork_from_main":null,"head":null,'
                    f'"kind":"rename","name":"after.zxs","node_id":null,"operation_id":"{OP}",'
                    '"tags":null}],"placements":[['
                    f'"{C.id}","容器工艺"'
                    ']],"version":1}').encode("utf-8")
        self.assertEqual(_metadata_plan(Rename(C, "after.zxs"), OP).encode(), original)
        self.assertEqual(Plan.decode(original).encode(), original)

    def test_unsupported_nonmain_history_keeps_readable_metadata(self):
        fixture = NativeFixture()
        fixture.info = detail(OTHER, BASE.id)
        result = open_checked(fixture, C.id)
        self.assertEqual(result.branches, (MAIN, OTHER))
        self.assertEqual(result.tags, ("生产", "原\r\n标记"))
        self.assertIsNone(result.body)
        self.assertIsNone(save_choice(fixture.info, OTHER, BASE.id))

    def test_wrong_branch_parent_or_body_receipt_remains_unknown(self):
        fixture = NativeFixture()
        change = save_choice(fixture.info, MAIN, BASE.id)
        for field, different in (("branch", "main"), ("parent", HEAD.id), ("length", 4095),
                                 ("sha256", "e" * 64)):
            with self.subTest(field=field):
                fixture.reply = lambda ticket: confirmation(ticket, **{field: different})
                self.assertEqual(perform(fixture, change).phase, Phase.UNKNOWN)

    def test_lost_reply_queries_only_the_original(self):
        fixture = NativeFixture()
        change = save_choice(fixture.info, MAIN, BASE.id)
        def lost(ticket):
            raise ConnectionError("closed during the original operation")
        fixture.reply = lost
        pending = perform(fixture, change)
        self.assertEqual(pending.phase, Phase.UNKNOWN)
        fixture.reply = confirmation
        confirmed = lookup(fixture, pending.ticket)
        self.assertEqual(confirmed.phase, Phase.CONFIRMED)
        self.assertEqual(fixture.calls[-1], ("status", Ticket(change, OP, SPOOL, C.revision)))

    def test_save_sequence_can_skip_unrelated_changes(self):
        fixture = NativeFixture()
        fixture.reply = lambda ticket: confirmation(ticket, revision=C.revision + 4)
        change = save_choice(fixture.info, MAIN, BASE.id)
        self.assertEqual(perform(fixture, change).receipt.revision, C.revision + 4)

    def test_only_terminal_recycle_without_live_sequence_confirms(self):
        fixture = NativeFixture()
        change = Recycle(C)
        fixture.reply = lambda ticket: Outcome(ticket, Phase.CONFIRMED, Receipt(ticket, 8, recycled=True))
        self.assertEqual(perform(fixture, change).phase, Phase.UNKNOWN)
        fixture.reply = lambda ticket: Outcome(ticket, Phase.CONFIRMED,
                                                Receipt(ticket, None, recycled=True,
                                                        recycle_source_cleanup_pending=True))
        receipt = perform(fixture, change).receipt
        self.assertIsNone(receipt.revision)
        self.assertIs(receipt.recycle_source_cleanup_pending, True)

    def test_embedded_html_is_only_escaped_local_display(self):
        markup = page("容器 <工艺>", "状态 <待决>", (("标签", "<保留> & 原标记"),))
        self.assertIn("容器 &lt;工艺&gt;", markup)
        self.assertIn("&lt;保留&gt; &amp; 原标记", markup)
        self.assertNotIn("href=", markup)
        self.assertNotIn("src=", markup)
