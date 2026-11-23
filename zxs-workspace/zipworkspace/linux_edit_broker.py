"""Private Linux editable-volume handoff. Not wired to a window, CLI or package."""
from contextlib import ExitStack, contextmanager
from dataclasses import asdict, dataclass, field
import fcntl
import hashlib
import os
from pathlib import Path
import re
import stat
from threading import Event, Lock, get_native_id

from .document import native_identity
from .freeze import _distinct_roots
from .linux_authority import ProcView, owned_directory
from .linux_edit import edit_live_worktree
from .linux_edit_mount import installed_fuse
from .linux_edit_state import running_state
from .linux_mount import MOUNTINFO_BUDGET, _ordinary_user, held_point, mountinfo_path, point_path
from .model import Rejected, canonical, new_id
from .private_freeze import mount_id

EDIT_FLAGS = frozenset({"rw", "nodev", "nosuid", "noexec"})
_KERNEL_FLAGS = frozenset(flag.encode() for flag in EDIT_FLAGS)


@dataclass(frozen=True, slots=True)
class MountReceipt:
    point: Path
    source: str
    mount_id: bytes


def _mount_table(proc, namespace):
    task = f"task/{get_native_id()}"
    def same_namespace():
        return os.readlink(f"{task}/ns/mnt", dir_fd=proc.task) == namespace
    if not same_namespace(): raise Rejected("the original mount namespace changed")
    raw = proc.read(f"{task}/mountinfo", MOUNTINFO_BUDGET)
    if not same_namespace() or not 0 < len(raw) <= MOUNTINFO_BUDGET or not raw.endswith(b"\n"):
        raise Rejected("incomplete original kernel mount inventory")
    rows = {}
    for line in raw.split(b"\n")[:-1]:
        a, sep, b = line.partition(b" - ")
        left, right = a.split(b" "), b.split(b" ")
        if (not sep or len(left) < 6 or len(right) != 3 or not all(left) or not all(right)
                or not left[0].isdigit() or not left[1].isdigit() or left[0] in rows):
            raise Rejected("ambiguous original kernel mount inventory")
        rows[left[0]] = left, right
    return rows


