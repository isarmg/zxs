"""A verified, normally stopped FSKit original into the existing private R3 SaveNode machinery."""
import ctypes as C
from dataclasses import dataclass
from hashlib import sha256
import json
import os
import re
import sys
from threading import Lock

from .container_plan import FullBody
from .desktop.container_connection import _LOGIN, _catalog_name
from .desktop.container_management import Container, Node, _node, branch_ok, id_ok, number
from .desktop.macos_fskit_host import MountPhase, OriginalDrive, _ok
from .macos_edit_file import SealedFile, SelectedFile, _read_at
from .macos_fskit_registration import _generation, _group, _named, _witness
from .macos_retained_edit import RetentionKey, _connection, _on_mac, _retain, _visit_original
from .model import CHUNK, MAX_FILE, Rejected, canonical
from .publish import Uncertain


def _digest(s):
    return type(s) is str and re.fullmatch("[0-9a-f]{64}", s) is not None


@dataclass(frozen=True, slots=True)
class KnownDrive:
    token: str
    digest: str
    destination: str
    account: str
    selected: tuple[tuple[str, str, str], ...]  # Container, original branch, original chosen node. No credentials.

    def __post_init__(self):
        if (not id_ok(self.token) or not _digest(self.digest) or not _digest(self.destination)
                or type(self.account) is not str or not _LOGIN.fullmatch(self.account)
                or type(self.selected) is not tuple or not 1 <= len(self.selected) <= 128
                or not all(type(v) is tuple and len(v) == 3 and id_ok(v[0]) and branch_ok(v[1]) and id_ok(v[2])
                           for v in self.selected) or tuple(sorted(self.selected)) != self.selected
                or len({v[0] for v in self.selected}) != len(self.selected)):
            raise Rejected("not an exact known FSKit registration")

    def _scope(self):
        parts = [self.token, self.digest, self.destination, self.account, *(":".join(v) for v in self.selected)]
        return sha256(("\n".join(parts) + "\n").encode("ascii")).hexdigest()

    def encode(self):
        return canonical([self.token, self.digest, self.destination, self.account, self.selected])

    @classmethod
    def decode(cls, data):
        try:
            if type(data) is not bytes or len(data) > 64 << 10:
                raise ValueError()
            token, digest, destination, account, selected = json.loads(data)
            out = cls(token, digest, destination, account, tuple(tuple(v) for v in selected))
            if out.encode() != data:
                raise ValueError()
            return out
        except (TypeError, ValueError, RecursionError) as exc:
            raise Rejected("not the previous exact drive key") from exc

    @classmethod
    def from_original(cls, drive):
        if type(drive) is not OriginalDrive:
            raise Rejected("an original drive prepared by the signed host is required")
        r = drive.registration
        return cls(r.token, r.digest, r.selections[0].destination, r.selections[0].account,
                   tuple((s.container.id, s.branch, s.node.id) for s in r.selections))


def _functions():
    if sys.platform != "darwin":
        raise Rejected("this handoff requires the original signed macOS host")
    exe = C.CDLL(None, mode=C.RTLD_LOCAL | 0x100)  # Darwin RTLD_MAIN_ONLY, never a searched path.
    try:
        reopen, lend, retire = exe.zxs_fskit_completed, exe.zxs_fskit_lend, exe.zxs_fskit_release_completed
    except AttributeError as exc:
        raise Rejected("this signed application does not support the original FSKit handoff") from exc
    reopen.argtypes = [C.c_char_p, C.c_char_p, C.c_char_p, C.POINTER(C.c_uint64)]
    lend.argtypes = [C.c_uint64, C.c_char_p, C.POINTER(C.c_int32), C.POINTER(C.c_char), C.c_ssize_t]
    retire.argtypes = [C.c_uint64]
    reopen.restype = lend.restype = retire.restype = C.c_int32
    return reopen, lend, retire


@dataclass(frozen=True, slots=True)
class KeptFile:
    selected: SelectedFile
    body: FullBody
    key: RetentionKey | None  # None means verified unchanged; never create an empty version for that case.


