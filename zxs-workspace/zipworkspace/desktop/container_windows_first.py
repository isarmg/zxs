"""Explicit first local file for Windows remote containers; no mount, network send or ZIP."""
from contextlib import ExitStack
from dataclasses import dataclass
from hashlib import sha256
from ntpath import split
from threading import Lock

from .._windows_output import PinnedParent, leaf
from ..model import CHUNK, Rejected, new_id
from .container_management import Container, _container
from .container_windows_draft import DraftSource, _require_reader
from .container_windows_save import _capture_exact


@dataclass(frozen=True, slots=True)
class FirstLocalReview:
    source_path: str
    private_directory: str
    destination: str
    account: str
    container: Container
    length: int
    sha256: str


class WindowsFirstLocalChoice:
    """Hold the exact ordinary local NTFS file until explicit private capture."""

    def __init__(self, connection, source_path, private_parent, remote_name):
        self._lock, self._keep = Lock(), ExitStack()
        try:
            exchange, auth, reader = connection._exchange, connection._authorization, connection.operations
            destination, origin, account = exchange.destination_identity, exchange._location, connection._account

            def live():
                _require_reader(connection, reader, exchange, auth, destination, origin, account)
            self._live = live
            live()
            owner = Container(new_id(), remote_name, 0)
            if not _container(owner):
                raise Rejected("choose a valid exact name for the new remote container")
            parent, filename = split(source_path)
            filename = leaf(filename)
            src = self._keep.enter_context(PinnedParent(parent))
            self._home = self._keep.enter_context(PinnedParent(private_parent, output=True))
            api = src.api
            h = self._keep.enter_context(api.input_file(src.handle, filename))
            native_name = src.name(filename)
            row = api.ordinary(h, src.volume, native_name)
            if not 0 <= row.size < 1 << 63:
                raise Rejected("first local file is out of range")

            def observe():
                live()
                src.check()
                self._home.check()
                if api.ordinary(h, src.volume, native_name) != row:
                    raise Rejected("the selected original Windows input file changed")
                api.only_default_stream(h, row.size)
            self._observe, self._handle = observe, h
            observe()
            api.rewind(h)
            remaining, digest = row.size, sha256()
            while remaining:
                live()
                chunk = api.read(h, min(CHUNK, remaining))
                if not chunk or len(chunk) > remaining:
                    raise Rejected("the first local file did not expose its full reported length")
                digest.update(chunk)
                remaining -= len(chunk)
            if api.read(h, 1):
                raise Rejected("the first local file exceeded its reported length")
            observe()
            self._source = DraftSource(destination, origin, account, owner, "main", None)
            self._review = FirstLocalReview(source_path, private_parent, origin, account, owner,
                                            row.size, digest.hexdigest())
            self._used = False
        except BaseException:
            self.close()
            raise

    @property
    def review(self):
        return self._review

    def capture(self, review, *, confirmed=False):
        with self._lock:
            if confirmed is not True or self._used or self._keep is None or review is not self._review:
                raise Rejected("confirm and capture the exact reviewed first local file once")
            self._used = True
            return _capture_exact(self._source, self._home, self._handle, new_id() + ".zwd",
                                  review.length, review.sha256, self._live, self._observe)

    def close(self):
        with self._lock:
            keep, self._keep = self._keep, None
            if keep is not None:
                keep.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
