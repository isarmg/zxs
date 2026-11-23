"""One private Mac whole-file Submit intent. No provider, public export, transport or retry."""
from contextlib import ExitStack, contextmanager
from dataclasses import asdict, dataclass
import ctypes as C
import hashlib
import json
import os
from pathlib import Path
import stat
import struct
import sys
from threading import get_ident

from . import macos_read as mac
from ._macos_copy_evidence import _absent, _attributes, apfs, native, require_exclusive_rename
from .container_plan import Action, Plan
from .container_spool import PendingSubmit, _INTENT, _destination, _id, _leaf, _private, _write_all
from .desktop.container_connection import OpenConnection, _Authorization, _LOGIN, _catalog_name
from .desktop.container_management import Container, Node, _node, branch_ok, id_ok
from .desktop.container_client import ContainerClient
from .container_https import HttpsExchange
from .document import native_identity
from .macos_edit_file import SelectedFile, _EditFile
from .model import CHUNK, MAX_FILE, Rejected, canonical, new_id
from .private_candidates import CandidateDirectory
from .publish import Uncertain, rename_new_at

_ROOT = "org.zipworkspace.desktop.remote-state"
_LIMIT = 64 << 10


def _support():
    # Current non-sandboxed desktop distribution. Never honor a download path,
    # environment-supplied HOME, working directory or caller-selected output folder.
    import pwd
    home = Path(pwd.getpwuid(os.getuid()).pw_dir)
    if home.anchor != "/" or len(home.parts) < 2 or ".." in home.parts:
        raise Rejected("the local user's Application Support directory is unavailable")
    return home / "Library" / "Application Support"


def _on_mac(work, *args):
    if sys.platform != "darwin":
        raise Rejected("this private retention owner requires macOS")
    def protected():
        with mac.no_materialization():
            return work(*args)
    return mac.in_disposable_thread(protected)


def _birth(fd):
    # Darwin CPython exposes only float st_birthtime. Read the original kernel
    # timespec via fgetattrlist, not a float conversion or a reboot-local st_dev.
    _, _, attrs = mac._require()
    mask = 0x80000200  # ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_CRTIME (sys/attr.h).
    request = mac._Attributes(5, 0, mask)
    out = (C.c_uint32 * 10)()  # uint32 length, attribute_set_t, LP64 packed timespec.
    if C.sizeof(C.c_long) != 8 or attrs(fd, C.byref(request), out, C.sizeof(out), 4):
        raise Rejected("could not read the original APFS creation time")
    length, common, vol, dirs, files, forks, sec, nsec = struct.unpack("=6I2q", bytes(out))
    ino = os.fstat(fd).st_ino
    if (length != C.sizeof(out) or common != mask or vol or dirs or files or forks
            or ino <= 0 or sec <= 0 or not 0 <= nsec < 1_000_000_000):
        raise Rejected("the original APFS file identity and complete creation time are required")
    return ino, sec * 1_000_000_000 + nsec


def _root_id(volume, fd):
    return (volume[1].hex(), *_birth(fd))


def _check_root_id(value):
    if type(value) is not tuple or len(value) != 3 or any(type(n) is not int or n <= 0 for n in value[1:]):
        raise Rejected("unknown APFS STATE identity")
    _id(value[0])


@dataclass(frozen=True, slots=True)
class RetentionKey:
    """Exact, non-secret owner record. Exists durably before final packet publication."""
    root: tuple
    submission: str
    destination: str
    account: str
    container: str
    branch: str
    node: str | None  # None only for first-save owner on main.
    issuer: tuple
    intent_sha256: str

    def __post_init__(self):
        _check_root_id(self.root)
        _id(self.submission)
        _destination(self.destination)
        _destination(self.intent_sha256)
        _packet_owner(self.destination, self.account, self.container, self.branch, self.node)
        if type(self.issuer) is not tuple or len(self.issuer) != 2 or any(
                type(n) is not int or n <= 0 for n in self.issuer):
            raise Rejected("unknown original key identity")

    def encode(self):
        return canonical(asdict(self))

    @classmethod
    def decode(cls, raw):
        try:
            if type(raw) is not bytes or len(raw) > 2048:
                raise ValueError()
            obj = json.loads(raw)
            key = cls(**{**obj, "root": tuple(obj["root"]), "issuer": tuple(obj["issuer"])})
            if key.encode() != raw:
                raise ValueError()
            return key
        except (TypeError, KeyError, ValueError, RecursionError) as exc:
            raise Rejected("not an exact original macOS retention key") from exc


