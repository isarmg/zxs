"""Private data plane for one selected remote Mac file. No provider, mount, publication or send."""
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
import errno
import hashlib
import os
from pathlib import Path
import stat
import sys
from threading import Lock

from . import macos_read as mac
from ._macos_copy_evidence import _attributes, apfs
from .container_https import HttpsExchange
from .container_plan import FullBody
from .desktop.container_client import ContainerClient
from .desktop.container_connection import OpenConnection, _Metadata
from .desktop.container_management import Branch, Container, Detail, Node, _node, id_ok
from .document import archive_observation
from .model import CHUNK, MAX_FILE, Rejected, new_id

_MAX_IO = 16 << 20


@dataclass(frozen=True, slots=True)
class SelectedFile:
    destination: str
    account: str
    container: Container  # The actual server spelling; never used to create a local name.
    branch: str
    head: str
    node: Node


@dataclass(frozen=True, slots=True)
class SealedFile:
    """Process-local evidence only; not a durable submission or remote receipt."""
    selected: SelectedFile
    body: FullBody


def _admit(connection, detail, branch, node):
    if (type(connection) is not OpenConnection or not connection._listed
            or type(connection.operations) is not ContainerClient
            or type(connection._exchange) is not HttpsExchange
            or type(detail) is not Detail or type(detail.container) is not Container
            or type(branch) is not Branch or branch not in detail.branches
            or not _node(node) or node not in branch.nodes or not id_ok(branch.head)
            or node.length > MAX_FILE or (node.id != branch.head and branch.id != "main")):
        raise Rejected("need a selected current branch head or exact historical main node")
    exchange, authority = connection._exchange, connection._authorization
    source = SelectedFile(exchange.destination_identity, connection._account, detail.container,
                          branch.id, branch.head, node)
    location = exchange._location
    wire = _Metadata(exchange)  # Real read-only R3 reader, never an injected local metadata view.

    def original():
        if (connection.operations._exchange is not exchange or exchange._authorization is not authority
                or exchange.destination_identity != source.destination or exchange._location != location):
            raise Rejected("the original authenticated connection changed")
        authority.require_live()

    def selection():
        # A detached selection with the same visible name is not authority.
        original()
        cid, revision = source.container.id, source.container.revision
        first = wire._read_detail(cid, revision)
        if first[0] != source.container:
            raise Rejected("the selected remote container changed")
        # R3 requires the exact branch/head pair at every page and walks its
        # linked ancestry. Never load the histories of unrelated branches.
        history = wire._branch_history(cid, source.branch, source.head, revision)
        if source.node not in history.nodes or wire._read_detail(cid, revision) != first:
            raise Rejected("the selected remote container, branch or node changed")
        original()

    return source, original, selection


def _open_pair(private_parent):
    """The source body never visits a name. A setup failure can leave only an empty stage."""
    import fcntl

    spelling = os.fspath(private_parent)
    if type(spelling) is not str:
        raise Rejected("select an exact private local APFS directory")
    path = Path(spelling)
    if str(path) != spelling or path.anchor != "/" or ".." in path.parts:
        raise Rejected("select an exact private local APFS directory")
    with ExitStack() as owners:
        def open_at(parent, leaf, flags, mode):
            fd = os.open(leaf, flags | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, 0o600, dir_fd=parent)
            try:
                file = os.fdopen(fd, mode, buffering=0)
            except BaseException:
                os.close(fd)
                raise
            return owners.enter_context(file)

        with mac.no_materialization(), mac.literal_name(path, directory=True) as parent:
            _attributes(parent, directory=True)
            volume = apfs(parent, fresh_snapshot=True)
            leaf = ".zipworkspace-edit-" + new_id()
            writer = open_at(parent, leaf, os.O_RDWR | os.O_CREAT | os.O_EXCL, "r+b")
            _attributes(writer.fileno())
            reader = open_at(parent, leaf, os.O_RDONLY, "rb")  # dup(writer) would still be writable.
            for file, access in ((writer, os.O_RDWR), (reader, os.O_RDONLY)):
                fd = file.fileno()
                _attributes(fd)
                if (fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_ACCMODE != access or os.get_inheritable(fd)
                        or apfs(fd, fresh_snapshot=True) != volume or os.fstat(fd).st_size != 0):
                    raise Rejected("the private APFS handles are not the original empty file")
            named = os.stat(leaf, dir_fd=parent, follow_symlinks=False)
            if (named.st_nlink != 1 or not stat.S_ISREG(named.st_mode)
                    or not all(os.path.samestat(named, os.fstat(f.fileno())) for f in (reader, writer))):
                raise Rejected("private edit staging name changed")
            # Only the new, witnessed empty stage is unlinked, under an owner-only
            # quiescent parent. Never attempt name-based cleanup on failure.
            os.unlink(leaf, dir_fd=parent)
            os.fsync(parent)
            if any(os.fstat(f.fileno()).st_nlink != 0 for f in (reader, writer)):
                raise Rejected("the new APFS file still has a name")
        return owners.pop_all(), writer, reader


