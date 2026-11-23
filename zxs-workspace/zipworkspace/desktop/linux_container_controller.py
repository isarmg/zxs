"""Explicit selected-file binding and local Submit controller for Linux Workspace."""
from contextlib import ExitStack
from dataclasses import dataclass
import hashlib
import os
import stat
from threading import RLock

from .. import lifecycle, worktree
from ..container_plan import Baseline, FullBody
from ..container_spool import reopen_submit
from ..linux_authority import ProcView
from ..linux_edit_state import _RUNNING
from ..linux_submit import OriginalBinding
from ..model import CHUNK, Rejected, member_name, new_id
from ..private_freeze import local_mount, mount_id
from ..publish import Uncertain
from .container_connection import NativeWriteProfile, OpenConnection
from .container_management import Kind
from .container_outbox import NativeOutbox
from .live_checkout import LiveCheckout


def _stamp(row):
    return (row.st_dev, row.st_ino, row.st_mode, row.st_uid, row.st_nlink,
            row.st_size, row.st_mtime_ns, row.st_ctime_ns)


@dataclass(frozen=True)
class PreparedQueues:
    submission: str
    plan_sha256: str
    ready: tuple[str, ...]
    blocked: tuple[str, ...]


class LinuxContainerController:
    """An explicitly connected trusted owner, not a path-only recovery or a background uploader."""

    def __init__(self, connection, profile, live):
        if (type(connection) is not OpenConnection or not connection.writable
                or type(profile) is not NativeWriteProfile or Kind.SAVE not in profile._allowed
                or type(live) is not LiveCheckout):
            raise Rejected("select an authorized native Linux checkout and account")
        connection._authorization.require_live()
        outbox = profile.bind(connection._exchange.destination_identity, connection._account)
        if (outbox is None or type(outbox.registry) is not NativeOutbox
                or outbox.state != live.state or connection.operations._outbox is not outbox):
            raise Rejected("native container outbox does not own this session's original STATE")
        profile_path, profile_fd, profile_id = profile._state_selection
        state_id = _stamp(os.fstat(live.state_fd))[:2]
        if (profile_path != live.state or profile_id != state_id
                or _stamp(os.fstat(profile_fd))[:2] != state_id
                or local_mount(profile_fd) != local_mount(live.state_fd)):
            raise Rejected("authorized outbox and live checkout have different original STATE handles")
        self._profile_state_fd = profile_fd
        self._connection, self._outbox = connection, outbox
        self._origin = live
        self._held = ExitStack()
        self._live = live.borrow()
        self._held.callback(self._live.close)
        self._work_id = _stamp(os.fstat(self._live.work_fd))[:2]
        self._state_id = _stamp(os.fstat(self._live.state_fd))[:2]
        self._selected, self._originals, self._closed = {}, None, False
        self._destination = connection._exchange.destination_identity
        self._selection_lock, self._selection_uncertain = RLock(), False
        self._empty_confirmed = False

    def close(self):
        with self._selection_lock:
            self._closed = True
            self._held.close()

    def _require_original(self):
        if self._closed or self._origin.closed or self._live.review_origin is not self._origin.review_origin:
            raise Rejected("the original checkout is no longer active")
        self._connection._authorization.require_live()
        self._outbox.require_original()
        if (_stamp(os.fstat(self._live.state_fd))[:2] != self._state_id
                or _stamp(os.fstat(self._profile_state_fd))[:2] != self._state_id
                or local_mount(self._profile_state_fd) != local_mount(self._live.state_fd)
                or _stamp(os.fstat(self._live.work_fd))[:2] != self._work_id):
            raise Rejected("original checkout moved or was replaced")

    def _check_work(self):
        self._require_original()
        # Read-only exact live-catalog check before/after transfer, never held
        # across the network. The provider will independently bind this FD.
        with worktree.locked(self._live.state, read_only=True, live=self._live, local_only=True) as (db, row, _):
            self._live.check_instance(row, lifecycle.instance(db))
            lifecycle.require_visible(db, row)

    def _choice(self, container, branch, node):
        detail = self._connection.operations.open_container(container)
        found = tuple(item for item in detail.branches if item.id == branch)
        if len(found) != 1 or found[0].head is None:
            raise Rejected("no original selected branch head")
        chosen = tuple(item for item in found[0].nodes if item.id == node)
        if len(chosen) != 1:
            raise Rejected("no original selected node in this branch")
        owner = detail.container
        return Baseline(owner.id, owner.name, owner.revision, FullBody(chosen[0].length, chosen[0].sha256),
                        detail.tags, branch, node, found[0].head)

    def verify_existing_checkout(self, *, container, branch, node, path, expected, confirmed=False):
        with self._selection_lock:
            return self._verify_existing_checkout(container=container, branch=branch, node=node,
                                                  path=path, expected=expected, confirmed=confirmed)

    def _verify_existing_checkout(self, *, container, branch, node, path, expected, confirmed=False):
        """Bind selected remote bytes to an existing exact top-level local checkout file.

        Does not import a missing file, overwrite any file, contact the server
        on ordinary editor saves, or take a path as evidence of identity.
        """
        if (confirmed is not True or len(member_name(path)) != 1 or self._originals is not None
                or self._selection_uncertain):
            raise Rejected("explicit pre-edit verification of one original checkout file is required")
        self._check_work()
        baseline = self._choice(container, branch, node)
        if type(expected) is not Baseline or baseline != expected:
            raise Rejected("the selected remote revision, head or body changed after confirmation")
        if baseline.name != path or path in self._selected or any(x[0].container_id == container
                                                                    for x in self._selected.values()):
            raise Rejected("selected container name or binding conflicts with this checkout")
        flags = os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK
        with ExitStack() as acquired:
            # Open a stable original named Linux WORK directory, not a stale or
            # potentially O_PATH loan, for descriptor-relative file and fsync.
            root = os.open(".", flags | os.O_DIRECTORY, dir_fd=self._live.work_fd)
            acquired.callback(os.close, root)
            if _stamp(os.fstat(root))[:2] != self._work_id or local_mount(root) != local_mount(self._live.work_fd):
                raise Rejected("not the original local WORK mount")
            fd = os.open(path, flags, dir_fd=root)
            acquired.callback(os.close, fd)
            original = os.fstat(fd)
            before = _stamp(original)

            def still_same():
                return (_stamp(os.fstat(fd)) == before
                        and _stamp(os.stat(path, dir_fd=root, follow_symlinks=False)) == before)

            if (not stat.S_ISREG(original.st_mode) or original.st_uid != os.geteuid()
                    or original.st_nlink != 1 or original.st_size != baseline.body.length
                    or mount_id(fd) != mount_id(root) or not still_same()):
                raise Rejected("checkout does not hold one exact original local file")
            with self._connection._exchange.download_node(container, node, length=baseline.body.length,
                                                          sha256=baseline.body.sha256) as downloaded:
                offset = 0
                while block := downloaded.read(CHUNK):
                    if os.pread(fd, len(block), offset) != block:
                        raise Rejected("local file differs from the selected immutable node")
                    offset += len(block)
                if offset != baseline.body.length or os.pread(fd, 1, offset):
                    raise Rejected("local file length differs from original")
            if not still_same() or self._choice(container, branch, node) != baseline:
                raise Rejected("selected local file or remote revision changed during verification")
            os.fsync(fd)
            os.fsync(root)
            self._check_work()
            if not still_same():
                raise Rejected("original local file changed before binding")
            self._selected[path] = (baseline, fd, before)
            self._held.enter_context(acquired.pop_all())

    def _require_import_slot(self, root, path):
        self._check_work()
        require = ((self._live.work_fd, self._work_id), (root, self._work_id))
        if any(_stamp(os.fstat(fd))[:2] != identity for fd, identity in require):
            raise Rejected("the original local WORK directory changed")
        for directory, name, message in ((self._live.state_fd, _RUNNING, "an original native edit is still active"),
                                         (root, path, "the selected checkout name already exists; no overwrite")):
            try:
                os.stat(name, dir_fd=directory, follow_symlinks=False)
            except FileNotFoundError:
                continue
            raise Rejected(message)

    def import_checkout(self, *, container, branch, node, path, expected, confirmed=False, exclusive=False):
        """Explicitly import one selected immutable node before entering the mounted editor."""
        with self._selection_lock:
            if (confirmed is not True or exclusive is not True or len(member_name(path)) != 1
                    or self._originals is not None or self._selection_uncertain):
                raise Rejected("explicit exclusive pre-mount checkout selection is required")
            return self._import_checkout(container, branch, node, path, expected)

    def _import_checkout(self, container, branch, node, path, expected):
        self._check_work()
        if not getattr(os, "O_TMPFILE", 0):
            raise Rejected("the selected Linux work directory lacks anonymous import staging")
        baseline = self._choice(container, branch, node)
        if type(expected) is not Baseline or baseline != expected:
            raise Rejected("the selected remote revision, head or body changed after confirmation")
        if (baseline.name != path or path in self._selected
                or any(row[0].container_id == container for row in self._selected.values())):
            raise Rejected("checkout needs one exact locally selected name for the selected container")
        import fcntl
        flags = os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK
        try:
            with ExitStack() as stable:
                proc = ProcView(stable)
                root = os.open(".", flags | os.O_DIRECTORY, dir_fd=self._live.work_fd)
                stable.callback(os.close, root)
                mount = local_mount(root, read_proc=proc.read)
                if (mount != local_mount(self._live.work_fd, read_proc=proc.read)
                        or os.readlink(str(root), dir_fd=proc.fds) != str(self._live.work)):
                    raise Rejected("not the same original local WORK mount")
                try:
                    fcntl.flock(root, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError as error:
                    raise Rejected("the original WORK already has a native editing or import owner") from error
                self._require_import_slot(root, path)
                with self._connection._exchange.download_node(container, node, length=baseline.body.length,
                                                              sha256=baseline.body.sha256) as source:
                    output = os.open(".", os.O_TMPFILE | os.O_RDWR | os.O_CLOEXEC, 0o600, dir_fd=root)
                    try:
                        start = os.fstat(output)
                        if (not stat.S_ISREG(start.st_mode) or start.st_uid != os.geteuid()
                                or start.st_nlink != 0 or start.st_size != 0
                                or mount_id(output, read_proc=proc.read) != mount):
                            raise Rejected("cannot qualify the anonymous original checkout file")
                        digest, size = hashlib.sha256(), 0
                        while block := source.read(CHUNK):
                            size += len(block)
                            if size > baseline.body.length:
                                raise Rejected("the private source exceeded the selected length")
                            digest.update(block)
                            rest = memoryview(block)
                            while rest:
                                count = os.write(output, rest)
                                if count <= 0:
                                    raise OSError("short anonymous checkout write")
                                rest = rest[count:]
                        if size != baseline.body.length or digest.hexdigest() != baseline.body.sha256:
                            raise Rejected("the private source differed from selected full bytes")
                        os.fsync(output)
                        sealed = _stamp(os.fstat(output))
                        if (sealed[:4] != _stamp(start)[:4] or sealed[4:6] != (0, size)
                                or not stat.S_ISLNK(os.stat(str(output), dir_fd=proc.fds,
                                                             follow_symlinks=False).st_mode)
                                or _stamp(os.stat(str(output), dir_fd=proc.fds)) != sealed):
                            raise Rejected("the verified anonymous original changed")
                        # Recheck metadata and source before any permanent name appears.
                        if self._choice(container, branch, node) != baseline:
                            raise Rejected("selected container revision changed during import")
                        self._require_import_slot(root, path)
                        if _stamp(os.fstat(output)) != sealed:
                            raise Rejected("the anonymous checkout changed before publication")
                        self._selection_uncertain = True
                        try:
                            os.link(str(output), path, src_dir_fd=proc.fds, dst_dir_fd=root,
                                    follow_symlinks=True)
                        except FileExistsError as error:
                            self._selection_uncertain = False
                            raise Rejected("the selected checkout name appeared; no overwrite") from error
                        published = _stamp(os.fstat(output))
                        if (published[:4] != sealed[:4] or published[4] != 1
                                or published[5:7] != sealed[5:7]):
                            raise Rejected("the linked file is not the original checked output")
                        retained = os.open(path, flags, dir_fd=root)
                        stable.callback(os.close, retained)
                        if _stamp(os.fstat(retained)) != published:
                            raise Rejected("the newly linked local checkout was replaced")
                        os.fsync(root)
                    finally:
                        os.close(output)
                self._check_work()
                if (_stamp(os.stat(path, dir_fd=root, follow_symlinks=False)) != published
                        or _stamp(os.fstat(retained)) != published):
                    raise Rejected("the new original changed before local checkout confirmation")
                try:
                    os.stat(_RUNNING, dir_fd=self._live.state_fd, follow_symlinks=False)
                except FileNotFoundError:
                    pass
                else:
                    raise Rejected("native editing began before checkout confirmation")
                fcntl.flock(root, fcntl.LOCK_UN)
                self._selected[path] = (baseline, retained, published)
                self._held.enter_context(stable.pop_all())
                self._selection_uncertain = False
        except Exception as error:
            if self._selection_uncertain:
                raise Uncertain("complete checkout may already exist; inspect original WORK before reopening") from error
            raise

    def confirm_empty_checkout(self, *, confirmed=False):
        """Choose the authentic newly registered and physically empty local WORK."""
        with self._selection_lock:
            if (confirmed is not True or self._originals is not None or self._selected
                    or self._selection_uncertain):
                raise Rejected("explicit original empty checkout selection is required")
            self._require_empty_checkout()
            self._empty_confirmed = True

    def _require_empty_checkout(self):
        self._require_original()
        with worktree.locked(self._live.state, read_only=True, live=self._live, local_only=True) as (db, row, _):
            self._live.check_instance(row, lifecycle.instance(db))
            lifecycle.require_visible(db, row)
            original = self._live.initial_base
            if (original is None or self._live.checkout_marker is not None
                    or (row["repo"], row["branch"], row["generation"], row["parent"], row["base_sha"]) != original
                    or row["pending"] is not None or row["pending_sha"] is not None):
                raise Rejected("use a freshly registered original local WORK with no history")
            flags = os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW
            root = os.open(".", flags, dir_fd=self._live.work_fd)
            try:
                if (_stamp(os.fstat(root))[:2] != self._work_id
                        or local_mount(root) != local_mount(self._live.work_fd) or os.listdir(root)):
                    raise Rejected("the original local WORK is not empty; no files are adopted")
            finally:
                os.close(root)

    def bind_originals(self, bind_retained):
        """Same worker, before native dispatcher begins; invoked by the reviewed broker."""
        with self._selection_lock:
            self._bind_originals(bind_retained)

    def _bind_originals(self, bind_retained):
        self._check_work()
        if self._originals is not None or self._selection_uncertain:
            raise Rejected("this original edit session is already bound or checkout is uncertain")
        if self._empty_confirmed:
            if self._selected:
                raise Rejected("empty start cannot adopt an original remote selection")
            self._require_empty_checkout()  # same native worker under its original WORK lease
        elif not self._selected:
            raise Rejected("select one original file or explicitly confirm a fresh empty WORK")
        bound = []
        for path, (baseline, fd, before) in self._selected.items():
            if _stamp(os.fstat(fd)) != before:
                raise Rejected("pre-edit original file changed before native binding")
            loan = bind_retained(path, fd)
            if loan.path != path or loan.local_identity != before[:2] or _stamp(os.fstat(fd)) != before:
                raise Rejected("native provider did not bind the sealed original local file")
            bound.append(OriginalBinding(loan.local_identity, path, baseline))
        self._originals = tuple(bound)

    def prepare_cut(self, stopped, *, submission, branch_intents=()):
        """One originally confirmed Submit, closes native cut and registers queues; no send."""
        self._require_original()
        if self._originals is None:
            raise Rejected("original edit session never bound before its dispatcher")
        stopped.capture_linux(state=self._outbox.state, submission=submission, destination=self._destination,
                              originals=self._originals, admit_created=True, branch_intents=branch_intents)
        return self.register_original(submission)

    def new_submission_id(self):
        self._require_original()
        return new_id()

    def register_original(self, submission):
        """Register each exact spool queue independently; retry only this original ID."""
        self._require_original()
        with reopen_submit(self._outbox.state, submission, destination=self._destination) as pending:
            plan = pending.plan
            digest = hashlib.sha256(plan.encode()).hexdigest()
        available, blocked = [], []
        registry = self._outbox.registry
        for container, _ in plan.queues():
            self._require_original()
            try:
                with registry.hold(container):
                    if registry.bind_submission(container, submission) != digest:
                        raise Rejected("outbox registered a different original plan")
            except (Rejected, OSError):
                blocked.append(container)
            else:
                available.append(container)
        self._require_original()
        return PreparedQueues(submission, digest, tuple(available), tuple(blocked))