class _Point:
    def __init__(self, held, spelling, live, proc, uid):
        self.proc, self.uid, self.gid = proc, uid, os.getgid()
        self.namespace = os.readlink(f"task/{get_native_id()}/ns/mnt", dir_fd=proc.task)
        if re.fullmatch(r"mnt:\[[0-9]+\]", self.namespace) is None:
            raise Rejected("no original native mount namespace")
        self.point = point_path(spelling)
        self.source = "zip-workspace-edit-" + new_id()
        self.path_key = mountinfo_path(self.point)
        if self._related(_mount_table(proc, self.namespace)):
            raise Rejected("a mount already covers the selected point")
        _, self.fds, self._check_empty = held.enter_context(held_point(spelling, uid))
        self.chain = tuple(self._identity(fd) for fd in self.fds)
        self.parent = self.chain[-1][2]
        self.device = f"{os.major(self.chain[-1][0])}:{os.minor(self.chain[-1][0])}".encode()
        self.roots = live.work_fd, live.state_fd, self.fds[-1]
        self.store, self.store_identity = live.store, live.store_identity

    def _identity(self, fd):
        return (*native_identity(os.fstat(fd), stat.S_IFDIR), mount_id(fd, read_proc=self.proc.read))

    def _rows(self):
        rows = _mount_table(self.proc, self.namespace)
        parent = rows.get(self.parent)
        if (parent is None or parent[0][2:4] != [self.device, b"/"]
                or parent[0][6:] not in ([], [b"unbindable"]) or parent[1][0] not in (b"ext4", b"xfs")):
            raise Rejected("the original point parent is not a private admitted local mount")
        return rows

    def _related(self, rows, receipt=None):
        source = self.source.encode("ascii")
        return [(left, right) for mid, (left, right) in rows.items()
                if left[4] == self.path_key or left[4].startswith(self.path_key + b"/")
                or right[1] == source or (receipt is not None and mid == receipt.mount_id)]

    def before(self):
        # Do not open or walk a live FUSE volume to qualify its own mountpoint.
        if self._related(self._rows()): raise Rejected("a mount already covers the selected point")
        self._check_empty()
        if (tuple(self._identity(fd) for fd in self.fds) != self.chain
                or self.chain[-2][2] != self.parent
                or os.readlink(str(self.fds[-1]), dir_fd=self.proc.fds) != str(self.point)):
            raise Rejected("the originally held mountpoint changed")
        self._protected_parent()
        _distinct_roots(self.roots)
        # STORE may be remote. Use only its already retained identity and spelling.
        if (self.point == self.store or self.point in self.store.parents or self.store in self.point.parents
                or self.store_identity in tuple(part[:2] for part in self.chain)):
            raise Rejected("the point overlaps the original archive store")
        if self._related(self._rows()): raise Rejected("the original point is no longer vacant")

    def claim(self):
        fcntl.flock(self.fds[-1], fcntl.LOCK_EX | fcntl.LOCK_NB)
        self.before()

    def _protected_parent(self):
        # Check the actual ancestor chain without opening the live FUSE root.
        with ExitStack() as held:
            owner = os.fstat(self.fds[-2]).st_uid
            if owner not in (0, self.uid):
                raise Rejected("the original mountpoint parent changed ownership")
            parent, chain = owned_directory(held, self.point.parent, owner, self.proc.read,
                                            metadata_only=True)
            if (tuple(chain) != self.chain[:-1] or self._identity(parent) != self.chain[-2]
                    or self._identity(self.fds[-1]) != self.chain[-1]
                    or stat.S_IMODE(os.fstat(self.fds[-1]).st_mode) != 0o700
                    or os.readlink(str(self.fds[-1]), dir_fd=self.proc.fds) != str(self.point)):
                raise Rejected("the exact private mountpoint parent changed")

    def protected_active(self, receipt):
        self.present(receipt)
        self._protected_parent()
        self.present(receipt)

    def present(self, receipt=None):
        related = self._related(self._rows(), receipt)
        if len(related) != 1: raise Rejected("no unique exact editable mount receipt")
        left, right = related[0]
        system = set(right[2].split(b","))
        permissions = {b"default_permissions", f"user_id={self.uid}".encode(), f"group_id={self.gid}".encode()}
        if (left[0] == self.parent or left[1] != self.parent or left[3:5] != [b"/", self.path_key]
                or left[6:] not in ([], [b"unbindable"])
                or not (right[0] == b"fuse" or right[0].startswith(b"fuse."))
                or right[1] != self.source.encode("ascii")
                or not _KERNEL_FLAGS <= set(left[5].split(b",")) or b"ro" in left[5].split(b",")
                or not permissions <= system or b"allow_other" in system
                or (receipt is not None and left[0] != receipt.mount_id)):
            raise Rejected("the original editable FUSE mount or restrictions were not proved")
        return MountReceipt(self.point, self.source, left[0])

    def absent(self, receipt):
        if self._related(self._rows(), receipt):
            raise Rejected("the original mount may still be present or its place/ID has been reused")
        with ExitStack() as hold:
            fd, chain = owned_directory(hold, self.point, self.uid, self.proc.read, metadata_only=True)
            if (tuple(chain) != self.chain or self._identity(self.fds[-1]) != self.chain[-1]
                    or stat.S_IMODE(os.fstat(fd).st_mode) != 0o700
                    or os.readlink(str(fd), dir_fd=self.proc.fds) != str(self.point)):
                raise Rejected("the originally held directory is not back at this mountpoint")
            with os.scandir(self.fds[-1]) as old:
                if next(old, None) is not None:
                    raise Rejected("the original empty mountpoint contents changed")
            if self._related(self._rows(), receipt):
                raise Rejected("the original mountpoint changed during detach verification")


