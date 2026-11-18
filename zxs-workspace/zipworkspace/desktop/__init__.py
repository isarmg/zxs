"""Optional native desktop app. The core/CLI never imports Qt."""
import sys


def main():
    try:
        from PySide6.QtWidgets import QApplication
    except ModuleNotFoundError as err:
        if err.name != "PySide6": raise
        if sys.stderr is not None:
            print("Install this project's optional desktop extra first.", file=sys.stderr)
        return 2
    from .window import Window
    app = QApplication(sys.argv)
    app.setApplicationName("ZIP Workspace")
    window = Window()
    window.show()
    return app.exec()
