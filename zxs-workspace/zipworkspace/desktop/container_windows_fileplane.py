"""One exact Windows working-file callback target. No volume creation, transfer or public namespace."""
import re
from threading import Lock

from ..model import Rejected
from .container_windows_draft import WindowsHeldDraft

READ, WRITE, APPEND = 1, 2, 4
ATTR, CONTROL, SYNC = 0x80, 0x20000, 0x100000
_ALLOWED = READ | WRITE | APPEND | ATTR | CONTROL | SYNC
_DIRECTORY, _NON_DIRECTORY = 0x1, 0x40
# Sequential/random hints, synchronous alert/nonalert and the ordinary nondirectory constraint.
_OPTIONS = 0x4 | 0x10 | 0x20 | _NON_DIRECTORY | 0x800
_DEVICE = re.compile(r"CON|PRN|AUX|NUL|CLOCK\$|CONIN\$|CONOUT\$|(?:COM|LPT)[1-9¹²³]", re.ASCII | re.I)


def _alias(leaf):
    if (type(leaf) is not str or not leaf or leaf in (".", "..") or leaf.endswith((" ", "."))
            or any(ord(c) < 32 or c in '<>:"/\\|?*' for c in leaf)
            or _DEVICE.fullmatch(leaf.partition(".")[0].rstrip(" "))):
        raise Rejected("choose one ordinary, non-device local Windows display alias")
    try:
        units = len(leaf.encode("utf-16-le")) // 2
    except UnicodeError as error:
        raise Rejected("invalid UTF-16 Windows alias") from error
    if units > 255:
        raise Rejected("the local Windows alias exceeds one NTFS component")
    return leaf


class WindowsDraftFilePlane:
    """Owner of a bounded table of distinct original opens. Numeric IDs are never native pointers.

    This is the callable target for the native SDK file operations; it does not attest the
    Windows caller, publish a root/ADS, or perform an OS detach. No namespace is enabled.
    """
    def __init__(self, draft, alias, *, confirmed=False):
        if (confirmed is not True or type(draft) is not WindowsHeldDraft
                or draft._choice.max_bytes > (1 << 63) - 4096):
            raise Rejected("confirm the existing original Windows working file and this local alias")
        self._draft, self._path = draft, "\\" + _alias(alias)  # Never derive a new server display name.
        self._lock, self._rows = Lock(), {}
        self._next, self._state, self._lease = 1, "ready", None

    def bind(self, lease):
        with self._lock:
            if self._state != "ready" or type(lease) is not object:
                raise Rejected("Windows file plane was already claimed")
            self._lease = lease
            self._state = "binding"
            try:
                self._draft._claim_provider(self)
                self._state = "bound"
            except BaseException:
                self._draft._abandon_provider(self, missing_ok=True)
                self._state = "fault"
                raise

    def _row(self, slot):
        if (self._state not in ("bound", "draining") or type(slot) is not int or slot not in self._rows):
            raise PermissionError("stale or unavailable Windows file context")
        return self._rows[slot]

    def open_file(self, path, options, granted):
        with self._lock:
            if self._state != "bound":
                raise PermissionError("not accepting Windows file opens")
            if path != self._path:
                raise FileNotFoundError("not the one exact selected alias or stream")
            if type(options) is not int or type(granted) is not int or granted < 0 or granted & ~_ALLOWED:
                raise PermissionError("unapproved Windows file access")
            if options & _DIRECTORY:
                raise NotADirectoryError("the selected item is an ordinary remote file")
            if options >> 24 != 1 or options & 0xffffff & ~_OPTIONS:  # Only FILE_OPEN.
                raise PermissionError("this Windows callback profile never creates or replaces")
            if len(self._rows) >= 1024 or self._next >= 1 << 63:
                raise MemoryError("Windows file-context budget exhausted")
            slot = self._next
            self._next += 1
            handle = self._draft.open(writable=bool(granted & (WRITE | APPEND)), _provider=self)
            try:
                info = self._draft._provider_info(self)
                self._rows[slot] = (handle, granted)
                return slot, info
            except BaseException:
                self._rows.pop(slot, None)
                self._draft.release(handle)
                raise

    def info(self, slot):
        with self._lock:
            self._row(slot)
            return self._draft._provider_info(self)

    def read(self, slot, offset, length):
        with self._lock:
            handle, rights = self._row(slot)
            if not rights & READ:
                raise PermissionError("Windows file context has no read-data access")
            data = self._draft.read(handle, offset, length)
            if not data and length:
                raise EOFError("end of this original Windows file")
            return data

    def write(self, slot, offset, data, append, constrained):
        with self._lock:
            handle, rights = self._row(slot)
            if not (rights & WRITE or (rights & APPEND and append is True)):
                raise PermissionError("Windows file context does not grant this write")
            count = self._draft.write(handle, offset, data, append=append, constrained=constrained)
            return count, self._draft._provider_info(self)

    def resize(self, slot, length, allocation):
        with self._lock:
            handle, rights = self._row(slot)
            if not rights & WRITE:
                raise PermissionError("write-data access required to change EOF")
            if allocation is not False:
                raise NotImplementedError("allocation-size changes are not EOF changes")
            self._draft.resize(handle, length)
            return self._draft._provider_info(self)

    def flush(self, slot):
        with self._lock:
            handle, _ = self._row(slot)
            self._draft.flush(handle)
            return self._draft._provider_info(self)

    def close_file(self, slot):
        with self._lock:
            handle, _ = self._row(slot)
            self._draft.release(handle)
            del self._rows[slot]

    def stop_opening(self):
        with self._lock:
            if self._state not in ("bound", "draining"):
                raise Rejected("the Windows file plane cannot begin a local drain")
            self._state = "draining"
            count = self._draft.begin_drain(_provider=self)
            if count != len(self._rows):
                self._state = "fault"
                raise Rejected("native and backing file-context counts disagree")
            return count

    def fault(self):
        with self._lock:
            self._state = "fault"

    def abandon(self, lease):
        # Caller must first stop/join any native dispatcher. Even zero outstanding opens
        # is not Windows kernel cache/mapping/detach evidence. Never produce a submit cut.
        with self._lock:
            if lease is not self._lease:
                return  # A competing native binding never owned the original plane.
            self._draft._abandon_provider(self, missing_ok=True)  # Safe after an interrupted claim/release.
            self._rows.clear()
            self._state = "closed"
            self._lease = None
