"""Private WinFsp directory-mount owner for one R3 draft. No network or implicit submit."""
import ctypes as c
import re
import struct
from threading import Event, Lock, current_thread, main_thread

from .._windows_access import ACL, ALLOW, SD, FILE_ALL
from .._windows_output import ATTR, OPEN, READ_CONTROL, SYNC, _ok
from ..model import Rejected, new_id
from ..windows_nt import I32, I64, P, U32, _Basic, _Id, _IO, _Object, _Standard, _String, _function
from .container_windows_fileplane import WindowsDraftFilePlane, _MARK

_DEVICE = re.compile(r"\\Device\\Volume\{[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}\}")


class _FullSize(c.Structure):
    _fields_ = [("total", I64), ("caller", I64), ("actual", I64), ("sectors", U32), ("bytes", U32)]


def _capacity(home):
    # Aggregate backing-volume capacity, NOT object size, promised reservation or remote quota.
    full, io = _FullSize(), _IO()
    status = home.api.volume_info(home.handle, c.byref(io), c.byref(full), c.sizeof(full), 7)
    unit = full.sectors * full.bytes
    if (status or io.result.status or io.information < c.sizeof(full) or not unit or unit & (unit-1)
            or not 0 <= full.caller <= full.actual <= full.total or full.total <= 0
            or full.total * unit >= 1 << 63):
        raise Rejected("cannot establish the original local NTFS capacity")
    return full.total * unit, full.caller * unit


def _reparse_record(device):
    if type(device) is not str or not _DEVICE.fullmatch(device):
        raise Rejected("not an original local WinFsp kernel disk identifier")
    name = (device + "\\").encode("utf-16-le")
    each = name + b"\0\0"
    return struct.pack("<I6H", 0xa0000003, 8+2*len(each), 0, 0, len(name), len(each), len(name)) + each*2


