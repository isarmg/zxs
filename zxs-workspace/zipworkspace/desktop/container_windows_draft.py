"""Dormant NTFS working body of one exact remote branch head. No WinFsp mount or Submit."""
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from hashlib import sha256
import sys
from threading import Event, Lock

from .._windows_access import OutputProtection
from .._windows_node import _check_origin, _effective_zone, _receive_node, _remote_mark, _same_file
from .._windows_output import PinnedParent
from ..container_https import HttpsExchange
from ..model import CHUNK, Rejected, new_id
from .container_client import ContainerClient
from .container_connection import OpenConnection, _Authorization, _LOGIN, _Metadata
from .container_management import Branch, Container, Detail, Node, _branch, _container, _node, open_checked

_LIMIT = (1 << 63) - 1


@dataclass(frozen=True, slots=True)
class DraftSource:
    destination: str
    origin: str
    account: str
    container: Container  # Exact wire spelling, ID and observed sequence; never a local leaf.
    branch: str
    head: Node


@dataclass(frozen=True, slots=True)
class DraftReview:
    """Observation of a held local file, NOT a durable intent, driver drain, or server receipt."""
    source: DraftSource
    local_file: str
    volume: int
    file_id: bytes
    length: int
    sha256: str

    @property
    def changed(self):
        return (self.length, self.sha256) != (self.source.head.length, self.source.head.sha256)


class WindowsDraftChoice:
    """Review an already listed connection, branch head and explicitly selected private parent.

    Construction opens the parent only. begin() claims its handles once; closing a choice
    during acquisition cancels the handoff without seizing the worker's handles. Once begun,
    the draft owns its own lifetime. Failures may leave the generated private file; no cleanup
    by name, user-visible export or cold adoption.
    """
    def __init__(self, connection, detail, branch, node, private_parent):
        if (sys.platform != "win32" or type(connection) is not OpenConnection or not connection._listed
                or connection.writable or type(connection.operations) is not ContainerClient
                or type(connection._exchange) is not HttpsExchange
                or type(connection._authorization) is not _Authorization
                or type(connection._account) is not str or _LOGIN.fullmatch(connection._account) is None
                or type(detail) is not Detail or not _container(detail.container)
                or not detail.container.revision or type(branch) is not Branch or not _branch(branch)
                or branch not in detail.branches or not _node(node)
                or node.id != branch.head or node not in branch.nodes or node.length > _LIMIT):
            raise Rejected("choose the original authenticated Windows reader and one exact branch head")
        self._connection, self._reader = connection, connection.operations
        self._exchange, self._auth = connection._exchange, connection._authorization
        self._source = DraftSource(self._exchange.destination_identity, self._exchange._location,
                                   connection._account, detail.container, branch.id, node)
        self._lock, self._abandoned = Lock(), Event()
        self._stack = ExitStack()
        try:
            self._live()
            self._home = self._stack.enter_context(PinnedParent(private_parent, output=True))
            self._protection = OutputProtection(self._home.handle)
            self._leaf = new_id() + ".zwd"  # Never use a remote display name, alias, or extension here.
            self._local_file = self._home.name(self._leaf)  # Diagnostic survives a failed or closed plan.
            self._home.api.absent(self._home.handle, self._leaf)
            self._live()
        except BaseException:
            self.close()
            raise

    @property
    def source(self):
        return self._source

    @property
    def local_file(self):
        return self._local_file

    def _live(self):
        reader, exchange, s = self._reader, self._exchange, self._source
        if (self._connection.operations is not reader or reader._exchange is not exchange
                or reader._outbox is not None or type(reader._selected) is not _Metadata
                or reader._selected._exchange is not exchange or reader._selected._profile is not None
                or self._connection._exchange is not exchange or self._connection._authorization is not self._auth
                or exchange._authorization is not self._auth or self._connection._account != s.account
                or (exchange.destination_identity, exchange._location) != (s.destination, s.origin)):
            raise Rejected("the original authenticated R3 reader, account or destination changed")
        self._auth.require_live()

    def _check_acquisition(self):
        if self._abandoned.is_set():
            raise Rejected("the selected Windows draft acquisition was abandoned")
        self._live()

    def _observe_selection(self):
        # A typed detail alone does not prove which account supplied it. Re-read over the
        # original authenticated metadata adapter before GET, never follow a replacement HEAD.
        self._check_acquisition()
        s = self._source
        detail = open_checked(self._reader, s.container.id)
        branch = next((b for b in detail.branches if b.id == s.branch), None)
        if (detail.container != s.container or branch is None or branch.head != s.head.id
                or s.head not in branch.nodes):
            raise Rejected("selected container, sequence, branch or original head changed")
        self._check_acquisition()

    def begin(self, *, confirmed=False):
        if confirmed is not True:
            raise Rejected("confirm the exact remote source and the retained private local draft first")
        with self._lock:
            keep, self._stack = self._stack, None
        if keep is None:
            raise Rejected("Windows draft choice already closed or taken")
        created = False
        with keep:
            home, leaf, s = self._home, self._leaf, self._source
            path = home.name(leaf)
            try:
                self._observe_selection()
                h2, node = self._exchange, s.head
                url = s.origin + h2._node_target(s.container.id, node.id, node.length, node.sha256)
                zone = _effective_zone(url)
                mark = _remote_mark(url, zone)
                home.api.flush(home.handle)
                home.check()
                self._protection.check()
                self._check_acquisition()
                h = keep.enter_context(home.api.new_file(home.handle, leaf))
                created = True
                seeded = _receive_node(keep, home, h, leaf, self._protection, h2, s.container.id,
                                       node.id, node.length, node.sha256, mark, self._check_acquisition)
                draft = WindowsHeldDraft(self, h, seeded.file_id, url, zone, mark)
                home.api.flush(home.handle)
                if draft._observe(node.length, policy=True) != seeded:
                    raise Rejected("original Windows seed changed before handoff")
                with self._lock:
                    self._check_acquisition()
                    draft._keep = keep.pop_all()
                return draft
            except Exception as error:
                if created:
                    raise Rejected(f"Windows draft was not opened; private bytes may remain at {path!r}: {error}") from error
                raise

    def close(self):
        with self._lock:
            self._abandoned.set()
            keep, self._stack = self._stack, None
        if keep is not None:
            keep.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


