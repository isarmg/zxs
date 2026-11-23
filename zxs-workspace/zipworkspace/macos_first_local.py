"""Explicit one-file first remote capture; preserve the local APFS source without mounting."""
from contextlib import ExitStack
from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import stat
from threading import Lock

from . import macos_read as mac
from ._macos_copy_evidence import apfs, native
from .container_plan import FullBody
from .desktop.container_connection import _catalog_name
from .desktop.container_management import Container
from .macos_retained_edit import FirstFile, _birth, _connection, _on_mac, _retain
from .model import CHUNK, MAX_FILE, Rejected, new_id
from .publish import Uncertain


@dataclass(frozen=True, slots=True)
class FirstLocalReview:
    selected: FirstFile
    origin: str
    path: str
    body: FullBody


def _stamp(fd):
    row = os.fstat(fd)
    if (not stat.S_ISREG(row.st_mode) or row.st_uid != os.getuid() or row.st_nlink != 1
            or row.st_flags or row.st_mode & 0o022 or not 0 <= row.st_size <= MAX_FILE):
        raise Rejected("choose one original unlinked-to-others local regular file owned by this account")
    return (_birth(fd), row.st_mode, row.st_uid, row.st_size, row.st_mtime_ns, row.st_ctime_ns)


class MacFirstLocalChoice:
    """Own the held input FD and original byte review until capture or close."""

    def __init__(self, connection, selected_path, remote_name):
        if connection.writable or not _catalog_name(remote_name):
            raise Rejected("select a read-authorized connection and an exact new remote filename")
        self._lock = Lock()
        self._fd = self._review = None
        self._owns = False
        self._used = False
        self._connection = connection
        self._path = Path(selected_path)
        try:
            self._review = _on_mac(self._open, remote_name)
        except BaseException:
            self.close()
            raise

    def _open(self, name):
        destination, account, authorize = _connection(self._connection)
        self._original = authorize
        with ExitStack() as keep, mac.literal_name(self._path.parent, directory=True) as parent:
            before = os.stat(self._path.name, dir_fd=parent, follow_symlinks=False)
            mac.resident_stat(self._path.name, before)
            if not stat.S_ISREG(before.st_mode):
                raise Rejected("select an original ordinary file, not a link or directory")
            fd = os.open(self._path.name, mac.named_flags(), dir_fd=parent)
            keep.callback(os.close, fd)
            if not os.path.samestat(before, os.fstat(fd)):
                raise Rejected("the selected file changed during the original open")
            self._fd = fd
            self._volume, self._stamp = apfs(fd, fresh_snapshot=True), _stamp(fd)
            self._check(metadata=True)
            digest = hashlib.sha256()
            for block in self._blocks():
                digest.update(block)
            body = FullBody(self._stamp[3], digest.hexdigest())
            review = FirstLocalReview(FirstFile(destination, account, Container(new_id(), name, 0)),
                                     self._connection._exchange._location, str(self._path), body)
            keep.pop_all()  # This instance owns the same fd between explicit decisions.
            self._owns = True
            return review

    @property
    def review(self):
        return self._review

    def _check(self, *, metadata=False):
        self._original()
        fd = self._fd
        mac.check_literal_path(self._path, fd)
        mac.check_spelling(fd, self._path.name)
        if _stamp(fd) != self._stamp or apfs(fd, fresh_snapshot=True) != self._volume:
            raise Rejected("the original local APFS file changed")
        if metadata:
            native().no_payload_metadata(self._path, fd, literal=True)

    def _blocks(self):
        self._check(metadata=True)
        offset, size = 0, self._stamp[3]
        while offset < size:
            self._check()
            block = os.pread(self._fd, min(CHUNK, size-offset), offset)
            if not block:
                raise Rejected("the original local file was not fully readable")
            offset += len(block)
            yield block
        if os.pread(self._fd, 1, size):
            raise Rejected("the original local file length changed")
        self._check(metadata=True)

    def _claim_retention(self, review):
        if self._fd is None or review is not self._review or self._used:
            raise Rejected("not the exact unconsumed first selected file")
        self._check(metadata=True)
        self._used = True

    def blocks(self, review):
        if not self._used or review is not self._review:
            raise Rejected("cannot copy an unclaimed first local file")
        return self._blocks()

    def capture(self, review, state, *, confirmed=False):
        if confirmed is not True:
            raise Rejected("explicitly confirm the complete first file to retain")
        with self._lock:
            attempted = []
            try:
                return _on_mac(_retain, self, review, state, attempted)
            except BaseException as exc:
                if attempted:
                    raise Uncertain("first-file owner may exist; reopen only the same container identifier") from exc
                raise

    def close(self):
        with self._lock:
            if self._owns:
                fd, self._fd, self._owns = self._fd, None, False
                os.close(fd)
