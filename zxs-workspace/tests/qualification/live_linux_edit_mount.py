"""Real editable-volume and exact first-submit qualification, no Qt or stubs."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack
import errno
import hashlib
import os
from pathlib import Path
import stat
import time

from zipworkspace.desktop.container_client import ContainerClient
from zipworkspace.desktop.container_connection import OpenConnection, _Authorization, _Metadata
from zipworkspace.desktop.container_management import Kind, Phase
from zipworkspace.desktop.initialize_choice import InitializeChoice
from zipworkspace.desktop.linux_container_controller import LinuxContainerController
from zipworkspace.desktop.linux_container_runtime import linux_write_profile
from zipworkspace.linux_edit_broker import EditMountChoice

_OWNER = 23100
_ROOT = Path("/mnt/zxs-acceptance-workspace")
_LEAF = "mounted-qualification.bin"
_BODY = b"complete body from the real editable volume \x00\xff\n" * 19
_HASH = hashlib.sha256(_BODY).hexdigest()
_TIMEOUT = 40


def require(condition, reason):
    if not condition:
        raise RuntimeError(reason)


def mount_rows(point):
    return [(left.split(b" "), right.split(b" "))
            for line in Path("/proc/thread-self/mountinfo").read_bytes().splitlines()
            for left, sep, right in (line.partition(b" - "),)
            if sep and left.split(b" ")[4] == os.fsencode(point)]


def prerequisites():
    m = _ROOT.lstat()
    rows = mount_rows(_ROOT)
    require(os.getuid() == os.geteuid() == _OWNER and os.getgid() == os.getegid() == _OWNER
            and _ROOT.resolve(strict=True) == _ROOT and m.st_uid == _OWNER
            and stat.S_IMODE(m.st_mode) == 0o700 and not any(_ROOT.iterdir())
            and len(rows) == 1 and rows[0][0][6:] == [] and rows[0][1][0] == b"ext4",
            "editable fixture needs an original private empty ext4 mount")
    p = Path("/dev/fuse").lstat()
    status = Path("/proc/self/status").read_text().splitlines()
    require(stat.S_ISCHR(p.st_mode) and os.major(p.st_rdev) == 10 and os.minor(p.st_rdev) == 229
            and "NoNewPrivs:\t0" in status and "CapEff:\t0000000000000000" in status,
            "exact FUSE device or ordinary subprocess privilege state unavailable")


def normal_detach(choice, receipt):
    deadline = time.monotonic() + _TIMEOUT
    while True:
        try:
            choice.detach(receipt, confirmed=True)
            return
        except OSError as exc:
            # A close can precede the kernel's normal RELEASE callback. Retry
            # only this pre-detach busy result, never a real helper failure.
            if exc.errno != errno.EBUSY or time.monotonic() >= deadline:
                raise
            time.sleep(0.05)


def mounted_body(point, choice, controller):
    workers = ThreadPoolExecutor(max_workers=1)
    result = workers.submit(choice.run, confirmed=True, exclusive=True, controller=controller)
    stopped = False
    attempted_detach = False
    try:
        deadline = time.monotonic() + _TIMEOUT
        while (receipt := choice.receipt) is None:
            require(time.monotonic() < deadline, "FUSE mount receipt timeout")
            if result.done():
                result.result()
                raise RuntimeError("editable dispatch ended before a live mount receipt")
            time.sleep(0.05)
        m = mount_rows(point)
        require(len(m) == 1 and m[0][0][0] == receipt.mount_id
                and m[0][1][0].split(b".", 1)[0] == b"fuse"
                and m[0][1][1] == receipt.source.encode("ascii")
                and {b"rw", b"nodev", b"nosuid", b"noexec"} <= set(m[0][0][5].split(b",")),
                "the exact kernel editable FUSE mount is missing")
        child = point / _LEAF
        fd = os.open(child, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
        try:
            require(os.write(fd, _BODY) == len(_BODY), "short write through original FUSE mount")
            os.fsync(fd)
        finally:
            os.close(fd)
        with child.open("rb") as file:
            require(file.read(len(_BODY) + 1) == _BODY, "mounted whole-file read differs")
        submission = controller.new_submission_id()
        choice.prepare_submit_and_stop(
            receipt, lambda stopped_work: controller.prepare_cut(stopped_work, submission=submission),
            confirmed=True)
        stopped = True
        attempted_detach = True
        normal_detach(choice, receipt)
        prepared = result.result(timeout=_TIMEOUT)
        require(prepared.submission == submission and len(prepared.ready) == 1 and not prepared.blocked
                and not mount_rows(point) and not any(point.iterdir()),
                "the original detached volume did not register exactly one original queue")
        return prepared
    finally:
        # Only normal detach of the same receipt is allowed even on a failure.
        # A failed or uncertain attempt is deliberately not retried by path.
        try:
            if not attempted_detach and not result.done() and (receipt := choice.receipt) is not None:
                if not stopped:
                    choice.stop(receipt, confirmed=True)
                normal_detach(choice, receipt)
        finally:
            workers.shutdown(wait=False, cancel_futures=True)


def run_mounted_first_save(*, exchange, fixture_login, fixture_password, expect):
    prerequisites()
    with ExitStack() as keep:
        credentials = _Authorization(fixture_login, fixture_password)
        keep.callback(credentials.revoke)
        # Use the supplied test trust anchors rather than replacing global trust.
        # This is the same native authenticated read and adoption path as the GUI.
        client = exchange(credentials)
        reader = OpenConnection(ContainerClient(_Metadata(client), client), credentials, client, fixture_login)
        expect("GET", "/zxs/v1/containers", 200, None)
        listed, _ = reader.establish(None)

        work, private, store, point = (_ROOT / name for name in ("work", "state", "store", "edit-mount"))
        for path in (work, private, store, point):
            path.mkdir(mode=0o700)
        initialization = InitializeChoice()
        keep.callback(initialization.close)
        initialization.pick_work(work)
        initialization.pick_private(private)
        initialization.pick_store(store)
        initialization.review("selected")
        initialization.run(confirmed=True)
        live = initialization.take_session()
        require(live is not None, "missing actual original registered WORK")
        keep.callback(live.close)

        choice = EditMountChoice(live, str(point))
        keep.callback(choice.close)
        choice.review()
        owner = keep.enter_context(linux_write_profile(listed, live, allowed=frozenset({Kind.SAVE}),
                                                       confirmed=True, initialize=True))
        active = listed.adopt_local(owner.profile, confirmed=True)
        controller = LinuxContainerController(active, owner.profile, live)
        keep.callback(controller.close)
        controller.confirm_empty_checkout(confirmed=True)
        prepared = mounted_body(point, choice, controller)

        work_file = work / _LEAF
        with work_file.open("rb") as file:
            require(file.read(len(_BODY) + 1) == _BODY, "original WORK data differs after normal detach")
        offers, complete = owner.first_saves(active)
        require(complete and len(offers) == 1, "expected one retained first-save offer")
        offer = offers[0]
        require(offer.submission == prepared.submission and (offer.container,) == prepared.ready
                and offer.name == _LEAF and offer.length == len(_BODY)
                and offer.sha256 == _HASH and offer.original is None,
                "the retained generated operation does not own the mounted body")
        ticket = owner.prepare_first_save(active, offer.submission, offer.container, confirmed=True)
        require(ticket.submission == offer.submission and ticket.operation == offer.operation,
                "original generated save ticket changed")
        root = "/zxs/v1/containers/" + offer.container
        lookup = root + "/operations/" + offer.operation
        expect("GET", lookup, 404, None)
        expect("POST", root + "/branches/main/nodes", 200, _BODY)
        result = active.operations.apply(ticket)
        require(result.phase is Phase.CONFIRMED and result.receipt is not None
                and result.receipt.sequence == 1 and result.receipt.node == offer.node
                and result.receipt.length == len(_BODY) and result.receipt.sha256 == _HASH,
                "explicit mounted-volume submission did not return the exact remote receipt")
        expect("GET", lookup, 200, None)
        again = active.operations.status(ticket)
        require(again.phase is Phase.CONFIRMED and again.receipt == result.receipt,
                "the same original mounted-volume receipt could not be recovered")
        expect("GET", root + "/nodes/" + offer.node + "/data", 200, None)
        received = bytearray()

        def sink(chunk):
            received.extend(chunk)
            return len(chunk)

        client.read_node_into(offer.container, offer.node, length=len(_BODY), sha256=_HASH, write=sink)
        require(received == _BODY, "mounted whole file did not survive exact server readback")
    print("ZXS-WORKSPACE-FUSE original editable kernel mount, normal detach and exact full R3 submit passed", flush=True)