@contextmanager
def _fd(parent, leaf, *, directory=False, create=False):
    flags = mac.named_flags(directory=directory)
    if create:
        flags = (flags & ~os.O_ACCMODE) | os.O_RDWR | os.O_CREAT | os.O_EXCL
    fd = os.open(leaf, flags, 0o600, dir_fd=parent)
    try:
        yield fd
    finally:
        os.close(fd)


def _barrier(fd):
    import fcntl
    os.fsync(fd)
    fcntl.fcntl(fd, 51)  # Darwin F_FULLFSYNC: ask the device to flush buffered writes as well.


@contextmanager
def _state(expected, authorize, *, create=False):
    authorize()
    if not create:
        _check_root_id(expected)
    support = _support()
    with mac.literal_name(support, directory=True) as parent:
        volume = apfs(parent, fresh_snapshot=True)
        def support_ok():
            authorize()
            info = os.fstat(parent)
            if (os.getuid() == 0 or os.geteuid() != os.getuid() or info.st_uid != os.getuid()
                    or info.st_flags or info.st_mode & 0o022 or apfs(parent, fresh_snapshot=True) != volume):
                raise Rejected("need the original owner-controlled local Application Support directory")
            mac.check_literal_path(support, parent)
        support_ok()
        if create:
            os.mkdir(_ROOT, 0o700, dir_fd=parent)  # Exclusive initialization; never adopt or repair a root.
        with _fd(parent, _ROOT, directory=True) as root:
            identity = _root_id(volume, root)
            if not create and identity != expected:
                raise Rejected("different APFS volume or original private STATE")
            def guard():
                support_ok()
                _attributes(root, directory=True)
                mac.check_literal_path(support / _ROOT, root)
                mac.check_spelling(root, _ROOT)
                if (not os.path.samestat(os.stat(_ROOT, dir_fd=parent, follow_symlinks=False), os.fstat(root))
                        or apfs(root, fresh_snapshot=True) != volume or _root_id(volume, root) != identity):
                    raise Rejected("original application STATE changed")
            guard()
            if create:
                os.fsync(root)
                os.fsync(parent)
            yield root, support / _ROOT, identity, volume, guard
            guard()


def initialize_state(*, confirmed=False):
    """Explicit local enrollment; future callers must keep this original identity. Never replace it."""
    if confirmed is not True:
        raise Rejected("explicit initialization of fixed private application STATE required")
    def create():
        with _state(None, lambda: None, create=True) as (_, _, identity, _, _):
            return identity
    return _on_mac(create)


class _PrivateArea(CandidateDirectory):
    def __init__(self, path, fd, volume, guard):
        super().__init__(path, fd, native_identity(os.fstat(fd), stat.S_IFDIR))
        self._volume, self._guard = volume, guard

    def check_state(self):
        self._guard()
        super().check_state()

    def check_file(self, name, fd, expected=None):
        stamp = super().check_file(name, fd, expected)
        info = os.fstat(fd)
        mac.check_spelling(fd, name)
        if info.st_uid != os.getuid() or info.st_flags or apfs(fd, fresh_snapshot=True) != self._volume:
            raise Rejected("not the original local private APFS member")
        native().no_acl(fd)
        return stamp


class _Area(_PrivateArea):
    def __init__(self, root, path, fd, volume, guard):
        super().__init__(path, fd, volume, guard)
        self._root = root

    def check_state(self):
        super().check_state()
        _attributes(self.parent, directory=True)
        mac.check_literal_path(self.path, self.parent)
        mac.check_spelling(self.parent, self.path.name)
        named = os.stat(self.path.name, dir_fd=self._root, follow_symlinks=False)
        if not os.path.samestat(named, os.fstat(self.parent)) or apfs(self.parent, fresh_snapshot=True) != self._volume:
            raise Rejected("private submission no longer names the original APFS directory")


