"""Dormant NTFS working body of one exact remote node. No WinFsp mount or Submit."""
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from errno import EBUSY
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
_APPEND = (1 << 64) - 1  # Unsigned WinFsp WriteToEndOfFile sentinel, never an NTFS offset.


@dataclass(frozen=True, slots=True)
class DraftSource:
    destination: str
    origin: str
    account: str
    container: Container  # Exact wire spelling, ID and observed sequence; never a local leaf.
    branch: str
    head: Node  # Selected base node; retained key spelling.
    observed_main_head: str | None = None


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


def _require_reader(connection, reader, exchange, auth, destination, origin, account):
    if (sys.platform != "win32" or type(connection) is not OpenConnection or not connection._listed
            or connection.writable or type(reader) is not ContainerClient or type(exchange) is not HttpsExchange
            or type(auth) is not _Authorization or type(account) is not str or _LOGIN.fullmatch(account) is None
            or connection.operations is not reader or reader._exchange is not exchange
            or reader._outbox is not None or type(reader._selected) is not _Metadata
            or reader._selected._exchange is not exchange or reader._selected._profile is not None
            or connection._exchange is not exchange or connection._authorization is not auth
            or exchange._authorization is not auth or connection._account != account
            or (exchange.destination_identity, exchange._location) != (destination, origin)):
        raise Rejected("the original authenticated R3 reader, account or destination changed")
    auth.require_account(account)


