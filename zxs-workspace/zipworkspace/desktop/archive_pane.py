"""Real archive navigation, selected store, branch head and historical node actions."""
from pathlib import Path
import sys
from threading import Event

from PySide6.QtWidgets import QFileDialog, QHBoxLayout, QTabWidget, QVBoxLayout, QWidget

from .comparison_pane import ComparisonPane
from .checkout_pane import CheckoutPane
from .selection import Selection, StoreSelection
from .widgets import Table, button, confirm, label


class ArchivePane(QWidget):
    FILE_PAGE = 100

    def __init__(self, owner):
        super().__init__()
        self.owner = owner
        self.selection = self.store = self.catalog = self.file_page = None
        self.directory, self.page_number, self.search_dialog = "", 0, None
        self.text_dialog = None
        layout = QVBoxLayout(self)
        actions = QHBoxLayout()
        actions.addWidget(button("打开 .zxs 快照…", self.choose))
        actions.addWidget(button("选择归档存储目录…", self.choose_store))
        self.reload = button("读取分支与历史", self.load_catalog)
        actions.addWidget(self.reload)
        self.save_copy = self.save_zip = None
        if sys.platform == "linux":
            self.save_zip = button("另存完整快照为 ZIP…", self.choose_zip)
            actions.addWidget(self.save_zip)
        if sys.platform == "darwin":
            self.save_copy = button("另存同名完整归档…", self.choose_copy)
            actions.addWidget(self.save_copy)
        layout.addLayout(actions)
        self.details = label()
        self.store_label = label("尚未选择归档存储；浏览一个快照不需要存储目录。")
        layout.addWidget(self.details)
        layout.addWidget(self.store_label)
        self.views = QTabWidget()
        layout.addWidget(self.views)
        self.folder = QWidget()
        fl = QVBoxLayout(self.folder)
        self.at = label()
        fl.addWidget(self.at)
        self.find = button("在本快照内按名称查找…", self.open_search)
        fl.addWidget(self.find)
        self.files = Table(("名称", "类型", "字节数", "SHA-256"), "快照里的直接子项")
        self.files.doubleClicked.connect(lambda _index: self.enter())
        fl.addWidget(self.files)
        self.preview_file = button("预览选中的纯文本…", self.preview_selected)
        fl.addWidget(self.preview_file)
        self.files.selectionModel().selectionChanged.connect(self.update_member_action)
        self.save_member_action = None
        if sys.platform == "linux":
            self.save_member_action = button("另存选中的文件…", self.choose_member)
            self.save_member_action.setObjectName("save-selected-member")
            fl.addWidget(self.save_member_action)
        nav = QHBoxLayout()
        self.up = button("上一级", self.go_up)
        self.into = button("打开选中的文件夹", self.enter)
        self.back = button("上一页", self.previous)
        self.more = button("下一页", self.next)
        for item in (self.up, self.into, self.back, self.more):
            nav.addWidget(item)
        fl.addLayout(nav)
        self.views.addTab(self.folder, "文件")
        self.history = self.make_table(("节点 ID", "分支", "代", "父节点"), "历史", "查看选中的历史快照")
        self.copy_history = None
        if sys.platform == "linux":
            self.copy_history = button("将当前版本及祖先复制到新本机库…", self.choose_history_copy)
            self.history.parentWidget().layout().addWidget(self.copy_history)
        self.branches = self.make_table(("分支", "最高观察代", "已核验分支头", "缺失父节点"), "分支", "查看选中的分支头")
        self.comparison = ComparisonPane(owner)
        self.views.addTab(self.comparison, "只读对比")
        self.checkout = CheckoutPane(owner, lambda: self.store)
        self.views.addTab(self.checkout, "工作树")
        self.mount = None
        if sys.platform == "linux":
            from .mount_pane import MountPane
            self.mount = MountPane(owner)
            self.views.addTab(self.mount, "只读虚拟盘")
        self.clear_document()

    def make_table(self, headers, title, action):
        pane = QWidget()
        layout = QVBoxLayout(pane)
        table = Table(headers, title)
        layout.addWidget(table)
        actions = QHBoxLayout()
        actions.addWidget(button(action, lambda: self.open_row(table)))
        compare = button("与当前快照比较所选版本", lambda: self.compare_row(table))
        compare.setObjectName("compare-selected-revision")
        actions.addWidget(compare)
        layout.addLayout(actions)
        table.doubleClicked.connect(lambda _index: self.open_row(table))
        self.views.addTab(pane, title)
        return table

    def clear_catalog(self):
        self.comparison.invalidate_versions()
        self.catalog = None
        self.history.rows.replace(())
        self.branches.rows.replace(())
        if self.copy_history is not None: self.copy_history.setEnabled(False)

    def clear_document(self):
        self.close_search()
        self.close_text()
        if self.mount is not None: self.mount.set_snapshot(None)
        self.checkout.set_snapshot(None)  # Revoke the old branch source before releasing its display.
        if self.selection is not None: self.selection.release_display()
        self.selection = self.file_page = None
        self.directory, self.page_number = "", 0
        if self.save_copy is not None: self.save_copy.setEnabled(False)
        if self.save_zip is not None: self.save_zip.setEnabled(False)
        self.comparison.set_snapshot(None)
        self.clear_catalog()
        self.details.setText("尚未打开快照")
        self.at.setText("")
        self.files.rows.replace(())
        self.update_member_action()
        for widget in (self.reload, self.up, self.into, self.back, self.more, self.find):
            widget.setEnabled(False)

    def close_snapshot(self):
        if self.owner.pending is not None:
            self.owner.inform("原操作仍未返回；请等待后再关闭当前浏览快照。")
            return
        if self.selection is None: return
        self.clear_document()
        self.owner.inform("已关闭浏览快照与原浏览句柄；未关闭本窗口工作树，也未卸载已挂好的只读盘。")

    def choose(self):
        if self.owner.pending is not None: return
        if self.mount is not None: self.mount.changing()
        name, _ = QFileDialog.getOpenFileName(self, "选择要核验的目录快照", "", "目录快照 (*.zxs *.ZXS);;所有文件 (*)")
        if not name: return
        self.clear_document()
        self.views.setCurrentIndex(0)
        self.owner.job("正在核验所选快照中的全部成员…", lambda: Selection.open(name, retain=True), self.show_selection)

    def update_member_action(self, *_):
        entry = self.files.item() if self.file_page is not None else None
        self.preview_file.setEnabled(entry is not None and entry.kind == "file" and
                                     self.selection is not None and self.selection.held is not None)
        if self.save_member_action is not None:
            self.save_member_action.setEnabled(entry is not None and entry.kind in ("directory", "file"))
            self.save_member_action.setText("另存选中的文件夹…" if entry is not None and entry.kind == "directory" else
                                            "另存选中的文件…")

    def close_text(self):
        out, self.text_dialog = self.text_dialog, None
        if out is not None: out.reject()

    def preview_selected(self):
        selected, view, entry = self.selection, self.file_page, self.files.item()
        if (selected is None or view is None or entry is None or entry.kind != "file" or
                self.owner.pending is not None or self.text_dialog is not None):
            return
        from . import text_preview
        from ..model import Rejected

        def current():
            return (selected is self.selection and view is self.file_page and entry is self.files.item() and
                    selected.held is not None and not selected.held.closed)

        name = f"{view.directory}/{entry.name}" if view.directory else entry.name
        size = entry.size_bytes
        if not current() or not confirm(self, "核验后查看纯文本",
                f"归档：{str(selected.path)!r}\n成员：{name!r}\n将从本次原件读取全部 {size:,} 字节。"
                + ("\n显示通过 UTF-8 核验的全文。" if size <= text_preview.LIMIT else
                   "\n仅判断并显示通过核验的开头最多 256 KiB，后续不判定为文本。")
                + "\n不打开外部应用、不写文件；可取消。",
                attestation="我确认只查看所选成员，不启动任何程序。", action_text="核验后预览") or not current():
            return
        stopped = Event()

        def cancel():
            stopped.set()
            return True

        def check():
            if stopped.is_set(): raise Rejected("已取消本次预览，未显示或保存正文。")

        def read():
            try: return text_preview.prepare(selected, view.directory, entry, check=check), None
            except Exception as error: return None, error

        def apply(result):
            preview, error = result
            if stopped.is_set():
                self.owner.inform("已取消本次预览，未显示或保存正文。")
            elif not current():
                self.owner.inform("选择已变更，没有显示上一个成员的预览。")
            elif error is not None:
                self.owner.inform(str(error))
            else:
                out = text_preview.dialog(self, preview)
                self.text_dialog = out
                def finished(_result):
                    if self.text_dialog is out: self.text_dialog = None
                out.finished.connect(finished)
                out.open()
                self.owner.inform("本次实读内容与原成员的 SHA-256 相符；未确认来源，没有交给外部应用。")

        self.owner.job(f"正在核验所选成员的全部 {size:,} 字节…",
                       read, apply, cancel=cancel, cancel_text="取消本次文本预览")

    def choose_member(self):
        selected, page, entry = self.selection, self.file_page, self.files.item()
        if (self.save_member_action is None or self.owner.pending is not None or selected is None or
                page is None or entry is None or entry.kind not in ("directory", "file")):
            return
        directory = entry.kind == "directory"

        def still_selected():
            return selected is self.selection and page is self.file_page and entry is self.files.item()

        # The widget picker only supplies the chosen local string. It does not
        # create/reserve a file, mutate a folder, or delegate to a portal writer.
        opts = (QFileDialog.Option.DontUseNativeDialog | QFileDialog.Option.ReadOnly |
                QFileDialog.Option.DontResolveSymlinks | QFileDialog.Option.DontConfirmOverwrite)
        prompt = "选择目的父目录和新文件夹名（不覆盖）" if directory else "选择现有目录和新文件名（不覆盖）"
        name, _ = QFileDialog.getSaveFileName(self, prompt, entry.name, "所有文件 (*)", options=opts)
        if not name or not still_selected(): return

        from .member_copy import DirectoryCopy, MemberCopy
        plan = None
        try:
            plan = (DirectoryCopy if directory else MemberCopy)(selected, page.directory, entry, name)
            if directory:
                scratch = QFileDialog.getExistingDirectory(self, "选择同一挂载上本人独用的 0700 暂存目录",
                                                           "", opts | QFileDialog.Option.ShowDirsOnly)
                if not scratch or not still_selected():
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
                requested = {"cancel": plan.cancel, "progress_report": plan.progress} if cancellable else {}
                future = self.owner.job(title, call, apply, failed=lambda _error: plan.close(), **requested)
                if future is None: plan.close()
                else: future.add_done_callback(lambda f: plan.close() if f.cancelled() else None)
            except BaseException:
                plan.close()
                raise

        def review(detail):
            try:
                label = "文件夹" if directory else "文件"
                terms = "我选择这个新位置，并会保持原归档和目的目录在操作期间不被移动或改写。"
                if directory:
                    terms += "暂存目录由本人独占，不同步，无其他程序改动；同意在此留下失败材料。"
                if still_selected() and confirm(self, f"另存这一{label}，不覆盖", detail,
                        attestation=terms, action_text="另存并核验") and still_selected():
                    title = "正在只写私有暂存并完整核验所选子目录…" if directory else "正在复核原成员并写入无名称副本…"
                    submit(title, lambda: plan.run(confirmed=True), self.owner.inform, cancellable=True)
                else:
                    plan.close()
                    self.owner.inform("没有写入副本。")
            except BaseException:
                plan.close()
                raise

        submit("正在核对原归档和选定目录；尚未写入…", plan.review, review)

    def choose_zip(self):
        selected = self.selection
        if self.save_zip is None or selected is None or self.owner.pending is not None: return
        from .member_copy import ZipCopy
        opts = (QFileDialog.Option.DontUseNativeDialog | QFileDialog.Option.ReadOnly |
                QFileDialog.Option.DontResolveSymlinks | QFileDialog.Option.DontConfirmOverwrite)
        name, _ = QFileDialog.getSaveFileName(self, "选择现有本机目录和全新 .zip 名称",
                                               selected.path.with_suffix(".zip").name, "ZIP 文件 (*.zip)", options=opts)
        if not name or selected is not self.selection: return
        try: plan = ZipCopy(selected, name)
        except Exception as error:
            self.owner.inform(str(error))
            return

        def submit(title, work, done, *, cancellable=False):
            try:
                requested = {"cancel": plan.cancel, "progress_report": plan.progress} if cancellable else {}
                future = self.owner.job(title, work, done, failed=lambda _error: plan.close(), **requested)
                if future is None: plan.close()
                else: future.add_done_callback(lambda task: plan.close() if task.cancelled() else None)
            except BaseException:
                plan.close()
                raise

        def reviewed(details):
            try:
                if (selected is self.selection and confirm(self, "另存完整原包，不解压", details,
                        attestation="我选择这个全新位置，原归档和目的目录在操作期间不被移动或改写。",
                        action_text="另存 ZIP") and selected is self.selection):
                    submit("正在逐字节复制原包、核验并读回…", lambda: plan.run(confirmed=True), self.owner.inform,
                           cancellable=True)
                else:
                    plan.close()
                    self.owner.inform("没有写入 ZIP 副本。")
            except BaseException:
                plan.close()
                raise

        submit("正在只读核对原归档和目的位置，尚未写入…", plan.review, reviewed)

    def choose_copy(self):
        selected = self.selection
        if sys.platform != "darwin" or selected is None or self.owner.pending is not None: return
        from ..macos_copy import SnapshotCopy
        opts = QFileDialog.Option.ShowDirsOnly | QFileDialog.Option.DontResolveSymlinks
        directory = QFileDialog.getExistingDirectory(self, "选择同一 APFS 卷上仅本人使用的私有目的目录", "", opts)
        if not directory or selected is not self.selection: return
        try: plan = SnapshotCopy(selected, directory)
        except Exception as error:
            self.owner.inform(str(error))
            return

        def submit(title, call, success):
            try:
                job = self.owner.job(title, call, success, failed=lambda error: plan.close())
                if job is None: plan.close()
                else: job.add_done_callback(lambda f: plan.close() if f.cancelled() else None)
            except BaseException:
                plan.close()
                raise

        def review(detail):
            try:
                if selected is self.selection and confirm(self, "另存整个快照，不签出", detail,
                        attestation="我同意将整个原件和当时的元数据克隆到这个私有目录；原包和目的目录在操作期间无其他写者或同步。",
                        action_text="按原名另存") and selected is self.selection:
                    submit("正在复制和读回完整归档，完成前不要移动目录…", lambda: plan.run(confirmed=True), self.owner.inform)
                else:
                    plan.close()
                    self.owner.inform("没有写入副本。")
            except BaseException:
                plan.close()
                raise

        submit("正在只读核验归档来源、目标目录和完整字节…", plan.review, review)

    def choose_history_copy(self):
        selected, store, catalog = self.selection, self.store, self.catalog
        if (self.copy_history is None or selected is None or store is None or catalog is None or
                self.owner.pending is not None): return
        from .member_copy import HistoryCopy
        def current():
            return selected is self.selection and store is self.store and catalog is self.catalog

        opts = (QFileDialog.Option.DontUseNativeDialog | QFileDialog.Option.ReadOnly |
                QFileDialog.Option.DontResolveSymlinks | QFileDialog.Option.DontConfirmOverwrite)
        name, _ = QFileDialog.getSaveFileName(self, "指定新本机 STORE 的父目录和空缺新名称",
                                               "local-store", "所有文件 (*)", options=opts)
        if not name or not current(): return
        plan = None
        try:
            plan = HistoryCopy(selected, store, catalog, name)
            scratch = QFileDialog.getExistingDirectory(self, "选择本机同挂载、本人独用的已有 0700 暂存目录",
                                                       "", opts | QFileDialog.Option.ShowDirsOnly)
            if not scratch or not current():
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
                           "cancel_text": "取消本机历史复制（发布前）"} if cancellable else {}
                future = self.owner.job(title, call, apply, failed=lambda _error: plan.close(), **options)
                if future is None: plan.close()
                else: future.add_done_callback(lambda task: plan.close() if task.cancelled() else None)
            except BaseException:
                plan.close()
                raise

        def completed(message):
            self.owner.inform(f"{message}\n可另行选这个新 STORE，再打开其中的精确末代快照："
                              f"{str(plan.path / plan.tip.marker.filename)!r}。未改当前 STORE 或工作树。")

        def review(details):
            try:
                if (current() and confirm(self, "建立新本机库，不改变原 STORE", details,
                        attestation="原版本和两个本机父不移动、不改写。本机两处不属于原共享的服务器目录、别名或同步位置；本人独用私处，并同意留下失败材料。",
                        action_text="复制完整祖先到本机") and current()):
                    submit("正在从原位置逐份复制、核验原始字节并读回新本机库…",
                           lambda: plan.run(confirmed=True), completed, cancellable=True)
                else:
                    plan.close()
                    self.owner.inform("没有创建本机库或修改原 STORE。")
            except BaseException:
                plan.close()
                raise

        submit("正在核对原 STORE 与所选目录；尚未写入…", plan.review, review)

    def choose_store(self):
        name = QFileDialog.getExistingDirectory(self, "选择已有的归档存储目录")
        if name: self.choose_store_path(Path(name))

    def choose_store_path(self, store):
        self.store = None
        self.clear_catalog()
        self.reload.setEnabled(False)
        self.store_label.setText(f"等待核验存储目录：{store}")
        self.owner.job("正在核验选择的归档存储目录…", lambda: StoreSelection.open(store), self.use_store)

    def use_store(self, store):
        if store is not self.store: self.clear_catalog()
        self.store = store
        self.store_label.setText(f"归档存储：{store.path}")
        self.reload.setEnabled(self.selection is not None)
        self.owner.inform("已登记本次选择的存储目录。读取分支与历史时将复核原目录。")

    def show_selection(self, selected):
        self.close_search()
        self.close_text()
        if self.mount is not None: self.mount.set_snapshot(selected)
        self.checkout.set_snapshot(selected)
        if self.selection is not None and self.selection is not selected: self.selection.release_display()
        self.selection = selected
        if self.save_copy is not None: self.save_copy.setEnabled(True)
        if self.save_zip is not None: self.save_zip.setEnabled(selected.held is not None)
        self.comparison.set_snapshot(selected)
        marker = selected.browser.commit
        evidence = (f"原归档 SHA-256：{selected.exact.sha256}" if selected.exact is not None
                    else "成员内容已完整核验；未计算原 ZIP 字节摘要。")
        self.details.setText(f"{selected.path}\n仓库 {marker.repo}  ·  分支 {marker.branch}  ·  第 {marker.generation} 代\n"
                             f"节点 {marker.operation}  ·  整树 SHA-256：{marker.tree}\n{evidence}")
        self.reload.setEnabled(self.store is not None)
        self.find.setEnabled(True)
        self.views.setCurrentWidget(self.folder)
        self.show_folder("")
        if self.catalog is not None: self.show_history()
        else: self.owner.inform("已核验完整快照。双击目录进入。")

    def load_catalog(self):
        if not self.selection or not self.store: return
        chosen, store = self.selection, self.store
        self.clear_catalog()
        def done(catalog):
            if self.selection is chosen and self.store is store: self.show_catalog(catalog)
        self.owner.job("正在核验存储上的归档和原始父节点…", lambda: chosen.catalog(store), done)

    def show_catalog(self, catalog):
        if self.catalog is not catalog: self.comparison.invalidate_versions()
        self.catalog = catalog
        self.branches.rows.replace(((r["branch"], str(r["observed_generation"]),
            r["complete_observed_head"] or "不可选择", r["missing_parent"] or "无"),
            catalog.select_node(r["complete_observed_head"]) if r["complete_observed_head"] else None)
            for r in catalog.branches())
        self.show_history()
        self.views.setCurrentIndex(1)

    def show_history(self):
        chain, missing = self.catalog.history(self.selection.browser.commit.operation)
        self.history.rows.replace(((m.operation, m.branch, str(m.generation), m.parent or "起点"),
                                   self.catalog.select_node(m.operation)) for m in chain)
        if self.copy_history is not None:
            tip = self.catalog.nodes.get(self.selection.browser.commit.operation)
            self.copy_history.setEnabled(missing is None and self.selection.held is not None and
                tip is not None and str(tip.path) == str(self.selection.path) and tip.observed == self.selection.observed)
        self.owner.inform("历史不完整。缺少原始父节点：" + missing if missing else "已沿原始父节点读到仓库起点。")

    def open_row(self, table):
        node = table.item()
        if node is None:
            self.owner.inform("请选择有原始证明的节点；缺少祖先的分支头不可选。")
            return
        if self.mount is not None: self.mount.changing()
        store = self.store
        def read_original():
            store.check()
            original = Selection.open(node.path, expected=node, retain=True)
            try:
                store.check()
                return original
            except BaseException:
                original.release_display()
                raise
        self.owner.job("正在复核所选原节点的归档字节…", read_original, self.show_selection)

    def compare_row(self, table):
        node, catalog, store, left = table.item(), self.catalog, self.store, self.selection
        if (node is None or catalog is None or store is None or left is None or
                catalog.nodes.get(node.marker.operation) is not node or
                node.marker.operation == left.browser.commit.operation):
            self.owner.inform("请选择本次存储中的另一已列版本。当前版本、散包和无已核头的分支不能作此对照。")
            return
        if self.comparison.compare_version(node, store, catalog):
            self.views.setCurrentWidget(self.comparison)

    def close_search(self):
        dialog, self.search_dialog = self.search_dialog, None
        if dialog is not None:
            dialog.retire()
            dialog.reject()

    def open_search(self):
        selected = self.selection
        if selected is None or self.search_dialog is not None or self.owner.pending is not None: return
        from .name_search import NameSearch

        def current(dialog):
            return self.search_dialog is dialog and self.selection is selected

        def go_to(dialog, hit):
            if not current(dialog): return None
            from ..model import Rejected
            browser = selected.browser
            number, row = browser.locate_name(hit, limit=self.FILE_PAGE)
            page = browser.page_at(hit.parent, number, limit=self.FILE_PAGE)
            if row >= len(page.entries) or page.entries[row] is not hit.entry:
                raise Rejected("原结果不在该目录页")
            self.views.setCurrentWidget(self.folder)
            self.show_page(page, number)
            if not current(dialog) or self.file_page is not page: return None
            self.files.selectRow(row)
            self.files.scrollTo(self.files.rows.index(row, 0))
            return self.files

        dialog = NameSearch(self.owner, selected.browser, current, go_to)
        self.search_dialog = dialog

        def dismissed(_):
            if self.search_dialog is dialog: self.search_dialog = None
            dialog.deleteLater()

        dialog.finished.connect(dismissed)
        dialog.open()

    def show_folder(self, directory, number=0):
        if self.selection is None: return
        self.show_page(self.selection.browser.page_at(directory, number, limit=self.FILE_PAGE), number)

    def show_page(self, page, number):
        self.directory, self.page_number, self.file_page = page.directory, number, page
        self.at.setText(f"/{page.directory}  ·  第 {number + 1} 页" + ("" if page.entries else "  ·  空文件夹"))
        self.files.rows.replace(((entry.name, "目录" if entry.kind == "directory" else "文件",
            "" if entry.size_bytes is None else str(entry.size_bytes), entry.sha256 or ""), entry)
            for entry in page.entries)
        self.update_member_action()
        self.up.setEnabled(bool(self.directory))
        self.into.setEnabled(any(entry.kind == "directory" for entry in page.entries))
        self.back.setEnabled(number > 0)
        self.more.setEnabled(page.next_cursor is not None)

    def enter(self):
        entry = self.files.item()
        if entry is not None and entry.kind == "directory":
            self.show_folder(f"{self.directory}/{entry.name}" if self.directory else entry.name)

    def go_up(self):
        if self.directory: self.show_folder(self.directory.rpartition("/")[0])

    def previous(self):
        if self.page_number > 0:
            self.show_folder(self.directory, self.page_number - 1)

    def next(self):
        if self.file_page is not None and self.file_page.next_cursor is not None:
            self.show_folder(self.directory, self.page_number + 1)