def _read(area, leaf, fd, length, *, metadata=False):
    stamp = _private(area, leaf, fd)
    if stamp[2] != length or (metadata and length > _LIMIT):
        raise Rejected("incorrect retained length")
    result = bytearray() if metadata else hashlib.sha256()
    offset = 0
    while offset < length:
        _private(area, leaf, fd, stamp)
        block = os.pread(fd, min(CHUNK, length - offset), offset)
        if not block:
            raise Rejected("retained member ended before its original complete length")
        if metadata:
            result.extend(block)
        else:
            result.update(block)
        offset += len(block)
    if os.pread(fd, 1, length):
        raise Rejected("retained member grew after capture")
    _private(area, leaf, fd, stamp)
    return (bytes(result) if metadata else result.hexdigest()), stamp


@dataclass(frozen=True, slots=True)
class FirstFile:
    """A new container, not an existing branch or node selection."""
    destination: str
    account: str
    container: Container
    branch: str = "main"


def _source_node(src):
    return None if type(src) is FirstFile else src.node.id


def _operation(src, body, operation, node, fork):
    if type(src) is FirstFile:
        if src.branch != "main" or src.container.revision != 0 or fork is not None:
            raise Rejected("a first local file must create main on a new container")
        return Action(operation, src.container.id, "save_node", 0, None, body=body,
                      name=src.container.name, branch="main", node_id=node)
    tip = src.head == src.node.id
    if not tip and src.branch != "main":
        raise Rejected("a historical save requires the original main selection")
    return Action(operation, src.container.id, "save_node", src.container.revision, None, body=body,
                  branch=src.branch if tip else fork, head=src.node.id if tip else None,
                  node_id=node, fork_from_main=None if tip else src.node.id)


def _plan(seal):
    src = seal.selected
    action = _operation(src, seal.body, new_id(), new_id(), new_id() if type(src) is SelectedFile and src.head != src.node.id else None)
    # Internal placement only, never the remote display name or an invented rename.
    return Plan((action,), ((src.container.id, _leaf(src.container.id)),))


def _names(child, leaf):
    with os.scandir(child) as entries:
        names = {entry.name for _, entry in zip(range(3), entries)}
    if names != {_INTENT, leaf}:
        raise Rejected("unexpected content in private submission")


def _owner_name(destination, account, container, branch, node):
    _id(node)
    return _packet_owner(destination, account, container, branch, node)


def _packet_owner(destination, account, container, branch, node):
    _destination(destination)
    _id(container)
    if node is None:
        if branch != "main":
            raise Rejected("a first file requires the original main owner")
    else:
        _id(node)
    if type(account) is not str or _LOGIN.fullmatch(account) is None or not branch_ok(branch):
        raise Rejected("unknown original selection")
    return "owner-" + hashlib.sha256(canonical([destination, account, container, branch, node])).hexdigest()


def retain_edit(edit, seal, root_identity, *, confirmed=False):
    """Reserve this exact original selection once and durably record its key before final publication."""
    if type(edit) is not _EditFile or confirmed is not True:
        raise Rejected("explicit retention of the original sealed edit is required")
    attempted = []
    try:
        return _on_mac(_retain, edit, seal, root_identity, attempted)
    except BaseException as exc:
        if attempted:
            raise Uncertain("a reservation or publication may exist; only reopen the original selection, never recapture") from exc
        raise


def _mount_binding(value):
    if type(value) is not tuple or len(value) != 3 or not id_ok(value[0]):
        raise Rejected("not the exact original FSKit session")
    _destination(value[1])
    _destination(value[2])
    return value