class WindowsDraftChoice:
    """Review an already listed connection, an eligible remote node and a private parent.

    Construction opens the parent only. begin() claims its handles once; closing a choice
    during acquisition cancels the handoff without seizing the worker's handles. Once begun,
    the draft owns its own lifetime. Failures may leave the generated private file; no cleanup
    by name, user-visible export or cold adoption.
    """
    def __init__(self, connection, detail, branch, node, private_parent, *, max_bytes):
        if (sys.platform != "win32" or type(connection) is not OpenConnection or not connection._listed
                or connection.writable or type(connection.operations) is not ContainerClient
                or type(connection._exchange) is not HttpsExchange
                or type(connection._authorization) is not _Authorization
                or type(connection._account) is not str or _LOGIN.fullmatch(connection._account) is None
                or type(detail) is not Detail or not _container(detail.container)
                or not detail.container.revision or type(branch) is not Branch or not _branch(branch)
                or branch not in detail.branches or not _node(node)
                or node not in branch.nodes or (node.id != branch.head and branch.id != "main")):
            raise Rejected("choose the original authenticated Windows reader and one exact branch head or historical main node")
        if type(max_bytes) is not int or not node.length <= max_bytes <= _LIMIT:
            raise Rejected("select an explicit logical byte ceiling large enough for the original Windows draft")
        self._max_bytes = max_bytes
        self._connection, self._reader = connection, connection.operations
        self._exchange, self._auth = connection._exchange, connection._authorization
        self._source = DraftSource(self._exchange.destination_identity, self._exchange._location,
                                   connection._account, detail.container, branch.id, node,
                                   branch.head if node.id != branch.head else None)
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
    def max_bytes(self):
        return self._max_bytes

    @property
    def local_file(self):
        return self._local_file

    def _live(self):
        s = self._source
        _require_reader(self._connection, self._reader, self._exchange, self._auth,
                        s.destination, s.origin, s.account)

    def cancel_acquisition(self):
        """Signal only. The worker keeps its original file/connection custody until the next checkpoint."""
        self._abandoned.set()

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
        if (detail.container != s.container or branch is None or branch.head != (s.observed_main_head or s.head.id)
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


@dataclass(eq=False, slots=True)
class _Backing:
    # Actual pinned private file. Only the published pointer may select bytes for review/Submit.
    leaf: str
    handle: object
    file_id: bytes
    length: int
    index: int
    flushed: bool = False
    faulted: bool = False


class WindowsHeldDraft:
    """Only this object's serialized requests have access to the original synchronous NT handle.

    File contexts denote native-final-close *candidates*. begin_drain blocks new contexts but
    keeps already open ones writable until release (never use WinFsp Cleanup as Close).
    No dispatcher is attached: a local review is not proof of kernel cache or mapping drain.
    """
    def __init__(self, choice, h, file_id, url, zone, mark):
        self._choice, self._home = choice, choice._home
        self._url, self._zone, self._mark = url, zone, mark
        self._initial = self._selected = _Backing(choice._leaf, h, file_id, choice.source.head.length, 2)
        self._scratch, self._attempted, self._witnessed = None, False, None
        self._lock, self._contexts, self._state, self._keep = Lock(), {}, "editing", None
        self._provider, self._project_origin = None, False

    @property
    def _h(self):
        return self._selected.handle

    @property
    def _leaf(self):
        return self._selected.leaf

    @property
    def _file_id(self):
        return self._selected.file_id

    @property
    def _length(self):
        return self._selected.length

    def _observe_backing(self, body, length, *, policy=False):
        if body.faulted:
            raise OSError("uncertain Windows scratch file; its original private bytes are retained")
        self._choice._live()  # Checks this window in memory only, never initiates a request.
        self._home.check()
        self._choice._protection.check(body.handle)
        if (policy or self._project_origin) and _effective_zone(self._url) != self._zone:
            raise Rejected("the original OS download-origin policy changed")
        row = _same_file(self._home, body.handle, body.leaf, length, body.file_id)
        _check_origin(self._home.api, body.handle, row, self._mark)
        self._choice._live()
        return row

    def _observe(self, length, *, policy=False):
        return self._observe_backing(self._selected, length, policy=policy)

    @contextmanager
    def _fault_on_error(self, body=None):
        # Never retry a possibly partial mutation. Scratch failure cannot select it or
        # erase the original; the failed scratch is preserved and blocks review.
        try:
            yield
        except BaseException:
            if body is None or body is self._selected:
                self._state = "fault"
            else:
                body.faulted = True
            raise

    def _claim_provider(self, owner, *, project_origin=False):
        with self._lock:
            if owner is None or self._provider is not None or self._state != "editing" or self._contexts:
                raise Rejected("the Windows draft is not exclusively available to the native provider")
            row = self._observe(self._length, policy=True)
            self._root_birth = row.birth
            self._provider, self._project_origin = owner, project_origin

    def _provider_info(self, owner, kind=2, *, backing=None):
        from ..windows_nt import _Basic
        with self._lock:
            if owner is not self._provider or owner is None or self._state not in ("editing", "draining"):
                raise Rejected("not the current original Windows file provider")
            body = self._selected if backing is None else backing
            if body is not self._selected and body is not self._scratch:
                raise Rejected("not an active retained backing")
            with self._fault_on_error(body):
                row = self._observe_backing(body, body.length)
                if kind == 1:  # Stable synthetic root, not the parent's real timestamps.
                    born = self._root_birth
                    return 0, born, born, born, born, 1, 1
                basic = self._home.api._info(body.handle, 0, _Basic)
                if ((basic.birth, basic.write, basic.change) != (row.birth, row.write, row.change)
                        or basic.access <= 0):
                    raise Rejected("inconsistent original Windows draft timestamps")
                if kind == 3 and self._mark is None:
                    raise Rejected("this original has no verified Windows origin stream")
                size = len(self._mark) if kind == 3 else body.length
                return size, basic.birth, basic.access, basic.write, basic.change, kind, body.index

    def _new_scratch(self, owner, leaf):
        # No data/paths are taken from an editor here. The original owner reserved this
        # generated private name; FILE_CREATE never truncates/reuses an existing file.
        with self._lock:
            if (owner is None or owner is not self._provider or self._state != "editing"
                    or self._attempted or self._selected is not self._initial or leaf == self._initial.leaf):
                raise PermissionError("the one allowed Windows scratch file is unavailable")
            with self._fault_on_error():
                self._observe(self._length, policy=True)
            self._attempted = True  # Failure may leave an actual retained scratch; no new name/retry.
            home, api = self._home, self._home.api
            api.flush(home.handle)
            file = self._keep.enter_context(api.new_file(home.handle, leaf))
            row = _same_file(home, file, leaf, 0)
            if row.file_id == self._file_id:
                raise Rejected("a scratch file must be a different original NTFS entity")
            self._choice._protection.check(file)
            api.only_default_stream(file, 0)
            body = _Backing(leaf, file, row.file_id, 0, 3)
            if self._mark is not None:
                stream = self._keep.enter_context(api.new_zone(file))
                api.write(stream, self._mark)
                api.flush(stream)
            api.flush(file)
            api.flush(home.handle)
            self._observe_backing(body, 0, policy=True)
            self._scratch = body
            return body

    def _promote_scratch(self, owner, key, body):
        with self._lock:
            active, writable = self._contexts.get(key, (None, False))
            if (owner is None or owner is not self._provider or self._state != "editing"
                    or body is None or body is not self._scratch or body is not active
                    or self._selected is not self._initial or writable):
                raise PermissionError("not the original active Windows scratch rename handle")
            if any(other is not key and backing is not None for other, (backing, _) in self._contexts.items()):
                raise BlockingIOError(EBUSY, "finish all Windows file and provenance contexts before replacement")
            if not body.flushed or body.faulted:
                raise PermissionError("flush and close the original scratch writer before attempting replacement")
            # Both real files remain at their original generated names. No overwrite, copy onto
            # the primary, deletion, native rename or R3 action. Commit once with no failing I/O afterward.
            old, api = self._selected, self._home.api
            with self._fault_on_error():
                self._observe(old.length, policy=True)
                api.flush(old.handle)
                self._observe(old.length, policy=True)
            with self._fault_on_error(body):
                before = self._observe_backing(body, body.length, policy=True)
                api.flush(body.handle)
                api.flush(self._home.handle)
                if self._observe_backing(body, body.length, policy=True) != before:
                    raise Rejected("the actual scratch changed during its replacement barrier")
            self._selected = body

    def _fault_replacement_reply(self, owner, body):
        with self._lock:
            if owner is self._provider and body is self._scratch and self._selected is body:
                # The callback itself is returning failure, not an SDK success reply.
                # Preserve both files and never let an interrupted reply select the body.
                self._selected, self._state = self._initial, "fault"

    def _observe_replacement(self, owner, index):
        with self._lock:
            if (owner is None or owner is not self._provider or self._state != "editing"
                    or self._selected is self._initial or type(index) is not int or self._selected.index != index):
                raise Rejected("no independent Windows lookup of the promoted original backing")
            self._observe(self._length, policy=True)
            self._witnessed = self._selected

    def _finish_provider(self, owner):
        # Only the private host calls this after its real directory, dispatcher and volume are gone.
        with self._lock:
            if owner is None or owner is not self._provider or self._state != "draining" or self._contexts:
                raise Rejected("the original Windows draft still has outstanding ownership")
            with self._fault_on_error():
                self._observe(self._length, policy=True)
                self._home.api.flush(self._h)
                self._home.api.flush(self._home.handle)
                self._observe(self._length, policy=True)
            self._project_origin, self._provider = False, None

    def _abandon_provider(self, owner, *, missing_ok=False):
        # A callback table or an SDK StopDispatcher return is not a normal OS detach receipt.
        with self._lock:
            if self._provider is not owner or owner is None:
                if missing_ok and owner is not None:
                    return  # A failed bind never acquired this draft; never seize a different owner.
                raise Rejected("cannot detach another Windows draft owner")
            self._state, self._provider, self._project_origin = "fault", None, False
            self._contexts.clear()

    def open(self, *, writable=False, _provider=None, _backing=None, _root=False):
        with self._lock:
            if (self._provider is not _provider or self._state != "editing" or type(writable) is not bool
                    or (_root and (_backing is not None or writable or _provider is None))):
                raise Rejected("no new Windows draft contexts")
            body = self._selected if _backing is None else _backing
            if body is not self._selected and (body is not self._scratch or _provider is None):
                raise Rejected("not an active original backing")
            with self._fault_on_error(body):
                self._observe_backing(body, body.length)
            key = object()
            self._contexts[key] = (None if _root else body, writable)
            return key

    def _allow(self, key, *, write=False):
        if self._state not in ("editing", "draining") or key not in self._contexts:
            raise Rejected("this original Windows draft context is not available")
        body, writable = self._contexts[key]
        if body is None or (write and not writable):
            raise Rejected("this original Windows context does not grant that data access")
        return body

    @staticmethod
    def _span(offset, count):
        if (type(offset) is not int or type(count) is not int or not 0 <= count <= CHUNK
                or not 0 <= offset <= _LIMIT - count):
            raise Rejected("invalid bounded Windows draft byte range")

    def read(self, key, offset, count):
        self._span(offset, count)
        with self._lock:
            body = self._allow(key)
            with self._fault_on_error(body):
                self._observe_backing(body, body.length)
                wanted = min(count, max(0, body.length - offset))
                if not wanted:
                    return b""
                api = self._home.api
                api.position(body.handle, offset)
                result = api.read(body.handle, wanted)
                if len(result) != wanted:
                    raise Rejected("short read of the held Windows draft")
                self._choice._live()
                return result

    def _write_range(self, body, offset, count, append, constrained):
        # Called under the same lock as the ensuing seek/write. Concurrent appends cannot
        # reuse a stale EOF; constrained lazy writes following a truncate cannot re-extend it.
        if (type(append) is not bool or type(constrained) is not bool or type(offset) is not int
                or not 0 <= count <= CHUNK or (offset != _APPEND if append else not 0 <= offset <= _LIMIT)):
            raise Rejected("invalid Windows draft write flags, size, or offset")
        start = body.length if append else offset
        if constrained:
            count = min(count, max(0, body.length - start))
        if count and start > self._choice.max_bytes - count:
            raise Rejected("Windows draft write would exceed the selected logical byte ceiling")
        return start, count

    def write(self, key, offset, data, *, append=False, constrained=False):
        if type(data) is not bytes:
            raise Rejected("Windows draft write requires immutable bytes")
        with self._lock:
            body = self._allow(key, write=True)
            offset, count = self._write_range(body, offset, len(data), append, constrained)
            with self._fault_on_error(body):
                self._observe_backing(body, body.length)
                if count:
                    body.flushed = False
                    api = self._home.api
                    if offset > body.length:
                        api.resize(body.handle, offset)  # FileEndOfFileInformation zero-pads the gap.
                    api.position(body.handle, offset)
                    api.write(body.handle, data[:count])
                    length = max(body.length, offset + count)
                    self._observe_backing(body, length)
                    body.length = length
                return count

    def flush(self, key, *, editor_data=False):
        """Flush this original file only. Not a volume flush, drain, freeze or server request."""
        with self._lock:
            body = self._allow(key, write=editor_data)
            with self._fault_on_error(body):
                self._observe_backing(body, body.length)
                self._home.api.flush(body.handle)
                self._observe_backing(body, body.length)
                if editor_data:
                    body.flushed = True  # Native file Flush after the last data change, not cleanup.
                return body.length

    def resize(self, key, size):
        self._span(size, 0)
        with self._lock:
            body = self._allow(key, write=True)
            if size > self._choice.max_bytes:
                raise Rejected("Windows draft resize would exceed the selected logical byte ceiling")
            with self._fault_on_error(body):
                self._observe_backing(body, body.length)
                if size != body.length:
                    body.flushed = False
                    self._home.api.resize(body.handle, size)
                    self._observe_backing(body, size)
                    body.length = size

    def release(self, key):
        # Releasing a context does not flush, review, send, or remove any file.
        with self._lock:
            if key not in self._contexts:
                raise Rejected("unknown or previously released Windows draft context")
            del self._contexts[key]

    def begin_drain(self, *, _provider=None):
        with self._lock:
            if self._provider is not _provider or self._state not in ("editing", "draining"):
                raise Rejected("this Windows draft cannot begin a local drain")
            self._state = "draining"
            return len(self._contexts)

    def review(self, *, confirmed=False):
        with self._lock:
            if confirmed is not True or self._provider is not None or self._state != "draining" or self._contexts:
                raise Rejected("stop new opens, release every original context, then confirm local review")
            if self._attempted and (self._selected is self._initial or self._witnessed is not self._selected):
                raise Rejected("原卷已关闭，不能确认临时内容已替换所选文件；保留两处私有原件，不准备或提交。")
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
                self._review, self._review_row, self._state = result, first, "reviewed"
                return result

    def prepare_save(self, review, *, confirmed=False):
        """Persist one exact existing-branch full-body intent. Never contact the server."""
        from .container_windows_save import _capture_review

        with self._lock:
            if (confirmed is not True or self._state != "reviewed" or review is not self._review
                    or not review.changed or self._contexts):
                raise Rejected("confirm this original, changed, fully reviewed Windows draft")
            with self._fault_on_error():
                key = _capture_review(self, review)
                self._state = "prepared"
                return key

    def close(self):
        # Abandonment, even with outstanding contexts, cannot manufacture a drained review.
        # Keep the generated private file, including dirty/partial edits. Never unlink or upload.
        with self._lock:
            if self._provider is not None:
                raise Rejected("the native dispatcher still owns the original Windows draft")
            self._state = "closed"
            self._contexts.clear()
            keep, self._keep = self._keep, None
        if keep is not None:
            keep.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
