"""One immutable request at a time; GUI objects never enter the worker thread."""
from concurrent.futures import ThreadPoolExecutor
import sys
from PySide6.QtCore import Qt, QTimer
from PySide6.QtGui import QKeySequence
from PySide6.QtWidgets import (QDialog, QDialogButtonBox, QFrame, QMainWindow, QProgressBar,
                               QTabWidget, QVBoxLayout, QWidget)

from . import local_snapshot_url, requested_snapshot
from .archive_pane import ArchivePane
from .import_pane import ImportPane
from .widgets import button, label


class SnapshotDrop(QFrame):
    """Receive only a pathname posted by a drag. Do not grant move or open it here."""
    def __init__(self, offer):
        super().__init__()
        self.offer = offer
        self.setAcceptDrops(True)
        self.setFrameShape(QFrame.Shape.StyledPanel)
        self.setAccessibleName("拖入一份 .zxs 快照以在新窗口只读打开")
        layout = QVBoxLayout(self)
        layout.addWidget(label("将一份 .zxs 拖到这里，在新窗口确认后只读打开。"))

    @staticmethod
    def pathname(event):
        if not event.possibleActions() & Qt.DropAction.CopyAction: return None
        data = event.mimeData()
        if not data.hasUrls() or data.data("text/uri-list").size() > 64 * 1024: return None
        urls = data.urls()
        if len(urls) != 1: return None
        return local_snapshot_url(urls[0])

    @staticmethod
    def accept_name(event):
        # Explorer/Finder may propose Move. Copy in the GUI protocol means
        # the source keeps its file; Workspace itself only takes the name.
        event.setDropAction(Qt.DropAction.CopyAction)
        event.accept()

    def dragEnterEvent(self, event):
        if self.pathname(event) is None: event.ignore()
        else: self.accept_name(event)

    def dragMoveEvent(self, event):
        self.dragEnterEvent(event)

    def dropEvent(self, event):
        path = self.pathname(event)
        if path is None or not self.offer(path): event.ignore()
        else: self.accept_name(event)


class DesktopWindows:
    """Each top-level window starts without inherited archive or writer authority."""
    def __init__(self):
        self.windows, self.request = {}, None

    def open(self):
        window = Window(new_window=self.open, offer_snapshot=self.offer_snapshot)
        token = object()
        self.windows[token] = window
        window.setAttribute(Qt.WidgetAttribute.WA_DeleteOnClose)
        window.destroyed.connect(lambda _obj=None: self.windows.pop(token, None))
        window.show()
        return window

    def offer_snapshot(self, raw):
        """Prompt in one isolated window, with no I/O before the user accepts."""
        if self.request is not None: return False
        path = requested_snapshot(raw)
        window = self.open()
        prompt = QDialog(window)
        prompt.setAttribute(Qt.WidgetAttribute.WA_DeleteOnClose)
        prompt.setObjectName("open-offered-snapshot")
        prompt.setWindowTitle("在新窗口打开目录快照")
        contents = QVBoxLayout(prompt)
        contents.addWidget(label(f"将只读核验这一个 .zxs 的全部成员：\n{path!r}\n"
                                 "不接管其他窗口的工作树，不写入或挂载。已挂载路径可能由系统访问远端。"))
        choice = QDialogButtonBox(QDialogButtonBox.StandardButton.Open | QDialogButtonBox.StandardButton.Cancel)
        accept = choice.button(QDialogButtonBox.StandardButton.Open)
        accept.setText("只读打开并核验")
        accept.setAutoDefault(False)
        choice.button(QDialogButtonBox.StandardButton.Cancel).setDefault(True)
        contents.addWidget(choice)
        choice.accepted.connect(prompt.accept)
        choice.rejected.connect(prompt.reject)
        token = self.request = object()

        def clear():
            if self.request is not token: return False
            self.request = None
            return True

        def finish(result):
            if not clear() or not window.isVisible(): return
            if result == QDialog.DialogCode.Accepted: window.archive.open_snapshot(path)
            else: window.close()

        prompt.finished.connect(finish)
        prompt.destroyed.connect(lambda *_: clear())
        prompt.open()
        return True


