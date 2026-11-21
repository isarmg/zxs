"""Read-only path differences between verified snapshots or against one observed folder."""
from contextlib import ExitStack, closing, contextmanager
from dataclasses import dataclass, field
from pathlib import Path
import os
import stat
import sys
from threading import Lock

from .archive import stamp
from .document import named_document, native_identity
from .freeze import _sig, observe_held
from .linux_authority import ProcView
from .model import Rejected, member_name
from .private_freeze import local_mount, mount_id

COMPARE_BYTES = 64 * 1024


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


def _archive_stamp(info):
    return (*native_identity(info, stat.S_IFREG), info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def _check_archive(selected, original, proc=None, expected_mount=None):
    if _archive_stamp(os.fstat(original)) != selected.observed:
        raise Rejected("已显示的原归档发生变化；请重新打开后比较")
    if proc is None:
        if _archive_stamp(selected.path.stat(follow_symlinks=False)) != selected.observed:
            raise Rejected("归档已不在原名称；未重新打开正文")
        return None
    named = os.open(selected.path, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        actual = mount_id(original, read_proc=proc.read)
        if (_archive_stamp(os.fstat(named)) != selected.observed or
                mount_id(named, read_proc=proc.read) != actual or
                expected_mount is not None and expected_mount != actual):
            raise Rejected("原归档的名称或实际挂载已变化；未重新打开正文")
        return actual
    finally:
        os.close(named)


def _check_root(evidence, root, mount, proc):
    named = os.open(evidence.path, os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        if (any(native_identity(os.fstat(fd), stat.S_IFDIR) != evidence.identity or
                mount_id(fd, read_proc=proc.read) != mount for fd in (root, named)) or
                os.readlink(str(root), dir_fd=proc.fds) != str(evidence.path)):
            raise Rejected("原文件夹或实际挂载已改变；未选用同名替件")
    finally:
        os.close(named)


class _FolderText:
    """One original directory and metadata from this report, not a second inventory or text cache."""
    _root = None

    def __init__(self, selected, evidence, root, mount, archive_mount, pairs):
        self.selected, self.evidence, self.mount, self.archive_mount = selected, evidence, mount, archive_mount
        self._lock, self._pairs = Lock(), pairs
        self._root = os.dup(root)

    def _pair(self, selected, row):
        if self._root is None or self.selected is not selected or row is None: return None
        pair = self._pairs.get(row.path)
        return pair if pair is not None and pair[0] is row else None

    def supports(self, selected, row):
        with self._lock: return self._pair(selected, row) is not None

    def close(self):
        if self._root is None: return
        with self._lock:
            root, self._root = self._root, None
            self._pairs.clear()
        if root is not None: os.close(root)

    __del__ = close  # Normal ownership ends on cancellation, replacement or report close.

    @contextmanager
    def open_pair(self, selected, row, check):
        check()
        with ExitStack() as held:
            with self._lock:
                pair = self._pair(selected, row)
                if pair is None: raise Rejected("不属于本次文件夹对照的原差异行")
                root = os.dup(self._root)
                held.callback(os.close, root)
            _, expected, left_sha, right_sha = pair
            if ("/".join(member_name(row.path)) != row.path or expected[3] != row.after_bytes):
                raise Rejected("原相对路径或已观察大小不符")
            original = held.enter_context(selected.held.read_lease())
            proc = ProcView(held)
            metadata = os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC

            def exact_leaf(fd):
                info = os.fstat(fd)
                if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or _sig(info) != expected or
                        mount_id(fd, read_proc=proc.read) != self.mount):
                    raise Rejected("本机文件与对照时的原名称、实体或状态不同；请刷新对照")

            def originals():
                check()
                _check_archive(selected, original, proc, self.archive_mount)
                _check_root(self.evidence, root, self.mount, proc)
                check()

            originals()
            named = proc.open_beneath(root, row.path, metadata)
            held.callback(os.close, named)
            exact_leaf(named)
            # Read that exact regular object, not a second lookup of its leaf.
            # Positively admit the numeric proc entry itself before following its magic link.
            referent = proc.open_beneath(proc.fds, str(named), metadata)
            held.callback(os.close, referent)
            if not stat.S_ISLNK(os.fstat(referent).st_mode):
                raise Rejected("未取得原普通文件的 procfs 句柄链接")
            item = os.open(str(named), os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=proc.fds)
            held.callback(os.close, item)
            exact_leaf(item)
            check()
            yield original, item, left_sha, right_sha
            originals()
            final = proc.open_beneath(root, row.path, metadata)
            held.callback(os.close, final)
            for fd in (named, item, final): exact_leaf(fd)
            check()


@dataclass(frozen=True, slots=True)
class Report(Changes):
    folder: FolderEvidence
    operation: str
    text: _FolderText | None = field(default=None, repr=False, compare=False)

    def close(self):
        if self.text is not None: self.text.close()


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


def compare(selected, folder: Folder, *, observer=None, retain_text=False):
    """Use the already verified archive content, not registered state or current HEAD."""
    with ExitStack() as output:
        with closing(folder):
            if observer is not None: observer.begin("identity")
            if retain_text and sys.platform == "linux" and type(folder) is Folder and selected.held is not None:
                report = _compare_held(selected, folder, output, observer)
            elif sys.platform == "darwin":
                from . import macos_read
                with macos_read.no_materialization():  # Includes the archive's final named recheck.
                    report = _compare(selected, folder, macos_read, observer=observer)
            else: report = _compare(selected, folder, observer=observer)
        if observer is not None: observer.check()
        output.pop_all()
        return report


def _compare_held(selected, folder, output, observer):
    # Only the extra row proof needs this native Linux capability. Other
    # folders keep the ordinary comparison without a text-reading grant.
    with ExitStack() as scan:
        original = scan.enter_context(selected.held.read_lease())
        root = folder.check()
        try:
            proc = ProcView(scan)
            mount = local_mount(root, read_proc=proc.read)
            _check_root(folder.evidence, root, mount, proc)
        except (OSError, Rejected):
            proc = None
        archive_mount = _check_archive(selected, original, proc)
        candidates = {}

        def remember(path, child, before, sha):
            if before.st_size > COMPARE_BYTES or before.st_nlink != 1: return
            parent, _, name = path.rpartition("/")
            entry = selected.browser.entry(parent, name)
            if (entry is None or entry.kind != "file" or entry.size_bytes > COMPARE_BYTES or
                    entry.sha256 == sha): return
            info = os.fstat(child)
            if (_sig(info) == _sig(before) and info.st_nlink == 1 and
                    mount_id(child, read_proc=proc.read) == mount):
                candidates[path] = _sig(before), entry.sha256, sha

        tree = observe_held(root, folder.evidence.identity, require_ids=True, observer=observer,
                            on_file=remember if proc is not None else None)
        if observer is not None: observer.begin("compare")
        delta = _changes(_rows(selected.browser), tree.rows)
        if observer is not None: observer.begin("identity")
        folder.check()
        _check_archive(selected, original, proc, archive_mount)
        if proc is not None: _check_root(folder.evidence, root, mount, proc)
        text = None
        if candidates:
            pairs = {row.path: (row, *candidates[row.path]) for row in delta[3] if row.path in candidates}
            text = _FolderText(selected, folder.evidence, root, mount, archive_mount, pairs)
            output.callback(text.close)
        return Report(*delta, folder.evidence, selected.browser.commit.operation, text)


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
