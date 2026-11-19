"""Read-only ordinary ZIP preview; there is no publication or cleanup action."""
from PySide6.QtWidgets import QFileDialog, QVBoxLayout, QWidget

from .. import plain_import
from .widgets import button, label


class ImportPane(QWidget):
    def __init__(self, owner):
        super().__init__()
        self.owner = owner
        layout = QVBoxLayout(self)
        layout.addWidget(label("选择普通 Store ZIP，核验全部成员、整树摘要及原包摘要。不提取文件，不登记工作树。"))
        self.choose = button("选择 ZIP…", self.pick)
        layout.addWidget(self.choose)
        self.details = label("尚未选择文件。")
        layout.addWidget(self.details)
        self.outcome = label(plain_import.OUTPUT_UNAVAILABLE)
        layout.addWidget(self.outcome)
        layout.addStretch()

    def pick(self):
        source, _ = QFileDialog.getOpenFileName(self, "选择普通 Store ZIP", "", "普通 ZIP (*.zip *.ZIP)")
        if not source: return
        self.details.setText("尚无本次核验结果。")
        self.owner.job("正在核验整个 ZIP；不会写入文件…",
                       lambda: plain_import.inspect(source), self.show_preview)

    def show_preview(self, preview):
        self.details.setText(f"来源：{preview.source}\n完整 ZIP 的 SHA-256：{preview.sha256}\n"
                             f"整树 SHA-256：{preview.tree}\n"
                             f"{preview.count} 个目录项，{preview.size} 字节文件内容")
        self.owner.inform("只读核验完成；没有创建或更改任何目录。")
