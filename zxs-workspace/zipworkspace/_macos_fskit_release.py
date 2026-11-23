"""Maintainer-reviewed native macOS release profiles. No dynamic manifest or user switch."""
import ctypes as C
from dataclasses import dataclass
import json
import platform
import re
import sys

from .model import Rejected


@dataclass(frozen=True, slots=True)
class Release:
    team: str
    bundle_build: str
    os_build: str
    module_unique: str     # Security.framework's kSecCodeInfoUnique for the independently signed embedded extension.
    qualification_record: str  # Preallocated ID of the independent release evidence; never embed a self-referential digest.

    def __post_init__(self):
        if (not re.fullmatch("[A-Z0-9]{10}",self.team) or not re.fullmatch("[0-9]+(?:[.][0-9]+){0,2}",self.bundle_build)
                or not re.fullmatch("[A-Za-z0-9]{3,32}",self.os_build)
                or not re.fullmatch("[0-9a-f]{40}(?:[0-9a-f]{24})?",self.module_unique)
                or not re.fullmatch("[0-9a-f]{32}",self.qualification_record)
                or any(set(x)=={"0"} for x in (self.module_unique,self.qualification_record))):
            raise Rejected("incomplete qualified macOS release")


# Empty until a separately signed/provisioned distribution actually passes real kernel/editor/receipt tests.
# The existing Apple-signed release process must reserve a new CFBundleVersion for
# every changed executable, embedded Python/Qt payload or dependency. Require its
# final inventory and native qualification before accepting/distributing the frozen
# candidate containing the preallocated record ID; never reuse a build.
# Runtime verifies that signed release and original module/OS, not a self-hash of the
# whole host. No detached release keys, post-install capability files or overrides.
RELEASES: tuple[Release,...] = ()


def packaged():
    return bool(RELEASES) and sys.platform == "darwin" and platform.machine() == "arm64" and getattr(sys,"frozen",False) is True


def require_release():
    if not packaged():
        raise Rejected("当前版本没有通过原机验收的 macOS 可写卷。")
    try:
        identity=C.CDLL(None,mode=C.RTLD_LOCAL|0x100).zxs_fskit_release_identity
        identity.argtypes,identity.restype=[C.POINTER(C.c_char),C.c_ssize_t],C.c_int32
        out=C.create_string_buffer(1024)
        if identity(out,len(out)):
            raise Rejected("无法确认原签名宿主、扩展和系统版本。")
        current=json.loads(out.value)
        rows=[r for r in RELEASES if [r.team,r.bundle_build,r.os_build,r.module_unique]==current]
        if len(rows)!=1:
            raise Rejected("此系统与签名组件的组合没有通过原生验收。")
        return rows[0]
    except (AttributeError,TypeError,ValueError,RecursionError) as exc:
        raise Rejected("原签名程序没有提供受信发行身份。") from exc