class WindowsHeldDraft:
    """Only this object's serialized requests have access to the original synchronous NT handle.

    File contexts denote native-final-close *candidates*. begin_drain blocks new contexts but
    keeps already open ones writable until release (never use WinFsp Cleanup as Close).
    No dispatcher is attached: a local review is not proof of kernel cache or mapping drain.
    """
    def __init__(self, choice, h, file_id, url, zone, mark):
        self._choice, self._home, self._leaf = choice, choice._home, choice._leaf
        self._h, self._file_id, self._url, self._zone, self._mark = h, file_id, url, zone, mark
        self._length = choice.source.head.length
        self._lock, self._contexts, self._state, self._keep = Lock(), {}, "editing", None

    def _observe(self, length, *, policy=False):
        self._choice._live()  # Checks this window in memory only, never initiates a request.
        self._home.check()
        self._choice._protection.check(self._h)
        if policy and _effective_zone(self._url) != self._zone:
            raise Rejected("the original OS download-origin policy changed")
        row = _same_file(self._home, self._h, self._leaf, length, self._file_id)
        _check_origin(self._home.api, self._h, row, self._mark)
        self._choice._live()
        return row

    @contextmanager
    def _fault_on_error(self):
        # Native I/O can have taken effect when an error or interruption is observed.
        # Never issue a usable local cut or retry a mutation after such an error.
        try:
            yield
        except BaseException:
            self._state = "fault"
            raise

    def open(self, *, writable=False):
        with self._lock:
            if self._state != "editing" or type(writable) is not bool:
                raise Rejected("no new Windows draft contexts")
            with self._fault_on_error():
                self._observe(self._length)
            key = object()
            self._contexts[key] = writable
            return key

    def _allow(self, key, *, write=False):
        if (self._state not in ("editing", "draining") or key not in self._contexts
                or (write and not self._contexts[key])):
            raise Rejected("this original Windows draft context does not grant that access")

    @staticmethod
    def _span(offset, count):
        if (type(offset) is not int or type(count) is not int or not 0 <= count <= CHUNK
                or not 0 <= offset <= _LIMIT - count):
            raise Rejected("invalid bounded Windows draft byte range")

    def read(self, key, offset, count):
        self._span(offset, count)
        with self._lock:
            self._allow(key)
            with self._fault_on_error():
                self._observe(self._length)
                wanted = min(count, max(0, self._length - offset))
                if not wanted:
                    return b""
                api = self._home.api
                api.position(self._h, offset)
                result = api.read(self._h, wanted)
                if len(result) != wanted:
                    raise Rejected("short read of the held Windows draft")
                self._choice._live()
                return result

    def write(self, key, offset, data):
        if type(data) is not bytes:
            raise Rejected("Windows draft write requires immutable bytes")
        self._span(offset, len(data))
        with self._lock:
            self._allow(key, write=True)
            with self._fault_on_error():
                self._observe(self._length)
                if data:
                    api = self._home.api
                    if offset > self._length:
                        api.resize(self._h, offset)  # FileEndOfFileInformation zero-pads the gap.
                    api.position(self._h, offset)
                    api.write(self._h, data)
                    length = max(self._length, offset + len(data))
                    self._observe(length)
                    self._length = length
                return len(data)

    def resize(self, key, size):
        self._span(size, 0)
        with self._lock:
            self._allow(key, write=True)
            with self._fault_on_error():
                self._observe(self._length)
                if size != self._length:
                    self._home.api.resize(self._h, size)
                    self._observe(size)
                    self._length = size

    def release(self, key):
        # Releasing a context does not flush, review, send, or remove any file.
        with self._lock:
            if key not in self._contexts:
                raise Rejected("unknown or previously released Windows draft context")
            del self._contexts[key]

    def begin_drain(self):
        with self._lock:
            if self._state not in ("editing", "draining"):
                raise Rejected("this Windows draft cannot begin a local drain")
            self._state = "draining"
            return len(self._contexts)

    def review(self, *, confirmed=False):
        with self._lock:
            if confirmed is not True or self._state != "draining" or self._contexts:
                raise Rejected("stop new opens, release every original context, then confirm local review")
            with self._fault_on_error():
                api, length = self._home.api, self._length
                api.flush(self._h)
                api.flush(self._home.handle)
                first = self._observe(length, policy=True)
                api.rewind(self._h)
                digest, left = sha256(), length
                while left:
                    self._choice._live()
                    data = api.read(self._h, min(CHUNK, left))
                    if not data or len(data) > left:
                        raise Rejected("held Windows draft ended before its observed length")
                    digest.update(data)
                    left -= len(data)
                if api.read(self._h, 1) or self._observe(length, policy=True) != first:
                    raise Rejected("original Windows draft changed during full-body review")
                result = DraftReview(self._choice.source, self._home.name(self._leaf),
                                     self._home.volume, self._file_id, length, digest.hexdigest())
                self._state = "reviewed"
                return result

    def close(self):
        # Abandonment, even with outstanding contexts, cannot manufacture a drained review.
        # Keep the generated private file, including dirty/partial edits. Never unlink or upload.
        with self._lock:
            self._state = "closed"
            self._contexts.clear()
            keep, self._keep = self._keep, None
        if keep is not None:
            keep.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
