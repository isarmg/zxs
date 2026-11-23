"""Qt-main-thread adapter to the optional statically linked, signed 27+ host. No public enable switch."""
import ctypes as C
from dataclasses import dataclass
from enum import IntEnum
import os
import sys

from ..macos_fskit_registration import SelectionRequest
from ..model import Rejected


class MountPhase(IntEnum):
    READY = 1
    ATTACHING = 2
    UNKNOWN = 3
    MOUNTED = 4
    STOPPING = 5
    GONE_WAITING_PROVIDER = 6
    LOCAL_NORMAL = 7
    DETACHING = 8
    STOPPED_READY = 9  # Kernel normal unmount has not yet succeeded.


@dataclass(frozen=True, slots=True)
class MountStatus:
    phase: MountPhase
    errno: int
    mounted_path: str | None  # Display only; never a path-based detach or Submit capability.


def _symbols():
    if sys.platform != "darwin":
        raise Rejected("this native drive requires macOS 27")
    main = C.CDLL(None, mode=C.RTLD_LOCAL | 0x100)  # RTLD_FIRST: exact main executable, not every loaded image.
    try:
        session, action, status = main.zxs_fskit_session, main.zxs_fskit_action, main.zxs_fskit_status
    except AttributeError as exc:
        raise Rejected("this distribution has no signed FSKit host") from exc
    session.argtypes = [C.c_char_p, C.c_char_p, C.POINTER(C.c_uint64)]
    action.argtypes = [C.c_uint64, C.c_int32]
    status.argtypes = [C.c_uint64, C.POINTER(C.c_int32), C.POINTER(C.c_int32), C.POINTER(C.c_char), C.c_ssize_t]
    session.restype = action.restype = status.restype = C.c_int32
    return session, action, status


def _ok(code):
    if code:
        raise OSError(code, os.strerror(code))


class OriginalDrive:
    def __init__(self, request, registration):
        if type(request) is not SelectionRequest:
            raise Rejected("the signed host requires the original prepared selection")
        request._authority(registration)
        create, self._action, self._status = _symbols()
        key = C.c_uint64()
        _ok(create(registration.token.encode("ascii"), registration.digest.encode("ascii"), C.byref(key)))
        if not key.value:
            raise Rejected("native host did not reserve an original session")
        self._key, self.request, self.registration = key.value, request, registration

    def attach(self, *, confirmed=False):
        if confirmed is not True:
            raise Rejected("explicit mount of this registered original is required")
        self.request._authority(self.registration)
        _ok(self._action(self._key, 1))  # Queued only. Do not announce attachment before MOUNTED.

    def status(self):
        phase, error, path = C.c_int32(), C.c_int32(), C.create_string_buffer(1024)
        _ok(self._status(self._key, C.byref(phase), C.byref(error), path, len(path)))
        kind = MountPhase(phase.value)
        if bool(path.value) != (kind in (MountPhase.MOUNTED, MountPhase.STOPPING, MountPhase.STOPPED_READY)):
            raise Rejected("incoherent response from the original native mount")
        return MountStatus(kind, error.value, os.fsdecode(path.value) if path.value else None)

    def stop_new_opens(self, *, confirmed=False):
        if confirmed is not True:
            raise Rejected("explicit stop of this original mount is required")
        _ok(self._action(self._key, 2))  # Local teardown remains possible after a network disconnect.

    def normal_unmount(self, *, confirmed=False):
        if confirmed is not True:
            raise Rejected("explicit normal unmount is required")
        _ok(self._action(self._key, 3))  # Busy remains stopped; never automatically retry, force, submit or purge.


    def release_unstarted(self):
        _ok(self._action(self._key, 4))  # No filesystem mutation: never release an attempted or uncertain original.

    def park_system_gone(self, *, confirmed=False):
        if confirmed is not True:
            raise Rejected("explicit parking of an already normally detached original is required")
        _ok(self._action(self._key, 5))
