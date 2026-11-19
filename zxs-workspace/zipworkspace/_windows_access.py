"""One small, deliberately restrictive NTFS output profile. Observe security; never set it."""
import ctypes as c
from dataclasses import dataclass
import struct

from .model import Rejected
from .windows_nt import Handle, I32, P, U32, U64, _function


READ_CONTROL = 0x00020000
# No SACL_SECURITY_INFORMATION or security privilege: request only the filtered label,
# resource attributes and central-policy IDs, as well as owner, group and DACL.
REQUEST = 0x01 | 0x02 | 0x04 | 0x10 | 0x20 | 0x40
FILE_ALL = 0x001f01ff
SD = struct.Struct("<BBH4I")
ACL = struct.Struct("<BBHHH")
ALLOW = struct.Struct("<BBHI")
LIMIT = 8192


def _sid(authority, *rids):
    return bytes((1, len(rids))) + authority.to_bytes(6, "big") + struct.pack("<" + "I"*len(rids), *rids)


SYSTEM, ADMINS, MEDIUM = _sid(5,18), _sid(5,32,544), _sid(16,8192)


def _sid_at(data, pos, end):
    if pos < 0 or pos % 4 or pos+8 > end or data[pos] != 1 or not 1 <= data[pos+1] <= 15:
        raise Rejected("invalid native SID")
    stop = pos+8+4*data[pos+1]
    if stop > end: raise Rejected("truncated native SID")
    return data[pos:stop]


@dataclass(frozen=True, slots=True)
class Profile:
    group: bytes
    control: int
    grants: tuple  # Exact ordered (SID, inherit flags); every admitted grant is mapped FILE_ALL_ACCESS.


def profile(data, actor, *, directory):
    """Match a bounded system-returned relative descriptor, not an ACL evaluator or editor."""
    if not SD.size <= len(data) <= LIMIT: raise Rejected("invalid size of Windows security evidence")
    rev, reserved, ctl, owner, group, sacl, dacl = SD.unpack_from(data)
    if (rev != 1 or reserved or ctl & 0x40c0 or (ctl & 0x8004) != 0x8004 or
            min(owner,group,dacl) < SD.size or any(n%4 for n in (owner,group,dacl,sacl))):
        raise Rejected("missing, null or unsupported Windows owner/DACL evidence")
    actual_owner = _sid_at(data, owner, len(data))
    actual_group = _sid_at(data, group, len(data))
    if actual_owner != actor: raise Rejected("the selected object is not owned by the current user")

    def header(at):
        if at < SD.size or at+ACL.size > len(data): raise Rejected("invalid native ACL offset")
        version, pad, size, count, reserved = ACL.unpack_from(data, at)
        if pad or reserved or size < ACL.size or size%4 or at+size > len(data):
            raise Rejected("invalid native ACL extent")
        return version, at+size, count

    if sacl:
        sv, se, sc = header(sacl)
        if not ctl & 0x10 or sv not in (2,4) or sc or se != sacl+ACL.size:
            raise Rejected("an explicit integrity label, resource attribute or policy is not admitted")
    dv, end, count = header(dacl)
    if dv != 2 or not 1 <= count <= 3:
        raise Rejected("a private directory and inherited ordinary-file grants are required")
    if not directory and ctl & 0x1000: raise Rejected("the new file DACL is protected from inheritance")
    grants, seen, pos = [], set(), dacl+ACL.size
    for _ in range(count):
        if pos+ALLOW.size > end: raise Rejected("incomplete file access grant")
        kind, flags, size, mask = ALLOW.unpack_from(data, pos)
        if (kind != 0 or mask != FILE_ALL or flags not in ((3,19) if directory else (16,)) or
                size < ALLOW.size+12 or size%4 or pos+size > end):
            raise Rejected("unsupported grant, rights or inheritance in the output ACL")
        sid = _sid_at(data, pos+ALLOW.size, pos+size)
        if pos+ALLOW.size+len(sid) != pos+size or sid not in (actor,SYSTEM,ADMINS) or sid in seen:
            raise Rejected("only one grant per owner, SYSTEM or built-in Administrators is admitted")
        seen.add(sid)
        grants.append((sid, flags))
        pos += size
    if pos != end or actor not in seen: raise Rejected("incomplete or non-private output ACL")
    return Profile(actual_group, ctl, tuple(grants))


