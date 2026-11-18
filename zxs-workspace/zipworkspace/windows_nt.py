"""Read-only native calls for a selected local Windows directory. No DLL loads on import."""
import ctypes as c
from dataclasses import dataclass
from functools import lru_cache
import os
import re
import struct

from .model import CHUNK, Rejected, member_name

U8, U16, U32, I32, I64, U64 = c.c_uint8, c.c_uint16, c.c_uint32, c.c_int32, c.c_int64, c.c_uint64
P = c.c_void_p
# FILE_ID_EXTD_DIR_INFO up to (but not including) the variable UTF-16 long name.
HEADER = struct.Struct("<II6qIIII16s")
BUFFER = 64 * 1024
DOT = {".", ".."}
# Only understood attributes of local ordinary files/dirs. No device, reparse,
# offline, recall-on-open/data, or cloud pin state is admitted.
ATTRIBUTES = 0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0020 | 0x0080 | 0x0100 | 0x0200 | 0x0800 | 0x2000 | 0x4000 | 0x8000 | 0x20000
GUID_ROOT = re.compile(r"\\\\\?\\Volume\{[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}\}\\")


@dataclass(frozen=True, slots=True)
class Entry:
    file_id: bytes
    attributes: int
    birth: int
    write: int
    change: int
    size: int

    @property
    def directory(self):
        return bool(self.attributes & 0x10)


def entry(fid, attrs, birth, write, change, size):
    if (len(fid) != 16 or not any(fid) or not attrs or attrs & ~ATTRIBUTES or
            min(birth, write, change) <= 0 or size < 0):
        raise Rejected("unsupported Windows entry type, timestamp or file identity")
    return Entry(fid, attrs, birth, write, change, 0 if attrs & 0x10 else size)


def entries(data):
    """The API provides buffer capacity, not bytes used. Never walk past its terminal record."""
    start = 0
    while True:
        if len(data) > BUFFER or start + HEADER.size > len(data):
            raise Rejected("incomplete native directory record")
        (nxt, _, born, _atime, written, changed, length, _, attrs, count, _, _tag, fid) = HEADER.unpack_from(data, start)
        end = start + HEADER.size + count
        if (not count or count > 510 or count % 2 or end > len(data) or nxt and
                (nxt % 8 or start+nxt < end or start+nxt+HEADER.size > len(data))):
            raise Rejected("invalid native directory name length or next record")
        try: name = data[start+HEADER.size:end].decode("utf-16-le", "strict")
        except UnicodeError as err: raise Rejected("invalid UTF-16 directory name") from err
        if name in DOT:
            yield name, None
        else:
            if len(member_name(name)) != 1: raise Rejected("not one native directory component")
            # ReparsePointTag is not defined for an ordinary file. The attributes
            # decide whether the row is admissible before opening its name.
            yield name, entry(fid, attrs, born, written, changed, length)
        if not nxt: return
        start += nxt


class _String(c.Structure):
    _fields_ = [("length",U16), ("maximum",U16), ("buffer",P)]


class _Object(c.Structure):
    _fields_ = [("length",U32), ("parent",P), ("name",c.POINTER(_String)), ("flags",U32),
                ("security",P), ("quality",P)]


class _Result(c.Union):
    _fields_ = [("status",I32), ("pointer",P)]


class _IO(c.Structure):
    _fields_ = [("result",_Result), ("information",c.c_size_t)]


class _Basic(c.Structure):
    _fields_ = [("birth",I64), ("access",I64), ("write",I64), ("change",I64), ("attrs",U32)]


class _Standard(c.Structure):
    _fields_ = [("allocation",I64), ("eof",I64), ("links",U32), ("deleting",U8), ("directory",U8)]


class _Id(c.Structure):
    _fields_ = [("volume",U64), ("fid",U8*16)]


class _Device(c.Structure):
    _fields_ = [("kind",U32), ("characteristics",U32)]


class Handle:
    def __init__(self, api, value): self.api, self.value = api, value

    def close(self):
        h, self.value = self.value, None
        if h is not None and not self.api.close_handle(h): raise c.WinError(c.get_last_error())

    def __enter__(self): return self.value
    def __exit__(self, *_): self.close()


def _function(dll, name, result, *args):
    f = getattr(dll, name)
    f.restype, f.argtypes = result, args
    return f


