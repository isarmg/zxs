"""Plan explicit per-container actions from one attested local view; no I/O."""
from dataclasses import asdict, dataclass, replace
import json
import re

from .model import Rejected, canonical, check_id, member_name, new_id


def _id(value):
    value = check_id(value)
    if value == "0" * 32:
        raise Rejected("zero is a reserved wire sentinel, not an identity")
    return value


def _revision(value):
    if type(value) is not int or not 0 <= value < (1 << 64) - 1:
        raise Rejected("invalid exact container revision")
    return value


def _branch(value):
    return value if value == "main" else _id(value)


def _tags(values):
    if (type(values) is not tuple or any(type(v) is not str or not v for v in values)
            or len(values) != len(set(values))):
        raise Rejected("invalid exact container tag set")
    for value in values:
        value.encode("utf-8")
    return values


@dataclass(frozen=True)
class FullBody:
    length: int
    sha256: str

    def __post_init__(self):
        if type(self.length) is not int or not 0 <= self.length < (1 << 64):
            raise Rejected("invalid full file length")
        if type(self.sha256) is not str or re.fullmatch(r"[0-9a-f]{64}", self.sha256) is None:
            raise Rejected("invalid full file SHA-256")


@dataclass(frozen=True)
class LocalFile:
    container_id: str | None
    path: str
    body: FullBody
    tags: tuple[str, ...] = ()

    def __post_init__(self):
        if self.container_id is not None:
            _id(self.container_id)
        member_name(self.path)
        _tags(self.tags)
        if type(self.body) is not FullBody:
            raise Rejected("local file does not describe complete bytes")


@dataclass(frozen=True)
class Baseline:
    container_id: str
    name: str
    revision: int
    body: FullBody
    tags: tuple[str, ...] = ()
    branch: str = "main"
    selected_node: str | None = None
    head: str | None = None

    def __post_init__(self):
        _id(self.container_id)
        _branch(self.branch)
        _id(self.selected_node)
        _id(self.head)
        if len(member_name(self.name)) != 1 or _revision(self.revision) == 0:
            raise Rejected("baseline needs one server display name and an existing revision")
        if type(self.body) is not FullBody:
            raise Rejected("body must identify the exact selected baseline node")
        _tags(self.tags)


@dataclass(frozen=True)
class DeleteIntent:
    container_id: str
    revision: int

    def __post_init__(self):
        _id(self.container_id)
        if _revision(self.revision) == 0:
            raise Rejected("refusing an unregistered remote deletion")


@dataclass(frozen=True)
class BranchIntent:
    container_id: str
    new_branch_id: str
    source_node_id: str

    def __post_init__(self):
        _id(self.container_id)
        _id(self.new_branch_id)
        _id(self.source_node_id)


def from_atomic_editor_replace(temporary, exact_target, *, journal_target_id, journal_target_path):
    """Only after the native journal proved one temp overwrite of this exact target.

    The caller retires the transient name locally. A scan of path/inode/name
    alone is not this proof; a tracked source may not be absorbed into another ID.
    """
    expected = _id(journal_target_id)
    if (temporary.container_id is not None or exact_target.container_id != expected
            or exact_target.path != journal_target_path or temporary.path == exact_target.path):
        raise Rejected("not the proven atomic temporary replacement of an existing container")
    return replace(exact_target, body=temporary.body)