def _write_at(fd, offset, data):
    view = memoryview(data)
    while view:
        count = os.pwrite(fd, view, offset)
        if not 0 < count <= len(view):
            raise OSError(errno.EIO, "short local write")
        offset += count
        view = view[count:]


def _read_at(fd, offset, wanted):
    parts = bytearray()
    while len(parts) < wanted:
        part = os.pread(fd, wanted - len(parts), offset + len(parts))
        if not part:
            raise OSError(errno.EIO, "premature end of the private edit file")
        parts.extend(part)
    return bytes(parts)


def _generation(file):
    info = os.fstat(file.fileno())
    if info.st_nlink or info.st_flags or info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o600:
        raise Rejected("private APFS body no longer has its original access and anonymous identity")
    return archive_observation(info)


def _whole_file(file, original):
    initial = _generation(file)
    size = initial[2]
    if not 0 <= size <= MAX_FILE:
        raise Rejected("private body exceeds the local edit limit")
    digest = hashlib.sha256()
    for offset in range(0, size, CHUNK):
        original()
        digest.update(_read_at(file.fileno(), offset, min(CHUNK, size - offset)))
    if os.pread(file.fileno(), 1, size) or _generation(file) != initial:
        raise Rejected("private body changed during full verification")
    original()
    return FullBody(size, digest.hexdigest()), initial


