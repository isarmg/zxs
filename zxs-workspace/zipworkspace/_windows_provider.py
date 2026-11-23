"""Admit one exact frozen application provider and its existing machine-scoped vendor installation."""
from contextlib import ExitStack
import ctypes as c
from hashlib import sha256
from importlib.machinery import ExtensionFileLoader
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path
import re
import sys
import sysconfig
from threading import Lock

from . import _windows_release as release
from ._windows_output import PinnedParent
from .model import CHUNK, Rejected
from .windows_nt import I32, P, U32, _function

_FULL = "zipworkspace._workspace_winfsp_draft_host"
_DLL, _SYS = "winfsp-x64.dll", "winfsp-x64.sys"
_W = c.c_wchar_p
_lock, _active, _tainted = Lock(), None, False


def packaged():
    return bool(sys.platform == "win32" and getattr(sys, "frozen", False) is True and release.RELEASES)


def _need(ok, message):
    if not ok:
        raise Rejected(message)


class _SignedFile(c.Structure):
    _fields_ = [("size", U32), ("path", _W), ("file", P), ("subject", P)]


class _Policy(c.Structure):
    _fields_ = [("size", U32), ("callback", P), ("client", P), ("ui", U32), ("revoke", U32),
                ("choice", U32), ("file", c.POINTER(_SignedFile)), ("action", U32), ("state", P),
                ("url", _W), ("flags", U32), ("context", U32), ("signature", P)]


def signed(path, handle):
    # Embedded Authenticode file signature; no UI, trust downloads, or caller-selected issuer.
    trust = c.WinDLL("wintrust.dll", winmode=0x800, use_last_error=True)
    fn = _function(trust, "WinVerifyTrust", I32, P, P, c.POINTER(_Policy))
    from uuid import UUID
    action = (c.c_ubyte * 16).from_buffer_copy(UUID("00aac56b-cd44-11d0-8cc2-00c04fc295ee").bytes_le)
    file = _SignedFile(c.sizeof(_SignedFile), str(path), handle, None)
    data = _Policy()
    data.size, data.ui, data.revoke, data.choice = c.sizeof(data), 2, 1, 1
    data.file, data.action, data.flags = c.pointer(file), 1, 0x1000 | 0x80
    try:
        _need(fn(P(-1), c.byref(action), c.byref(data)) == 0, "the original file has no valid offline Authenticode chain")
    finally:
        data.action = 2
        fn(P(-1), c.byref(action), c.byref(data))


class _Image:
    def __init__(self, keep, path, *, digest=True):
        self.parent = keep.enter_context(PinnedParent(path.parent))
        self.path = self.parent.name(path.name)
        self.file = keep.enter_context(self.parent.api.input_file(self.parent.handle, path.name))
        self.dos = self.parent.shell_name(path.name, existing=self.file)
        self.row = self.parent.api.ordinary(self.file, self.parent.volume, self.path)
        _need(0 < self.row.size <= 512 << 20, "unbounded or empty Windows release image")
        self.hash = None
        if digest:
            h, left, api = sha256(), self.row.size, self.parent.api
            while left:
                block = api.read(self.file, min(CHUNK, left))
                _need(0 < len(block) <= min(CHUNK, left), "incomplete original release image")
                h.update(block)
                left -= len(block)
            _need(not api.read(self.file, 1), "release image grew while checking")
            self.hash = h.hexdigest()
            api.rewind(self.file)
        signed(self.path, self.file)
        self.check()

    def check(self):
        self.parent.check()
        _need(self.row == self.parent.api.ordinary(self.file, self.parent.volume, self.path),
              "the admitted signed Windows image changed")