@dataclass(frozen=True)
class Action:
    operation_id: str
    container_id: str
    kind: str
    base_revision: int | None
    after: str | None
    body: FullBody | None = None
    name: str | None = None
    tags: tuple[str, ...] | None = None
    branch: str | None = None
    head: str | None = None
    node_id: str | None = None
    fork_from_main: str | None = None

    def __post_init__(self):
        _id(self.operation_id)
        _id(self.container_id)
        if (self.base_revision is None) == (self.after is None):
            raise Rejected("exactly one original base or verified predecessor is required")
        if self.after is not None:
            _id(self.after)
        initial = self.base_revision is not None and _revision(self.base_revision) == 0
        if initial and (self.kind != "save_node" or self.branch != "main" or self.head is not None
                        or self.fork_from_main is not None):
            raise Rejected("only the first whole-file save on main can create a container")
        expected = {"create_branch": (False, False, False, True),
                    "save_node": (True, initial, False, True), "rename": (False, True, False, False),
                    "delete": (False, False, False, False), "set_tags": (False, False, True, False)}
        actual = (self.body is not None, self.name is not None, self.tags is not None, self.branch is not None)
        if expected.get(self.kind) != actual:
            raise Rejected("action includes fields belonging to another operation")
        if self.body is not None and type(self.body) is not FullBody:
            raise Rejected("content action must describe the full individual file")
        if self.name is not None and len(member_name(self.name)) != 1:
            raise Rejected("server rename only accepts a container display name, not a workspace path")
        if self.tags is not None:
            _tags(self.tags)
        if self.branch is not None:
            _branch(self.branch)
        if self.head is not None:
            if self.branch is None:
                raise Rejected("only node or branch actions have a source head")
            _id(self.head)
        if self.kind == "create_branch" and (self.branch == "main" or self.head is None):
            raise Rejected("a new branch must originate at one exact existing node")
        if self.kind == "save_node":
            _id(self.node_id)
            if self.node_id in (self.head, self.fork_from_main):
                raise Rejected("SaveNode must create a distinct node")
            if initial is False and self.head is None and self.fork_from_main is None:
                raise Rejected("existing container save needs the exact target head or historical source")
            if self.fork_from_main is not None:
                if self.branch == "main" or self.head is not None:
                    raise Rejected("historic main source requires one separate branch in the same save")
                _id(self.fork_from_main)
        elif self.node_id is not None or self.fork_from_main is not None:
            raise Rejected("only whole-file SaveNode has new node or atomic historical source identity")


@dataclass(frozen=True)
class ReceiptFence:
    """Use only after transport verified an exact kind-specific durable receipt."""

    container_id: str
    operation_id: str
    kind: str
    base_revision: int | None
    sequence: int | None
    branch: str | None = None
    head: str | None = None


@dataclass(frozen=True)
class Plan:
    actions: tuple[Action, ...]
    placements: tuple[tuple[str, str], ...]

    def __post_init__(self):
        seen, paths, last, mutations = set(), set(), {}, set()
        for container_id, path in self.placements:
            _id(container_id)
            folded = tuple(p.casefold() for p in member_name(path))
            if container_id in seen or folded in paths:
                raise Rejected("ambiguous local container identity or file name")
            seen.add(container_id)
            paths.add(folded)
        if any(path[:n] in paths for path in paths for n in range(1, len(path))):
            raise Rejected("one local file shadows a directory")
        operations = set()
        for op in self.actions:
            prev = last.get(op.container_id)
            key = (op.container_id, op.kind)
            if (op.operation_id in operations or key in mutations or op.after != prev
                    or (op.kind == "delete" and op.container_id in seen)
                    # Explicit CreateBranch may be a no-body metadata command with no local file.
                    or (op.kind not in ("delete", "create_branch") and op.container_id not in seen)
                    or (prev is not None and op.kind == "delete")):
                raise Rejected("ambiguous operation set or missing exact predecessor")
            operations.add(op.operation_id)
            mutations.add(key)
            last[op.container_id] = op.operation_id

    def queues(self):
        by_container = {}
        for action in self.actions:
            by_container.setdefault(action.container_id, []).append(action)
        return tuple((container_id, tuple(actions)) for container_id, actions in by_container.items())

    def next_for(self, container_id, receipts=()):
        """Independent queue: only exact confirmed prefix advances. Never sends/retries itself."""
        _id(container_id)
        queue = dict(self.queues()).get(container_id, ())
        if len(receipts) > len(queue):
            raise Rejected("receipts exceed the container's original pending queue")
        last = None
        for action, receipt in zip(queue, receipts):
            base = action.base_revision if last is None else last.sequence
            if (type(receipt) is not ReceiptFence or
                    (receipt.container_id, receipt.operation_id, receipt.kind) !=
                    (container_id, action.operation_id, action.kind)
                    or (action.kind == "delete" and receipt.sequence is not None)
                    or (action.kind != "delete" and _revision(receipt.sequence) <= base)
                    or (action.kind in ("save_node", "create_branch") and receipt.base_revision is not None)
                    or (action.kind not in ("save_node", "create_branch") and receipt.base_revision != base)):
                raise Rejected("receipt did not confirm an exact original container action")
            # Save and CreateBranch use exact node targets, not an unrelated
            # revision CAS. Recycle is terminal and reports only original sequence.
            if action.kind == "create_branch" and (receipt.branch, receipt.head) != (action.branch, action.head):
                raise Rejected("branch receipt did not confirm the exact inherited node")
            if action.kind == "save_node" and (receipt.branch, receipt.head) != (action.branch, action.node_id):
                raise Rejected("save receipt did not confirm the exact new node in the original branch")
            last = receipt
        if len(receipts) == len(queue):
            return None
        action = queue[len(receipts)]
        return action, action.base_revision if last is None else last.sequence

    def encode(self):
        """Persist this exact byte record and seal every body before sending an action."""
        return canonical({"version": 1, "actions": [asdict(op) for op in self.actions],
                          "placements": self.placements})

    @classmethod
    def decode(cls, raw):
        try:
            record = json.loads(raw)
            ops = tuple(Action(**{**op, "body": None if op["body"] is None else FullBody(**op["body"]),
                                  "tags": None if op["tags"] is None else tuple(op["tags"])})
                        for op in record["actions"])
            plan = cls(ops, tuple(tuple(entry) for entry in record["placements"]))
            if plan.encode() != raw:
                raise Rejected("operation set was not the original canonical record")
            return plan
        except (TypeError, ValueError, KeyError) as failure:
            raise Rejected("cannot recover the exact original submit plan") from failure


