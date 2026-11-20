"""Ordinary ZIP preview and a separate, explicit Linux extraction action."""
import sys

from PySide6.QtWidgets import QFileDialog, QVBoxLayout, QWidget

from .. import plain_import
from .selection import PlainSelection
from .widgets import button, confirm, label


class ImportPane(QWidget):
    def __init__(self, owner):
        super().__init__()
        self.owner = owner
        self.selected = None
        layout = QVBoxLayout(self)
        layout.addWidget(label("选择普通 Store ZIP，核验全部成员、整树摘要及原包摘要。不提取文件，不登记工作树。"))
        self.choose = button("选择 ZIP…", self.pick)
        layout.addWidget(self.choose)
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
        layout.addStretch()

    def release_display(self):
        if self.selected is not None:
            self.selected.release_display()
            self.selected = None
        if self.extract is not None: self.extract.setEnabled(False)

    def pick(self):
        if self.owner.pending is not None: return
        source, _ = QFileDialog.getOpenFileName(self, "选择普通 Store ZIP", "", "普通 ZIP (*.zip *.ZIP)")
        if not source: return
        self.release_display()
        self.details.setText("尚无本次核验结果。")
        if self.extract is not None:
            self.owner.job("正在只读核验整个 ZIP；不会写入文件…",
                           lambda: PlainSelection.open(source), self.show_selection)
        else:
            self.owner.job("正在核验整个 ZIP；不会写入文件…",
                           lambda: plain_import.inspect(source), self.show_preview)

    def show_selection(self, selected):
        self.release_display()
        self.selected = selected
        self.extract.setEnabled(True)
        self.show_preview(selected.preview)

    def show_preview(self, preview):
        self.details.setText(f"来源：{preview.source}\n完整 ZIP 的 SHA-256：{preview.sha256}\n"
                             f"整树 SHA-256：{preview.tree}\n"
                             f"{preview.count} 个目录项，{preview.size} 字节文件内容")
        self.owner.inform("只读核验完成；没有创建或更改任何目录。")

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