def _installed():
    import winreg
    with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\WinFsp", 0,
                        winreg.KEY_READ | winreg.KEY_WOW64_32KEY) as k:
        def folder(name):
            value, kind = winreg.QueryValueEx(k, name)
            _need(kind == winreg.REG_SZ and type(value) is str and re.fullmatch(r"[A-Z]:\\[^/\x00]+\\", value)
                  and not any(x in ("", ".", "..") for x in value[3:-1].split("\\")),
                  "not an exact machine-installed WinFsp directory")
            return Path(value[:-1])
        root, sxs = folder("InstallDir"), folder("SxsDir")
    match = re.fullmatch(r"sxs\.([0-9A-Za-z_-]{1,32})", sxs.name)
    _need(match and str(sxs.parent) == str(root / "SxS"), "not an original WinFsp side-by-side installation")
    return sxs / "bin", match.group(1)


class _ServiceConfig(c.Structure):
    _fields_ = [("kind", U32), ("start", U32), ("error", U32), ("binary", _W), ("group", _W),
                ("tag", U32), ("depends", _W), ("account", _W), ("display", _W)]


class _Service:
    def __init__(self, keep, ident, driver):
        a = c.WinDLL("advapi32.dll", winmode=0x800, use_last_error=True)
        manager = _function(a, "OpenSCManagerW", P, _W, _W, U32)
        service = _function(a, "OpenServiceW", P, P, _W, U32)
        close = _function(a, "CloseServiceHandle", I32, P)
        self.config = _function(a, "QueryServiceConfigW", I32, P, P, U32, c.POINTER(U32))
        self.status = _function(a, "QueryServiceStatus", I32, P, P)
        def owned(h):
            _need(h, "cannot inspect the original installed WinFsp driver service")
            keep.callback(close, h)
            return h
        self.h = owned(service(owned(manager(None, None, 1)), "WinFsp+" + ident, 1 | 4))
        self.driver = driver
        # A per-logon DOS letter can shadow the SCM/LocalSystem global letter. Hold both
        # roots; a matching textual X: in QueryServiceConfig is not sufficient evidence.
        parent = driver.parent
        self.global_name = "\\GLOBAL??\\" + parent.drive + ":\\"
        self.global_root = keep.enter_context(parent.api.directory(None, self.global_name, False))
        self.check(first=True)

    def check(self, *, first=False):
        parent = self.driver.parent
        def same_root(h):
            volume, row = parent.api.info(h)
            _need(parent.api.admit_volume(h) == parent.guid and volume == parent.volume
                  and row.directory and row.file_id == parent.chain[0][1],
                  "SCM and this logon do not resolve the service drive to the same original NTFS root")
        same_root(self.global_root)
        with parent.api.directory(None, self.global_name, False) as current:
            same_root(current)
        block, size = c.create_string_buffer(8192), U32()
        _need(self.config(self.h, block, len(block), c.byref(size)), "cannot query the original WinFsp service")
        cfg = _ServiceConfig.from_buffer(block)
        # Only the exact ordinary file already held, never a quoted command, expansion or bin junction.
        _need(cfg.kind == 2 and cfg.start == 3 and cfg.binary in
              (self.driver.dos, "\\??\\" + self.driver.dos),
              "WinFsp filesystem service is not registered to the pinned signed original SYS")
        st = (U32 * 7)()
        _need(self.status(self.h, st) and st[0] == 2 and st[1] in ((1,) if first else (1, 4)),
              "cannot establish a cold original WinFsp driver; never stop an already-running service")


def _application(keep):
    _need(packaged() and sys.version_info[:3] == (3, 14, 10) and c.sizeof(P) == 8 and sys._is_gil_enabled()
          and not sysconfig.get_config_var("Py_GIL_DISABLED")
          and sys.getwindowsversion().product_type == 1 and sys.getwindowsversion().build >= 22000,
          "此版本没有已验收并签名的 Windows 原生磁盘组件。")
    k = c.WinDLL("kernel32.dll", winmode=0x800, use_last_error=True)
    machine = _function(k, "IsWow64Process2", I32, P, c.POINTER(c.c_ushort), c.POINTER(c.c_ushort))
    process, host = c.c_ushort(), c.c_ushort()
    _need(machine(P(-1), c.byref(process), c.byref(host)) and process.value == 0 and host.value == 0x8664,
          "the qualified Windows AMD64 profile does not admit emulation")
    exe = Path(sys.executable)
    internal = exe.parent / "_internal"
    _need(exe.name == "ZipWorkspace.exe" and str(getattr(sys, "_MEIPASS", "")) == str(internal),
          "not the exact onedir Workspace executable and native package layout")
    app = _Image(keep, exe, digest=False)
    ext = _Image(keep, internal / "zipworkspace" / release.PROVIDER)
    return app, ext, release.for_provider(ext.hash)


