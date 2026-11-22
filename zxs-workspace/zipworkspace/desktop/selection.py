"""A verified display is not authority to write or a claim about raw ZIP bytes."""
from bisect import bisect_left
from contextlib import ExitStack, contextmanager, nullcontext
from dataclasses import dataclass, field, replace
import os
from pathlib import Path
import stat
import sys
from threading import Event, Lock
from types import MappingProxyType

from .. import plain_import

from ..archive import stamp
from ..browser import ArchiveBrowser, DirectoryIndex
from ..comparison import Changes, Difference
from ..document import (PositionalDocument, archive_observation, archive_sha, named_archive_observation,
                        named_document, native_identity, relative_document)
from ..model import Rejected
from ..refs import Catalog, Node, SlotPreview, slot_previews


class Verification:
    """One read-only verification; visible counts are for the actual current pass."""
    PHASE = {"catalog": "正在核对存储目录与历史关系",
             "layout": "正在检查 ZIP 布局和目录",
             "members": "正在读取并核验成员正文",
             "raw": "正在核对原 ZIP 字节摘要",
             "identity": "正在核对原件与目录",
             "folder": "正在读取实际文件夹的普通文件正文",
             "inventory": "正在复核原目录及各成员的第二遍清单",
             "compare": "正在比较两份已核验的目录内容"}

    def __init__(self):
        self._lock, self._stop = Lock(), Event()
        self._progress = (self.PHASE["layout"], 0, None)
        self._archive = None

    def cancel(self):
        self._stop.set()
        return True

    @property
    def cancelled(self): return self._stop.is_set()

    def check(self):
        if self.cancelled: raise Rejected("已取消本次只读核验")

    def begin(self, phase, total=None):
        self.check()
        with self._lock:
            label = self.PHASE[phase]
            if self._archive is not None:
                position, count = self._archive
                label = f"存储归档 {position}/{count} · {label}"
            self._progress = (label, 0, total)

    def in_archive(self, index, count):
        self.check()
        with self._lock: self._archive = (index, count) if index else None

    def advance(self, size):
        self.check()
        with self._lock:
            phase, done, total = self._progress
            self._progress = phase, done + size, total

    def progress(self):
        with self._lock: return self._progress


def submit_verification(owner, title, select, accepted, *, current, discard=lambda view: view.release_display()):
    """Never publish a partial display, even if cancel arrives after the read finishes."""
    observer = Verification()

    def work():
        try:
            observer.check()
            return select(observer), None
        except Exception as err: return None, err

    def done(outcome):
        selection, failure = outcome
        active = current()
        if observer.cancelled or not active:
            if selection is not None: discard(selection)
            if active and observer.cancelled: owner.inform("已取消核验；未显示本次结果，没有写入或提取。")
            return
        if failure is not None:
            owner.inform(str(failure))
        else:
            accepted(selection)

    def progress():
        return observer.progress() if current() else ("", 0, None)

    return owner.job(title, work, done, cancel=observer.cancel, cancel_text="取消本次只读核验",
                     progress_report=progress)


class _DisplayFD:
    """Sole owner of one display duplicate; not a buffered body reader."""
    _fd = -1

    def __init__(self, doc, *, seek_lock=None):
        self._lock, self._seek_lock = Lock(), seek_lock if seek_lock is not None else Lock()
        self._fd = os.dup(doc.fileno())
    @property
    def closed(self): return self._fd < 0
    def fileno(self):
        if self.closed: raise ValueError("the display descriptor is closed")
        return self._fd
    def duplicate(self):
        with self._lock: return os.dup(self.fileno())
    def duplicate_display(self):
        # Each view can close independently, but CRT duplicates share their
        # physical seek position; never allocate a second lock for that cursor.
        with self._lock: return _DisplayFD(self, seek_lock=self._seek_lock)
    @contextmanager
    def read_lease(self):
        # On Windows the CRT duplicate shares seek state with this original.
        # Preview readers are serialized; other desktop tasks on Windows use
        # their own independently opened archival streams. POSIX uses pread.
        with (nullcontext() if hasattr(os, "pread") else self._seek_lock):
            fd = self.duplicate()
            try:
                position = None if hasattr(os, "pread") else os.lseek(fd, 0, os.SEEK_CUR)
                try: yield fd
                finally:
                    if position is not None: os.lseek(fd, position, os.SEEK_SET)
            finally: os.close(fd)

    def close(self):
        if self.closed: return  # also safe after a failed construction
        with self._lock:
            if self.closed: return
            fd, self._fd = self._fd, -1
        os.close(fd)
    __del__ = close  # Fallback for an undelivered result; normal ownership is explicit.


