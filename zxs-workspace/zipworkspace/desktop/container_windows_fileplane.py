"""One exact remote working file and its Windows-native local alias. No transfer or volume creation."""
import re
from threading import Lock

from ..model import Rejected
from .container_windows_draft import WindowsHeldDraft

READ, WRITE, APPEND = 1, 2, 4
ATTR, CONTROL, SYNC = 0x80, 0x20000, 0x100000
_ALLOWED = READ | WRITE | APPEND | ATTR | CONTROL | SYNC
_ROOT, _DATA, _ZONE = 1, 2, 3
_READ_EA, _TRAVERSE = 0x8, 0x20
_MARK = ":Zone.Identifier"
_DIRECTORY, _NON_DIRECTORY = 0x1, 0x40
# Sequential/random hints, synchronous alert/nonalert and the ordinary nondirectory constraint.
_OPTIONS = 0x4 | 0x10 | 0x20 | 0x800
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

    The separate scoped native table can request the single-entry root and physical origin
    stream. Native per-create checks, never this Python code, establish OS caller scope.
    """
    def __init__(self, draft, alias, *, confirmed=False):
        if (confirmed is not True or type(draft) is not WindowsHeldDraft
                or draft._choice.max_bytes > (1 << 63) - 4096):
            raise Rejected("confirm the existing original Windows working file and this local alias")
        self._draft, self._path = draft, "\\" + _alias(alias)  # Never derive a new server display name.
        self._lock, self._rows = Lock(), {}
        self._next, self._state, self._lease = 1, "ready", None

    def bind(self, lease, namespace=False):
        with self._lock:
            if self._state != "ready" or type(lease) is not object or type(namespace) is not bool:
                raise Rejected("Windows file plane was already claimed")
            # Pinned FSD bounds the full component *before* stripping the stream/type suffix.
            if namespace and len((self._path[1:] + _MARK + ":$DATA").encode("utf-16-le")) > 510:
                raise Rejected("choose a shorter Windows alias so the complete provenance stream remains addressable")
            self._lease, self._namespace = lease, namespace
            self._state = "binding"
            try:
                self._draft._claim_provider(self, project_origin=namespace)
                self._state = "bound"
            except BaseException:
                self._draft._abandon_provider(self, missing_ok=True)
                self._state = "fault"
                raise

    def _row(self, slot):
        if (self._state not in ("bound", "draining") or type(slot) is not int or slot not in self._rows):
            raise PermissionError("stale or unavailable Windows file context")
        return self._rows[slot]

    def _kind(self, path):
        if path == self._path:
            return _DATA
        if self._namespace:
            if path == "\\":
                return _ROOT
            # WinFsp normalizes the NT $DATA type before calling Open. Never case-fold a server name.
            if path == self._path + _MARK and self._draft._mark is not None:
                return _ZONE
        raise FileNotFoundError("not the selected Windows alias or its verified stream")

    def lookup(self, path):
        with self._lock:
            if not self._namespace or self._state != "bound":
                raise PermissionError("not accepting Windows name lookups")
            return self._draft._provider_info(self, self._kind(path))

    def open_file(self, path, options, granted):
        with self._lock:
            if self._state != "bound":
                raise PermissionError("not accepting Windows file opens")
            kind = self._kind(path)
            rights = (_ALLOWED if kind == _DATA else READ | ATTR | CONTROL | SYNC)
            if self._namespace:
                rights |= _READ_EA  # Generic read; this volume does not implement EAs.
            if kind == _ROOT:
                rights |= _TRAVERSE
            if type(options) is not int or type(granted) is not int or granted < 0 or granted & ~rights:
                raise PermissionError("unapproved Windows object access")
            if options & _DIRECTORY and kind != _ROOT:
                raise NotADirectoryError("not a Windows directory")
            if options & _NON_DIRECTORY and kind == _ROOT:
                raise IsADirectoryError("the Windows view root is not a file")
            if options >> 24 != 1 or options & 0xffffff & ~(_OPTIONS | _DIRECTORY | _NON_DIRECTORY):
                raise PermissionError("this Windows callback profile never creates or replaces")
            if len(self._rows) >= 1024 or self._next >= 1 << 63:
                raise MemoryError("Windows file-context budget exhausted")
            slot = self._next
            self._next += 1
            handle = self._draft.open(writable=bool(granted & (WRITE | APPEND)), _provider=self)
            try:
                info = self._draft._provider_info(self, kind)
                self._rows[slot] = (handle, granted, kind)
                return slot, info
            except BaseException:
                self._rows.pop(slot, None)
                self._draft.release(handle)
                raise

    def info(self, slot):
        with self._lock:
            _, _, kind = self._row(slot)
            return self._draft._provider_info(self, kind)

    def security_info(self, slot):
        with self._lock:
            _, rights, kind = self._row(slot)
            if not rights & CONTROL:
                raise PermissionError("read-control access required for a security descriptor")
            return self._draft._provider_info(self, kind)

    def directory(self, slot, marker):
        with self._lock:
            _, rights, kind = self._row(slot)
            if kind != _ROOT:
                raise NotADirectoryError("not the Windows volume root")
            if not rights & READ:
                raise PermissionError("list-directory access required")
            self._draft._provider_info(self, _ROOT)
            alias = self._path[1:]
            if marker is not None and alias.encode("utf-16-be") <= marker.encode("utf-16-be"):
                return None
            return alias, self._draft._provider_info(self, _DATA)

    def streams(self, slot):
        with self._lock:
            _, rights, kind = self._row(slot)
            if not rights & (READ | ATTR):
                raise PermissionError("read attributes or data required to enumerate Windows streams")
            data = self._draft._provider_info(self, _DATA)
            if kind == _ROOT:
                return ()  # No default $DATA on a directory and no hidden parent streams.
            if self._draft._mark is None:
                return (data,)
            return data, self._draft._provider_info(self, _ZONE)

    def read(self, slot, offset, length):
        with self._lock:
            handle, rights, kind = self._row(slot)
            if not rights & READ or kind == _ROOT:
                raise PermissionError("Windows file context has no read-data access")
            if kind == _ZONE:
                self._draft._span(offset, length)
                self._draft._provider_info(self, _ZONE)  # Reads and verifies the real held ADS and current policy.
                data = self._draft._mark[offset:offset+length]
            else:
                data = self._draft.read(handle, offset, length)
            if not data and length:
                raise EOFError("end of this original Windows stream")
            return data

    def write(self, slot, offset, data, append, constrained):
        with self._lock:
            handle, rights, kind = self._row(slot)
            if kind != _DATA or not (rights & WRITE or (rights & APPEND and append is True)):
                raise PermissionError("Windows file context does not grant this write")
            count = self._draft.write(handle, offset, data, append=append, constrained=constrained)
            return count, self._draft._provider_info(self)

    def resize(self, slot, length, allocation):
        with self._lock:
            handle, rights, kind = self._row(slot)
            if kind != _DATA or not rights & WRITE:
                raise PermissionError("write-data access required to change EOF")
            if allocation is not False:
                raise NotImplementedError("allocation-size changes are not EOF changes")
            self._draft.resize(handle, length)
            return self._draft._provider_info(self)

    def flush(self, slot):
        with self._lock:
            handle, _, kind = self._row(slot)
            self._draft.flush(handle)
            return self._draft._provider_info(self, kind)

    def close_file(self, slot):
        with self._lock:
            handle, _, _ = self._row(slot)
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

    def volume(self):
        # Does not disclose the private file length, alias, ID, account, path or origin.
        from .container_windows_mount import _capacity
        with self._lock:
            if not self._namespace or self._state not in ("bound", "draining"):
                raise PermissionError("no original Windows volume")
            self._draft._home.check()
            return _capacity(self._draft._home)

    def _finish_native(self, lease, proof):
        from .._windows_provider import selected_host
        with self._lock:
            if not selected_host().consume_certificate(proof, self, lease):
                raise Rejected("the original joined and deleted driver instance did not attest its drain")
            if self._lease is not lease or not self._namespace or self._state != "draining" or self._rows:
                raise Rejected("missing the original drained Windows namespace lease")
            self._draft._finish_provider(self)
            self._state, self._lease = "closed", None

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
