"""A read-only difference between an admitted snapshot and one observed ordinary folder."""
from dataclasses import dataclass
from pathlib import Path
import os
import stat
import sys

from .archive import stamp
from .document import named_document, native_identity
from .freeze import observe_held
from .model import Rejected


def available():
    if os.name == "nt":
        from .windows_nt import backend
        return backend() is not None  # Per-volume/per-handle eligibility is checked on selection.
    if sys.platform == "darwin":
        from . import macos_read
        if not macos_read.available(): return False
    return (os.name == "posix"
            and all(hasattr(os, name) for name in ("O_DIRECTORY", "O_NOFOLLOW", "O_CLOEXEC", "O_NONBLOCK"))
            and os.open in os.supports_dir_fd and os.stat in os.supports_dir_fd
            and os.stat in os.supports_follow_symlinks and os.scandir in os.supports_fd)


@dataclass(frozen=True, slots=True)
class FolderEvidence:
    path: Path
    identity: tuple[int, int]


class Folder:
    """One live selection. compare() consumes it; close it if the job never starts."""
    def __init__(self, evidence, fd, darwin=None):
        self.evidence = evidence
        self._fd = fd
        self._darwin = darwin

    @classmethod
    def choose(cls, path):
        if os.name == "nt":
            from .windows_nt import backend
            from .windows_folder import WindowsFolder
            return WindowsFolder.choose(path, backend())
        if not available():
            raise Rejected("cannot securely read a selected folder on this platform")
        if sys.platform != "darwin": return cls._choose_posix(path)
        from . import macos_read
        selected = None
        try:
            with macos_read.no_materialization():  # Even first lookup can touch a dataless intermediate folder.
                selected = cls._choose_posix(path, macos_read)
            return selected
        except BaseException:
            if selected is not None: selected.close()  # Policy restore failed before ownership transfer.
            raise

    @classmethod
    def _choose_posix(cls, path, darwin=None):
        path = Path(path).absolute()
        named = native_identity(path.stat(follow_symlinks=False), stat.S_IFDIR)
        fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            result = cls(FolderEvidence(path, named), fd, darwin)
            result._check()
            return result
        except BaseException:
            os.close(fd)
            raise

    def _check(self):
        if (self._fd is None
                or native_identity(os.fstat(self._fd), stat.S_IFDIR) != self.evidence.identity
                or native_identity(self.evidence.path.stat(follow_symlinks=False), stat.S_IFDIR) != self.evidence.identity):
            raise Rejected("chosen folder no longer names the held directory")
        if self._darwin:
            self._darwin.local_object(self.evidence.path, self._fd)
        return self._fd

    def check(self):
        if self._darwin:
            with self._darwin.no_materialization(): return self._check()
        return self._check()

    def observe(self):
        if self._darwin:
            with self._darwin.no_materialization():
                return observe_held(self._check(), self.evidence.identity, require_ids=True, check_held=self._darwin.resident)
        return observe_held(self.check(), self.evidence.identity, require_ids=True)

    def close(self):
        fd, self._fd = self._fd, None
        if fd is not None: os.close(fd)


@dataclass(frozen=True, slots=True)
class Difference:
    path: str
    change: str       # added, changed or missing in the observed local folder
    snapshot: str | None
    local: str | None


@dataclass(frozen=True, slots=True)
class Report:
    folder: FolderEvidence
    operation: str
    added: int
    changed: int
    missing: int
    rows: tuple[Difference, ...]

    PAGE = 100

    def page(self, number):
        if type(number) is not int or not 0 <= number < max(1, (len(self.rows) + self.PAGE - 1) // self.PAGE):
            raise Rejected("comparison page outside the observed report")
        return self.rows[number*self.PAGE:(number+1)*self.PAGE]


def compare(selected, folder: Folder):
    """Use the already verified archive content, not registered state or current HEAD."""
    try:
        if sys.platform == "darwin":
            from . import macos_read
            with macos_read.no_materialization():  # Worker, including the archive's final named recheck.
                return _compare(selected, folder, macos_read)
        return _compare(selected, folder)
    finally:
        folder.close()


def _compare(selected, folder, darwin=None):
    changes = []
    added = changed = missing = 0
    with named_document(selected.path) as doc:
        if stamp(doc) != selected.observed:
            raise Rejected("displayed archive changed; reopen it before comparing")
        if darwin: darwin.local_object(selected.path, doc.fileno())
        tree = folder.observe()
        # This Manifest belongs exclusively to this scan. Consuming its rows
        # avoids another full set of keys. At most the two admitted row budgets
        # can differ; no file payloads or live file handles enter the result.
        local = tree.rows
        for path, row in selected.browser.verified_entries():
            value = local.pop(path, None)
            is_dir = row.kind == "directory"
            if value is None:
                changes.append(Difference(path, "missing", row.kind, None))
                missing += 1
            elif value != (is_dir, 0 if is_dir else row.size_bytes, row.sha256):
                changes.append(Difference(path, "changed", row.kind, "directory" if value[0] else "file"))
                changed += 1
        for path, (is_dir, _, _) in local.items():
            changes.append(Difference(path, "added", None, "directory" if is_dir else "file"))
            added += 1
        changes.sort(key=lambda item: item.path)
        folder.check()
        if darwin: darwin.local_object(selected.path, doc.fileno())
    return Report(folder.evidence, selected.browser.commit.operation, added, changed, missing, tuple(changes))