class NativeSecurity:
    """Local OS queries only. No principal-name lookup, privilege adjustment, or security writes."""
    def __init__(self):
        a = c.WinDLL("advapi32.dll", use_last_error=True, winmode=0x800)
        k = c.WinDLL("kernel32.dll", use_last_error=True, winmode=0x800)
        self.thread = _function(k, "GetCurrentThread", P)
        self.process = _function(k, "GetCurrentProcess", P)
        self.close_handle = _function(k, "CloseHandle", I32, P)
        self.thread_token = _function(a, "OpenThreadToken", I32, P, U32, I32, c.POINTER(P))
        self.process_token = _function(a, "OpenProcessToken", I32, P, U32, c.POINTER(P))
        self.token_info = _function(a, "GetTokenInformation", I32, P, I32, P, U32, c.POINTER(U32))
        self.descriptor = _function(a, "GetKernelObjectSecurity", I32, P, U32, P, U32, c.POINTER(U32))
        self.valid = _function(a, "IsValidSecurityDescriptor", I32, P)

    def caller(self):
        token = P()
        if self.thread_token(self.thread(), 8, 1, c.byref(token)):
            with Handle(self, token.value): raise Rejected("impersonated export is not admitted")
        if c.get_last_error() != 1008: raise Rejected("cannot identify this worker's effective user")
        if not self.process_token(self.process(), 8, c.byref(token)) or not token.value:
            raise Rejected("cannot query the current process token")

        def query(h, cls, *, sid=False):
            buf, n = (U64*32)(), U32()
            if not self.token_info(h, cls, c.byref(buf), c.sizeof(buf), c.byref(n)) or not 0 < n.value <= c.sizeof(buf):
                raise Rejected("cannot verify the effective Windows token")
            data = bytes(buf)[:n.value]
            if not sid:
                if len(data) != 4: raise Rejected("invalid token flag")
                return int.from_bytes(data, "little")
            # TOKEN_USER and TOKEN_MANDATORY_LABEL both begin with native SID_AND_ATTRIBUTES.
            ptrsize = c.sizeof(P)
            if len(data) < ptrsize*2: raise Rejected("invalid token SID record")
            at = int.from_bytes(data[:ptrsize], "little") - c.addressof(buf)
            if at < ptrsize*2: raise Rejected("token SID was not supplied in the returned buffer")
            return _sid_at(data, at, len(data))

        with Handle(self, token.value) as h:
            # TokenElevation, TokenUIAccess, TokenIsAppContainer, TokenIntegrityLevel, TokenUser.
            if any(query(h, cls) != 0 for cls in (20,26,29)) or query(h, 25, sid=True) != MEDIUM:
                raise Rejected("select a non-elevated ordinary medium-integrity Windows session")
            user = query(h, 1, sid=True)
            # Strict subset: ordinary local/domain account or Entra account SID, not a service identity.
            if len(user) != 28 or user[2:12] not in (_sid(5,21)[2:], _sid(12,1)[2:]):
                raise Rejected("an ordinary Windows user identity is required")
            return user

    def read(self, h, actor, *, directory):
        buf, n = (U64*(LIMIT//8))(), U32()
        if (not self.descriptor(h, REQUEST, c.byref(buf), c.sizeof(buf), c.byref(n)) or
                not SD.size <= n.value <= c.sizeof(buf) or not self.valid(c.byref(buf))):
            raise Rejected("cannot establish the complete bounded permissions of the selected NTFS object")
        return profile(bytes(buf)[:n.value], actor, directory=directory)


class OutputProtection:
    def __init__(self, staging, destination):
        self.api, self.parents = NativeSecurity(), (staging, destination)
        self.actor = self.api.caller()
        self.original = self.api.read(staging, self.actor, directory=True)
        self.file = None
        self.check()

    def check(self, h=None):
        if self.api.caller() != self.actor: raise Rejected("the active Windows user changed")
        if any(self.api.read(p, self.actor, directory=True) != self.original for p in self.parents):
            raise Rejected("staging and output require unchanged, matching private directory permissions")
        if h is None: return
        actual = self.api.read(h, self.actor, directory=False)
        if (actual.group != self.original.group or
                {s for s,_ in actual.grants} != {s for s,_ in self.original.grants} or
                (self.file is not None and self.file != actual)):
            raise Rejected("the actual output file does not have the approved inherited private grants")
        self.file = actual
