"""Read-only results for a local folder, a catalog version or a verified ordinary ZIP."""
from PySide6.QtWidgets import QFileDialog, QHBoxLayout, QVBoxLayout, QWidget

from .. import comparison
from ..model import Rejected
from .selection import Verification, compare_snapshot_file, compare_version as compare_listed_version
from .widgets import Table, button, label


class ComparisonPane(QWidget):
    def __init__(self, owner):
        super().__init__()
        self.owner = owner
        self.active_read = None
        layout = QVBoxLayout(self)
        layout.addWidget(label("可以直接选另一份快照、比本机文件夹、从历史选版本，或比较普通 ZIP。所有比较只读，不同步文件。"))
        targets = QHBoxLayout()
        self.choose = button("选择文件夹并比较…", self.pick)
        self.choose_snapshot = button("选择另一份 .zxs 并比较…", self.pick_snapshot)
        targets.addWidget(self.choose); targets.addWidget(self.choose_snapshot)
        layout.addLayout(targets)
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
        self.choose_snapshot.setEnabled(snapshot is not None)
        self.summary.setText("请先打开并核验一个快照。" if snapshot is None else
            f"当前快照：{snapshot.path}\n节点：{snapshot.browser.commit.operation}\n"
            + ("尚未开始比较。" if enabled else "本机目录比较在此平台不可用；也可以从历史或分支选择版本对照。"))

    def clear(self):
        previous, self.active_read = self.active_read, None
        if previous is not None: previous.cancel()
        self.report, self.number, self.mode, self.request = None, 0, None, None
        self.plain = None
        self.table.rows.replace(())
        self.back.setEnabled(False); self.more.setEnabled(False)
        self.page_label.setText("")

    def invalidate_versions(self):
        if self.mode == "version":
            self.set_snapshot(self.snapshot)
            self.owner.inform("已清除原版本对照。")

    def invalidate_plain(self, selection):
        if self.mode == "plain" and self.plain is selection:
            self.clear()
            self.summary.setText("原普通 ZIP 已更换或关闭；旧对照已清除。")
            self.owner.inform("已清除普通 ZIP 对照。")

    def matches(self, snapshot, request):
        return self.snapshot is snapshot and self.request is request

    def pick_snapshot(self):
        chosen = self.snapshot
        if chosen is None or self.owner.pending is not None: return
        name, _ = QFileDialog.getOpenFileName(self, "选择用于内容对照的另一份 .zxs", "", "目录快照 (*.zxs *.ZXS)",
                                             options=QFileDialog.Option.ReadOnly)
        if name and self.snapshot is chosen: self.compare_file(name)

    def compare_file(self, name):
        chosen = self.snapshot
        if chosen is None or self.owner.pending is not None: return False
        self.clear()
        self.mode = "file"
        request = self.request = object()
        observer = self.active_read = Verification()
        pair = (f"当前快照：{str(chosen.path)!r} · 节点 {chosen.browser.commit.operation}\n"
                f"本次所选：{str(name)!r}\n只比较两个归档内容，不推断仓库、先后或共同历史。")
        self.summary.setText(pair + "\n正在核验所选 .zxs…")

        def work():
            try: return compare_snapshot_file(chosen, name, observer=observer), None
            except Exception as error: return None, error

        def done(outcome):
            if not self.matches(chosen, request): return
            self.active_read = None
            result, error = outcome
            if observer.cancelled:
                self.summary.setText(pair + "\n已取消；未显示本次对照。")
                self.owner.inform("已取消快照文件对照，当前快照未更改。")
            elif error is not None:
                self.summary.setText(pair + f"\n未完成文件对照：{error}")
                self.owner.inform("未完成快照文件对照；请查看对比页。")
            else:
                _, marker, self.report = result
                delta = self.report
                self.summary.setText(pair + f"\n所选节点：{marker.operation}"
                                     f"\n仅所选 {delta.added} · 不同 {delta.changed} · 仅当前 {delta.missing}。"
                                     + ("逻辑树内容没有差异。" if not delta.rows else ""))
                self.show_page(0)
                self.owner.inform("只读的快照内容对照已完成，未切换快照、归档存储或工作树。")

        def progress():
            return observer.progress() if self.matches(chosen, request) else ("", 0, None)

        try:
            future = self.owner.job("正在只读核验另一份快照的内容…", work, done, cancel=observer.cancel,
                                    cancel_text="取消快照文件对照", progress_report=progress)
        except Exception as error:
            done((None, error))
        else:
            if future is None: done((None, Rejected("另一个任务正在执行；本次未启动")))
        return True

    def pick(self):
        chosen = self.snapshot
        if chosen is None or self.owner.pending is not None: return
        # Pass the selected spelling to the provider rather than having Qt resolve a link for it.
        options = (QFileDialog.Option.ShowDirsOnly | QFileDialog.Option.DontResolveSymlinks |
                   QFileDialog.Option.DontUseNativeDialog | QFileDialog.Option.ReadOnly)
        name = QFileDialog.getExistingDirectory(self, "选择只读比较的本机文件夹", "", options)
        if name and self.snapshot is chosen: self.compare_path(name)

    def compare_path(self, name):
        chosen = self.snapshot
        if chosen is None or self.owner.pending is not None or not comparison.available(): return
        self.clear()
        request = self.request = object()
        observer = self.active_read = Verification()
        self.mode = "folder"
        self.summary.setText(f"等待核验：{name}\n快照节点：{chosen.browser.commit.operation}")
        def work():
            try: return comparison.compare(chosen, folder, observer=observer), None
            except Exception as error: return None, error
        def done(outcome):
            if not self.matches(chosen, request): return
            self.active_read = None
            if observer.cancelled:
                self.summary.setText("已取消本次文件夹对照，保留当前浏览快照。未显示部分结果。")
                self.owner.inform("已取消文件夹对照；没有写入。")
                return
            report, error = outcome
            if error is not None:
                failed(error)
                return
            self.report = report
            self.summary.setText(f"快照：{chosen.path} · 节点 {report.operation}\n本次观察：{report.folder.path}\n"
                                 f"新增 {report.added} · 修改 {report.changed} · 缺少 {report.missing}。"
                                 + ("没有差异。" if not report.rows else ""))
            self.show_page(0)
            self.owner.inform("只读比较完成；后续编辑不会自动更新这份结果。")
        def failed(err):
            if self.matches(chosen, request):
                self.active_read = None
                self.summary.setText(f"未取得完整比较结果：{err}")
                self.owner.inform("未完成文件夹对照；请查看对比页。")
        def progress():
            return observer.progress() if self.matches(chosen, request) else ("", 0, None)

        try:
            folder = comparison.Folder.choose(name)  # Bind before queuing the slow inventory.
        except (OSError, Rejected) as err:
            failed(err)
            return
        try:
            submitted = self.owner.job("正在只读核验所选文件夹…", work, done, cancel=observer.cancel,
                                       cancel_text="取消文件夹对照", progress_report=progress)
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
        observer = self.active_read = Verification()

        def describe(side, path, marker):
            return (f"{side}：{path.name} · 第 {marker.generation} 代\n"
                    f"节点 {marker.operation} · 分支 {marker.branch}")

        pair = (describe("此前已核验的当前视图", chosen.path, chosen.browser.commit) + "\n" +
                describe("本次选择的原归档", node.path, node.marker))
        self.summary.setText(pair + "\n正在重新核验右侧原归档…")

        def work():
            # Treat this read's outcome as data, so Window.poll cannot report a
            # stale exception on the main status bar before we check the request.
            try: return compare_listed_version(chosen, store, catalog, node, observer=observer), None
            except Exception as err: return None, err

        def failed(err):
            if self.matches(chosen, request):
                self.active_read = None
                self.summary.setText(pair + f"\n未完成版本比较：{err}")
                self.owner.inform("未完成版本比较；请查看对比页。")

        def done(outcome):
            if not self.matches(chosen, request): return
            self.active_read = None
            if observer.cancelled:
                self.summary.setText(pair + "\n已取消本次版本对照；未显示本次结果。")
                self.owner.inform("已取消版本对照；没有改变当前快照或写入。")
                return
            report, error = outcome
            if error is not None:
                failed(error)
                return
            self.report = report
            self.summary.setText(pair + f"\n仅所选 {report.added} · 不同 {report.changed} · 仅当前 {report.missing}。"
                                 + ("树内容没有差异。" if not report.rows else ""))
            self.show_page(0)
            self.owner.inform("所选原版本已重新核验。结果不跟随最新分支头，也不是恢复或工作树状态。")

        def progress():
            return observer.progress() if self.matches(chosen, request) else ("", 0, None)

        try:
            submitted = self.owner.job("正在核验所选原版本的完整归档…", work, done,
                                       cancel=observer.cancel, cancel_text="取消版本对照", progress_report=progress)
        except Exception as err:
            failed(err)
        else:
            if submitted is None:
                failed(Rejected("另一个操作正在执行；未开始比较。"))
        return True

    def compare_ordinary_zip(self, plain):
        chosen = self.snapshot
        if chosen is None or self.owner.pending is not None: return False
        self.clear()
        request = self.request = object()
        self.mode, self.plain = "plain", plain
        directory, rows = chosen.browser, plain.rows
        pair = (f"此前已核验的快照：{str(chosen.path)!r} · 节点 {directory.commit.operation}\n"
                f"此前已核验的普通 ZIP：{str(plain.path)!r} · SHA-256 {plain.preview.sha256}\n"
                "只比较这两次读取的内容。不会重新打开路径或从 ZIP 注释取得版本身份。")
        self.summary.setText(pair + "\n正在比较已核目录…")

        def work():
            try: return comparison.compare_plain(directory, rows), None
            except Exception as err: return None, str(err)

        def done(outcome):
            if not self.matches(chosen, request) or self.plain is not plain: return
            report, error = outcome
            if error is not None:
                self.summary.setText(pair + f"\n未完成比较：{error}")
                self.owner.inform("未完成普通 ZIP 比较；请查看对比页。")
                return
            self.report = report
            self.summary.setText(pair + f"\n仅普通 ZIP {report.added} · 不同 {report.changed} · 仅快照 {report.missing}。"
                                 + ("树内容没有差异；此处未比较两个归档的原始字节。" if not report.rows else ""))
            self.show_page(0)
            self.owner.inform("完成两份已核验内容的只读比较；没有提取、同步或修改来源。")

        try:
            submitted = self.owner.job("正在比较两份已核验目录，不读取归档正文…", work, done)
        except Exception as err:
            done((None, str(err)))
        else:
            if submitted is None:
                done((None, "另一个操作正在执行。"))
        return True

    def show_page(self, number):
        if self.report is None: return
        rows = self.report.page(number)
        kind = {"directory":"目录", "file":"文件", None:"无"}
        changes = {
            "version": {"added":"仅所选", "changed":"不同", "missing":"仅当前"},
            "file": {"added":"仅所选", "changed":"不同", "missing":"仅当前"},
            "plain": {"added":"仅普通 ZIP", "changed":"不同", "missing":"仅快照"},
            "folder": {"added":"新增", "changed":"修改", "missing":"缺少"},
        }
        change = changes[self.mode]
        def side(role, size):
            return f"{kind[role]} · {size:,} B" if self.mode != "folder" and size is not None else kind[role]
        self.table.rows.replace(((r.path, change[r.change], side(r.before, r.before_bytes),
                                  side(r.after, r.after_bytes)), r) for r in rows)
        self.number = number
        count = max(1, (len(self.report.rows)+self.report.PAGE-1)//self.report.PAGE)
        self.page_label.setText(f"第 {number+1}/{count} 页")
        self.back.setEnabled(number > 0); self.more.setEnabled(number+1 < count)