class Window(QMainWindow):
    def __init__(self, *, new_window=None, offer_snapshot=None):
        super().__init__()
        self.setWindowTitle("ZIP Workspace")
        self.resize(1000, 700)
        self.pool = ThreadPoolExecutor(max_workers=1, thread_name_prefix="workspace-operation")
        self.pending = self.stop_transfer = self.transfer_progress = None
        self.last_progress, self.cancel_requested = None, False
        central = QWidget()
        self.setCentralWidget(central)
        layout = QVBoxLayout(central)
        self.tabs = QTabWidget()
        def open_verified_window(selection, message):
            # Transfer only the read-only archive display. A new window starts
            # with no STORE, worktree, original operation, or mount.
            fresh = new_window()
            try:
                fresh.archive.show_selection(selection)
                fresh.inform(message + "没有继承归档存储、原工作树或待决操作。")
            except BaseException:
                fresh.close()
                raise

        self.archive = ArchivePane(self, **({} if new_window is None else {
            "open_baseline": lambda selected: open_verified_window(selected, "已只读打开登记基线。"),
            "open_copy": lambda selected: open_verified_window(selected, "已复用原节点和原只读归档句柄。"),
        }))
        self.tabs.addTab(self.archive, "快照与历史")
        self.plain_import = ImportPane(self)
        self.tabs.addTab(self.plain_import, "核验普通 ZIP")
        self.tabs.currentChanged.connect(lambda _index: self.plain_import.sync_work_action())
        files = self.menuBar().addMenu("文件")
        if new_window is not None:
            open_window = files.addAction("新窗口")
            open_window.setShortcut(QKeySequence(QKeySequence.StandardKey.New))
            open_window.triggered.connect(lambda _checked=False: new_window())
        open_snapshot = files.addAction("打开 .zxs 快照…")
        open_snapshot.setShortcut(QKeySequence(QKeySequence.StandardKey.Open))
        open_snapshot.triggered.connect(lambda _checked=False: self.archive.choose())
        if sys.platform in ("linux", "darwin"):
            save_folder = files.addAction("从现有目录生成全新 .zxs…")
            save_folder.triggered.connect(lambda _checked=False: self.archive.choose_folder_snapshot())
        files.addAction("关闭当前快照").triggered.connect(lambda _checked=False: self.archive.close_snapshot())
        if offer_snapshot is not None:
            self.snapshot_drop = SnapshotDrop(offer_snapshot)
            layout.addWidget(self.snapshot_drop)
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

    def job(self, title, work, apply, *, failed=None, cancel=None, cancel_text=None, progress_report=None):
        if self.pending is not None: return
        self.tabs.setEnabled(False)
        self.inform(title)
        self.progress.setRange(0, 0)
        self.progress.setFormat("%p%")
        self.progress.show()
        self.timer.start(125)
        try:
            if sys.platform == "darwin":
                # A failed native policy restore must not affect another job on this pool.
                from ..macos_read import in_disposable_thread
                future = self.pool.submit(lambda: in_disposable_thread(work))
            else:
                future = self.pool.submit(work)
        except BaseException:
            self.timer.stop(); self.progress.hide(); self.tabs.setEnabled(True)
            raise
        self.pending = (future, apply, failed)
        self.stop_transfer = cancel
        self.transfer_progress = progress_report
        self.last_progress, self.cancel_requested = None, False
        self.transfer_cancel.setText(cancel_text or "取消本次复制（发布前）")
        self.transfer_cancel.setEnabled(cancel is not None)
        self.transfer_cancel.setVisible(cancel is not None)
        return future

    def cancel_transfer(self):
        if self.pending is None or self.stop_transfer is None: return
        stop = self.stop_transfer
        self.stop_transfer = None
        self.transfer_cancel.setEnabled(False)
        self.cancel_requested = True
        self.progress.setRange(0, 0)
        if stop():
            self.inform("已请求取消。请等待正在进行的系统调用返回。")
        else:
            self.inform("操作已进入不可取消的阶段，或已结束。正在核对本次结果。")

    def progress_tick(self):
        if self.cancel_requested or self.transfer_progress is None: return
        phase = self.transfer_progress()
        if phase == self.last_progress: return
        self.last_progress = phase
        label, done, total = phase
        if not label: return
        if total is None or total <= 0:
            self.progress.setRange(0, 0)
            self.inform(f"{label}；已处理 {done:,} B（本阶段总量未预扫）。" if done else label + "…")
            return
        self.progress.setRange(0, 1_000)
        self.progress.setValue(min(1_000, done * 1_000 // total))
        self.progress.setFormat(f"{done:,} / {total:,} B")
        self.inform(label + "；本阶段计数，不代表已经完成。")

    def poll(self):
        if self.pending is None: return
        future, apply, failed = self.pending
        if not future.done():
            self.progress_tick()
            return
        self.timer.stop()
        self.pending = self.stop_transfer = self.transfer_progress = None
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
            self.archive.close_slots()
            self.archive.close_folder_snapshot_choice()
            self.archive.close_store_import_choice()
            self.archive.comparison.clear()
            self.archive.checkout.close_session()
            if self.archive.selection is not None: self.archive.selection.release_display()
            self.plain_import.release_display()
            self.pool.shutdown(wait=False)
            super().closeEvent(event)
