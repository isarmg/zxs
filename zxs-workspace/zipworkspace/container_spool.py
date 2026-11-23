"""Durable, private per-container Submit intents. No socket or borrowed WORK body."""
from contextlib import contextmanager
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys

from .container_plan import Action, FullBody, Plan
from .document import native_identity
from .freeze import _distinct_roots
from .model import CHUNK, Rejected, canonical, check_id, new_id
from .private_candidates import CandidateDirectory, held_candidates
from .private_freeze import local_mount, mount_id
from .publish import rename_new_at

_INTENT = "submit.json"
_MAX_INTENT = 16 << 20
_SAFE = getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
_DIR = os.O_RDONLY | _SAFE | getattr(os, "O_DIRECTORY", 0)
_READ = os.O_RDONLY | _SAFE | getattr(os, "O_NONBLOCK", 0)
_WRITE = os.O_RDWR | _SAFE | os.O_CREAT | os.O_EXCL


def _linux():
    if not sys.platform.startswith("linux") or not getattr(os, "O_NOFOLLOW", 0) or not hasattr(os, "pread"):
        raise Rejected("the original per-file capture currently requires the Linux local adapter")


def _destination(value):
    if type(value) is not str or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise Rejected("invalid stable authenticated destination identity")
    return value


def _id(value):
    value = check_id(value)
    if value == "0" * 32:
        raise Rejected("zero is not a submit or container identity")
    return value


def _leaf(container_id):
    return "body-" + _id(container_id)


def _private(area, leaf, fd, expected=None):
    stamp = area.check_file(leaf, fd, expected)
    file = os.fstat(fd)
    if file.st_uid != os.geteuid() or stat.S_IMODE(file.st_mode) != 0o400:
        raise Rejected("a retained body or intent is not the private read-only original")
    return stamp


def _directory(path, fd, required_mount):
    m = os.fstat(fd)
    if m.st_uid != os.geteuid() or stat.S_IMODE(m.st_mode) != 0o700 or mount_id(fd) != required_mount:
        raise Rejected("not the original private same-mount submit directory")
    area = CandidateDirectory(path, fd, native_identity(m, stat.S_IFDIR))
    area.check_state()
    return area, m


def _write_all(fd, bytes_):
    left = memoryview(bytes_)
    while left:
        written = os.write(fd, left)
        if not 0 < written <= len(left):
            raise OSError("short private submit write")
        left = left[written:]


def _copy_original(area, container_id, image, cut):
    """Stay inside both original WORK guards for the one local copy."""
    leaf = _leaf(container_id)
    fd = os.open(leaf, _WRITE, 0o600, dir_fd=area.parent)
    try:
        image.check_file()
        if type(image.size) is not int or image.size < 0:
            raise Rejected("invalid original whole file size")
        written, digest = 0, hashlib.sha256()
        while written < image.size:
            part = os.pread(image.fd, min(CHUNK, image.size - written), written)
            if not part:
                raise Rejected("the original local file ended during capture")
            _write_all(fd, part)
            digest.update(part)
            written += len(part)
        if os.pread(image.fd, 1, written):
            raise Rejected("the original local file grew during capture")
        image.check_file()
        cut.check_work()
        os.fchmod(fd, 0o400)
        os.fsync(fd)
        fingerprint = _private(area, leaf, fd)
        sealed_hash = hashlib.sha256()
        offset = 0
        while offset < written:
            part = os.pread(fd, min(CHUNK, written - offset), offset)
            if not part:
                raise Rejected("the private whole file was shorter on readback")
            sealed_hash.update(part)
            offset += len(part)
        if (fingerprint[2] != written or sealed_hash.digest() != digest.digest()):
            raise Rejected("private sealed body differed from the original complete bytes")
        _private(area, leaf, fd, fingerprint)
        return FullBody(written, digest.hexdigest()), fingerprint
    finally:
        os.close(fd)


def _intent(submission, destination, plan, seals):
    return canonical({"format": 1, "submission": _id(submission), "destination": _destination(destination),
                      "plan": plan.encode().decode(), "seals": {key: list(fp) for key, fp in seals.items()}})


def _saved_actions(plan):
    return {action.container_id: action for action in plan.actions if action.kind == "save_node"}


