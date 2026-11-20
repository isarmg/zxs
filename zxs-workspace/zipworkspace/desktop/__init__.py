"""Optional native desktop app. The core/CLI never imports Qt."""
import os
from pathlib import Path
import sys


def requested_snapshot(name):
    """Interpret one external pathname without opening it or inferring any URL."""
    if (type(name) is not str or not 1 <= len(name) <= 4096 or
            any(ord(ch) < 32 or ord(ch) == 127 for ch in name)):
        raise ValueError("请提供一个普通 .zxs 快照的完整文件路径。")
    path = Path(name)
    _, extension = os.path.splitext(os.path.basename(name))
    if (not path.is_absolute() or extension.lower() != ".zxs" or
            (os.name == "nt" and (len(path.drive) != 2 or ":" in name[2:])) or
            (os.name != "nt" and name.startswith("//"))):
        raise ValueError("只接收一个绝对 .zxs 文件路径；不接收网址、设备路径或普通 ZIP。")
    return name


def startup_snapshot(args):
    if not args: return None
    if len(args) != 1: raise ValueError("每次只可传入一个 .zxs 快照。")
    return requested_snapshot(args[0])


def local_snapshot_url(url):
    """Qt's file scheme alone also admits host-bearing URLs; never resolve them here."""
    if (not url.isValid() or not url.isLocalFile() or url.authority() or
            url.hasQuery() or url.hasFragment()):
        return None
    try: return requested_snapshot(url.toLocalFile())
    except ValueError: return None


class _FinderOffers:
    """Accept one native path at a time, without reading or keeping a bookmark."""
    def __init__(self, startup):
        self.pending, self.windows, self.post = startup, None, None
        self.scheduled = False

    def receive(self, url):
        if self.pending is not None or (self.windows is not None and self.windows.request is not None):
            return False
        name = local_snapshot_url(url)
        if name is None: return False
        self.pending = name
        self.schedule()
        return True

    def attach(self, windows, post):
        self.windows, self.post = windows, post
        self.schedule()

    def schedule(self):
        if self.post is not None and not self.scheduled:
            self.scheduled = True
            self.post(self.present)

    def present(self):
        self.scheduled = False
        name, self.pending = self.pending, None
        if name is not None: self.windows.offer_snapshot(name)
        elif not self.windows.windows: self.windows.open()


def _finder_application(base, args, offers):
    from PySide6.QtCore import QEvent
    from PySide6.QtGui import QFileOpenEvent

    class Application(base):
        def event(self, event):
            if event.type() == QEvent.Type.FileOpen:
                if isinstance(event, QFileOpenEvent): offers.receive(event.url())
                return True  # No URL, shell command or automatic open fallback.
            return super().event(event)

    return Application(args)


def main():
    try:
        requested = startup_snapshot(sys.argv[1:])
    except ValueError as err:
        if sys.stderr is not None: print(err, file=sys.stderr)
        return 2
    try:
        from PySide6.QtWidgets import QApplication
    except ModuleNotFoundError as err:
        if err.name != "PySide6": raise
        if sys.stderr is not None:
            print("Install this project's optional desktop extra first.", file=sys.stderr)
        return 2
    from .window import DesktopWindows
    # The Cocoa delegate suppresses a startup OpenDocument echo for a path
    # already in QApplication.arguments(). Pass only the validated absolute
    # pathname, never unvetted toolkit switches or flags from a file event.
    offers = _FinderOffers(requested) if sys.platform == "darwin" else None
    args = sys.argv[:1]
    if offers is not None and requested is not None: args.append(requested)
    app = QApplication(args) if offers is None else _finder_application(QApplication, args, offers)
    app.setApplicationName("ZIP Workspace")
    windows = DesktopWindows()
    if offers is not None:
        from PySide6.QtCore import QTimer
        offers.attach(windows, lambda work: QTimer.singleShot(0, work))
    elif requested is None: windows.open()
    else: windows.offer_snapshot(requested)
    return app.exec()
