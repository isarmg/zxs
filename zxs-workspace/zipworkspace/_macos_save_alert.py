"""An affirmative AppKit decision for a single download. No non-native fallback."""
from contextlib import ExitStack
import ctypes as C
import sys
from threading import current_thread, main_thread

from .model import Rejected


def approve_new_file(detail):
    """Require the second, non-default NSAlert button in the foreground GUI."""
    if sys.platform != "darwin" or current_thread() is not main_thread():
        raise Rejected("本次另存须在 macOS 原生前台窗口中确认。")
    try:
        # No AppleScript, helper process, Qt dialog fallback or new PyObjC dependency.
        _appkit = C.CDLL("/System/Library/Frameworks/AppKit.framework/AppKit")
        cf = C.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
        objc = C.CDLL("/usr/lib/libobjc.A.dylib")
        p = C.c_void_p
        cls = objc.objc_getClass
        cls.argtypes, cls.restype = [C.c_char_p], p
        sel = objc.sel_registerName
        sel.argtypes, sel.restype = [C.c_char_p], p
        # objc_msgSend is variadic in headers. Call each actual fixed signature
        # (in particular on Apple Silicon); all arguments here are object pointers.
        send = C.cast(objc.objc_msgSend, p).value
        get = C.CFUNCTYPE(p, p, p)(send)
        flag = C.CFUNCTYPE(C.c_bool, p, p)(send)
        add = C.CFUNCTYPE(p, p, p, p)(send)
        put = C.CFUNCTYPE(None, p, p, p)(send)
        run = C.CFUNCTYPE(C.c_long, p, p)(send)   # NSModalResponse (NSInteger)
        release = C.CFUNCTYPE(None, p, p)(send)
        string = cf.CFStringCreateWithBytes
        string.argtypes, string.restype = [p, C.c_char_p, C.c_long, C.c_uint32, C.c_ubyte], p
        cf.CFRelease.argtypes, cf.CFRelease.restype = [p], None

        app = get(cls(b"NSApplication"), sel(b"sharedApplication"))
        if not app or not flag(app, sel(b"isActive")) or not get(app, sel(b"keyWindow")):
            raise Rejected("没有可用于本次确认的 macOS 前台窗口。")
        with ExitStack() as owned:
            def text(value):
                raw = value.encode("utf-8")
                result = string(None, raw, len(raw), 0x08000100, 0)
                if not result: raise Rejected("macOS 无法完整显示本次确认文字。")
                owned.callback(cf.CFRelease, result)
                return result

            alert = get(get(cls(b"NSAlert"), sel(b"alloc")), sel(b"init"))
            if not alert: raise Rejected("macOS 无法建立本次确认窗口。")
            owned.callback(release, alert, sel(b"release"))
            put(alert, sel(b"setMessageText:"), text("仅另存所示远端节点为新文件？"))
            put(alert, sel(b"setInformativeText:"), text(detail))
            # NSAlertFirstButtonReturn is the default. Only a deliberate choice
            # of the other button admits publication; Escape/closure never do.
            cancel = add(alert, sel(b"addButtonWithTitle:"), text("取消"))
            save = add(alert, sel(b"addButtonWithTitle:"), text("同意并另存新文件"))
            if not cancel or not save:
                raise Rejected("macOS 无法显示本次原生确认选项。")
            # Do not depend on translated button titles for the affirmative key.
            # Default Return remains with the first button; an unhandled Escape
            # aborts the modal session and cannot return the second button.
            put(save, sel(b"setKeyEquivalent:"), text(""))
            put(cancel, sel(b"setKeyEquivalent:"), text("\r"))
            return run(alert, sel(b"runModal")) == 1001  # NSAlertSecondButtonReturn
    except (OSError, AttributeError, ValueError) as error:
        raise Rejected("macOS 原生另存确认不可用；未启用备用确认。") from error
