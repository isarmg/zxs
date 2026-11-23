"""Explicit R3 checkout into the signed desktop's fixed FSKit group. No upload or user-selected path."""
from contextlib import ExitStack, contextmanager
import ctypes as C
from dataclasses import dataclass
import hashlib
import os
import stat
import sys
from threading import Lock
from uuid import uuid4

from . import macos_read as mac
from ._macos_copy_evidence import _attributes, apfs, native
from .document import archive_observation
from .macos_edit_file import SelectedFile, _admit, _read_at, _write_at
from .macos_retained_edit import _barrier, _birth, _fd, _on_mac
from .model import CHUNK, Rejected, canonical, new_id
from .publish import Uncertain


@dataclass(frozen=True, slots=True)
class Registration:
    token: str
    digest: str
    volume: str
    selections: tuple[SelectedFile, ...]


def _host_group():
    if sys.platform != "darwin":
        raise Rejected("the signed macOS 27 desktop host is required")
    # Darwin RTLD_FIRST (dlfcn.h): dlopen(NULL) maps to RTLD_MAIN_ONLY, not RTLD_DEFAULT.
    # Handle to the current signed foreground application only. Do not dlopen a
    # library from the checkout, bundle paths, environment variables or user input.
    try:
        group = C.CDLL(None, mode=C.RTLD_LOCAL | 0x100).zxs_fskit_group_fd
    except AttributeError as exc:
        raise Rejected("this desktop does not include the signed FSKit host") from exc
    group.argtypes, group.restype = [], C.c_int32
    fd = group()
    if fd < 0:
        raise OSError(-fd, "the signed macOS host could not authorize its fixed App Group")
    return fd


