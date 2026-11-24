"""One exact remote working file and its Windows-native local alias. No transfer or volume creation."""
import re
from threading import Lock

from ..model import Rejected, new_id
from .container_windows_draft import WindowsHeldDraft

READ, WRITE, APPEND = 1, 2, 4
ATTR, CONTROL, SYNC = 0x80, 0x20000, 0x100000
_ALLOWED = READ | WRITE | APPEND | ATTR | CONTROL | SYNC
_ROOT, _DATA, _ZONE, _TEMP = 1, 2, 3, 4
_DELETE = 0x10000
_TEMP_ALIAS = re.compile(r"[a-zA-Z0-9._~-]{1,120}")
_READ_EA, _WRITE_EA, _TRAVERSE, _WRITE_ATTR = 0x8, 0x10, 0x20, 0x100
# Exact expanded FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE for the one
# nonshared CREATE_NEW. Never accept MAXIMUM_ALLOWED, or this combination on reopen.
_CREATOR_RIGHTS = _ALLOWED | _READ_EA | _WRITE_EA | _WRITE_ATTR | _DELETE
_BACKUP_INTENT, _COMPLETE_IF_OPLOCKED, _OPEN_REPARSE = 0x4000, 0x100, 0x00200000
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

    This table never reopens by displayed name. Native per-create checks, never
    this Python code, establish OS caller scope.
    """
    def __init__(self, draft, alias, *, confirmed=False, replacement=False):
        if (confirmed is not True or type(draft) is not WindowsHeldDraft
                or draft._choice.max_bytes > (1 << 63) - 4096 or type(replacement) is not bool):
            raise Rejected("confirm the existing original Windows working file and this local alias")
        self._draft, self._path = draft, "\\" + _alias(alias)  # Never derive a new server display name.
        self._retained_leaf = None
        self._scratch = None  # (virtual name, exact physical object); remains as evidence after promotion.
        if replacement:
            if not _TEMP_ALIAS.fullmatch(alias):
                raise Rejected("choose a simple ASCII local alias to admit an editor scratch file")
            self._retained_leaf = new_id() + ".zwd"
            draft._home.api.absent(draft._home.handle, self._retained_leaf)
        self._lock, self._rows = Lock(), {}
        self._next, self._state, self._lease = 1, "ready", None

    @property
    def retained_scratch(self):
        return None if self._retained_leaf is None else self._draft._home.name(self._retained_leaf)

    def bind(self, lease, namespace=False, replacement=False):
        with self._lock:
            if (self._state != "ready" or type(lease) is not object or type(namespace) is not bool
                    or type(replacement) is not bool or replacement != (self._retained_leaf is not None)
                    or (replacement and not namespace)):
                raise Rejected("Windows file plane already claimed, or requires the exact scoped callback table")
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
        if (self._state not in ("bound", "witnessing", "draining") or type(slot) is not int or slot not in self._rows):
            raise PermissionError("stale or unavailable Windows file context")
        return self._rows[slot]

    def _visible_scratch(self):
        if self._scratch is not None and self._draft._selected is not self._scratch[1]:
            return self._scratch
        return None

    def _object(self, path):
        body = self._draft._selected
        if path == self._path:
            return _DATA, body
        if self._namespace:
            if path == "\\":
                return _ROOT, None
            # The SDK normalizes the exact NT $DATA type. Never case-fold a wire name.
            if path == self._path + _MARK and self._draft._mark is not None:
                return _ZONE, body
            scratch = self._visible_scratch()
            if scratch is not None:
                name, body = scratch
                if path == name:
                    return _TEMP, body
                if path == name + _MARK and self._draft._mark is not None:
                    return _ZONE, body
        raise FileNotFoundError("not a selected Windows alias or its verified stream")

    def _info(self, kind, body):
        if kind == _TEMP and body is self._draft._selected:
            kind = _DATA  # A Rename handle continues to belong to the exact promoted object.
        return self._draft._provider_info(self, kind, backing=body)

    def lookup(self, path):
        with self._lock:
            if not self._namespace or self._state not in ("bound", "witnessing"):
                raise PermissionError("not accepting Windows name lookups")
            return self._info(*self._object(path))

    def _rights(self, kind, options, granted, *, target=False, create=False, probe=False, rename_writer=False):
        if type(options) is not int or type(granted) is not int or granted < 0:
            raise PermissionError("invalid Windows options or access mask")
        rights = _ALLOWED if kind in (_DATA, _TEMP) else READ | ATTR | CONTROL | SYNC
        if self._namespace:
            rights |= _READ_EA  # Generic read; this volume does not implement EAs.
            if kind in (_DATA, _TEMP):
                rights |= _WRITE_EA | _WRITE_ATTR  # Accept ordinary GENERIC_WRITE, but no EA/attribute edits.
            if kind == _TEMP and (rename_writer or (not create and options >> 24 == 1)):
                rights |= _DELETE  # A rename token only, never delete-on-close or disposition.
        if kind == _ROOT:
            rights |= _TRAVERSE
            if target:
                rights |= WRITE  # FILE_ADD_FILE bit on the SDK's exact target-parent request only.
        if type(rename_writer) is not bool or (rename_writer and
                (not self._namespace or not create or kind != _TEMP or options >> 24 != 2 or granted != _CREATOR_RIGHTS)):
            raise PermissionError("not the exact nonshared Windows scratch create")
        if granted & ~rights or (not rename_writer and granted & _DELETE and granted & (WRITE | APPEND)):
            raise PermissionError("unapproved Windows object access")
        if options & _DIRECTORY and kind != _ROOT:
            raise NotADirectoryError("not a Windows directory")
        if options & _NON_DIRECTORY and kind == _ROOT:
            raise IsADirectoryError("the Windows view root is not a file")
        allowed = _OPTIONS | _DIRECTORY | _NON_DIRECTORY
        if options & _OPEN_REPARSE:
            if (not self._namespace or kind not in (_DATA, _TEMP) or options >> 24 != 1 or target or probe
                    or not granted & ATTR or granted & ~(ATTR | SYNC)):
                raise PermissionError("only read attributes from the exact existing stream base")
            allowed |= _OPEN_REPARSE
        if probe:
            if (kind != _DATA or options >> 24 != 1 or not options & _COMPLETE_IF_OPLOCKED
                    or not granted & ATTR or granted & ~(ATTR | SYNC)):
                raise PermissionError("not the exact read-only WinFsp probe")
            allowed |= _COMPLETE_IF_OPLOCKED
        if self._namespace and kind == _ROOT:
            # Ordinary root handles may use BACKUP_SEMANTICS, not an elevated backup.
            allowed |= _BACKUP_INTENT
        dispositions = (2,) if create else (1, 3) if self._namespace and kind in (_DATA, _TEMP) else (1,)
        if options >> 24 not in dispositions or options & 0xffffff & ~allowed:
            raise PermissionError("only the exact admitted Windows create/open variant is supported")

    def _issue(self, kind, body, granted, *, rename_writer=False):
        if len(self._rows) >= 1024 or self._next >= 1 << 63:
            raise MemoryError("Windows file-context budget exhausted")
        slot = self._next
        self._next += 1
        writable = kind in (_DATA, _TEMP) and bool(granted & (WRITE | APPEND))
        handle = self._draft.open(writable=writable, _provider=self, _backing=body, _root=kind == _ROOT,
                                  _rename_writer=rename_writer)
        try:
            info = self._info(kind, body)
            self._rows[slot] = (handle, granted, kind, body)
            return slot, info
        except BaseException:
            self._rows.pop(slot, None)
            self._draft.release(handle)
            raise

    def open_file(self, path, options, granted, target=False, probe=False):
        with self._lock:
            if type(target) is not bool or type(probe) is not bool:
                raise PermissionError("invalid native open mode")
            if probe:
                if self._state != "witnessing" or target or path not in (self._path, self._scratch[0]):
                    raise PermissionError("not this original native probe")
            elif self._state != "bound":
                raise PermissionError("not accepting Windows file opens")
            kind, body = self._object(path)
            if target and (kind != _ROOT or self._visible_scratch() is None):
                raise PermissionError("not the original selected replacement parent")
            self._rights(kind, options, granted, target=target, probe=probe)
            if kind == _TEMP and body.creator is not None and granted & _DELETE:
                raise PermissionError("only the nonshared scratch's original creator may rename it")
            return self._issue(kind, body, granted)

    def scratch_name(self, path):
        # With both leaves ASCII, the required Windows case-insensitive disjointness is
        # unambiguous without applying ZIP filename rules or Unicode casefold to R3 names.
        if self._retained_leaf is None or not path.startswith("\\") or not _TEMP_ALIAS.fullmatch(path[1:]):
            return False
        try:
            return _alias(path[1:]).lower() != self._path[1:].lower()
        except Rejected:
            return False

    def permits_stream_main(self, path):
        with self._lock:
            scratch = self._visible_scratch()
            return bool(self._namespace and self._state == "bound"
                        and (path == self._path or (scratch is not None and path == scratch[0])))

    def permits_target(self, path, slot=None, old=None):
        with self._lock:
            scratch = self._visible_scratch()
            if self._state != "bound" or path != self._path or scratch is None:
                return False
            if slot is None:
                return True
            row = self._rows.get(slot)
            return bool(row is not None and old == scratch[0] and row[1] & _DELETE
                        and row[2] == _TEMP and row[3] is scratch[1]
                        and (row[3].creator is None or row[3].creator is row[0]))

    def create_scratch(self, path, options, granted, rename_writer=False):
        with self._lock:
            if self._state != "bound" or not self._namespace or not self.scratch_name(path):
                raise PermissionError("not an admitted one-time Windows scratch create")
            if self._scratch is not None:
                raise FileExistsError("only one original editor scratch name can be created")
            if len(self._rows) >= 1024:
                raise MemoryError("Windows file-context budget exhausted")
            self._rights(_TEMP, options, granted, create=True, rename_writer=rename_writer)
            if not granted & (WRITE | APPEND):
                raise PermissionError("scratch creation requires data writing")
            body = self._draft._new_scratch(self, self._retained_leaf)
            self._scratch = (path, body)
            return self._issue(_TEMP, body, granted, rename_writer=rename_writer)

    def replace(self, slot, old, new):
        with self._lock:
            key, rights, kind, body = self._row(slot)
            scratch = self._visible_scratch()
            if (self._state != "bound" or new != self._path or scratch is None or old != scratch[0]
                    or body is not scratch[1] or not rights & _DELETE or kind != _TEMP):
                raise PermissionError("only the original scratch can replace the selected Windows alias")
            self._draft._promote_scratch(self, key, body)

    def failed_replace_reply(self, slot):
        with self._lock:
            row = self._rows.get(slot)
            if row is not None:
                self._draft._fault_replacement_reply(self, row[3])

    def replacement_witness(self):
        with self._lock:
            if (self._state == "bound" and self._scratch is not None
                    and self._draft._selected is self._scratch[1]):
                if any(kind != _ROOT for _, _, kind, _ in self._rows.values()):
                    raise Rejected("close all editor and origin-stream contexts before observing replacement")
                self._state = "witnessing"  # Close all other admission before re-entering the original kernel.
                return self._path, self._scratch[0], self._scratch[1].index
            return None

    def confirm_replacement(self, alias, scratch, index):
        with self._lock:
            if (self._state != "witnessing" or self._scratch is None or alias != self._path
                    or scratch != self._scratch[0] or self._draft._selected is not self._scratch[1]):
                raise Rejected("the original replacement changed while independently reopening the alias")
            self._draft._observe_replacement(self, index)

    def info(self, slot):
        with self._lock:
            _, _, kind, body = self._row(slot)
            return self._info(kind, body)

    def security_info(self, slot):
        with self._lock:
            _, rights, kind, body = self._row(slot)
            if not rights & CONTROL:
                raise PermissionError("read-control access required for a security descriptor")
            return self._info(kind, body)

    def directory(self, slot, marker):
        with self._lock:
            _, rights, kind, _ = self._row(slot)
            if kind != _ROOT:
                raise NotADirectoryError("not the Windows volume root")
            if not rights & READ:
                raise PermissionError("list-directory access required")
            self._info(_ROOT, None)
            entries = [(self._path[1:], _DATA, self._draft._selected)]
            scratch = self._visible_scratch()
            if scratch is not None:
                entries.append((scratch[0][1:], _TEMP, scratch[1]))
            entries.sort(key=lambda entry: entry[0].encode("utf-16-be"))
            limit = marker.encode("utf-16-be") if marker is not None else b""
            return tuple((name, self._info(k, body)) for name, k, body in entries
                         if name.encode("utf-16-be") > limit)

    def streams(self, slot):
        with self._lock:
            _, rights, kind, body = self._row(slot)
            if not rights & (READ | ATTR):
                raise PermissionError("read attributes or data required to enumerate Windows streams")
            if kind == _ROOT:
                return ()
            data = self._info(_TEMP if self._visible_scratch() and body is self._scratch[1] else _DATA, body)
            if self._draft._mark is None:
                return (data,)
            return data, self._info(_ZONE, body)

    def read(self, slot, offset, length):
        with self._lock:
            handle, rights, kind, body = self._row(slot)
            if not rights & READ or kind == _ROOT:
                raise PermissionError("Windows file context has no read-data access")
            if kind == _ZONE:
                self._draft._span(offset, length)
                self._info(_ZONE, body)  # Verify actual NTFS stream and unchanged origin policy.
                data = self._draft._mark[offset:offset+length]
            else:
                data = self._draft.read(handle, offset, length)
            if not data and length:
                raise EOFError("end of this original Windows stream")
            return data

    def write(self, slot, offset, data, append, constrained):
        with self._lock:
            handle, rights, kind, body = self._row(slot)
            if kind not in (_DATA, _TEMP) or not (rights & WRITE or (rights & APPEND and append is True)):
                raise PermissionError("Windows file context does not grant this write")
            count = self._draft.write(handle, offset, data, append=append, constrained=constrained)
            return count, self._info(kind, body)

    def resize(self, slot, length, allocation):
        with self._lock:
            handle, rights, kind, body = self._row(slot)
            if kind not in (_DATA, _TEMP) or not rights & WRITE:
                raise PermissionError("write-data access required to change EOF")
            if allocation is not False:
                raise NotImplementedError("allocation-size changes are not EOF changes")
            self._draft.resize(handle, length)
            return self._info(kind, body)

    def flush(self, slot):
        with self._lock:
            handle, rights, kind, body = self._row(slot)
            if kind == _ROOT:
                raise NotImplementedError("directory flush is not file flush")
            self._draft.flush(handle, editor_data=kind == _TEMP and bool(rights & (WRITE | APPEND)))
            return self._info(kind, body)

    def close_file(self, slot):
        with self._lock:
            handle, _, _, _ = self._row(slot)
            self._draft.release(handle)
            del self._rows[slot]

    def stop_opening(self):
        with self._lock:
            if self._state not in ("bound", "witnessing", "draining"):
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
            if not self._namespace or self._state not in ("bound", "witnessing", "draining"):
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
