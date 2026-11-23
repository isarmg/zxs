"""Observe a real, already-completed GUI save in an isolated R3 fixture. NEVER create a node, retry or retire."""
import argparse
from contextlib import ExitStack
from getpass import GetPassWarning, getpass
from hashlib import sha256
import json
from pathlib import Path
import re
import sys
import warnings

if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from zipworkspace.container_http import ReplyKind, lookup_message, verified_result
from zipworkspace.desktop.container_connection import open_connection
from zipworkspace.desktop.container_client import _receipt
from zipworkspace.desktop.container_management import open_checked
from zipworkspace.desktop.container_windows_receipt import _original_proof, _ticket
from zipworkspace.desktop.container_windows_retire import _chosen_key
from zipworkspace.desktop.container_windows_save import WindowsSaveKey, _original_record
from zipworkspace.model import Rejected


def require(value, message):
    if not value:
        raise Rejected(message)


def inspect(conn, state, key, *, origin, login, name, branch, original_head, original_revision,
            original_sha, length, expected_sha):
    with _original_record(conn, state, key, confirmed=True) as chosen, ExitStack() as keep:
        home, source, plan, _, _, check, _, retired = chosen
        ticket = _ticket(key, source, plan)
        keyrow, check_key = _chosen_key(keep, home, source, key)
        require(retired and keyrow[1] == key.submission + ".zwk", "the GUI did not complete both exact local archive transitions")
        prior = _original_proof(home, key, ticket)
        require(prior is not None, "no already-retained durable typed receipt from the selected GUI save")
        require((source.origin, source.account, source.container.id, source.container.name, source.branch, source.head.id,
                 source.container.revision, source.head.sha256) ==
                (origin, login, key.container, name, branch, original_head, original_revision, original_sha),
                "the original does not match the independently prepared test case")
        a, = plan.actions
        require(a.name is None and a.body is not None and (a.body.length, a.body.sha256) == (length, expected_sha),
                "the GUI did not retain a whole unchanged-name save of the expected test bytes")
        check()
        check_key()
        # Only three read surfaces: exact original receipt GET, typed metadata GETs, exact new-node data GET.
        reply = conn._exchange.request(lookup_message(a))
        require(reply.kind is ReplyKind.SELECTED, "test server did not return its exact original operation")
        verified = _receipt(ticket, a, verified_result(a, ticket.base_sequence, reply.receipt))
        require(prior == verified, "the final GUI proof differs from the independent server reply")
        detail = open_checked(conn.operations, key.container)
        current = next((b for b in detail.branches if b.id == source.branch), None)
        node = next((n for n in current.nodes if n.id == prior.node), None) if current else None
        require(detail.container.name == name and detail.container.revision == prior.revision and current
                and current.head == prior.node and node is not None and
                (node.parent, node.length, node.sha256) == (original_head, length, expected_sha),
                "the isolated fixture is not at the confirmed new head, parent, name and hash")
        digest, received = sha256(), 0
        def observe(block):
            nonlocal received
            received += len(block)
            require(received <= length, "excess response body")
            digest.update(block)
            return len(block)
        conn._exchange.read_node_into(key.container, node.id, length=length, sha256=expected_sha, write=observe)
        require((received, digest.hexdigest()) == (length, expected_sha), "downloaded node differs from the intended full payload")
        check()
        check_key()
        require(_original_proof(home, key, ticket) == prior, "local original GUI proof changed during observation")
    return {"format": "workspace-windows-gui-save-observation/1", "container": key.container, "branch": branch,
            "operation": key.operation, "submission": key.submission, "parent": original_head, "node": prior.node,
            "revision": prior.revision, "body_length": received, "body_sha256": digest.hexdigest()}


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    for arg in ("origin", "login", "state", "container", "submission", "operation", "record-sha256", "name",
                "branch", "original-head", "original-sha256", "expected-sha256"):
        p.add_argument("--"+arg, required=True)
    p.add_argument("--original-revision", type=int, required=True)
    p.add_argument("--expected-length", type=int, required=True)
    p.add_argument("--consent-read-isolated-server-and-original-state", action="store_true")
    o = p.parse_args()
    require(sys.platform == "win32" and o.consent_read_isolated_server_and_original_state
            and 0 <= o.expected_length <= 16 << 20 and o.original_revision > 0
            and all(re.fullmatch("[0-9a-f]{64}", x) for x in (o.original_sha256, o.expected_sha256)),
            "select a bounded independently prepared Windows fixture and explicitly allow its read-only observation")
    selected = WindowsSaveKey(o.container, o.submission, o.operation, o.record_sha256)
    # No redirected input, command-line/environment secret, or echoed getpass fallback.
    require(sys.stdin is sys.__stdin__ and sys.stdin is not None and sys.stdin.isatty(),
            "use an interactive Windows console with hidden password entry")
    with warnings.catch_warnings():
        warnings.simplefilter("error", GetPassWarning)
        secret = getpass("Isolated R3 fixture password (not saved): ")
    try:
        connection = open_connection(o.origin, o.login, secret)
    finally:
        del secret
    try:
        connected, _ = connection.establish(None)
        require(connected._exchange._location == o.origin and not connected.writable, "not the exact original HTTPS reader")
        result = inspect(connected, o.state, selected, origin=o.origin, login=o.login, name=o.name, branch=o.branch,
                         original_head=o.original_head, original_revision=o.original_revision, original_sha=o.original_sha256,
                         length=o.expected_length, expected_sha=o.expected_sha256)
        print(json.dumps(result, ensure_ascii=True, sort_keys=True))
    finally:
        connection.revoke()