class CompletedDrive:
    """Single GUI owner, true local confirmations only. A cold reopen never asks FSClient to mount."""
    def __init__(self, known, slot):
        self.known, self._slot = known, slot
        self._finished, self._borrowed = set(), set()
        self._lock = Lock()

    @classmethod
    def after_normal(cls, drive, connection):
        known = KnownDrive.from_original(drive)
        destination, account, _ = _connection(connection)
        if (destination, account) != (known.destination, known.account) or drive.status().phase is not MountPhase.LOCAL_NORMAL:
            raise Rejected("this selected original has not completed both sides of normal teardown")
        return cls(known, drive._key)

    @classmethod
    def reopen(cls, known, connection, *, confirmed=False):
        dest, account, _ = _connection(connection)
        if confirmed is not True or type(known) is not KnownDrive or (known.destination, known.account) != (dest, account):
            raise Rejected("explicit reopen of the exact previously known original under its account is required")
        reopen, _, _ = _functions()
        slot = C.c_uint64()
        _ok(reopen(known.token.encode("ascii"), known.digest.encode("ascii"), known._scope().encode("ascii"), C.byref(slot)))
        if not slot.value:
            raise Rejected("no original lease")
        return cls(known, slot.value)

    def borrow(self, connection, original, *, confirmed=False):
        if confirmed is not True or original not in self.known.selected or type(original) is not tuple:
            raise Rejected("select one of the originally registered container/branch/node identities")
        dest, account, auth = _connection(connection)
        if (dest, account) != (self.known.destination, self.known.account):
            raise Rejected("not the original authenticated principal")
        _, lend, _ = _functions()
        fds, out = (C.c_int32 * 3)(-1, -1, -1), C.create_string_buffer(4096)
        with self._lock:
            if not self._slot or original in self._borrowed or original in self._finished:
                raise Rejected("this original is no longer available for a new local handoff")
            _ok(lend(self._slot, original[0].encode("ascii"), fds, out, len(out)))
            self._borrowed.add(original)
        def done(kept):
            with self._lock:
                self._borrowed.discard(original)
                if kept:
                    self._finished.add(original)
        try:
            return _MountedOriginal(self.known, original, auth, tuple(fds), out.value, done)
        except BaseException:
            try:
                _close_all(tuple(fd for fd in dict.fromkeys(fds) if fd >= 0))
            finally:
                done(False)
            raise

    def release(self, *, confirmed=False):
        # No deletion or discard; every original must be checked and unchanged or retained.
        _, _, release = _functions()
        with self._lock:
            if confirmed is not True or not self._slot or self._borrowed or self._finished != set(self.known.selected):
                raise Rejected("not all original files have been safely handed off")
            _ok(release(self._slot))
            self._slot = 0

    def park(self, *, confirmed=False):
        # The private files and previously saved exact recovery reference survive.
        # Never park a live mount, caller-owned outstanding loan or incomplete provider.
        _, _, release = _functions()
        with self._lock:
            if confirmed is not True or not self._slot or self._borrowed:
                raise Rejected("cannot park a live or borrowed original")
            _ok(release(self._slot)); self._slot = 0


def _close_all(fds):
    first = None
    for fd in fds:
        try:
            os.close(fd)  # Exactly once; never retry a potentially recycled FD.
        except OSError as exc:
            if first is None:
                first = exc
    if first is not None:
        raise first