def _retain(edit, seal, expected, attempted, *, mount=None):
    if mount is not None:
        _mount_binding(mount)
    with _state(expected, edit._original) as (root, path, identity, volume, guard):
        require_exclusive_rename(root)
        edit._claim_retention(seal)
        src = seal.selected
        node = _source_node(src)
        if node is None and mount is not None:
            raise Rejected("a first file cannot use an existing native mount")
        owner = _packet_owner(src.destination, src.account, src.container.id, src.branch, node)
        _absent(root, owner)  # An existing owner (even incomplete) must never be overwritten.
        plan, submission, scratch = _plan(seal), new_id(), ".capture-" + new_id()
        cid = src.container.id
        final, leaf = submission + ".pending", _leaf(cid)
        _absent(root, final)
        with ExitStack() as held:
            key_writer = held.enter_context(ExitStack())
            # Interrupted exclusive creation itself can leave an original reservation. From
            # here onward never imply that a fresh attempt is safe, even on policy/close failure.
            attempted.append(True)
            kfd = key_writer.enter_context(_fd(root, owner, create=True))
            root_area = _PrivateArea(path, root, volume, guard)
            _attributes(kfd)
            root_area.check_file(owner, kfd)
            os.mkdir(scratch, 0o700, dir_fd=root)
            child = held.enter_context(_fd(root, scratch, directory=True))
            # Before a verified key/final exist, incomplete scratch is not recoverable authority.
            original = os.fstat(child)
            area = _Area(root, path / scratch, child, volume, guard)
            area.check_state()
            with _fd(child, leaf, create=True) as body:
                _attributes(body)
                area.check_file(leaf, body)
                for block in edit.blocks(seal):
                    area.check_state()
                    _write_all(body, block)
                os.fchmod(body, 0o400)
                _barrier(body)
                digest, stamp = _read(area, leaf, body, seal.body.length)
                if digest != seal.body.sha256:
                    raise Rejected("retained full file differs from the original seal")
                evidence = [*_birth(body), *stamp[2:]]  # Do not persist reboot-dependent device numbers.
            with _fd(child, _INTENT, create=True) as intent:
                _attributes(intent)
                record = {"format": "macos-private-submit-1", "submission": submission, "state": identity,
                          "packet": _birth(child), "body": evidence, "intent": _birth(intent), "key": _birth(kfd),
                          "source": asdict(seal.selected), "plan": plan.encode().decode("utf-8")}
                if mount is not None:
                    record.update(format="macos-fskit-submit-1", mount=mount)
                elif node is None:
                    record["format"] = "macos-first-submit-1"
                raw = canonical(record)
                if len(raw) > _LIMIT:
                    raise Rejected("private single-file intent is too large")
                _write_all(intent, raw)
                os.fchmod(intent, 0o400)
                _barrier(intent)
                saved, intent_stamp = _read(area, _INTENT, intent, len(raw), metadata=True)
                if saved != raw:
                    raise Rejected("did not read back the exact original private intent")
            os.fsync(child)
            key = RetentionKey(identity, submission, src.destination, src.account, cid, src.branch, node,
                               _birth(kfd), hashlib.sha256(raw).hexdigest())
            key_bytes = key.encode()
            _write_all(kfd, key_bytes)
            os.fchmod(kfd, 0o400)
            _barrier(kfd)
            saved, key_stamp = _read(root_area, owner, kfd, len(key_bytes), metadata=True)
            if saved != key_bytes:
                raise Rejected("the original key was not retained")
            os.fsync(root)
            _barrier(kfd)  # Key data AND parent entry are fenced before an exclusive final rename can occur.
            key_writer.close()
            with _fd(root, owner) as key_ro, _fd(child, leaf) as body_ro, _fd(child, _INTENT) as intent_ro:
                def members():
                    _private(root_area, owner, key_ro, key_stamp)
                    _names(child, leaf)
                    _private(area, _INTENT, intent_ro, intent_stamp)
                    _private(area, leaf, body_ro, stamp)
                members()
                _absent(root, final)
                rename_new_at(root, scratch, final, expected_source=original)
                area = _Area(root, path / final, child, volume, guard)
                members()
                os.fsync(child)
                os.fsync(root)
                _barrier(intent_ro)
                members()
                return key