def capture_submit(state, owner, *, submission, destination, select_files=None, build_plan, select_cut=None):
    """Stage one explicit plan; controller has already stopped admission of new writers.

    select_files(cut.changed_files) returns stable ContainerId -> exact cut member.
    It must use the same established local identity binding used by build_plan.
    build_plan(captured) consumes {ContainerId: (logical path, FullBody)} from
    stable restricted records; it does not access a borrowed FD or transport.
    A journal-aware controller supplies select_cut(cut) instead of select_files.
    Callers resume ordinary editing separately after this capture returns.
    """
    _linux()
    if (select_files is None) == (select_cut is None):
        raise Rejected("select files from exactly one cut observer")
    _id(submission)
    _destination(destination)
    final = submission + ".pending"
    temporary = ".capture-" + new_id()
    state = Path(state).absolute()
    with held_candidates(state) as spool:
        admitted_mount = local_mount(spool.parent)
        try:
            os.stat(final, dir_fd=spool.parent, follow_symlinks=False)
        except FileNotFoundError:
            pass
        else:
            raise Rejected("original Submit is already retained; reopen it, never recapture")
        child = None
        try:
            captured, outputs = {}, {}
            with owner.freeze() as cut:
                cut.check_work()
                _distinct_roots((cut._work_fd, spool.parent))
                members = cut.changed_files
                allowed = {id(member): member for member in members}
                selected = dict(select_cut(cut) if select_cut is not None else select_files(members))
                if (len({id(item) for item in selected.values()}) != len(selected)
                        or any(allowed.get(id(item)) is not item for item in selected.values())):
                    raise Rejected("capture request is not a subset of this exact changed-file cut")
                for container_id in selected:
                    _id(container_id)
                spool.check_state()
                os.mkdir(temporary, 0o700, dir_fd=spool.parent)
                child = os.open(temporary, _DIR, dir_fd=spool.parent)
                area, original = _directory(state / temporary, child, admitted_mount)
                for container_id, member in sorted(selected.items()):
                    with cut.open_file(member) as image:
                        if image.bound is not member:
                            raise Rejected("provider did not lend this exact file")
                        content, fingerprint = _copy_original(area, container_id, image, cut)
                    captured[container_id] = (member.path, content)
                    outputs[container_id] = fingerprint
            # The provider's final successful exit postcheck happened before
            # this point. No captured source descriptor survives it.
            plan = build_plan(captured.copy())
            if type(plan) is not Plan:
                raise Rejected("no immutable per-container submit plan")
            actions = _saved_actions(plan)
            placements = dict(plan.placements)
            for container_id, action in actions.items():
                if captured.get(container_id) != (placements.get(container_id), action.body):
                    raise Rejected("SaveNode is not the selected original whole file in this capture")
            for container_id in outputs.keys() - actions.keys():
                # Only discard the exclusive scratch file made above, before
                # any receipt or pending plan exists. Never touch WORK.
                leaf = _leaf(container_id)
                fd = os.open(leaf, _READ, dir_fd=child)
                try:
                    _private(area, leaf, fd, outputs[container_id])
                    os.unlink(leaf, dir_fd=child)
                finally:
                    os.close(fd)
            intent = _intent(submission, destination, plan, {key: outputs[key] for key in actions})
            if len(intent) > _MAX_INTENT:
                raise Rejected("private submit intent exceeds its supported metadata bound")
            fd = os.open(_INTENT, _WRITE, 0o600, dir_fd=child)
            try:
                _write_all(fd, intent)
                os.fchmod(fd, 0o400)
                os.fsync(fd)
                _private(area, _INTENT, fd)
            finally:
                os.close(fd)
            os.fsync(child)
            spool.check_state()
            area.check_state()
            rename_new_at(spool.parent, temporary, final, expected_source=original)
            committed_dir, _ = _directory(state / final, child, admitted_mount)
            os.fsync(spool.parent)
            spool.check_state()
            committed_dir.check_state()
            return submission
        finally:
            if child is not None:
                os.close(child)