class _MountedOriginal:
    def __init__(self, known, original, auth, fds, raw, done):
        if len(set(fds)) != 3 or min(fds) < 0:
            raise Rejected("signed original body, parent and independent owner lease required")
        self.known, self.original, self._auth = known, original, auth
        self._body, self._directory, self._lease = fds
        self._done, self._lock, self._used = done, Lock(), False
        self._guard, self._claimed = None, False
        try:
            p = json.loads(raw)
            src, final = p["source"], p["final"]
            if (canonical(p) != raw or set(p) != {"token", "digest", "destination", "account", "directory", "emptyTicket", "source", "final"}
                    or (p["token"], p["digest"], p["destination"], p["account"])
                        != (known.token, known.digest, known.destination, known.account)
                    or (src["container"], src["branch"], src["node"]) != original or src["body"] != final["body"]
                    or final["container"] != original[0]):
                raise ValueError()
            name = bytes.fromhex(src["remoteNameHex"]).decode("utf-8")
            if not _catalog_name(name) or name.encode().hex() != src["remoteNameHex"]:
                raise ValueError()
            remote = FullBody(src["generation"]["size"], src["generation"]["sha256"])
            n = Node(src["node"], src["parent"] or None, remote.length, remote.sha256)
            container = Container(src["container"], name, src["revision"])
            if (not _node(n) or not id_ok(src["head"]) or not number(src["revision"]) or not src["revision"]
                    or (src["head"] != n.id and src["branch"] != "main")):
                raise ValueError()
            self.source = SelectedFile(known.destination, known.account, container, src["branch"], src["head"], n)
            self.final = FullBody(final["generation"]["size"], final["generation"]["sha256"])
            if self.final.length > MAX_FILE:
                raise ValueError()
            self._proof, self.binding = p, (known.token, known.digest, sha256(raw).hexdigest())
            self._seal = SealedFile(self.source, self.final)
        except (KeyError, ValueError, TypeError, UnicodeError, RecursionError) as exc:
            raise Rejected("invalid original signed-bridge file handoff") from exc

    def _original(self):
        self._auth()
        if self._guard is None:
            raise Rejected("the original APFS body is outside its verified handoff")
        self._guard()

    def _claim_retention(self, seal):
        self._original()
        if seal is not self._seal or self._claimed or self.final == FullBody(self.source.node.length, self.source.node.sha256):
            raise Rejected("not the original unclaimed changed whole-file generation")
        self._claimed = True

    def blocks(self, seal):
        if seal is not self._seal or not self._claimed:
            raise Rejected("not the claimed original streamed body")
        yield from self._blocks()

    def _blocks(self):
        self._original()
        for offset in range(0, self.final.length, CHUNK):
            self._original()
            b = _read_at(self._body, offset, min(CHUNK, self.final.length-offset))
            self._original()
            yield b
        self._original()
        if os.pread(self._body, 1, self.final.length):
            raise Rejected("sealed original grew during private handoff")

    def retain(self, connection, root_identity, *, confirmed=False):
        if confirmed is not True:
            raise Rejected("explicit complete-file handoff is required")
        with self._lock:
            if self._used:
                raise Rejected("not a reusable held native original")
            self._used = True
        kept, attempted = False, []
        try:
            destination, account, checked_account = _connection(connection)
            if (destination, account) != (self.known.destination, self.known.account):
                raise Rejected("cannot retain the original under another destination or account")
            def capture():
                import fcntl
                with _group() as (group, volume, group_ok):
                    self._auth()
                    if any(os.get_inheritable(fd) or fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_ACCMODE != os.O_RDONLY
                           for fd in (self._body, self._directory, self._lease)):
                        raise Rejected("the signed bridge did not lend three independent read-only handles")
                    def guard():
                        checked_account()
                        group_ok()
                        p = self._proof
                        _named(group, "work-" + self.known.token, self._directory, volume, directory=True)
                        ticket = _named(self._directory, "ticket", self._lease, volume)
                        info = _named(self._directory, "body-" + self.original[0], self._body, volume)
                        if (_witness(volume, self._directory) != p["directory"] or _witness(volume, self._lease) != p["emptyTicket"]
                                or ticket.st_size or _witness(volume, self._body) != p["final"]["body"]
                                or _generation(info, self.final.sha256) != p["final"]["generation"]):
                            raise Rejected("sealed body, exact attached owner or held generation changed")
                    self._guard = guard
                    try:
                        digest = sha256()
                        for b in self._blocks():
                            digest.update(b)
                        if digest.hexdigest() != self.final.sha256:
                            raise Rejected("physical complete original does not match the postdrain digest")
                        if self.final == FullBody(self.source.node.length, self.source.node.sha256):
                            return KeptFile(self.source, self.final, None)
                        def previous(p):
                            self._original()
                            if p.mount != self.binding or p.source != self.source or p.action.body != self.final:
                                raise Rejected("another mount or different generation owns the prior submit key")
                            return p.key
                        try:
                            key = _visit_original(connection, root_identity, *self.original, previous)
                        except FileNotFoundError:
                            key = _retain(self, self._seal, root_identity, attempted, mount=self.binding)
                        return KeptFile(self.source, self.final, key)
                    finally:
                        self._guard = None
            result = _on_mac(capture)
            kept = True
            return result
        except BaseException as exc:
            if attempted:
                raise Uncertain("the original mount's submit may be reserved; reopen only the same known generation") from exc
            raise
        finally:
            self._close(kept)

    def cancel_unclaimed(self):
        with self._lock:
            if self._used:
                raise Rejected("already transferring this original")
            self._used = True
        self._close(False)

    def _close(self, kept):
        try:
            _close_all((self._body, self._directory, self._lease))
        except OSError as exc:
            self._done(False)
            raise Uncertain("local handle release uncertain; preserve and reopen only the exact original") from exc
        self._done(kept)
