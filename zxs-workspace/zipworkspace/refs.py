"""Verified directory-level references from one observed store; never file-member refs."""
from __future__ import annotations
from dataclasses import dataclass
import os
from pathlib import Path
import stat
from .archive import read_managed_document, stamp
from .document import archive_sha, named_document, native_identity
from .model import Commit, Rejected, check_id


@dataclass(frozen=True)
class Node:
    marker: Commit
    path: Path
    sha256: str
    observed: tuple[int, int, int, int, int]  # Original held slot, never persisted in the ZIP.


def _origin(path):
    m = path.stat(follow_symlinks=False)
    native_identity(m, stat.S_IFREG)
    return (m.st_dev, m.st_ino, m.st_size, m.st_mtime_ns, m.st_ctime_ns)


def _read_node(doc, path, expected, *, observer=None):
    observed = stamp(doc)
    if expected is not None and observed != expected:
        raise Rejected("opened archive is not the selected or inventoried slot")
    options = {} if observer is None else {"observer": observer}
    marker, _ = read_managed_document(doc, display_name=path.name, **options)
    node = Node(marker, path, archive_sha(doc, **options), observed)
    if stamp(doc) != observed:
        raise Rejected("selected or inventoried archive changed while reading")
    return node


def _members(store, repo, *, check=None):
    result = {}
    with os.scandir(store) as names:
        for n, entry in enumerate(names, 1):
            if check is not None: check()
            if n > 10_000: raise Rejected("directory catalog exceeds current scan budget")
            if entry.name.startswith(repo+".") and entry.name.endswith(".zxs"):
                result[entry.name] = _origin(store / entry.name)
    return result


