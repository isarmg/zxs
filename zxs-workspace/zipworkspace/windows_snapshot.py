"""One read-only WinFsp data source. No driver loading, drive creation or Windows mount."""
from bisect import bisect_right
from contextlib import contextmanager, ExitStack
import errno
import os
from pathlib import Path
from threading import RLock

from .archive import stamp
from .document import archive_sha
from .model import MAX_DEPTH, Rejected, member_name
from .snapshot_view import ROOT, SnapshotView


def _component(name):
    # A normal Win32 DOS spelling may resolve a device if spaces precede the first dot.
    # Decline the uncertain spelling here; do not silently rename a virtual archive entry.
    stem, dot, _ = name.partition(".")
    if len(member_name(name)) != 1 or dot and stem.endswith(" "):
        raise ValueError("not an exact ordinary Windows component")
    return name.encode("utf-8")


class _WindowsView(SnapshotView):
    """Called only while Volume owns the exclusive cursor of this unbuffered archive."""
    def _read_at(self, length, offset):
        self.document.seek(offset)
        return self.document.read(length)


class _Volume:
    def __init__(self, view, observe):
        self.view, self.observe, self.lock = view, observe, RLock()
        self.closed = False
        # Refuse the whole namespace rather than returning two spellings for one member.
        for node in view.nodes.values():
            if node.children is not None:
                for name, _ in node.children: _component(name.decode("utf-8"))

    def _check(self):
        if self.closed: raise OSError(errno.EIO, "the original Windows provider is closed")
        self.view.check()
        try: self.observe()
        except (OSError, Rejected) as err:
            self.view.failed = True
            raise OSError(errno.EIO, "the held Windows archive is no longer the admitted object") from err

    def check(self):
        with self.lock: self._check()

    def close(self):
        # Wait for any actual seek+read to finish before the owner closes the CRT descriptor.
        with self.lock: self.closed = True

    def _info(self, ino):
        node = self.view.node(ino)
        return ino, node.children is not None, node.size

    def info(self, ino):
        with self.lock:
            self._check()
            return self._info(ino)

    def lookup(self, path):
        with self.lock:
            self._check()
            if path == "\\": return self._info(ROOT)
            if not path.startswith("\\") or len(path) > 1025:
                raise ValueError("not a bounded, volume-relative WinFsp name")
            parts = path[1:].split("\\")
            if len(parts) > MAX_DEPTH: raise ValueError("invalid virtual path depth")
            ino = ROOT
            for part in parts: ino = self.view.lookup(ino, _component(part))
            return self._info(ino)

    def directory(self, ino, marker):
        """Return a lazy cursor into this immutable, bounded tree, not a whole directory copy."""
        with self.lock:
            self._check()
            node = self.view.node(ino)
            children = node.children
            if children is None: raise NotADirectoryError(errno.ENOTDIR, "not a directory")
            dots = ((".", ino), ("..", node.parent)) if ino != ROOT else ()
            if marker is None: position = 0
            elif dots and marker == ".": dots, position = dots[1:], 0
            elif dots and marker == "..": dots, position = (), 0
            else:
                dots = ()
                # Order after the two special entries is literal UTF-8 byte order, not locale.
                if not isinstance(marker, str) or len(marker) > 255:
                    raise ValueError("invalid WinFsp directory marker")
                needle = marker.encode("utf-8")  # May be a nonmember; never interpreted as a path.
                position = bisect_right(children, needle, key=lambda row: row[0])

        def rows():
            for name, child in dots: yield name, self._info(child)
            for i in range(position, len(children)):
                name, child = children[i]
                yield name.decode("utf-8"), self._info(child)
        return rows()  # No OS handles in this iterator; the SDK adapter rechecks before reporting a page.

    def read(self, ino, offset, length):
        # Only this provider uses the dedicated file cursor after full admission. An entire
        # native request is serialized, not just each seek. The actual hashed blocks supply bytes.
        with self.lock:
            self._check()
            data = self.view.read(ino, offset, length)
            self._check()
            if not data and length and offset >= self.view.node(ino).size: raise EOFError()
            return data

    def volume_size(self):
        with self.lock:
            self._check()
            return self.view.total


@contextmanager
def selected_snapshot(path, *, expected=None):
    """Explicit *new selection*. An optional listed Node fixes both the native stamp and ZIP SHA.
    Must remain entered until all native callbacks have drained. Never a path-based resume.
    """
    if os.name != "nt": raise Rejected("this source requires fixed local Windows NTFS")
    import msvcrt
    from ._windows_output import PinnedParent

    path = Path(path)
    if not path.is_absolute() or path.suffix.lower() != ".zxs" or expected is not None and str(expected.path) != str(path):
        raise Rejected("select the exact absolute ordinary .zxs path")
    with ExitStack() as stack:
        parent = stack.enter_context(PinnedParent(path.parent))
        own = parent.api.input_file(parent.handle, path.name)  # existing, read only; denies data-write/delete sharing
        h = stack.enter_context(own)
        name = parent.name(path.name)
        original = parent.api.ordinary(h, parent.volume, name)
        fd = msvcrt.open_osfhandle(h, os.O_RDONLY)
        own.value = None  # CRT owns that same HANDLE from this point.
        stack.callback(os.close, fd)
        msvcrt.setmode(fd, os.O_BINARY)
        doc = stack.enter_context(os.fdopen(fd, "rb", buffering=0, closefd=False))
        first = stamp(doc)
        if expected is not None and first != expected.observed:
            raise Rejected("the originally listed slot instance changed")

        def observe():
            if parent.api.ordinary(h, parent.volume, name) != original:
                raise Rejected("the held NTFS file changed")

        view = _WindowsView(doc, display_name=path.name)
        if expected is not None and (expected.marker != view.commit or archive_sha(doc) != expected.sha256):
            raise Rejected("not the originally listed snapshot")
        parent.check()
        observe()
        if stamp(doc) != first: raise Rejected("archive changed during admission")
        volume = _Volume(view, observe)
        stack.callback(volume.close)
        yield volume
