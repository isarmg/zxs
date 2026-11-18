"""Explicit ordinary ZIP import; never registers a worktree or opens private SQLite."""
from pathlib import Path
from PySide6.QtWidgets import QFileDialog, QInputDialog, QVBoxLayout, QWidget

from .. import plain_import
from ..model import Rejected, member_name
from ..publish import require_native_writer
from .widgets import button, confirm, label


class ImportPane(QWidget):
    def __init__(self, owner):
        super().__init__()
        self.owner = owner
        self.proposal = None
        layout = QVBoxLayout(self)
        layout.addWidget(label("从普通 Store ZIP 还原到新的普通目录，保留文件、层级和空目录。不会登记工作树或创建版本。"))
        self.choose = button("选择 ZIP 和新目录…", self.pick)
        self.start = button("确认导入…", self.run)
        layout.addWidget(self.choose)
        self.details = label("尚未选择文件。")
        layout.addWidget(self.details)
        layout.addWidget(self.start)
        self.outcome = label()
        layout.addWidget(self.outcome)
        layout.addStretch()
        try:
            require_native_writer()
            self.writer_error = None
        except (OSError,Rejected) as err:
            self.writer_error = str(err)
            self.outcome.setText("此平台可核验所选 ZIP，但不开放导入写入。"+self.writer_error)
        self.start.setEnabled(False)

    def reset(self):
        self.proposal = None
        self.details.setText("尚无已核验的 ZIP 和新目录。")
        self.start.setEnabled(False)

    def pick(self):
        self.reset()
        self.outcome.setText("")
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
        self.start.setEnabled(self.writer_error is None)
        self.owner.inform("已核验。尚未写文件。" if self.writer_error is None else "只读核验成功；当前平台不支持安全导入写入。")

    def run(self):
        proposal = self.proposal
        if proposal is None or self.writer_error is not None: return
        if not confirm(self,"确认导入普通 ZIP",self.details.text()+"\n只创建上述新目录；没有工作树或历史操作。"):
            self.owner.inform("已取消；没有写文件。")
            return
        self.proposal = None
        self.start.setEnabled(False)
        def done(result):
            self.outcome.setText(f"已核实 {proposal.target} 的 {len(result.rows)} 个目录项，{result.total} 字节。")
            self.owner.inform("普通目录导入完成。没有登记工作树。")
        def failed(err):
            self.outcome.setText(f"没有取得已核验的导入回执：{err}\n请查看目标和保留的暂存目录；不会自动覆盖、删除或重试。")
        self.owner.job("正在导入并独立核验最终目录…",lambda: plain_import.execute(proposal),done,failed=failed)
