"""Real archive navigation, selected store, branch head and historical node actions."""
from pathlib import Path
from PySide6.QtWidgets import QFileDialog, QHBoxLayout, QTabWidget, QVBoxLayout, QWidget

from .comparison_pane import ComparisonPane
from .selection import Selection, StoreSelection
from .widgets import Table, button, label


class ArchivePane(QWidget):
    def __init__(self, owner):
        super().__init__()
        self.owner = owner
        self.selection = self.store = self.catalog = None
        self.directory, self.pages, self.next_cursor = "", [None], None
        layout = QVBoxLayout(self)
        actions = QHBoxLayout()
        actions.addWidget(button("打开 .zxs 快照…", self.choose))
        actions.addWidget(button("选择归档存储目录…", self.choose_store))
        self.reload = button("读取分支与历史", self.load_catalog)
        actions.addWidget(self.reload)
        layout.addLayout(actions)
        self.details = label()
        self.store_label = label("尚未选择归档存储；浏览一个快照不需要存储目录。")
        layout.addWidget(self.details)
        layout.addWidget(self.store_label)
        self.views = QTabWidget()
        layout.addWidget(self.views)
        folder = QWidget()
        fl = QVBoxLayout(folder)
        self.at = label()
        fl.addWidget(self.at)
        self.files = Table(("名称", "类型", "字节数", "SHA-256"), "快照里的直接子项")
        self.files.doubleClicked.connect(lambda _index: self.enter())
        fl.addWidget(self.files)
        nav = QHBoxLayout()
        self.up = button("上一级", self.go_up)
        self.into = button("打开选中的文件夹", self.enter)
        self.back = button("上一页", self.previous)
        self.more = button("下一页", self.next)
        for item in (self.up, self.into, self.back, self.more):
            nav.addWidget(item)
        fl.addLayout(nav)
        self.views.addTab(folder, "文件")
        self.history = self.make_table(("节点 ID", "分支", "代", "父节点"), "历史", "查看选中的历史快照")
        self.branches = self.make_table(("分支", "最高观察代", "已核验分支头", "缺失父节点"), "分支", "查看选中的分支头")
        self.comparison = ComparisonPane(owner)
        self.views.addTab(self.comparison, "与本机文件夹比较")
        self.clear_document()

    def make_table(self, headers, title, action):
        pane = QWidget()
        layout = QVBoxLayout(pane)
        table = Table(headers, title)
        layout.addWidget(table)
        layout.addWidget(button(action, lambda: self.open_row(table)))
        table.doubleClicked.connect(lambda _index: self.open_row(table))
        self.views.addTab(pane, title)
        return table

    def clear_catalog(self):
        self.catalog = None
        self.history.rows.replace(())
        self.branches.rows.replace(())

    def clear_document(self):
        self.selection = self.next_cursor = None
        self.comparison.set_snapshot(None)
        self.clear_catalog()
        self.details.setText("尚未打开快照")
        self.at.setText("")
        self.files.rows.replace(())
        for widget in (self.reload, self.up, self.into, self.back, self.more):
            widget.setEnabled(False)

    def choose(self):
        name, _ = QFileDialog.getOpenFileName(self, "选择要核验的目录快照", "", "目录快照 (*.zxs *.ZXS);;所有文件 (*)")
        if not name: return
        self.clear_document()
        self.views.setCurrentIndex(0)
        self.owner.job("正在核验所选快照中的全部成员…", lambda: Selection.open(name), self.show_selection)

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
        self.store = store
        self.store_label.setText(f"归档存储：{store.path}")
        self.reload.setEnabled(self.selection is not None)
        self.owner.inform("已登记本次选择的存储目录。读取分支与历史时将复核原目录。")

    def show_selection(self, selected):
        self.selection = selected
        self.comparison.set_snapshot(selected)
        marker = selected.browser.commit
        evidence = (f"原归档 SHA-256：{selected.exact.sha256}" if selected.exact is not None
                    else "成员内容已完整核验；未计算原 ZIP 字节摘要。")
        self.details.setText(f"{selected.path}\n仓库 {marker.repo}  ·  分支 {marker.branch}  ·  第 {marker.generation} 代\n"
                             f"节点 {marker.operation}  ·  整树 SHA-256：{marker.tree}\n{evidence}")
        self.reload.setEnabled(self.store is not None)
        self.views.setCurrentIndex(0)
        self.show_folder("")
        if self.catalog is not None: self.show_history()
        else: self.owner.inform("已核验完整快照。双击目录进入。")

    def load_catalog(self):
        if not self.selection or not self.store: return
        chosen, store = self.selection, self.store
        self.clear_catalog()
        self.owner.job("正在核验存储上的归档和原始父节点…", lambda: chosen.catalog(store), self.show_catalog)

    def show_catalog(self, catalog):
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
        self.owner.inform("历史不完整。缺少原始父节点：" + missing if missing else "已沿原始父节点读到仓库起点。")

    def open_row(self, table):
        node = table.item()
        if node is None:
            self.owner.inform("请选择有原始证明的节点；缺少祖先的分支头不可选。")
            return
        store = self.store
        def read_original():
            store.check()
            original = Selection.open(node.path, expected=node)
            store.check()
            return original
        self.owner.job("正在复核所选原节点的归档字节…", read_original, self.show_selection)

    def show_folder(self, directory):
        self.directory, self.pages = directory, [None]
        self.show_page()

    def show_page(self):
        if self.selection is None: return
        page = self.selection.browser.list_directory(self.directory, cursor=self.pages[-1])
        self.next_cursor = page.next_cursor
        self.at.setText(f"/{page.directory}  ·  第 {len(self.pages)} 页" + ("" if page.entries else "  ·  空文件夹"))
        self.files.rows.replace(((entry.name, "目录" if entry.kind == "directory" else "文件",
            "" if entry.size_bytes is None else str(entry.size_bytes), entry.sha256 or ""), entry)
            for entry in page.entries)
        self.up.setEnabled(bool(self.directory))
        self.into.setEnabled(any(entry.kind == "directory" for entry in page.entries))
        self.back.setEnabled(len(self.pages) > 1)
        self.more.setEnabled(self.next_cursor is not None)

    def enter(self):
        entry = self.files.item()
        if entry is not None and entry.kind == "directory":
            self.show_folder(f"{self.directory}/{entry.name}" if self.directory else entry.name)

    def go_up(self):
        if self.directory: self.show_folder(self.directory.rpartition("/")[0])

    def previous(self):
        if len(self.pages) > 1:
            self.pages.pop()
            self.show_page()

    def next(self):
        if self.next_cursor is not None:
            self.pages.append(self.next_cursor)
            self.show_page()
