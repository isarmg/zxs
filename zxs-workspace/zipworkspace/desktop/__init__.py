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
    # A pathname is not a Qt option or an argument to another program.
    app = QApplication(sys.argv[:1])
    app.setApplicationName("ZIP Workspace")
    windows = DesktopWindows()
    if requested is None: windows.open()
    else: windows.offer_snapshot(requested)
    return app.exec()