@dataclass(frozen=True)
class PendingSubmit:
    submission: str
    destination: str
    plan: Plan
    _area: CandidateDirectory
    _seals: dict

    @contextmanager
    def body(self, action):
        """Lend a stream only from this exact action's sealed private file."""
        if type(action) is not Action or action.kind != "save_node" or action not in self.plan.actions:
            raise Rejected("this action has no retained complete body")
        leaf = _leaf(action.container_id)
        fingerprint = self._seals[action.container_id]
        fd = os.open(leaf, _READ, dir_fd=self._area.parent)
        try:
            _private(self._area, leaf, fd, fingerprint)

            def blocks():
                at, digest = 0, hashlib.sha256()
                while at < action.body.length:
                    _private(self._area, leaf, fd, fingerprint)
                    part = os.pread(fd, min(CHUNK, action.body.length - at), at)
                    if not part:
                        raise Rejected("sealed complete body ended before its recorded length")
                    digest.update(part)
                    at += len(part)
                    yield part
                _private(self._area, leaf, fd, fingerprint)
                if digest.hexdigest() != action.body.sha256:
                    raise Rejected("sealed body differed from the recorded exact full-file SHA")

            yield blocks()
            _private(self._area, leaf, fd, fingerprint)
        finally:
            os.close(fd)


@contextmanager
def reopen_submit(state, submission, *, destination):
    """Recover exact pending plan by ID. Never consult live WORK or recapture."""
    _linux()
    _id(submission)
    _destination(destination)
    final = submission + ".pending"
    state = Path(state).absolute()
    with held_candidates(state) as spool:
        admitted_mount = local_mount(spool.parent)
        directory = os.open(final, _DIR, dir_fd=spool.parent)
        try:
            area, _ = _directory(state / final, directory, admitted_mount)
            fd = os.open(_INTENT, _READ, dir_fd=directory)
            try:
                known = _private(area, _INTENT, fd)
                if known[2] > _MAX_INTENT:
                    raise Rejected("private submit intent exceeds its supported metadata bound")
                with os.fdopen(fd, "rb", closefd=False) as original:
                    raw = original.read(_MAX_INTENT + 1)
                _private(area, _INTENT, fd, known)
            finally:
                os.close(fd)
            try:
                intent = json.loads(raw)
                plan = Plan.decode(intent["plan"].encode())
                seals = {key: tuple(fp) for key, fp in intent["seals"].items()}
                actions = _saved_actions(plan)
                if (seals.keys() != actions.keys()
                        or any(len(fp) != 5 or not all(type(part) is int for part in fp)
                               or fp[2] != actions[key].body.length for key, fp in seals.items())
                        or _intent(submission, destination, plan, seals) != raw):
                    raise Rejected("not the exact original submit intent")
            except (TypeError, ValueError, KeyError, AttributeError) as exc:
                raise Rejected("cannot recover the recorded original submit intent") from exc
            # An uncertain final directory sync must be closed before any send.
            os.fsync(directory)
            os.fsync(spool.parent)
            spool.check_state()
            area.check_state()
            yield PendingSubmit(submission, destination, plan, area, seals)
        finally:
            os.close(directory)


def seal_metadata_submit(state, submission, destination, plan):
    """Publish an original no-body plan, never synthesize a WORK selection."""
    _linux()
    _id(submission)
    _destination(destination)
    if type(plan) is not Plan or not plan.actions or _saved_actions(plan):
        raise Rejected("metadata intent must not contain an opaque body")
    root = Path(state).absolute()
    final = submission + ".pending"
    scratch = ".capture-" + new_id()
    raw = _intent(submission, destination, plan, {})
    if len(raw) > _MAX_INTENT:
        raise Rejected("private submit intent exceeds its supported metadata bound")
    with held_candidates(root) as spool:
        mount = local_mount(spool.parent)
        os.mkdir(scratch, 0o700, dir_fd=spool.parent)
        child = os.open(scratch, _DIR, dir_fd=spool.parent)
        try:
            area, original = _directory(root / scratch, child, mount)
            fd = os.open(_INTENT, _WRITE, 0o600, dir_fd=child)
            try:
                _write_all(fd, raw)
                os.fchmod(fd, 0o400)
                os.fsync(fd)
                _private(area, _INTENT, fd)
            finally:
                os.close(fd)
            os.fsync(child)
            spool.check_state()
            area.check_state()
            rename_new_at(spool.parent, scratch, final, expected_source=original)
            exact, _ = _directory(root / final, child, mount)
            os.fsync(spool.parent)
            spool.check_state()
            exact.check_state()
        finally:
            os.close(child)
    return submission
