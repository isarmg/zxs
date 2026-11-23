"""Desktop ownership of one in-flight original Mac drive per authenticated container. No scan or upload."""
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from hashlib import sha256
import json
import os

from ._macos_copy_evidence import _absent, require_exclusive_rename
from .container_plan import FullBody, ReceiptFence
from .container_spool import _private, _write_all
from .desktop.container_management import id_ok
from .document import archive_observation
from .macos_fskit_handoff import KeptFile, KnownDrive
from .macos_fskit_registration import SelectionRequest, _group, _headroom, _named, _witness
from .macos_retained_edit import (RetentionKey, _PrivateArea, _barrier, _birth, _check_root_id, _connection,
                                  _fd, _on_mac, _owner_name, _read, _state, _visit_original)
from .macos_retained_submit import OriginalSave, Phase, SaveResult, _container_lease, _create, _load, _prefix
from .model import Rejected, canonical
from .publish import Uncertain, rename_new_at


def _original_root(create):
    # Signed host's fixed Apple App Group independently remembers the exact APFS
    # identity of the foreground app's private STATE. No QSettings, paths from
    # the user, adoption of a pre-existing root, or second body store.
    leaf = "workspace-state-v1"
    with _group() as (group, volume, group_ok):
        try:
            os.stat(leaf, dir_fd=group, follow_symlinks=False)
        except FileNotFoundError:
            if not create:
                raise
            with _fd(group, leaf, create=True) as writer:
                _named(group, leaf, writer, volume)
                with _state(None, group_ok, create=True) as (_, _, root, _, check):
                    check()
                    raw = canonical([1, _witness(volume, group), _birth(writer), root])
                    _write_all(writer, raw)
                    os.fchmod(writer, 0o400)
                    _barrier(writer)
                    os.fsync(group)
                    _barrier(writer)
        with _fd(group, leaf) as fd:
            s = _named(group, leaf, fd, volume, readonly_record=True)
            if not 0 < s.st_size <= 1024:
                raise Rejected("unconfirmed original application STATE pointer; do not initialize another")
            try:
                raw = os.pread(fd, s.st_size+1, 0)
                root = tuple(json.loads(raw)[3])
                _check_root_id(root)
                if canonical([1, _witness(volume, group), _birth(fd), root]) != raw or len(raw) != s.st_size:
                    raise ValueError()
            except (IndexError, TypeError, KeyError, ValueError, RecursionError) as exc:
                raise Rejected("invalid original application STATE pointer") from exc
            os.fsync(group); _barrier(fd)
            again=_named(group,leaf,fd,volume,readonly_record=True)
            if archive_observation(again)!=archive_observation(s) or os.pread(fd,s.st_size+1,0)!=raw:
                raise Rejected("original application STATE selection changed")
            group_ok()
            return root


@dataclass(frozen=True, slots=True)
class Route:
    root: tuple
    destination: str
    account: str
    container: str
    branch: str
    node: str
    token: str

    def __post_init__(self):
        _check_root_id(self.root)
        _owner_name(self.destination, self.account, self.container, self.branch, self.node)
        if not id_ok(self.token):
            raise Rejected("original registration token required")

    @property
    def source(self):
        return self.container, self.branch, self.node

    def value(self):
        return [self.token, self.branch, self.node]


def _slot(d, a, c):
    return sha256(canonical([d, a, c])).hexdigest()


@dataclass(frozen=True, slots=True)
class Capture:
    route: Route
    known: KnownDrive
    body: FullBody
    key: RetentionKey | None


@contextmanager
def _area(conn, root, d, a):
    dest, account, guard = _connection(conn)
    if (d, a) != (dest, account):
        raise Rejected("not the same authenticated destination and account")
    with _state(root, guard) as (fd, path, _, vol, check):
        area = _PrivateArea(path, fd, vol, check)
        area.check_state()
        yield area
        area.check_state()


def _need(result):
    if result is None:
        raise Rejected("original drive record not found; no new reservation or recovery by name")
    value, check, fd = result
    check()
    return value, check, fd


def _active(stack, area, route, *, archived=False):
    bound = _slot(route.destination, route.account, route.container)
    leaf = ("ended-" + route.token) if archived else ("drive-" + bound)
    value, check, fd = _need(_load(stack, area, leaf, "selected", bound))
    if value != route.value():
        raise Rejected("a different drive owns this slot")
    return leaf, check, fd