def plan_submit(baseline, observed, explicit_deletions, *, branch_intents=(), attested_complete=False, force_save=(),
                operation_ids=new_id, branch_ids=new_id, node_ids=new_id):
    """Use an already verified whole local view; the server never sees its tree.

    Missing entries are insufficient to authorize delete. Callers persist the
    plan under the explicit submit and attach retained per-file bodies to its
    save_node operation IDs. No directory pack, offset/delta or global atomicity.
    """
    if attested_complete is not True:
        raise Rejected("incomplete local observation is not a submit plan")
    prior, now, deletes = {}, {}, {}
    for row in baseline:
        if type(row) is not Baseline or row.container_id in prior:
            raise Rejected("ambiguous source baseline")
        prior[row.container_id] = row
    for row in observed:
        if type(row) is not LocalFile or row.container_id is None or row.container_id in now:
            raise Rejected("unresolved local file identity")
        now[row.container_id] = row
    for deletion in explicit_deletions:
        if type(deletion) is not DeleteIntent or deletion.container_id in deletes:
            raise Rejected("ambiguous explicit deletion")
        old = prior.get(deletion.container_id)
        if old is None or old.revision != deletion.revision or deletion.container_id in now:
            raise Rejected("explicit deletion does not match the exact missing original")
        deletes[deletion.container_id] = deletion.revision
    if prior.keys() - now.keys() != deletes.keys():
        raise Rejected("missing original lacks an exact explicit deletion")

    forced = set(force_save)
    if not forced <= prior.keys() & now.keys():
        raise Rejected("explicit replacement must still identify one live original container")

    forks = {}
    for intent in branch_intents:
        if type(intent) is not BranchIntent or intent.container_id in forks:
            raise Rejected("ambiguous explicit branch intent")
        old = prior.get(intent.container_id)
        if (old is None or intent.container_id not in now or old.selected_node != intent.source_node_id
                or old.branch == intent.new_branch_id):
            raise Rejected("branch intent is not tied to this exact original node")
        forks[intent.container_id] = intent

    actions = []
    for container_id in sorted(prior.keys() | now.keys()):
        old, current = prior.get(container_id), now.get(container_id)
        prior_revision = old.revision if old is not None else 0
        previous = None

        def add(kind, **change):
            nonlocal previous
            operation = operation_ids()
            actions.append(Action(operation, container_id, kind,
                                  prior_revision if previous is None else None, previous, **change))
            previous = operation

        if current is None:
            add("delete")
            continue
        name = member_name(current.path)[-1]
        if old is None:
            add("save_node", body=current.body, branch="main", name=name, node_id=node_ids())
        else:
            changed = current.body != old.body or container_id in forced
            target_branch, base_head = old.branch, old.selected_node
            fork = forks.get(container_id)
            from_main = None
            if changed and fork is None and old.selected_node != old.head:
                if old.branch != "main":
                    raise Rejected("historical non-main edit needs an exact explicit branch intent")
                # One explicit Submit, one atomic branch+node SaveNode. Do not
                # send a separate CreateBranch or modify the original main head.
                target_branch = _id(branch_ids())
                from_main, base_head = old.selected_node, None
            if fork is not None:
                target_branch = fork.new_branch_id
                add("create_branch", branch=target_branch, head=fork.source_node_id)
            if changed:
                add("save_node", body=current.body, branch=target_branch, head=base_head,
                    node_id=node_ids(), fork_from_main=from_main)
            if name != old.name:
                add("rename", name=name)
        old_tags = set() if old is None else set(old.tags)
        if old_tags != set(current.tags):
            add("set_tags", tags=tuple(sorted(current.tags)))

    return Plan(tuple(actions), tuple(sorted((id_, file.path) for id_, file in now.items())))
