"""A verified display is not authority to write or a claim about raw ZIP bytes."""
from contextlib import ExitStack
from dataclasses import dataclass, field
import os
from pathlib import Path
import stat
from threading import Lock
from types import MappingProxyType

from .. import plain_import

from ..archive import stamp
from ..browser import ArchiveBrowser
from ..document import archive_sha, named_document, native_identity
from ..model import Rejected
from ..refs import Catalog, Node


class _DisplayFD:
    """Sole owner of one display duplicate; not a buffered body reader."""
    _fd = -1

    def __init__(self, doc):
        self._lock = Lock()
        self._fd = os.dup(doc.fileno())
    @property
    def closed(self): return self._fd < 0
    def fileno(self):
        if self.closed: raise ValueError("the display descriptor is closed")
        return self._fd
    def duplicate(self):
        with self._lock: return os.dup(self.fileno())
    def close(self):
        if self.closed: return  # also safe after a failed construction
        with self._lock:
            if self.closed: return
            fd, self._fd = self._fd, -1
        os.close(fd)
    __del__ = close  # Fallback for an undelivered result; normal ownership is explicit.


@dataclass(frozen=True, slots=True)
class PlainSelection:
    """Ordinary ZIP display with its original read-only descriptor retained for explicit Linux export."""
    preview: plain_import.Preview
    observed: tuple[int, int, int, int, int]
    rows: MappingProxyType
    held: _DisplayFD

    @property
    def path(self): return self.preview.source

    def release_display(self): self.held.close()

    @classmethod
    def open(cls, path):
        path = Path(path).absolute()
        if path.suffix.lower() != ".zip": raise Rejected("只能选择普通 .zip 文件")
        with ExitStack() as guard:
            with named_document(path) as doc:
                preview, manifest, observed = plain_import._inspect_document(doc, path)
                held = _DisplayFD(doc)
                guard.callback(held.close)
            choice = cls(preview, observed, MappingProxyType(manifest.rows), held)
            guard.pop_all()
            return choice


@dataclass(frozen=True, slots=True)
class StoreSelection:
    path: Path
    identity: tuple[int, int]

    @classmethod
    def open(cls, path):
        path = Path(path).absolute()
        return cls(path, native_identity(path.stat(follow_symlinks=False), stat.S_IFDIR))

    def check(self):
        if StoreSelection.open(self.path) != self:
            raise Rejected("selected archive store was replaced; choose the store again")


@dataclass(frozen=True, slots=True)
class Selection:
    path: Path
    observed: tuple[int, int, int, int, int]
    browser: ArchiveBrowser
    exact: Node | None  # Present only when explicitly reopening a previously byte-bound node.
    held: _DisplayFD | None = field(default=None, repr=False, compare=False)

    def release_display(self):
        if self.held is not None: self.held.close()

    @classmethod
    def open(cls, path, *, expected: Node | None = None, retain=False):
        path = Path(path).absolute()
        if expected is not None and expected.path != path:
            raise Rejected("selected original belongs to a different archive path")
        with ExitStack() as guard:
            with named_document(path) as doc:
                original = stamp(doc)
                if expected is not None and original != expected.observed:
                    raise Rejected("the listed archive instance changed; choose or inspect it again")
                browser = ArchiveBrowser(doc, display_name=path.name)
                if expected is not None and (browser.commit != expected.marker or archive_sha(doc) != expected.sha256):
                    raise Rejected("the selected original archive changed; select it again")
                # Only an explicit consumer asks to retain this same open object for handoff.
                held = None
                if retain:
                    held = _DisplayFD(doc)
                    guard.callback(held.close)
            choice = cls(path, original, browser, expected, held)
            guard.pop_all()
            return choice

    def catalog(self, store: StoreSelection):
        # Catalog holds and rechecks the selected anchor through both inventories;
        # a loose display requires its exact original generation, never a bare path.
        result = Catalog(self.path, store.path, expected_store=store.identity,
                         expected_anchor=self.observed)
        if (result.anchor.marker != self.browser.commit or
                (self.exact is not None and result.anchor != self.exact)):
            raise Rejected("selected original changed before catalog inspection")
        return result


def compare_version(left: Selection, store: StoreSelection, catalog: Catalog, node: Node):
    """Compare the admitted left view to exactly one canonical catalog row, not to a later HEAD."""
    from ..comparison import compare_snapshots

    current = left.browser.commit
    if (str(catalog.store) != str(store.path) or catalog.nodes.get(node.marker.operation) is not node or
            str(node.path) != str(store.path / node.marker.filename) or node.marker.repo != current.repo or
            node.marker.operation == current.operation):
        raise Rejected("请选择本次存储中另一条有原始证明的规范版本")
    store.check()
    right = Selection.open(node.path, expected=node)  # No retained display FD, no change to the current selection.
    result = compare_snapshots(left.browser, right.browser)
    store.check()
    return result