def _connection(connection):
    if (type(connection) is not OpenConnection or not connection._listed
            or type(connection.operations) is not ContainerClient or type(connection._exchange) is not HttpsExchange
            or type(connection._authorization) is not _Authorization):
        raise Rejected("original authenticated native connection required")
    exchange, authority = connection._exchange, connection._authorization
    destination, account = exchange.destination_identity, connection._account
    def check():
        if (connection._exchange is not exchange or connection._authorization is not authority
                or connection._account != account or exchange.destination_identity != destination
                or connection.operations._exchange is not exchange or exchange._authorization is not authority):
            raise Rejected("not the original authenticated destination or principal")
        authority.require_account(account)
    check()
    return destination, account, check


def read_retained(connection, root_identity, container, branch, node, consume, *, confirmed=False):
    """Reopen by the known original selected IDs after restart, with no directory scan, GET or send."""
    if confirmed is not True or not callable(consume):
        raise Rejected("explicit selection of an original private packet and a scoped reader are required")
    def read(packet):
        with packet.pending.body(packet.action) as blocks:
            return consume(packet.pending.plan, blocks)
    return _visit_original(connection, root_identity, container, branch, node, read)


def _visit_original(connection, root_identity, container, branch, node, visit, *, exact_key=None):
    destination, account, authorize = _connection(connection)
    source = destination, account, container, branch, node
    owner = _packet_owner(*source)
    if exact_key is not None and (type(exact_key) is not RetentionKey or exact_key.root != root_identity
                or (exact_key.destination,exact_key.account,exact_key.container,exact_key.branch,exact_key.node) != source):
        raise Rejected("the chosen key belongs to another original")
    def bound(packet):
        if exact_key is not None and packet.key != exact_key:
            raise Rejected("not the supplied original physical key")
        return visit(packet)
    occupied = []
    def read():
        if exact_key is not None:
            # Only the caller's immutable receipt-fenced archive slot, never a scan or latest head.
            name = "settled-"+exact_key.submission
            with _state(root_identity,authorize) as (root,_,_,_,guard):
                try:
                    os.stat(name,dir_fd=root,follow_symlinks=False)
                except FileNotFoundError:
                    pass
                else:
                    guard()
                    return _read_retained(root_identity,source,name,authorize,bound,occupied)
        return _read_retained(root_identity,source,owner,authorize,bound,occupied)
    try:
        return _on_mac(read)
    except (OSError, Rejected) as exc:
        if occupied:
            raise Uncertain("original or operation state unconfirmed; preserve it, never recapture or retry blindly") from exc
        raise


@dataclass(frozen=True, slots=True)
class _Packet:
    key: RetentionKey
    source: SelectedFile | FirstFile
    action: Action
    pending: object
    root_area: _PrivateArea
    require: object
    mount: tuple | None
    archived: bool


class _ScopedPending:
    """The common stream contract, narrowed to the original thread and open physical evidence."""
    def __init__(self, pending, require):
        self._pending, self._require = pending, require
        self.plan, self.destination = pending.plan, pending.destination

    @contextmanager
    def body(self, action):
        self._require()
        with self._pending.body(action) as stream:
            loan = True
            def check():
                self._require()
                if not loan:
                    raise Rejected("the original body loan is closed")
            def blocks():
                while True:
                    check()
                    try:
                        part = next(stream)
                    except StopIteration:
                        return
                    check()
                    yield part
            try:
                yield blocks()
                check()
            finally:
                loan = False