class WindowsPrivateDraftMount:
    """One original owner, one generated new leaf, one foreground choice. No cold adoption by name."""
    def __init__(self, draft, alias, *, confirmed=False, replacement=False):
        self._plane = WindowsDraftFilePlane(draft, alias, confirmed=confirmed, replacement=replacement)
        self._replace = replacement
        self.retained_scratch = self._plane.retained_scratch
        self._draft, self._home = draft, draft._home
        self._leaf = new_id() + ".zwm"
        self._native = self._home.name(self._leaf)
        self._shell = self._home.shell_name(self._leaf)
        if replacement and len((self._shell + "\\" + "x"*120 + _MARK + ":$DATA").encode("utf-16-le")) >= 520:
            raise Rejected("choose a shorter Windows private directory to leave room for editor/stream names")
        self._lock, self._ready = Lock(), Event()
        self._used = self._running = self._claimed = self._sealing = False
        self._mode = 0  # 0 active, 1 orderly stop, 2 explicitly abandon including outstanding references
        self._visible, self._identity, self.remaining, self._cleanup_lease = None, None, None, None
        self._check_parent()
        self._home.api.absent(self._home.handle, self._leaf)

    def _check_parent(self):
        self._draft._choice._live()
        self._home.check()
        self._draft._choice._protection.check()
        if (self._home.name(self._leaf), self._home.shell_name(self._leaf)) != (self._native, self._shell):
            raise Rejected("the approved private Windows mount parent or spelling changed")

    def _junction_sd(self):
        # Explicit protected ACL created ONLY on a newly generated SDK mount directory; never
        # modify the selected parent. Let Windows choose owner/group and verify both after create.
        grants = self._draft._choice._protection.original.grants
        aces = b"".join(ALLOW.pack(0, 3, ALLOW.size + len(sid), FILE_ALL) + sid for sid, _ in grants)
        acl = ACL.pack(2, 0, ACL.size + len(aces), len(grants), 0) + aces
        return SD.pack(1, 0, 0x9004, 0, 0, 0, SD.size) + acl

    def _claim(self, plane):
        self._check_parent()
        self._home.api.absent(self._home.handle, self._leaf)
        with self._lock:
            if plane is not self._plane or not self._running or self._claimed:
                raise Rejected("not this active one-shot Windows directory mount")
            if self._mode:
                return None
            self._claimed = True
            return self._native, self._junction_sd(), self._replace

    def _verify(self, plane, device, volume, fid):
        if plane is not self._plane or type(fid) is not bytes or len(fid) != 16 or not any(fid):
            raise Rejected("missing the actual WinFsp mount directory identity")
        required = _reparse_record(device)
        self._check_parent()
        H, api = self._home, self._home.api
        if type(volume) is not int or volume != H.volume:
            raise Rejected("the mount directory is not on the original held NTFS")
        # Open ONLY the relative, original reparse directory. Never follow it to validate itself.
        with api._request(H.handle, self._leaf, SYNC | ATTR | READ_CONTROL, 7, OPEN, 0) as witness:
            id_ = api._info(witness, 18, _Id)
            b, st = api._info(witness, 0, _Basic), api._info(witness, 1, _Standard)
            if (id_.volume != volume or bytes(id_.fid) != fid or not st.directory or not st.links
                    or b.attrs & 0x410 != 0x410 or b.attrs & 0x4000 or api.guid_name(witness) != self._native):
                raise Rejected("SDK mount handle and original directory-relative name no longer identify one junction")
            protection = self._draft._choice._protection
            access = protection.api.read(witness, protection.actor, directory=True)
            if (access.group != protection.original.group or not access.control & 0x1000 or
                    set(access.grants) != {(sid, 3) for sid, _ in protection.original.grants}):
                raise Rejected("the generated mount directory does not have the selected private grants")
            k = c.WinDLL("kernel32.dll", use_last_error=True, winmode=0x800)
            ioctl = _function(k, "DeviceIoControl", I32, P, U32, P, U32, P, U32, c.POINTER(U32), P)
            buf, used = c.create_string_buffer(16384), U32()
            if (not ioctl(witness, 0x000900a8, None, 0, buf, len(buf), c.byref(used), None)
                    or bytes(buf)[:used.value] != required):
                raise Rejected("the original WinFsp junction does not name precisely this kernel volume")
        with self._lock:
            now = (device, volume, fid)
            if not self._claimed or self._identity not in (None, now):
                raise Rejected("the original Windows mount identity changed")
            self._identity = now
        self._check_parent()

    def _observe_replace_name(self, plane, device):
        # Separate from the dispatcher's Rename callback. Even a returned callback does
        # not prove the FSD finished installing the new name on the original volume.
        if plane is not self._plane or not self._replace or not _DEVICE.fullmatch(device):
            raise Rejected("not this original native replacement owner")
        snapshot = plane.replacement_witness()
        if snapshot is None:
            return
        alias, scratch, index = snapshot
        self._check_parent()
        with self._lock:
            if self._identity is None or self._identity[0] != device:
                raise Rejected("the native lookup does not use this original virtual volume")
        k = c.WinDLL("kernel32.dll", use_last_error=True, winmode=0x800)
        n = c.WinDLL("ntdll.dll", winmode=0x800)
        open_ = _function(n, "NtOpenFile", I32, c.POINTER(P), U32, c.POINTER(_Object), c.POINTER(_IO), U32, U32)
        close = _function(k, "CloseHandle", I32, P)
        name = _function(k, "GetFinalPathNameByHandleW", U32, P, c.POINTER(c.c_wchar), U32, U32)

        def by_name(suffix):
            # The mount pair was independently pinned before this lookup. Do not wait
            # for a conflicting editor oplock: treat its alternate status as unconfirmed.
            raw = ("\\??\\" + self._native[4:] + suffix).encode("utf-16-le")
            buf = c.create_string_buffer(raw + b"\0\0")
            us = _String(len(raw), len(raw) + 2, c.addressof(buf))
            obj = _Object(c.sizeof(_Object), None, c.pointer(us), 0x40, None, None)
            h, io = P(), _IO()
            status = open_(c.byref(h), ATTR | SYNC, c.byref(obj), c.byref(io), 7,
                           0x100 | 0x40 | 0x20)  # COMPLETE_IF_OPLOCKED | NON_DIRECTORY | SYNCHRONOUS_NONALERT.
            return h.value, status, io

        actual, status, io = by_name(alias)
        if not actual:
            raise Rejected("could not independently open the current WinFsp alias")
        try:
            _ok("independently open the current WinFsp alias without waiting for an oplock", status, io)
            got_path = c.create_unicode_buffer(1024)
            got = name(actual, got_path, len(got_path), 2)  # VOLUME_NAME_NT, FILE_NAME_NORMALIZED.
            number, io = I64(), _IO()
            result = self._home.api.query_info(actual, c.byref(io), c.byref(number), c.sizeof(number), 6)
            _ok("query the independently reopened WinFsp file index", result, io)  # FileInternalInformation.
            if (not 0 < got < len(got_path) or got_path.value != device + alias
                    or io.information != c.sizeof(number) or number.value != index):
                raise Rejected("the actual original WinFsp target has not confirmed the promoted file index")
        finally:
            if not close(actual):
                raise c.WinError(c.get_last_error())
        old, status, io = by_name(scratch)
        if old:
            if not close(old):
                raise c.WinError(c.get_last_error())
            raise Rejected("the original temporary alias may still resolve")
        if status & 0xffffffff != 0xc0000034:  # STATUS_OBJECT_NAME_NOT_FOUND at the fixed volume/root only.
            raise Rejected("could not establish the original scratch alias is absent")
        self._check_parent()
        plane.confirm_replacement(alias, scratch, index)

    def _advertise(self):
        with self._lock:
            if self._identity is None or not self._running or self._mode:
                return False
            self._visible = self._shell
            self._ready.set()
            return True

    def _gone(self):
        self._check_parent()
        self._home.api.absent(self._home.handle, self._leaf)
        self._home.api.flush(self._home.handle)
        self._home.api.absent(self._home.handle, self._leaf)

    def _poll(self):
        with self._lock:
            return self._mode

    def _final_choice(self):
        # Linearization point *after* deletion and final physical absence. A force that won
        # earlier cannot create a clean cut; after this point the worker owns the final result.
        with self._lock:
            self._sealing = bool(self._running and self._claimed and self._identity and self._mode == 1)
            return self._sealing

    def _count(self, remaining):
        with self._lock:
            self.remaining = remaining

    def wait_location(self, timeout):
        if not self._ready.wait(timeout):
            return None
        with self._lock:
            return self._visible  # Past observation only, never authority to adopt/remove a path.

    def stop(self):
        with self._lock:
            self._mode = max(self._mode, 1)
            self._visible = None

    def abandon(self, *, confirmed=False):
        if confirmed is not True:
            raise Rejected("forced removal can lose outstanding editor writes; explicitly confirm")
        with self._lock:
            if not self._sealing:
                self._mode = 2
            self._visible = None
            return not self._sealing

    def release_fault(self):
        # A Python interruption can outlast joined/deleted native ownership. Retry only the
        # original per-fs lease retained before Create. This can never produce a normal cut.
        with self._lock:
            if self._running or not self._used:
                raise Rejected("wait for the original native owner to finish first")
            lease = self._cleanup_lease
        if lease is not None:
            self._plane.abandon(lease)
            with self._lock:
                if self._cleanup_lease is lease:
                    self._cleanup_lease = None

    def run(self, *, allow_installed_driver_start=False, allow_directory_mount=False):
        if (allow_installed_driver_start is not True or allow_directory_mount is not True
                or current_thread() is main_thread()):
            raise Rejected("confirm native volume/possible installed service start and private directory mount on a worker")
        with self._lock:
            if self._used:
                raise Rejected("this Windows directory mount was already used")
            self._used = self._running = True
        try:
            from .._windows_provider import selected_host
            result = selected_host().run(self, self._plane, True, True)
            if not result["local_release_pending"]:
                self._cleanup_lease = None
            return result
        finally:
            with self._lock:
                self._visible, self._running = None, False
                self._ready.set()