@dataclass(frozen=True, slots=True)
class PlainSelection:
    """Verified ordinary ZIP display with a borrowable read-only original FD."""
    preview: plain_import.Preview
    observed: tuple[int, int, int, int, int]
    rows: MappingProxyType
    index: DirectoryIndex
    held: _DisplayFD | None

    @property
    def path(self): return self.preview.source

    def release_display(self):
        if self.held is not None: self.held.close()

    @classmethod
    def open(cls, path, *, retain=True, observer=None):
        path = Path(path).absolute()
        if path.suffix.lower() != ".zip": raise Rejected("只能选择普通 .zip 文件")
        with ExitStack() as guard:
            with named_document(path) as doc:
                preview, manifest, observed = plain_import._inspect_document(doc, path, observer=observer)
                index = DirectoryIndex(manifest.rows)
                if observer is not None: observer.check()
                held = _DisplayFD(doc) if retain else None
                if held is not None: guard.callback(held.close)
            choice = cls(preview, observed, MappingProxyType(manifest.rows), index, held)
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

    def previews(self, *, observer):
        """Observe names only; never open one of their archives to choose a repo."""
        observer.begin("catalog")
        with ExitStack() as stack:
            stack.enter_context(_without_download())
            self.check()
            held = None
            if os.scandir in os.supports_fd and hasattr(os, "O_DIRECTORY"):
                flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
                held = os.open(self.path, flags)
                stack.callback(os.close, held)
                if native_identity(os.fstat(held), stat.S_IFDIR) != self.identity:
                    raise Rejected("所选存储目录已改变，未读取另一处")
                def read():
                    # Separate directory stream position for each bounded pass,
                    # opened underneath the same original directory FD.
                    cursor = os.open(".", flags, dir_fd=held)
                    try: return slot_previews(cursor, check=observer.check)
                    finally: os.close(cursor)
            else:
                def read(): return slot_previews(self.path, check=observer.check)
            before = read()
            self.check()
            after = read()
            self.check()
            if before != after: raise Rejected("存储目录核对期间变化；不显示部分列表")
        observer.check()
        return before


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
    def open(cls, path, *, expected: Node | None = None, slot: SlotPreview | None = None, retain=False, observer=None):
        path = Path(path).absolute()
        if expected is not None and expected.path != path:
            raise Rejected("selected original belongs to a different archive path")
        if slot is not None and (expected is not None or path.name != slot.leaf):
            raise Rejected("不是所选目录记录的准确归档名称")
        with ExitStack() as guard:
            with named_document(path) as doc:
                original = stamp(doc)
                if expected is not None and original != expected.observed:
                    raise Rejected("the listed archive instance changed; choose or inspect it again")
                if slot is not None and original != slot.observed:
                    raise Rejected("存储槽与本次看到的原件不同；未读取替件正文")
                browser = ArchiveBrowser(doc, display_name=path.name, observer=observer)
                if slot is not None and browser.commit.filename != slot.leaf:
                    raise Rejected("规范槽名与实际 ZIP 中的快照标记不符")
                options = {} if observer is None else {"observer": observer}
                if expected is not None and (browser.commit != expected.marker or archive_sha(doc, **options) != expected.sha256):
                    raise Rejected("the selected original archive changed; select it again")
                if observer is not None: observer.begin("identity")
                # Only an explicit consumer asks to retain this same open object for handoff.
                held = None
                if retain:
                    held = _DisplayFD(doc)
                    guard.callback(held.close)
            choice = cls(path, original, browser, expected, held)
            guard.pop_all()
            return choice

    def catalog(self, store: StoreSelection, *, observer=None):
        # Catalog holds and rechecks the selected anchor through both inventories;
        # a loose display requires its exact original generation, never a bare path.
        result = Catalog(self.path, store.path, expected_store=store.identity,
                         expected_anchor=self.observed, observer=observer)
        if (result.anchor.marker != self.browser.commit or
                (self.exact is not None and result.anchor != self.exact)):
            raise Rejected("selected original changed before catalog inspection")
        return result


def _without_download():
    if sys.platform == "darwin":
        from ..macos_read import no_materialization
        return no_materialization()
    return nullcontext()


def open_store_preview(store: StoreSelection, slot: SlotPreview, *, observer):
    """Read exactly the clicked candidate; inventory alone never authorizes a node."""
    with ExitStack() as guard:
        with _without_download():
            observer.check()
            store.check()
            selected = Selection.open(store.path / slot.leaf, slot=slot, retain=True, observer=observer)
            guard.callback(selected.release_display)
            store.check()
        observer.check()
        guard.pop_all()
        return selected


@contextmanager
def _display_policy(selected, observer):
    if not isinstance(selected, Selection) or selected.held is None or selected.held.closed:
        raise Rejected("没有可核验的原浏览句柄")
    observer.begin("identity")
    resident = None
    policy = nullcontext()
    if sys.platform == "darwin":
        from ..macos_read import no_materialization, resident
        policy = no_materialization()

    def current(fd):
        observer.check()
        if resident is not None: resident(selected.path.name, fd)
        if (archive_observation(os.fstat(fd)) != selected.observed or
                named_archive_observation(selected.path) != selected.observed):
            raise Rejected("原归档已变化，不按同名路径重开")

    with policy: yield current
    observer.check()


