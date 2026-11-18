"""A verified display is not authority to write or a claim about raw ZIP bytes."""
from dataclasses import dataclass
from pathlib import Path
import stat

from ..archive import stamp
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
    path: Path
    observed: tuple[int, int, int, int, int]
    browser: ArchiveBrowser
    exact: Node | None  # Present only when explicitly reopening a previously byte-bound node.

    @classmethod
    def open(cls, path, *, expected: Node | None = None):
        path = Path(path).absolute()
        if expected is not None and expected.path != path:
            raise Rejected("selected original belongs to a different archive path")
        with named_document(path) as doc:
            original = stamp(doc)
            if expected is not None and original != expected.observed:
                raise Rejected("the listed archive instance changed; choose or inspect it again")
            browser = ArchiveBrowser(doc, display_name=path.name)
            if expected is not None and (browser.commit != expected.marker or archive_sha(doc) != expected.sha256):
                raise Rejected("the selected original archive changed; select it again")
        # All member bytes and the typed marker were verified. Initial browsing
        # does not also stream ZIP headers, payloads and comment for a raw SHA.
        return cls(path, original, browser, expected)

    def catalog(self, store: StoreSelection):
        store.check()
        with named_document(self.path) as doc:
            if stamp(doc) != self.observed:
                raise Rejected("the displayed archive changed before catalog inspection")
            # Catalog admits byte-exact anchors and store rows: this is the
            # first place a loose display can obtain an exact original Node.
            result = Catalog(self.path, store.path, expected_store=store.identity,
                             expected_anchor=self.observed)
            if (result.anchor.marker != self.browser.commit or
                    (self.exact is not None and result.anchor != self.exact)):
                raise Rejected("selected original changed before catalog inspection")
        return result