class Catalog:
    def __init__(self, anchor: Path, store: Path, *, expected_store=None, expected_anchor=None, observer=None):
        if observer is not None: observer.begin("catalog")
        anchor = Path(anchor).absolute()  # Do not resolve aliases into canonical store slots.
        self.store = Path(store).absolute()
        store_id = native_identity(self.store.stat(follow_symlinks=False), stat.S_IFDIR)
        if expected_store is not None and store_id != expected_store:
            raise Rejected("selected original archive store changed before inventory")
        # Keep the original file open through both inventories. named_document
        # also rechecks this name and its held generation before returning the catalog.
        with named_document(anchor) as doc:
            self.anchor = _read_node(doc, anchor, expected_anchor, observer=observer)
            self._discover(store_id, observer=observer)

    def _discover(self, store_id, *, observer=None):
        self.repo = self.anchor.marker.repo
        self.nodes = {}
        self.slots = {}
        check = observer.check if observer is not None else None
        scan_options = {} if check is None else {"check": check}
        if observer is not None: observer.begin("catalog")
        before = _members(self.store, self.repo, **scan_options)
        for index, name in enumerate(sorted(before), 1):
            if observer is not None: observer.in_archive(index, len(before))
            path = self.store / name
            if name == self.anchor.marker.filename and str(path) == str(self.anchor.path):
                # Path equality alone may case-fold on Windows. An inode match
                # at a differently spelled path is not the selected storage name.
                if before[name] != self.anchor.observed:
                    raise Rejected("selected archive no longer occupies its inventoried slot")
                node = self.anchor
            else:
                with named_document(path) as doc:
                    node = _read_node(doc, path, before[name], observer=observer)
            m = node.marker
            if m.repo != self.repo or m.filename != name:
                raise Rejected("directory repository slot disagrees with its exact typed archive")
            if m.operation in self.nodes or (m.branch, m.generation) in self.slots:
                raise Rejected("duplicate operation or branch generation; no branch head selected")
            self.nodes[m.operation] = node
            self.slots[m.branch, m.generation] = node
        if observer is not None:
            observer.in_archive(0, 0)
            observer.begin("catalog")
        if before != _members(self.store, self.repo, **scan_options) or store_id != native_identity(self.store.stat(follow_symlinks=False), stat.S_IFDIR):
            raise Rejected("archive store changed during branch discovery; retry the read")
        a = self.anchor.marker
        occupied = self.slots.get((a.branch, a.generation))
        same_op = self.nodes.get(a.operation)
        if occupied is not None and (occupied.marker != a or occupied.sha256 != self.anchor.sha256):
            raise Rejected("chosen original is different from the occupied branch slot")
        if same_op is not None and (same_op.marker != a or same_op.sha256 != self.anchor.sha256):
            raise Rejected("chosen original operation conflicts with the store")
        # An isolated anchor still recovers its own files/history fragment. It
        # does not turn an absent storage slot into a publishable branch head.
        self.history_nodes = dict(self.nodes)
        self.history_nodes.setdefault(a.operation, self.anchor)
        self._validate_edges()
        self.store_gaps = self._missing(self.nodes)
        if check is not None: check()

    def _validate_edges(self):
        roots = 0
        for node in self.history_nodes.values():
            marker = node.marker
            if marker.parent is None:
                roots += 1
                continue
            parent = self.history_nodes.get(marker.parent)
            if parent is not None:
                prev = parent.marker
                if marker.generation == 1:
                    if prev.branch == marker.branch:
                        raise Rejected("first generation on a fork cannot reuse its parent's branch")
                elif prev.branch != marker.branch or prev.generation + 1 != marker.generation:
                    raise Rejected("same-branch ancestry does not advance exactly one generation")
            elif marker.generation > 1 and (marker.branch, marker.generation-1) in self.slots:
                raise Rejected("occupied preceding generation does not contain the claimed parent")
        if roots > 1: raise Rejected("independent genesis claims for one repository")
        self._missing(self.history_nodes)  # validate cycles including a loose anchor

    @staticmethod
    def _missing(nodes):
        # Iterative memoized ancestry avoids deep recursion and repeated walks.
        # None = known genesis reached, otherwise exact first missing parent ID.
        result = {}
        for start in nodes:
            cursor, path, seen = start, [], set()
            while cursor is not None and cursor not in result and cursor in nodes:
                if cursor in seen: raise Rejected("directory ancestry cycle")
                seen.add(cursor); path.append(cursor)
                cursor = nodes[cursor].marker.parent
            missing = result[cursor] if cursor in result else cursor
            for operation in reversed(path): result[operation] = missing
        return result

    def history(self, operation):
        check_id(operation)
        if operation not in self.history_nodes: raise Rejected("requested directory node not found in this repository")
        chain = []
        while operation in self.history_nodes:
            marker = self.history_nodes[operation].marker
            chain.append(marker); operation = marker.parent
        return chain, operation

    def branches(self):
        tips = {}
        for (branch, generation), node in self.slots.items():
            if branch not in tips or tips[branch].marker.generation < generation: tips[branch] = node
        # Ancestors may be visible only as the caller's loose file, not in the
        # actual selected store. A loose anchor cannot close that evidence gap.
        result = []
        for branch, tip in sorted(tips.items()):
            missing = self.store_gaps[tip.marker.operation]
            result.append({"branch":branch, "observed_generation":tip.marker.generation,
                           "observed_operation":tip.marker.operation, "missing_parent":missing,
                           "complete_observed_head":tip.marker.operation if missing is None else None})
        return result

    def select_branch(self, branch):
        check_id(branch)
        for row in self.branches():
            if row["branch"] == branch:
                if row["complete_observed_head"] is None:
                    raise Rejected("branch has missing original ancestry; no head selected")
                return self.nodes[row["complete_observed_head"]]
        raise Rejected("branch not found in selected repository")

    def select_node(self, operation):
        check_id(operation)
        if operation not in self.history_nodes: raise Rejected("selected original directory commit unavailable")
        return self.history_nodes[operation]


def checkout(node: Node, destination, store, state, *, private_stage=False):
    # Do not let a re-read inside checkout substitute another commit after
    # branch selection. The exact marker+whole-file digest accompany the path.
    from .worktree import checkout as open_worktree
    return open_worktree(node.path, destination, store, state, expected=node, private_stage=private_stage)