def selected_raw_sha(selected: Selection, *, observer):
    """Re-read only the physical bytes of the held observation, not a path or catalog slot."""
    with _display_policy(selected, observer) as current, selected.held.read_lease() as fd:
        current(fd)
        source = (nullcontext(PositionalDocument(fd, selected.observed[2])) if hasattr(os, "pread")
                  else os.fdopen(fd, "rb", buffering=0, closefd=False))
        with source as original:
            sha = archive_sha(original, observer=observer)
        observer.begin("identity")
        current(fd)
        if selected.exact is not None and sha != selected.exact.sha256:
            raise Rejected("整包 SHA-256 与已列原版本不符")
    return sha


def duplicate_selected_view(selected: Selection, *, observer):
    """Offer the exact completed browser observation to a new, independent window."""
    with ExitStack() as release:
        with _display_policy(selected, observer) as current:
            copied = selected.held.duplicate_display()
            release.callback(copied.close)
            current(copied.fileno())
            result = replace(selected, held=copied)
            current(copied.fileno())
        release.pop_all()
        return result


def read_registered_baseline(live, displayed, *, observer):
    """Verify the original STORE slot of an already-held idle worktree, without adopting STATE."""
    from ..lifecycle import _base_leaf, _idle_live_base
    from ..private_stage import mount_id

    observer.begin("identity")
    expected = _idle_live_base(live)
    if expected[:4] != displayed: raise Rejected("登记基线已与界面不同；请先刷新原状态，不会改选新版本")
    name = _base_leaf(expected)
    official = live.store / name
    original_mount = mount_id(live.store_fd)
    observer.check()
    with ExitStack() as handoff:
        # No SQLite transaction, directory enumeration, work content read, or HEAD selection.
        with relative_document(live.store_fd, name) as doc:
            actual = stamp(doc)
            if mount_id(doc.fileno()) != original_mount: raise Rejected("登记基线不在原 STORE 的挂载")
            browser = ArchiveBrowser(doc, display_name=name, observer=observer)
            marker = browser.commit
            if ((marker.repo, marker.branch, marker.generation, marker.operation) != displayed or
                    archive_sha(doc, observer=observer) != expected[4]):
                raise Rejected("STORE 的登记基线节点或整包 SHA-256 不符")
            observer.begin("identity")
            if _idle_live_base(live) != expected:
                raise Rejected("核验期间原登记基线变化；未浏览另一个版本")
            still = os.open(name, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=live.store_fd)
            try:
                named = os.fstat(still)
                if ((*native_identity(named, stat.S_IFREG), named.st_size, named.st_mtime_ns, named.st_ctime_ns) != actual
                        or stamp(doc) != actual or mount_id(still) != original_mount):
                    raise Rejected("登记基线的原正式名称或挂载已改变")
                held = _DisplayFD(doc)
                handoff.callback(held.close)
            finally:
                os.close(still)
        observer.check()
        view = Selection(official, actual, browser, Node(marker, official, expected[4], actual), held)
        handoff.pop_all()
        return view


@dataclass(frozen=True, slots=True)
class ComparedArchives:
    right: Selection
    changes: Changes


@dataclass(frozen=True, slots=True)
class PlainComparison(Changes):
    """A delivered list tied to these two existing display owners, not a new loan."""
    left: Selection = field(repr=False, compare=False)
    plain: PlainSelection = field(repr=False, compare=False)

    def contains(self, row):
        if not isinstance(row, Difference): return False
        index = bisect_left(self.rows, row.path, key=lambda item: item.path)
        return index < len(self.rows) and self.rows[index] is row


def compare_version(left: Selection, store: StoreSelection, catalog: Catalog, node: Node, *, observer=None,
                    retain_right=False):
    """Compare the admitted left view to exactly one canonical catalog row, not to a later HEAD."""
    from ..comparison import compare_snapshots

    current = left.browser.commit
    if (str(catalog.store) != str(store.path) or catalog.nodes.get(node.marker.operation) is not node or
            str(node.path) != str(store.path / node.marker.filename) or node.marker.repo != current.repo or
            node.marker.operation == current.operation):
        raise Rejected("请选择本次存储中另一条有原始证明的规范版本")
    if observer is not None: observer.check()
    store.check()
    if observer is not None: observer.check()
    options = {} if observer is None else {"observer": observer}
    right = Selection.open(node.path, expected=node, retain=retain_right, **options)
    try:
        if observer is not None: observer.begin("compare")
        result = compare_snapshots(left.browser, right.browser)
        if observer is not None: observer.begin("identity")
        store.check()
        if observer is not None: observer.check()
        return ComparedArchives(right, result) if retain_right else result
    except BaseException:
        right.release_display()
        raise


def compare_snapshot_file(left: Selection, path, *, observer, retain_right=False):
    """The chosen counterpart may be any .zxs, not necessarily a node of left's STORE."""
    from ..comparison import compare_archive_contents

    observer.check()
    path = Path(path).absolute()
    if path.suffix.lower() != ".zxs": raise Rejected("请另选一份 .zxs 快照文件")
    right = Selection.open(path, observer=observer, retain=retain_right)
    try:
        observer.begin("compare")
        changes = compare_archive_contents(left.browser, right.browser)
        observer.check()
        return ComparedArchives(right, changes) if retain_right else (right.path, right.browser.commit, changes)
    except BaseException:
        right.release_display()
        raise
