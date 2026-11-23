"""R3 abstract HTTP request shape and exact typed receipts; no HTTP/network client."""
from collections.abc import Iterable
from dataclasses import dataclass
from enum import Enum
import json
from typing import Protocol

from .container_plan import Action, FullBody, ReceiptFence
from .model import Rejected, canonical

_ROOT = "/zxs/v1/containers"


@dataclass(frozen=True)
class Message:
    method: str
    target: str
    headers: tuple[tuple[str, str], ...] = ()
    body: bytes | Iterable[bytes] | None = None


@dataclass(frozen=True)
class Effect:
    """The exact typed R3 Change/Recycle evidence; wire response codec is separate."""
    kind: str
    expected_sequence: int | None = None
    name: str | None = None
    tags: tuple[str, ...] | None = None
    branch: str | None = None
    target: str | None = None
    source_node: str | None = None
    new_node: str | None = None
    body: FullBody | None = None


@dataclass(frozen=True)
class SelectedReceipt:
    container_id: str
    operation_id: str
    sequence: int | None
    effect: Effect
    cleanup_pending: bool | None = None


class ReplyKind(Enum):
    SELECTED = "selected"
    ABSENT = "absent"
    UNKNOWN = "unknown"
    CONFLICT = "conflict"


@dataclass(frozen=True)
class Reply:
    kind: ReplyKind
    receipt: SelectedReceipt | None = None


_MAX_REPLY = 1 << 20


def _object(pairs):
    result = dict(pairs)
    if len(result) != len(pairs):
        raise ValueError("duplicate JSON field")
    return result


def _fields(obj, keys):
    if type(obj) is not dict or obj.keys() != set(keys.split()):
        raise ValueError("not the exact selected R3 response shape")
    return obj


def _uint(value):
    if type(value) is not int or not 0 <= value < 1 << 64:
        raise ValueError("invalid JSON unsigned integer")
    return value


def project_reply(status, raw, *, lookup=False):
    """Project bounded, fully received R3 JSON; an unknown response never authorizes sending."""
    if status == 409:
        return Reply(ReplyKind.CONFLICT)
    if type(raw) is not bytes or len(raw) > _MAX_REPLY:
        return Reply(ReplyKind.UNKNOWN)
    if status not in (200, 404):
        return Reply(ReplyKind.UNKNOWN)
    try:
        record = json.loads(raw.decode("utf-8"), object_pairs_hook=_object)
        if status == 404:
            return Reply(ReplyKind.ABSENT if lookup and record == {"state": "not_found"} else ReplyKind.UNKNOWN)
        kind = record["kind"]
        common = "container_id operation_id kind "
        sequence = cleanup = None
        if kind == "recycle":
            _fields(record, common + "original_sequence source_cleanup_pending")
            cleanup = record["source_cleanup_pending"]
            if cleanup is not None and type(cleanup) is not bool:
                raise ValueError("invalid recycle cleanup evidence")
            effect = Effect("delete", expected_sequence=_uint(record["original_sequence"]))
        else:
            common += "sequence "
            sequence = _uint(record["sequence"])
            if kind == "rename":
                _fields(record, common + "expected_sequence name")
                effect = Effect("rename", expected_sequence=_uint(record["expected_sequence"]), name=record["name"])
            elif kind == "replace_tags":
                _fields(record, common + "expected_sequence tags")
                if type(record["tags"]) is not list or any(type(tag) is not str for tag in record["tags"]):
                    raise ValueError("invalid complete tag list")
                effect = Effect("set_tags", expected_sequence=_uint(record["expected_sequence"]),
                                tags=tuple(record["tags"]))
            elif kind == "create_branch":
                _fields(record, common + "branch from_node")
                effect = Effect("create_branch", branch=record["branch"], source_node=record["from_node"])
            elif kind == "save_node":
                _fields(record, common + "node_id length sha256 target")
                target = record["target"]
                selector = target["kind"]
                initial_name = None
                if selector == "first_main":
                    _fields(target, "kind name")
                    initial_name, branch, source = target["name"], "main", None
                elif selector == "branch":
                    _fields(target, "kind branch expected_head")
                    branch, source = target["branch"], target["expected_head"]
                elif selector == "historical_main":
                    _fields(target, "kind branch from_node")
                    branch, source = target["branch"], target["from_node"]
                else:
                    raise ValueError("unknown submitted target")
                effect = Effect("save_node", name=initial_name, branch=branch, target=selector, source_node=source,
                                new_node=record["node_id"], body=FullBody(_uint(record["length"]), record["sha256"]))
            else:
                raise ValueError("unknown selected kind")
        return Reply(ReplyKind.SELECTED, SelectedReceipt(record["container_id"], record["operation_id"], sequence,
                                                        effect, cleanup))
    except (ValueError, TypeError, KeyError):
        return Reply(ReplyKind.UNKNOWN)


class AuthenticatedExchange(Protocol):
    """A future mature HTTPS adapter binds authenticated destination and projects R3 replies.

    Do not follow a redirect, merge ambiguous response fields, or auto-retry.
    For a completed send, exhaust the body iterator and its final checks.
    An early response may instead carry an independently verified prior receipt.
    """

    destination_identity: str

    def request(self, message: Message) -> Reply: ...


class SubmitUnconfirmed(Rejected):
    """Keep original pending identity and bytes; don't automatically reissue."""


class SubmitAbsent(SubmitUnconfirmed):
    """An exact authority lookup found no durable receipt; no change was sent."""


class SubmitConflict(SubmitUnconfirmed):
    """Original action did not confirm; don't advance that container's queue."""


def _base(value):
    if type(value) is not int or not 0 <= value < (1 << 64) - 1:
        raise Rejected("invalid bound request base sequence")
    return value