def _witness(volume, fd):
    ino, birth = _birth(fd)
    return {"volumeHex": volume[1].hex(), "inode": ino, "birthSec": birth // 10**9, "birthNsec": birth % 10**9}


def _generation(info, digest):
    return {"size": info.st_size, "mtimeSec": info.st_mtime_ns // 10**9, "mtimeNsec": info.st_mtime_ns % 10**9,
            "ctimeSec": info.st_ctime_ns // 10**9, "ctimeNsec": info.st_ctime_ns % 10**9, "sha256": digest}


def _named(parent, name, fd, volume, *, directory=False, readonly_record=False):
    info = os.fstat(fd)
    if readonly_record:
        if (not stat.S_ISREG(info.st_mode) or stat.S_IMODE(info.st_mode) != 0o400 or info.st_nlink != 1
                or info.st_flags or info.st_uid != os.geteuid()):
            raise Rejected("not the original private read-only macOS registration")
        native().no_acl(fd)
    else:
        _attributes(fd, directory=directory)
    if (not os.path.samestat(os.stat(name, dir_fd=parent, follow_symlinks=False), info)
            or apfs(fd, fresh_snapshot=True) != volume or os.get_inheritable(fd)):
        raise Rejected("not the original group object")
    mac.check_spelling(fd, name)
    return info


@contextmanager
def _group():
    fd = _host_group()
    try:
        volume = apfs(fd, fresh_snapshot=True)
        def guard():
            info = os.fstat(fd)
            if (os.geteuid() == 0 or os.geteuid() != os.getuid() or os.getegid() != os.getgid()
                    or os.get_inheritable(fd) or not stat.S_ISDIR(info.st_mode) or info.st_flags
                    or info.st_uid != os.geteuid() or info.st_mode & 0o022
                    or apfs(fd, fresh_snapshot=True) != volume):
                raise Rejected("the app's original provisioned local group changed")
        guard()
        yield fd, volume, guard
        # Foundation must still resolve the fixed entitlement to this same held root.
        again = _host_group()
        try:
            if not os.path.samestat(os.fstat(again), os.fstat(fd)):
                raise Rejected("the signed host's original App Group changed")
            guard()
        finally:
            os.close(again)
    finally:
        os.close(fd)


def _headroom(group, remaining):
    v = os.fstatvfs(group)
    unit = v.f_frsize or v.f_bsize
    if unit <= 0 or v.f_bavail < 0 or v.f_bavail * unit < remaining + (256 << 20):
        raise OSError(28, "insufficient private APFS space for complete selections and 256 MiB headroom")


class SelectionRequest:
    """One explicit prepare request. Keep token before any side effect. No automatic reissue or cleanup."""
    def __init__(self, connection, choices, *, confirmed=False):
        if sys.platform != "darwin" or confirmed is not True or type(choices) is not tuple or not 1 <= len(choices) <= 128:
            raise Rejected("select 1–128 actual remote container files for this macOS drive")
        if not all(type(choice) is tuple and len(choice) == 3 for choice in choices):
            raise Rejected("original detail, branch, and node selections are required")
        admitted = sorted((_admit(connection, *choice) for choice in choices), key=lambda row: row[0].container.id)
        if len({r[0].container.id for r in admitted}) != len(admitted):
            raise Rejected("a container may appear only once on this drive")
        for _, principal, _ in admitted:
            principal()
        self._connection, self._exchange, self._admitted = connection, connection._exchange, tuple(admitted)
        self._token, self._volume = new_id(), str(uuid4())
        self._lock, self._started, self._prepared = Lock(), False, None

    @property
    def token(self):
        return self._token

    @property
    def total_bytes(self):
        return sum(row[0].node.length for row in self._admitted)

    def cancel_preflight(self):
        with self._lock:
            if self._started:
                raise Rejected("a reserved checkout is not a cancellable preflight")
            self._eligibility(3)

    def preflight(self, *, start=False):
        """Begin on the GUI thread, then poll without blocking. No body read or creation before success."""
        if type(start) is not bool:
            raise Rejected("invalid native preflight choice")
        return self._eligibility(1 if start else 0) == 1

    def _eligibility(self, verb):
        try:
            call = C.CDLL(None, mode=C.RTLD_LOCAL | 0x100).zxs_fskit_preflight
        except AttributeError as exc:
            raise Rejected("this distribution lacks a signed macOS 27 FSKit host") from exc
        call.argtypes, call.restype = [C.c_char_p, C.c_int32], C.c_int32
        value = call(self._token.encode("ascii"), verb)
        if value < 0:
            if verb == 0:
                call(self._token.encode("ascii"), 2)  # Retire a final refusal, never another mount attempt.
            raise OSError(-value, "FSKit host, original extension, or user enablement unavailable")
        return value

    def prepare(self):
        with self._lock:
            if self._started:
                raise Rejected("this original preparation is already reserved")
            if self._eligibility(2) != 1:
                raise Rejected("native signed-module preflight is still pending")
            self._started = True
        reserved = []
        try:
            out = _on_mac(self._prepare, reserved)
            with self._lock:
                self._prepared = out
            return out
        except BaseException as exc:
            if reserved:
                raise Uncertain("original private drive registration may be incomplete: retain its token; do not remount or scan") from exc
            raise

    def _authority(self, registration):
        with self._lock:
            if type(registration) is not Registration or registration is not self._prepared:
                raise Rejected("this registration was not prepared from this original authenticated selection")
        for _, principal, _ in self._admitted:
            principal()

    def _prepare(self, reserved):
        token, virtual, admitted = self._token, self._volume, self._admitted
        exchange = self._exchange
        with _group() as (group, volume, group_ok), ExitStack() as owners:
            session_name = "work-" + token
            group_ok()
            for _, principal, _ in admitted:
                principal()
            remaining = self.total_bytes
            def headroom():
                _headroom(group, remaining)  # No reservation or sparse-size optimism; available space may change.
            headroom()
            reserved.append(True)
            os.mkdir(session_name, 0o700, dir_fd=group)
            directory = owners.enter_context(_fd(group, session_name, directory=True))
            def base_ok():
                group_ok()
                _named(group, session_name, directory, volume, directory=True)
            base_ok()
            dir_id = _witness(volume, directory)
            with _fd(directory, "ticket", create=True) as ticket:
                _named(directory, "ticket", ticket, volume)
                if os.fstat(ticket).st_size:
                    raise Rejected("ticket must be the original empty file")
                _barrier(ticket)
                ticket_id = _witness(volume, ticket)
            os.mkdir("names", 0o700, dir_fd=directory)
            names = owners.enter_context(_fd(directory, "names", directory=True))
            _named(directory, "names", names, volume, directory=True)
            names_id = _witness(volume, names)
            entries = []
            for source, principal, selection in admitted:
                headroom()
                selection()  # Genuine R3 membership before the GET, never a naked client-supplied name/ID.
                cid, node = source.container.id, source.node
                leaf = "body-" + cid
                with _fd(directory, leaf, create=True) as writer:
                    info = _named(directory, leaf, writer, volume)
                    original = _witness(volume, writer)
                    if info.st_size != 0:
                        raise Rejected("new private registration body wasn't empty")
                    received = 0
                    def fill(block):
                        nonlocal received, remaining
                        principal()
                        if received == 0 or received % (64 << 20) < len(block):
                            headroom()
                        base_ok()
                        _named(directory, leaf, writer, volume)
                        if type(block) is not bytes or len(block) > node.length - received:
                            raise Rejected("wrong amount of exact remote content")
                        _write_at(writer, received, block)
                        received += len(block)
                        remaining -= len(block)
                        return len(block)
                    exchange.read_node_into(cid, node.id, length=node.length, sha256=node.sha256, write=fill)
                    if received != node.length:
                        raise Rejected("incomplete remote node")
                    _barrier(writer)
                # Truly O_RDONLY after the writer closes; verify every byte + generation before registration.
                with _fd(directory, leaf) as ro:
                    info = _named(directory, leaf, ro, volume)
                    if info.st_size != node.length or _witness(volume, ro) != original:
                        raise Rejected("private work body no longer names the original GET")
                    before, digest = archive_observation(info), hashlib.sha256()
                    for offset in range(0, node.length, CHUNK):
                        principal()
                        digest.update(_read_at(ro, offset, min(CHUNK, node.length-offset)))
                    if (digest.hexdigest() != node.sha256 or os.pread(ro, 1, node.length)
                            or archive_observation(_named(directory, leaf, ro, volume)) != before):
                        raise Rejected("complete body differs from the exact source node")
                    gen = _generation(info, node.sha256)
                selection()
                entries.append({"container": cid, "remoteNameHex": source.container.name.encode("utf-8").hex(),
                                "branch": source.branch, "head": source.head, "node": node.id, "parent": node.parent or "",
                                "revision": source.container.revision, "body": original, "generation": gen})
            for _, _, selection in admitted:
                selection()
            base_ok()
            with _fd(directory, "projection.json", create=True) as record:
                _named(directory, "projection.json", record, volume)
                raw = canonical({"format": "zxs-fskit-projection-1", "token": token, "volume": virtual,
                                 "directory": dir_id, "emptyTicket": ticket_id, "names": names_id,
                                 "record": _witness(volume, record), "entries": entries,
                                 "destination": admitted[0][0].destination, "account": admitted[0][0].account})
                if not 0 < len(raw) <= 1 << 20:
                    raise Rejected("oversized drive registration")
                _write_at(record, 0, raw)
                os.fchmod(record, 0o400)
                _barrier(record)
                _named(directory, "projection.json", record, volume, readonly_record=True)
                if os.pread(record, len(raw)+1, 0) != raw:
                    raise Rejected("the complete original registration did not read back")
                os.fsync(names)
                os.fsync(directory)
                os.fsync(group)
                _barrier(record)
                base_ok()
            for _, principal, _ in admitted:
                principal()
            return Registration(token, hashlib.sha256(raw).hexdigest(), virtual, tuple(row[0] for row in admitted))
