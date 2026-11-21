"""Read only a resident original Darwin folder and one exact pair of files from its report."""
from contextlib import ExitStack, contextmanager
import os
from pathlib import Path
import stat
from threading import Lock

from . import macos_read as mac
from .comparison import Folder, FolderEvidence, _check_held_archive, _small_changed_left
from .document import native_identity
from .freeze import _sig
from .model import Rejected, member_name


class MacComparisonFolder(Folder):
    """A literal original directory for the comparison UI, never resolved through a link."""
    @classmethod
    def choose(cls, path):
        return mac.in_disposable_thread(cls._choose, path)

    @classmethod
    def _choose(cls, path):
        path = Path(path).absolute()
        selected = None
        try:
            with mac.no_materialization(), mac.literal_name(path, directory=True) as named:
                identity = native_identity(os.fstat(named), stat.S_IFDIR)
                fd = os.dup(named)
                try: selected = cls(FolderEvidence(path, identity), fd, mac)
                except BaseException:
                    os.close(fd)
                    raise
            return selected
        except BaseException:
            if selected is not None: selected.close()
            raise

    def _check(self):
        return self.check_original(self._fd)

    def check_original(self, root, volume=None):
        """The caller holds the thread policy and owns this actual file descriptor."""
        path = self.evidence.path
        if root is None: raise Rejected("原文件夹句柄已经关闭")
        mac.local_object(path, root)
        mac.check_literal_path(path, root)
        with mac.literal_name(path, directory=True) as named:
            for fd in (root, named):
                if (native_identity(os.fstat(fd), stat.S_IFDIR) != self.evidence.identity or
                        volume is not None and mac.apfs_volume(fd)[0] != volume):
                    raise Rejected("本次原文件夹或实际 APFS 卷已改变；未选用替件")
        return root


def check_archive(selected, original, volume=None):
    """The real left descriptor stays open; the exact visible name is verified without reading data."""
    _check_held_archive(selected, original)
    mac.local_object(selected.path, original)
    mac.check_literal_path(selected.path, original)
    with mac.literal_name(selected.path, directory=False) as named:
        for fd in (original, named):
            _check_held_archive(selected, fd)
            if volume is not None and (os.fstat(fd).st_nlink != 1 or mac.apfs_volume(fd)[0] != volume):
                raise Rejected("原归档不是本次本机 APFS 单链接文件")


class MacFolderText:
    _held = None

    def __init__(self, selected, folder, root, original):
        self.selected, self.folder = selected, folder
        self.archive_volume = mac.apfs_volume(original)[0]
        self.folder_volume = mac.apfs_volume(root)[0]
        self._lock, self._pairs = Lock(), {}
        if os.fstat(original).st_nlink != 1: raise Rejected("原归档不是单链接文件")

    def _check_file(self, fd, path, expected):
        m = os.fstat(fd)
        if (not stat.S_ISREG(m.st_mode) or m.st_nlink != 1 or _sig(m) != expected or
                mac.apfs_volume(fd)[0] != self.folder_volume):
            raise Rejected("原本机文件的身份、代次或实际 APFS 卷已改变；请刷新对照")
        mac.check_literal_path(path, fd)
        mac.check_spelling(fd, path.name)

    def remember(self, path, item, before, sha):
        previous = _small_changed_left(self.selected, path, before.st_size, sha)
        if previous is None or before.st_nlink != 1: return
        try: self._check_file(item, self.folder.evidence.path.joinpath(*member_name(path)), _sig(before))
        except (OSError, Rejected): return  # No extra text authority for an unproved row.
        self._pairs[path] = _sig(before), previous, sha

    def bind(self, rows, root, original):
        check_archive(self.selected, original, self.archive_volume)
        self.folder.check_original(root, self.folder_volume)
        self._pairs = {row.path: (row, *self._pairs[row.path]) for row in rows if row.path in self._pairs}
        if not self._pairs: return False
        self._held = os.dup(root)
        return True

    def _pair(self, selected, row):
        if self._held is None or self.selected is not selected or row is None: return None
        pair = self._pairs.get(row.path)
        return pair if pair is not None and pair[0] is row else None

    def supports(self, selected, row):
        with self._lock: return self._pair(selected, row) is not None

    def close(self):
        if self._held is None: return
        with self._lock:
            root, self._held = self._held, None
            self._pairs.clear()
        if root is not None: os.close(root)

    __del__ = close

    def _parent(self, root, parts, scope, check):
        parent = root
        prefix = self.folder.evidence.path
        for part in parts:
            check()
            prefix = prefix / part
            parent = os.open(part, mac.named_flags(directory=True), dir_fd=parent)
            scope.callback(os.close, parent)
            if mac.apfs_volume(parent)[0] != self.folder_volume:
                raise Rejected("原文件父目录不在本次已核验的 APFS 卷")
            native_identity(os.fstat(parent), stat.S_IFDIR)
            mac.check_literal_path(prefix, parent)
        return parent

    def _file(self, parent, path, expected, scope, *, metadata):
        found = os.stat(path.name, dir_fd=parent, follow_symlinks=False)
        if not stat.S_ISREG(found.st_mode) or found.st_nlink != 1 or _sig(found) != expected:
            raise Rejected("本机文件与已核验的原相对名和身份不符")
        mac.resident_stat(path.name, found)
        fd = os.open(path.name, mac.named_flags(metadata=metadata), dir_fd=parent)
        scope.callback(os.close, fd)
        self._check_file(fd, path, expected)
        return fd

    @contextmanager
    def open_pair(self, selected, row, check):
        check()
        # UI opt-out does not travel to the reading thread. Borrow, check,
        # read, recheck and close all this worker's FDs under its own opt-out.
        with mac.no_materialization(), ExitStack() as held:
            with self._lock:
                pair = self._pair(selected, row)
                if pair is None: raise Rejected("不属于本次文件夹对照的原差异行")
                root = os.dup(self._held)
                held.callback(os.close, root)
            _, expected, left_sha, right_sha = pair
            parts = member_name(row.path)
            path = self.folder.evidence.path.joinpath(*parts)
            if expected[3] != row.after_bytes: raise Rejected("原文件与报告中的大小不符")
            original = held.enter_context(selected.held.read_lease())

            def originals():
                check()
                check_archive(selected, original, self.archive_volume)
                self.folder.check_original(root, self.folder_volume)
                check()

            originals()
            parent = self._parent(root, parts[:-1], held, check)
            witness = self._file(parent, path, expected, held, metadata=True)
            check()
            body = self._file(parent, path, expected, held, metadata=False)
            check()
            yield original, lambda n: os.read(body, n), left_sha, right_sha
            originals()
            self._check_file(witness, path, expected)
            self._check_file(body, path, expected)
            with ExitStack() as names:
                current = self._parent(root, parts[:-1], names, check)
                self._file(current, path, expected, names, metadata=True)
            check()