def reserve(connection, request):
    if type(request) is not SelectionRequest or request._connection is not connection or len(request._admitted) != 1:
        raise Rejected("choose exactly one original container file in the desktop")
    selected, guard, _ = request._admitted[0]
    def write():
        guard()
        with _group() as (group,_,group_ok):
            group_ok(); _headroom(group,request.total_bytes)
        root = _original_root(True)
        route = Route(root, selected.destination, selected.account, selected.container.id, selected.branch,
                      selected.node.id, request.token)
        with _area(connection, root, route.destination, route.account) as area:
            bound = _slot(route.destination, route.account, route.container)
            with _container_lease(area,bound), ExitStack() as stack:
                _create(area, "drive-"+bound, "selected", bound, route.value())
                _, check, fd = _active(stack, area, route)
                check(); os.fsync(area.parent); _barrier(fd); check()
        guard()
        return route
    return _on_mac(write)


def registered(connection, route, registration):
    s, = registration.selections
    known = KnownDrive(registration.token, registration.digest, s.destination, s.account,
                       ((s.container.id, s.branch, s.node.id),))
    _matches(route, known)
    def write():
        with _area(connection, route.root, route.destination, route.account) as area, ExitStack() as stack:
            _, check, _ = _active(stack, area, route)
            _create(area, "registered-"+route.token, "registered", route.token, known.encode().decode())
            check()
            if _known(stack, area, route) != known:
                raise Rejected("the exact issued projection was not recorded")
        return known
    return _on_mac(write)


def _matches(route, known):
    if (type(route) is not Route or type(known) is not KnownDrive or known.token != route.token
            or known.selected != (route.source,) or (known.destination, known.account) != (route.destination, route.account)):
        raise Rejected("not the exact originally registered drive")


def _known(stack, area, route):
    raw, check, fd = _need(_load(stack, area, "registered-"+route.token, "registered", route.token))
    if type(raw) is not str:
        raise Rejected("invalid registered drive")
    known = KnownDrive.decode(raw.encode())
    _matches(route, known)
    os.fsync(area.parent); _barrier(fd); check()
    return known


def _bind(known):
    return sha256(known.encode()).hexdigest()


def _outcome(stack, area, route, known):
    k = _load(stack, area, "captured-"+route.token, "captured", _bind(known))
    if k is None:
        return None
    value, check, fd = k
    try:
        length, digest, key = value
        body = FullBody(length, digest)
        key = None if key is None else RetentionKey.decode(key.encode())
        if key is not None and ((key.destination,key.account,key.container,key.branch,key.node) !=
                                (route.destination,route.account,*route.source) or key.root != route.root):
            raise ValueError()
    except (TypeError, ValueError, AttributeError) as exc:
        raise Rejected("retained original does not match this drive") from exc
    os.fsync(area.parent); _barrier(fd); check()
    return Capture(route, known, body, key)


def recover(connection, container, *, ended_token=None):
    if not id_ok(container) or (ended_token is not None and not id_ok(ended_token)):
        raise Rejected("exact original container and optional archived drive token required")
    dest, account, _ = _connection(connection)
    def read():
        root = _original_root(False)
        with _area(connection, root, dest, account) as area, ExitStack() as stack:
            binding = _slot(dest, account, container)
            leaf = ("ended-"+ended_token) if ended_token else ("drive-"+binding)
            val, check, _ = _need(_load(stack, area, leaf, "selected", binding))
            if type(val) is not list or len(val)!=3:
                raise Rejected("unconfirmed original selection")
            token, branch, node = val
            if ended_token is not None and ended_token != token:
                raise Rejected("archived original identity mismatch")
            route = Route(root, dest, account, container, branch, node, token)
            check()
            try:
                known = _known(stack, area, route)
            except (Rejected, OSError) as exc:
                raise Uncertain("原磁盘 " + token + " 已保留，但完整登记尚不能确认。不按文件名搜索或重新挂载。") from exc
            outcome = _outcome(stack, area, route, known)
            check()
            if ended_token and outcome is None:
                raise Rejected("archived original was not captured")
            return route, known, outcome
    return _on_mac(read)


def captured(connection, route, known, kept):
    _matches(route, known)
    if (type(kept) is not KeptFile or (kept.selected.destination, kept.selected.account,
            kept.selected.container.id, kept.selected.branch, kept.selected.node.id) !=
            (route.destination, route.account, *route.source)
            or (kept.key is None) != (kept.body == FullBody(kept.selected.node.length,kept.selected.node.sha256))):
        raise Rejected("not the returned whole original or its genuine unchanged result")
    cap = Capture(route, known, kept.body, kept.key)
    if cap.key is not None:
        original_save(connection, cap)  # Pin the returned key, not merely the display source IDs.
    def write():
        with _area(connection, route.root, route.destination, route.account) as area, ExitStack() as stack:
            _, check, _ = _active(stack, area, route)
            if _known(stack, area, route) != known:
                raise Rejected("changed original registration")
            old = _outcome(stack, area, route, known)
            if old is None:
                _create(area, "captured-"+route.token, "captured", _bind(known),
                        [kept.body.length, kept.body.sha256, kept.key.encode().decode() if kept.key else None])
            if _outcome(stack, area, route, known) != cap:
                raise Rejected("not the original durably retained result")
            check()
            return cap
    return _on_mac(write)