def lookup_message(action):
    if type(action) is not Action:
        raise Rejected("invalid original operation")
    return Message("GET", f"{_ROOT}/{action.container_id}/operations/{action.operation_id}")


def _json(method, target, headers, content):
    body = canonical(content)
    return Message(method, target, headers + (("Content-Type", "application/json"),
                                                ("Content-Length", str(len(body)))), body)


def mutation_message(action, base, *, payload=None):
    """Only use the receipt-gated action/base; raw bytes come from pending.body(action)."""
    if type(action) is not Action:
        raise Rejected("invalid original operation")
    _base(base)
    if action.base_revision is not None and action.base_revision != base:
        raise Rejected("request would change its original base")
    container = f"{_ROOT}/{action.container_id}"
    headers = (("Zxs-Operation", action.operation_id),)
    if action.kind == "save_node":
        if payload is None:
            raise Rejected("no sealed complete body")
        headers += (("Content-Type", "application/octet-stream"),
                    ("Content-Length", str(action.body.length)), ("Zxs-Node", action.node_id),
                    ("Zxs-Sha256", action.body.sha256))
        if action.name is not None:
            headers += (("Zxs-Base-Node", "0"), ("Zxs-Initial-Name", action.name.encode("utf-8").hex()))
        elif action.fork_from_main is not None:
            headers += (("Zxs-Fork-From-Main", action.fork_from_main),)
        else:
            headers += (("Zxs-Base-Node", action.head),)
        return Message("POST", f"{container}/branches/{action.branch}/nodes", headers, payload)
    if payload is not None:
        raise Rejected("metadata mutation cannot carry a file")
    if action.kind == "create_branch":
        return _json("PUT", f"{container}/branches/{action.branch}", headers, {"from_node": action.head})
    if base == 0:
        raise Rejected("no original revision for this metadata mutation")
    headers += (("Zxs-Base-Generation", str(base)),)
    if action.kind == "delete":
        return Message("DELETE", container, headers + (("Content-Length", "0"),))
    if action.kind == "rename":
        return _json("PUT", f"{container}/name", headers, {"name": action.name})
    if action.kind == "set_tags":
        return _json("PUT", f"{container}/tags", headers, {"tags": action.tags})
    raise Rejected("not an R3 mutation")


def _expected(action, base):
    if action.kind == "save_node":
        if action.name is not None:
            target, source = "first_main", None
        elif action.fork_from_main is not None:
            target, source = "historical_main", action.fork_from_main
        else:
            target, source = "branch", action.head
        return Effect(action.kind, name=action.name, branch=action.branch, target=target, source_node=source,
                      new_node=action.node_id, body=action.body)
    if action.kind == "create_branch":
        return Effect(action.kind, branch=action.branch, source_node=action.head)
    if action.kind == "delete":
        # This exact effect is projected only from a durable selected recycle.
        return Effect(action.kind, expected_sequence=base)
    return Effect(action.kind, expected_sequence=base, name=action.name, tags=action.tags)


@dataclass(frozen=True)
class ConfirmedAction:
    fence: ReceiptFence
    recycle_source_cleanup_pending: bool | None


def verified_result(action, base, selected):
    _base(base)
    exact = _expected(action, base)
    if (type(selected) is not SelectedReceipt or type(selected.effect) is not Effect
            or (selected.container_id, selected.operation_id, selected.effect) !=
                (action.container_id, action.operation_id, exact)):
        raise SubmitUnconfirmed("selected receipt is not the exact original action")
    if action.kind == "delete":
        if selected.sequence is not None or (selected.cleanup_pending is not None
                                              and type(selected.cleanup_pending) is not bool):
            raise SubmitUnconfirmed("recycle receipt is not the selected original terminal operation")
    elif (type(selected.sequence) is not int or not base < selected.sequence < (1 << 64) - 1
          or selected.cleanup_pending is not None):
        raise SubmitUnconfirmed("no exact committed container sequence")
    head = action.node_id if action.kind == "save_node" else action.head
    return ConfirmedAction(ReceiptFence(action.container_id, action.operation_id, action.kind, exact.expected_sequence,
                                        selected.sequence, action.branch, head),
                           selected.cleanup_pending)


def invoke_original(pending, container_id, receipts, exchange: AuthenticatedExchange, *, send_if_absent=False):
    """One container step only. A failure preserves other containers and the entire original plan.

    The controller supplies the verified prefix from local/remote receipts. A caller
    must explicitly allow sending if absent; status-only uses the default.
    """
    if exchange.destination_identity != pending.destination:
        raise SubmitUnconfirmed("not the recorded authenticated destination")
    step = pending.plan.next_for(container_id, receipts)
    if step is None:
        return None
    action, base = step
    observed = exchange.request(lookup_message(action))
    if observed.kind is ReplyKind.SELECTED:
        return verified_result(action, base, observed.receipt)
    if observed.kind is ReplyKind.CONFLICT:
        raise SubmitConflict("the original action conflicts; leave its queue pending")
    if observed.kind is not ReplyKind.ABSENT or observed.receipt is not None:
        raise SubmitUnconfirmed("the original operation is unconfirmed; no request was sent")
    if send_if_absent is not True:
        raise SubmitAbsent("the original receipt does not exist; no request was sent")
    if action.kind == "save_node":
        with pending.body(action) as payload:
            applied = exchange.request(mutation_message(action, base, payload=payload))
    else:
        applied = exchange.request(mutation_message(action, base))
    if applied.kind is ReplyKind.CONFLICT:
        raise SubmitConflict("the original action conflicts; leave its queue pending")
    if applied.kind is not ReplyKind.SELECTED:
        raise SubmitUnconfirmed("no exact original settlement; query or explicitly retry this action only")
    return verified_result(action, base, applied.receipt)
