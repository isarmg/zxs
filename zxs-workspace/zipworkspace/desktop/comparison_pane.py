"""Compare with any chosen folder. This is not a registered worktree status."""
from PySide6.QtWidgets import QFileDialog, QHBoxLayout, QVBoxLayout, QWidget

from .. import comparison
from ..model import Rejected
from .widgets import Table, button, label


class ComparisonPane(QWidget):
    def __init__(self, owner):
        super().__init__()
        self.owner = owner
        layout = QVBoxLayout(self)
        layout.addWidget(label("与已打开的快照逐项比较，只读取所选文件夹。此比较不登记工作树，不是工作树状态，也不会同步文件。"))
        self.choose = button("选择文件夹并比较…", self.pick)
        layout.addWidget(self.choose)
        self.summary = label()
        layout.addWidget(self.summary)
        self.table = Table(("相对路径", "本机相对快照", "快照中", "本机中"), "目录对比结果")
        layout.addWidget(self.table)
        nav = QHBoxLayout()
        self.back = button("上一页", lambda: self.show_page(self.number-1))
        self.more = button("下一页", lambda: self.show_page(self.number+1))
        nav.addWidget(self.back); nav.addWidget(self.more)
        self.page_label = label(); nav.addWidget(self.page_label)
        layout.addLayout(nav)
        self.set_snapshot(None)

    def set_snapshot(self, snapshot):
        self.snapshot = snapshot
        self.clear()
        enabled = comparison.available()
        self.choose.setEnabled(snapshot is not None and enabled)
        self.summary.setText("请先打开并核验一个快照。" if snapshot is None else
            f"当前快照：{snapshot.path}\n节点：{snapshot.browser.commit.operation}\n"
            + ("尚未比较本机目录。" if enabled else "此平台未提供可核验的目录读取能力。"))

    def clear(self):
        self.report, self.number = None, 0
        self.table.rows.replace(())
        self.back.setEnabled(False); self.more.setEnabled(False)
        self.page_label.setText("")

    def pick(self):
        if self.snapshot is None: return
        name = QFileDialog.getExistingDirectory(self, "选择只读比较的本机文件夹")
        if name: self.compare_path(name)

    def compare_path(self, name):
        chosen = self.snapshot
        if chosen is None or not comparison.available(): return
        self.clear()
        self.summary.setText(f"等待核验：{name}\n快照节点：{chosen.browser.commit.operation}")
        def work():
            return comparison.compare(chosen, folder)
        def done(report):
            if self.snapshot is not chosen: return
            self.report = report
            self.summary.setText(f"快照：{chosen.path} · 节点 {report.operation}\n本次观察：{report.folder.path}\n"
                                 f"新增 {report.added} · 修改 {report.changed} · 缺少 {report.missing}。"
                                 + ("没有差异。" if not report.rows else ""))
            self.show_page(0)
            self.owner.inform("只读比较完成；后续编辑不会自动更新这份结果。")
        def failed(err):
            if self.snapshot is chosen:
                self.summary.setText(f"未取得完整比较结果：{err}")
        try:
            folder = comparison.Folder.choose(name)  # Bind before queuing the slow inventory.
        except (OSError, Rejected) as err:
            failed(err)
            return
        try:
            submitted = self.owner.job("正在只读核验所选文件夹…", work, done, failed=failed)
        except BaseException as err:
            folder.close()
            if not isinstance(err, Exception): raise
            failed(err)
            return
        if submitted is None:
            folder.close()
            failed(Rejected("另一个操作正在执行；未开始比较。"))
        else:
            # A cancelled queued job never enters compare() to release its FD.
            submitted.add_done_callback(lambda future: folder.close() if future.cancelled() else None)

    def show_page(self, number):
        if self.report is None: return
        rows = self.report.page(number)
        kind = {"directory":"目录", "file":"文件", None:"无"}
        change = {"added":"新增", "changed":"修改", "missing":"缺少"}
        self.table.rows.replace(((r.path,change[r.change],kind[r.snapshot],kind[r.local]), r) for r in rows)
        self.number = number
        count = max(1, (len(self.report.rows)+self.report.PAGE-1)//self.report.PAGE)
        self.page_label.setText(f"第 {number+1}/{count} 页")
        self.back.setEnabled(number > 0); self.more.setEnabled(number+1 < count)