def original_save(conn, cap):
    if type(cap) is not Capture or type(cap.key) is not RetentionKey:
        raise Rejected("there is no original changed whole-file save")
    _matches(cap.route, cap.known)
    def read(p):
        if p.key != cap.key or p.mount is None or p.mount[:2] != (cap.known.token,cap.known.digest) or p.action.body != cap.body:
            raise Rejected("the result no longer belongs to the same drive and exact operation")
        return OriginalSave(p.key,p.action)
    return _visit_original(conn, cap.route.root, *cap.route.source, read, exact_key=cap.key)


def archive(connection, cap, receipt=None):
    """Only an unchanged source or durably matched original receipt can free the active slot."""
    if type(cap) is not Capture:
        raise Rejected("verified original capture required")
    route, known = cap.route, cap.known
    _matches(route, known)
    if cap.key is None:
        if receipt is not None:
            raise Rejected("no remote transaction for an unchanged file")
    else:
        choice = original_save(connection, cap)
        if (type(receipt) is not SaveResult or receipt.phase is not Phase.CONFIRMED or receipt.original != choice
                or type(receipt.fence) is not ReceiptFence):
            raise Rejected("only a matched original receipt can free this container for another mount")
    moved=[]
    def complete():
        with _area(connection, route.root, route.destination, route.account) as area, ExitStack() as stack:
            binding=_slot(route.destination,route.account,route.container)
            guard=stack.enter_context(_container_lease(area,binding))
            src,check,fd=_active(stack,area,route)
            if _known(stack,area,route)!=known or _outcome(stack,area,route,known)!=cap:
                raise Rejected("different pending drive")
            require_exclusive_rename(area.parent)
            if cap.key is not None:
                fence,verify,_=_need(_load(stack,area,"receipt-"+cap.key.submission,"receipt",_prefix(cap.key)))
                if type(fence) is not dict or ReceiptFence(**fence)!=receipt.fence:
                    raise Rejected("the original receipt was not locally confirmed")
                verify(); guard()
                _archive_owner(area,stack,cap.key,moved)
            check(); guard()
            _move_record(area,src,"ended-"+known.token,fd,moved)
            _,after,archivedfd=_active(stack,area,route,archived=True)
            if _birth(archivedfd)!=_birth(fd):
                raise Uncertain("different physical drive after retirement")
            after(); guard()
    try:
        return _on_mac(complete)
    except BaseException as exc:
        if moved:
            raise Uncertain("原磁盘 "+known.token+" 的归档进度未确认。保留原号，先核对，不重新提交。") from exc
        raise


def _move_record(area,src,dst,fd,moved):
    original=_birth(fd)
    raw,stamp=_read(area,src,fd,os.fstat(fd).st_size,metadata=True)
    _absent(area.parent,dst)
    _private(area,src,fd,stamp)
    moved.append(True)
    rename_new_at(area.parent,src,dst)  # RENAME_EXCL; regular immutable file, not directory-only expected_source.
    after,now=_read(area,dst,fd,len(raw),metadata=True)
    if after!=raw or _birth(fd)!=original:
        raise Uncertain("different original after its archive rename")
    _absent(area.parent,src)
    os.fsync(area.parent); _barrier(fd)
    _private(area,dst,fd,now)
    _absent(area.parent,src)


def _archive_owner(area, stack, key, moved):
    # Move only the receipt-fenced source's immutable key. This makes the same
    # historical original available for a later independent edit. Old receipts
    # stay addressable by the exact returned key; no bytes are deleted.
    src = _owner_name(key.destination,key.account,key.container,key.branch,key.node)
    dst = "settled-"+key.submission
    def evidence(name):
        fd = stack.enter_context(_fd(area.parent,name))
        raw,_ = _read(area,name,fd,os.fstat(fd).st_size,metadata=True)
        if raw!=key.encode() or _birth(fd)!=key.issuer:
            raise Rejected("unrelated original key at retention boundary")
        return fd
    try:
        os.stat(dst,dir_fd=area.parent,follow_symlinks=False)
    except FileNotFoundError:
        _move_record(area,src,dst,evidence(src),moved)
    else:
        moved.append(True)  # A previous exact retirement may have stopped between its two moves.
    fd=evidence(dst)
    _absent(area.parent,src)
    os.fsync(area.parent); _barrier(fd)
