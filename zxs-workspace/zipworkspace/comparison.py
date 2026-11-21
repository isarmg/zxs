"""Read-only path differences between verified snapshots or against one observed folder."""
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

    def observe(self, *, observer=None):
        options = {} if observer is None else {"observer": observer}
        if self._darwin:
            with self._darwin.no_materialization():
                return observe_held(self._check(), self.evidence.identity, require_ids=True,
                                    check_held=self._darwin.resident, **options)
        return observe_held(self.check(), self.evidence.identity, require_ids=True, **options)

    def close(self):
        fd, self._fd = self._fd, None
        if fd is not None: os.close(fd)


@dataclass(frozen=True, slots=True)
class Difference:
    path: str
    change: str       # added, changed or missing on the comparison side
    before: str | None
    after: str | None
    before_bytes: int | None
    after_bytes: int | None


@dataclass(frozen=True, slots=True)
class Changes:
    added: int
    changed: int
    missing: int
    rows: tuple[Difference, ...]

    PAGE = 100

    def page(self, number):
        if type(number) is not int or not 0 <= number < max(1, (len(self.rows) + self.PAGE - 1) // self.PAGE):
            raise Rejected("comparison page outside the observed report")
        return self.rows[number*self.PAGE:(number+1)*self.PAGE]


@dataclass(frozen=True, slots=True)
class Report(Changes):
    folder: FolderEvidence
    operation: str


def _value(entry):
    is_dir = entry.kind == "directory"
    return is_dir, 0 if is_dir else entry.size_bytes, entry.sha256


def _rows(browser):
    return ((path, _value(entry)) for path, entry in browser.verified_entries())


def _side(value):
    if value is None: return None, None
    return ("directory", None) if value[0] else ("file", value[1])


def _changes(before, other):
    # Consume the transient right-hand inventory. Use exact portable paths, not
    # the host's spelling rules or a guessed rename/chronological direction.
    changes = []
    added = changed = missing = 0
    for path, row in before:
        value = other.pop(path, None)
        if value == row: continue
        change = "missing" if value is None else "changed"
        missing += value is None
        changed += value is not None
        old_kind, old_size = _side(row)
        new_kind, new_size = _side(value)
        changes.append(Difference(path, change, old_kind, new_kind, old_size, new_size))
    for path, value in other.items():
        kind, size = _side(value)
        changes.append(Difference(path, "added", None, kind, None, size))
        added += 1
    changes.sort(key=lambda item: item.path)
    return added, changed, missing, tuple(changes)


def compare_manifests(base, observed):
    """Compare verified content; consumes the transient observed Manifest's rows."""
    return Changes(*_changes(base.rows.items(), observed.rows))


def compare_to_index(index, observed):
    """A previous full archive verification, compared to the selected local scan."""
    return Changes(*_changes(_rows(index), observed.rows))


def compare_plain(before, verified_rows):
    """Compare an admitted .zxs view to a verified plain ZIP manifest, without any I/O or repo identity."""
    return Changes(*_changes(_rows(before), dict(verified_rows)))


def compare_snapshots(before, other):
    """Both ArchiveBrowsers already verified; never open live bodies here."""
    if before.commit.repo != other.commit.repo:
        raise Rejected("the two snapshots are not from one archive repository")
    return compare_archive_contents(before, other)


def compare_archive_contents(before, other):
    """Compare two verified logical trees without implying any history relation."""
    if before.commit.tree == other.commit.tree:
        return Changes(0, 0, 0, ())
    return Changes(*_changes(_rows(before), dict(_rows(other))))


def compare(selected, folder: Folder, *, observer=None):
    """Use the already verified archive content, not registered state or current HEAD."""
    try:
        if observer is not None: observer.begin("identity")
        if sys.platform == "darwin":
            from . import macos_read
            with macos_read.no_materialization():  # Worker, including the archive's final named recheck.
                report = _compare(selected, folder, macos_read, observer=observer)
        else: report = _compare(selected, folder, observer=observer)
        if observer is not None: observer.check()
        return report
    finally:
        folder.close()


def _compare(selected, folder, darwin=None, *, observer=None):
    with named_document(selected.path) as doc:
        if stamp(doc) != selected.observed:
            raise Rejected("displayed archive changed; reopen it before comparing")
        if darwin: darwin.local_object(selected.path, doc.fileno())
        # The scan does not return payload bytes. Its own Manifest rows can be
        # consumed without making a third copy of the path inventory.
        tree = folder.observe() if observer is None else folder.observe(observer=observer)
        if observer is not None: observer.begin("compare")
        delta = _changes(_rows(selected.browser), tree.rows)
        if observer is not None: observer.begin("identity")
        folder.check()
        if darwin: darwin.local_object(selected.path, doc.fileno())
    return Report(*delta, folder.evidence, selected.browser.commit.operation)