class _EditFile:
    """Opaque handles under one lock. No raw or mappable file descriptor escapes."""
    def __init__(self, selected, original, writer, reader, generation):
        self._source, self._original = selected, original
        self._writer, self._reader, self._generation = writer, reader, generation
        self._lock, self._handles, self._sealed = Lock(), {}, None
        self._phase = "editing"

    def _check_file(self):
        self._original()
        if _generation(self._reader) != self._generation:
            self._phase = "failed"
            raise Rejected("the held edit file changed outside its admitted operations")

    def open_handle(self, *, writable=False):
        with self._lock:
            if self._phase != "editing" or type(writable) is not bool:
                raise Rejected("file no longer accepts new handles")
            self._check_file()
            if writable and any(self._handles.values()):
                raise OSError(errno.EBUSY, "the selected file already has a writer")
            if len(self._handles) >= 1024:
                raise OSError(errno.EMFILE, "too many open handles for one file")
            handle = object()
            self._handles[handle] = writable
            return handle

    def release(self, handle):
        with self._lock:
            if handle not in self._handles:
                raise OSError(errno.EBADF, "not an open handle of this file")
            del self._handles[handle]  # Neither closing the last writer nor flushing seals or sends.

    def _handle(self, handle, *, writing=False):
        if self._phase not in ("editing", "draining") or handle not in self._handles:
            raise OSError(errno.EBADF, "not a live handle of this edit")
        if writing and not self._handles[handle]:
            raise OSError(errno.EBADF, "a writer is required")
        self._check_file()

    def read(self, handle, offset, size):
        with self._lock:
            self._handle(handle)
            if type(offset) is not int or not 0 <= offset < 1 << 63 or type(size) is not int or not 0 <= size <= _MAX_IO:
                raise OSError(errno.EINVAL, "invalid private edit read")
            data = _read_at(self._reader.fileno(), offset, min(size, max(0, self._generation[2] - offset)))
            self._check_file()
            return data

    def _mutation(self, handle, operation):
        with self._lock:
            self._handle(handle, writing=True)
            try:
                operation(self._writer.fileno())
                updated = _generation(self._reader)
                if updated[:2] != self._generation[:2]:
                    raise Rejected("private edit changed its original inode")
                self._generation = updated
                self._original()
            except BaseException:
                self._phase = "failed"  # Includes partial pwrite, ftruncate, fsync and lost authority.
                raise

    def write(self, handle, offset, data):
        if (type(data) is not bytes or len(data) > _MAX_IO or type(offset) is not int
                or not 0 <= offset <= MAX_FILE - len(data)):
            raise OSError(errno.EINVAL, "invalid private edit write")
        self._mutation(handle, lambda fd: _write_at(fd, offset, data))
        return len(data)

    def truncate(self, handle, size):
        if type(size) is not int or not 0 <= size <= MAX_FILE:
            raise OSError(errno.EINVAL, "invalid private edit length")
        self._mutation(handle, lambda fd: os.ftruncate(fd, size))

    def flush(self, handle):
        self._mutation(handle, os.fsync)

    def stop_new_handles(self):
        with self._lock:
            if self._phase != "editing":
                raise Rejected("edit cannot start draining")
            self._check_file()
            self._phase = "draining"  # Existing admitted requests may finish and release.

    def seal_for_submit(self, *, confirmed=False):
        with self._lock:
            if confirmed is not True or self._phase != "draining" or self._handles:
                raise Rejected("explicit sealing requires this stopped file and all original handles drained")
            self._check_file()
            self._phase = "failed"  # A failed barrier/close/readback cannot be turned into success by retry.
            os.fsync(self._writer.fileno())
            self._writer.close()
            body, generation = _whole_file(self._reader, self._original)
            if generation != self._generation:
                raise Rejected("the sealed file is not the original edit generation")
            self._sealed = SealedFile(self._source, body)
            self._phase = "sealed"
            return self._sealed

    def blocks(self, seal):
        """Lend complete contents of this exact process-local seal, never its FD."""
        offset = 0
        while True:
            with self._lock:
                if self._phase != "sealed" or self._sealed is not seal:
                    raise Rejected("not this original live seal")
                self._check_file()
                left = seal.body.length - offset
                if not left:
                    return
                block = _read_at(self._reader.fileno(), offset, min(CHUNK, left))
                self._check_file()
                offset += len(block)
            yield block

    def _retire(self):
        with self._lock:
            self._phase = "closed"
            self._handles.clear()


@contextmanager
def selected_edit_file(connection, detail, branch, node, private_parent, *, confirmed=False):
    """One explicit authenticated read into anonymous APFS storage, not a desktop action."""
    if sys.platform != "darwin" or confirmed is not True:
        raise Rejected("a confirmed private macOS edit of one original node is required")
    source, original, verify_selection = _admit(connection, detail, branch, node)
    original()
    # Bind the local choice before any potentially long metadata/network work.
    owners, writer, reader = mac.in_disposable_thread(_open_pair, private_parent)
    with owners:
        verify_selection()
        received = 0

        def fill(data):
            nonlocal received
            original()
            if type(data) is not bytes or len(data) > node.length - received:
                raise Rejected("selected remote bytes exceeded the original size")
            _write_at(writer.fileno(), received, data)
            received += len(data)
            return len(data)

        original()
        connection._exchange.read_node_into(source.container.id, node.id, length=node.length,
                                            sha256=node.sha256, write=fill)
        os.fsync(writer.fileno())
        body, generation = _whole_file(reader, original)
        if received != node.length or body != FullBody(node.length, node.sha256):
            raise Rejected("private APFS bytes differ from the selected remote node")
        verify_selection()  # An old GET result must not authorize a different current choice.
        edit = _EditFile(source, original, writer, reader, generation)
        try:
            yield edit
        finally:
            edit._retire()
