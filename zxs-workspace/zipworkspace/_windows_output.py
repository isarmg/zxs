"""Fixed-local NTFS output primitives. Not a platform capability offered by the CLI or Qt."""
from contextlib import ExitStack
import ctypes as c
import hashlib
import os

from .model import CHUNK, Rejected, member_name
from ._windows_access import READ_CONTROL
from ._windows_origin import DATA, ZONE, LIMIT, stream_lengths
from .windows_folder import drive_path
from .windows_nt import (Handle, Native, P, U16, U32, I32, I64, U64, _Object, _String,
                         _Standard, _IO, _function)

OPEN, CREATE, OPENED, CREATED = 1, 2, 1, 2
SYNC, ATTR, READ, WRITE, DELETE = 0x100000, 0x80, 1, 2, 0x10000
# No FILE_DIRECTORY_FILE on existing directories: no-reparse/no-recall are required.
FLAGS = 0x20 | 0x00200000 | 0x00400000
FILE_ONLY = 0x40
EFS = 0x4000  # FILE_ATTRIBUTE_ENCRYPTED; cross-directory EFS policy is not qualified.


class _Rename(c.Structure):
    _fields_ = [("replace", U32), ("parent", P), ("length", U32), ("first", U16)]


class _NTError(Rejected):
    def __init__(self, action, status):
        self.code = status & 0xffffffff
        super().__init__(f"{action}: NTSTATUS {self.code:08x}")


def _ok(action, status, io):
    if status or io.result.status:
        raise _NTError(action, status or io.result.status)


def leaf(name):
    if len(member_name(name)) != 1:
        raise Rejected("expected one exact ordinary filename")
    return name


