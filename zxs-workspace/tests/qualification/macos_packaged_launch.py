"""Observe a normal Cocoa window owned by the locally built bundle's executable."""
import ctypes as C
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time


class Rect(C.Structure):
    _fields_ = [(name, C.c_double) for name in ("x", "y", "width", "height")]


def bind(library, name, result, *arguments):
    fn = getattr(library, name)
    fn.restype, fn.argtypes = result, arguments
    return fn


def window_observer():
    frameworks = "/System/Library/Frameworks"
    cg = C.CDLL(frameworks + "/CoreGraphics.framework/CoreGraphics")
    cf = C.CDLL(frameworks + "/CoreFoundation.framework/CoreFoundation")
    P = C.c_void_p
    windows = bind(cg, "CGWindowListCopyWindowInfo", P, C.c_uint32, C.c_uint32)
    rectangle = bind(cg, "CGRectMakeWithDictionaryRepresentation", C.c_bool, P, C.POINTER(Rect))
    count = bind(cf, "CFArrayGetCount", C.c_long, P)
    at = bind(cf, "CFArrayGetValueAtIndex", P, P, C.c_long)
    get = bind(cf, "CFDictionaryGetValue", P, P, P)
    number = bind(cf, "CFNumberGetValue", C.c_bool, P, C.c_long, P)
    release = bind(cf, "CFRelease", None, P)
    owner, layer, bounds = (P.in_dll(cg, "kCGWindow" + name)
                            for name in ("OwnerPID", "Layer", "Bounds"))

    def integer(item, key):
        value, out = get(item, key), C.c_int()
        return out.value if value and number(value, 9, C.byref(out)) else None  # kCFNumberIntType

    def has_normal_window(pid):
        observed = windows(0x11, 0)  # OnScreenOnly | ExcludeDesktopElements; kCGNullWindowID.
        if not observed:
            raise RuntimeError("the packaged-app probe is not in a WindowServer session")
        try:
            for i in range(count(observed)):
                item = at(observed, i)
                if integer(item, owner) != pid or integer(item, layer) != 0:
                    continue
                rect, value = Rect(), get(item, bounds)
                if value and rectangle(value, C.byref(rect)) and rect.width >= 200 and rect.height >= 100:
                    return True
            return False
        finally:
            release(observed)

    return has_normal_window


def check(app):
    # The calling job has just verified the bundle and its arm64 executable.
    # No venv, project directory or external Qt plugin path is supplied to it.
    binary = app / "Contents/MacOS/ZipWorkspace"
    visible = window_observer()
    with tempfile.TemporaryDirectory(prefix="packaged-window-", dir=app.parent) as directory:
        env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": directory, "TMPDIR": directory}
        with subprocess.Popen([str(binary)], cwd=directory, env=env, stdin=subprocess.DEVNULL) as child:
            try:
                deadline, samples = time.monotonic() + 20, 0
                while time.monotonic() < deadline:
                    result = child.poll()
                    if result is not None:
                        raise RuntimeError(f"packaged ZipWorkspace exited before it showed a window ({result})")
                    samples = samples + 1 if visible(child.pid) else 0
                    if samples == 2:
                        break
                    time.sleep(0.25)
                else:
                    raise RuntimeError("packaged ZipWorkspace did not expose a normal window")
            finally:
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=5)
    print("Unpacked ZipWorkspace.app launched and showed a native window; test process reaped.", flush=True)


if __name__ == "__main__":
    if sys.platform != "darwin" or platform.machine() != "arm64" or os.environ.get("GITHUB_ACTIONS") != "true":
        raise SystemExit("run only on the arm64 macOS hosted qualification")
    if len(sys.argv) != 2:
        raise SystemExit("expected one locally built and verified app bundle")
    app = Path(sys.argv[1])
    if app.name != "ZipWorkspace.app" or app.parent.name != "unpacked" or app.resolve(strict=True) != app:
        raise SystemExit("expected the actual app unpacked by the hosted qualification")
    check(app)
