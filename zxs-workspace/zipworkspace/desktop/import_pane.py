"""Ordinary ZIP verification and read-only directories; Linux extraction remains separate."""
import sys

from PySide6.QtWidgets import QFileDialog, QHBoxLayout, QVBoxLayout, QWidget

from .. import plain_import
from .selection import PlainSelection, submit_verification
from .widgets import Table, button, confirm, label


class ImportPane(QWidget):
    PAGE = 100

    def __init__(self, owner):
        super().__init__()
        self.owner = owner
        self.selected = self.file_page = self.search_dialog = self.text_dialog = None
        self.verification = None
        self.number = 0
        layout = QVBoxLayout(self)
        layout.addWidget(label("选择普通 Store ZIP，核验全部成员、整树摘要及原包摘要。不提取文件，不登记工作树。"))
        self.choose = button("选择 ZIP…", self.pick)
        self.close_zip = button("关闭此 ZIP", self.close_archive)
        self.close_zip.setEnabled(False)
        selection_line = QHBoxLayout()
        selection_line.addWidget(self.choose); selection_line.addWidget(self.close_zip)
        layout.addLayout(selection_line)
        self.details = label("尚未选择文件。")
        layout.addWidget(self.details)
        self.extract = None
        if sys.platform == "linux":
            self.extract = button("提取到新的本机目录…", self.choose_destination)
            self.extract.setEnabled(False)
            layout.addWidget(self.extract)
        message = (plain_import.OUTPUT_UNAVAILABLE if self.extract is None else
                   "核验不会提取。提取须另行选择同一实际 ext4/XFS 挂载上的两个不同目录与一个新目的名称。")
        self.outcome = label(message)
        layout.addWidget(self.outcome)
        self.compare = button("与当前已核验 .zxs 快照比较", self.compare_to_current)
        self.compare.setEnabled(False)
        layout.addWidget(self.compare)
        self.at = label("尚无已核验的目录。")
        layout.addWidget(self.at)
        self.files = Table(("名称", "类型", "字节数", "SHA-256"), "已核验普通 ZIP 的当前目录")
        self.files.doubleClicked.connect(lambda _index: self.enter())
        layout.addWidget(self.files, 1)
        texts = QHBoxLayout()
        self.preview_file = button("预览选中的纯文本…", self.preview_selected)
        self.preview_whole = button("完整核验大文本后跳页…", lambda: self.preview_selected(indexed=True))
        for control in (self.preview_file, self.preview_whole):
            control.setEnabled(False)
            texts.addWidget(control)
        layout.addLayout(texts)
        self.files.selectionModel().selectionChanged.connect(self.update_text_actions)
        nav = QHBoxLayout()
        self.up = button("上一级", self.go_up)
        self.into = button("打开选中的文件夹", self.enter)
        self.back = button("上一页", lambda: self.turn(-1))
        self.more = button("下一页", lambda: self.turn(1))
        self.find = button("在此 ZIP 按名称查找…", self.open_search)
        for control in (self.up, self.into, self.back, self.more, self.find):
            control.setEnabled(False)
            nav.addWidget(control)
        layout.addLayout(nav)

    def release_display(self):
        self.verification = None
        dialog, self.search_dialog = self.search_dialog, None
        if dialog is not None: dialog.reject()
        self.close_text()
        if self.selected is not None:
            self.owner.archive.comparison.invalidate_plain(self.selected)
            self.selected.release_display()
        self.selected = self.file_page = None
        self.compare.setEnabled(False)
        self.close_zip.setEnabled(False)
        self.number = 0
        self.files.rows.replace(())
        self.update_text_actions()
        self.at.setText("尚无已核验的目录。")
        for control in (self.up, self.into, self.back, self.more, self.find):
            control.setEnabled(False)
        if self.extract is not None: self.extract.setEnabled(False)

    def close_archive(self):
        if self.selected is None or self.owner.pending is not None: return
        self.release_display()
        self.details.setText("尚未选择文件。")
        self.owner.inform("已关闭普通 ZIP 的观察与原句柄；未改变快照或工作树。")

    def pick(self):
        if self.owner.pending is not None: return
        source, _ = QFileDialog.getOpenFileName(self, "选择普通 Store ZIP", "", "普通 ZIP (*.zip *.ZIP)")
        if not source: return
        self.release_display()
        self.details.setText("尚无本次核验结果。")
        request = self.verification = object()
        submit_verification(self.owner, "正在只读核验整个 ZIP；不会写入文件…",
                            lambda observer: PlainSelection.open(source, retain=True, observer=observer),
                            self.show_selection, current=lambda: request is self.verification)

    def show_selection(self, selected):
        self.release_display()
        self.selected = selected
        self.compare.setEnabled(True)
        self.close_zip.setEnabled(True)
        if self.extract is not None:
            self.extract.setEnabled(selected.held is not None and not selected.held.closed)
        preview = selected.preview
        self.show_folder("")

        self.details.setText(f"来源：{preview.source}\n完整 ZIP 的 SHA-256：{preview.sha256}\n"
                             f"整树 SHA-256：{preview.tree}\n"
                             f"{preview.count} 个目录项，{preview.size} 字节文件内容")
        self.owner.inform("只读核验完成；可以浏览已核验目录。不是仓库快照，没有创建或更改目录。")

    def show_folder(self, directory, number=0):
        if self.selected is not None:
            self.show_page(self.selected.index.folder_page(directory, number, limit=self.PAGE), number)

    def show_page(self, page, number):
        self.close_text()
        self.file_page, self.number = page, number
        self.at.setText(f"/{page.directory}  ·  第 {number + 1} 页" + ("" if page.entries else "  ·  空文件夹"))
        self.files.rows.replace(((entry.name, "目录" if entry.kind == "directory" else "文件",
                                  "" if entry.size_bytes is None else str(entry.size_bytes), entry.sha256 or ""), entry)
                                for entry in page.entries)
        self.update_text_actions()
        self.up.setEnabled(bool(page.directory))
        self.into.setEnabled(any(entry.kind == "directory" for entry in page.entries))
        self.back.setEnabled(number > 0)
        self.more.setEnabled(page.has_more)
        self.find.setEnabled(True)

    def close_text(self):
        old, self.text_dialog = self.text_dialog, None
        if old is not None: old.reject()

    def update_text_actions(self, *_):
        from .text_preview import LIMIT
        self.close_text()
        selected, entry = self.selected, self.files.item()
        available = (self.file_page is not None and selected is not None and selected.held is not None and
                     not selected.held.closed and entry is not None and entry.kind == "file")
        self.preview_file.setEnabled(available)
        self.preview_whole.setEnabled(available and entry.size_bytes > LIMIT)

    def preview_selected(self, *, indexed=False):
        selected, view, entry = self.selected, self.file_page, self.files.item()
        if (selected is None or view is None or entry is None or entry.kind != "file" or
                self.owner.pending is not None or self.text_dialog is not None): return
        from .text_preview import request_for_row

        def current():
            return (selected is self.selected and view is self.file_page and entry is self.files.item() and
                    selected.held is not None and not selected.held.closed)

        request_for_row(self, selected, view, entry, current=current, confirm_action=confirm, indexed=indexed)

    def enter(self):
        page, entry = self.file_page, self.files.item()
        if page is not None and entry is not None and entry.kind == "directory":
            self.show_folder(f"{page.directory}/{entry.name}" if page.directory else entry.name)

    def go_up(self):
        if self.file_page is not None and self.file_page.directory:
            self.show_folder(self.file_page.directory.rpartition("/")[0])

    def turn(self, delta):
        page = self.file_page
        if page is None or (delta < 0 and not self.number) or (delta > 0 and not page.has_more): return
        self.show_folder(page.directory, self.number + delta)

    def open_search(self):
        selected = self.selected
        if selected is None or self.search_dialog is not None or self.owner.pending is not None: return
        from .name_search import NameSearch

        def current(dialog):
            return self.search_dialog is dialog and self.selected is selected

        def locate(dialog, hit):
            if not current(dialog): return None
            index = selected.index
            number, row = index.locate_name(hit, limit=self.PAGE)
            page = index.folder_page(hit.parent, number, limit=self.PAGE)
            if row >= len(page.entries) or page.entries[row] is not hit.entry:
                raise ValueError("不在原归档目录页内")
            self.show_page(page, number)
            if not current(dialog) or self.file_page is not page: return None
            self.files.selectRow(row)
            self.files.scrollTo(self.files.rows.index(row, 0))
            return self.files

        dialog = NameSearch(self.owner, selected.index, current, locate)
        self.search_dialog = dialog

        def dismissed(_):
            if self.search_dialog is dialog: self.search_dialog = None
            dialog.deleteLater()

        dialog.finished.connect(dismissed)
        dialog.open()

    def compare_to_current(self):
        selected = self.selected
        if selected is None or self.owner.pending is not None: return
        archive = self.owner.archive
        if archive.selection is None:
            self.owner.inform("请先在这个窗口打开并核验一个 .zxs 快照，再比较这份普通 ZIP。")
        elif archive.comparison.compare_ordinary_zip(selected):
            self.owner.tabs.setCurrentWidget(archive)
            archive.views.setCurrentWidget(archive.comparison)

    def choose_destination(self):
        selected = self.selected
        if self.extract is None or selected is None or self.owner.pending is not None: return
        from .member_copy import PlainZipCopy
        opts = (QFileDialog.Option.DontUseNativeDialog | QFileDialog.Option.ReadOnly |
                QFileDialog.Option.DontResolveSymlinks | QFileDialog.Option.DontConfirmOverwrite)
        name, _ = QFileDialog.getSaveFileName(self, "选择目的父目录和一个新文件夹名", "", "所有文件 (*)", options=opts)
        if not name or self.selected is not selected: return
        plan = None
        try:
            plan = PlainZipCopy(selected, name)
            scratch = QFileDialog.getExistingDirectory(self, "选择同一挂载、本人独用且不与目的父交叠的 0700 暂存目录",
                                                       "", opts | QFileDialog.Option.ShowDirsOnly)
            if not scratch or self.selected is not selected:
                plan.close()
                return
            plan.pick_private(scratch)
        except BaseException as err:
            if plan is not None: plan.close()
            if not isinstance(err, Exception): raise
            self.owner.inform(str(err))
            return

        def submit(title, call, apply, *, cancellable=False):
            try:
                options = {"cancel": plan.cancel, "progress_report": plan.progress,
                           "cancel_text": "取消本次提取（发布前）"} if cancellable else {}
                future = self.owner.job(title, call, apply, failed=lambda _error: plan.close(), **options)
                if future is None: plan.close()
                else: future.add_done_callback(lambda task: plan.close() if task.cancelled() else None)
            except BaseException:
                plan.close()
                raise

        def review(details):
            try:
                if (self.selected is selected and confirm(self, "提取此普通 ZIP，不覆盖", details,
                        attestation="原包及两选定目录没有其他写者或同步，不会移位；我独用此私处，并同意在此留下失败材料。",
                        action_text="提取并核验") and self.selected is selected):
                    submit("正在提取到私处、完整读回并尝试正式发布…",
                           lambda: plan.run(confirmed=True), self.owner.inform, cancellable=True)
                else:
                    plan.close()
                    self.owner.inform("没有提取或写入任何文件。")
            except BaseException:
                plan.close()
                raise

        submit("正在核对原普通 ZIP 和两处目的目录；尚未写入…", plan.review, review)