def admitted_client():
    # Cold exact original receipt/retry does not invoke, inspect, start or adopt a driver.
    with ExitStack() as proof:
        return _application(proof)[2]


def _load():
    keep = ExitStack()
    global _tainted
    try:
        app, ext, profile = _application(keep)
        bindir, ident = _installed()
        dll, driver = _Image(keep, bindir / _DLL), _Image(keep, bindir / _SYS)
        _need((dll.hash, driver.hash) == (profile.vendor, profile.driver), "installed WinFsp bytes are not the qualified pair")
        service = _Service(keep, ident, driver)
        k = c.WinDLL("kernel32.dll", winmode=0x800, use_last_error=True)
        present = _function(k, "GetModuleHandleW", P, _W)
        link = _function(k, "LoadLibraryExW", P, _W, P, U32)
        name = _function(k, "GetModuleFileNameW", U32, P, c.POINTER(c.c_wchar), U32)
        _need(_FULL not in sys.modules and not present(_DLL) and not present(release.PROVIDER),
              "unknown WinFsp or application module is already loaded; restart the original application")
        def exact(image, base, h):
            buf = c.create_unicode_buffer(32768)
            n = name(h, buf, len(buf))
            _need(h and present(base) == h and 0 < n < len(buf) and buf.value in (image.dos, image.path),
                  "loader did not bind the exact reviewed original Windows image")
        def pinned(image, base):
            image.check()
            # Explicitly preload both modules with SYSTEM32-only dependency search: vendor is
            # already loaded when importing the application host; no PATH/CWD/AddDllDirectory.
            h = link(image.path, None, 0x800)
            _need(h, "could not load a verified native Windows component")
            exact(image, base, h)
            return h
        _tainted = True  # No retry using a possibly half-initialized executable module.
        vend = pinned(dll, _DLL)
        vendor_api = c.WinDLL(dll.path, handle=vend)
        get_id = _function(vendor_api, "FspSxsIdent", _W)
        _need(get_id() == ident, "the loaded WinFsp DLL selected a different installed driver instance")
        py = pinned(ext, release.PROVIDER)
        loader = ExtensionFileLoader(_FULL, ext.path)
        spec = spec_from_file_location(_FULL, ext.path, loader=loader)
        module = module_from_spec(spec)
        loader.exec_module(module)
        exact(ext, release.PROVIDER, py)
        exact(dll, _DLL, vend)
        _need(type(module).__name__ == "module" and callable(getattr(module, "run", None)), "unexpected Workspace provider ABI")
        def check():
            _need(_installed() == (bindir, ident) and get_id() == ident, "the installed WinFsp instance changed")
            for file in (app, ext, dll, driver):
                file.check()
            exact(ext, release.PROVIDER, py)
            exact(dll, _DLL, vend)
            service.check()
        check()
        return keep, module, check
    except BaseException:
        keep.close()
        raise


def admitted_provider():
    global _active
    with _lock:
        if _active is None:
            _need(not _tainted, "native provider admission was interrupted; restart Workspace")
            _active = _load()  # Retain original files, service handles and mapped modules until process exit.
        _active[2]()
        return _active[1]


def selected_host():
    if getattr(sys, "frozen", False):
        return admitted_provider()  # No development-import fallback from a packaged process.
    from . import _workspace_winfsp_draft_host  # Only the explicitly run source qualification.
    return _workspace_winfsp_draft_host
