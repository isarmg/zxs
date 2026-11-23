from contextlib import contextmanager
import json
from types import SimpleNamespace
import unittest

from zipworkspace.container_http import (Effect, Reply, ReplyKind, SelectedReceipt, SubmitUnconfirmed,
                                         invoke_original, mutation_message, project_reply, verified_result)
from zipworkspace.container_plan import Action, FullBody, Plan
from zipworkspace.model import canonical

CONTAINER, OP, NODE, BRANCH, ORIGINAL = tuple(character * 32 for character in "abcd1")
HASH = "2" * 64


class ContainerHttpTest(unittest.TestCase):
    def test_historical_main_request_is_one_full_raw_post_with_exact_source(self):
        action = Action(OP, CONTAINER, "save_node", 8, None, body=FullBody(4, HASH),
                        branch=BRANCH, node_id=NODE, fork_from_main=ORIGINAL)
        message = mutation_message(action, 8, payload=(b"full",))
        self.assertEqual("POST", message.method)
        self.assertEqual(f"/zxs/v1/containers/{CONTAINER}/branches/{BRANCH}/nodes", message.target)
        self.assertEqual(b"full", b"".join(message.body))
        self.assertEqual({"Zxs-Operation": OP, "Content-Type": "application/octet-stream", "Content-Length": "4",
                          "Zxs-Node": NODE, "Zxs-Sha256": HASH, "Zxs-Fork-From-Main": ORIGINAL},
                         dict(message.headers))
        reply = project_reply(200, canonical({"container_id": CONTAINER, "operation_id": OP, "kind": "save_node",
                                             "sequence": 12, "node_id": NODE, "length": 4, "sha256": HASH,
                                             "target": {"kind": "historical_main", "branch": BRANCH,
                                                        "from_node": ORIGINAL}}))
        verified = verified_result(action, 8, reply.receipt)
        self.assertEqual((12, NODE), (verified.fence.sequence, verified.fence.head))
        self.assertIsNone(Plan((action,), ((CONTAINER, "name.txt"),)).next_for(CONTAINER, (verified.fence,)))

    def test_first_main_also_creates_and_file_can_be_empty(self):
        action = Action(OP, CONTAINER, "save_node", 0, None, body=FullBody(0, HASH),
                        branch="main", node_id=NODE, name="试验.txt")
        message = mutation_message(action, 0, payload=())
        self.assertEqual("0", dict(message.headers)["Zxs-Base-Node"])
        self.assertEqual("0", dict(message.headers)["Content-Length"])
        self.assertEqual("试验.txt".encode().hex(), dict(message.headers)["Zxs-Initial-Name"])

    def test_standalone_branch_has_exact_source_and_no_metadata_revision_header(self):
        action = Action(OP, CONTAINER, "create_branch", 8, None, branch=BRANCH, head=ORIGINAL)
        message = mutation_message(action, 8)
        self.assertEqual("PUT", message.method)
        self.assertEqual({"from_node": ORIGINAL}, json.loads(message.body))
        self.assertNotIn("Zxs-Base-Generation", dict(message.headers))
        selected = SelectedReceipt(CONTAINER, OP, 13, Effect("create_branch", branch=BRANCH, source_node=ORIGINAL))
        verified = verified_result(action, 8, selected)
        self.assertIsNone(Plan((action,), ((CONTAINER, "name.txt"),)).next_for(CONTAINER, (verified.fence,)))

    def test_selected_recycle_is_terminal_even_if_cleanup_still_pending(self):
        action = Action(OP, CONTAINER, "delete", 8, None)
        message = mutation_message(action, 8)
        self.assertEqual("DELETE", message.method)
        self.assertEqual("8", dict(message.headers)["Zxs-Base-Generation"])
        self.assertIsNone(message.body)
        reply = project_reply(200, canonical({"kind": "recycle", "container_id": CONTAINER,
                                             "operation_id": OP, "original_sequence": 8,
                                             "source_cleanup_pending": True}))
        verified = verified_result(action, 8, reply.receipt)
        self.assertTrue(verified.recycle_source_cleanup_pending)
        self.assertIsNone(Plan((action,), ()).next_for(CONTAINER, (verified.fence,)))

    def test_receipt_only_and_generic_404_never_open_the_sealed_file(self):
        action = Action(OP, CONTAINER, "save_node", 8, None, body=FullBody(4, HASH),
                        branch="main", node_id=NODE, head=ORIGINAL)
        plan = Plan((action,), ((CONTAINER, "name.txt"),))
        fixture = self

        @contextmanager
        def body(_):
            fixture.fail("the read-only/unknown outcome borrowed a body")
            yield None

        subject = SimpleNamespace(plan=plan, destination="correct-pin", body=body)
        exact = SelectedReceipt(CONTAINER, OP, 9, Effect("save_node", branch="main", target="branch",
                                                         source_node=ORIGINAL, new_node=NODE, body=action.body))
        server = SimpleNamespace(destination_identity="correct-pin", request=lambda _: Reply(ReplyKind.SELECTED, exact))
        self.assertEqual(NODE, invoke_original(subject, CONTAINER, (), server).fence.head)
        server.request = lambda _: project_reply(404, b"not an R3 absence", lookup=True)
        with self.assertRaises(SubmitUnconfirmed):
            invoke_original(subject, CONTAINER, (), server, send_if_absent=True)
        self.assertIs(ReplyKind.ABSENT, project_reply(404, b'{"state":"not_found"}', lookup=True).kind)
        self.assertIs(ReplyKind.UNKNOWN,
                      project_reply(404, b'{"state":"not_found","state":"not_found"}', lookup=True).kind)