class Native:
    def __init__(self):
        # System32 only. A project, working directory or archive never supplies native code.
        k = c.WinDLL("kernel32.dll", use_last_error=True, winmode=0x800)
        n = c.WinDLL("ntdll.dll", use_last_error=True, winmode=0x800)
        self.create = _function(n,"NtCreateFile",I32,c.POINTER(P),U32,c.POINTER(_Object),
                                c.POINTER(_IO),P,U32,U32,U32,U32,P,U32)
        self.volume_info = _function(n,"NtQueryVolumeInformationFile",I32,P,c.POINTER(_IO),P,U32,I32)
        self.file_info = _function(k,"GetFileInformationByHandleEx",I32,P,I32,P,U32)
        self.file_type = _function(k,"GetFileType",U32,P)
        self.volume_name = _function(k,"GetVolumeInformationByHandleW",I32,P,P,U32,P,P,P,P,U32)
        self.final_name = _function(k,"GetFinalPathNameByHandleW",U32,P,P,U32,U32)
        self.drive_type = _function(k,"GetDriveTypeW",U32,c.c_wchar_p)
        self.read_file = _function(k,"ReadFile",I32,P,P,U32,c.POINTER(U32),P)
        self.close_handle = _function(k,"CloseHandle",I32,P)

    def _open(self, parent, name, kind):
        raw = name.encode("utf-16-le")
        buf = c.create_string_buffer(raw + b"\0\0")
        string = _String(len(raw), len(raw)+2, c.addressof(buf))
        obj = _Object(c.sizeof(_Object), parent, c.pointer(string), 0, None, None)
        value, io = P(), _IO()
        directory = kind != "file"
        # SYNCHRONIZE | READ_ATTRIBUTES, plus LIST_DIRECTORY/TRAVERSE or READ_DATA.
        access = 0x100000 | 0x80 | (0x20 if directory else 0) | (0 if kind == "ancestor" else 1)
        # No WRITE share on a content handle. Directory names may change; both
        # complete inventories and the selected original are independently checked.
        share = 7 if directory else 5
        # Microsoft's FILE_DIRECTORY_FILE compatibility list excludes the
        # no-reparse/no-recall flags we require. Open existing directories
        # type-neutrally, then admit the actual standard/basic handle type
        # before enumerating or opening a child. Never retry with weaker flags.
        options = 0x00200000 | 0x00400000 | 0x20 | (0 if directory else 0x40)
        status = self.create(c.byref(value), access, c.byref(obj), c.byref(io), None, 0,
                             share, 1, options, None, 0)  # FILE_OPEN only, never create/overwrite.
        if status < 0: raise Rejected(f"original Windows directory/file unavailable: NTSTATUS {status & 0xffffffff:08x}")
        if not value.value: raise Rejected("no original Windows handle")
        handle = Handle(self, value.value)
        if status or io.result.status or io.information != 1:  # FILE_OPENED
            handle.close()
            raise Rejected("did not synchronously open the existing Windows object")
        return handle

    def root(self, letter, enumerate_it):
        if not re.fullmatch("[A-Z]", letter) or self.drive_type(letter+":\\") != 3:
            raise Rejected("select a fixed local drive-letter folder")
        return self._open(None, "\\??\\"+letter+":\\", "directory" if enumerate_it else "ancestor")

    def child(self, parent, name, kind):
        if kind not in ("ancestor","directory","file") or len(member_name(name)) != 1:
            raise Rejected("expected one ordinary relative Windows component")
        return self._open(parent, name, kind)

    def _info(self, h, cls, layout):
        value = layout()
        if not self.file_info(h, cls, c.byref(value), c.sizeof(value)):
            raise c.WinError(c.get_last_error())
        return value

    def info(self, h):
        if self.file_type(h) != 1: raise Rejected("not an ordinary disk handle")
        fid = self._info(h,18,_Id)
        b, s = self._info(h,0,_Basic), self._info(h,1,_Standard)
        e = entry(bytes(fid.fid), b.attrs, b.birth, b.write, b.change, s.eof)
        if not fid.volume or s.deleting or not s.links or bool(s.directory) != e.directory:
            raise Rejected("unavailable or changing Windows native identity")
        return fid.volume, e

    def guid_name(self, h):
        # Existing project member bounds do not justify arbitrary provider allocations.
        buf = c.create_unicode_buffer(32768)
        size = self.final_name(h, buf, len(buf), 1)  # VOLUME_NAME_GUID | FILE_NAME_NORMALIZED
        if not 0 < size < len(buf): raise Rejected("cannot verify the held volume GUID name")
        return buf.value

    def admit_volume(self, root):
        guid = self.guid_name(root)
        dev, io = _Device(), _IO()
        s = self.volume_info(root,c.byref(io),c.byref(dev),c.sizeof(dev),4)  # FileFsDeviceInformation
        fs = c.create_unicode_buffer(32)
        if (not GUID_ROOT.fullmatch(guid) or self.drive_type(guid) != 3 or s or io.result.status or
                io.information != c.sizeof(dev) or dev.kind != 7 or dev.characteristics & 0x1055 or
                not self.volume_name(root,None,0,None,None,None,fs,len(fs)) or fs.value != "NTFS"):
            raise Rejected("cannot prove a fixed, nonremote NTFS volume for the selected directory")
        return guid

    def listing(self, h):
        first = True
        while True:
            buf = (U64*(BUFFER//8))()  # Aligned, completely cleared before each API call.
            if not self.file_info(h, 20 if first else 19, c.byref(buf), BUFFER):
                code = c.get_last_error()
                if code == 18: return  # ERROR_NO_MORE_FILES only.
                raise c.WinError(code)
            first = False
            yield from entries(bytes(buf))

    def read(self, h, length):
        if not 0 < length <= CHUNK: raise Rejected("out-of-range Windows content read")
        buf, got = c.create_string_buffer(length), U32()
        if not self.read_file(h,c.byref(buf),length,c.byref(got),None):
            raise c.WinError(c.get_last_error())
        if got.value > length: raise Rejected("invalid native byte count")
        return buf.raw[:got.value]


@lru_cache(maxsize=1)
def backend():
    if os.name != "nt": return None
    try: return Native()
    except (AttributeError,OSError): return None
