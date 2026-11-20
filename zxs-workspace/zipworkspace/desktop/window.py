"""One immutable request at a time; GUI objects never enter the worker thread."""
from concurrent.futures import ThreadPoolExecutor
from PySide6.QtCore import QTimer
from PySide6.QtWidgets import QMainWindow, QProgressBar, QTabWidget, QVBoxLayout, QWidget

from .archive_pane import ArchivePane
from .import_pane import ImportPane
from .widgets import button, label


class Window(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("ZIP Workspace")
        self.resize(1000, 700)
        self.pool = ThreadPoolExecutor(max_workers=1, thread_name_prefix="workspace-operation")
        self.pending = self.stop_transfer = None
        central = QWidget()
        self.setCentralWidget(central)
        layout = QVBoxLayout(central)
        self.tabs = QTabWidget()
        self.archive = ArchivePane(self)
        self.tabs.addTab(self.archive, "快照与历史")
        self.plain_import = ImportPane(self)
        self.tabs.addTab(self.plain_import, "核验普通 ZIP")
        layout.addWidget(self.tabs)
        if self.archive.mount is not None: layout.addWidget(self.archive.mount.banner)
        self.progress = QProgressBar()
        self.progress.setRange(0, 0)
        self.progress.hide()
        layout.addWidget(self.progress)
        self.transfer_cancel = button("取消本次复制（发布前）", self.cancel_transfer)
        self.transfer_cancel.hide()
        layout.addWidget(self.transfer_cancel)
        self.note = label("选择快照开始。浏览不会签出或修改工作树。")
        layout.addWidget(self.note)
        self.timer = QTimer(self)
        self.timer.timeout.connect(self.poll)

    def inform(self, message):
        self.note.setText(str(message))

    def job(self, title, work, apply, *, failed=None, cancel=None, cancel_text=None):
        if self.pending is not None: return
        self.tabs.setEnabled(False)
        self.inform(title)
        self.progress.show()
        self.timer.start(125)
        try:
            future = self.pool.submit(work)
        except BaseException:
            self.timer.stop(); self.progress.hide(); self.tabs.setEnabled(True)
            raise
        self.pending = (future, apply, failed)
        self.stop_transfer = cancel
        self.transfer_cancel.setText(cancel_text or "取消本次复制（发布前）")
        self.transfer_cancel.setEnabled(cancel is not None)
        self.transfer_cancel.setVisible(cancel is not None)
        return future

    def cancel_transfer(self):
        if self.pending is None or self.stop_transfer is None: return
        stop = self.stop_transfer
        self.stop_transfer = None
        self.transfer_cancel.setEnabled(False)
        if stop():
            self.inform("已请求取消。请等待正在进行的系统调用返回。")
        else:
            self.inform("复制已开始正式发布或已经结束，不能撤销。正在核对本次结果。")

    def poll(self):
        if self.pending is None: return
        future, apply, failed = self.pending
        if not future.done(): return
        self.timer.stop()
        self.pending = self.stop_transfer = None
        self.transfer_cancel.hide()
        self.progress.hide()
        self.tabs.setEnabled(True)
        try:
            result = future.result()
        except Exception as err:
            self.inform(str(err))
            if failed is not None: failed(err)
        else:
            try:
                apply(result)
            except Exception as err:
                self.inform(f"操作已返回，但显示结果失败。请先查看原状态，不要重复发起。{err}")

    def closeEvent(self, event):
        if self.pending is not None or self.archive.checkout.active or self.archive.checkout.commits.running is not None:
            event.ignore()
            self.inform("操作尚未返回，请等待。关闭窗口不等于撤销已登记的操作。")
        else:
            if self.archive.mount is not None and not self.archive.mount.close_window():
                event.ignore(); return
            self.archive.close_search()
            self.archive.close_text()
            self.archive.checkout.close_session()
            if self.archive.selection is not None: self.archive.selection.release_display()
            self.plain_import.release_display()
            self.pool.shutdown(wait=False)
            super().closeEvent(event)