@dataclass(slots=True)
class _Detach:
    receipt: MountReceipt
    done: Event = field(default_factory=Event)
    normal: bool = False


@dataclass(frozen=True, slots=True)
class _Pending:
    submission: str
    destination: str
    plan_sha256: str
    effects_sha256: str
    directory: tuple
    queues: tuple[str, ...]


class _StoppedWork:
    """One same-worker cut with verified retained local Submit, never a transport."""
    def __init__(self, owner, journal, detached, controller):
        self._owner, self._journal, self._detached = owner, journal, detached
        self._controller, self._outbox = controller, controller._outbox
        self._registry = self._outbox.registry
        self._thread, self._used, self.complete = get_native_id(), False, False
        self._capturing = False
        self._effects = self._pending = None

    @contextmanager
    def freeze(self):
        if (self._owner is None or not self._capturing or self._used
                or get_native_id() != self._thread):
            raise Rejected("only this native Submit may borrow the stopped original cut")
        self._used = True
        self._detached()
        with self._owner.freeze() as cut:
            effects = hashlib.sha256(b"workspace-linux-cut/1\n")
            def add(kind, row):
                raw = canonical(row)
                effects.update(kind + len(raw).to_bytes(4, "big"))
                effects.update(raw)
            for event in cut.namespace_events:
                add(b"E", asdict(event))
            for bound, stamp in cut._changed.values():
                add(b"I", (bound.path, stamp))
            for bound in cut.created_files:
                add(b"C", bound.local_identity)
            yield cut
            self._detached()
        self._effects = effects.hexdigest()
        self.complete = True

    def _require_outbox(self, destination):
        from .desktop.container_outbox import NativeOutbox
        self._controller._require_original()
        if (self._controller._outbox is not self._outbox or self._outbox.registry is not self._registry
                or type(self._registry) is not NativeOutbox or self._outbox.state != self._journal.live.state
                or self._registry._state_identity != self._journal.state_identity
                or destination != self._controller._destination or destination != self._registry.destination_identity):
            raise Rejected("the exact connected controller no longer owns this original edit STATE outbox")

    def capture_linux(self, *, state, submission, destination, originals, admit_created=False, branch_intents=()):
        if (self._owner is None or self._used or get_native_id() != self._thread
                or state != self._journal.live.state or originals is not self._controller._originals):
            raise Rejected("capture must use the original worker, STATE and pre-edit bindings")
        self._require_outbox(destination)
        from .linux_submit import capture_linux_submit
        self._capturing = True
        try:
            actual = capture_linux_submit(state, self, submission=submission, destination=destination,
                                          originals=originals, admit_created=admit_created,
                                          branch_intents=branch_intents)
        finally:
            self._capturing = False
        if actual != submission or not self.complete:
            raise Rejected("no durable Submit from the complete stopped cut")
        self._pending = self._inspect(submission, destination)
        return actual

    def _inspect(self, submission, destination, *, running=True):
        from .container_spool import reopen_submit
        self._detached()
        if running: self._owner.check_settled_effects()
        with reopen_submit(self._journal.live.state, submission, destination=destination) as pending:
            directory = self._journal.pending_identity(submission, pending._area)
            for action in pending.plan.actions:
                if action.kind == "save_node":
                    # The trusted bridge already read back every sealed byte.
                    # Reopen each recorded immutable file and require its
                    # original fingerprint without another full-body copy.
                    with pending.body(action):
                        pass
            return _Pending(submission, destination, hashlib.sha256(pending.plan.encode()).hexdigest(),
                            self._effects, directory, tuple(key for key, _ in pending.plan.queues()))

    def settle(self, result):
        from .desktop.linux_container_controller import PreparedQueues
        original = self._pending
        if type(result) is not PreparedQueues or original is None or (
                result.submission, result.plan_sha256) != (original.submission, original.plan_sha256):
            raise Rejected("no exact registered submission for this stopped local cut")
        if result.blocked or result.ready != original.queues:
            raise Rejected("本次已保留确切待决，但有队列尚未登记；不清除 RUNNING")

        def verify(*, running=True):
            self._require_outbox(original.destination)
            if self._inspect(original.submission, original.destination, running=running) != original:
                raise Rejected("the original pending plan or its physical directory changed")
            # PreparedQueues is a display value, not authority. prefix()
            # independently requires the actual durable READY row, the exact
            # selected spool digest and a valid prefix under this same lease.
            for container in original.queues:
                with self._registry.hold(container):
                    self._registry.prefix(original.submission, container)
            self._require_outbox(original.destination)

        proof = asdict(original)
        del proof["queues"]  # Already bound by the canonical plan digest.
        self._journal.finish_pending(proof, verify_before=verify, verify_after=lambda: verify(running=False))

    def close(self):
        self._owner = None