class OutputNative(Native):
    """Only the separate future exporter resolves these write APIs; normal readers do not."""
    def __init__(self):
        if os.name != "nt": raise Rejected("fixed local Windows NTFS is required")
        super().__init__()
        n = c.WinDLL("ntdll.dll", use_last_error=True, winmode=0x800)
        k = c.WinDLL("kernel32.dll", use_last_error=True, winmode=0x800)
        self.set_info = _function(n, "NtSetInformationFile", I32, P, c.POINTER(_IO), P, U32, I32)
        self.query_info = _function(n, "NtQueryInformationFile", I32, P, c.POINTER(_IO), P, U32, I32)
        self.flush_ex = _function(n, "NtFlushBuffersFileEx", I32, P, U32, P, U32, c.POINTER(_IO))
        self.write_file = _function(k, "WriteFile", I32, P, P, U32, c.POINTER(U32), P)

    def _request(self, parent, name, access, share, disposition, options):
        b = name.encode("utf-16-le")
        buf = c.create_string_buffer(b + b"\0\0")
        us = _String(len(b), len(b)+2, c.addressof(buf))
        # FileRenameInformation inherits case matching from the source Open, not the parent.
        # Including the original FILE_CREATE here protects a later case-only target collision.
        obj = _Object(c.sizeof(_Object), parent, c.pointer(us), 0x40, None, None)  # OBJ_CASE_INSENSITIVE
        h, io = P(), _IO()
        code = self.create(c.byref(h), access, c.byref(obj), c.byref(io), None, 0, share,
                           disposition, FLAGS | options, None, 0)
        if code < 0: raise _NTError("open original or create new file", code)
        if not h.value: raise Rejected("native call did not return a Windows handle")
        handle = Handle(self, h.value)
        try:
            _ok("synchronous native open", code, io)
            if io.information != (CREATED if disposition == CREATE else OPENED):
                raise Rejected("native call did not return the requested existing/new object")
            return handle
        except BaseException:
            handle.close()
            raise

    def directory(self, parent, name, output):
        # LIST_DIRECTORY participates in sharing. Pin each live ancestor without DELETE share.
        # FILE_ADD_FILE on the actual output directory also permits its normal NTFS flush request.
        return self._request(parent, name if parent is None else leaf(name),
                             SYNC | ATTR | READ | 0x20 | ((WRITE | READ_CONTROL) if output else 0), 3, OPEN, 0)

    def input_file(self, parent, name):
        return self._request(parent, leaf(name), SYNC | ATTR | READ, 1, OPEN, FILE_ONLY)

    def new_file(self, parent, name):
        # FILE_CREATE never opens an occupied leaf. Deny others data read/write/delete access.
        return self._request(parent, leaf(name), SYNC | ATTR | READ | WRITE | DELETE | READ_CONTROL, 0, CREATE, FILE_ONLY)

    def inspected_file(self, parent, name):
        # Existing-only after the security service releases the object. No create/write/delete.
        return self._request(parent, leaf(name), SYNC | ATTR | READ | READ_CONTROL, 0, OPEN, FILE_ONLY)

    def witness(self, parent, name):
        # Metadata only, sharing all access used by the original still-open data handle.
        return self._request(parent, leaf(name), SYNC | ATTR, 7, OPEN, 0)

    def absent(self, parent, name):
        try:
            with self.witness(parent, name): pass
        except _NTError as err:
            if err.code in (0xc0000034, 0xc000000f): return  # Missing exact leaf in held parent.
            raise
        raise FileExistsError("the requested output name is already occupied")

    def ordinary(self, h, volume, wanted_name):
        actual_volume, row = self.info(h)
        if (actual_volume != volume or row.directory or row.attributes & EFS or self._info(h, 1, _Standard).links != 1 or
                self.guid_name(h) != wanted_name):
            raise Rejected("not the original ordinary single-link named NTFS file")
        return row

    def streams(self, primary):
        # Fixed local NTFS, synchronous original: refuse overflow or a partial/unknown shape.
        buf, io = (U64 * (LIMIT // 8))(), _IO()
        rc = self.query_info(primary, c.byref(io), c.byref(buf), c.sizeof(buf), 22)
        _ok("enumerate original NTFS streams", rc, io)
        if io.information > c.sizeof(buf): raise Rejected("invalid NTFS stream extent")
        return stream_lengths(bytes(buf)[:io.information])

    def only_default_stream(self, primary, size):
        if self.streams(primary) != {DATA:size}:
            raise Rejected("new output already carries other data streams")

    # The related FILE_OBJECT for an ADS can be the already open primary data stream.
    # Never open an adjacent pathname or accept an archive-controlled stream name.
    def source_zone(self, primary):
        try: return self._request(primary, ZONE, SYNC | ATTR | READ, 1, OPEN, FILE_ONLY)
        except _NTError as err:
            if err.code in (0xc0000034, 0xc000000f):
                raise Rejected("unmarked archive: local storage alone is not proof of trust") from err
            raise

    def new_zone(self, primary):
        # DELETE sharing is required by our already-held primary DELETE handle; data writes
        # by another opener are denied. FILE_CREATE only: never replace or repair a mark.
        return self._request(primary, ZONE, SYNC | ATTR | READ | WRITE, 5, CREATE, FILE_ONLY)

    def zone_witness(self, primary):
        return self._request(primary, ZONE, SYNC | ATTR | READ, 7, OPEN, FILE_ONLY)

    def require_case_insensitive(self, h):
        flags = U32(0xffffffff)
        if not self.file_info(h, 23, c.byref(flags), c.sizeof(flags)) or flags.value:
            raise Rejected("cannot prove an ordinary case-insensitive NTFS output directory")

    def flush(self, h):
        io = _IO()
        rc = self.flush_ex(h, 0, None, 0, c.byref(io))  # Normal file/data/metadata/device flush.
        _ok("flush original file or original NTFS directory", rc, io)

    def write(self, h, data):
        if not 0 < len(data) <= CHUNK: raise Rejected("invalid native write span")
        got = U32()
        if not self.write_file(h, c.c_char_p(data), len(data), c.byref(got), None):
            raise c.WinError(c.get_last_error())
        if got.value != len(data): raise Rejected("incomplete native write; nothing was retried")

    def _set_offset(self, h, cls, offset):
        if type(offset) is not int or not 0 <= offset < 1 << 63:
            raise Rejected("invalid signed NTFS file offset")
        io, value = _IO(), I64(offset)
        rc = self.set_info(h, c.byref(io), c.byref(value), c.sizeof(value), cls)
        _ok("position or resize held data stream", rc, io)

    def position(self, h, offset):
        self._set_offset(h, 14, offset)  # FilePositionInformation; synchronous handle.

    def resize(self, h, size):
        self._set_offset(h, 20, size)  # FileEndOfFileInformation, never set valid-data length.

    def rewind(self, h):
        self.position(h, 0)

    def verify_bytes(self, h, size, sha, *, checkpoint=None, on_read=None):
        if checkpoint is not None: checkpoint()
        self.rewind(h)
        out, left = hashlib.sha256(), size
        while left:
            if checkpoint is not None: checkpoint()
            b = self.read(h, min(left, CHUNK))
            if not b or len(b) > min(left, CHUNK): raise Rejected("short or invalid held output read")
            out.update(b)
            left -= len(b)
            if on_read is not None: on_read(len(b))
        if checkpoint is not None: checkpoint()
        if self.read(h, 1) or out.hexdigest() != sha:
            raise Rejected("the actual staged file does not contain the selected source bytes")

    def rename_new(self, h, parent, name):
        raw = leaf(name).encode("utf-16-le")
        block = c.create_string_buffer(c.sizeof(_Rename) + len(raw))
        fields = _Rename.from_buffer(block)
        fields.replace, fields.parent, fields.length = 0, parent, len(raw)
        c.memmove(c.addressof(block) + _Rename.first.offset, raw, len(raw))
        io = _IO()
        rc = self.set_info(h, c.byref(io), c.byref(block), c.sizeof(block), 10)
        _ok("native rename of held original, never replace", rc, io)


class PinnedParent:
    """Own the actual drive and every intermediate directory from the new selection until close."""
    def __init__(self, path, *, output=False):
        self.stack, self.api, self.output = ExitStack(), OutputNative(), output
        self.drive, self.parts = drive_path(path)
        if output and not self.parts: raise Rejected("a writable volume root is not an export directory")
        if self.api.drive_type(self.drive+":\\") != 3: raise Rejected("not a fixed local drive")
        self.chain = []
        try:
            root = self.stack.enter_context(self.api.directory(None, "\\??\\"+self.drive+":\\", False))
            self.guid = self.api.admit_volume(root)
            self.volume, initial = self.api.info(root)
            if not initial.directory: raise Rejected("not a directory at the selected volume root")
            self.chain.append((root, initial.file_id, self.guid))
            for i, part in enumerate(self.parts):
                h = self.stack.enter_context(self.api.directory(self.handle, part, output and i+1 == len(self.parts)))
                volume, row = self.api.info(h)
                native = self.guid + "\\".join(self.parts[:i+1])
                if volume != self.volume or not row.directory or self.api.guid_name(h) != native:
                    raise Rejected("original directory path crossed a provider or changed spelling")
                self.chain.append((h, row.file_id, native))
            self.check()
        except BaseException:
            self.close()
            raise

    @property
    def handle(self): return self.chain[-1][0]

    def name(self, child): return self.chain[-1][2].rstrip("\\") + "\\" + leaf(child)

    def shell_name(self, child, *, existing=None):
        """One conventional MAX_PATH spelling, matched to live normalized DOS handles. No alias fallback."""
        # Ordinary Win32/Shell paths can identify a legacy DOS device even when its
        # base ends in an ASCII space before the first dot. NT-relative/extended names
        # are not enough. Decline every such component; never ask Shell to resolve it.
        for part in (*self.parts, leaf(child)):
            stem, dot, _ = part.partition(".")
            if dot and stem.endswith(" "):
                raise Rejected("ambiguous component for an ordinary Windows Shell path")
        self.check()
        root = self.drive + ":\\" + "\\".join(self.parts)
        path = root.rstrip("\\") + "\\" + leaf(child)
        if len(path.encode("utf-16-le")) // 2 >= 260:
            raise Rejected("selected attachment path is outside the admitted Windows Shell path length")
        # Native I/O remains handle-relative/volume-GUID bound. The Shell receives ONLY the
        # corresponding ordinary local spelling, not an NT/extended path, URL or 8.3 alias.
        def require(actual, spelling):
            b = c.create_unicode_buffer(265)
            n = self.api.final_name(actual, b, len(b), 0)  # VOLUME_NAME_DOS | FILE_NAME_NORMALIZED
            if not 0 < n < len(b) or b.value != "\\\\?\\" + spelling:
                raise Rejected("the selected Windows Shell spelling does not identify the held original")
        require(self.handle, root)
        if existing is not None: require(existing, path)
        return path

    def check(self):
        if not self.chain: raise Rejected("the original selected directory has been released")
        # A drive letter can be reassigned while original directory handles remain open.
        with self.api.directory(None, "\\??\\"+self.drive+":\\", False) as live:
            volume, row = self.api.info(live)
            if self.api.admit_volume(live) != self.guid or volume != self.volume or row.file_id != self.chain[0][1]:
                raise Rejected("the selected drive now names a different original volume")
        if self.output: self.api.require_case_insensitive(self.handle)
        for h, file_id, native in self.chain:
            volume, row = self.api.info(h)
            if row.attributes & EFS: raise Rejected("EFS directories are not admitted for member export")
            if volume != self.volume or not row.directory or row.file_id != file_id or self.api.guid_name(h) != native:
                raise Rejected("an original directory or its long name changed")

    def close(self):
        self.chain.clear()
        self.stack.close()

    def __enter__(self): return self
    def __exit__(self, *_): self.close()
