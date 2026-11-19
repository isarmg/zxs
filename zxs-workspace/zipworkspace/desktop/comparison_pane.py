"""One read-only result view, either a local folder or one explicitly selected catalog version."""
from PySide6.QtWidgets import QFileDialog, QHBoxLayout, QVBoxLayout, QWidget

from .. import comparison
from ..model import Rejected
from .selection import compare_version as compare_listed_version
from .widgets import Table, button, label


class ComparisonPane(QWidget):
    def __init__(self, owner):
        super().__init__()
        self.owner = owner
        layout = QVBoxLayout(self)
        layout.addWidget(label("可以比较本机文件夹，也可以从「历史」或「分支」选定另一归档版本。两种比较都只读，不同步文件。"))
        self.choose = button("选择文件夹并比较…", self.pick)
        layout.addWidget(self.choose)
        self.summary = label()
        layout.addWidget(self.summary)
        self.table = Table(("相对路径", "差异", "当前快照", "对照对象"), "只读版本或目录对比结果")
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
            + ("尚未开始比较。" if enabled else "本机目录比较在此平台不可用；也可以从历史或分支选择版本对照。"))

    def clear(self):
        self.report, self.number, self.mode, self.request = None, 0, None, None
        self.table.rows.replace(())
        self.back.setEnabled(False); self.more.setEnabled(False)
        self.page_label.setText("")

    def invalidate_versions(self):
        if self.mode == "version":
            self.set_snapshot(self.snapshot)
            self.owner.inform("已清除原版本对照。")

    def matches(self, snapshot, request):
        return self.snapshot is snapshot and self.request is request

    def pick(self):
        if self.snapshot is None: return
        # Pass the selected spelling to the provider rather than having Qt resolve a link for it.
        options = QFileDialog.Option.ShowDirsOnly | QFileDialog.Option.DontResolveSymlinks
        name = QFileDialog.getExistingDirectory(self, "选择只读比较的本机文件夹", "", options)
        if name: self.compare_path(name)

    def compare_path(self, name):
        chosen = self.snapshot
        if chosen is None or not comparison.available(): return
        self.clear()
        request = self.request = object()
        self.mode = "folder"
        self.summary.setText(f"等待核验：{name}\n快照节点：{chosen.browser.commit.operation}")
        def work():
            return comparison.compare(chosen, folder)
        def done(report):
            if not self.matches(chosen, request): return
            self.report = report
            self.summary.setText(f"快照：{chosen.path} · 节点 {report.operation}\n本次观察：{report.folder.path}\n"
                                 f"新增 {report.added} · 修改 {report.changed} · 缺少 {report.missing}。"
                                 + ("没有差异。" if not report.rows else ""))
            self.show_page(0)
            self.owner.inform("只读比较完成；后续编辑不会自动更新这份结果。")
        def failed(err):
            if self.matches(chosen, request):
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

    def compare_version(self, node, store, catalog):
        chosen = self.snapshot
        if chosen is None or self.owner.pending is not None: return False
        self.clear()
        self.mode = "version"
        request = self.request = object()

        def describe(side, path, marker):
            return (f"{side}：{path.name} · 第 {marker.generation} 代\n"
                    f"节点 {marker.operation} · 分支 {marker.branch}")

        pair = (describe("此前已核验的当前视图", chosen.path, chosen.browser.commit) + "\n" +
                describe("本次选择的原归档", node.path, node.marker))
        self.summary.setText(pair + "\n正在重新核验右侧原归档…")

        def work():
            # Treat this read's outcome as data, so Window.poll cannot report a
            # stale exception on the main status bar before we check the request.
            try: return compare_listed_version(chosen, store, catalog, node), None
            except Exception as err: return None, err

        def failed(err):
            if self.matches(chosen, request):
                self.summary.setText(pair + f"\n未完成版本比较：{err}")
                self.owner.inform("未完成版本比较；请查看对比页。")

        def done(outcome):
            if not self.matches(chosen, request): return
            report, error = outcome
            if error is not None:
                failed(error)
                return
            self.report = report
            self.summary.setText(pair + f"\n仅所选 {report.added} · 不同 {report.changed} · 仅当前 {report.missing}。"
                                 + ("树内容没有差异。" if not report.rows else ""))
            self.show_page(0)
            self.owner.inform("所选原版本已重新核验。结果不跟随最新分支头，也不是恢复或工作树状态。")

        try:
            submitted = self.owner.job("正在核验所选原版本的完整归档…", work, done)
        except Exception as err:
            failed(err)
        else:
            if submitted is None:
                failed(Rejected("另一个操作正在执行；未开始比较。"))
        return True

    def show_page(self, number):
        if self.report is None: return
        rows = self.report.page(number)
        kind = {"directory":"目录", "file":"文件", None:"无"}
        revision = self.mode == "version"
        change = ({"added":"仅所选", "changed":"不同", "missing":"仅当前"} if revision else
                  {"added":"新增", "changed":"修改", "missing":"缺少"})
        def side(role, size):
            return f"{kind[role]} · {size:,} B" if revision and size is not None else kind[role]
        self.table.rows.replace(((r.path, change[r.change], side(r.before, r.before_bytes),
                                  side(r.after, r.after_bytes)), r) for r in rows)
        self.number = number
        count = max(1, (len(self.report.rows)+self.report.PAGE-1)//self.report.PAGE)
        self.page_label.setText(f"第 {number+1}/{count} 页")
        self.back.setEnabled(number > 0); self.more.setEnabled(number+1 < count)