class EditMountChoice:
    """Same reviewed window and point; run/stop may block and belong on workers."""
    def __init__(self, live, spelling):
        if not spelling.isprintable(): raise Rejected("请选可直接显示的挂载路径")
        self._origin, self._spelling = live, str(point_path(spelling))
        self._lock, self._held = Lock(), ExitStack()
        self._live = self._point = self._owner = self._receipt = self._stop = None
        self._helper = self._detach = self._controller = None
        self._running = self._closed = False

    def review(self):
        with self._lock, ExitStack() as pending:
            if self._closed or self._live is not None:
                raise Rejected("本次可编辑卷选择已失效")
            proc = ProcView(pending)
            uid = _ordinary_user(read_proc=proc.read)
            live = self._origin.borrow()
            pending.callback(live.close)
            point = _Point(pending, self._spelling, live, proc, uid)
            point.before()
            if self._origin.closed:
                raise Rejected("原窗口工作树已关闭")
            self._held = pending.pop_all()
            self._live, self._point = live, point
            return (f"原本机 WORK：{live.work}\n既有私有空挂载点：{point.point}\n"
                    f"本次来源：{point.source}\n普通保存只改此 WORK；不会联网或自动提交。"
                    "仅选本人独占的 WORK 和挂载点，不在会话期间绕过程序替换、重挂或写入。")

    @property
    def receipt(self):
        with self._lock: return self._receipt

    def stop(self, receipt, *, confirmed=False):
        """Stop editing without preparing a submission."""
        return self._stop_original(receipt, None, confirmed)

    def prepare_submit_and_stop(self, receipt, capture, *, confirmed=False):
        """This separate explicit Submit action may stage locally, not send."""
        if not callable(capture):
            raise Rejected("缺少本次原卷专用的本机提交准备回调")
        return self._stop_original(receipt, capture, confirmed)

    def _stop_original(self, receipt, capture, confirmed):
        with self._lock:
            if (confirmed is not True or self._owner is None or self._receipt is not receipt
                    or self._stop is not None or (capture is not None and self._controller is None)):
                raise Rejected("缺少本次原卷停止或准备提交的明确确认")
            self._point.present(receipt)
            writers = self._owner.stop_writes()
            self._stop = (receipt, capture)
            return writers  # Advisory; not a drain, detach or submission receipt.

    def _accepted(self, owner):
        with self._lock:
            receipt = self._point.present()
            self._point.protected_active(receipt)
            self._receipt, self._owner = receipt, owner

    def detach(self, receipt, *, confirmed=False):
        """One explicit, normal native unmount of this exact already stopped volume."""
        with self._lock:
            if (confirmed is not True or self._owner is None or self._receipt is not receipt
                    or self._stop is None or self._detach is not None):
                raise Rejected("没有本次已停写原卷的明确卸载授权")
            self._owner.quiesce_for_detach()
            self._point.protected_active(receipt)
            ticket = self._detach = _Detach(receipt)
            helper = self._helper
        try:
            code = helper.unmount(self._point.point)
            if code != 0:
                # Busy/failed is only repeatable on a later explicit action if
                # the same original mount is still present and serving.
                self._point.protected_active(receipt)
                with self._lock:
                    if self._owner is not None and self._detach is ticket: self._detach = None
                raise Rejected(f"本次正常卸载未完成（{code}）；关掉占用后可再核对原卷")
            self._point.absent(receipt)
            ticket.normal = True
        finally:
            ticket.done.set()

    def _wait_detach(self):
        # Keep original point/helper descriptors alive even if dispatch raises
        # while the separately authorized normal detach is in flight.
        with self._lock:
            self._owner = None
            ticket = self._detach
        if ticket is not None: ticket.done.wait()
        return ticket

    def _ended(self):
        ticket = self._wait_detach()
        stopped, receipt = self._stop, self._receipt
        if ticket is None or ticket.receipt is not receipt or not ticket.normal or stopped is None:
            raise Rejected("absence alone is not a normal detach of this original receipt")
        self._point.absent(receipt)
        return stopped

    def run(self, *, confirmed=False, exclusive=False, controller=None):
        with self._lock:
            if (confirmed is not True or exclusive is not True or self._closed or self._running
                    or self._live is None or self._origin.closed):
                raise Rejected("缺少原工作树与空挂载点的独占保管及本次明确授权")
            if controller is not None:
                from .desktop.linux_container_controller import LinuxContainerController
                if type(controller) is not LinuxContainerController or controller._origin is not self._origin:
                    raise Rejected("提交控制器不是本窗口原工作树")
                controller._require_original()
            self._controller = controller
            self._running = True
        try:
            with ExitStack() as scope:
                self._point.claim()
                owner = scope.enter_context(edit_live_worktree(self._live, exclusive=exclusive))
                try: from ._fuse3_edit import qualify_edit_driver, serve_edit
                except (ImportError, OSError) as err:
                    raise Rejected("同一环境缺少可编辑卷的 FUSE3 组件；未安装任何组件") from err
                qualify_edit_driver()
                self._helper = scope.enter_context(installed_fuse(scope, self._point.proc))
                if controller is not None:
                    active, worker = True, get_native_id()

                    def bind(path, fd):
                        if not active or get_native_id() != worker:
                            raise Rejected("the original pre-edit binder expired")
                        return owner.bind_retained(path, fd)
                    try: controller.bind_originals(bind)
                    finally: active = False

                def last_check():
                    if self._origin.closed: raise Rejected("原窗口工作树已关闭")
                    owner.check_mount_start()
                    self._point.before()

                journal = scope.enter_context(running_state(self._live, owner))
                owner.attach_running_state(journal)
                try:
                    receipt, capture = serve_edit(
                        owner, lambda: self._helper.mount(self._point.point, self._point.source, EDIT_FLAGS),
                        self._point.source, last_check, lambda: self._accepted(owner), self._ended)
                    owner.require_dispatch_drained()
                    if capture is None:
                        if not owner.finish_unchanged_mount():
                            raise Rejected("编辑已停止；保留 RUNNING 和原件，未准备提交")
                        return None
                    task = _StoppedWork(owner, journal, lambda: self._point.absent(receipt), controller)
                    try:
                        result = capture(task)  # Local durable pending and registrations only.
                        task.settle(result)
                        return result
                    finally:
                        task.close()
                finally:
                    self._wait_detach()
        finally:
            with self._lock:
                self._owner = None
                self._running, self._closed = False, True
                self._held.close()

    def close(self):
        """Before grant: revoke. Running: refuse discard, no kill or path-unmount."""
        with self._lock:
            if self._running: return False
            if not self._closed:
                self._closed = True
                self._held.close()
            return True
