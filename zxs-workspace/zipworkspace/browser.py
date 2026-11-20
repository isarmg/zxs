"""Read-only, paged navigation of one completely validated directory commit."""
from __future__ import annotations

from bisect import bisect_left
from dataclasses import dataclass
from operator import attrgetter
import os
from pathlib import Path
from typing import BinaryIO, Literal

from .archive import read_managed_document
from .document import named_document
from .model import Commit, Rejected, hexdigest, member_name


MAX_PAGE = 256
MAX_NAME_QUERY = 128
_NAME = attrgetter("name")


@dataclass(frozen=True, slots=True)
class Entry:
    name: str
    kind: Literal["directory", "file"]
    size_bytes: int | None
    sha256: str | None


@dataclass(frozen=True, slots=True)
class NameMatch:
    parent: str
    entry: Entry

    @property
    def path(self) -> str:
        return f"{self.parent}/{self.entry.name}" if self.parent else self.entry.name


@dataclass(frozen=True, slots=True)
class DirectoryPage:
    commit: Commit
    directory: str
    entries: tuple[Entry, ...]
    next_cursor: str | None


class ArchiveBrowser:
    """An in-memory view of verified names and digests, never of live archive bytes."""

    def __init__(self, document: BinaryIO, *, display_name: str):
        self._commit, tree = read_managed_document(document, display_name=display_name)
        self._data_bytes = tree.total  # Already summed under the member limits and verified tree digest.
        self._key = hexdigest(self._commit.encode())
        children: dict[str, list[Entry]] = {"": []}
        for path, (directory, size, digest) in tree.rows.items():
            parent, _, name = path.rpartition("/")
            if directory:
                children.setdefault(path, [])
                row = Entry(name, "directory", None, None)
            else:
                row = Entry(name, "file", size, digest)
            children.setdefault(parent, []).append(row)
        for rows in children.values():
            rows.sort(key=_NAME)
        self._children = children

    @classmethod
    def open(cls, path: str | os.PathLike[str]) -> ArchiveBrowser:
        """CLI/path adapter. Refuse non-regular documents and a rebound selection."""
        path = Path(path)
        with named_document(path) as doc:
            return cls(doc, display_name=path.name)

    @property
    def commit(self) -> Commit:
        return self._commit

    @property
    def data_bytes(self) -> int:
        return self._data_bytes

    def entry(self, parent, name):
        """Return the exact admitted row, not a matching name from another folder."""
        rows = self._children.get(parent, ())
        i = bisect_left(rows, name, key=_NAME)
        return rows[i] if i < len(rows) and rows[i].name == name else None

    def verified_entries(self):
        """Iterate the admitted rows, including empty and implicit directories; no archive I/O."""
        for parent, children in self._children.items():
            for entry in children:
                yield (f"{parent}/{entry.name}" if parent else entry.name), entry

    def find_names(self, query: str, *, cancelled=None) -> tuple[NameMatch, ...]:
        """One literal scan of the verified index; retain only references, not full path copies."""
        if type(query) is not str or not 1 <= len(query) <= MAX_NAME_QUERY:
            raise Rejected(f"请输入 1 至 {MAX_NAME_QUERY} 个字符的名称片段")
        matches = []
        for parent, rows in self._children.items():
            for entry in rows:
                if cancelled is not None and cancelled():
                    return ()  # Caller must discard the cancelled result, not present it as empty.
                if query in entry.name:
                    matches.append(NameMatch(parent, entry))
        return tuple(matches)

    def locate_name(self, match: NameMatch, *, limit: int = 100) -> tuple[int, int]:
        """Return zero-based page and row in the original parent's entire ordered listing."""
        rows = self._listing(match.parent, limit)
        i = bisect_left(rows, match.entry.name, key=_NAME)
        if i == len(rows) or rows[i] is not match.entry:
            raise Rejected("查找条目不属于当前核验的快照和目录")
        return divmod(i, limit)

    def _listing(self, directory, limit):
        if type(limit) is not int or not 1 <= limit <= MAX_PAGE:
            raise Rejected(f"page size must be between 1 and {MAX_PAGE}")
        if directory != "":
            member_name(directory)
        if directory not in self._children:
            raise Rejected("requested folder does not exist in this snapshot")
        return self._children[directory]

    def page_at(self, directory: str, number: int, *, limit: int = 100) -> DirectoryPage:
        """Index the original ordering without constructing earlier pages."""
        rows = self._listing(directory, limit)
        if type(number) is not int or not 0 <= number <= max(0, (len(rows) - 1) // limit):
            raise Rejected("page number outside this directory")
        return self._page(directory, rows, number * limit, limit)

    def list_directory(
        self, directory: str = "", *, cursor: str | None = None, limit: int = 100
    ) -> DirectoryPage:
        """Immediate children only. A continuation names its original snapshot and folder."""
        rows = self._listing(directory, limit)
        start = 0
        if cursor is not None:
            # ':' cannot occur in an admitted member. The marker includes the
            # complete verified tree digest; a cursor is a position, not authority.
            if not isinstance(cursor, str) or len(cursor) > 64 + 1 + 1024:
                raise Rejected("invalid listing cursor")
            key, sep, path = cursor.partition(":")
            parent, _, name = path.rpartition("/")
            if not sep or key != self._key or parent != directory or not name:
                raise Rejected("cursor belongs to a different snapshot or folder")
            member_name(path)
            i = bisect_left(rows, name, key=_NAME)
            if i == len(rows) or rows[i].name != name:
                raise Rejected("cursor does not identify an original list entry")
            start = i + 1
        return self._page(directory, rows, start, limit)

    def _page(self, directory, rows, start, limit):
        entries = tuple(rows[start:start + limit])
        next_cursor = None
        if start + len(entries) < len(rows):
            path = f"{directory}/{entries[-1].name}" if directory else entries[-1].name
            next_cursor = f"{self._key}:{path}"
        return DirectoryPage(self._commit, directory, entries, next_cursor)
