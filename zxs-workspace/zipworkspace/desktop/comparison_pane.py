"""Read-only results for a local folder, a catalog version or a verified ordinary ZIP."""
from PySide6.QtWidgets import QFileDialog, QHBoxLayout, QVBoxLayout, QWidget

from .. import comparison
from ..model import Rejected
from .selection import PlainComparison, Verification, compare_snapshot_file, compare_version as compare_listed_version
from .text_preview import can_compare_text, compare_folder_text, compare_plain_text, compare_text
from .widgets import Table, button, confirm, label, line_diff_dialog


class ComparisonPane(QWidget):
    def __init__(self, owner):
        super().__init__()
        self.owner = owner
        self.active_read = self.report = self.right = self.text_read = self.text_dialog = self.row_epoch = None
        layout = QVBoxLayout(self)
        layout.addWidget(label("可以直接选另一份快照、比本机文件夹、从历史选版本，或比较普通 ZIP。所有比较只读，不同步文件。"))
        targets = QHBoxLayout()
        self.choose = button("选择文件夹并比较…", self.pick)
        self.choose_snapshot = button("选择另一份 .zxs 并比较…", self.pick_snapshot)
        targets.addWidget(self.choose); targets.addWidget(self.choose_snapshot)
        targets.addWidget(button("关闭本次对照", self.close_report))
        layout.addLayout(targets)
        self.summary = label()
        layout.addWidget(self.summary)
        self.table = Table(("相对路径", "差异", "当前快照", "对照对象"), "只读版本或目录对比结果")
        layout.addWidget(self.table)
        self.lines = button("查看所选文件的逐行差异…", self.read_lines)
        layout.addWidget(self.lines)
        self.table.selectionModel().selectionChanged.connect(self._row_changed)
        self.table.rows.modelReset.connect(self._row_changed)
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
        report = self.report
        self.report, self.number, self.mode, self.request = None, 0, None, None
        self.plain = None
        self.close_lines()
        if isinstance(report, comparison.Report): report.close()
        right, self.right = self.right, None
        if right is not None: right.release_display()
        self.table.rows.replace(())
        self.back.setEnabled(False); self.more.setEnabled(False)
        self.page_label.setText("")

    def invalidate_versions(self):
        if self.mode == "version":
            self.set_snapshot(self.snapshot)
            self.owner.inform("已清除原版本对照。")

    def close_report(self):
        if self.request is None: return
        self.set_snapshot(self.snapshot)
        self.owner.inform("已清除本次对照并释放其原件，当前浏览快照不变。")

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
            try: return compare_snapshot_file(chosen, name, observer=observer, retain_right=True), None
            except Exception as error: return None, error

        def done(outcome):
            result, error = outcome
            if not self.matches(chosen, request) or observer.cancelled:
                if result is not None: result.right.release_display()
                if not self.matches(chosen, request): return
            self.active_read = None
            if observer.cancelled:
                self.summary.setText(pair + "\n已取消；未显示本次对照。")
                self.owner.inform("已取消快照文件对照，当前快照未更改。")
            elif error is not None:
                self.summary.setText(pair + f"\n未完成文件对照：{error}")
                self.owner.inform("未完成快照文件对照；请查看对比页。")
            else:
                self.right, self.report = result.right, result.changes
                marker = self.right.browser.commit
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
            try: return comparison.compare(chosen, folder, observer=observer, retain_text=True), None
            except Exception as error: return None, error
        def done(outcome):
            report, error = outcome
            if report is not None and (not self.matches(chosen, request) or observer.cancelled): report.close()
            if not self.matches(chosen, request): return
            self.active_read = None
            if observer.cancelled:
                self.summary.setText("已取消本次文件夹对照，保留当前浏览快照。未显示部分结果。")
                self.owner.inform("已取消文件夹对照；没有写入。")
                return
            if error is not None:
                failed(error)
                return
            self.report = report
            self.summary.setText(f"快照：{chosen.path} · 节点 {report.operation}\n本次观察：{report.folder.path}\n"
                                 f"新增 {report.added} · 修改 {report.changed} · 缺少 {report.missing}。"
                                 + ("没有差异。" if not report.rows else "")
                                 + ("\n可另行核验有凭据的小文件对，查看逐行差异。" if report.text is not None else
                                    "\n本次没有可逐行复核的原文件对。" if report.rows else ""))
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
            # Bind before queuing the slow inventory. A held Darwin archive
            # uses only original, literal, no-link name validation from here.
            folder = comparison.choose_comparison_folder(name, literal=chosen.held is not None)
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
            try: return compare_listed_version(chosen, store, catalog, node, observer=observer, retain_right=True), None
            except Exception as err: return None, err

        def failed(err):
            if self.matches(chosen, request):
                self.active_read = None
                self.summary.setText(pair + f"\n未完成版本比较：{err}")
                self.owner.inform("未完成版本比较；请查看对比页。")

        def done(outcome):
            result, error = outcome
            if not self.matches(chosen, request) or observer.cancelled:
                if result is not None: result.right.release_display()
                if not self.matches(chosen, request): return
            self.active_read = None
            if observer.cancelled:
                self.summary.setText(pair + "\n已取消本次版本对照；未显示本次结果。")
                self.owner.inform("已取消版本对照；没有改变当前快照或写入。")
                return
            if error is not None:
                failed(error)
                return
            self.right, self.report = result.right, result.changes
            report = self.report
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

    def _line_selection(self, row):
        left = self.snapshot
        if not can_compare_text(row) or left is None or left.held is None or left.held.closed: return False
        if self.mode == "folder":
            return (self.report is not None and self.report.text is not None and self.report.text.supports(left, row))
        if self.mode == "plain":
            report, plain = self.report, self.plain
            return (isinstance(report, PlainComparison) and report.left is left and report.plain is plain
                    and plain.held is not None and not plain.held.closed and report.contains(row))
        return (self.mode in ("file", "version") and self.right is not None and self.right.held is not None
                and not self.right.held.closed)

    def close_lines(self):
        read, self.text_read = self.text_read, None
        if read is not None: read.cancel()
        dialog, self.text_dialog = self.text_dialog, None
        if dialog is not None: dialog.reject()

    def _row_changed(self, *_):
        self.row_epoch = object()
        self.close_lines()
        self.lines.setEnabled(self._line_selection(self.table.item()))

    def read_lines(self):
        left, right, row = self.snapshot, self.right, self.table.item()
        report, origin, picked, plain = self.report, self.request, self.row_epoch, self.plain
        if (not self._line_selection(row) or self.owner.pending is not None or self.text_read is not None
                or self.text_dialog is not None):
            return

        def same_pair():
            return (self.matches(left, origin) and self.report is report and self.right is right and self.row_epoch is picked
                    and self.plain is plain and self.table.item() is row and self._line_selection(row))

        in_folder = self.mode == "folder"
        in_plain = self.mode == "plain"
        if in_folder: other = f"本次观察的原文件夹：{str(report.folder.path)!r}"
        elif in_plain: other = f"此前已核验的普通 ZIP：{str(plain.path)!r}"
        else: other = f"所选节点：{right.browser.commit.operation}"
        detail = (f"相对路径：{row.path!r}\n当前快照：{str(left.path)!r}\n"
                  f"节点 {left.browser.commit.operation} · {row.before_bytes:,} B\n{other} · {row.after_bytes:,} B")
        if in_folder: reading = "只复核这次观察的原文件和对应归档成员，不重扫或改用其他文件。"
        elif in_plain: reading = "只从两份原句柄复核这一个成员。不按名称重开、不重新计算整包摘要或解释普通 ZIP 注释。"
        else: reading = "只读取两份原归档中这一个成员的完整正文。"
        if (not confirm(self, "核验后查看逐行差异", detail + "\n" + reading
                        + "结果只在此处以纯文本显示，换行采用预览规则；每侧限 64 KiB、1,024 行，可取消。",
                        attestation="我确认只看上面两份原件的这个文本，不打开外部应用。",
                        action_text="核验并比较") or not same_pair() or self.owner.pending is not None):
            return
        watch = self.text_read = Verification()

        def current(): return self.text_read is watch and same_pair()

        def work():
            try:
                if in_folder: return compare_folder_text(left, report, row, observer=watch), None
                if in_plain: return compare_plain_text(report, row, observer=watch), None
                return compare_text(left, right, row, observer=watch), None
            except Exception as error: return None, error

        def done(outcome):
            if not current(): return
            self.text_read = None
            value, error = outcome
            if watch.cancelled:
                self.owner.inform("已取消这次逐行比较，没有显示部分差异。")
            elif error is not None:
                self.owner.inform(f"没有显示逐行差异：{error}")
            else:
                self.show_lines(detail, value)
                self.owner.inform("已核验这两个原件并显示纯文本差异，未修改文件或启动外部应用。")

        try:
            future = self.owner.job("正在只读核验这两个原件…", work, done, cancel=watch.cancel,
                                    cancel_text="取消逐行比较",
                                    progress_report=lambda: watch.progress() if current() else ("", 0, None))
            if future is None: raise Rejected("另一个窗口任务尚未结束；未开始逐行核验")
        except Exception as error: done((None, error))

    def show_lines(self, detail, value):
        out = line_diff_dialog(self, detail, value)

        def finished(_):
            if self.text_dialog is out: self.text_dialog = None

        out.finished.connect(finished)
        self.text_dialog = out
        out.open()

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
            self.report = PlainComparison(report.added, report.changed, report.missing, report.rows, chosen, plain)
            self.summary.setText(pair + f"\n仅普通 ZIP {report.added} · 不同 {report.changed} · 仅快照 {report.missing}。"
                                 + ("\n选中同路径变化的两个小文件，可另行确认并查看逐行差异。" if report.rows else
                                    "\n树内容没有差异；此处未比较两个归档的原始字节。"))
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
