"""Bind a displayed logical snapshot to the actual archive selected for checkout."""
from dataclasses import dataclass
from pathlib import Path
import stat

from ..browser import ArchiveBrowser
from ..document import archive_sha, named_document, native_identity
from ..model import Rejected
from ..refs import Catalog, Node


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
    node: Node
    browser: ArchiveBrowser

    @classmethod
    def open(cls, path, *, expected: Node | None = None):
        path = Path(path).absolute()
        with named_document(path) as doc:
            browser = ArchiveBrowser(doc, display_name=path.name)
            node = Node(browser.commit, path, archive_sha(doc))
        if expected is not None and node != expected:
            raise Rejected("the selected original archive changed; select it again")
        return cls(node, browser)

    def catalog(self, store: StoreSelection):
        store.check()
        result = Catalog(self.node.path, store.path, expected_store=store.identity)
        if result.anchor != self.node:
            raise Rejected("selected original changed before catalog inspection")
        return result
