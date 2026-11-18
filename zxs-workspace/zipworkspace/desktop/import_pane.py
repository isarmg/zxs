"""Read-only ordinary ZIP preview; there is no file publication or cleanup action."""
from pathlib import Path
from PySide6.QtWidgets import QFileDialog, QInputDialog, QVBoxLayout, QWidget

from .. import plain_import
from ..model import Rejected, member_name
from .widgets import button, label


class ImportPane(QWidget):
    def __init__(self, owner):
        super().__init__()
        self.owner = owner
        self.proposal = None
        layout = QVBoxLayout(self)
        layout.addWidget(label("核验普通 Store ZIP 的全部成员及原包摘要，并查看拟用目标。预览不提取、不登记工作树。"))
        self.choose = button("选择 ZIP 和拟用新目录…", self.pick)
        layout.addWidget(self.choose)
        self.details = label("尚未选择文件。")
        layout.addWidget(self.details)
        self.outcome = label(plain_import.OUTPUT_UNAVAILABLE)
        layout.addWidget(self.outcome)
        layout.addStretch()

    def reset(self):
        self.proposal = None
        self.details.setText("尚无已核验的 ZIP 和新目录。")

    def pick(self):
        self.reset()
        source, _ = QFileDialog.getOpenFileName(self,"选择普通 Store ZIP","","普通 ZIP (*.zip *.ZIP)")
        if not source: return
        parent = QFileDialog.getExistingDirectory(self,"选择新目录的父目录")
        if not parent: return
        name, ok = QInputDialog.getText(self,"新目录名称","请输入一个新目录名。现有名称不会覆盖。",text=Path(source).stem)
        if not ok: return
        try:
            if len(member_name(name)) != 1: raise Rejected("只能输入一个目录名。")
        except Rejected as err:
            self.owner.inform(err); return
        target = Path(parent) / name
        self.owner.job("正在核验整个 ZIP 和所选目的；此阶段不写文件…",
                       lambda: plain_import.inspect(source,target),self.show_proposal)

    def show_proposal(self, proposal):
        self.proposal = proposal
        self.details.setText(f"来源：{proposal.source}\n完整 ZIP 的 SHA-256：{proposal.sha256}\n"
                             f"{proposal.count} 个目录项，{proposal.size} 字节文件内容\n新目录：{proposal.target}")
        self.owner.inform("只读核验完成；没有创建或更改任何目录。")