def _read_retained(identity, source, owner, authorize, visit, occupied):
    with _state(identity, authorize) as (root, path, _, volume, guard), ExitStack() as held:
        # Absence of the original owner is different from a missing final after a key was reserved.
        os.stat(owner, dir_fd=root, follow_symlinks=False)
        occupied.append(True)
        root_area = _PrivateArea(path, root, volume, guard)
        kfd = held.enter_context(_fd(root, owner))
        raw_key, key_stamp = _read(root_area, owner, kfd, os.fstat(kfd).st_size, metadata=True)
        key = RetentionKey.decode(raw_key)
        if (key.root != identity or key.issuer != _birth(kfd)
                or (key.destination, key.account, key.container, key.branch, key.node) != source):
            raise Rejected("not the original physical owner key for this selected source")
        def key_guard():
            _private(root_area, owner, kfd, key_stamp)
        final = key.submission + ".pending"
        child = held.enter_context(_fd(root, final, directory=True))
        area = _Area(root, path / final, child, volume, key_guard)
        area.check_state()
        fd = held.enter_context(_fd(child, _INTENT))
        raw, intent_stamp = _read(area, _INTENT, fd, os.fstat(fd).st_size, metadata=True)
        if hashlib.sha256(raw).hexdigest() != key.intent_sha256:
            raise Rejected("not the exact private intent originally acknowledged before publication")
        # This pre-publication key pins every byte, including the stable inode/birth observations,
        # exact authenticated selection and canonical Plan. Never rebuild a missing plan.
        try:
            obj = json.loads(raw)
            plan = Plan.decode(obj["plan"].encode("utf-8"))
            action, = plan.actions
            cid, leaf = action.container_id, _leaf(action.container_id)
            saved = obj["source"]
            first = obj.get("format") == "macos-first-submit-1"
            if first:
                src = FirstFile(saved["destination"], saved["account"], Container(**saved["container"]), saved["branch"])
                valid_source = type(src.container.revision) is int and src.container.revision == 0 and src.branch == "main"
                fork = None
            else:
                src = SelectedFile(saved["destination"], saved["account"], Container(**saved["container"]),
                                   saved["branch"], saved["head"], Node(**saved["node"]))
                valid_source = (type(src.container.revision) is int and src.container.revision > 0
                                and branch_ok(src.branch) and id_ok(src.head) and _node(src.node)
                                and src.node.length <= MAX_FILE
                                and (src.node.length, src.node.sha256) != (action.body.length, action.body.sha256))
                fork = action.branch
            fields = {"format", "submission", "state", "packet", "body", "intent", "key", "source", "plan"}
            if obj.get("format") == "macos-fskit-submit-1":
                if type(obj.get("mount")) is not list:
                    raise ValueError()
                mount = _mount_binding(tuple(obj["mount"]))
                fields.add("mount")
            elif obj.get("format") in ("macos-private-submit-1", "macos-first-submit-1"):
                mount = None
            else:
                raise ValueError()
            if (obj.keys() != fields or obj["submission"] != key.submission
                    or obj["state"] != list(key.root) or obj["packet"] != list(_birth(child))
                    or obj["intent"] != list(_birth(fd)) or obj["key"] != list(key.issuer) or asdict(src) != saved
                    or (src.destination, src.account, src.container.id, src.branch, _source_node(src)) != source
                    or src.container.id != cid or not _catalog_name(src.container.name) or not valid_source
                    or action.kind != "save_node" or action.body.length > MAX_FILE
                    or action != _operation(src, action.body, action.operation_id, action.node_id, fork)
                    or plan.placements != ((cid, leaf),) or canonical(obj) != raw):
                raise ValueError()
        except (TypeError, KeyError, AttributeError, ValueError, RecursionError) as exc:
            raise Rejected("not the original APFS packet and selected single-file operation") from exc
        _names(child, leaf)
        body = held.enter_context(_fd(child, leaf))
        digest, stamp = _read(area, leaf, body, action.body.length)
        if digest != action.body.sha256 or obj["body"] != [*_birth(body), *stamp[2:]]:
            raise Rejected("body is not the original complete sealed APFS generation")
        os.fsync(child)
        os.fsync(root)
        _barrier(fd)
        # Use the established shared stream contract only after full preverification.
        pending = PendingSubmit(key.submission, key.destination, plan, area, {cid: stamp})
        thread, live = get_ident(), True
        def members():
            if not live or get_ident() != thread:
                raise Rejected("retained blocks outlived their original APFS owner")
            _private(area, _INTENT, fd, intent_stamp)
            _private(area, leaf, body, stamp)
        members()
        try:
            checked = _ScopedPending(pending, members)
            result = visit(_Packet(key, src, action, checked, root_area, members, mount, owner == "settled-"+key.submission))
            members()
            return result
        finally:
            live = False
