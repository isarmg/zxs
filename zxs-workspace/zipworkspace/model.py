"""Strict directory-level identity in an ordinary ZIP archive comment."""
from __future__ import annotations

import hashlib
import json
import re
import unicodedata
import uuid
from dataclasses import dataclass, asdict

MAX_MEMBERS = 150_000
MAX_CD = 32 << 20
MAX_TOTAL = 256 << 30
MAX_FILE = 128 << 30
MAX_DEPTH = 48
CHUNK = 1 << 20
MARKER = b"zip-workspace/1\n"


class Rejected(ValueError):
    """Input or original state does not satisfy the repository contract."""


def new_id() -> str:
    return uuid.uuid4().hex


def hexdigest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(obj) -> bytes:
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"), sort_keys=True, allow_nan=False).encode()


def check_id(value: str) -> str:
    if type(value) is not str or not re.fullmatch(r"[0-9a-f]{32}", value):
        raise Rejected("invalid repository identifier")
    return value


@dataclass(frozen=True)
class Commit:
    repo: str
    branch: str
    generation: int
    operation: str
    parent: str | None
    tree: str

    def __post_init__(self):
        for value in (self.repo, self.branch, self.operation):
            check_id(value)
        if self.parent is not None:
            check_id(self.parent)
        if type(self.generation) is not int or not (1 <= self.generation <= 2**63 - 1):
            raise Rejected("invalid branch generation")
        if self.generation > 1 and self.parent is None or self.parent == self.operation:
            raise Rejected("invalid parent")
        if type(self.tree) is not str or not re.fullmatch(r"[a-f0-9]{64}", self.tree):
            raise Rejected("invalid directory digest")

    @property
    def filename(self) -> str:
        # A single immutable slot protects this exact branch generation.
        return f"{self.repo}.{self.branch}.{self.generation:016x}.zxs"

    def encode(self) -> bytes:
        return MARKER + canonical(asdict(self))

    @classmethod
    def decode(cls, data: bytes):
        if not data.startswith(MARKER) or len(data) > 1024:
            raise Rejected("not a typed ZIP Workspace directory repository")
        try:
            obj = json.loads(data[len(MARKER):])
            commit = cls(**obj)
        except (ValueError, TypeError, UnicodeError) as err:
            raise Rejected("invalid directory commit marker") from err
        if commit.encode() != data:
            raise Rejected("noncanonical, duplicate or unknown commit metadata")
        return commit


def member_name(name: str) -> tuple[str, ...]:
    # No lossy sanitisation, rewriting, case conversion or unpacking links.
    if type(name) is not str or not name or len(name.encode("utf-8", "surrogatepass")) > 1024:
        raise Rejected("invalid member name length")
    parts = name.split("/")
    if len(parts) > MAX_DEPTH:
        raise Rejected("member nesting exceeds the supported bound")
    for part in parts:
        if not part or part in (".", "..") or part.endswith((" ", ".")):
            raise Rejected("empty or non-portable member component")
        if unicodedata.normalize("NFC", part) != part or len(part.encode("utf-8", "surrogatepass")) > 240:
            raise Rejected("non-portable member spelling")
        if any(c in '<>:"\\|?*' or unicodedata.category(c) in ("Cc", "Cs") for c in part):
            raise Rejected("forbidden character in member name")
        if re.fullmatch(r"(CON|PRN|AUX|NUL|CLOCK\$|CONIN\$|CONOUT\$|COM[1-9¹²³]|LPT[1-9¹²³])", part.split(".")[0], re.I):
            raise Rejected("reserved device name")
    return tuple(parts)


class Manifest:
    """Tree-wide alias, duplicate, parent and resource validation."""
    def __init__(self):
        self.rows: dict[str, tuple[bool, int, str | None]] = {}
        self.keys: dict[tuple[str, ...], str] = {}
        self.explicit: set[str] = set()
        self.total = 0

    def add(self, name: str, directory: bool, size: int, digest: str | None):
        parts = member_name(name)
        if name in self.explicit:
            raise Rejected("duplicate archive member")
        if type(size) is not int or size < 0 or size > MAX_FILE or directory and size:
            raise Rejected("invalid member size")
        if self.total + size > MAX_TOTAL:
            raise Rejected("total archive data exceeds budget")
        self.total += size
        for end in range(1, len(parts) + 1):
            path = "/".join(parts[:end])
            key = tuple(x.casefold() for x in parts[:end])
            if key in self.keys and self.keys[key] != path:
                raise Rejected("case or spelling aliases would overwrite one another")
            self.keys[key] = path
            kind = end < len(parts) or directory
            if path in self.rows and self.rows[path][0] != kind:
                raise Rejected("member conflicts with file/directory parent")
            self.rows.setdefault(path, (True, 0, None))
        if len(self.rows) > MAX_MEMBERS:
            raise Rejected("tree exceeds member budget")
        self.rows[name] = (directory, size, digest)
        self.explicit.add(name)

    def digest(self) -> str:
        h = hashlib.sha256(b"zip-workspace tree 1\x00")
        for path, (directory, size, digest) in sorted(self.rows.items()):
            if not directory and digest is None:
                raise Rejected("unverified file cannot supply a commit digest")
            row = canonical([path, "d" if directory else "f", size, digest])
            h.update(len(row).to_bytes(4, "big"))
            h.update(row)
        return h.hexdigest()
